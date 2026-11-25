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
//!
//! WAL batches (task #114): a crash can tear only the last append, but a bad block in the middle of the log would read
//! as a torn tail too, and recovery would truncate every acknowledged record after it. So each fsync's records are
//! written as one batch, a line `B <len> <hash>` followed by `len` bytes of records, where `hash` is the first 16 hex
//! digits of the payload's SHA-256. A bad or incomplete batch with no valid batch after it is a torn tail (dropped as
//! before); one followed by a valid batch is damage, and open refuses the store, naming the offset, without changing
//! it. The first batch is preceded by the fence `walcrc-1 ` at the front of CURRENT, so older binaries refuse the
//! directory instead of truncating batches they cannot parse. Records written before the fence (a bare `C` after each)
//! still replay. Damage among those followed by batches is refused like damage before a batch, which is stricter than
//! before (it used to truncate there); damage among them with nothing after still reads as a torn tail. Bit rot in the
//! last acknowledged batch looks like a torn tail too and is dropped: telling them apart would need a trailer after it.
use crate::model::{DatasetChange, GraphScope, GraphTarget, Perm, TermId, TripleBatch};
use crate::storage::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use triplelight_rdf::{Term, TermRef, Triple};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

mod group_commit;
mod named;
#[cfg(unix)]
pub(crate) mod text_postings;
pub use named::{GraphStateError, NamedGraph};
#[cfg(test)]
mod fault_tests;

/// Test-only fault injection at named points of the write path (`fault_tests.rs` drives it). A point armed in the calling
/// thread either returns an I/O error there (as a full disk or a failed fsync would) or ends the process on the spot,
/// as a crash does: no destructor runs and no buffered write is flushed.
///
/// Arming is per thread. With group commit, whichever caller holds the WAL lock writes for everyone
/// (`flush_until`), so a test that commits from several threads needs a process-wide arm, or the fault fires on a
/// thread that did not arm it, or not at all.
#[cfg(test)]
pub(crate) mod fault {
    use std::cell::RefCell;
    #[derive(Clone, Copy, Debug)]
    pub enum Mode { NoSpace, Exit }
    thread_local! { static ARMED: RefCell<Option<(String, Mode)>> = const { RefCell::new(None) }; }
    pub fn arm(point: &str, mode: Mode) { ARMED.with(|a| *a.borrow_mut() = Some((point.to_string(), mode))) }
    pub fn disarm() { ARMED.with(|a| *a.borrow_mut() = None) }
    /// Exit status of a process ended at a fault point.
    pub const EXIT: i32 = 86;
    pub fn hit(point: &str) -> std::io::Result<()> {
        let armed = ARMED.with(|a| a.borrow().clone());
        match armed {
            Some((p, Mode::Exit)) if p == point => {
                eprintln!("fault: exit at {point}");
                std::process::exit(EXIT)
            }
            // ENOSPC is 28 on Linux and macOS alike.
            Some((p, Mode::NoSpace)) if p == point => Err(std::io::Error::from_raw_os_error(28)),
            _ => Ok(()),
        }
    }
}
#[cfg(not(test))]
mod fault {
    #[inline(always)]
    pub fn hit(_point: &str) -> std::io::Result<()> { Ok(()) }
}

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
    /// No change among `perm`'s rows that start with `prefix`, so the base alone answers that range (#111): two binary
    /// searches. A prefix holding an ID first seen in the delta is exact too: rows with that ID exist only as adds.
    fn untouched(&self, perm: Perm, prefix: &[TermId]) -> bool { untouched(&self.adds[slot(perm)], &self.dels[slot(perm)], prefix) }
}

/// `adds` and `dels` (sorted rows of one order) have nothing starting with `prefix`.
fn untouched(adds: &[[TermId; 3]], dels: &[[TermId; 3]], prefix: &[TermId]) -> bool {
    range(adds, prefix).is_empty() && range(dels, prefix).is_empty()
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
    /// Nothing to compact: no pending changes, and the named graphs are exactly the base's.
    pub(crate) fn matches_base(&self) -> bool { self.delta.is_empty() && self.named_graphs_match_base() }
    pub fn delta_len(&self) -> usize { self.delta.len() }
    #[cfg(test)]
    pub(crate) fn base(&self) -> &dyn Snapshot { &*self.base }
    /// The base's triples in SPO order, ignoring the delta: statistics describe the base, and `base_id` says which base.
    pub fn scan_base_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> { self.base.scan_for_stats() }
    /// The number of triples in the base alone.
    pub fn base_triples(&self) -> Result<u64, StorageError> { self.base.count(Perm::Spo, &[]) }
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
    fn text_candidates(&self, tokens: &[&str], limit: usize) -> Option<triplelight_core::storage::TextCandidates> {
        if !self.matches_base() { return None; }
        self.base.text_candidates(tokens, limit)
    }
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        if self.delta.untouched(perm, prefix) { self.base.matching_rows(perm, prefix) } else { None }
    }
    fn matching_tail(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> {
        if self.delta.untouched(perm, prefix) { self.base.matching_tail(perm, prefix) } else { None }
    }

    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        merged_scan(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix)
    }

    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        if self.delta.is_empty() { return self.base.scan_for_stats(); }
        let (adds, dels) = (range(&self.delta.adds[slot(Perm::Spo)], &[]), range(&self.delta.dels[slot(Perm::Spo)], &[]));
        Box::new(MergeScan { base: self.base.scan_for_stats(), cur: None, at: 0, adds, ai: 0, dels, done: false })
    }

    fn scan_from<'a>(&'a self, perm: Perm, prefix: &[TermId], from: TermId) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        if self.delta.untouched(perm, prefix) { return self.base.scan_from(perm, prefix, from) }
        triplelight_core::storage::scan_from_by_scan(self, perm, prefix, from)
    }

    fn first_keys(&self, perm: Perm) -> Result<Vec<TermId>, StorageError> {
        if self.delta.is_empty() { return self.base.first_keys(perm) }
        triplelight_core::storage::first_keys_by_scan(self, perm)
    }

    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        merged_count(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix)
    }

    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        merged_seek(&*self.base, &self.delta.adds[slot(perm)], &self.delta.dels[slot(perm)], perm, prefix, target)
    }
}

