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
use triplelight_rdf::{Term, TermRef, Triple};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

mod named;
pub use named::{GraphStateError, NamedGraph};

/// IDs for terms first seen in the delta.
const DELTA_TAG: u64 = crate::model::id_tag::DELTA;

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
    /// Some term first seen in this delta is an RDF 1.2 term (`Snapshot::has_rdf12_terms`).
    #[cfg(feature = "rdf-12")]
    rdf12: bool,
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
    /// `Snapshot::object_memo` for this overlay's own contents (an overlay never changes once published).
    object_memo: triplelight_core::storage::ObjectMemo,
}

impl Overlay {
    pub fn delta_len(&self) -> usize { self.delta.len() }
    pub fn base(&self) -> &dyn Snapshot { &*self.base }
    /// Identity of the base (stable across overlays sharing it).
    pub fn base_id(&self) -> usize { Arc::as_ptr(&self.base).cast::<()>() as usize }

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
    /// A base ID is answered by the base (one block read on a disk base with the `triple_terms` table); a delta
    /// triple term's parts were interned with it, so the default lookup finds them.
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> {
        if id.0 >> 60 != DELTA_TAG { return self.base.triple_term_parts(id) }
        let Term::Triple(t) = self.delta_term(id)? else { return Ok(None) };
        let t = (**t).clone();
        let part = |term: Term| self.lookup(&term)?.ok_or_else(|| StorageError::other("triple-term part missing from the dictionary"));
        Ok(Some([part(t.subject.into())?, part(t.predicate.into())?, part(t.object)?]))
    }
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        Ok(self.base.lookup(term)?.or_else(|| self.delta.terms.get(term).copied()))
    }
    /// The base's literal IDs keep their order; terms first seen in the delta have the delta tag and are outside
    /// every range this returns (callers cover them separately).
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> {
        self.base.literal_text_bound(text, strict)
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
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> { self.dict().triple_term_parts(id) }
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.dict().lookup(term) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { self.dict().literal_text_bound(text, strict) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.dict().resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.dict().with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> {
        self.dict().with_terms(ids, f)
    }
}

impl Snapshot for Overlay {
    #[cfg(feature = "rdf-12")]
    fn has_rdf12_terms(&self) -> bool { self.delta.rdf12 || self.base.has_rdf12_terms() }
    /// Without changes the overlay's contents are the base's, so it shares the base's memo (which then survives
    /// commits that publish new overlays only until the first change).
    fn object_memo(&self) -> Option<&triplelight_core::storage::ObjectMemo> {
        if self.delta.is_empty() { self.base.object_memo() } else { Some(&self.object_memo) }
    }
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        if self.delta.is_empty() { self.base.matching_rows(perm, prefix) } else { None }
    }
    fn matching_tail(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> {
        if self.delta.is_empty() { self.base.matching_tail(perm, prefix) } else { None }
    }

    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        if self.delta.is_empty() { return self.base.scan(perm, prefix) }
        merged_scan(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix)
    }

    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        if self.delta.is_empty() { return self.base.scan_for_stats(); }
        let (adds, dels) = (range(&self.delta.adds[slot(Perm::Spo)], &[]), range(&self.delta.dels[slot(Perm::Spo)], &[]));
        Box::new(MergeScan { base: self.base.scan_for_stats(), cur: None, at: 0, adds, ai: 0, dels, done: false })
    }

    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        merged_count(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix)
    }

    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        if self.delta.is_empty() { return self.base.seek(perm, prefix, target) }
        merged_seek(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix, target)
    }
}

/// `base` with the sorted rows `adds` added and `dels` removed (one order; `adds` absent from `base`, `dels` present
/// in it): the scan, count and seek that the default graph and every stored named graph share.
fn merged_scan<'a>(base: &'a dyn Snapshot, adds: &'a [[TermId; 3]], dels: &'a [[TermId; 3]], perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
    let (adds, dels) = (range(adds, prefix), range(dels, prefix));
    Box::new(MergeScan { base: base.scan(perm, prefix), cur: None, at: 0, adds, ai: 0, dels, done: false })
}
fn merged_count(base: &dyn Snapshot, adds: &[[TermId; 3]], dels: &[[TermId; 3]], perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
    let n = base.count(perm, prefix)?;
    if adds.is_empty() && dels.is_empty() { return Ok(n) }
    Ok(n - range(dels, prefix).len() as u64 + range(adds, prefix).len() as u64)
}
fn merged_seek(base: &dyn Snapshot, adds: &[[TermId; 3]], dels: &[[TermId; 3]], perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
    let k = prefix.len();
    if k >= 3 { return Ok(None) }
    // Smallest base value >= target that still has a visible triple under prefix + [value].
    let mut t = target;
    let from_base = loop {
        let Some(v) = base.seek(perm, prefix, t)? else { break None };
        let mut p = prefix.to_vec(); p.push(v);
        if base.count(perm, &p)? > range(dels, &p).len() as u64 { break Some(v) }
        if v.0 == u64::MAX { break None }
        t = TermId(v.0 + 1);
    };
    let adds = range(adds, prefix);
    let from_adds = adds[adds.partition_point(|r| r[k] < target)..].first().map(|r| r[k]);
    Ok(match (from_base, from_adds) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) })
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
    /// Whether this format's bases can hold named graphs (task #36, step 3c). If not, compaction carries them into
    /// the new WAL instead.
    fn holds_named_graphs(&self) -> bool { false }
    /// `write` for a whole dataset: the default graph and every named graph of `src`.
    fn write_dataset(&self, _dir: &Path, _src: &Overlay) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "this base format does not hold named graphs"))
    }
    /// Compaction's write: `write_dataset` (if `dataset`) or `write` of `src` into `dir`, a base that replaces
    /// `previous`. A format whose bases record build choices keeps them here (the disk base keeps its order set, task
    /// #73), so compaction never changes them; only a new import does.
    fn rewrite(&self, _previous: &Path, dir: &Path, src: &Overlay, dataset: bool) -> std::io::Result<()> {
        if dataset { self.write_dataset(dir, src) } else { self.write(dir, src) }
    }
    /// `open`, plus the base's named graphs: each graph's ID with its view, sharing the base's dictionary.
    fn open_dataset(&self, dir: &Path) -> std::io::Result<(Arc<dyn Snapshot>, Vec<(TermId, Arc<dyn Snapshot>)>)> {
        Ok((self.open(dir)?, Vec::new()))
    }
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

