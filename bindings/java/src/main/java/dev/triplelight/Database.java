package dev.triplelight;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.locks.ReentrantReadWriteLock;

import static java.lang.foreign.ValueLayout.*;

/**
 * An embedded TripleLight database, persistent ({@link #open}) or in-memory ({@link #inMemory}).
 *
 * <p>Thread-safe: queries and updates may run concurrently. {@link #close} waits for calls in flight and later calls
 * throw {@link TripleLightException.Kind#CLOSED}. Results are plain Java objects, independent of the database.
 * Requires {@code --enable-native-access} for this module (or ALL-UNNAMED) on Java 25.
 */
public final class Database implements AutoCloseable {
    private final ReentrantReadWriteLock lock = new ReentrantReadWriteLock();
    private MemorySegment handle;

    private Database(MemorySegment handle) { this.handle = handle; }

    /** Opens (or creates) a persistent database in {@code dir}. */
    public static Database open(Path dir) {
        try (Arena a = Arena.ofConfined()) {
            byte[] p = dir.toString().getBytes(StandardCharsets.UTF_8);
            MemorySegment out = a.allocate(ADDRESS), err = a.allocate(ADDRESS);
            check((int) Native.OPEN.invokeExact(a.allocateFrom(JAVA_BYTE, p), (long) p.length, out, err), err);
            return new Database(out.get(ADDRESS, 0));
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        }
    }

    /** Creates an isolated, non-durable in-memory database. */
    public static Database inMemory() {
        try (Arena a = Arena.ofConfined()) {
            MemorySegment out = a.allocate(ADDRESS), err = a.allocate(ADDRESS);
            check((int) Native.OPEN_IN_MEMORY.invokeExact(out, err), err);
            return new Database(out.get(ADDRESS, 0));
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        }
    }

    /** Runs a SPARQL SELECT, ASK or CONSTRUCT query. */
    public Result query(String sparql) {
        lock.readLock().lock();
        MemorySegment result = MemorySegment.NULL;
        try (Arena a = Arena.ofConfined()) {
            MemorySegment h = live();
            byte[] q = sparql.getBytes(StandardCharsets.UTF_8);
            MemorySegment out = a.allocate(ADDRESS), err = a.allocate(ADDRESS);
            check((int) Native.QUERY.invokeExact(h, a.allocateFrom(JAVA_BYTE, q), (long) q.length, out, err), err);
            result = out.get(ADDRESS, 0);
            return decode((MemorySegment) Native.RESULT_VIEW.invokeExact(result));
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            try { Native.RESULT_FREE.invokeExact(result); } catch (Throwable ignored) { }
            lock.readLock().unlock();
        }
    }

    /**
     * Runs a SPARQL SELECT and exports its solutions as an Arrow C Stream into the caller-owned
     * {@code struct ArrowArrayStream} at {@code streamAddress} (for example
     * {@code org.apache.arrow.c.ArrowArrayStream#memoryAddress()}). On success the caller owns the stream and must
     * release it; it stays valid after {@link #close}. On failure the struct is left untouched. Each variable is a
     * nullable struct column {@code {kind, value, datatype, language}}; a null struct is an unbound cell. ASK and
     * CONSTRUCT throw {@link TripleLightException.Kind#UNSUPPORTED}.
     */
    public void queryArrow(String sparql, long streamAddress) {
        if (streamAddress == 0) throw new IllegalArgumentException("null stream address");
        lock.readLock().lock();
        try (Arena a = Arena.ofConfined()) {
            byte[] q = sparql.getBytes(StandardCharsets.UTF_8);
            MemorySegment err = a.allocate(ADDRESS);
            check((int) Native.QUERY_ARROW.invokeExact(live(), a.allocateFrom(JAVA_BYTE, q), (long) q.length,
                    MemorySegment.ofAddress(streamAddress), err), err);
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            lock.readLock().unlock();
        }
    }

    /** Applies a SPARQL Update; returns the number of triples inserted or deleted. */
    public long update(String sparql) { return counted(Native.UPDATE_KIND, sparql); }

    /** Loads an N-Triples ({@code .nt}) or Turtle ({@code .ttl}) file; returns the number of triples added. */
    public long load(Path file) { return counted(Native.LOAD_KIND, file.toString()); }

