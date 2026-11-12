//! Durable updates (task #8). The base snapshot is immutable; committed changes live in a write-ahead log (WAL)
//! and an in-memory delta layered over the base. Readers take an `Arc<Overlay>`: a consistent snapshot that later
//! commits never change. Commit = append the change set to the WAL, fsync, then publish a new overlay.
//! Compaction folds the delta into a new base file and truncates the WAL.
//!
//! On-disk layout of a data directory:
//!   base.nt   the base data (N-Triples), replaced atomically by compaction (write tmp, fsync, rename)
//!   wal.log   committed change sets since the base was written
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
    base: Arc<MemorySnapshot>,
    delta: Arc<Delta>,
}

impl Overlay {
    pub fn delta_len(&self) -> usize { self.delta.len() }
    pub fn base(&self) -> &MemorySnapshot { &self.base }

    fn visible_in_base(&self, spo: [TermId; 3]) -> Result<bool, StorageError> {
        Ok(self.base.count(Perm::Spo, &spo)? > 0 && self.delta.dels[0].binary_search(&spo).is_err())
    }
    fn contains(&self, spo: [TermId; 3]) -> Result<bool, StorageError> {
        Ok(self.delta.adds[0].binary_search(&spo).is_ok() || self.visible_in_base(spo)?)
    }
}

impl Dictionary for Overlay {
    fn lookup(&self, term: &Term) -> Option<TermId> { self.base.lookup(term).or_else(|| self.delta.terms.get(term).copied()) }
    fn resolve(&self, id: TermId) -> Term {
        if id.0 >> 60 == DELTA_TAG { self.delta.rev[(id.0 & !(0xf << 60)) as usize].clone() } else { self.base.resolve(id) }
    }
    fn resolve_ref(&self, id: TermId) -> TermRef<'_> {
        if id.0 >> 60 == DELTA_TAG { self.delta.rev[(id.0 & !(0xf << 60)) as usize].as_ref() } else { self.base.resolve_ref(id) }
    }
}

impl Snapshot for Overlay {
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        if self.delta.is_empty() { return self.base.scan(perm, prefix) }
        let (adds, dels) = (range(&self.delta.adds[slot(perm)], prefix), range(&self.delta.dels[slot(perm)], prefix));
        // Merge the base range (minus deletions) with the added keys, in key order.
        let mut base_rows: Vec<[TermId; 3]> = Vec::new();
        for b in self.base.scan(perm, prefix) {
            let b = match b { Ok(b) => b, Err(e) => return Box::new(std::iter::once(Err(e))) };
            for i in 0..b.len() {
                let k = [b.cols[0][i], b.cols[1][i], b.cols[2][i]];
                if dels.binary_search(&k).is_err() { base_rows.push(k) }
            }
        }
        let mut merged = Vec::with_capacity(base_rows.len() + adds.len());
        let (mut i, mut j) = (0, 0);
        while i < base_rows.len() || j < adds.len() {
            if j == adds.len() || (i < base_rows.len() && base_rows[i] < adds[j]) { merged.push(base_rows[i]); i += 1 } else { merged.push(adds[j]); j += 1 }
        }
        Box::new(merged.chunks(4096).map(|c| {
            let mut b = TripleBatch::default();
            for r in c { for k in 0..3 { b.cols[k].push(r[k]) } }
            Ok(b)
        }).collect::<Vec<_>>().into_iter())
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change { Insert, Delete }

/// A persistent store: base file + WAL in a directory.
pub struct Store {
    dir: PathBuf,
    current: RwLock<Arc<Overlay>>,
    /// Serializes WAL appends; holds the open WAL.
    wal: Mutex<File>,
    /// Serializes update requests (read-compute-commit).
    writer: Mutex<()>,
}

impl Store {
    /// Opens `dir`, creating it from `initial` (an N-Triples file) if it has no base yet; replays the WAL.
    pub fn open(dir: impl AsRef<Path>, initial: Option<&Path>, workers: usize) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let base_path = dir.join("base.nt");
        if !base_path.exists() {
            let tmp = dir.join("base.nt.tmp");
            match initial { Some(src) => { std::fs::copy(src, &tmp)?; } None => { File::create(&tmp)?; } }
            File::open(&tmp)?.sync_all()?;
            std::fs::rename(&tmp, &base_path)?;
            sync_dir(&dir)?;
        }
        let base = Arc::new(MemorySnapshot::from_ntriples_parallel_file(&base_path, workers.max(1))?);
        let mut overlay = Overlay { base, delta: Arc::new(Delta::default()) };
        let wal_path = dir.join("wal.log");
        let (records, valid_len) = read_wal(&wal_path)?;
        for rec in records { overlay = apply(&overlay, &rec)?; }
        let wal = OpenOptions::new().create(true).read(true).write(true).open(&wal_path)?;
        wal.set_len(valid_len)?; // drop a torn tail
        wal.sync_all()?;
        let mut wal = wal;
        use std::io::Seek;
        wal.seek(std::io::SeekFrom::End(0))?;
        Ok(Store { dir, current: RwLock::new(Arc::new(overlay)), wal: Mutex::new(wal), writer: Mutex::new(()) })
    }