/// The store root's scratch directory, beside the bases: large derived-order regroups spill here (`derived_order.rs`).
/// Emptied at every open, which holds the directory lock.
pub(crate) const SCRATCH_DIR: &str = "scratch";

/// A persistent store: base file + WAL in a directory.
pub struct Store {
    dir: PathBuf,
    format: Box<dyn BaseFormat>,
    /// Number of the active base directory, and whether CURRENT carries the dataset fence (see the module docs).
    generation: Mutex<u64>,
    fenced: std::sync::atomic::AtomicBool,
    /// Whether CURRENT carries the RDF 1.2 fence `rdf12-1` (see `parse_current`).
    rdf12_fenced: std::sync::atomic::AtomicBool,
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
    #[cfg(test)]
    fail_at: Mutex<Option<String>>,
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
        // A bare base (what `build-disk` writes: MANIFEST and order files, no CURRENT) is not a store. Opening it as
        // one would write an empty `base-0` and serve no triples, so refuse before touching the directory.
        if dir.join("MANIFEST").is_file() && !dir.join("CURRENT").exists() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!(
                "{} is a base directory (as written by build-disk), not a store; query it directly, or create a store with --init",
                dir.display())));
        }
        std::fs::create_dir_all(&dir)?;
        // Lock before reading CURRENT, recovery, or deleting any abandoned generation.
        // The file remains in place to avoid lock-inode replacement races between processes.
        let directory_lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("LOCK"))?;
        lock_directory(&directory_lock)?;
        let (generation, fenced, rdf12_fenced) = match std::fs::read_to_string(dir.join("CURRENT")) {
            Ok(s) => parse_current(&s)?.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "corrupt CURRENT"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let base0 = dir.join("base-0");
                // No CURRENT means no base was ever committed: a `base-0` here is an abandoned earlier attempt.
                if base0.exists() { std::fs::remove_dir_all(&base0)?; }
                format.import(&base0, initial)?;
                set_current(&dir, 0, false, false)?;
                (0, false, false)
            }
            Err(e) => return Err(e),
        };
        // Spill scratch of a process that ended mid-regroup (derived orders): nothing in it outlives that process.
        let _ = std::fs::remove_dir_all(dir.join(SCRATCH_DIR));
        // Leftovers of crashed compactions or superseded bases.
        for entry in std::fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("base-") && name != format!("base-{generation}") { std::fs::remove_dir_all(dir.join(&name))?; }
        }
        let (base, named) = format.open_dataset(&dir.join(format!("base-{generation}")))?;
        let mut overlay = Overlay::from_dataset_base(base, named);
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
            rdf12_fenced: std::sync::atomic::AtomicBool::new(rdf12_fenced),
            current: RwLock::new(overlay.clone()),
            pending: Mutex::new(Pending { latest: overlay, buf: String::new(), seq: 0 }),
            wal: Mutex::new(Wal { file: wal, durable: 0 }),
            writer: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            _directory_lock: directory_lock,
            #[cfg(test)]
            fail_at: Mutex::new(None),
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
        self.submit(|_| self.enqueue_triples(changes), &|e| e)
    }

    /// Durably commits a dataset update (task #36): the steps apply in order and atomically, as in
    /// `Overlay::with_dataset_changes`, and their net effect is one WAL record. Returns the number of triples changed.
    /// The first record that touches a named graph first fences CURRENT (see the module docs).
    pub fn commit_dataset(&self, changes: &[DatasetChange]) -> std::io::Result<usize> {
        self.submit(|_| self.enqueue_dataset(changes), &|e| e)
    }

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
        self.submit(|snap| {
            let changes = compute(snap, text)?;
            self.enqueue_triples(&changes).map_err(storage_error)
        }, storage_error)
    }

    /// `update_with` for dataset updates: `compute` returns the ordered steps against the latest accepted state.
    pub fn update_dataset_with<E>(
        &self,
        text: &str,
        compute: &dyn Fn(&Overlay, &str) -> Result<Vec<DatasetChange>, E>,
        storage_error: &dyn Fn(std::io::Error) -> E,
    ) -> Result<usize, E> {
        self.submit(|snap| {
            let changes = compute(snap, text)?;
            self.enqueue_dataset(&changes).map_err(storage_error)
        }, storage_error)
    }

    /// The one write path. Under the writer lock, `enqueue` sees the latest accepted state (durable or not) and appends
    /// one record; the lock is released before the fsync, so concurrent writers share one group commit.
    fn submit<E>(&self, enqueue: impl FnOnce(&Overlay) -> Result<(u64, usize), E>, storage_error: &dyn Fn(std::io::Error) -> E) -> Result<usize, E> {
        let (seq, n) = {
            let _writer = self.writer.lock().unwrap();
            let snap = self.pending.lock().unwrap().latest.clone();
            enqueue(&snap)?
        };
        self.flush_until(seq).map_err(storage_error)?;
        Ok(n)
    }

    fn check_poisoned(&self) -> std::io::Result<()> {
        if self.poisoned.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("store is read-only after a WAL write failure; reopen it to recover"));
        }
        Ok(())
    }

    /// Validates default-graph triple changes, then appends them as one record; returns the sequence number to wait
    /// for and the net change. Validation runs before the conversion to dataset steps, so a rejected batch is not
    /// copied under the writer lock.
    fn enqueue_triples(&self, changes: &[(Change, Triple)]) -> std::io::Result<(u64, usize)> {
        self.check_poisoned()?;
        // Only inserts are checked: deleting an oversized term, which a legacy WAL record can have put in the store,
        // must stay possible.
        for (c, t) in changes { if *c == Change::Insert { check_triple_size(t)? } }
        self.append(&default_changes(changes), Count::Net)
    }

    /// Validates a dataset update, then appends it as one record; returns the sequence number to wait for and the
    /// sum of each step's changed triples.
    fn enqueue_dataset(&self, changes: &[DatasetChange]) -> std::io::Result<(u64, usize)> {
        self.check_poisoned()?;
        // Only what adds data is checked (inserts and CREATE): deleting an oversized term, or dropping or clearing a
        // graph named by one, which a legacy WAL record can have put in the store, must stay possible.
        for change in changes {
            match change {
                DatasetChange::Quad(g, Change::Insert, t) => {
                    check_triple_size(t)?;
                    if let GraphTarget::Named(g) = g { check_term_size(triplelight_rdf::TermRef::NamedNode(g.as_ref()))? }
                }
                DatasetChange::Create { graph, .. } => check_term_size(triplelight_rdf::TermRef::NamedNode(graph.as_ref()))?,
                _ => {}
            }
        }
        self.append(changes, Count::Steps)
    }

    /// Appends validated steps to the pending state as one record (fencing CURRENT first if the record is the first
    /// to touch a named graph); returns the sequence number to wait for and the count `count` asks for.
    fn append(&self, changes: &[DatasetChange], count: Count) -> std::io::Result<(u64, usize)> {
        let mut p = self.pending.lock().unwrap();
        let cur = p.latest.clone();
        let (next, steps, rec) = cur.apply_dataset(changes, true)?;
        let n = match count { Count::Net => rec.len(), Count::Steps => steps };
        // Even a no-op must wait for earlier accepted changes it may have observed.
        if rec.is_empty() { return Ok((p.seq, n)) }
        let dataset = needs_fence(&rec) && !self.fenced.load(std::sync::atomic::Ordering::SeqCst);
        let rdf12 = needs_rdf12_fence(&rec) && !self.rdf12_fenced.load(std::sync::atomic::Ordering::SeqCst);
        if dataset || rdf12 {
            // Durable before any named-graph or RDF 1.2 line can reach the WAL.
            let generation = self.generation.lock().unwrap();
            let fenced = dataset || self.fenced.load(std::sync::atomic::Ordering::SeqCst);
            let rdf12_fenced = rdf12 || self.rdf12_fenced.load(std::sync::atomic::Ordering::SeqCst);
            set_current(&self.dir, *generation, fenced, rdf12_fenced)?;
            self.fenced.store(fenced, std::sync::atomic::Ordering::SeqCst);
            self.rdf12_fenced.store(rdf12_fenced, std::sync::atomic::Ordering::SeqCst);
        }
        push_record(&mut p.buf, &rec);
        p.latest = Arc::new(next);
        p.seq += 1;
        Ok((p.seq, n))
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
        // After a failed switch the on-disk generation may differ from `generation`; only a reopen can tell.
        self.check_poisoned()?;
        let seq = self.pending.lock().unwrap().seq;
        self.flush_until(seq)?;
        let mut w = self.wal.lock().unwrap();
        let mut p = self.pending.lock().unwrap(); // no new commits while we switch
        let cur = p.latest.clone();
        if cur.delta.is_empty() && cur.named_graphs_match_base() { return Ok(()) }
        let mut generation = self.generation.lock().unwrap();
        let next = *generation + 1;
        let new_dir = self.dir.join(format!("base-{next}"));
        // Not named by CURRENT, so at most an abandoned earlier attempt in this process.
        if new_dir.exists() { std::fs::remove_dir_all(&new_dir)?; }
        // Named graphs go into the base when the format holds them (step 3c), otherwise into the new WAL below.
        let into_base = cur.has_named_graphs() && self.format.holds_named_graphs();
        self.format.rewrite(&self.dir.join(format!("base-{}", *generation)), &new_dir, &cur, into_base)?;
        // Open (and so validate) the new base before committing to it: if this fails, CURRENT, the WAL and the
        // in-memory state all still describe the old base.
        let (base, named) = self.format.open_dataset(&new_dir)?;
        let fenced = self.fenced.load(std::sync::atomic::Ordering::SeqCst);
        let rdf12_fenced = self.rdf12_fenced.load(std::sync::atomic::Ordering::SeqCst);
        let mut fresh = Overlay::from_dataset_base(base, named);
        if cur.has_named_graphs() && !into_base {
            // Bases do not hold named graphs yet, so they move into the new WAL as its first record. That WAL is
            // durable before CURRENT switches; a crash before the rename leaves the old WAL, whose full history
            // re-applies idempotently on the new base.
            let carried = cur.named_graph_changes()?;
            let (moved, _, rec) = fresh.apply_dataset(&carried, true)?;
            let mut buf = String::new();
            push_record(&mut buf, &rec);
            let tmp = self.dir.join("wal.log.tmp");
            // This handle (positioned at its end) becomes the live WAL: after the rename it is `wal.log`.
            let mut f = File::create(&tmp)?;
            f.write_all(buf.as_bytes())?;
            f.sync_all()?;
            self.publish(|| {
                set_current(&self.dir, next, fenced, rdf12_fenced)?;
                std::fs::rename(&tmp, self.dir.join("wal.log"))?;
                self.fail_point("after-wal-rename")?;
                sync_dir(&self.dir)
            })?;
            w.file = f;
            fresh = moved;
        } else {
            self.publish(|| {
                set_current(&self.dir, next, fenced, rdf12_fenced)?;
                self.fail_point("after-current")?;
                // The new base is durable and current; only now drop the log.
                w.file.set_len(0)?;
                w.file.sync_all()?;
                use std::io::Seek;
                w.file.seek(std::io::SeekFrom::Start(0)).map(|_| ())
            })?;
        }
        let fresh = Arc::new(fresh);
        p.latest = fresh.clone();
        *self.current.write().unwrap() = fresh;
        *generation = next;
        Ok(())
    }

    /// Runs the steps that switch the store to a new base. If one fails, CURRENT may already name the new base while
    /// the WAL handle, `generation` and the published overlay still describe the old one, so the store is poisoned:
    /// commits and compaction are refused until a reopen recovers from what is on disk.
    fn publish(&self, steps: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        steps().inspect_err(|_| self.poisoned.store(true, std::sync::atomic::Ordering::SeqCst))
    }

    /// Test-only fault injection: fails at the named point when a test armed it.
    #[cfg(test)]
    fn fail_point(&self, point: &str) -> std::io::Result<()> {
        if *self.fail_at.lock().unwrap() == Some(point.to_string()) { return Err(std::io::Error::other(format!("injected failure at {point}"))) }
        Ok(())
    }
    #[cfg(not(test))]
    fn fail_point(&self, _point: &str) -> std::io::Result<()> { Ok(()) }
}