/// `base` with the sorted rows `adds` added and `dels` removed (one order; `adds` absent from `base`, `dels` present
/// in it): the scan, count and seek that the default graph and every stored named graph share.
fn merged_scan<'a>(base: &'a dyn Snapshot, adds: &'a [[TermId; 3]], dels: &'a [[TermId; 3]], perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
    let (adds, dels) = (range(adds, prefix), range(dels, prefix));
    // A range the changes don't touch keeps the base's own scan (no merge step per row).
    if adds.is_empty() && dels.is_empty() { return base.scan(perm, prefix) }
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
    // A range the changes don't touch: the base's seek, without a count per candidate value.
    if untouched(adds, dels, prefix) { return base.seek(perm, prefix, target) }
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

/// Takes a shared directory lock (readers), retrying briefly like `lock_directory`; an open writer holds it exclusively.
fn lock_directory_shared(file: &File) -> std::io::Result<()> {
    let start = std::time::Instant::now();
    let mut pause = std::time::Duration::from_millis(1);
    loop {
        match file.try_lock_shared() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < std::time::Duration::from_millis(250) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_millis(32));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "database directory is open for writing"))
            }
        }
    }
}

/// The store root's scratch directory, beside the bases: large derived-order regroups spill here (`derived_order.rs`).
/// Emptied at every open, which holds the directory lock.
pub(crate) const SCRATCH_DIR: &str = "scratch";

/// Canonical roots of the stores this process holds open read-only. A read-only store writes nothing under its root, not even the `scratch/`
/// directory large derived-order regroups spill into (`derived_order::spill_dir` skips these roots), so it works on a read-only mount.
static READ_ONLY_ROOTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(crate) fn is_read_only_root(root: &Path) -> bool {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    READ_ONLY_ROOTS.lock().unwrap().contains(&canonical)
}

impl Drop for Store {
    fn drop(&mut self) {
        if !self.read_only { return }
        let canonical = std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        let mut roots = READ_ONLY_ROOTS.lock().unwrap();
        if let Some(i) = roots.iter().position(|r| *r == canonical) { roots.remove(i); }
    }
}

/// A persistent store: base file + WAL in a directory.
pub struct Store {
    dir: PathBuf,
    format: Box<dyn BaseFormat>,
    /// Number of the active base directory.
    generation: Mutex<u64>,
    /// The fences CURRENT carries (see the module docs and `parse_current`).
    fences: Mutex<Fences>,
    /// What readers see: everything durable.
    current: RwLock<Arc<Overlay>>,
    /// Accepted change sets, possibly not yet durable (group commit).
    pending: Mutex<Pending>,
    /// Signalled whenever a record joins `pending` (the adaptive gather in `lead_flush`).
    enqueued: std::sync::Condvar,
    /// The sequence number of the last durable record; set under `pending` by `publish`.
    durable: std::sync::atomic::AtomicU64,
    /// Signalled when a leader steps down, after `durable` grew or the store was poisoned (`flush_until` waits on it).
    flushed: std::sync::Condvar,
    /// The open WAL; whoever holds it flushes all pending records with one fsync.
    wal: Mutex<Wal>,
    /// Serializes update requests (read-compute-enqueue), so each sees all earlier ones.
    writer: Mutex<()>,
    /// Set if a WAL write or fsync failed: in-memory and durable state may differ, so further writes are refused.
    poisoned: std::sync::atomic::AtomicBool,
    /// Opened by `open_read_only`: the directory lock is shared, nothing was written at open, and every write is refused.
    read_only: bool,
    /// Held until the other store state is dropped; the persistent file must never be removed on close.
    _directory_lock: File,
    #[cfg(test)]
    fail_at: Mutex<Option<String>>,
    /// Waits in `flush_until` that ended on the safety timeout while a leader was still marked leading (tests).
    #[cfg(test)]
    timeout_wakes: std::sync::atomic::AtomicU64,
}