    /** Rewrites the current state into a new base (persistent) or rebuilds the indexes (in-memory). */
    public void compact() {
        lock.readLock().lock();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment err = a.allocate(ADDRESS);
            check((int) Native.COMPACT.invokeExact(live(), err), err);
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            lock.readLock().unlock();
        }
    }

    /** Whether planner statistics for the current base are ready; queries may run either way. */
    public boolean statsReady() {
        lock.readLock().lock();
        try {
            return (int) Native.STATS_READY.invokeExact(live()) == 1;
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            lock.readLock().unlock();
        }
    }

    /** Closes the database and releases its directory lock. Idempotent. */
    @Override
    public void close() {
        lock.writeLock().lock();
        try {
            if (handle != null) {
                Native.CLOSE.invokeExact(handle);
                handle = null;
            }
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            lock.writeLock().unlock();
        }
    }

    private long counted(int which, String text) {
        lock.readLock().lock();
        try (Arena a = Arena.ofConfined()) {
            MemorySegment h = live();
            byte[] b = text.getBytes(StandardCharsets.UTF_8);
            MemorySegment n = a.allocate(JAVA_LONG), err = a.allocate(ADDRESS), s = a.allocateFrom(JAVA_BYTE, b);
            int code = which == Native.UPDATE_KIND
                    ? (int) Native.UPDATE.invokeExact(h, s, (long) b.length, n, err)
                    : (int) Native.LOAD.invokeExact(h, s, (long) b.length, n, err);
            check(code, err);
            return n.get(JAVA_LONG, 0);
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw internal(t);
        } finally {
            lock.readLock().unlock();
        }
    }

    private MemorySegment live() {
        if (handle == null) throw new TripleLightException(TripleLightException.Kind.CLOSED, "the database is closed");
        return handle;
    }

    /** Copies everything out of the native view: the arena once, each distinct term once, cells as one int[]. */
    private static Result decode(MemorySegment view) {
        MemorySegment v = view.reinterpret(Native.VIEW.byteSize());
        int kind = v.get(JAVA_INT, Native.VIEW_KIND);
        if (kind == 1) return new Result(Result.Type.BOOLEAN, List.of(), List.of(), v.get(JAVA_INT, Native.VIEW_BOOLEAN) != 0);
        int nVars = Math.toIntExact(v.get(JAVA_LONG, Native.VIEW_N_VARS));
        int nRows = Math.toIntExact(v.get(JAVA_LONG, Native.VIEW_N_ROWS));
        int nTerms = Math.toIntExact(v.get(JAVA_LONG, Native.VIEW_N_TERMS));
        long nBytes = v.get(JAVA_LONG, Native.VIEW_N_BYTES);
        byte[] bytes = nBytes == 0 ? new byte[0]
                : v.get(ADDRESS, Native.VIEW_BYTES).reinterpret(nBytes).toArray(JAVA_BYTE);
        List<String> vars;
        if (kind == 2) {
            vars = List.of();
        } else {
            MemorySegment vs = v.get(ADDRESS, Native.VIEW_VARS).reinterpret(Native.STR.byteSize() * nVars);
            String[] names = new String[nVars];
            for (int i = 0; i < nVars; i++) names[i] = str(bytes, vs, i * Native.STR.byteSize());
            vars = List.of(names);
        }
        Term[] terms = new Term[nTerms];
        if (nTerms > 0) {
            MemorySegment ts = v.get(ADDRESS, Native.VIEW_TERMS).reinterpret(Native.TERM.byteSize() * nTerms);
            String lastDatatype = null;
            for (int i = 0; i < nTerms; i++) {
                long base = i * Native.TERM.byteSize();
                String value = ts.get(JAVA_BYTE, base) == 4 ? "" : str(bytes, ts, base + 8);
                terms[i] = switch (ts.get(JAVA_BYTE, base)) {
                    case 1 -> new Term.Iri(value);
                    case 2 -> new Term.BlankNode(value);
                    // RDF 1.2 triple term: 1-based indices of its parts in value.off, value.len and language.off. A term's
                    // parts are always earlier in the table than the term, so they are decoded already.
                    case 4 -> new Term.Triple(terms[(int) ts.get(JAVA_LONG, base + 8) - 1], terms[(int) ts.get(JAVA_LONG, base + 16) - 1],
                            terms[(int) ts.get(JAVA_LONG, base + 24) - 1]);
                    default -> {
                        String language = str(bytes, ts, base + 24);
                        String datatype = str(bytes, ts, base + 40);
                        // Datatypes repeat; keep one String instance for equal consecutive values.
                        if (datatype.equals(lastDatatype)) datatype = lastDatatype; else lastDatatype = datatype;
                        byte dir = ts.get(JAVA_BYTE, base + 1); // TL_DIR_*: 0 none, 1 ltr, 2 rtl
                        yield new Term.Literal(value, language, datatype, dir == 1 ? "ltr" : dir == 2 ? "rtl" : "");
                    }
                };
            }
        }
        long nCells = (long) nRows * nVars;
        int[] cells = nCells == 0 ? new int[0]
                : v.get(ADDRESS, Native.VIEW_CELLS).reinterpret(nCells * 4).toArray(JAVA_INT);
        List<Term[]> rows = new ArrayList<>(nRows);
        for (int r = 0; r < nRows; r++) {
            Term[] row = new Term[nVars];
            for (int c = 0; c < nVars; c++) {
                int idx = cells[r * nVars + c];
                row[c] = idx == 0 ? null : terms[idx - 1];
            }
            rows.add(row);
        }
        return new Result(kind == 2 ? Result.Type.TRIPLES : Result.Type.BINDINGS, vars, rows, false);
    }

    private static String str(byte[] bytes, MemorySegment s, long at) {
        long off = s.get(JAVA_LONG, at), len = s.get(JAVA_LONG, at + 8);
        return len == 0 ? "" : new String(bytes, Math.toIntExact(off), Math.toIntExact(len), StandardCharsets.UTF_8);
    }

    private static void check(int code, MemorySegment errOut) throws Throwable {
        if (code == 0) return;
        MemorySegment e = errOut.get(ADDRESS, 0);
        String message = "";
        if (!e.equals(MemorySegment.NULL)) {
            try (Arena a = Arena.ofConfined()) {
                MemorySegment len = a.allocate(JAVA_LONG);
                MemorySegment p = (MemorySegment) Native.ERROR_MESSAGE.invokeExact(e, len);
                long n = len.get(JAVA_LONG, 0);
                message = n == 0 ? "" : new String(p.reinterpret(n).toArray(JAVA_BYTE), StandardCharsets.UTF_8);
            } finally {
                Native.ERROR_FREE.invokeExact(e);
            }
        }
        throw new TripleLightException(TripleLightException.fromCode(code), message);
    }

    private static TripleLightException internal(Throwable t) {
        return new TripleLightException(TripleLightException.Kind.INTERNAL, String.valueOf(t));
    }

    @Override
    public String toString() { return "Database" + (handle == null ? "(closed)" : ""); }
}
