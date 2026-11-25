package dev.triplelight;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.time.Duration;
import java.util.concurrent.locks.ReentrantReadWriteLock;

import static java.lang.foreign.ValueLayout.ADDRESS;

/**
 * Stops queries from outside them (#110): pass it to {@link Database#query(String, QueryInterrupt, long)} and call
 * {@link #cancel} from any thread, or give it a timeout. A stopped query throws {@link TripleLightException.Kind#INTERRUPTED}
 * and leaves no partial result. Cancelling is final, so use a new interrupt per query. {@link #close} waits for queries
 * using it.
 */
public final class QueryInterrupt implements AutoCloseable {
    private final ReentrantReadWriteLock lock = new ReentrantReadWriteLock();
    private MemorySegment handle;

    private QueryInterrupt(MemorySegment handle) { this.handle = handle; }

    /** An interrupt that stops a query only when cancelled. */
    public static QueryInterrupt create() { return create(Duration.ZERO); }

    /** An interrupt that also stops a query {@code timeout} from now ({@link Duration#ZERO}: no deadline). */
    public static QueryInterrupt create(Duration timeout) {
        if (timeout.isNegative()) throw new IllegalArgumentException("timeout must not be negative");
        long ms = timeout.isZero() ? 0 : Math.max(1, timeout.toMillis());
        try (Arena a = Arena.ofConfined()) {
            MemorySegment out = a.allocate(ADDRESS), err = a.allocate(ADDRESS);
            Database.check((int) Native.INTERRUPT_NEW.invokeExact(ms, out, err), err);
            return new QueryInterrupt(out.get(ADDRESS, 0));
        } catch (TripleLightException e) {
            throw e;
        } catch (Throwable t) {
            throw Database.internal(t);
        }
    }

    /** Stops every query running, or later run, with this interrupt. Safe from any thread; a no-op once closed. */
    public void cancel() {
        lock.readLock().lock();
        try {
            if (handle != null) Native.INTERRUPT_CANCEL.invokeExact(handle);
        } catch (Throwable t) {
            throw Database.internal(t);
        } finally {
            lock.readLock().unlock();
        }
    }

    /** The native handle while {@code body} runs; the interrupt cannot be freed meanwhile. */
    <T> T use(Database.Call<T> body) throws Throwable {
        lock.readLock().lock();
        try {
            if (handle == null) throw new TripleLightException(TripleLightException.Kind.CLOSED, "the interrupt is closed");
            return body.run(handle);
        } finally {
            lock.readLock().unlock();
        }
    }

    @Override
    public void close() {
        lock.writeLock().lock();
        try {
            if (handle != null) Native.INTERRUPT_FREE.invokeExact(handle);
        } catch (Throwable t) {
            throw Database.internal(t);
        } finally {
            handle = null;
            lock.writeLock().unlock();
        }
    }
}
