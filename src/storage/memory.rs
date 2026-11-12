//! Immutable, in-memory reference snapshot. Bulk construction deduplicates RDF triples.
use super::{Dictionary, Snapshot, StorageError};
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::{Term, Triple};
use std::collections::HashMap;

const INLINE: u64 = 0xf000_0000_0000_0000;
fn inline(term: &Term) -> Option<TermId> {
    if let Term::Literal(l) = term {
        let b = l.value().as_bytes();
        if l.language().is_none()
            && l.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#string"
            && b.len() <= 7
        {
            let mut n = INLINE | (b.len() as u64) << 56;
            for (i, byte) in b.iter().enumerate() {
                n |= (*byte as u64) << (i * 8);
            }
            return Some(TermId(n));
        }
    }
    None
}
fn slot(p: Perm) -> usize {
    Perm::ALL.iter().position(|&v| v == p).unwrap()
}

#[derive(Clone, Copy)]
struct ArenaEntry {
    kind: u8,
    start: usize,
    end: usize,
    aux_start: usize,
    aux_end: usize,
}
#[derive(Default)]
struct TermArena {
    text: String,
    direct: Vec<Option<(TermId, ArenaEntry)>>,
    inline: HashMap<TermId, ArenaEntry>,
}
impl TermArena {
    fn insert(&mut self, id: TermId, term: &Term) {
        let (kind, value, aux) = match term {
            Term::NamedNode(v) => (1, v.as_str(), ""),
            Term::BlankNode(v) => (2, v.as_str(), ""),
            Term::Literal(v) => match v.language() {
                Some(lang) => (4, v.value(), lang),
                None if v.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#string" => {
                    (3, v.value(), "")
                }
                None => (5, v.value(), v.datatype().as_str()),
            },
        };
        let start = self.text.len();
        self.text.push_str(value);
        let end = self.text.len();
        let aux_start = end;
        self.text.push_str(aux);
        let aux_end = self.text.len();
        let entry = ArenaEntry {
            kind,
            start,
            end,
            aux_start,
            aux_end,
        };
        if id.0 >> 60 == 15 {
            self.inline.insert(id, entry);
        } else {
            let index = (id.0 & 0x0fff_ffff_ffff_ffff) as usize;
            if self.direct.len() <= index {
                self.direct.resize(index + 1, None);
            }
            self.direct[index] = Some((id, entry));
        }
    }
    fn get(&self, id: TermId) -> oxrdf::TermRef<'_> {
        let entry = if id.0 >> 60 == 15 {
            *self.inline.get(&id).expect("unknown inline term ID")
        } else {
            let (found, entry) = self
                .direct
                .get((id.0 & 0x0fff_ffff_ffff_ffff) as usize)
                .and_then(Option::as_ref)
                .expect("unknown term ID");
            assert_eq!(*found, id, "unknown term ID");
            *entry
        };
        let value = &self.text[entry.start..entry.end];
        let aux = &self.text[entry.aux_start..entry.aux_end];
        match entry.kind {
            1 => oxrdf::NamedNodeRef::new_unchecked(value).into(),
            2 => oxrdf::BlankNodeRef::new_unchecked(value).into(),
            3 => oxrdf::LiteralRef::new_simple_literal(value).into(),
            4 => oxrdf::LiteralRef::new_language_tagged_literal_unchecked(value, aux).into(),
            5 => {
                oxrdf::LiteralRef::new_typed_literal(value, oxrdf::NamedNodeRef::new_unchecked(aux))
                    .into()
            }
            _ => unreachable!(),
        }
    }
}

