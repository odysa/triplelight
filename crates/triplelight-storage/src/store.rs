//! Durable updates (task #8). The base snapshot is immutable; committed changes live in a write-ahead log (WAL)
//! and an in-memory delta layered over the base. Readers take an `Arc<Overlay>`: a consistent snapshot that later
//! commits never change. Commit = append the change set to the WAL, fsync, then publish a new overlay.
//! Compaction writes the merged data as a new base and truncates the WAL.
//!
//! On-disk layout of a data directory:
//!   CURRENT    one line: the name of the active base directory
//!   base-<n>/  an immutable base in some `BaseFormat` (never modified after it is written)
//!   wal.log    committed change sets since that base was written
//! Compaction writes `base-<n+1>`, then atomically replaces CURRENT (write tmp, fsync, rename, fsync dir), then
//! truncates the WAL. A crash leaves either the old base with the full WAL, or the new base with a WAL whose records
//! re-apply idempotently. Base directories not named by CURRENT are deleted at open.
//!
//! WAL record: one line per change, then a line `C`. A change is `+ <triple> .` or `- <triple> .` (N-Triples, the
//! default graph), `+ <triple> <graph> .` or `- <triple> <graph> .` (N-Quads, a named graph), `G+ <graph>` (the graph
//! joins the catalog) or `G- <graph>` (it leaves the catalog with its triples). A record is the net effect of one
//! update, so replaying it on a state that already contains it changes nothing.
//! Recovery replays complete records only; a torn tail (crash mid-append) is ignored and truncated.
//!
//! Format fence (task #36): a binary that predates named graphs would read the first named-graph line as a torn tail
//! and truncate the WAL there. So before the first record that touches a named graph, CURRENT is rewritten as
//! `dataset-1 base-<n>`. Older binaries accept only `base-<n>` and refuse to open such a directory before they touch
//! the WAL; the change is one-way.
use crate::model::{DatasetChange, GraphScope, GraphTarget, Perm, TermId, TripleBatch};
use crate::storage::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use oxrdf::{Term, TermRef, Triple};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

mod named;
pub use named::{GraphStateError, NamedGraph};

/// IDs for terms first seen in the delta.
const DELTA_TAG: u64 = 0xd;

fn slot(p: Perm) -> usize { Perm::ALL.iter().position(|&q| q == p).unwrap() }
fn key(perm: Perm, spo: [TermId; 3]) -> [TermId; 3] { let o = perm.order(); [spo[o[0]], spo[o[1]], spo[o[2]]] }

/// Range of sorted keys starting with `prefix`.
fn range<'a>(rows: &'a [[TermId; 3]], prefix: &[TermId]) -> &'a [[TermId; 3]] {
    let (mut lo, mut hi) = ([TermId(0); 3], [TermId(u64::MAX); 3]);
    lo[..prefix.len()].copy_from_slice(prefix);
    hi[..prefix.len()].copy_from_slice(prefix);
    let a = rows.partition_point(|r| *r < lo);
    let b = a + rows[a..].partition_point(|r| *r <= hi);
    &rows[a..b]
}

/// Changes since the base. Invariants: `adds` are absent from the base; `dels` are present in the base.
#[derive(Default, Clone)]
pub struct Delta {
    adds: [Vec<[TermId; 3]>; 6],
    dels: [Vec<[TermId; 3]>; 6],
    terms: HashMap<Term, TermId>,
    rev: Vec<Term>,
}

impl Delta {
    pub fn len(&self) -> usize { self.adds[0].len() + self.dels[0].len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

/// A read snapshot: base plus a frozen delta.
pub struct Overlay {
    base: Arc<dyn Snapshot>,
    delta: Arc<Delta>,
    /// The named graphs (task #36): every graph in the catalog, empty ones included, over this overlay's dictionary.
    named: Arc<named::Catalog>,
}

impl Overlay {
    pub fn delta_len(&self) -> usize { self.delta.len() }
    pub fn base(&self) -> &dyn Snapshot { &*self.base }
    /// Identity of the base (stable across overlays sharing it).
    pub fn base_id(&self) -> usize { Arc::as_ptr(&self.base).cast::<()>() as usize }

    fn visible_in_base(&self, spo: [TermId; 3]) -> Result<bool, StorageError> {
        Ok(self.base.count(Perm::Spo, &spo)? > 0 && self.delta.dels[0].binary_search(&spo).is_err())
    }
    fn contains(&self, spo: [TermId; 3]) -> Result<bool, StorageError> {
        Ok(self.delta.adds[0].binary_search(&spo).is_ok() || self.visible_in_base(spo)?)
    }
}

impl Overlay {
    fn dict(&self) -> DictRef<'_> { DictRef { base: &*self.base, delta: &self.delta } }
}

/// The dictionary of a base plus a delta: base terms first, then terms first seen in the delta. Shared by the
/// default graph (`Overlay`) and every named graph view.
#[derive(Clone, Copy)]
pub(crate) struct DictRef<'a> { base: &'a dyn Snapshot, delta: &'a Delta }

impl DictRef<'_> {
    fn delta_term(&self, id: TermId) -> Result<&Term, StorageError> {
        self.delta.rev.get((id.0 & !(0xf << 60)) as usize).ok_or_else(|| StorageError::other("unknown delta term ID"))
    }
}

impl Dictionary for DictRef<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        Ok(self.base.lookup(term)?.or_else(|| self.delta.terms.get(term).copied()))
    }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        if id.0 >> 60 == DELTA_TAG { Ok(self.delta_term(id)?.clone()) } else { self.base.resolve(id) }
    }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> {
        if id.0 >> 60 == DELTA_TAG { f(self.delta_term(id)?.as_ref()); Ok(()) } else { self.base.with_term(id, f) }
    }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> {
        // Delta terms are in memory; pass base IDs to the base in one batch, keeping input positions.
        let (mut base_ids, mut base_pos) = (Vec::new(), Vec::new());
        for (i, &id) in ids.iter().enumerate() {
            if id.0 >> 60 == DELTA_TAG { f(i, self.delta_term(id)?.as_ref()) } else { base_ids.push(id); base_pos.push(i) }
        }
        self.base.with_terms(&base_ids, &mut |j, t| f(base_pos[j], t))
    }
}

