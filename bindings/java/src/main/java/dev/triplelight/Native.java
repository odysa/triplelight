package dev.triplelight;

import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;
import java.nio.file.Path;

import static java.lang.foreign.ValueLayout.*;

/** Downcall handles for the C ABI in bindings/c/include/triplelight.h. */
final class Native {
    private Native() {}

    static final int ABI_VERSION = 1;
    static final int UPDATE_KIND = 0, LOAD_KIND = 1;

    static final StructLayout STR = MemoryLayout.structLayout(JAVA_LONG.withName("off"), JAVA_LONG.withName("len"));
    static final StructLayout TERM = MemoryLayout.structLayout(
            JAVA_BYTE.withName("kind"), MemoryLayout.paddingLayout(7),
            STR.withName("value"), STR.withName("language"), STR.withName("datatype"));
    static final StructLayout VIEW = MemoryLayout.structLayout(
            JAVA_INT.withName("kind"), JAVA_INT.withName("boolean"),
            JAVA_LONG.withName("n_vars"), ADDRESS.withName("vars"),
            JAVA_LONG.withName("n_rows"), ADDRESS.withName("cells"),
            JAVA_LONG.withName("n_terms"), ADDRESS.withName("terms"),
            JAVA_LONG.withName("n_bytes"), ADDRESS.withName("bytes"));

    static final long VIEW_KIND = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("kind"));
    static final long VIEW_BOOLEAN = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("boolean"));
    static final long VIEW_N_VARS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("n_vars"));
    static final long VIEW_VARS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("vars"));
    static final long VIEW_N_ROWS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("n_rows"));
    static final long VIEW_CELLS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("cells"));
    static final long VIEW_N_TERMS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("n_terms"));
    static final long VIEW_TERMS = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("terms"));
    static final long VIEW_N_BYTES = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("n_bytes"));
    static final long VIEW_BYTES = VIEW.byteOffset(MemoryLayout.PathElement.groupElement("bytes"));

    static final MethodHandle ABI, OPEN, OPEN_IN_MEMORY, CLOSE, QUERY, QUERY_ARROW, UPDATE, LOAD, COMPACT, STATS_READY,
            RESULT_VIEW, RESULT_FREE, ERROR_KIND, ERROR_MESSAGE, ERROR_FREE;

    static {
        String configured = System.getProperty("triplelight.library", System.getenv("TRIPLELIGHT_LIBRARY"));
        SymbolLookup lib = configured != null
                ? SymbolLookup.libraryLookup(Path.of(configured), Arena.global())
                : SymbolLookup.libraryLookup(System.mapLibraryName("triplelight_c"), Arena.global());
        Linker linker = Linker.nativeLinker();
        ABI = h(linker, lib, "tl_abi_version", FunctionDescriptor.of(JAVA_INT));
        OPEN = h(linker, lib, "tl_open", FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
        OPEN_IN_MEMORY = h(linker, lib, "tl_open_in_memory", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
        CLOSE = h(linker, lib, "tl_close", FunctionDescriptor.ofVoid(ADDRESS));
        QUERY = h(linker, lib, "tl_query", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
        QUERY_ARROW = h(linker, lib, "tl_query_arrow", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
        UPDATE = h(linker, lib, "tl_update", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
        LOAD = h(linker, lib, "tl_load", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
        COMPACT = h(linker, lib, "tl_compact", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
        STATS_READY = h(linker, lib, "tl_stats_ready", FunctionDescriptor.of(JAVA_INT, ADDRESS));
        RESULT_VIEW = h(linker, lib, "tl_result_get_view", FunctionDescriptor.of(ADDRESS.withTargetLayout(VIEW), ADDRESS));
        RESULT_FREE = h(linker, lib, "tl_result_free", FunctionDescriptor.ofVoid(ADDRESS));
        ERROR_KIND = h(linker, lib, "tl_error_kind", FunctionDescriptor.of(JAVA_INT, ADDRESS));
        ERROR_MESSAGE = h(linker, lib, "tl_error_message", FunctionDescriptor.of(ADDRESS, ADDRESS, ADDRESS));
        ERROR_FREE = h(linker, lib, "tl_error_free", FunctionDescriptor.ofVoid(ADDRESS));
        try {
            int v = (int) ABI.invokeExact();
            if (v != ABI_VERSION) throw new IllegalStateException("triplelight C ABI " + v + ", binding expects " + ABI_VERSION);
        } catch (RuntimeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ExceptionInInitializerError(t);
        }
    }

    private static MethodHandle h(Linker linker, SymbolLookup lib, String name, FunctionDescriptor d) {
        return linker.downcallHandle(lib.find(name).orElseThrow(() -> new UnsatisfiedLinkError(name)), d);
    }
}