pub struct MemorySnapshot {
    pub(super) dictionary: HashMap<Term, TermId>,
    reverse: HashMap<TermId, Term>,
    arena: TermArena,
    pub(super) indexes: [Vec<[TermId; 3]>; 6],
    first_keys: [Vec<(TermId, usize)>; 6],
}
impl MemorySnapshot {
    /// Parse N-Triples and construct a snapshot only if the entire input is valid.
    pub fn from_ntriples(reader: impl std::io::Read) -> Result<Self, oxttl::TurtleParseError> {
        let triples = oxttl::NTriplesParser::new()
            .for_reader(reader)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_triples(triples))
    }
    /// Parse Turtle and reject the entire load on syntax or I/O errors.
    pub fn from_turtle(reader: impl std::io::Read) -> Result<Self, oxttl::TurtleParseError> {
        let triples = oxttl::TurtleParser::new()
            .for_reader(reader)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_triples(triples))
    }
    /// Parse an N-Triples file in bounded worker threads, then sort the six indexes in parallel.
    /// Dictionary construction remains serial and the full parsed dataset remains in memory.
    pub fn from_ntriples_parallel_file(path: impl AsRef<std::path::Path>, workers: usize) -> std::io::Result<Self> {
        use rayon::prelude::*;
        if workers == 0 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "workers must be nonzero")); }
        let readers = oxttl::NTriplesParser::new().split_file_for_parallel_parsing(path, workers)?;
        let pool = rayon::ThreadPoolBuilder::new().num_threads(workers).build().map_err(std::io::Error::other)?;
        pool.install(|| {
            let chunks: Result<Vec<Vec<Triple>>, _> = readers.into_par_iter().map(|reader| reader.collect()).collect();
            let chunks = chunks.map_err(std::io::Error::other)?;
            Ok(Self::build_triples(chunks.into_iter().flatten(), true))
        })
    }
    /// Constructs a complete immutable snapshot from parsed RDF triples.
    pub fn from_triples(triples: impl IntoIterator<Item = Triple>) -> Self {
        Self::build_triples(triples, false)
    }
    fn build_triples(triples: impl IntoIterator<Item = Triple>, parallel_sort: bool) -> Self {
        let mut store = Self {
            dictionary: HashMap::new(),
            reverse: HashMap::new(),
            arena: TermArena::default(),
            indexes: Default::default(),
            first_keys: Default::default(),
        };
        for triple in triples {
            let ids = [
                store.intern(triple.subject.into()),
                store.intern(triple.predicate.into()),
                store.intern(triple.object),
            ];
            for perm in Perm::ALL {
                let order = perm.order();
                store.indexes[slot(perm)].push([ids[order[0]], ids[order[1]], ids[order[2]]]);
            }
        }
        if parallel_sort {
            use rayon::prelude::*;
            store.indexes.par_iter_mut().for_each(|index| { index.sort_unstable(); index.dedup(); });
        } else {
            for index in &mut store.indexes { index.sort_unstable(); index.dedup(); }
        }
        store.rebuild_first_keys();
        store
    }
    fn rebuild_first_keys(&mut self) {
        for (idx, keys) in self.indexes.iter().zip(&mut self.first_keys) {
            keys.clear();
            for (offset, row) in idx.iter().enumerate() {
                if keys.last().is_none_or(|entry| entry.0 != row[0]) {
                    keys.push((row[0], offset));
                }
            }
        }
    }
    /// Writes a portable snapshot stream, including dictionary IDs and all six indexes.
    /// The caller owns atomic file replacement and durability (flush/fsync).
    pub fn write_snapshot(&self, mut out: impl std::io::Write) -> std::io::Result<()> {
        out.write_all(b"RDFSNP01")?;
        out.write_all(&(self.dictionary.len() as u64).to_le_bytes())?;
        let mut entries: Vec<_> = self
            .dictionary
            .iter()
            .map(|(term, id)| (id, term))
            .collect();
        entries.sort_unstable_by_key(|(id, _)| **id);
        for (id, term) in entries {
            let text = term.to_string();
            out.write_all(&id.0.to_le_bytes())?;
            out.write_all(&(text.len() as u64).to_le_bytes())?;
            out.write_all(text.as_bytes())?;
        }
        for index in &self.indexes {
            out.write_all(&(index.len() as u64).to_le_bytes())?;
            for row in index {
                for id in row {
                    out.write_all(&id.0.to_le_bytes())?;
                }
            }
        }
        Ok(())
    }
    /// Reads and validates a snapshot stream. Not a memory-mapped on-disk index.
    pub fn read_snapshot(mut input: impl std::io::Read) -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind};
        fn num(r: &mut impl std::io::Read) -> std::io::Result<u64> {
            let mut b = [0; 8];
            r.read_exact(&mut b)?;
            Ok(u64::from_le_bytes(b))
        }
        fn bad() -> Error {
            Error::new(ErrorKind::InvalidData, "invalid RDF snapshot")
        }
        let mut magic = [0; 8];
        input.read_exact(&mut magic)?;
        if &magic != b"RDFSNP01" {
            return Err(bad());
        }
        let mut snapshot = Self {
            dictionary: HashMap::new(),
            reverse: HashMap::new(),
            arena: TermArena::default(),
            indexes: Default::default(),
            first_keys: Default::default(),
        };
        let n = num(&mut input)?;
        // An explicit safety cap limits allocation when reading corrupt/untrusted files.
        if n > 100_000_000 {
            return Err(bad());
        }
        for _ in 0..n {
            let id = TermId(num(&mut input)?);
            let len = num(&mut input)?;
            if id.0 == 0 || len > 16_777_216 {
                return Err(bad());
            }
            let mut bytes = vec![0; len as usize];
            input.read_exact(&mut bytes)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| bad())?;
            let term: Term = text.parse().map_err(|_| bad())?;
            let tag = id.0 >> 60;
            let valid_tag = match &term {
                Term::NamedNode(_) => tag == 1,
                Term::BlankNode(_) => tag == 2,
                Term::Literal(_) => {
                    if let Some(expected) = inline(&term) {
                        expected == id
                    } else {
                        tag == 3
                    }
                }
            };
            if !valid_tag
                || snapshot.dictionary.insert(term.clone(), id).is_some()
                || snapshot.reverse.insert(id, term).is_some()
            {
                return Err(bad());
            }
        }
        for index in &mut snapshot.indexes {
            let count = num(&mut input)?;
            if count > 100_000_000 {
                return Err(bad());
            }
            for _ in 0..count {
                let row = [
                    TermId(num(&mut input)?),
                    TermId(num(&mut input)?),
                    TermId(num(&mut input)?),
                ];
                if row.iter().any(|id| !snapshot.reverse.contains_key(id))
                    || index.last().is_some_and(|prev| prev >= &row)
                {
                    return Err(bad());
                }
                index.push(row);
            }
        }
        // Ensure every permutation represents exactly the same RDF triple set.
        let base = &snapshot.indexes[0];
        for perm in Perm::ALL {
            let order = perm.order();
            let mut expected: Vec<_> = base
                .iter()
                .map(|r| [r[order[0]], r[order[1]], r[order[2]]])
                .collect();
            expected.sort_unstable();
            if expected != snapshot.indexes[slot(perm)] {
                return Err(bad());
            }
        }
        let mut trailing = [0];
        if input.read(&mut trailing)? != 0 {
            return Err(bad());
        }
        for (&id, term) in &snapshot.reverse {
            if id.0 >> 60 != 15 {
                let index = id.0 & 0x0fff_ffff_ffff_ffff;
                if index >= n
                    || snapshot
                        .arena
                        .direct
                        .get(index as usize)
                        .is_some_and(Option::is_some)
                {
                    return Err(bad());
                }
            }
            snapshot.arena.insert(id, term);
        }
        snapshot.reverse = HashMap::new();
        snapshot.rebuild_first_keys();
        Ok(snapshot)
    }
    fn intern(&mut self, term: Term) -> TermId {
        if let Some(&id) = self.dictionary.get(&term) {
            return id;
        }
        let id = inline(&term).unwrap_or_else(|| {
            let kind = match &term {
                Term::NamedNode(_) => 1,
                Term::BlankNode(_) => 2,
                Term::Literal(_) => 3,
            };
            TermId((kind << 60) | self.dictionary.len() as u64)
        });
        self.arena.insert(id, &term);
        self.dictionary.insert(term, id);
        id
    }
    fn matching<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> &'a [[TermId; 3]] {
        if prefix.len() > 3 {
            return &[];
        }
        let order = slot(perm);
        let index = &self.indexes[order];
        if prefix.is_empty() {
            return index;
        }
        let keys = &self.first_keys[order];
        let Ok(group) = keys.binary_search_by_key(&prefix[0], |entry| entry.0) else {
            return &[];
        };
        let start = keys[group].1;
        let end = keys.get(group + 1).map_or(index.len(), |entry| entry.1);
        let index = &index[start..end];
        if prefix.len() == 1 {
            return index;
        }
        // Pad the prefix to full keys so each probe is a fixed-size array comparison, not a slice comparison.
        let (mut low, mut high) = ([TermId(0); 3], [TermId(u64::MAX); 3]);
        low[..prefix.len()].copy_from_slice(prefix);
        high[..prefix.len()].copy_from_slice(prefix);
        let lo = index.partition_point(|row| *row < low);
        let hi = lo + index[lo..].partition_point(|row| *row <= high);
        &index[lo..hi]
    }
}
impl Dictionary for MemorySnapshot {
    fn lookup(&self, term: &Term) -> Option<TermId> {
        self.dictionary.get(term).copied()
    }
    fn resolve_ref(&self, id: TermId) -> oxrdf::TermRef<'_> {
        self.arena.get(id)
    }
    fn resolve(&self, id: TermId) -> Term {
        self.arena.get(id).into_owned()
    }
}
impl Snapshot for MemorySnapshot {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        Box::new(self.matching(perm, prefix).chunks(4096).map(|rows| {
            let mut b = TripleBatch::default();
            for r in rows {
                for (column, value) in b.cols.iter_mut().zip(r) {
                    column.push(*value);
                }
            }
            Ok(b)
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        if let [a, b, c] = prefix {
            return Ok(u64::from(
                self.matching(perm, &[*a])
                    .binary_search(&[*a, *b, *c])
                    .is_ok(),
            ));
        }
        Ok(self.matching(perm, prefix).len() as u64)
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
        let rows = self.matching(perm, prefix);
        let i = rows.partition_point(|row| row[prefix.len()] < target);
        Ok(rows.get(i).map(|row| row[prefix.len()]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{BlankNode, Literal, NamedNode};
    fn triple(s: &str, p: &str, o: &str) -> Triple {
        Triple::new(
            NamedNode::new(s).unwrap(),
            NamedNode::new(p).unwrap(),
            Literal::new_simple_literal(o),
        )
    }
    #[test]
    fn six_permutations_deduplicate_count_and_seek() {
        let a = triple("urn:s1", "urn:p1", "a");
        let b = triple("urn:s1", "urn:p2", "long value");
        let c = triple("urn:s2", "urn:p1", "a");
        let store = MemorySnapshot::from_triples([a.clone(), b, c, a.clone()]);
        let values = [
            store.lookup(&a.subject.clone().into()).unwrap(),
            store.lookup(&a.predicate.clone().into()).unwrap(),
            store.lookup(&a.object).unwrap(),
        ];
        assert_eq!(store.resolve(values[2]), a.object);
        for perm in Perm::ALL {
            let order = perm.order();
            let ordered = [values[order[0]], values[order[1]], values[order[2]]];
            for n in 0..=3 {
                let prefix = &ordered[..n];
                let batches: Vec<_> = store.scan(perm, prefix).collect::<Result<_, _>>().unwrap();
                let count: usize = batches.iter().map(|x| x.len()).sum();
                assert_eq!(store.count(perm, prefix).unwrap(), count as u64);
                assert!(count > 0);
                if n < 3 {
                    assert_eq!(
                        store.seek(perm, prefix, ordered[n]).unwrap(),
                        Some(ordered[n])
                    );
                }
            }
            assert_eq!(store.count(perm, &[]).unwrap(), 3);
            assert_eq!(
                store
                    .count(perm, &[TermId(0), ordered[1], ordered[2]])
                    .unwrap(),
                0
            );
            assert_eq!(store.seek(perm, &ordered, ordered[0]).unwrap(), None);
        }
    }
    #[test]
    fn first_key_ranges_match_independent_full_scan() {
        let triples = (0..1000).map(|i| {
            triple(
                &format!("urn:s{}", i % 53),
                &format!("urn:p{}", i % 7),
                &format!("v{}", i % 31),
            )
        });
        let store = MemorySnapshot::from_triples(triples);
        for perm in Perm::ALL {
            let all: Vec<[TermId; 3]> = store.indexes[slot(perm)].clone();
            for row in all.iter().step_by(11) {
                for n in 0..=3 {
                    let prefix = &row[..n];
                    let expected: Vec<_> =
                        all.iter().copied().filter(|r| r[..n] == *prefix).collect();
                    let actual: Vec<_> = store
                        .scan(perm, prefix)
                        .map(|b| b.unwrap())
                        .flat_map(|b| {
                            (0..b.len())
                                .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    assert_eq!(actual, expected);
                    assert_eq!(store.count(perm, prefix).unwrap(), expected.len() as u64);
                }
            }
        }
    }
    #[test]
    fn ntriples_loader_rejects_bad_input_and_deduplicates() {
        let row = "<urn:s> <urn:p> \"ok\" .\n";
        let data = format!("{row}{row}");
        let snap = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
        assert_eq!(snap.count(Perm::Spo, &[]).unwrap(), 1);
        assert!(MemorySnapshot::from_ntriples(format!("{row}broken").as_bytes()).is_err());
    }
    #[test]
    fn parallel_file_load_matches_serial_and_rejects_invalid() {
        let path=std::env::temp_dir().join(format!("rdf-parallel-load-{}.nt",std::process::id()));
        let mut text=String::new();
        for i in 0..4000 { text.push_str(&format!("_:node{} <urn:p> \"value{}\" .\n", i%100, i%113)); }
        std::fs::write(&path,&text).unwrap();
        let serial=MemorySnapshot::from_ntriples(text.as_bytes()).unwrap();
        let parallel=MemorySnapshot::from_ntriples_parallel_file(&path,4).unwrap();
        for perm in Perm::ALL {
            let rows=|s:&MemorySnapshot| -> Vec<[TermId;3]> { s.scan(perm,&[]).map(|b|b.unwrap()).flat_map(|b|(0..b.len()).map(|i|[b.cols[0][i],b.cols[1][i],b.cols[2][i]]).collect::<Vec<_>>()).collect() };
            assert_eq!(rows(&serial),rows(&parallel));
        }
        assert!(MemorySnapshot::from_ntriples_parallel_file(&path,0).is_err());
        text.push_str("not valid");std::fs::write(&path,text).unwrap();
        assert!(MemorySnapshot::from_ntriples_parallel_file(&path,4).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn turtle_loader_handles_prefixes_and_errors() {
        let good = b"@prefix ex: <urn:example:> . ex:s ex:p \"hi\" .";
        assert_eq!(
            MemorySnapshot::from_turtle(good.as_slice())
                .unwrap()
                .count(Perm::Spo, &[])
                .unwrap(),
            1
        );
        assert!(
            MemorySnapshot::from_turtle(b"@prefix ex: <urn:example:> . ex:s".as_slice()).is_err()
        );
    }
    #[test]
    fn snapshot_roundtrip_and_corruption() {
        let source = MemorySnapshot::from_triples([triple("urn:s", "urn:p", "hello")]);
        let mut bytes = Vec::new();
        source.write_snapshot(&mut bytes).unwrap();
        let restored = MemorySnapshot::read_snapshot(bytes.as_slice()).unwrap();
        for perm in Perm::ALL {
            let a: Vec<_> = source
                .scan(perm, &[])
                .flat_map(|b| b.unwrap().cols[0].clone())
                .collect();
            let b: Vec<_> = restored
                .scan(perm, &[])
                .flat_map(|b| b.unwrap().cols[0].clone())
                .collect();
            assert_eq!(a, b);
            assert_eq!(
                source.count(perm, &[]).unwrap(),
                restored.count(perm, &[]).unwrap()
            );
        }
        assert!(MemorySnapshot::read_snapshot(&bytes[..bytes.len() - 1]).is_err());
        bytes.push(0);
        assert!(MemorySnapshot::read_snapshot(bytes.as_slice()).is_err());
    }
    #[test]
    fn borrowed_arena_roundtrips_all_rdf_term_kinds() {
        let mut store = MemorySnapshot::from_triples([]);
        let terms: Vec<Term> = vec![
            NamedNode::new("urn:例").unwrap().into(),
            BlankNode::new("abc").unwrap().into(),
            Literal::new_simple_literal("").into(),
            Literal::new_simple_literal("🦀long string").into(),
            Literal::new_language_tagged_literal("bonjour", "FR")
                .unwrap()
                .into(),
            Literal::new_typed_literal(
                "01",
                NamedNode::new("http://www.w3.org/2001/XMLSchema#integer").unwrap(),
            )
            .into(),
        ];
        let ids: Vec<_> = terms.iter().map(|t| store.intern(t.clone())).collect();
        for (id, term) in ids.iter().zip(&terms) {
            assert_eq!(store.resolve_ref(*id).into_owned(), *term);
        }
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).unwrap();
        let reopened = MemorySnapshot::read_snapshot(bytes.as_slice()).unwrap();
        for (id, term) in ids.iter().zip(&terms) {
            assert_eq!(reopened.resolve_ref(*id).into_owned(), *term);
        }
    }
    #[test]
    fn rdf_terms_do_not_collapse_lexical_forms() {
        let mut s = MemorySnapshot::from_triples([]);
        let x: Term = Literal::new_typed_literal(
            "01",
            NamedNode::new("http://www.w3.org/2001/XMLSchema#integer").unwrap(),
        )
        .into();
        let y: Term = Literal::new_typed_literal(
            "1",
            NamedNode::new("http://www.w3.org/2001/XMLSchema#integer").unwrap(),
        )
        .into();
        let x_id = s.intern(x);
        let y_id = s.intern(y);
        assert_ne!(x_id, TermId(0));
        assert_ne!(y_id, TermId(0));
        assert_ne!(x_id, y_id);
        for v in ["", "é", "1234567", "12345678"] {
            let t: Term = Literal::new_simple_literal(v).into();
            let id = s.intern(t.clone());
            assert_ne!(id, TermId(0));
            assert_eq!(s.resolve(id), t);
            assert_eq!(s.resolve_ref(id).into_owned(), t);
        }
    }
}