impl Dictionary for Overlay {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.dict().lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.dict().resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.dict().with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> {
        self.dict().with_terms(ids, f)
    }
}

impl Snapshot for Overlay {
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        if self.delta.is_empty() { self.base.matching_rows(perm, prefix) } else { None }
    }
    fn matching_tail(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> {
        if self.delta.is_empty() { self.base.matching_tail(perm, prefix) } else { None }
    }

    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        if self.delta.is_empty() { return self.base.scan(perm, prefix) }
        let (adds, dels) = (range(&self.delta.adds[slot(perm)], prefix), range(&self.delta.dels[slot(perm)], prefix));
        Box::new(MergeScan { base: self.base.scan(perm, prefix), cur: None, at: 0, adds, ai: 0, dels, done: false })
    }

    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        if self.delta.is_empty() { return self.base.scan_for_stats(); }
        let (adds, dels) = (range(&self.delta.adds[slot(Perm::Spo)], &[]), range(&self.delta.dels[slot(Perm::Spo)], &[]));
        Box::new(MergeScan { base: self.base.scan_for_stats(), cur: None, at: 0, adds, ai: 0, dels, done: false })
    }

    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        let base = self.base.count(perm, prefix)?;
        if self.delta.is_empty() { return Ok(base) }
        let (adds, dels) = (range(&self.delta.adds[slot(perm)], prefix).len(), range(&self.delta.dels[slot(perm)], prefix).len());
        Ok(base - dels as u64 + adds as u64)
    }

    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        if self.delta.is_empty() { return self.base.seek(perm, prefix, target) }
        let k = prefix.len();
        // Smallest base value >= target that still has a visible triple under prefix + [value].
        let mut t = target;
        let from_base = loop {
            let Some(v) = self.base.seek(perm, prefix, t)? else { break None };
            let mut p = prefix.to_vec(); p.push(v);
            if self.base.count(perm, &p)? > range(&self.delta.dels[slot(perm)], &p).len() as u64 { break Some(v) }
            if v.0 == u64::MAX { break None }
            t = TermId(v.0 + 1);
        };
        let adds = range(&self.delta.adds[slot(perm)], prefix);
        let from_adds = adds[adds.partition_point(|r| r[k] < target)..].first().map(|r| r[k]);
        Ok(match (from_base, from_adds) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) })
    }
}

/// Lazy merge of a base scan (minus deleted keys) with the delta's added keys, batch by batch: memory stays at about
/// one base batch plus one output batch, whatever the size of the range. A base read error is passed through and
/// ends the scan.
struct MergeScan<'a> {
    base: Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>,
    cur: Option<TripleBatch>,
    at: usize,
    adds: &'a [[TermId; 3]],
    ai: usize,
    dels: &'a [[TermId; 3]],
    done: bool,
}

impl MergeScan<'_> {
    /// Next visible base key, pulling base batches as needed; `Ok(None)` at the end.
    fn peek_base(&mut self) -> Result<Option<[TermId; 3]>, StorageError> {
        loop {
            if let Some(b) = &self.cur {
                while self.at < b.len() {
                    let k = [b.cols[0][self.at], b.cols[1][self.at], b.cols[2][self.at]];
                    if self.dels.binary_search(&k).is_err() { return Ok(Some(k)) }
                    self.at += 1;
                }
            }
            match self.base.next() {
                None => { self.cur = None; return Ok(None) }
                Some(b) => { self.cur = Some(b?); self.at = 0; }
            }
        }
    }
}

impl Iterator for MergeScan<'_> {
    type Item = Result<TripleBatch, StorageError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done { return None }
        let mut out = TripleBatch::default();
        while out.len() < 4096 {
            let base = match self.peek_base() { Ok(k) => k, Err(e) => { self.done = true; return Some(Err(e)) } };
            let add = self.adds.get(self.ai).copied();
            let k = match (base, add) {
                (None, None) => { self.done = true; break }
                (Some(b), Some(a)) if a < b => { self.ai += 1; a }
                (Some(b), _) => { self.at += 1; b }
                (None, Some(a)) => { self.ai += 1; a }
            };
            for c in 0..3 { out.cols[c].push(k[c]) }
        }
        if out.is_empty() { None } else { Some(Ok(out)) }
    }
}

pub use crate::model::Change;

/// How bases are written and opened (the memory/N-Triples format here; the disk snapshot plugs in the same way).
pub trait BaseFormat: Send + Sync {
    /// Creates directory `dir` (which must not exist; fail if it does) and writes `src` into it as a new base.
    /// Before returning, every file, `dir` itself and its parent directory are fsynced.
    fn write(&self, dir: &Path, src: &dyn Snapshot) -> std::io::Result<()>;
    /// Same contract as `write`, creating the first base from an N-Triples file (or empty).
    fn import(&self, dir: &Path, ntriples: Option<&Path>) -> std::io::Result<()>;
    fn open(&self, dir: &Path) -> std::io::Result<Arc<dyn Snapshot>>;
}

/// Base = `base.nt` in the directory, loaded into a `MemorySnapshot`.
pub struct NTriplesBase { pub workers: usize }

