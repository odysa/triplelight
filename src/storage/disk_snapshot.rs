//! Read-only disk-backed snapshot with an in-memory dictionary and bounded index caches.
//! v1 creation starts from an existing memory snapshot; files must remain immutable.
use super::disk_index::{DiskIndex, write_index};
use super::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::Term;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

const FILES: [&str; 7] = ["dictionary", "spo", "sop", "pso", "pos", "osp", "ops"];
fn bad(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn digest(path: &Path) -> io::Result<[u8; 32]> {
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
    let mut entries: Vec<_> = source
        .dictionary
        .iter()
        .map(|(term, id)| (id, term))
        .collect();
    entries.sort_unstable_by_key(|(id, _)| **id);
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
pub struct DiskSnapshot {
    forward: HashMap<Term, TermId>,
    reverse: HashMap<TermId, Term>,
    indexes: Vec<DiskIndex>,
}
impl DiskSnapshot {
    /// Writes into a new directory; the manifest is published last. Does not replace existing snapshots.
    pub fn create(dir: impl AsRef<Path>, source: &MemorySnapshot) -> io::Result<()> {
        let dir = dir.as_ref();
        fs::create_dir(dir)?;
        write_dictionary(source, &dir.join(FILES[0]))?;
        for (i, name) in FILES[1..].iter().enumerate() {
            let file = File::create(dir.join(name))?;
            let mut out = BufWriter::new(file);
            write_index(&mut out, source.indexes[i].iter().copied())?;
            out.flush()?;
            out.get_ref().sync_all()?;
        }
        let mut manifest = BufWriter::new(File::create(dir.join("MANIFEST"))?);
        manifest.write_all(b"RDFSNAP1")?;
        for name in FILES {
            manifest.write_all(&digest(&dir.join(name))?)?;
        }
        manifest.flush()?;
        manifest.get_ref().sync_all()?;
        File::open(dir)?.sync_all()?;
        Ok(())
    }
    /// Validate digests and sorted indexes before opening; callers must not modify files afterward.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let mut manifest = BufReader::new(File::open(dir.join("MANIFEST"))?);
        let mut magic = [0; 8];
        manifest.read_exact(&mut magic)?;
        if &magic != b"RDFSNAP1" {
            return Err(bad("bad snapshot manifest"));
        }
        for name in FILES {
            let mut expected = [0; 32];
            manifest.read_exact(&mut expected)?;
            if digest(&dir.join(name))? != expected {
                return Err(bad("snapshot checksum mismatch"));
            }
        }
        let mut extra = [0];
        if manifest.read(&mut extra)? != 0 {
            return Err(bad("manifest trailing bytes"));
        }
        let (forward, reverse) = read_dictionary(&dir.join(FILES[0]))?;
        let mut indexes = Vec::new();
        for name in &FILES[1..] {
            let index = DiskIndex::open(dir.join(name))?;
            index.validate()?;
            indexes.push(index);
        }
        let rows = indexes[0].len();
        for index in &indexes {
            if index.len() != rows {
                return Err(bad("index counts differ"));
            }
            for i in 0..rows {
                if index.get(i)?.iter().any(|id| !reverse.contains_key(id)) {
                    return Err(bad("index references unknown term"));
                }
            }
        }
        Ok(Self {
            forward,
            reverse,
            indexes,
        })
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
    fn resolve_ref(&self, id: TermId) -> oxrdf::TermRef<'_> {
        self.reverse.get(&id).expect("unknown term ID").as_ref()
    }
    fn lookup(&self, term: &Term) -> Option<TermId> {
        self.forward.get(term).copied()
    }
    fn resolve(&self, id: TermId) -> Term {
        self.reverse.get(&id).expect("unknown term ID").clone()
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
                        match index.get(pos) {
                            Ok(row) => {
                                for (col, value) in batch.cols.iter_mut().zip(row) {
                                    col.push(value);
                                }
                                pos += 1;
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
        if prefix.len() >= 3 {
            return Ok(None);
        }
        let index = self.index(perm);
        let range = index.range(prefix)?;
        let mut lo = range.start;
        let mut hi = range.end;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if index.get(mid)?[prefix.len()] < target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == range.end {
            Ok(None)
        } else {
            Ok(Some(index.get(lo)?[prefix.len()]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode, Triple};
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
        write_index(&mut out, source.indexes[0].iter().copied()).unwrap();
        drop(out);
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        let offset = 8 + 2 * 32;
        manifest[offset..offset + 32].copy_from_slice(&digest(&dir.join("sop")).unwrap());
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify_permutations().is_err());
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
        assert_eq!(disk.lookup(&t), source.lookup(&t));
        assert_eq!(disk.resolve(disk.lookup(&t).unwrap()), t);
        drop(disk);
        let path = dir.join("spo");
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(DiskSnapshot::open(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
