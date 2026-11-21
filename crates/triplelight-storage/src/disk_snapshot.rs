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
            let index = DiskIndex::open(dir.join(name))?;
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
        let lazy = match &magic {
            b"RDFSNAP2" => false,
            b"RDFSNAP3" => true,
            _ => return Err(bad("bad snapshot manifest or version")),
        };
        let mut expected = vec![[0u8; 32]; if lazy { 8 } else { 7 }];
        for item in &mut expected {
            manifest.read_exact(item)?;
        }
        let mut extra = [0];
        if manifest.read(&mut extra)? != 0 {
            return Err(bad("manifest trailing bytes"));
        }
        let (dictionary, index_offset) = if lazy {
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
            let index = DiskIndex::open(dir.join(name))?;
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
    /// Re-read every block, check dictionary references, then verify the exact permutation sets.
    pub fn verify(&self) -> io::Result<()> {
        if let DictionaryBackend::Lazy { by_term, by_id } = &self.dictionary {
            by_term.clear_cache()?;
            by_id.clear_cache()?;
            let mut term_count = 0u64;
            let mut id_count = 0u64;
            by_term.for_each(&mut |id, term| {
                let mut found = None;
                by_id.with_id(id, &mut |v| found = Some(v.into_owned()))?;
                if found.as_ref() != Some(term) {
                    return Err(bad("dictionary index disagreement"));
                }
                term_count += 1;
                Ok(())
            })?;
            by_id.for_each(&mut |id, term| {
                if by_term.find(term.to_string().as_bytes())?.map(|v| v.0) != Some(id) {
                    return Err(bad("dictionary index disagreement"));
                }
                id_count += 1;
                Ok(())
            })?;
            if term_count != id_count {
                return Err(bad("dictionary index count mismatch"));
            }
        }
        for index in &self.indexes {
            index.clear_cache()?;
            index.validate()?;
            for i in 0..index.len() {
                for id in index.get(i)? {
                    self.with_term(id, &mut |_| ())?;
                }
            }
        }
        self.verify_permutations()
    }
    /// Exhaustively check that all permutations describe the same triple set.
    /// This can be expensive; open() does not perform it.
    pub fn verify_permutations(&self) -> io::Result<()> {
        let base = self.index(Perm::Spo);
        for perm in Perm::ALL {
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
        }
        Ok(())
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
            .copy_from_slice(&DiskIndex::open(dir.join("sop")).unwrap().identity());
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify_permutations().is_err());
        drop(disk);
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