impl BaseFormat for NTriplesBase {
    fn write(&self, dir: &Path, src: &dyn Snapshot) -> std::io::Result<()> {
        std::fs::create_dir(dir)?;
        let path = dir.join("base.nt");
        let mut out = std::io::BufWriter::new(File::create(&path)?);
        for b in src.scan(Perm::Spo, &[]) {
            let b = b?;
            for i in 0..b.len() {
                let t = |id| src.resolve(id);
                writeln!(out, "{} {} {} .", t(b.cols[0][i])?, t(b.cols[1][i])?, t(b.cols[2][i])?)?;
            }
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        sync_dir(dir)?;
        sync_dir(dir.parent().unwrap())
    }
    fn import(&self, dir: &Path, ntriples: Option<&Path>) -> std::io::Result<()> {
        std::fs::create_dir(dir)?;
        let path = dir.join("base.nt");
        match ntriples { Some(src) => { std::fs::copy(src, &path)?; } None => { File::create(&path)?; } }
        File::open(&path)?.sync_all()?;
        sync_dir(dir)?;
        sync_dir(dir.parent().unwrap())
    }
    fn open(&self, dir: &Path) -> std::io::Result<Arc<dyn Snapshot>> {
        Ok(Arc::new(MemorySnapshot::from_ntriples_parallel_file(dir.join("base.nt"), self.workers.max(1))?))
    }
}

/// Takes the exclusive directory lock, retrying briefly before reporting `WouldBlock` ("already open").
///
/// `flock` locks belong to the open file description, and a child process spawned anywhere in this process gets a
/// copy of every descriptor (including a just-closed database's LOCK) until it execs and close-on-exec drops it. A
/// database closed while another thread spawns can therefore stay locked for a moment after its last handle is
/// dropped. A database that is really open keeps the lock, so it is still reported after the bounded wait.
fn lock_directory(file: &File) -> std::io::Result<()> {
    const WAIT: std::time::Duration = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();
    let mut pause = std::time::Duration::from_millis(1);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < WAIT => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_millis(32));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "database directory is already open"))
            }
        }
    }
}

/// A persistent store: base file + WAL in a directory.
pub struct Store {
    dir: PathBuf,
    format: Box<dyn BaseFormat>,
    /// Number of the active base directory, and whether CURRENT carries the dataset fence (see the module docs).
    generation: Mutex<u64>,
    fenced: std::sync::atomic::AtomicBool,
    /// What readers see: everything durable.
    current: RwLock<Arc<Overlay>>,
    /// Accepted change sets, possibly not yet durable (group commit).
    pending: Mutex<Pending>,
    /// The open WAL; whoever holds it flushes all pending records with one fsync.
    wal: Mutex<Wal>,
    /// Serializes update requests (read-compute-enqueue), so each sees all earlier ones.
    writer: Mutex<()>,
    /// Set if a WAL write or fsync failed: in-memory and durable state may differ, so further writes are refused.
    poisoned: std::sync::atomic::AtomicBool,
    /// Held until the other store state is dropped; the persistent file must never be removed on close.
    _directory_lock: File,
}

struct Pending { latest: Arc<Overlay>, buf: String, seq: u64 }
struct Wal { file: File, durable: u64 }

impl Store {
    /// Opens `dir` with N-Triples bases, creating it from `initial` if it has no base yet; replays the WAL.
    pub fn open(dir: impl AsRef<Path>, initial: Option<&Path>, workers: usize) -> std::io::Result<Self> {
        Self::open_with(dir, initial, Box::new(NTriplesBase { workers }))
    }

