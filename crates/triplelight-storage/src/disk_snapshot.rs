//! Read-only disk-backed snapshot with an in-memory dictionary and bounded index caches.
//! Snapshot files must remain immutable while open.
use super::disk_index::{DiskIndex, write_index};
use super::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::Term;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub(super) const FILES: [&str; 7] = ["dictionary", "spo", "sop", "pso", "pos", "osp", "ops"];
fn bad(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
pub(super) fn digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = BufReader::new(File::open(path)?);
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().into())
}
fn number(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn write_dictionary(source: &MemorySnapshot, path: &Path) -> io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    out.write_all(b"RDFDIC01")?;
    let entries = source.terms();
    out.write_all(&(entries.len() as u64).to_le_bytes())?;
    for (id, term) in entries {
        let text = term.to_string();
        out.write_all(&id.0.to_le_bytes())?;
        out.write_all(&(text.len() as u64).to_le_bytes())?;
        out.write_all(text.as_bytes())?;
    }
    out.flush()?;
    out.get_ref().sync_all()
}
fn read_dictionary(path: &Path) -> io::Result<(HashMap<Term, TermId>, HashMap<TermId, Term>)> {
    let mut input = BufReader::new(File::open(path)?);
    let mut header = [0; 8];
    input.read_exact(&mut header)?;
    if &header != b"RDFDIC01" {
        return Err(bad("bad dictionary magic"));
    }
    let n = number(&mut input)?;
    if n > 100_000_000 {
        return Err(bad("dictionary count too large"));
    }
    let (mut forward, mut reverse) = (HashMap::new(), HashMap::new());
    for _ in 0..n {
        let id = TermId(number(&mut input)?);
        let len = number(&mut input)?;
        if id.0 == 0 || len > 16_777_216 {
            return Err(bad("invalid dictionary entry"));
        }
        let mut bytes = vec![0; len as usize];
        input.read_exact(&mut bytes)?;
        let term: Term = std::str::from_utf8(&bytes)
            .map_err(|_| bad("invalid term UTF-8"))?
            .parse()
            .map_err(|_| bad("invalid term"))?;
        if forward.insert(term.clone(), id).is_some() || reverse.insert(id, term).is_some() {
            return Err(bad("duplicate dictionary entry"));
        }
    }
    let mut extra = [0];
    if input.read(&mut extra)? != 0 {
        return Err(bad("dictionary trailing bytes"));
    }
    Ok((forward, reverse))
}
/// Build a new immutable disk snapshot from a consistent read snapshot using scratch-backed sorting.
pub fn write_disk_snapshot(dir: impl AsRef<Path>, source: &dyn Snapshot) -> io::Result<()> {
    super::external_build::from_snapshot(dir.as_ref(), source)
}
/// Build a new immutable disk snapshot from N-Triples using scratch-backed sorting.
pub fn build_disk_snapshot(dir: impl AsRef<Path>, source: Option<&Path>) -> io::Result<()> {
    super::external_build::adaptive(
        dir.as_ref(),
        source,
        std::thread::available_parallelism().map_or(1, |n| n.get()),
    )
}
/// Build from N-Triples with an explicit number of ingestion workers.
pub fn build_disk_snapshot_with_workers(dir: impl AsRef<Path>, source: Option<&Path>, workers: usize) -> io::Result<()> {
    if workers == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "workers must be positive")); }
    super::external_build::adaptive(dir.as_ref(), source, workers)
}
/// Persistent Store base format backed by immutable checksummed disk indexes.
pub struct DiskBase {
    pub workers: usize,
}
impl crate::store::BaseFormat for DiskBase {
    fn write(&self, dir: &Path, src: &dyn Snapshot) -> io::Result<()> {
        write_disk_snapshot(dir, src)
    }
    fn import(&self, dir: &Path, ntriples: Option<&Path>) -> io::Result<()> {
        super::external_build::adaptive(dir, ntriples, self.workers)
    }
    fn open(&self, dir: &Path) -> io::Result<std::sync::Arc<dyn Snapshot>> {
        Ok(std::sync::Arc::new(DiskSnapshot::open(dir)?))
    }
}
enum DictionaryBackend {
    Legacy {
        forward: HashMap<Term, TermId>,
        reverse: HashMap<TermId, Term>,
    },
    Lazy {
        by_term: super::lazy_dictionary::LazyIndex,
        by_id: super::lazy_dictionary::LazyIndex,
    },
    /// One file in ID order serving both lookups (manifest `RDFSNAP4`).
    Unified(super::lazy_dictionary::LazyIndex),
}
pub struct DiskSnapshot {
    dictionary: DictionaryBackend,
    indexes: Vec<DiskIndex>,
}
impl DiskSnapshot {
    /// Writes a new snapshot; the manifest is written last and old paths are never replaced.
    pub fn create(dir: impl AsRef<Path>, source: &MemorySnapshot) -> io::Result<()> {
        let dir = dir.as_ref();
        fs::create_dir(dir)?;
        write_dictionary(source, &dir.join(FILES[0]))?;
        let (term_identity, id_identity) = super::lazy_dictionary::convert_legacy(dir)?;
        let mut identities = vec![term_identity, id_identity];
        for (i, name) in FILES[1..].iter().enumerate() {
            let file = File::create(dir.join(name))?;
            let mut out = BufWriter::new(file);
            write_index(&mut out, source.index_rows(i))?;
            out.flush()?;
            out.get_ref().sync_all()?;
            let index = DiskIndex::<3>::open(dir.join(name))?;
            index.validate()?;
            identities.push(index.identity());
        }
        let mut manifest = BufWriter::new(File::create(dir.join("MANIFEST"))?);
        manifest.write_all(b"RDFSNAP3")?;
        for identity in identities {
            manifest.write_all(&identity)?;
        }
        manifest.flush()?;
        manifest.get_ref().sync_all()?;
        File::open(dir)?.sync_all()?;
        File::open(
            dir.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    }
    /// Checks the dictionary, index lengths and checksummed block directories. Data blocks are checked on cache miss.
    /// Files must remain immutable after opening; previously cached blocks are not re-read automatically.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let mut manifest = BufReader::new(File::open(dir.join("MANIFEST"))?);
        let mut magic = [0; 8];
        manifest.read_exact(&mut magic)?;
        // RDFSNAP2: one legacy dictionary file; RDFSNAP3: dictionary.term + dictionary.id; RDFSNAP4: one unified file.
        let version = match &magic {
            b"RDFSNAP2" => 2,
            b"RDFSNAP3" => 3,
            b"RDFSNAP4" => 4,
            _ => return Err(bad("bad snapshot manifest or version")),
        };
        let mut expected = vec![[0u8; 32]; if version == 3 { 8 } else { 7 }];
        for item in &mut expected {
            manifest.read_exact(item)?;
        }
        let mut extra = [0];
        if manifest.read(&mut extra)? != 0 {
            return Err(bad("manifest trailing bytes"));
        }
        let (dictionary, index_offset) = if version == 4 {
            use super::lazy_dictionary::{DictionaryCache, LazyIndex, Order, UNIFIED_FILE};
            let unified = LazyIndex::open_shared(&dir.join(UNIFIED_FILE), Order::Unified, DictionaryCache::new())?;
            if unified.identity() != expected[0] {
                return Err(bad("dictionary directory checksum mismatch"));
            }
            (DictionaryBackend::Unified(unified), 1)
        } else if version == 3 {
            use super::lazy_dictionary::{DictionaryCache, LazyIndex, Order};
            let cache = DictionaryCache::new();
            let by_term = LazyIndex::open_shared(&dir.join("dictionary.term"), Order::Term, std::sync::Arc::clone(&cache))?;
            let by_id = LazyIndex::open_shared(&dir.join("dictionary.id"), Order::Id, cache)?;
            if by_term.identity() != expected[0] || by_id.identity() != expected[1] {
                return Err(bad("dictionary directory checksum mismatch"));
            }
            (DictionaryBackend::Lazy { by_term, by_id }, 2)
        } else {
            if digest(&dir.join(FILES[0]))? != expected[0] {
                return Err(bad("dictionary checksum mismatch"));
            }
            let (forward, reverse) = read_dictionary(&dir.join(FILES[0]))?;
            (DictionaryBackend::Legacy { forward, reverse }, 1)
        };
        let mut indexes = Vec::new();
        for (i, name) in FILES[1..].iter().enumerate() {
            let index = DiskIndex::<3>::open(dir.join(name))?;
            if index.identity() != expected[i + index_offset] {
                return Err(bad("index directory checksum mismatch"));
            }
            indexes.push(index);
        }
        let rows = indexes[0].len();
        if indexes.iter().any(|index| index.len() != rows) {
            return Err(bad("index counts differ"));
        }
        Ok(Self {
            dictionary,
            indexes,
        })
    }
    /// Exhaustive, exact verification. The phases run in this order because the later ones rely on the earlier:
    /// 1. Dictionary: every term-order entry `(id, term)` must equal `by_id[id]`, adjacent term-order terms must
    ///    differ, and both indexes must hold the same number of entries. The term index is strictly sorted, so its
    ///    N terms are distinct and map injectively into the ID index; with equal counts the two are one bijection,
    ///    and no reverse lookup is needed.
    /// 2. Indexes (in parallel): every block re-read and validated (strictly sorted, so no duplicate rows), and all
    ///    six indexes have the same row count.
    /// 3. Permutations (in parallel): every row of every order is in SPO. With phase 2 (no duplicates, equal counts)
    ///    this makes all six orders the same triple set.
    /// 4. References: every ID in SPO resolves in the dictionary. Because of phases 2 and 3 this covers every ID in
    ///    every order. Do not reorder these phases.
    pub fn verify(&self) -> io::Result<()> {
        use rayon::prelude::*;
        if let DictionaryBackend::Unified(unified) = &self.dictionary {
            // Every block decode already enforces: one kind per block, strictly increasing IDs and, inside a kind,
            // strictly increasing term text that agrees with the kind. That makes (ID <-> term) one bijection and
            // `find_term` exact, so a single pass over all blocks is the whole dictionary check.
            unified.clear_cache()?;
            let mut count = 0u64;
            unified.for_each(&mut |_, _| {
                count += 1;
                Ok(())
            })?;
            if count == 0 && self.indexes[0].len() != 0 {
                return Err(bad("dictionary is empty but the indexes are not"));
            }
        }
        if let DictionaryBackend::Lazy { by_term, by_id } = &self.dictionary {
            by_term.clear_cache()?;
            by_id.clear_cache()?;
            let mut term_count = 0u64;
            let mut previous: Option<Term> = None;
            by_term.for_each(&mut |id, term| {
                if previous.as_ref() == Some(term) {
                    return Err(bad("dictionary term index repeats a term"));
                }
                let mut found = None;
                by_id.with_id(id, &mut |v| found = Some(v.into_owned()))?;
                if found.as_ref() != Some(term) {
                    return Err(bad("dictionary index disagreement"));
                }
                previous = Some(term.clone());
                term_count += 1;
                Ok(())
            })?;
            let mut id_count = 0u64;
            by_id.for_each(&mut |_, _| {
                id_count += 1;
                Ok(())
            })?;
            if term_count != id_count {
                return Err(bad("dictionary index count mismatch"));
            }
        }
        // Phase 2.
        self.indexes.par_iter().try_for_each(|index| -> io::Result<()> {
            index.clear_cache()?;
            index.validate()
        })?;
        let rows = self.index(Perm::Spo).len();
        if self.indexes.iter().any(|index| index.len() != rows) {
            return Err(bad("permutation row counts differ"));
        }
        // Phase 3.
        self.verify_permutations()?;
        // Phase 4: SPO only, skipping an ID equal to the previous row's in the same column.
        let spo = self.index(Perm::Spo);
        let mut last = [None; 3];
        for i in 0..spo.len() {
            for (k, id) in spo.get(i)?.into_iter().enumerate() {
                if last[k] != Some(id) {
                    self.with_term(id, &mut |_| ())?;
                    last[k] = Some(id);
                }
            }
        }
        Ok(())
    }
    /// Exhaustively check that every row of every permutation is in SPO (the six orders in parallel). Together with
    /// strictly sorted indexes of equal length (`verify` phase 2) this means all permutations hold the same set.
    /// This can be expensive; open() does not perform it.
    pub fn verify_permutations(&self) -> io::Result<()> {
        use rayon::prelude::*;
        let base = self.index(Perm::Spo);
        Perm::ALL.par_iter().try_for_each(|&perm| -> io::Result<()> {
            let index = self.index(perm);
            let order = perm.order();
            for i in 0..index.len() {
                let row = index.get(i)?;
                let mut spo = [TermId(0); 3];
                for k in 0..3 {
                    spo[order[k]] = row[k];
                }
                let matching = base.range(&spo)?;
                if matching.end - matching.start != 1 {
                    return Err(bad("permutation contents disagree"));
                }
            }
            Ok(())
        })
    }
    fn index(&self, perm: Perm) -> &DiskIndex {
        &self.indexes[Perm::ALL.iter().position(|x| *x == perm).unwrap()]
    }
}
impl Dictionary for DiskSnapshot {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { forward, .. } => Ok(forward.get(term).copied()),
            DictionaryBackend::Lazy { by_term, .. } => {
                Ok(by_term.find(term.to_string().as_bytes())?.map(|v| v.0))
            }
            DictionaryBackend::Unified(unified) => Ok(unified.find_term(term.to_string().as_bytes())?.map(|v| v.0)),
        }
    }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        let mut value = None;
        self.with_term(id, &mut |v| value = Some(v.into_owned()))?;
        value.ok_or_else(|| bad("dictionary callback not invoked"))
    }
    fn with_term(
        &self,
        id: TermId,
        f: &mut dyn FnMut(oxrdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { reverse, .. } => {
                f(reverse
                    .get(&id)
                    .ok_or_else(|| bad("unknown term ID"))?
                    .as_ref());
                Ok(())
            }
            DictionaryBackend::Lazy { by_id, .. } => by_id.with_id(id, f),
            DictionaryBackend::Unified(unified) => unified.with_id(id, f),
        }
    }
    fn with_terms(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, oxrdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { reverse, .. } => {
                for (i, id) in ids.iter().enumerate() {
                    f(
                        i,
                        reverse
                            .get(id)
                            .ok_or_else(|| bad("unknown term ID"))?
                            .as_ref(),
                    );
                }
                Ok(())
            }
            DictionaryBackend::Lazy { by_id, .. } => by_id.with_ids(ids, f),
            DictionaryBackend::Unified(unified) => unified.with_ids(ids, f),
        }
    }
}