/// `records`: how many records `buf` holds; `leading`: a leader is flushing (`group_commit`).
struct Pending { latest: Arc<Overlay>, buf: String, seq: u64, records: usize, leading: bool }
/// `last_batch`: records in the last batch written; `last_sync`: how long its write and fsync took.
struct Wal { file: File, last_batch: usize, last_sync: std::time::Duration }

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
        let (generation, fences) = match std::fs::read_to_string(dir.join("CURRENT")) {
            Ok(s) => parse_current(&s)?.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "corrupt CURRENT"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let base0 = dir.join("base-0");
                // No CURRENT means no base was ever committed: a `base-0` here is an abandoned earlier attempt.
                if base0.exists() { std::fs::remove_dir_all(&base0)?; }
                format.import(&base0, initial)?;
                fault::hit("init-after-import")?;
                set_current(&dir, 0, Fences::default())?;
                (0, Fences::default())
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
        #[cfg(unix)]
        text_postings::sweep(&dir, generation);
        Ok(Store {
            dir, format, generation: Mutex::new(generation), fences: Mutex::new(fences),
            current: RwLock::new(overlay.clone()),
            pending: Mutex::new(Pending { latest: overlay, buf: String::new(), seq: 0, records: 0, leading: false }),
            enqueued: std::sync::Condvar::new(),
            wal: Mutex::new(Wal { file: wal, last_batch: 0, last_sync: std::time::Duration::ZERO }),
            durable: std::sync::atomic::AtomicU64::new(0),
            flushed: std::sync::Condvar::new(),
            writer: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            read_only: false,
            _directory_lock: directory_lock,
            #[cfg(test)]
            fail_at: Mutex::new(None),
            #[cfg(test)]
            timeout_wakes: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Opens an existing store for reading only: a shared lock on the directory (many readers, or one writer), and nothing
    /// is written, truncated or deleted under the store root, not even a spill directory: large regroups spill to
    /// `TRIPLELIGHT_SCRATCH_DIR`, else the system temporary directory, so a read-only mount works. The WAL is replayed up to its last complete record; a torn tail is ignored and left
    /// in place, and a `base-N` that `CURRENT` does not name (a compaction in flight or abandoned) is left alone. Every write
    /// and `compact` fail with `PermissionDenied`. A store that does not exist is an error, never created.
    pub fn open_read_only(dir: impl AsRef<Path>, workers: usize) -> std::io::Result<Self> {
        Self::open_read_only_with(dir, Box::new(NTriplesBase { workers }))
    }

    pub fn open_read_only_with(dir: impl AsRef<Path>, format: Box<dyn BaseFormat>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.join("CURRENT").is_file() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} is not a store (no CURRENT)", dir.display())));
        }
        // Read-only: never create the lock file either. A store that was never opened for writing has none; refuse it.
        let directory_lock = OpenOptions::new().read(true).open(dir.join("LOCK")).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
            std::io::Error::new(e.kind(), format!("{} has no LOCK file: a writer must open this store once before readers can (a read-only open never creates files)", dir.display()))
        } else { e })?;
        lock_directory_shared(&directory_lock)?;
        // `parse_current` reads the fences (batch checksums, RDF 1.2, datasets) and refuses a store this build cannot read, as a writer's open does.
        let (generation, fences) = parse_current(&std::fs::read_to_string(dir.join("CURRENT"))?)?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "corrupt CURRENT"))?;
        let (base, named) = format.open_dataset(&dir.join(format!("base-{generation}")))?;
        let mut overlay = Overlay::from_dataset_base(base, named);
        let wal_path = dir.join("wal.log");
        // Damage before a valid batch is `InvalidData` here as for a writer, and nothing is truncated: a bad final batch is a torn tail that only a
        // writer cuts off, so this open ignores it and leaves the file as it is.
        let (records, _valid_len) = read_wal(&wal_path)?;
        for rec in records { overlay = overlay.with_dataset_changes(&rec)?.0; }
        // A placeholder: no write path reaches it (`check_poisoned` refuses first).
        let wal = File::open(&wal_path).or_else(|_| File::open(dir.join("CURRENT")))?;
        let overlay = Arc::new(overlay);
        READ_ONLY_ROOTS.lock().unwrap().push(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone()));      // registered only once the open succeeded: the Store's Drop removes it
        Ok(Store {
            dir, format, generation: Mutex::new(generation), fences: Mutex::new(fences),
            current: RwLock::new(overlay.clone()),
            pending: Mutex::new(Pending { latest: overlay, buf: String::new(), seq: 0, records: 0, leading: false }),
            enqueued: std::sync::Condvar::new(),
            wal: Mutex::new(Wal { file: wal, last_batch: 0, last_sync: std::time::Duration::ZERO }),
            durable: std::sync::atomic::AtomicU64::new(0),
            flushed: std::sync::Condvar::new(),
            writer: Mutex::new(()),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            read_only: true,
            _directory_lock: directory_lock,
            #[cfg(test)]
            fail_at: Mutex::new(None),
            #[cfg(test)]
            timeout_wakes: std::sync::atomic::AtomicU64::new(0),
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

    pub(crate) fn check_poisoned(&self) -> std::io::Result<()> {
        if self.read_only {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "store was opened read-only"));
        }
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
        // Lock order: `append` takes `fences` then `generation`, `compact` takes `generation` then `fences`. Both hold
        // `pending` first, which serializes them, so the two orders never run at once.
        let mut fences = self.fences.lock().unwrap();
        // Every record from now on reaches the WAL inside a batch, so the batch fence goes first too.
        let needed = Fences { dataset: fences.dataset || needs_fence(&rec), rdf12: fences.rdf12 || needs_rdf12_fence(&rec), walcrc: true };
        if needed != *fences {
            // Durable before any named-graph or RDF 1.2 line, or any batch, can reach the WAL.
            let generation = self.generation.lock().unwrap();
            set_current(&self.dir, *generation, needed)?;
            fault::hit("fence-after-current")?;
            *fences = needed;
        }
        drop(fences);
        push_record(&mut p.buf, &rec);
        p.latest = Arc::new(next);
        p.seq += 1;
        p.records += 1;
        self.enqueued.notify_all();
        Ok((p.seq, n))
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
        if cur.matches_base() { return Ok(()) }
        let mut generation = self.generation.lock().unwrap();
        let next = *generation + 1;
        let new_dir = self.dir.join(format!("base-{next}"));
        // Not named by CURRENT, so at most an abandoned earlier attempt in this process.
        if new_dir.exists() { std::fs::remove_dir_all(&new_dir)?; }
        // Named graphs go into the base when the format holds them (step 3c), otherwise into the new WAL below.
        let into_base = cur.has_named_graphs() && self.format.holds_named_graphs();
        self.format.rewrite(&self.dir.join(format!("base-{}", *generation)), &new_dir, &cur, into_base)?;
        fault::hit("compact-after-base")?;
        // Open (and so validate) the new base before committing to it: if this fails, CURRENT, the WAL and the
        // in-memory state all still describe the old base.
        let (base, named) = self.format.open_dataset(&new_dir)?;
        let fences = *self.fences.lock().unwrap();
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
            f.write_all(&if fences.walcrc { batch(&buf) } else { buf.into_bytes() })?;
            f.sync_all()?;
            fault::hit("compact-after-wal-tmp")?;
            self.publish(|| {
                set_current(&self.dir, next, fences)?;
                std::fs::rename(&tmp, self.dir.join("wal.log"))?;
                self.fail_point("after-wal-rename")?;
                sync_dir(&self.dir)
            })?;
            w.file = f;
            fresh = moved;
        } else {
            self.publish(|| {
                set_current(&self.dir, next, fences)?;
                self.fail_point("after-current")?;
                // The new base is durable and current; only now drop the log.
                w.file.set_len(0)?;
                w.file.sync_all()?;
                fault::hit("compact-after-truncate")?;
                use std::io::Seek;
                w.file.seek(std::io::SeekFrom::Start(0)).map(|_| ())
            })?;
        }
        let fresh = Arc::new(fresh);
        p.latest = fresh.clone();
        *self.current.write().unwrap() = fresh;
        *generation = next;
        #[cfg(unix)]
        text_postings::sweep(&self.dir, next);
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
        fault::hit(point)
    }
    #[cfg(not(test))]
    fn fail_point(&self, _point: &str) -> std::io::Result<()> { Ok(()) }
}