    pub fn open_with(dir: impl AsRef<Path>, initial: Option<&Path>, format: Box<dyn BaseFormat>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        // Lock before reading CURRENT, recovery, or deleting any abandoned generation.
        // The file remains in place to avoid lock-inode replacement races between processes.
        let directory_lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("LOCK"))?;
        lock_directory(&directory_lock)?;
        let (generation, fenced) = match std::fs::read_to_string(dir.join("CURRENT")) {
            Ok(s) => parse_current(&s).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "corrupt CURRENT"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let base0 = dir.join("base-0");
                // No CURRENT means no base was ever committed: a `base-0` here is an abandoned earlier attempt.
                if base0.exists() { std::fs::remove_dir_all(&base0)?; }
                format.import(&base0, initial)?;
                set_current(&dir, 0, false)?;
                (0, false)
            }
            Err(e) => return Err(e),
        };
        // Leftovers of crashed compactions or superseded bases.
        for entry in std::fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("base-") && name != format!("base-{generation}") { std::fs::remove_dir_all(dir.join(&name))?; }
        }
        let base = format.open(&dir.join(format!("base-{generation}")))?;
        let mut overlay = Overlay::from_base(base);
        let wal_path = dir.join("wal.log");
        let (records, valid_len) = read_wal(&wal_path)?;
        for rec in records { overlay = overlay.with_dataset_changes(&rec)?.0; }
        let mut wal = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&wal_path)?;
        wal.set_len(valid_len)?; // drop a torn tail
        wal.sync_all()?;
        use std::io::Seek;
        wal.seek(std::io::SeekFrom::End(0))?;
        let overlay = Arc::new(overlay);
        Ok(Store {
            dir, format, generation: Mutex::new(generation), fenced: std::sync::atomic::AtomicBool::new(fenced),
            current: RwLock::new(overlay.clone()),
            pending: Mutex::new(Pending { latest: overlay, buf: String::new(), seq: 0 }),
            wal: Mutex::new(Wal { file: wal, durable: 0 }),
            writer: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            _directory_lock: directory_lock,
        })
    }

    /// The current read snapshot.
    pub fn snapshot(&self) -> Arc<Overlay> { self.current.read().unwrap().clone() }

    /// The active base generation and a snapshot of it, read consistently (compaction switches both while holding
    /// the generation lock).
    pub fn generation_snapshot(&self) -> (u64, Arc<Overlay>) {
        let generation = self.generation.lock().unwrap();
        (*generation, self.snapshot())
    }

    /// Runs `f` on the database directory only while `generation` is still the active base, holding the generation
    /// lock so compaction cannot switch bases meanwhile. For small derived files tied to one base (planner
    /// statistics); the caller holds this `Store`, so the directory lock is held too. `None` if the base changed.
    pub fn with_generation<R>(&self, generation: u64, f: impl FnOnce(&Path) -> R) -> Option<R> {
        let current = self.generation.lock().unwrap();
        (*current == generation).then(|| f(&self.dir))
    }

    /// Durably commits a change set; returns once it is fsynced and visible to new readers. Concurrent commits
    /// share one fsync (group commit). No-op changes (inserting a present triple, deleting an absent one) are dropped.
    pub fn commit(&self, changes: &[(Change, Triple)]) -> std::io::Result<usize> {
        // Serialize acceptance with SPARQL update's read/compute/enqueue transaction.
        // Release before fsync so concurrent writers can still share a group commit.
        let (seq, n) = {
            let _writer = self.writer.lock().unwrap();
            self.enqueue(changes)?
        };
        self.flush_until(seq)?;
        Ok(n)
    }

    fn check_poisoned(&self) -> std::io::Result<()> {
        if self.poisoned.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("store is read-only after a WAL write failure; reopen it to recover"));
        }
        Ok(())
    }

    /// Validates and appends a change set to the pending state; returns the sequence number to wait for.
    fn enqueue(&self, changes: &[(Change, Triple)]) -> std::io::Result<(u64, usize)> {
        self.check_poisoned()?;
        let mut p = self.pending.lock().unwrap();
        let cur = p.latest.clone();
        let rec = net_changes(&cur, changes)?;
        // Even a no-op must wait for earlier accepted changes it may have observed.
        if rec.is_empty() { return Ok((p.seq, 0)) }
        for (c, t) in &rec { push_quad(&mut p.buf, *c, t, None) }
        p.buf.push_str("C\n");
        p.latest = Arc::new(apply(&cur, &rec)?);
        p.seq += 1;
        Ok((p.seq, rec.len()))
    }

    /// Makes everything up to `seq` durable and visible. One caller writes and fsyncs all pending records; callers
    /// whose records it covered return without another fsync.
    fn flush_until(&self, seq: u64) -> std::io::Result<()> {
        let mut w = self.wal.lock().unwrap();
        if w.durable >= seq { return Ok(()) }
        self.check_poisoned()?;
        let (buf, upto, overlay) = {
            let mut p = self.pending.lock().unwrap();
            (std::mem::take(&mut p.buf), p.seq, p.latest.clone())
        };
        let written = w.file.write_all(buf.as_bytes()).and_then(|_| w.file.sync_data());
        if let Err(e) = written {
            self.poisoned.store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(e);
        }
        w.durable = upto;
        *self.current.write().unwrap() = overlay;
        Ok(())
    }

    /// Writes base and delta as a new base, switches to it atomically, and empties the WAL. Readers are not
    /// blocked (they keep their snapshot); writers wait.
    pub fn compact(&self) -> std::io::Result<()> {
        let _writer = self.writer.lock().unwrap();
        let seq = self.pending.lock().unwrap().seq;
        self.flush_until(seq)?;
        let mut w = self.wal.lock().unwrap();
        let mut p = self.pending.lock().unwrap(); // no new commits while we switch
        let cur = p.latest.clone();
        if cur.delta.is_empty() && !cur.has_named_graphs() { return Ok(()) }
        let mut generation = self.generation.lock().unwrap();
        let next = *generation + 1;
        let new_dir = self.dir.join(format!("base-{next}"));
        // Not named by CURRENT, so at most an abandoned earlier attempt in this process.
        if new_dir.exists() { std::fs::remove_dir_all(&new_dir)?; }
        self.format.write(&new_dir, &*cur)?;
        // Open (and so validate) the new base before committing to it: if this fails, CURRENT, the WAL and the
        // in-memory state all still describe the old base.
        let base = self.format.open(&new_dir)?;
        let fenced = self.fenced.load(std::sync::atomic::Ordering::SeqCst);
        let mut fresh = Overlay::from_base(base);
        if cur.has_named_graphs() {
            // Bases do not hold named graphs yet, so they move into the new WAL as its first record. That WAL is
            // durable before CURRENT switches; a crash before the rename leaves the old WAL, whose full history
            // re-applies idempotently on the new base.
            let carried = cur.named_graph_changes()?;
            let (moved, _, rec) = fresh.apply_dataset(&carried, true)?;
            let mut buf = String::new();
            push_record(&mut buf, &rec);
            let tmp = self.dir.join("wal.log.tmp");
            let mut f = File::create(&tmp)?;
            f.write_all(buf.as_bytes())?;
            f.sync_all()?;
            set_current(&self.dir, next, fenced)?;
            std::fs::rename(&tmp, self.dir.join("wal.log"))?;
            sync_dir(&self.dir)?;
            let mut file = OpenOptions::new().read(true).write(true).open(self.dir.join("wal.log"))?;
            use std::io::Seek;
            file.seek(std::io::SeekFrom::End(0))?;
            w.file = file;
            fresh = moved;
        } else {
            set_current(&self.dir, next, fenced)?;
            // The new base is durable and current; only now drop the log.
            w.file.set_len(0)?;
            w.file.sync_all()?;
            use std::io::Seek;
            w.file.seek(std::io::SeekFrom::Start(0))?;
        }
        let fresh = Arc::new(fresh);
        p.latest = fresh.clone();
        *self.current.write().unwrap() = fresh;
        *generation = next;
        Ok(())
    }
}

/// CURRENT's content: `base-<n>`, or `dataset-1 base-<n>` once fenced. `None` if it is neither.
fn parse_current(s: &str) -> Option<(u64, bool)> {
    let s = s.trim();
    let (fenced, base) = match s.strip_prefix("dataset-1 ") { Some(rest) => (true, rest), None => (false, s) };
    let n = base.strip_prefix("base-")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { return None }
    Some((n.parse().ok()?, fenced))
}

