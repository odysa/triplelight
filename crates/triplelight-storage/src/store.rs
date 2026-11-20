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
//! WAL record: one line per change, `+ <triple> .` or `- <triple> .` in N-Triples syntax, then a line `C`.
//! Recovery replays complete records only; a torn tail (crash mid-append) is ignored and truncated.
use crate::model::{Perm, TermId, TripleBatch};
use crate::storage::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use oxrdf::{Term, TermRef, Triple};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

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
    fn delta_term(&self, id: TermId) -> Result<&Term, StorageError> {
        self.delta.rev.get((id.0 & !(0xf << 60)) as usize).ok_or_else(|| StorageError::other("unknown delta term ID"))
    }
}

impl Dictionary for Overlay {
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

impl Snapshot for Overlay {
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        if self.delta.is_empty() { self.base.matching_rows(perm, prefix) } else { None }
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

/// A persistent store: base file + WAL in a directory.
pub struct Store {
    dir: PathBuf,
    format: Box<dyn BaseFormat>,
    /// Number of the active base directory.
    generation: Mutex<u64>,
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
        directory_lock.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => std::io::Error::new(std::io::ErrorKind::WouldBlock, "database directory is already open"),
            std::fs::TryLockError::Error(e) => e,
        })?;
        let generation = match std::fs::read_to_string(dir.join("CURRENT")) {
            Ok(s) => s.trim().strip_prefix("base-").and_then(|n| n.parse().ok())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "corrupt CURRENT"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let base0 = dir.join("base-0");
                // No CURRENT means no base was ever committed: a `base-0` here is an abandoned earlier attempt.
                if base0.exists() { std::fs::remove_dir_all(&base0)?; }
                format.import(&base0, initial)?;
                set_current(&dir, 0)?;
                0
            }
            Err(e) => return Err(e),
        };
        // Leftovers of crashed compactions or superseded bases.
        for entry in std::fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("base-") && name != format!("base-{generation}") { std::fs::remove_dir_all(dir.join(&name))?; }
        }
        let base = format.open(&dir.join(format!("base-{generation}")))?;
        let mut overlay = Overlay { base, delta: Arc::new(Delta::default()) };
        let wal_path = dir.join("wal.log");
        let (records, valid_len) = read_wal(&wal_path)?;
        for rec in records { overlay = apply(&overlay, &rec)?; }
        let mut wal = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&wal_path)?;
        wal.set_len(valid_len)?; // drop a torn tail
        wal.sync_all()?;
        use std::io::Seek;
        wal.seek(std::io::SeekFrom::End(0))?;
        let overlay = Arc::new(overlay);
        Ok(Store {
            dir, format, generation: Mutex::new(generation),
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
        for (c, t) in &rec { p.buf.push_str(if *c == Change::Insert { "+ " } else { "- " }); p.buf.push_str(&t.to_string()); p.buf.push_str(" .\n"); }
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
        if cur.delta.is_empty() { return Ok(()) }
        let mut generation = self.generation.lock().unwrap();
        let next = *generation + 1;
        let new_dir = self.dir.join(format!("base-{next}"));
        // Not named by CURRENT, so at most an abandoned earlier attempt in this process.
        if new_dir.exists() { std::fs::remove_dir_all(&new_dir)?; }
        self.format.write(&new_dir, &*cur)?;
        // Open (and so validate) the new base before committing to it: if this fails, CURRENT, the WAL and the
        // in-memory state all still describe the old base.
        let base = self.format.open(&new_dir)?;
        set_current(&self.dir, next)?;
        // The new base is durable and current; only now drop the log.
        w.file.set_len(0)?;
        w.file.sync_all()?;
        use std::io::Seek;
        w.file.seek(std::io::SeekFrom::Start(0))?;
        let fresh = Arc::new(Overlay { base, delta: Arc::new(Delta::default()) });
        p.latest = fresh.clone();
        *self.current.write().unwrap() = fresh;
        *generation = next;
        Ok(())
    }
}

/// Atomically points CURRENT at `base-<n>`.
fn set_current(dir: &Path, n: u64) -> std::io::Result<()> {
    let tmp = dir.join("CURRENT.tmp");
    let mut f = File::create(&tmp)?;
    writeln!(f, "base-{n}")?;
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
    pub fn from_base(base: Arc<dyn Snapshot>) -> Overlay { Overlay { base, delta: Arc::new(Delta::default()) } }

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
    Ok(Overlay { base: cur.base.clone(), delta: Arc::new(d) })
}

/// Complete records in the WAL, and the byte length they span (everything after is a torn tail).
fn read_wal(path: &Path) -> std::io::Result<(Vec<Vec<(Change, Triple)>>, u64)> {
    let Ok(f) = File::open(path) else { return Ok((Vec::new(), 0)) };
    let mut records = Vec::new();
    let (mut cur, mut pos, mut valid) = (Vec::new(), 0u64, 0u64);
    for line in BufReader::new(f).split(b'\n') {
        let line = line?;
        pos += line.len() as u64 + 1;
        let Ok(line) = std::str::from_utf8(&line) else { break };
        if line == "C" { records.push(std::mem::take(&mut cur)); valid = pos; continue }
        let (c, rest) = match line.split_at_checked(2) {
            Some(("+ ", r)) => (Change::Insert, r),
            Some(("- ", r)) => (Change::Delete, r),
            _ => break,
        };
        let parsed = oxttl::NTriplesParser::new().for_slice(rest.as_bytes()).next();
        match parsed { Some(Ok(t)) => cur.push((c, t)), _ => break }
    }
    Ok((records, valid))
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
        let original = Overlay::from_base(base);
        assert_eq!(original.matching_rows(Perm::Spo, &[]).unwrap().len(), 1);
        let (added, _) = original.with_changes(&[(Change::Insert, b.clone())]).unwrap();
        assert!(added.matching_rows(Perm::Spo, &[]).is_none());
        assert_eq!(added.count(Perm::Spo, &[]).unwrap(), 2);
        let (deleted, _) = original.with_changes(&[(Change::Delete, a)]).unwrap();
        assert!(deleted.matching_rows(Perm::Spo, &[]).is_none());
        assert_eq!(deleted.count(Perm::Spo, &[]).unwrap(), 0);
        let (restored, _) = added.with_changes(&[(Change::Delete, b)]).unwrap();
        assert_eq!(restored.matching_rows(Perm::Spo, &[]).unwrap().len(), 1);
    }
}