/// CURRENT's content: `base-<n>`, with the fences in `Fences` (`walcrc-1 rdf12-1 dataset-1 base-<n>` with all three)
/// in front. `None` if it is not that.
///
/// RDF 1.2 (task #101): before the first WAL record holding a triple term or a directional literal, CURRENT gains the
/// prefix `rdf12-1 ` (`rdf12-1 dataset-1 base-<n>` with both fences). Binaries without RDF 1.2 support then refuse
/// the store before reading its WAL, instead of truncating it at the first line they cannot parse. Returns
/// `(n, dataset fence, RDF 1.2 fence)`, `Ok(None)` if the text is malformed, and an error for an RDF 1.2 store in a
/// build without the `rdf-12` feature.
fn parse_current(s: &str) -> std::io::Result<Option<(u64, Fences)>> {
    let s = s.trim();
    let (walcrc, s) = match s.strip_prefix("walcrc-1 ") { Some(rest) => (true, rest), None => (false, s) };
    let (rdf12, s) = match s.strip_prefix("rdf12-1 ") { Some(rest) => (true, rest), None => (false, s) };
    if rdf12 && !cfg!(feature = "rdf-12") {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "this store holds RDF 1.2 terms; open it with a build that has the rdf-12 feature"));
    }
    let (fenced, base) = match s.strip_prefix("dataset-1 ") { Some(rest) => (true, rest), None => (false, s) };
    let Some(n) = base.strip_prefix("base-") else { return Ok(None) };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { return Ok(None) }
    Ok(n.parse().ok().map(|n| (n, Fences { dataset: fenced, rdf12, walcrc })))
}

/// The one-way format fences CURRENT can carry, in the order they are written: `walcrc-1 ` (WAL batches), `rdf12-1 `
/// (RDF 1.2 terms), `dataset-1 ` (named graphs). Each makes binaries that predate it refuse the directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Fences { dataset: bool, rdf12: bool, walcrc: bool }

/// Atomically points CURRENT at `base-<n>`, with `fences`.
fn set_current(dir: &Path, n: u64, fences: Fences) -> std::io::Result<()> {
    let tmp = dir.join("CURRENT.tmp");
    fault::hit("current-before-write")?;
    let mut f = File::create(&tmp)?;
    let tag = |on: bool, t: &'static str| if on { t } else { "" };
    writeln!(f, "{}{}{}base-{n}", tag(fences.walcrc, "walcrc-1 "), tag(fences.rdf12, "rdf12-1 "), tag(fences.dataset, "dataset-1 "))?;
    f.sync_all()?;
    fault::hit("current-after-tmp")?;
    std::fs::rename(&tmp, dir.join("CURRENT"))?;
    fault::hit("current-after-rename")?;
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

/// `records` (WAL lines, each record ending in `C`) as one batch: `B <len> <hash>`, then the records (module docs).
fn batch(records: &str) -> Vec<u8> {
    let mut out = format!("B {} {}\n", records.len(), batch_hash(records.as_bytes())).into_bytes();
    out.extend_from_slice(records.as_bytes());
    out
}
fn batch_hash(payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(payload)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The records of a batch that starts at `bytes[0]` (its `B` line), and the batch's length; `None` if the batch is
/// incomplete, fails its hash, or holds anything but complete records.
fn read_batch(bytes: &[u8]) -> Option<(Vec<Vec<DatasetChange>>, usize)> {
    // A header is short; anything longer is not one (and is not scanned to the end of a large log).
    let eol = bytes.iter().take(64).position(|&b| b == b'\n')?;
    let header = std::str::from_utf8(&bytes[..eol]).ok()?;
    let mut fields = header.strip_prefix("B ")?.split(' ');
    let (len, hash) = (fields.next()?, fields.next()?);
    if fields.next().is_some() || len.is_empty() || !len.bytes().all(|b| b.is_ascii_digit()) { return None }
    // The length comes from disk: bounded by what is there before any arithmetic, so a corrupt one cannot overflow.
    let len: usize = len.parse().ok().filter(|&n| n <= bytes.len())?;
    let end = (eol + 1).checked_add(len)?;
    let payload = bytes.get(eol + 1..end)?;
    if batch_hash(payload) != hash { return None }
    let text = std::str::from_utf8(payload).ok()?;
    let (mut records, mut cur) = (Vec::new(), Vec::new());
    for line in text.strip_suffix('\n')?.split('\n') {
        if line == "C" { records.push(std::mem::take(&mut cur)) } else { cur.push(parse_line(line)?) }
    }
    cur.is_empty().then_some((records, end))
}

/// Complete records in the WAL, and the byte length they span (everything after is a torn tail). Records written
/// before the batch fence are lines ending in `C`; later ones come in batches (module docs). Where reading stops, a
/// valid batch further on means the log is damaged, not torn: that is an error, and nothing is truncated.
fn read_wal(path: &Path) -> std::io::Result<(Vec<Vec<DatasetChange>>, u64)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(e),
    };
    let (mut records, mut cur, mut pos, mut valid, mut batched) = (Vec::new(), Vec::new(), 0usize, 0usize, false);
    while pos < bytes.len() {
        if bytes[pos..].starts_with(b"B ") {
            let Some((batch, len)) = read_batch(&bytes[pos..]) else { break };
            records.extend(batch);
            pos += len;
            valid = pos;
            batched = true;
            continue;
        }
        // Once batches start, nothing else is ever written: a line that is not one ends the log.
        if batched { break }
        // A line without its newline is the torn end of an append, even if it reads as a complete change or `C`.
        let Some(eol) = bytes[pos..].iter().position(|&b| b == b'\n') else { break };
        let Ok(text) = std::str::from_utf8(&bytes[pos..pos + eol]) else { break };
        pos += eol + 1;
        if text == "C" { records.push(std::mem::take(&mut cur)); valid = pos; continue }
        match parse_line(text) { Some(change) => cur.push(change), None => break }
    }
    // Torn or damaged? Damage leaves a valid batch after the point where reading stopped.
    let rest = &bytes[valid..];
    let later = (1..rest.len()).filter(|&i| rest[i - 1] == b'\n' && rest[i..].starts_with(b"B ")).find(|&i| read_batch(&rest[i..]).is_some());
    if let Some(i) = later {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!(
            "{} is damaged at byte {valid}: a valid batch follows at byte {}, so this is not a torn tail; nothing was \
             truncated (restore from a backup, or move the WAL aside to open the store without these changes)",
            path.display(), valid + i)));
    }
    Ok((records, valid as u64))
}