/// Atomically points CURRENT at `base-<n>` (with the dataset fence if `fenced`).
fn set_current(dir: &Path, n: u64, fenced: bool) -> std::io::Result<()> {
    let tmp = dir.join("CURRENT.tmp");
    let mut f = File::create(&tmp)?;
    writeln!(f, "{}base-{n}", if fenced { "dataset-1 " } else { "" })?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join("CURRENT"))?;
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> std::io::Result<()> { File::open(dir)?.sync_all() }

fn ids_if_known(o: &Overlay, t: &Triple) -> Result<Option<[TermId; 3]>, StorageError> {
    let (Some(s), Some(p), Some(x)) = (o.lookup(&t.subject.clone().into())?, o.lookup(&t.predicate.clone().into())?, o.lookup(&t.object)?)
        else { return Ok(None) };
    Ok(Some([s, p, x]))
}

/// New overlay = `cur` + one committed record (copy-on-write delta).
/// The changes of `changes` that actually change `cur`: inserting a present triple or deleting an absent one is
/// dropped. Presence is tracked through the set, which may insert and later delete the same triple.
fn net_changes(cur: &Overlay, changes: &[(Change, Triple)]) -> std::io::Result<Vec<(Change, Triple)>> {
    let mut rec = Vec::new();
    let mut state: HashMap<Triple, bool> = HashMap::new();
    for (c, t) in changes {
        let present = match state.get(t) {
            Some(&v) => v,
            None => match ids_if_known(cur, t)? { Some(spo) => cur.contains(spo)?, None => false },
        };
        if (*c == Change::Insert) != present { rec.push((*c, t.clone())) }
        state.insert(t.clone(), *c == Change::Insert);
    }
    Ok(rec)
}

impl Overlay {
    /// A read snapshot over `base` with no pending changes: the starting point of an in-memory, non-durable store
    /// (used by the WebAssembly build, which has no file system).
    pub fn from_base(base: Arc<dyn Snapshot>) -> Overlay { Overlay { base, delta: Arc::new(Delta::default()), named: Default::default() } }

    /// This snapshot with `changes` applied, in memory and without a WAL, plus the number of triples that actually
    /// changed.
    pub fn with_changes(&self, changes: &[(Change, Triple)]) -> std::io::Result<(Overlay, usize)> {
        let rec = net_changes(self, changes)?;
        Ok((apply(self, &rec)?, rec.len()))
    }
}

fn apply(cur: &Overlay, rec: &[(Change, Triple)]) -> std::io::Result<Overlay> {
    let mut d: Delta = (*cur.delta).clone();
    let intern = |d: &mut Delta, term: Term| -> std::io::Result<TermId> {
        if let Some(id) = cur.base.lookup(&term)?.or_else(|| d.terms.get(&term).copied()) { return Ok(id) }
        let id = TermId((DELTA_TAG << 60) | d.rev.len() as u64);
        d.terms.insert(term.clone(), id);
        d.rev.push(term);
        Ok(id)
    };
    for (c, t) in rec {
        let spo = [intern(&mut d, t.subject.clone().into())?, intern(&mut d, t.predicate.clone().into())?, intern(&mut d, t.object.clone())?];
        let in_base = cur.base.count(Perm::Spo, &spo)? > 0;
        for p in Perm::ALL {
            let k = key(p, spo);
            let (adds, dels) = (&mut d.adds[slot(p)], &mut d.dels[slot(p)]);
            match (c, in_base) {
                (Change::Insert, true) => if let Ok(i) = dels.binary_search(&k) { dels.remove(i); },
                (Change::Insert, false) => if let Err(i) = adds.binary_search(&k) { adds.insert(i, k) },
                (Change::Delete, true) => if let Err(i) = dels.binary_search(&k) { dels.insert(i, k) },
                (Change::Delete, false) => if let Ok(i) = adds.binary_search(&k) { adds.remove(i); },
            }
        }
    }
    let delta = Arc::new(d);
    // Named graph views share the dictionary, so they follow the new delta (their rows are unchanged).
    let named = cur.named.rebound(&cur.base, &delta);
    Ok(Overlay { base: cur.base.clone(), delta, named })
}

/// One WAL line for a triple change, in the default graph (`graph == None`) or a named graph.
fn push_quad(buf: &mut String, c: Change, t: &Triple, graph: Option<&oxrdf::NamedNode>) {
    use std::fmt::Write as _;
    buf.push_str(if c == Change::Insert { "+ " } else { "- " });
    let _ = match graph { Some(g) => writeln!(buf, "{t} {g} ."), None => writeln!(buf, "{t} .") };
}

/// A net dataset record (from `apply_dataset`) as WAL lines, with its commit line.
fn push_record(buf: &mut String, rec: &[DatasetChange]) {
    for change in rec {
        match change {
            DatasetChange::Quad(GraphTarget::Default, c, t) => push_quad(buf, *c, t, None),
            DatasetChange::Quad(GraphTarget::Named(g), c, t) => push_quad(buf, *c, t, Some(g)),
            DatasetChange::Create { graph, .. } => { buf.push_str("G+ "); buf.push_str(&graph.to_string()); buf.push('\n') }
            DatasetChange::Drop { target: GraphScope::Graph(graph), .. } => { buf.push_str("G- "); buf.push_str(&graph.to_string()); buf.push('\n') }
            other => unreachable!("not a net record step: {other:?}"),
        }
    }
    buf.push_str("C\n");
}

/// Whether a record needs the dataset fence (it mentions a named graph).
fn needs_fence(rec: &[DatasetChange]) -> bool {
    rec.iter().any(|c| !matches!(c, DatasetChange::Quad(GraphTarget::Default, ..)))
}