/// CURRENT's content: `base-<n>`, or `dataset-1 base-<n>` once fenced. `None` if it is neither.
///
/// RDF 1.2 (task #101): before the first WAL record holding a triple term or a directional literal, CURRENT gains the
/// prefix `rdf12-1 ` (`rdf12-1 dataset-1 base-<n>` with both fences). Binaries without RDF 1.2 support then refuse
/// the store before reading its WAL, instead of truncating it at the first line they cannot parse. Returns
/// `(n, dataset fence, RDF 1.2 fence)`, `Ok(None)` if the text is malformed, and an error for an RDF 1.2 store in a
/// build without the `rdf-12` feature.
fn parse_current(s: &str) -> std::io::Result<Option<(u64, bool, bool)>> {
    let s = s.trim();
    let (rdf12, s) = match s.strip_prefix("rdf12-1 ") { Some(rest) => (true, rest), None => (false, s) };
    if rdf12 && !cfg!(feature = "rdf-12") {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "this store holds RDF 1.2 terms; open it with a build that has the rdf-12 feature"));
    }
    let (fenced, base) = match s.strip_prefix("dataset-1 ") { Some(rest) => (true, rest), None => (false, s) };
    let Some(n) = base.strip_prefix("base-") else { return Ok(None) };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { return Ok(None) }
    Ok(n.parse().ok().map(|n| (n, fenced, rdf12)))
}

