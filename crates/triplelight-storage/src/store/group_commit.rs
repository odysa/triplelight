//! Group commit (#105): how a commit's record becomes durable. Whoever takes the WAL lock leads: it writes and fsyncs
//! every pending record as one batch. The others wait for `Store::durable` to reach their record on `flushed`, not
//! on the WAL lock.
use super::{batch, fault, Arc, Overlay, Store, Wal};
use std::io::Write;

impl Store {
    /// Makes everything up to `seq` durable and visible (group commit, #105). Whoever takes the WAL lock is the
    /// leader: it writes and fsyncs every pending record as one batch. The others wait on `flushed`, not on the WAL
    /// lock, so a writer whose record a batch covered returns as soon as that fsync ends, while the next leader is
    /// still syncing; it can then enqueue again in time for the batch after. Waiting on the WAL lock instead held
    /// them until the next fsync ended too, which split concurrent writers into halves that took turns.
    pub(super) fn flush_until(&self, seq: u64) -> std::io::Result<()> {
        use std::sync::atomic::Ordering::SeqCst;
        loop {
            if self.durable.load(SeqCst) >= seq { return Ok(()) }
            self.check_poisoned()?;
            match self.wal.try_lock() {
                Ok(mut w) => {
                    let flushed = self.lead_flush(&mut w);
                    // The leader steps down as it frees the WAL lock and wakes the waiters: one whose record the
                    // batch missed then finds nobody leading and leads the next batch at once.
                    self.end_leading(w);
                    flushed?
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    // A leader is flushing: wait for it to step down, which `end_leading` signals under `pending`, so
                    // the check and the wait cannot miss it. Without a leader the WAL lock is held by compaction,
                    // which flushes everything first; poll briefly then. The long timeout is only a safety net.
                    let p = self.pending.lock().unwrap();
                    let wait = if p.leading { std::time::Duration::from_millis(20) } else { std::time::Duration::from_millis(1) };
                    let _ = self.flushed.wait_timeout_while(p, wait, |p| {
                        p.leading && self.durable.load(SeqCst) < seq && !self.poisoned.load(SeqCst)
                    }).unwrap();
                }
                Err(std::sync::TryLockError::Poisoned(e)) => panic!("WAL lock poisoned: {e}"),
            }
        }
    }

    /// The leader's flush: takes every pending record, writes and fsyncs them as one batch, then publishes them.
    fn lead_flush(&self, w: &mut Wal) -> std::io::Result<()> {
        let (buf, upto, overlay, records) = {
            let mut p = self.pending.lock().unwrap();
            p.leading = true;
            // Adaptive gather: after a batch of several commits, writers are committing concurrently, and those it
            // acknowledged are about to enqueue again. Waiting for as many records as that batch held (never longer
            // than a tenth of its fsync) lets one fsync carry them all. After a batch of one, as with a lone writer,
            // the leader never waits.
            if w.last_batch > 1 && p.records < w.last_batch {
                let deadline = std::time::Instant::now() + (w.last_sync / 10).min(std::time::Duration::from_millis(2));
                while p.records < w.last_batch {
                    let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else { break };
                    p = self.enqueued.wait_timeout(p, left).unwrap().0;
                }
            }
            let records = std::mem::take(&mut p.records);
            (std::mem::take(&mut p.buf), p.seq, p.latest.clone(), records)
        };
        // Nothing to make durable (a batch must hold at least one record, `read_batch`).
        if buf.is_empty() {
            self.publish_durable(upto, overlay);
            return Ok(())
        }
        // Every record reaches the WAL after the batch fence (`append`), so the whole flush is one batch.
        let started = std::time::Instant::now();
        let batch = batch(&buf);
        let written = fault::hit("wal-before-write")
            .and_then(|_| w.file.write_all(&batch))
            .and_then(|_| fault::hit("wal-after-write"))
            .and_then(|_| w.file.sync_data())
            .and_then(|_| fault::hit("wal-after-sync"));
        if let Err(e) = written {
            // `end_leading` wakes the waiters; they see the poison and fail too.
            self.poisoned.store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(std::io::Error::new(e.kind(), format!(
                "appending to {} failed: {e}; the update is not acknowledged and may or may not be durable, and the \
                 store refuses writes until it is reopened", self.dir.join("wal.log").display())));
        }
        (w.last_batch, w.last_sync) = (records, started.elapsed());
        self.publish_durable(upto, overlay);
        Ok(())
    }

    /// Makes `overlay` (everything up to `upto`) visible, then durable for `flush_until`. Its waiters wake when the
    /// leader steps down (`end_leading`).
    pub(super) fn publish_durable(&self, upto: u64, overlay: Arc<Overlay>) {
        *self.current.write().unwrap() = overlay;
        let _p = self.pending.lock().unwrap();
        self.durable.store(upto, std::sync::atomic::Ordering::SeqCst);
    }

    /// The leader steps down: it frees the WAL lock while holding `pending`, so a successor that takes the WAL lock
    /// in between cannot mark itself leading before this clears the flag, then wakes every waiter.
    pub(super) fn end_leading(&self, wal: std::sync::MutexGuard<'_, Wal>) {
        let mut p = self.pending.lock().unwrap();
        drop(wal);
        p.leading = false;
        drop(p);
        self.flushed.notify_all();
    }
}
