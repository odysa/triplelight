package dev.triplelight;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ConcurrentHashMap;

import static java.lang.foreign.ValueLayout.*;

/** Native scalar upcalls share two stubs. Each registration owns a unique native context address. */
final class ScalarCallbacks {
    private static final int EXPRESSION_ERROR = 6; // TL_ERR_INTERNAL at the callback boundary
    private record Registration(Arena arena, Database.ScalarFunction function) {}
    private static final ConcurrentHashMap<Long, Registration> REGISTRATIONS = new ConcurrentHashMap<>();
    private static final MemorySegment INVOKE;
    private static final MemorySegment RELEASE;

    static {
        try {
            Linker linker = Linker.nativeLinker();
            MethodHandles.Lookup lookup = MethodHandles.lookup();
            var invoke = lookup.findStatic(ScalarCallbacks.class, "invoke",
                    MethodType.methodType(int.class, MemorySegment.class, MemorySegment.class, MemorySegment.class));
            var release = lookup.findStatic(ScalarCallbacks.class, "release",
                    MethodType.methodType(void.class, MemorySegment.class));
            // A release upcall can close the registration's arena without freeing its own running stub.
            INVOKE = linker.upcallStub(invoke, FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS), Arena.global());
            RELEASE = linker.upcallStub(release, FunctionDescriptor.ofVoid(ADDRESS), Arena.global());
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private ScalarCallbacks() {}

    static void register(MemorySegment db, String iri, long arity, boolean pure, Database.ScalarFunction function)
            throws Throwable {
        Arena shared = Arena.ofShared();
        long address = 0;
        boolean adopted = false;
        try (Arena caller = Arena.ofConfined()) {
            MemorySegment context = shared.allocate(JAVA_BYTE);
            address = context.address();
            REGISTRATIONS.put(address, new Registration(shared, function));
            byte[] iriBytes = utf8(iri);
            MemorySegment err = caller.allocate(ADDRESS);
            int result = (int) Native.REGISTER_FUNCTION.invokeExact(db, caller.allocateFrom(JAVA_BYTE, iriBytes),
                    (long) iriBytes.length, arity, pure ? 1 : 0, INVOKE, context, RELEASE, err);
            adopted = result == 0;
            Database.check(result, err);
        } finally {
            if (!adopted) {
                REGISTRATIONS.remove(address);
                shared.close();
            }
        }
    }

    private static int invoke(MemorySegment context, MemorySegment args, MemorySegment destination) {
        try (Arena caller = Arena.ofConfined()) {
            // tl_scalar_args: size_t len, cells pointer, size_t n_terms, terms pointer, size_t n_bytes, bytes pointer.
            MemorySegment view = args.reinterpret(48);
            int count = Math.toIntExact(view.get(JAVA_LONG, 0));
            int nTerms = Math.toIntExact(view.get(JAVA_LONG, 16));
            long nBytes = view.get(JAVA_LONG, 32);
            byte[] bytes = nBytes == 0 ? new byte[0] : view.get(ADDRESS, 40).reinterpret(nBytes).toArray(JAVA_BYTE);
            Term[] terms = Database.decodeTerms(view.get(ADDRESS, 24), nTerms, bytes);
            Term[] arguments = new Term[count];
            if (count != 0) {
                MemorySegment cells = view.get(ADDRESS, 8).reinterpret((long) count * Integer.BYTES);
                for (int i = 0; i < count; i++) {
                    arguments[i] = terms[cells.getAtIndex(JAVA_INT, i) - 1];
                }
            }
            Term result = REGISTRATIONS.get(context.address()).function().apply(arguments);
            if (result == null) return EXPRESSION_ERROR;
            StringBuilder term = new StringBuilder();
            appendTerm(term, result, 0);
            byte[] returned = utf8(term);
            return (int) Native.SCALAR_SET_TERM.invokeExact(destination, caller.allocateFrom(JAVA_BYTE, returned),
                    (long) returned.length);
        } catch (Throwable error) {
            // Java exceptions must never escape a native upcall.
            return EXPRESSION_ERROR;
        }
    }

    private static void release(MemorySegment context) {
        try {
            Registration registration = REGISTRATIONS.remove(context.address());
            if (registration != null) registration.arena().close();
        } catch (Throwable ignored) {
            // The C release callback has no error return. Never unwind through the upcall.
        }
    }

    private static byte[] utf8(CharSequence text) throws java.nio.charset.CharacterCodingException {
        ByteBuffer encoded = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(text));
        byte[] bytes = new byte[encoded.remaining()];
        encoded.get(bytes);
        return bytes;
    }

    private static void appendTerm(StringBuilder out, Term term, int depth) {
        if (depth > 63) throw new IllegalArgumentException("RDF triple term is too deep");
        switch (term) {
            case Term.Iri iri -> appendIri(out, iri.value());
            case Term.BlankNode blank -> {
                String label = blank.value();
                if (label.isEmpty() || label.codePoints().anyMatch(c -> c <= ' ' || "<>()\"\\".indexOf(c) >= 0)) {
                    throw new IllegalArgumentException("invalid blank node label");
                }
                out.append("_:").append(label);
            }
            case Term.Literal literal -> appendLiteral(out, literal);
            case Term.Triple triple -> {
                if (!(triple.predicate() instanceof Term.Iri predicate)) {
                    throw new IllegalArgumentException("triple term predicate must be an IRI");
                }
                out.append("<<( ");
                appendTerm(out, triple.subject(), depth + 1);
                out.append(' ');
                appendIri(out, predicate.value());
                out.append(' ');
                appendTerm(out, triple.object(), depth + 1);
                out.append(" )>>");
            }
        }
    }

    private static void appendIri(StringBuilder out, String iri) {
        if (iri.isEmpty() || iri.codePoints().anyMatch(c -> c <= ' ' || "<>\"{}|^`\\".indexOf(c) >= 0)) {
            throw new IllegalArgumentException("invalid IRI");
        }
        out.append('<').append(iri).append('>');
    }

    private static void appendLiteral(StringBuilder out, Term.Literal literal) {
        out.append('"');
        literal.value().codePoints().forEach(c -> {
            if (c == '"' || c == '\\') out.append('\\').appendCodePoint(c);
            else if (c < 0x20 || c == 0x7f || c == 0xfffe || c == 0xffff) out.append(String.format("\\u%04X", c));
            else out.appendCodePoint(c);
        });
        out.append('"');
        if (!literal.language().isEmpty()) {
            String datatype = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                    + (literal.direction().isEmpty() ? "langString" : "dirLangString");
            if (!literal.datatype().isEmpty() && !literal.datatype().equals(datatype)) {
                throw new IllegalArgumentException("incompatible language and datatype");
            }
            if (!literal.language().matches("[A-Za-z]+(?:-[A-Za-z0-9]+)*")) {
                throw new IllegalArgumentException("invalid language tag");
            }
            out.append('@').append(literal.language());
            switch (literal.direction()) {
                case "" -> { }
                case "ltr", "rtl" -> out.append("--").append(literal.direction());
                default -> throw new IllegalArgumentException("invalid base direction");
            }
        } else {
            if (!literal.direction().isEmpty()) throw new IllegalArgumentException("base direction requires a language");
            if (!literal.datatype().isEmpty()) {
                out.append("^^");
                appendIri(out, literal.datatype());
            }
        }
    }
}
