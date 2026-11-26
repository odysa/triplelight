package dev.triplelight;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.StructLayout;
import java.lang.invoke.MethodHandles;
import java.util.concurrent.ConcurrentHashMap;

import static java.lang.foreign.ValueLayout.*;

/** Native upcalls for registered factories and their per-group state. */
final class AggregateCallbacks {
    private record Owned<T>(Arena arena, MemorySegment address, T value) {
        static <T> Owned<T> create(T value) {
            Arena arena = Arena.ofShared();
            try {
                return new Owned<>(arena, arena.allocate(JAVA_BYTE), value);
            } catch (Throwable error) {
                arena.close();
                throw error;
            }
        }
    }

    private static final ConcurrentHashMap<Long, Owned<Database.AggregateFactory>> FACTORIES =
            new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<Long, Owned<Database.AggregateState>> GROUPS =
            new ConcurrentHashMap<>();

    // Match tl_aggregate_callbacks in the C header. Native registration copies the table.
    private static final StructLayout CALLBACKS = MemoryLayout.structLayout(
            ADDRESS.withName("create"), ADDRESS.withName("step"), ADDRESS.withName("finish"),
            ADDRESS.withName("drop_state"), ADDRESS.withName("release_context"));

    private static final MemorySegment CREATE = stub("create", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
    private static final MemorySegment STEP = stub("step", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
    private static final MemorySegment FINISH = stub("finish", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
    private static final MemorySegment DROP_STATE = stub("dropState", FunctionDescriptor.ofVoid(ADDRESS));
    private static final MemorySegment RELEASE = stub("releaseContext", FunctionDescriptor.ofVoid(ADDRESS));

    private AggregateCallbacks() {}

    private static MemorySegment stub(String name, FunctionDescriptor signature) {
        try {
            var target = MethodHandles.lookup().findStatic(AggregateCallbacks.class, name, signature.toMethodType());
            return Linker.nativeLinker().upcallStub(target, signature, Arena.global());
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    static void register(MemorySegment db, String iri, Database.AggregateFactory factory) throws Throwable {
        var context = Owned.create(factory);
        long address = context.address().address();
        boolean adopted = false;
        try (Arena caller = Arena.ofConfined()) {
            FACTORIES.put(address, context);
            MemorySegment callbacks = caller.allocate(CALLBACKS);
            set(callbacks, "create", CREATE);
            set(callbacks, "step", STEP);
            set(callbacks, "finish", FINISH);
            set(callbacks, "drop_state", DROP_STATE);
            set(callbacks, "release_context", RELEASE);
            byte[] bytes = ScalarCallbacks.utf8(iri);
            MemorySegment err = caller.allocate(ADDRESS);
            int status = (int) Native.REGISTER_AGGREGATE_FUNCTION.invokeExact(
                    db, caller.allocateFrom(JAVA_BYTE, bytes), (long) bytes.length, callbacks, context.address(), err);
            adopted = status == 0;
            Database.check(status, err);
        } finally {
            if (!adopted) {
                FACTORIES.remove(address);
                context.arena().close();
            }
        }
    }

    private static void set(MemorySegment callbacks, String field, MemorySegment stub) {
        callbacks.set(ADDRESS, CALLBACKS.byteOffset(MemoryLayout.PathElement.groupElement(field)), stub);
    }

    private static int create(MemorySegment context, MemorySegment output) {
        Owned<Database.AggregateState> group = null;
        try {
            Database.AggregateState state = FACTORIES.get(context.address()).value().create();
            if (state == null) return Native.TL_ERR_INTERNAL;
            group = Owned.create(state);
            GROUPS.put(group.address().address(), group);
            output.reinterpret(ADDRESS.byteSize()).set(ADDRESS, 0, group.address());
            return 0;
        } catch (Throwable error) {
            if (group != null) {
                try {
                    GROUPS.remove(group.address().address());
                    group.arena().close();
                } catch (Throwable ignored) {
                    // Never unwind through an upcall.
                }
            }
            return Native.TL_ERR_INTERNAL;
        }
    }

    private static int step(MemorySegment state, MemorySegment arg) {
        try {
            GROUPS.get(state.address()).value().step(ScalarCallbacks.decodeArgs(arg)[0]);
            return 0;
        } catch (Throwable error) {
            return Native.TL_ERR_INTERNAL;
        }
    }

    private static int finish(MemorySegment state, MemorySegment destination) {
        try {
            return ScalarCallbacks.writeTerm(destination, GROUPS.get(state.address()).value().finish());
        } catch (Throwable error) {
            return Native.TL_ERR_INTERNAL;
        }
    }

    private static void dropState(MemorySegment state) {
        release(GROUPS, state);
    }

    private static void releaseContext(MemorySegment context) {
        release(FACTORIES, context);
    }

    private static void release(ConcurrentHashMap<Long, ? extends Owned<?>> objects, MemorySegment pointer) {
        try {
            Owned<?> owned = objects.remove(pointer.address());
            if (owned != null) owned.arena().close();
        } catch (Throwable ignored) {
            // Native cleanup has no error return.
        }
    }
}