#[cfg(test)]
mod wal_batch_tests {
    //! Task #114: WAL batches tell a torn last append from damage in the middle of the log.
    use super::*;
    use triplelight_rdf::NamedNode;
    fn t(k: u32) -> Triple { Triple::new(NamedNode::new(format!("urn:s{k}")).unwrap(), NamedNode::new("urn:p").unwrap(), NamedNode::new("urn:o").unwrap()) }
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rdf-walbatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn triples(store: &Store) -> u64 { store.snapshot().count(Perm::Spo, &[]).unwrap() }
    /// Three commits, three batches; returns the WAL bytes and the offset where each batch starts.
    fn three_batches(d: &Path) -> (Vec<u8>, Vec<usize>) {
        let store = Store::open(d, None, 1).unwrap();
        for k in 0..3 { store.commit(&[(Change::Insert, t(k))]).unwrap() ; }
        drop(store);
        let wal = std::fs::read(d.join("wal.log")).unwrap();
        let starts = (0..wal.len()).filter(|&i| (i == 0 || wal[i - 1] == b'\n') && wal[i..].starts_with(b"B ")).collect::<Vec<_>>();
        assert_eq!(starts.len(), 3, "one batch per commit");
        (wal, starts)
    }

    #[test]
    fn records_from_before_the_fence_replay_and_batches_follow() {
        let d = dir("legacy");
        drop(Store::open(&d, None, 1).unwrap());
        assert_eq!(std::fs::read_to_string(d.join("CURRENT")).unwrap(), "base-0\n", "no fence before the first write");
        // A log as the previous binary wrote it: bare records, each ending in `C`.
        std::fs::write(d.join("wal.log"), format!("+ {} .\nC\n+ {} .\nC\n", t(0), t(1))).unwrap();
        let store = Store::open(&d, None, 1).unwrap();
        assert_eq!(triples(&store), 2, "old records replay");
        store.commit(&[(Change::Insert, t(2))]).unwrap();
        assert!(std::fs::read_to_string(d.join("CURRENT")).unwrap().starts_with("walcrc-1 "), "the fence precedes the first batch");
        drop(store);
        let wal = std::fs::read_to_string(d.join("wal.log")).unwrap();
        assert!(wal.starts_with("+ ") && wal.contains("\nC\nB "), "old records, then a batch: {wal:?}");
        assert_eq!(triples(&Store::open(&d, None, 1).unwrap()), 3);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn damage_before_a_valid_batch_refuses_the_store_and_changes_nothing() {
        let d = dir("damage");
        let (mut wal, starts) = three_batches(&d);
        wal[starts[1] + 20] ^= 0x20; // inside the second batch's records
        std::fs::write(d.join("wal.log"), &wal).unwrap();
        let err = Store::open(&d, None, 1).err().expect("damage is not a torn tail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains(&format!("damaged at byte {}", starts[1])), "{err}");
        assert_eq!(std::fs::read(d.join("wal.log")).unwrap(), wal, "nothing truncated");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_torn_last_batch_is_dropped_at_every_byte() {
        let d = dir("torn");
        let (wal, starts) = three_batches(&d);
        for cut in starts[2]..wal.len() {
            std::fs::write(d.join("wal.log"), &wal[..cut]).unwrap();
            let store = Store::open(&d, None, 1).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
            assert_eq!(triples(&store), 2, "cut at {cut}");
            drop(store);
            assert_eq!(std::fs::metadata(d.join("wal.log")).unwrap().len(), starts[2] as u64, "cut at {cut}");
        }
        // Zeroed bytes inside the last batch (power loss on an unsynced block) are a torn tail too.
        let mut zeroed = wal.clone();
        for b in &mut zeroed[starts[2] + 10..starts[2] + 30] { *b = 0 }
        std::fs::write(d.join("wal.log"), &zeroed).unwrap();
        assert_eq!(triples(&Store::open(&d, None, 1).unwrap()), 2);
        std::fs::remove_dir_all(d).unwrap();
    }

    /// The steps of the updates in `deliverables/wal-fixture/make.sh`, which wrote `fixtures/wal-main.log` with the
    /// binary before #114 (main 5c74061d): two default inserts, CREATE <g1>, an insert into <g1> and one into <g2>, a
    /// default delete, DROP <g2>, a typed default insert.
    fn fixture_steps() -> Vec<Vec<DatasetChange>> {
        use triplelight_rdf::Literal;
        let n = |s: &str| NamedNode::new(s).unwrap();
        let tr = |s: &str, p: &str, o: Term| Triple::new(n(s), n(p), o);
        let b2 = tr("urn:b", "urn:p", Literal::new_language_tagged_literal("2", "en").unwrap().into());
        let def = |c, t| DatasetChange::Quad(GraphTarget::Default, c, t);
        vec![
            vec![def(Change::Insert, tr("urn:a", "urn:p", Literal::new_simple_literal("1").into())), def(Change::Insert, b2.clone())],
            vec![DatasetChange::Create { graph: n("urn:g1"), silent: false }],
            vec![DatasetChange::Quad(GraphTarget::Named(n("urn:g1")), Change::Insert, tr("urn:a", "urn:q", n("urn:b").into())),
                 DatasetChange::Quad(GraphTarget::Named(n("urn:g2")), Change::Insert, tr("urn:c", "urn:q", Literal::new_simple_literal("x").into()))],
            vec![def(Change::Delete, b2)],
            vec![DatasetChange::Drop { target: GraphScope::Graph(n("urn:g2")), silent: false }],
            vec![def(Change::Insert, tr("urn:d", "urn:p", Literal::new_typed_literal("3", n("http://www.w3.org/2001/XMLSchema#integer")).into()))],
        ]
    }

    /// Condition 1 of #114: a log written by the previous binary, with named-graph and catalog records and a torn tail,
    /// replays to the state the same updates give today; then new batches follow it and survive a reopen.
    #[test]
    fn a_log_written_by_the_previous_binary_replays() {
        let d = dir("fixture");
        drop(Store::open(&d, None, 1).unwrap());
        std::fs::write(d.join("CURRENT"), include_str!("store/fixtures/CURRENT-main")).unwrap();
        let mut wal = include_bytes!("store/fixtures/wal-main.log").to_vec();
        wal.extend_from_slice(b"+ <urn:torn> <urn:p> \"cut"); // the previous binary's torn tail: no newline
        std::fs::write(d.join("wal.log"), &wal).unwrap();
        let reference = dir("fixture-reference");
        let fresh = Store::open(&reference, None, 1).unwrap();
        for steps in fixture_steps() { fresh.commit_dataset(&steps).unwrap(); }
        let want = super::dataset_wal_tests::state(&fresh.snapshot());
        let store = Store::open(&d, None, 1).unwrap();
        assert_eq!(super::dataset_wal_tests::state(&store.snapshot()), want, "the old log replays");
        store.commit(&[(Change::Insert, t(9))]).unwrap();
        drop(store);
        let store = Store::open(&d, None, 1).unwrap();
        assert_eq!(triples(&store), triples(&fresh) + 1, "old records, then a batch, after a reopen");
        drop((store, fresh));
        std::fs::remove_dir_all(d).unwrap();
        std::fs::remove_dir_all(reference).unwrap();
    }

    /// Damage in the middle of the log, in each place a batch can break, refuses the store and changes nothing.
    #[test]
    fn mid_log_damage_of_any_kind_refuses_the_store() {
        let cases: [(&str, fn(&mut Vec<u8>, usize)); 3] = [
            // The second batch's length grows past the end of the log (same width, so later offsets hold).
            ("length past EOF", |wal, at| { wal[at + 2] = b'9'; }),
            // Its header is no longer a batch header.
            ("damaged header", |wal, at| { wal[at] = b'X'; }),
            // Its hash no longer matches.
            ("damaged hash", |wal, at| { let eol = at + wal[at..].iter().position(|&b| b == b'\n').unwrap(); wal[eol - 1] ^= 1; }),
        ];
        for (name, damage) in cases {
            let d = dir(&format!("mid-{}", name.replace(' ', "-")));
            let (mut wal, starts) = three_batches(&d);
            damage(&mut wal, starts[1]);
            std::fs::write(d.join("wal.log"), &wal).unwrap();
            let err = Store::open(&d, None, 1).err().unwrap_or_else(|| panic!("{name}: opened"));
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{name}");
            assert_eq!(std::fs::read(d.join("wal.log")).unwrap(), wal, "{name}: nothing truncated");
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    /// Damage among records written before the fence is refused once batches follow it (stricter than before #114,
    /// which truncated there); with nothing after it, it is still a torn tail.
    #[test]
    fn damage_among_old_records_before_batches_is_refused() {
        let d = dir("legacy-damage");
        drop(Store::open(&d, None, 1).unwrap());
        std::fs::write(d.join("wal.log"), format!("+ {} .\nC\n+ {} .\nC\n", t(0), t(1))).unwrap();
        let store = Store::open(&d, None, 1).unwrap();
        store.commit(&[(Change::Insert, t(2))]).unwrap();
        drop(store);
        let mut wal = std::fs::read(d.join("wal.log")).unwrap();
        let second = wal.windows(2).position(|w| w == b"C\n").unwrap() + 2;
        wal[second] = b'?'; // the second old record's change line no longer parses
        std::fs::write(d.join("wal.log"), &wal).unwrap();
        assert_eq!(Store::open(&d, None, 1).err().expect("a batch follows the damage").kind(), std::io::ErrorKind::InvalidData);
        // Without the batch, the same damage is a torn tail, as it always was.
        let batch = wal.windows(3).position(|w| w == b"\nB ").unwrap() + 1;
        std::fs::write(d.join("wal.log"), &wal[..batch]).unwrap();
        assert_eq!(triples(&Store::open(&d, None, 1).unwrap()), 1);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_corrupt_batch_length_is_rejected_without_overflow() {
        for header in ["B 18446744073709551615 0000000000000000\n", "B +12 0000000000000000\n", "B  0000000000000000\n", "B 99999999999999999999999 00\n"] {
            assert!(read_batch(format!("{header}+ <urn:a> <urn:p> <urn:o> .\nC\n").as_bytes()).is_none(), "{header:?}");
        }
    }

    /// The pre-#114 `parse_current`, verbatim: what an older binary does with a fenced CURRENT.
    fn previous_parse_current(s: &str) -> Option<(u64, bool, bool)> {
        let s = s.trim();
        let (rdf12, s) = match s.strip_prefix("rdf12-1 ") { Some(rest) => (true, rest), None => (false, s) };
        let (fenced, base) = match s.strip_prefix("dataset-1 ") { Some(rest) => (true, rest), None => (false, s) };
        let n = base.strip_prefix("base-")?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { return None }
        n.parse().ok().map(|n| (n, fenced, rdf12))
    }

    #[test]
    fn an_older_binary_refuses_a_fenced_store_before_reading_its_wal() {
        let d = dir("old-reader");
        let store = Store::open(&d, None, 1).unwrap();
        store.commit(&[(Change::Insert, t(0))]).unwrap();
        drop(store);
        let current = std::fs::read_to_string(d.join("CURRENT")).unwrap();
        // The older open maps `None` to "corrupt CURRENT" right after taking the lock, before recovery or cleanup.
        assert_eq!(previous_parse_current(&current), None, "{current:?}");
        assert_eq!(previous_parse_current("base-3"), Some((3, false, false)), "the copy still reads an unfenced store");
        std::fs::remove_dir_all(d).unwrap();
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

    /// #111: a range the delta doesn't touch takes the base's direct paths; every answer still equals the merged
    /// view's. Checked against the full merged scan for every order and every prefix of length 0 to 2, built from base
    /// IDs, IDs first seen in the delta (a query constant that exists only in the delta) and an ID nothing uses.
    #[test]
    fn untouched_ranges_answer_like_the_merged_view() {
        let iri = |s: String| NamedNode::new(format!("urn:{s}")).unwrap();
        let tr = |s: u32, p: u32, o: u32| Triple::new(iri(format!("s{s}")), iri(format!("p{p}")), iri(format!("o{o}")));
        let base = Arc::new(MemorySnapshot::from_triples((0..40).flat_map(|s| (0..3).map(move |p| tr(s, p, (s * 7 + p) % 11)))));
        let original = Overlay::from_base(base);
        // Changes touch subjects 3 and 4, predicate 2 of subject 5, and a brand-new subject, predicate and object.
        let changes = vec![(Change::Delete, tr(3, 0, 21 % 11)), (Change::Insert, tr(4, 1, 99)), (Change::Delete, tr(5, 2, 37 % 11)),
                           (Change::Insert, tr(500, 77, 3))];
        let (o, _) = original.with_changes(&changes).unwrap();
        let rows_in = |perm: Perm| {
            let mut rows: Vec<[TermId; 3]> = o.scan(Perm::Spo, &[]).flat_map(|b| { let b = b.unwrap(); (0..b.len()).map(move |i| key(perm, [b.cols[0][i], b.cols[1][i], b.cols[2][i]])).collect::<Vec<_>>() }).collect();
            rows.sort();
            rows
        };
        let mut ids: Vec<TermId> = rows_in(Perm::Spo).iter().flat_map(|r| r.iter().copied()).collect();
        ids.push(TermId(123_456)); // an ID nothing uses
        ids.sort(); ids.dedup();
        for perm in Perm::ALL {
            let all = rows_in(perm);
            let mut prefixes: Vec<Vec<TermId>> = vec![vec![]];
            prefixes.extend(ids.iter().map(|&a| vec![a]));
            prefixes.extend(all.iter().map(|r| vec![r[0], r[1]]));
            prefixes.push(vec![TermId(123_456), ids[0]]);
            for prefix in prefixes {
                let want: Vec<[TermId; 3]> = all.iter().filter(|r| r[..prefix.len()] == prefix[..]).copied().collect();
                let got: Vec<[TermId; 3]> = o.scan(perm, &prefix).flat_map(|b| { let b = b.unwrap(); (0..b.len()).map(move |i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]).collect::<Vec<_>>() }).collect();
                let label = format!("{perm:?} {prefix:?}");
                assert_eq!(got, want, "scan {label}"); // batches are in key order
                assert_eq!(o.count(perm, &prefix).unwrap(), want.len() as u64, "count {label}");
                if let Some(rows) = o.matching_rows(perm, &prefix) { assert_eq!(rows, &want[..], "rows {label}") }
                if let (Some(tail), false) = (o.matching_tail(perm, &prefix), prefix.is_empty()) {
                    assert_eq!(tail.iter().map(|t| [prefix[0], t[0], t[1]]).collect::<Vec<_>>(), want, "tail {label}");
                }
                if prefix.len() < 3 {
                    let next: Vec<TermId> = want.iter().map(|r| r[prefix.len()]).collect();
                    for target in [TermId(0), ids[ids.len() / 2], TermId(u64::MAX)] {
                        assert_eq!(o.seek(perm, &prefix, target).unwrap(), next.iter().copied().find(|&v| v >= target), "seek {label} {target:?}");
                        let from: Vec<[TermId; 3]> = o.scan_from(perm, &prefix, target).flat_map(|b| { let b = b.unwrap(); (0..b.len()).map(move |i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]).collect::<Vec<_>>() }).collect();
                        let want_from: Vec<[TermId; 3]> = want.iter().filter(|r| r[prefix.len()] >= target).copied().collect();
                        assert_eq!(from, want_from, "scan_from {label} {target:?}");
                    }
                }
            }
        }
        // The point of the change: an untouched range keeps the base's borrowed view; a touched one doesn't.
        let p0 = o.lookup(&iri("p0".into()).into()).unwrap().unwrap();
        let s3 = o.lookup(&iri("s3".into()).into()).unwrap().unwrap();
        let s10 = o.lookup(&iri("s10".into()).into()).unwrap().unwrap();
        assert!(o.matching_rows(Perm::Spo, &[s10]).is_some() || original.matching_rows(Perm::Spo, &[s10]).is_none());
        assert!(o.matching_rows(Perm::Spo, &[s3]).is_none());
        assert!(o.matching_tail(Perm::Pso, &[p0]).is_none(), "p0 lost a triple");
    }

    #[test]
    fn first_keys_follow_the_delta() {
        let t = |s: &str, o: &str| Triple::new(NamedNode::new(s).unwrap(), NamedNode::new("urn:p").unwrap(), NamedNode::new(o).unwrap());
        let base = Arc::new(MemorySnapshot::from_triples([t("urn:a", "urn:x"), t("urn:b", "urn:x")]));
        let original = Overlay::from_base(base);
        let id = |o: &Overlay, s: &str| o.lookup(&NamedNode::new(s).unwrap().into()).unwrap().unwrap();
        assert_eq!(original.first_keys(Perm::Spo).unwrap(), vec![id(&original, "urn:a"), id(&original, "urn:b")]);
        // Deleting a's only triple removes it as a subject; inserting c adds a subject and an object.
        let (changed, _) = original.with_changes(&[(Change::Delete, t("urn:a", "urn:x")), (Change::Insert, t("urn:c", "urn:y"))]).unwrap();
        let mut subjects = vec![id(&changed, "urn:b"), id(&changed, "urn:c")];
        subjects.sort();
        assert_eq!(changed.first_keys(Perm::Spo).unwrap(), subjects);
        assert_eq!(changed.first_keys(Perm::Sop).unwrap(), subjects);
        let mut objects = vec![id(&changed, "urn:x"), id(&changed, "urn:y")];
        objects.sort();
        assert_eq!(changed.first_keys(Perm::Osp).unwrap(), objects);
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
    pub(super) fn state(o: &Overlay) -> (Vec<String>, Vec<(String, Vec<String>)>) {
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
        // Since #114 the first write fences the WAL batches; a default-only commit still needs no dataset fence.
        assert_eq!(dir.current().trim(), "walcrc-1 base-0", "default-only commits fence only the WAL batches");
        assert_eq!(store.commit_dataset(&[ins_default(t("plain", "1"))]).unwrap(), 1);
        assert_eq!(dir.current().trim(), "walcrc-1 base-0", "a default-only dataset update needs no dataset fence either");
        store.commit_dataset(&sample()).unwrap();
        assert_eq!(dir.current().trim(), "walcrc-1 dataset-1 base-0");
        store.commit_dataset(&[del("g2", t("b", "2")), Drop { target: GraphScope::Graph(iri("g1")), silent: false }]).unwrap();
        let want = state(&store.snapshot());
        assert_eq!(want.1.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), ["<http://ex/empty>", "<http://ex/g2>"]);
        drop(store);
        let reopened = dir.open();
        assert_eq!(state(&reopened.snapshot()), want);
        assert_eq!(dir.current().trim(), "walcrc-1 dataset-1 base-0");
    }

    #[test]
    fn compaction_carries_named_graphs_and_keeps_the_fence() {
        let dir = Dir::new("compact");
        let store = dir.open();
        store.commit_dataset(&sample()).unwrap();
        let want = state(&store.snapshot());
        store.compact().unwrap();
        assert_eq!(dir.current().trim(), "walcrc-1 dataset-1 base-1");
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
        assert_eq!((dir.wal(), dir.current().trim().to_string()), (wal, "walcrc-1 base-0".to_string()));
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
        assert_eq!(dir.current().trim(), "walcrc-1 dataset-1 base-1");
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
        // Commits now refuse such a term (#95), so the legacy records are written as an older binary wrote them: bare
        // records, before this binary's first batch (an older binary cannot open the store once it is fenced, #114).
        drop(store);
        let mut legacy = String::new();
        push_record(&mut legacy, &[ins_default(t("before", "0"))]);
        push_record(&mut legacy, &[ins_default(t("big", &big))]);
        std::fs::write(dir.0.join("wal.log"), legacy.as_bytes()).unwrap();
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
        let f = |dataset, rdf12, walcrc| Fences { dataset, rdf12, walcrc };
        assert_eq!(parse_current("base-7\n").unwrap(), Some((7, f(false, false, false))));
        assert_eq!(parse_current("dataset-1 base-12\n").unwrap(), Some((12, f(true, false, false))));
        assert_eq!(parse_current("walcrc-1 dataset-1 base-5\n").unwrap(), Some((5, f(true, false, true))));
        for bad in ["", "base-", "base-+1", "dataset-2 base-1", "dataset-1base-1", "dataset-1 ", "base-1 extra", "dataset-1 rdf12-1 base-1", "rdf12-1base-1", "dataset-1 walcrc-1 base-1", "walcrc-1base-1"] {
            assert_eq!(parse_current(bad).unwrap(), None, "{bad:?}");
        }
        // The RDF 1.2 fence (task #101): read by an rdf-12 build, refused with a clear error by any other.
        for (text, expect) in [("rdf12-1 base-3", (3, f(false, true, false))), ("rdf12-1 dataset-1 base-4", (4, f(true, true, false))),
                               ("walcrc-1 rdf12-1 base-6", (6, f(false, true, true)))] {
            if cfg!(feature = "rdf-12") {
                assert_eq!(parse_current(text).unwrap(), Some(expect));
            } else {
                assert_eq!(parse_current(text).unwrap_err().kind(), std::io::ErrorKind::Unsupported);
            }
        }
    }
}

#[cfg(test)]
mod read_only_tests;