/// Atomically points CURRENT at `base-<n>` (with the dataset fence if `fenced`, the RDF 1.2 fence if `rdf12`).
fn set_current(dir: &Path, n: u64, fenced: bool, rdf12: bool) -> std::io::Result<()> {
    let tmp = dir.join("CURRENT.tmp");
    let mut f = File::create(&tmp)?;
    writeln!(f, "{}{}base-{n}", if rdf12 { "rdf12-1 " } else { "" }, if fenced { "dataset-1 " } else { "" })?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join("CURRENT"))?;
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> std::io::Result<()> { File::open(dir)?.sync_all() }

/// Refuses a term whose N-Triples text (what the WAL writes and a base stores) is longer than a base can hold.
/// Without this, the commit would be acknowledged, but no compaction could ever write the store's next base.
/// Checked before anything is applied, so a refused update changes nothing.
fn check_term_size(term: triplelight_rdf::TermRef<'_>) -> std::io::Result<()> {
    // RDF 1.2: a base parses its dictionary text back with `Term::from_str`, which reads at most
    // `MAX_TRIPLE_TERM_DEPTH` nested triple terms; a deeper one would block every later compaction.
    #[cfg(feature = "rdf-12")]
    {
        let depth = triplelight_rdf::triple_term_depth(term);
        if depth > triplelight_rdf::MAX_TRIPLE_TERM_DEPTH {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("triple term nested {depth} deep exceeds the limit of {}", triplelight_rdf::MAX_TRIPLE_TERM_DEPTH)));
        }
    }
    // The same limit as the base writer (`base_format::MAX_TERM`, Unix only) and the streaming parsers.
    let limit = triplelight_core::ntriples::MAX_TOKEN;
    // Cheap upper bound first: escaping turns one byte into at most six (`\u0000`), plus quotes and the suffix.
    let bound = match term {
        triplelight_rdf::TermRef::NamedNode(n) => n.as_str().len() + 2,
        triplelight_rdf::TermRef::BlankNode(b) => b.as_str().len() + 2,
        triplelight_rdf::TermRef::Literal(l) => l.value().len() * 6 + l.datatype().as_str().len() + l.language().map_or(0, str::len) + 8,
        // Nested parts make any cheap bound loose; triple terms are rare, so render them.
        #[cfg(feature = "rdf-12")]
        triplelight_rdf::TermRef::Triple(_) => usize::MAX,
    };
    if bound <= limit { return Ok(()) }
    let len = term.to_string().len();
    if len > limit {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("RDF term of {len} bytes exceeds the {limit}-byte limit")));
    }
    Ok(())
}
fn check_triple_size(t: &Triple) -> std::io::Result<()> {
    check_term_size(t.subject.as_ref().into())?;
    check_term_size(t.predicate.as_ref().into())?;
    check_term_size(t.object.as_ref())
}

/// The count a commit returns: the net change (the WAL record's length, which `commit` and `update_with` have always
/// reported), or the sum of each step's changed triples (`commit_dataset`, `update_dataset_with`).
#[derive(Clone, Copy)]
enum Count { Net, Steps }