    /// The current read snapshot.
    pub fn snapshot(&self) -> Arc<Overlay> { self.current.read().unwrap().clone() }

    /// Durably commits a change set; returns once it is fsynced and visible to new readers.
    /// Changes that are no-ops (inserting a present triple, deleting an absent one) are dropped.
    pub fn commit(&self, changes: &[(Change, Triple)]) -> std::io::Result<usize> {
        let mut wal = self.wal.lock().unwrap();
        let cur = self.snapshot();
        let mut rec = Vec::new();
        // Presence as of the changes applied so far in this set (a set may insert and later delete a triple).
        let mut state: HashMap<Triple, bool> = HashMap::new();
        for (c, t) in changes {
            let present = match state.get(t) {
                Some(&p) => p,
                None => match ids_if_known(&cur, t) { Some(spo) => cur.contains(spo)?, None => false },
            };
            if (*c == Change::Insert) != present { rec.push((*c, t.clone())) }
            state.insert(t.clone(), *c == Change::Insert);
        }
        if rec.is_empty() { return Ok(0) }
        let mut buf = String::new();
        for (c, t) in &rec { buf.push_str(if *c == Change::Insert { "+ " } else { "- " }); buf.push_str(&t.to_string()); buf.push_str(" .\n"); }
        buf.push_str("C\n");
        wal.write_all(buf.as_bytes())?;
        wal.sync_data()?;
        let next = apply(&cur, &rec)?;
        *self.current.write().unwrap() = Arc::new(next);
        Ok(rec.len())
    }

    /// Folds the delta into a new base file (atomic rename) and empties the WAL. Readers are not blocked; writers are.
    pub fn compact(&self, workers: usize) -> std::io::Result<()> {
        let mut wal = self.wal.lock().unwrap();
        let cur = self.snapshot();
        if cur.delta.is_empty() { return Ok(()) }
        let tmp = self.dir.join("base.nt.tmp");
        {
            let mut out = std::io::BufWriter::new(File::create(&tmp)?);
            for b in cur.scan(Perm::Spo, &[]) {
                let b = b?;
                for i in 0..b.len() {
                    let t = |id| cur.resolve(id);
                    writeln!(out, "{} {} {} .", t(b.cols[0][i]), t(b.cols[1][i]), t(b.cols[2][i]))?;
                }
            }
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join("base.nt"))?;
        sync_dir(&self.dir)?;
        // The new base is durable; only now drop the log.
        wal.set_len(0)?;
        wal.sync_all()?;
        use std::io::Seek;
        wal.seek(std::io::SeekFrom::Start(0))?;
        let base = Arc::new(MemorySnapshot::from_ntriples_parallel_file(self.dir.join("base.nt"), workers.max(1))?);
        *self.current.write().unwrap() = Arc::new(Overlay { base, delta: Arc::new(Delta::default()) });
        Ok(())
    }
}

fn sync_dir(dir: &Path) -> std::io::Result<()> { File::open(dir)?.sync_all() }

fn ids_if_known(o: &Overlay, t: &Triple) -> Option<[TermId; 3]> {
    Some([o.lookup(&t.subject.clone().into())?, o.lookup(&t.predicate.clone().into())?, o.lookup(&t.object)?])
}

/// New overlay = `cur` + one committed record (copy-on-write delta).
fn apply(cur: &Overlay, rec: &[(Change, Triple)]) -> std::io::Result<Overlay> {
    let mut d: Delta = (*cur.delta).clone();
    let intern = |d: &mut Delta, term: Term| -> TermId {
        if let Some(id) = cur.base.lookup(&term).or_else(|| d.terms.get(&term).copied()) { return id }
        let id = TermId((DELTA_TAG << 60) | d.rev.len() as u64);
        d.terms.insert(term.clone(), id);
        d.rev.push(term);
        id
    };
    for (c, t) in rec {
        let spo = [intern(&mut d, t.subject.clone().into()), intern(&mut d, t.predicate.clone().into()), intern(&mut d, t.object.clone())];
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
    /// Executes a SPARQL 1.1 Update request atomically: every operation sees the effects of the previous ones, and
    /// the net change set is committed once (all or nothing). Returns the number of triples changed.
    pub fn update(&self, text: &str) -> Result<usize, crate::exec::QueryError> {
        use crate::exec::{QueryError, QueryErrorKind};
        // Serializable: no other update commits between reading the snapshot and committing its changes.
        let _writer = self.writer.lock().unwrap();
        let snap = self.snapshot();
        let changes = crate::exec::update_changes(&*snap, text)?;
        self.commit(&changes).map_err(|e| QueryError::new(QueryErrorKind::Storage, e.to_string()))
    }
}