impl Snapshot for DiskSnapshot {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        let index = self.index(perm);
        match index.range(prefix) {
            Err(e) => Box::new(std::iter::once(Err(e))),
            Ok(range) => {
                let mut pos = range.start;
                Box::new(std::iter::from_fn(move || {
                    if pos >= range.end {
                        return None;
                    }
                    let end = pos.saturating_add(4096).min(range.end);
                    let mut batch = TripleBatch::default();
                    while pos < end {
                        match index.block_slice(pos, end) {
                            Ok(rows) => {
                                pos += rows.len() as u64;
                                for row in rows {
                                    for (col, value) in batch.cols.iter_mut().zip(row) {
                                        col.push(value);
                                    }
                                }
                            }
                            Err(e) => {
                                pos = range.end;
                                return Some(Err(e));
                            }
                        }
                    }
                    Some(Ok(batch))
                }))
            }
        }
    }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        let index = self.index(Perm::Spo);
        let mut pos = 0;
        Box::new(std::iter::from_fn(move || {
            if pos >= index.len() { return None; }
            let end = pos.saturating_add(4096).min(index.len());
            match index.block_slice_uncached(pos, end) {
                Ok(rows) => {
                    pos += rows.len() as u64;
                    let mut batch = TripleBatch::default();
                    for row in rows { for (col, value) in batch.cols.iter_mut().zip(row) { col.push(value); } }
                    Some(Ok(batch))
                }
                Err(e) => { pos = index.len(); Some(Err(e)) }
            }
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        let r = self.index(perm).range(prefix)?;
        Ok(r.end - r.start)
    }
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, StorageError> {
        self.index(perm).seek_value(prefix, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode, Triple};
    #[test]
    fn generic_snapshot_writer_reopens_and_refuses_overwrite() {
        let source = MemorySnapshot::from_triples([Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_language_tagged_literal("hello", "en").unwrap(),
        )]);
        let parent = std::env::temp_dir().join(format!("rdf-disk-generic-{}", std::process::id()));
        fs::create_dir(&parent).unwrap();
        let first = parent.join("first");
        let second = parent.join("second");
        write_disk_snapshot(&first, &source).unwrap();
        let disk = DiskSnapshot::open(&first).unwrap();
        write_disk_snapshot(&second, &disk).unwrap();
        assert!(write_disk_snapshot(&second, &disk).is_err());
        let copy = DiskSnapshot::open(&second).unwrap();
        copy.verify().unwrap();
        let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
        assert_eq!(
            format!("{:?}", crate::exec::query(&source, q).unwrap()),
            format!("{:?}", crate::exec::query(&copy, q).unwrap())
        );
        drop(copy);
        drop(disk);
        fs::remove_dir_all(parent).unwrap();
    }
    #[test]
    fn lazy_dictionary_block_corruption_is_reported_as_query_error() {
        let triple = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_simple_literal("data"),
        );
        let source = MemorySnapshot::from_triples([triple]);
        let dir =
            std::env::temp_dir().join(format!("rdf-lazy-query-corrupt-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("dictionary.term");
        let mut bytes = fs::read(&path).unwrap();
        bytes[8] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        let err = crate::exec::query(&disk, "SELECT ?o WHERE { <urn:s> <urn:p> ?o }").unwrap_err();
        assert_eq!(err.kind, crate::exec::QueryErrorKind::Storage);
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn lazy_dictionary_corruption_aborts_id_serialization() {
        let triple = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_simple_literal("data"),
        );
        let source = MemorySnapshot::from_triples([triple]);
        let dir =
            std::env::temp_dir().join(format!("rdf-lazy-output-corrupt-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("dictionary.id");
        let mut bytes = fs::read(&path).unwrap();
        bytes[8] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        let opts = crate::exec::Options {
            raw_ids: true,
            ..Default::default()
        };
        let crate::exec::QueryResult::Ids(result) =
            crate::exec::query_with(&disk, "SELECT ?s ?p ?o WHERE { ?s ?p ?o }", &opts).unwrap()
        else {
            panic!("expected IDs")
        };
        assert!(crate::exec::results::serialize_ids(&result, &disk).is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn deep_verification_finds_consistent_checksum_with_wrong_permutation() {
        let p = NamedNode::new("urn:p").unwrap();
        let t = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            p,
            Literal::new_simple_literal("x"),
        );
        let source = MemorySnapshot::from_triples([t]);
        let dir = std::env::temp_dir().join(format!("rdf-disk-perms-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let mut out = File::create(dir.join("sop")).unwrap();
        write_index(&mut out, source.index_rows(0)).unwrap();
        drop(out);
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        let offset = 8 + 3 * 32;
        manifest[offset..offset + 32]
            .copy_from_slice(&DiskIndex::<3>::open(dir.join("sop")).unwrap().identity());
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify_permutations().is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    /// Three triples whose IDs are all in the dictionary, written as a fresh snapshot in `dir`.
    fn three_triple_snapshot(dir: &Path) -> MemorySnapshot {
        let p = NamedNode::new("urn:p").unwrap();
        let source = MemorySnapshot::from_triples((0..3).map(|i| {
            Triple::new(NamedNode::new(format!("urn:s{i}")).unwrap(), p.clone(), Literal::new_simple_literal(format!("value number {i}")))
        }));
        DiskSnapshot::create(dir, &source).unwrap();
        source
    }
    /// Replace one manifest slot's file: slot 0 term dictionary, 1 ID dictionary, 2.. the six orders of `Perm::ALL`.
    fn replace_manifest_identity(dir: &Path, slot: usize, identity: [u8; 32]) {
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        manifest[8 + slot * 32..8 + (slot + 1) * 32].copy_from_slice(&identity);
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
    }
    fn rewrite_order(dir: &Path, perm: Perm, mut rows: Vec<[TermId; 3]>) {
        rows.sort_unstable();
        let name = FILES[1 + Perm::ALL.iter().position(|x| *x == perm).unwrap()];
        fs::remove_file(dir.join(name)).unwrap();
        let mut out = File::create(dir.join(name)).unwrap();
        write_index(&mut out, rows.into_iter()).unwrap();
        drop(out);
        let slot = 2 + Perm::ALL.iter().position(|x| *x == perm).unwrap();
        replace_manifest_identity(dir, slot, DiskIndex::<3>::open(dir.join(name)).unwrap().identity());
    }
    fn verify_error(dir: &Path) -> String {
        let disk = DiskSnapshot::open(dir).unwrap();
        let err = disk.verify().expect_err("verify must fail").to_string();
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
        err
    }
    #[test]
    fn verify_rejects_a_non_spo_order_with_one_triple_swapped_for_another_valid_triple() {
        // Same count, still sorted, every ID valid, but one row is a triple SPO does not contain (Kaiming 0b631830):
        // exactly the case the SPO-only reference check depends on the permutation phase to catch.
        let dir = std::env::temp_dir().join(format!("rdf-verify-swap-{}", std::process::id()));
        let source = three_triple_snapshot(&dir);
        let spo: Vec<[TermId; 3]> = source.index_rows(0).collect();
        let order = Perm::Osp.order();
        let mut osp: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let foreign = [spo[0][0], spo[0][1], spo[1][2]]; // s0 p o1: valid IDs, not a stored triple
        osp[2] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order(&dir, Perm::Osp, osp);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn verify_rejects_an_id_missing_from_the_dictionary_that_only_a_non_spo_order_contains() {
        let dir = std::env::temp_dir().join(format!("rdf-verify-unknown-{}", std::process::id()));
        let source = three_triple_snapshot(&dir);
        let spo: Vec<[TermId; 3]> = source.index_rows(0).collect();
        let order = Perm::Pos.order();
        let mut pos: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let unknown = TermId((1 << 60) | 0x0fff_ffff_ffff_fff0);
        let foreign = [unknown, spo[0][1], spo[0][2]];
        pos[1] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order(&dir, Perm::Pos, pos);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn verify_rejects_a_term_index_that_disagrees_with_the_id_index() {
        let dir = std::env::temp_dir().join(format!("rdf-verify-dict-swap-{}", std::process::id()));
        three_triple_snapshot(&dir);
        let by_term = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap();
        let mut rows = Vec::new();
        by_term.for_each(&mut |id, term| { rows.push((id, term.clone())); Ok(()) }).unwrap();
        drop(by_term);
        // Swap the IDs of the first two terms: still a valid, sorted term index, but it disagrees with the ID index.
        let (a, b) = (rows[0].0, rows[1].0);
        rows[0].0 = b;
        rows[1].0 = a;
        rows.sort_by(|x, y| x.1.to_string().cmp(&y.1.to_string()).then(x.0.cmp(&y.0)));
        fs::remove_file(dir.join("dictionary.term")).unwrap();
        super::super::lazy_dictionary::write_index(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term, rows.into_iter().map(Ok)).unwrap();
        let identity = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap().identity();
        replace_manifest_identity(&dir, 0, identity);
        assert!(verify_error(&dir).contains("dictionary index disagreement"));
    }
    #[test]
    fn a_term_index_cannot_repeat_a_term_under_two_ids() {
        // Term-order keys are the term text alone, and both the writer and every block decode require strictly
        // increasing keys, so a repeated term is unrepresentable; `verify` keeps an explicit guard as well.
        let dir = std::env::temp_dir().join(format!("rdf-verify-dict-dup-{}", std::process::id()));
        three_triple_snapshot(&dir);
        let by_term = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap();
        let mut rows = Vec::new();
        by_term.for_each(&mut |id, term| { rows.push((id, term.clone())); Ok(()) }).unwrap();
        drop(by_term);
        rows[1].1 = rows[0].1.clone();
        rows.sort_by(|x, y| x.1.to_string().cmp(&y.1.to_string()).then(x.0.cmp(&y.0)));
        let path = dir.join("dictionary.term.dup");
        let err = super::super::lazy_dictionary::write_index(&path, super::super::lazy_dictionary::Order::Term, rows.into_iter().map(Ok)).unwrap_err();
        assert!(err.to_string().contains("not unique"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn data_corruption_is_lazy_but_verify_and_query_fail() {
        let p = NamedNode::new("urn:p").unwrap();
        let source = MemorySnapshot::from_triples([Triple::new(
            NamedNode::new("urn:s").unwrap(),
            p,
            Literal::new_simple_literal("x"),
        )]);
        let dir = std::env::temp_dir().join(format!("rdf-disk-lazy-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("spo");
        let mut bytes = fs::read(&path).unwrap();
        bytes[24] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify().is_err());
        assert!(disk.scan(Perm::Spo, &[]).next().unwrap().is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cache_miss_after_truncation_is_an_error() {
        let p = NamedNode::new("urn:p").unwrap();
        let rows = (0..34_000).map(|i| {
            Triple::new(
                NamedNode::new(format!("urn:s{i:06}")).unwrap(),
                p.clone(),
                Literal::new_simple_literal("x"),
            )
        });
        let source = MemorySnapshot::from_triples(rows);
        let dir = std::env::temp_dir().join(format!("rdf-disk-short-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        File::create(dir.join("spo")).unwrap();
        let first = disk.scan(Perm::Spo, &[]).next().unwrap();
        assert!(first.is_err());
        assert!(crate::exec::query(&disk, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1").is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn roundtrip_prefix_seek_and_checksum() {
        let p = NamedNode::new("urn:p").unwrap();
        let triples: Vec<_> = (0..600)
            .map(|i| {
                Triple::new(
                    NamedNode::new(format!("urn:s{i}")).unwrap(),
                    p.clone(),
                    Literal::new_simple_literal(format!("v{}", i % 4)),
                )
            })
            .collect();
        let source = MemorySnapshot::from_triples(triples.clone());
        let dir = std::env::temp_dir().join(format!("rdf-disk-snapshot-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        disk.verify_permutations().unwrap();
        for perm in Perm::ALL {
            let src: Vec<_> = source
                .scan(perm, &[])
                .map(|b| b.unwrap())
                .flat_map(|b| {
                    (0..b.len())
                        .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                        .collect::<Vec<_>>()
                })
                .collect();
            let dst: Vec<_> = disk
                .scan(perm, &[])
                .map(|b| b.unwrap())
                .flat_map(|b| {
                    (0..b.len())
                        .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(src, dst);
            for row in src.iter().step_by(17) {
                for n in 0..=3 {
                    let prefix = &row[..n];
                    assert_eq!(
                        source.count(perm, prefix).unwrap(),
                        disk.count(perm, prefix).unwrap()
                    );
                    if n < 3 {
                        assert_eq!(
                            source.seek(perm, prefix, row[n]).unwrap(),
                            disk.seek(perm, prefix, row[n]).unwrap()
                        );
                    }
                }
            }
        }
        for query in [
            "SELECT ?s ?o WHERE { ?s <urn:p> ?o } ORDER BY ?s LIMIT 25",
            "ASK { <urn:s5> <urn:p> \"v1\" }",
            "SELECT ?s WHERE { ?s <urn:p> \"v2\" } ORDER BY ?s LIMIT 10",
        ] {
            let memory = crate::exec::query(&source, query).unwrap();
            let persisted = crate::exec::query(&disk, query).unwrap();
            assert_eq!(
                crate::exec::results::serialize(&memory),
                crate::exec::results::serialize(&persisted)
            );
        }
        let t: Term = triples[7].object.clone();
        assert_eq!(disk.lookup(&t).unwrap(), source.lookup(&t).unwrap());
        assert_eq!(disk.resolve(disk.lookup(&t).unwrap().unwrap()).unwrap(), t);
        drop(disk);
        let path = dir.join("spo");
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(DiskSnapshot::open(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