/// Default-graph triple changes as dataset steps.
fn default_changes(changes: &[(Change, Triple)]) -> Vec<DatasetChange> {
    changes.iter().map(|(c, t)| DatasetChange::Quad(GraphTarget::Default, *c, t.clone())).collect()
}

impl Overlay {
    /// A read snapshot over `base` with no pending changes: the starting point of an in-memory, non-durable store
    /// (used by the WebAssembly build, which has no file system).
    pub fn from_base(base: Arc<dyn Snapshot>) -> Overlay { Overlay { base, delta: Arc::new(Delta::default()), named: Default::default(), object_memo: Default::default() } }

    /// This snapshot with `changes` applied, in memory and without a WAL, plus the number of triples that actually
    /// changed.
    pub fn with_changes(&self, changes: &[(Change, Triple)]) -> std::io::Result<(Overlay, usize)> {
        // The count is the net change (the WAL record's length), as before the dataset path existed.
        let (next, _, rec) = self.apply_dataset(&default_changes(changes), true)?;
        Ok((next, rec.len()))
    }
}


/// One WAL line for a triple change, in the default graph (`graph == None`) or a named graph.
fn push_quad(buf: &mut String, c: Change, t: &Triple, graph: Option<&triplelight_rdf::NamedNode>) {
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

/// Whether a record needs the RDF 1.2 fence: it holds a triple term or a directional literal. Both occur only as
/// objects (a directional literal inside a triple term makes the object a triple term).
#[cfg(feature = "rdf-12")]
fn needs_rdf12_fence(rec: &[DatasetChange]) -> bool {
    rec.iter().any(|c| match c {
        DatasetChange::Quad(_, _, t) => match &t.object {
            Term::Triple(_) => true,
            Term::Literal(l) => l.direction().is_some(),
            _ => false,
        },
        _ => false,
    })
}
#[cfg(not(feature = "rdf-12"))]
fn needs_rdf12_fence(_rec: &[DatasetChange]) -> bool { false }

/// Whether a record needs the dataset fence (it mentions a named graph).
fn needs_fence(rec: &[DatasetChange]) -> bool {
    rec.iter().any(|c| !matches!(c, DatasetChange::Quad(GraphTarget::Default, ..)))
}

/// One WAL line as a replayable step, or `None` if it is not a valid change line (a torn tail).
fn parse_line(line: &str) -> Option<DatasetChange> {
    let graph = |rest: &str| rest.strip_prefix('<')?.strip_suffix('>').and_then(|iri| triplelight_rdf::NamedNode::new(iri).ok());
    if let Some(rest) = line.strip_prefix("G+ ") { return Some(DatasetChange::Create { graph: graph(rest)?, silent: true }) }
    if let Some(rest) = line.strip_prefix("G- ") {
        return Some(DatasetChange::Drop { target: GraphScope::Graph(graph(rest)?), silent: true });
    }
    let (c, rest) = match line.split_at_checked(2) {
        Some(("+ ", r)) => (Change::Insert, r),
        Some(("- ", r)) => (Change::Delete, r),
        _ => return None,
    };
    let quad = triplelight_core::ntriples::parse_nquads_line(rest)?;
    let target = match quad.graph_name {
        triplelight_rdf::GraphName::DefaultGraph => GraphTarget::Default,
        triplelight_rdf::GraphName::NamedNode(g) => GraphTarget::Named(g),
        triplelight_rdf::GraphName::BlankNode(_) => return None,
    };
    Some(DatasetChange::Quad(target, c, Triple::new(quad.subject, quad.predicate, quad.object)))
}

/// Complete records in the WAL, and the byte length they span (everything after is a torn tail).
fn read_wal(path: &Path) -> std::io::Result<(Vec<Vec<DatasetChange>>, u64)> {
    let Ok(f) = File::open(path) else { return Ok((Vec::new(), 0)) };
    let mut records = Vec::new();
    let (mut cur, mut pos, mut valid) = (Vec::new(), 0u64, 0u64);
    let mut reader = BufReader::new(f);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 { break }
        // A line without its newline is the torn end of an append, even if it reads as a complete change or `C`.
        if line.pop() != Some(b'\n') { break }
        pos += line.len() as u64 + 1;
        let Ok(text) = std::str::from_utf8(&line) else { break };
        if text == "C" { records.push(std::mem::take(&mut cur)); valid = pos; continue }
        match parse_line(text) { Some(change) => cur.push(change), None => break }
    }
    Ok((records, valid))
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
mod bare_base_tests {
    use super::*;
    /// A directory written by `build-disk` (a MANIFEST, no CURRENT) is refused, and left untouched, instead of being
    /// opened as a new empty store.
    #[test]
    fn opening_a_bare_base_as_a_store_is_refused() {
        let dir = std::env::temp_dir().join(format!("rdf-bare-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST"), "RDFSNAP6\n").unwrap();
        let err = Store::open(&dir, None, 1).err().expect("a bare base is not a store");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("build-disk"), "{err}");
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, ["MANIFEST"], "nothing was written into the base directory");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod borrowed_overlay_tests {
    use super::*;
    use triplelight_rdf::{NamedNode, Triple};

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
    use triplelight_rdf::{BlankNode, Literal, NamedNode};
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

    /// A failure after CURRENT may have switched poisons the store: no commit is acknowledged on a stale WAL handle
    /// and no retry runs against a stale generation. Reopening recovers everything acknowledged before.
    #[test]
    fn failed_compaction_switch_poisons_until_reopen() {
        for (point, named) in [("after-wal-rename", true), ("after-current", false)] {
            let dir = Dir::new(point);
            let store = dir.open();
            if named { store.commit_dataset(&sample()).unwrap(); } else { store.commit(&[(Change::Insert, t("d", "0"))]).unwrap(); }
            let want = state(&store.snapshot());
            *store.fail_at.lock().unwrap() = Some(point.to_string());
            assert!(store.compact().is_err(), "{point}");
            *store.fail_at.lock().unwrap() = None;
            assert!(store.commit(&[(Change::Insert, t("lost", "1"))]).is_err(), "{point}: commit after a failed switch");
            assert!(store.commit_dataset(&[ins("g9", t("lost", "2"))]).is_err(), "{point}");
            assert!(store.compact().is_err(), "{point}: compaction retry after a failed switch");
            drop(store);
            let reopened = dir.open();
            assert_eq!(state(&reopened.snapshot()), want, "{point}");
            assert!(dir.current().trim().ends_with("base-1"), "{point}: {}", dir.current());
            reopened.commit_dataset(&[ins_default(t("after", "1"))]).unwrap();
            let want = state(&reopened.snapshot());
            drop(reopened);
            assert_eq!(state(&dir.open().snapshot()), want, "{point}");
        }
    }

    /// A final `C` without its newline is a torn record: it is dropped, the file is cut back to the last complete record
    /// (no padding), and a later append stays readable.
    #[test]
    fn commit_marker_needs_its_newline() {
        let dir = Dir::new("torn-c");
        let store = dir.open();
        store.commit_dataset(&sample()).unwrap();
        let want = state(&store.snapshot());
        drop(store);
        let good = dir.wal();
        let mut torn = good.clone();
        torn.extend_from_slice(b"G+ <http://ex/torn>\nC");
        std::fs::write(dir.0.join("wal.log"), &torn).unwrap();
        let store = dir.open();
        assert_eq!(state(&store.snapshot()), want);
        assert_eq!(dir.wal(), good, "cut back exactly, without padding");
        store.commit_dataset(&[ins("g2", t("next", "1"))]).unwrap();
        let want = state(&store.snapshot());
        drop(store);
        assert_eq!(state(&dir.open().snapshot()), want);
        assert!(!want.1.iter().any(|(g, _)| g.contains("torn")));
    }

    /// The disk store with named graphs in its base (step 3c): random updates to the default graph and four named
    /// graphs (inserts, deletes, CREATE, CLEAR, DROP, CLEAR DEFAULT), with compactions and reopens in between, always
    /// match a set model. After a compaction the named graphs are in an RDFSNAP5 base and the WAL is empty.
    #[cfg(unix)]
    #[test]
    fn disk_store_named_graphs_in_the_base_match_a_model() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use std::collections::{BTreeMap, BTreeSet};
        let dir = Dir::new("named-base-model");
        let open = || Store::open_with(&dir.0, None, Box::new(crate::DiskBase { workers: 1 })).unwrap();
        let mut store = open();
        let mut default: BTreeSet<String> = BTreeSet::new();
        let mut graphs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut rng = 0x9e3779b97f4a7c15u64;
        let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
        let model_state = |default: &BTreeSet<String>, graphs: &BTreeMap<String, BTreeSet<String>>| -> (Vec<String>, Vec<(String, Vec<String>)>) {
            (default.iter().cloned().collect(), graphs.iter().map(|(g, rows)| (format!("<http://ex/{g}>"), rows.iter().cloned().collect())).collect())
        };
        let row = |tr: &Triple| tr.to_string().trim_end_matches(" .").to_string();
        for round in 0..60 {
            let mut steps = Vec::new();
            for _ in 0..(1 + next() % 12) {
                let r = next();
                let g = format!("g{}", r % 4);
                let tr = t(&format!("s{}", (r >> 8) % 9), &format!("{}", (r >> 16) % 5));
                match (r >> 24) % 20 {
                    0 => { steps.push(Clear { target: GraphScope::Graph(iri(&g)), silent: true }); if let Some(rows) = graphs.get_mut(&g) { rows.clear() } }
                    1 => { steps.push(Drop { target: GraphScope::Graph(iri(&g)), silent: true }); graphs.remove(&g); }
                    2 => { steps.push(Create { graph: iri(&g), silent: true }); graphs.entry(g).or_default(); }
                    3 if round % 7 == 0 => { steps.push(Clear { target: GraphScope::Default, silent: false }); default.clear(); }
                    4..=6 => { steps.push(ins_default(tr.clone())); default.insert(row(&tr)); }
                    7..=8 => { steps.push(Quad(GraphTarget::Default, Change::Delete, tr.clone())); default.remove(&row(&tr)); }
                    9..=11 => { steps.push(del(&g, tr.clone())); if let Some(rows) = graphs.get_mut(&g) { rows.remove(&row(&tr)); } }
                    _ => { steps.push(ins(&g, tr.clone())); graphs.entry(g).or_default().insert(row(&tr)); }
                }
            }
            store.commit_dataset(&steps).unwrap();
            assert_eq!(state(&store.snapshot()), model_state(&default, &graphs), "round {round}");
            if round % 10 == 9 {
                store.compact().unwrap();
                if !graphs.is_empty() {
                    assert_eq!(dir.wal(), Vec::<u8>::new(), "round {round}: compaction left the named graphs in the WAL");
                    let base = format!("base-{}", dir.current().trim().rsplit("base-").next().unwrap());
                    assert_eq!(&std::fs::read(dir.0.join(base).join("MANIFEST")).unwrap()[..8], b"RDFSNAP5");
                }
                assert_eq!(state(&store.snapshot()), model_state(&default, &graphs), "round {round} after compaction");
            }
            if round % 6 == 5 {
                drop(store);
                store = open();
                assert_eq!(state(&store.snapshot()), model_state(&default, &graphs), "round {round} after reopen");
            }
        }
    }

    /// A store from step 2 (named graphs only in the WAL, over an RDFSNAP4 base) moves them into the base at its next
    /// compaction; the crash window between the CURRENT switch and the WAL truncation recovers from the old WAL.
    #[cfg(unix)]
    #[test]
    fn named_graphs_move_from_the_wal_into_the_base() {
        let dir = Dir::new("named-wal-to-base");
        let open = || Store::open_with(&dir.0, None, Box::new(crate::DiskBase { workers: 1 })).unwrap();
        let store = open();
        store.commit_dataset(&sample()).unwrap();
        store.commit_dataset(&[del("g1", t("a", "1")), ins("g2", t("c", "3"))]).unwrap();
        let want = state(&store.snapshot());
        let old_wal = dir.wal();
        assert!(!old_wal.is_empty());
        store.compact().unwrap();
        assert_eq!(dir.wal(), Vec::<u8>::new());
        assert_eq!(state(&store.snapshot()), want);
        assert!(store.snapshot().named_graphs_match_base());
        // Nothing changed: a second compaction is a no-op.
        store.compact().unwrap();
        assert_eq!(dir.current().trim(), "dataset-1 base-1");
        drop(store);
        assert_eq!(state(&open().snapshot()), want);
        // New base with the old WAL (crash before truncation): replaying the history again changes nothing.
        std::fs::write(dir.0.join("wal.log"), &old_wal).unwrap();
        assert_eq!(state(&open().snapshot()), want);
    }

    /// Task #45 step 2: every line the current binary writes to the WAL parses exactly as `oxttl` parsed it, and the
    /// store replays to the same state; malformed complete lines are refused by both parsers.
    #[test]
    fn wal_lines_parse_as_oxttl_did() {
        let oxttl_line = |rest: &str| {
            let mut q = oxttl::NQuadsParser::new().for_slice(rest.as_bytes());
            match (q.next(), q.next()) { (Some(Ok(quad)), None) => Some(quad.to_string()), _ => None }
        };
        let dir = Dir::new("wal-parser-parity");
        let store = dir.open();
        let x = BlankNode::new("x").unwrap();
        let lit = |l: Literal| Triple::new(iri("s"), iri("p"), l);
        store.commit_dataset(&sample()).unwrap();
        store.commit_dataset(&[
            ins_default(lit(Literal::new_simple_literal("tab\tnew\nline \"quote\" back\\slash é \u{1F600} \u{0}"))),
            ins_default(lit(Literal::new_language_tagged_literal("hello", "en-GB").unwrap())),
            ins_default(lit(Literal::new_typed_literal("0042", NamedNode::new("http://www.w3.org/2001/XMLSchema#integer").unwrap()))),
            ins("g3", Triple::new(x.clone(), NamedNode::new("http://ex/é/%20?q#f").unwrap(), x.clone())),
            del("g1", t("a", "1")),
            Drop { target: GraphScope::Graph(iri("empty")), silent: false },
        ]).unwrap();
        let before = state(&store.snapshot());
        drop(store);
        let wal = String::from_utf8(dir.wal()).unwrap();
        let mut quad_lines = 0;
        for line in wal.lines() {
            if let Some(rest) = line.strip_prefix("+ ").or_else(|| line.strip_prefix("- ")) {
                assert_eq!(triplelight_core::ntriples::parse_nquads_line(rest).map(|q| q.to_string()), oxttl_line(rest), "{line:?}");
                assert!(parse_line(line).is_some(), "{line:?}");
                quad_lines += 1;
            }
        }
        assert!(quad_lines >= 8, "only {quad_lines} quad lines");
        assert_eq!(state(&dir.open().snapshot()), before);
        for bad in ["<http://a> <http://b> <http://c>", "<http://a> <http://b> \"x", "<http://a> <http://b> <http://c> . extra", "_:a <http://b> <http://c> _:g ."] {
            let ours = triplelight_core::ntriples::parse_nquads_line(bad);
            assert_eq!(ours.as_ref().map(|q| q.to_string()), oxttl_line(bad), "{bad:?}");
            if bad.ends_with("_:g .") { assert!(parse_line(&format!("+ {bad}")).is_none()) } else { assert!(ours.is_none(), "{bad:?}") }
        }
    }

    /// A WAL record with a term over the streaming parsers' 16 MiB token limit, which older binaries accepted on replay
    /// (`oxttl`'s `for_slice` had no limit), still replays, and so does the record after it (task #45, PR #94 review).
    #[test]
    #[cfg(unix)]
    fn oversized_wal_records_survive_reopen() {
        let dir = Dir::new("wal-oversized");
        let open = || Store::open_with(&dir.0, None, Box::new(crate::DiskBase { workers: 1 })).unwrap();
        let store = open();
        let big = "x".repeat(triplelight_core::ntriples::MAX_TOKEN + 1024);
        // Commits now refuse such a term (#95), so the legacy record is written as an older binary wrote it.
        store.commit(&[(Change::Insert, t("before", "0"))]).unwrap();
        drop(store);
        let mut legacy = String::new();
        push_record(&mut legacy, &[ins_default(t("big", &big))]);
        let mut wal = dir.wal();
        wal.extend_from_slice(legacy.as_bytes());
        std::fs::write(dir.0.join("wal.log"), &wal).unwrap();
        let store = open();
        store.commit_dataset(&[ins_default(t("after", "1")), ins("g", t("after", "2"))]).unwrap();
        let before = state(&store.snapshot());
        drop(store);
        assert!(dir.wal().len() > triplelight_core::ntriples::MAX_TOKEN);
        let store = open();
        assert_eq!(state(&store.snapshot()), before);
        assert_eq!(state(&store.snapshot()).0.len(), 3);
        // No base can hold the term, so compaction fails and changes nothing; deleting it (allowed, unlike inserting
        // it) makes the store compactable again.
        assert!(store.compact().is_err());
        assert_eq!(state(&store.snapshot()), before);
        store.commit(&[(Change::Delete, t("big", &big))]).unwrap();
        store.compact().unwrap();
        assert_eq!(state(&store.snapshot()).0.len(), 2);
        drop(store);
        assert_eq!(state(&open().snapshot()).0.len(), 2);
    }

    /// A term longer than a base can hold is refused at commit, before anything reaches the WAL, on both commit paths;
    /// a term just under the limit commits, survives reopen and compacts.
    #[cfg(unix)]
    #[test]
    fn oversized_terms_are_refused_at_commit() {
        let limit = crate::base_format::MAX_TERM;
        let dir = Dir::new("term-limit");
        let open = || Store::open_with(&dir.0, None, Box::new(crate::DiskBase { workers: 1 })).unwrap();
        let store = open();
        store.commit(&[(Change::Insert, t("before", "1"))]).unwrap();
        let wal = dir.wal();
        let big = Triple::new(iri("s"), iri("p"), Literal::new_simple_literal("x".repeat(limit)));
        let e = store.commit(&[(Change::Insert, t("same-update", "2")), (Change::Insert, big.clone())]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        assert!(store.commit_dataset(&[ins("g", big.clone())]).is_err());
        // Escapes count: a value under the limit in bytes whose escaped text is over it.
        let escaped = Triple::new(iri("s"), iri("p"), Literal::new_simple_literal("\u{0}".repeat(limit / 4)));
        assert!(store.commit(&[(Change::Insert, escaped)]).is_err());
        assert!(store.commit_dataset(&[Create { graph: NamedNode::new(format!("http://ex/{}", "g".repeat(limit))).unwrap(), silent: false }]).is_err());
        assert_eq!(dir.wal(), wal, "nothing of a refused update reaches the WAL");
        // Removing data is never refused: deleting an oversized triple, or dropping a graph with an oversized name, is
        // allowed (here a no-op), since a legacy WAL record may have put such a term in the store.
        assert_eq!(store.commit(&[(Change::Delete, big.clone())]).unwrap(), 0);
        let long_graph = NamedNode::new(format!("http://ex/{}", "g".repeat(limit))).unwrap();
        assert_eq!(store.commit_dataset(&[Drop { target: GraphScope::Graph(long_graph.clone()), silent: true },
                                         Clear { target: GraphScope::Graph(long_graph), silent: true },
                                         Quad(GraphTarget::Named(iri("g")), Change::Delete, big.clone())]).unwrap(), 0);
        assert_eq!(dir.wal(), wal, "no-op removals write nothing");
        let fits = Triple::new(iri("s"), iri("p"), Literal::new_simple_literal("y".repeat(limit - 16)));
        store.commit(&[(Change::Insert, fits.clone())]).unwrap();
        let want = state(&store.snapshot());
        drop(store);
        let store = open();
        assert_eq!(state(&store.snapshot()), want);
        store.compact().unwrap();
        assert_eq!(state(&store.snapshot()), want);
    }

    /// A large derived-order regroup on a store spills into `<store>/scratch/` (beside the bases, never inside one),
    /// leaves nothing there once the scan is done, and the next open removes the directory.
    #[cfg(unix)]
    #[test]
    fn derived_order_spill_uses_the_store_scratch() {
        use crate::model::Perm;
        let dir = std::env::temp_dir().join(format!("rdf-spill-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let open = || Store::open_with(&dir, None, Box::new(crate::DiskBase { workers: 1 })).unwrap();
        let iri = |s: String| triplelight_rdf::NamedNode::new(format!("http://ex/{s}")).unwrap();
        {
            let store = open();
            let changes: Vec<_> = (0..3000).map(|i| (Change::Insert, Triple::new(iri(format!("s{i}")), iri(format!("p{}", i % 3)), iri("o".into())))).collect();
            store.commit(&changes).unwrap();
            store.compact().unwrap();
            crate::derived_order::spill_on_this_thread(Some(1), None);
            let snap = store.snapshot();
            let o = snap.lookup(&iri("o".into()).into()).unwrap().unwrap();
            let rows: usize = snap.base().scan(Perm::Ops, &[o]).map(|b| b.unwrap().len()).sum();
            crate::derived_order::spill_on_this_thread(None, None);
            assert_eq!(rows, 3000);
            let scratch = dir.join(SCRATCH_DIR);
            assert!(scratch.is_dir(), "the spill used the store's scratch directory");
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "nothing left once the scan is done");
            let generation = std::fs::read_to_string(dir.join("CURRENT")).unwrap();
            let base = dir.join(generation.trim().rsplit(' ').next().unwrap());
            assert!(std::fs::read_dir(&base).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".rdf-")), "the base is untouched");
        }
        drop(open());
        assert!(!dir.join(SCRATCH_DIR).exists(), "open empties the scratch");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn current_parsing() {
        assert_eq!(parse_current("base-7\n").unwrap(), Some((7, false, false)));
        assert_eq!(parse_current("dataset-1 base-12\n").unwrap(), Some((12, true, false)));
        for bad in ["", "base-", "base-+1", "dataset-2 base-1", "dataset-1base-1", "dataset-1 ", "base-1 extra", "dataset-1 rdf12-1 base-1", "rdf12-1base-1"] {
            assert_eq!(parse_current(bad).unwrap(), None, "{bad:?}");
        }
        // The RDF 1.2 fence (task #101): read by an rdf-12 build, refused with a clear error by any other.
        for (text, expect) in [("rdf12-1 base-3", (3, false, true)), ("rdf12-1 dataset-1 base-4", (4, true, true))] {
            if cfg!(feature = "rdf-12") {
                assert_eq!(parse_current(text).unwrap(), Some(expect));
            } else {
                assert_eq!(parse_current(text).unwrap_err().kind(), std::io::ErrorKind::Unsupported);
            }
        }
    }
}