/// One WAL line as a replayable step, or `None` if it is not a valid change line (a torn tail).
fn parse_line(line: &str) -> Option<DatasetChange> {
    let graph = |rest: &str| rest.strip_prefix('<')?.strip_suffix('>').and_then(|iri| oxrdf::NamedNode::new(iri).ok());
    if let Some(rest) = line.strip_prefix("G+ ") { return Some(DatasetChange::Create { graph: graph(rest)?, silent: true }) }
    if let Some(rest) = line.strip_prefix("G- ") {
        return Some(DatasetChange::Drop { target: GraphScope::Graph(graph(rest)?), silent: true });
    }
    let (c, rest) = match line.split_at_checked(2) {
        Some(("+ ", r)) => (Change::Insert, r),
        Some(("- ", r)) => (Change::Delete, r),
        _ => return None,
    };
    let mut quads = oxttl::NQuadsParser::new().for_slice(rest.as_bytes());
    let quad = quads.next()?.ok()?;
    if quads.next().is_some() { return None }
    let target = match quad.graph_name {
        oxrdf::GraphName::DefaultGraph => GraphTarget::Default,
        oxrdf::GraphName::NamedNode(g) => GraphTarget::Named(g),
        oxrdf::GraphName::BlankNode(_) => return None,
    };
    Some(DatasetChange::Quad(target, c, Triple::new(quad.subject, quad.predicate, quad.object)))
}

/// Complete records in the WAL, and the byte length they span (everything after is a torn tail).
fn read_wal(path: &Path) -> std::io::Result<(Vec<Vec<DatasetChange>>, u64)> {
    let Ok(f) = File::open(path) else { return Ok((Vec::new(), 0)) };
    let mut records = Vec::new();
    let (mut cur, mut pos, mut valid) = (Vec::new(), 0u64, 0u64);
    for line in BufReader::new(f).split(b'\n') {
        let line = line?;
        pos += line.len() as u64 + 1;
        let Ok(line) = std::str::from_utf8(&line) else { break };
        if line == "C" { records.push(std::mem::take(&mut cur)); valid = pos; continue }
        match parse_line(line) { Some(change) => cur.push(change), None => break }
    }
    Ok((records, valid))
}

impl Store {
    /// Durably commits a dataset update (task #36): the steps apply in order and atomically, as in
    /// `Overlay::with_dataset_changes`, and their net effect is one WAL record. Returns the number of triples changed.
    /// The first record that touches a named graph first fences CURRENT (see the module docs).
    pub fn commit_dataset(&self, changes: &[DatasetChange]) -> std::io::Result<usize> {
        let (seq, n) = {
            let _writer = self.writer.lock().unwrap();
            self.enqueue_dataset(changes)?
        };
        self.flush_until(seq)?;
        Ok(n)
    }

    /// `update_with` for dataset updates: `compute` returns the ordered steps against the latest accepted state.
    pub fn update_dataset_with<E>(
        &self,
        text: &str,
        compute: &dyn Fn(&Overlay, &str) -> Result<Vec<DatasetChange>, E>,
        storage_error: &dyn Fn(std::io::Error) -> E,
    ) -> Result<usize, E> {
        let (seq, n) = {
            let _writer = self.writer.lock().unwrap();
            let snap = self.pending.lock().unwrap().latest.clone();
            let changes = compute(&snap, text)?;
            self.enqueue_dataset(&changes).map_err(storage_error)?
        };
        self.flush_until(seq).map_err(storage_error)?;
        Ok(n)
    }

    fn enqueue_dataset(&self, changes: &[DatasetChange]) -> std::io::Result<(u64, usize)> {
        self.check_poisoned()?;
        let mut p = self.pending.lock().unwrap();
        let cur = p.latest.clone();
        let (next, n, rec) = cur.apply_dataset(changes, true)?;
        if rec.is_empty() { return Ok((p.seq, n)) }
        if needs_fence(&rec) && !self.fenced.load(std::sync::atomic::Ordering::SeqCst) {
            // Durable before any named-graph line can reach the WAL.
            let generation = self.generation.lock().unwrap();
            set_current(&self.dir, *generation, true)?;
            self.fenced.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        push_record(&mut p.buf, &rec);
        p.latest = Arc::new(next);
        p.seq += 1;
        Ok((p.seq, n))
    }
}

impl Store {
    /// Executes an update request atomically: `compute` turns `text` into a change set against the latest accepted
    /// state, and the net change set is committed once (all or nothing). Returns the number of triples changed.
    ///
    /// Serializable: the writer lock is held while reading the latest state, computing the changes and enqueueing
    /// them, so every update sees all earlier accepted updates (durable or not) and none is accepted in between.
    /// The lock is released before waiting for the fsync, so concurrent writers share one group commit. The query
    /// engine is passed in as `compute` (the `triplelight` crate wires in SPARQL Update), which keeps this crate
    /// independent of the executor.
    pub fn update_with<E>(
        &self,
        text: &str,
        compute: &dyn Fn(&Overlay, &str) -> Result<Vec<(Change, Triple)>, E>,
        storage_error: &dyn Fn(std::io::Error) -> E,
    ) -> Result<usize, E> {
        let (seq, n) = {
            let _writer = self.writer.lock().unwrap();
            let snap = self.pending.lock().unwrap().latest.clone();
            let changes = compute(&snap, text)?;
            self.enqueue(&changes).map_err(storage_error)?
        };
        self.flush_until(seq).map_err(storage_error)?;
        Ok(n)
    }
}

#[cfg(test)]
mod writer_serialization_tests {
    use super::*;
    #[test]
    fn direct_commit_waits_for_update_writer() {
        let dir = std::env::temp_dir().join(format!("rdf-writer-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(Store::open(&dir, None, 1).unwrap());
        let guard = store.writer.lock().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread_store = Arc::clone(&store);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = thread_store.commit(&[]);
            done_tx.send(result.is_ok()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(std::time::Duration::from_millis(100)).is_err());
        drop(guard);
        assert!(done_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap());
        worker.join().unwrap();
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod borrowed_overlay_tests {
    use super::*;
    use oxrdf::{NamedNode, Triple};

    #[test]
    fn borrowed_rows_only_bypass_an_empty_delta() {
        let a = Triple::new(NamedNode::new("urn:a").unwrap(), NamedNode::new("urn:p").unwrap(), NamedNode::new("urn:o").unwrap());
        let b = Triple::new(NamedNode::new("urn:b").unwrap(), NamedNode::new("urn:p").unwrap(), NamedNode::new("urn:o").unwrap());
        let base = Arc::new(MemorySnapshot::from_triples([a.clone()]));
        let p = base.lookup(&a.predicate.clone().into()).unwrap().unwrap();
        let original = Overlay::from_base(base);
        assert_eq!(original.matching_tail(Perm::Pso, &[p]).unwrap().len(), 1);
        assert!(original.matching_tail(Perm::Pso, &[]).is_none(), "an empty prefix has no tail view");
        let (added, _) = original.with_changes(&[(Change::Insert, b.clone())]).unwrap();
        assert!(added.matching_tail(Perm::Pso, &[p]).is_none());
        assert_eq!(added.count(Perm::Spo, &[]).unwrap(), 2);
        let (deleted, _) = original.with_changes(&[(Change::Delete, a)]).unwrap();
        assert!(deleted.matching_tail(Perm::Pso, &[p]).is_none());
        assert_eq!(deleted.count(Perm::Spo, &[]).unwrap(), 0);
        let (restored, _) = added.with_changes(&[(Change::Delete, b)]).unwrap();
        assert_eq!(restored.matching_tail(Perm::Pso, &[p]).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod directory_lock_tests {
    use super::*;

    /// A child process spawned while a database closes holds a copy of its LOCK descriptor until it execs. Model
    /// that copy with a clone of the same open file description, released shortly after close: reopening must wait
    /// for it instead of reporting the closed database as still open.
    #[test]
    fn reopen_waits_out_a_briefly_inherited_lock_descriptor() {
        let dir = std::env::temp_dir().join(format!("rdf-inherited-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir, None, 1).unwrap();
        let inherited = store._directory_lock.try_clone().unwrap();
        drop(store);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            drop(inherited);
        });
        let reopened = Store::open(&dir, None, 1).expect("reopen after the inherited descriptor closes");
        releaser.join().unwrap();
        // A database that really is open still reports WouldBlock after the bounded wait.
        let err = Store::open(&dir, None, 1).err().expect("second open of an open directory");
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod dataset_wal_tests {
    use super::*;
    use triplelight_core::storage::DatasetView;
    use oxrdf::{BlankNode, Literal, NamedNode};
    use DatasetChange::{Clear, Create, Drop, Quad};

    fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
    fn t(s: &str, o: &str) -> Triple { Triple::new(iri(s), iri("p"), Literal::new_simple_literal(o)) }
    fn ins(g: &str, tr: Triple) -> DatasetChange { Quad(GraphTarget::Named(iri(g)), Change::Insert, tr) }
    fn del(g: &str, tr: Triple) -> DatasetChange { Quad(GraphTarget::Named(iri(g)), Change::Delete, tr) }
    fn ins_default(tr: Triple) -> DatasetChange { Quad(GraphTarget::Default, Change::Insert, tr) }

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("rdf-dataset-wal-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            Dir(d)
        }
        fn open(&self) -> Store { Store::open(&self.0, None, 1).unwrap() }
        fn current(&self) -> String { std::fs::read_to_string(self.0.join("CURRENT")).unwrap() }
        fn wal(&self) -> Vec<u8> { std::fs::read(self.0.join("wal.log")).unwrap_or_default() }
    }
    impl std::ops::Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    /// The default graph and every named graph, as sorted N-Triples strings keyed by graph IRI.
    fn state(o: &Overlay) -> (Vec<String>, Vec<(String, Vec<String>)>) {
        fn rows(view: &dyn Snapshot) -> Vec<String> {
            let mut v = Vec::new();
            for b in view.scan(Perm::Spo, &[]) {
                let b = b.unwrap();
                let r = |id: TermId| view.resolve(id).unwrap().to_string();
                for ((&s, &p), &x) in b.cols[0].iter().zip(&b.cols[1]).zip(&b.cols[2]) { v.push(format!("{} {} {}", r(s), r(p), r(x))) }
            }
            v.sort();
            v
        }
        let mut named: Vec<(String, Vec<String>)> = o.graph_names().unwrap().into_iter()
            .map(|g| (o.resolve(g).unwrap().to_string(), rows(o.graph(g).unwrap().unwrap()))).collect();
        named.sort();
        (rows(o), named)
    }

    fn sample() -> Vec<DatasetChange> {
        let x = BlankNode::new("x").unwrap();
        vec![ins_default(t("d", "0")), ins("g1", t("a", "1")), ins("g1", Triple::new(x.clone(), iri("p"), x)),
             Create { graph: iri("empty"), silent: false }, ins("g2", t("b", "2"))]
    }

    #[test]
    fn named_graphs_survive_reopen_and_fence_current_once() {
        let dir = Dir::new("reopen");
        let store = dir.open();
        store.commit(&[(Change::Insert, t("plain", "0"))]).unwrap();
        assert_eq!(dir.current().trim(), "base-0", "default-only commits leave the store readable by older binaries");
        assert_eq!(store.commit_dataset(&[ins_default(t("plain", "1"))]).unwrap(), 1);
        assert_eq!(dir.current().trim(), "base-0", "a default-only dataset update needs no fence either");
        store.commit_dataset(&sample()).unwrap();
        assert_eq!(dir.current().trim(), "dataset-1 base-0");
        store.commit_dataset(&[del("g2", t("b", "2")), Drop { target: GraphScope::Graph(iri("g1")), silent: false }]).unwrap();
        let want = state(&store.snapshot());
        assert_eq!(want.1.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), ["<http://ex/empty>", "<http://ex/g2>"]);
        drop(store);
        let reopened = dir.open();
        assert_eq!(state(&reopened.snapshot()), want);
        assert_eq!(dir.current().trim(), "dataset-1 base-0");
    }

    #[test]
    fn compaction_carries_named_graphs_and_keeps_the_fence() {
        let dir = Dir::new("compact");
        let store = dir.open();
        store.commit_dataset(&sample()).unwrap();
        let want = state(&store.snapshot());
        store.compact().unwrap();
        assert_eq!(dir.current().trim(), "dataset-1 base-1");
        assert_eq!(state(&store.snapshot()), want);
        store.commit_dataset(&[ins("g2", t("after", "3"))]).unwrap();
        let want = state(&store.snapshot());
        drop(store);
        let reopened = dir.open();
        assert_eq!(state(&reopened.snapshot()), want);
        // Compacting again with no default-graph changes still carries the graphs.
        reopened.compact().unwrap();
        drop(reopened);
        assert_eq!(state(&dir.open().snapshot()), want);
    }

    /// The crash windows of a compaction with named graphs: the new WAL is written but CURRENT still names the old
    /// base, and CURRENT names the new base but the old WAL is still in place.
    #[test]
    fn compaction_crash_states_recover() {
        let dir = Dir::new("crash");
        let store = dir.open();
        store.commit_dataset(&sample()).unwrap();
        store.commit_dataset(&[del("g1", t("a", "1")), ins_default(t("d", "9"))]).unwrap();
        let want = state(&store.snapshot());
        let old_wal = dir.wal();
        store.compact().unwrap();
        drop(store);
        // New base, old WAL: the whole history re-applies on top of the base that already contains it.
        std::fs::write(dir.0.join("wal.log"), &old_wal).unwrap();
        assert_eq!(state(&dir.open().snapshot()), want);
        // Old CURRENT target is gone after compaction, so model the other window directly: fenced CURRENT, no WAL.
        let fresh = Dir::new("crash-fence");
        drop(fresh.open());
        std::fs::write(fresh.0.join("CURRENT"), "dataset-1 base-0\n").unwrap();
        let s = fresh.open();
        assert!(!s.snapshot().has_named_graphs());
        s.commit_dataset(&[ins("g", t("a", "1"))]).unwrap();
        drop(s);
        assert_eq!(state(&fresh.open().snapshot()).1.len(), 1);
    }

    #[test]
    fn torn_and_duplicated_records() {
        let dir = Dir::new("torn");
        let store = dir.open();
        store.commit_dataset(&sample()).unwrap();
        let want = state(&store.snapshot());
        drop(store);
        let good = dir.wal();
        // A record appended twice changes nothing.
        let mut twice = good.clone();
        twice.extend_from_slice(&good);
        std::fs::write(dir.0.join("wal.log"), &twice).unwrap();
        assert_eq!(state(&dir.open().snapshot()), want);
        // A torn named-graph record (no commit line) is dropped and truncated, whatever line it stops at.
        for tail in ["G+ <http://ex/torn>\n+ <http://ex/s> <http://ex/p> \"v\" <http://ex/torn> .\n", "G+ <http://ex/to", "+ <http://ex/s> <http://ex/p> \"v\" <http://ex/torn"] {
            let mut torn = good.clone();
            torn.extend_from_slice(tail.as_bytes());
            std::fs::write(dir.0.join("wal.log"), &torn).unwrap();
            assert_eq!(state(&dir.open().snapshot()), want, "{tail}");
            assert_eq!(dir.wal(), good, "{tail}");
        }
    }

    #[test]
    fn refused_or_empty_updates_write_nothing() {
        let dir = Dir::new("refused");
        let store = dir.open();
        store.commit_dataset(&[ins_default(t("d", "0"))]).unwrap();
        let wal = dir.wal();
        let e = store.commit_dataset(&[ins("g", t("a", "1")), Drop { target: GraphScope::Graph(iri("absent")), silent: false }]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(store.commit_dataset(&[Clear { target: GraphScope::Graph(iri("absent")), silent: true }]).unwrap(), 0);
        assert_eq!((dir.wal(), dir.current().trim().to_string()), (wal, "base-0".to_string()));
        assert!(!store.snapshot().has_named_graphs());
    }

    #[test]
    fn clear_default_is_logged_as_its_net_deletions() {
        let dir = Dir::new("clear");
        std::fs::create_dir_all(&dir.0).unwrap();
        let nt = dir.0.with_extension("nt");
        std::fs::write(&nt, "<http://ex/b> <http://ex/p> \"base\" .\n").unwrap();
        let store = Store::open(&dir.0, Some(&nt), 1).unwrap();
        store.commit_dataset(&[ins_default(t("d", "1")), ins("g", t("a", "1")), Clear { target: GraphScope::Default, silent: false }, ins_default(t("d", "2"))]).unwrap();
        let want = state(&store.snapshot());
        assert_eq!(want.0, ["<http://ex/d> <http://ex/p> \"2\""]);
        drop(store);
        assert_eq!(state(&dir.open().snapshot()), want);
        let _ = std::fs::remove_file(nt);
    }

    #[test]
    fn current_parsing() {
        assert_eq!(parse_current("base-7\n"), Some((7, false)));
        assert_eq!(parse_current("dataset-1 base-12\n"), Some((12, true)));
        for bad in ["", "base-", "base-+1", "dataset-2 base-1", "dataset-1base-1", "dataset-1 ", "base-1 extra"] {
            assert_eq!(parse_current(bad), None, "{bad:?}");
        }
    }
}
