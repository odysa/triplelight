//! Immutable, in-memory reference snapshot. Bulk construction deduplicates RDF triples.
use super::{Dictionary, Snapshot};
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::{Literal, Term, Triple};
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

pub struct MemorySnapshot {
    dictionary: HashMap<Term, TermId>,
    reverse: HashMap<TermId, Term>,
    indexes: [Vec<[TermId; 3]>; 6],
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
    /// Constructs a complete immutable snapshot from parsed RDF triples.
    pub fn from_triples(triples: impl IntoIterator<Item = Triple>) -> Self {
        let mut store = Self {
            dictionary: HashMap::new(),
            reverse: HashMap::new(),
            indexes: Default::default(),
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
        for index in &mut store.indexes {
            index.sort_unstable();
            index.dedup();
        }
        store
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
            TermId((kind << 60) | self.reverse.len() as u64)
        });
        self.dictionary.insert(term.clone(), id);
        self.reverse.insert(id, term);
        id
    }
    fn matching<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> &'a [[TermId; 3]] {
        if prefix.len() > 3 {
            return &[];
        }
        let index = &self.indexes[slot(perm)];
        let lo = index.partition_point(|row| row[..prefix.len()] < *prefix);
        let hi = index.partition_point(|row| row[..prefix.len()] <= *prefix);
        &index[lo..hi]
    }
}
impl Dictionary for MemorySnapshot {
    fn lookup(&self, term: &Term) -> Option<TermId> {
        self.dictionary.get(term).copied()
    }
    fn resolve(&self, id: TermId) -> Term {
        if id.0 & 0xf000_0000_0000_0000 == INLINE {
            let n = ((id.0 >> 56) & 7) as usize;
            let bytes = (0..n).map(|i| (id.0 >> (8 * i)) as u8).collect();
            return Literal::new_simple_literal(
                String::from_utf8(bytes).expect("invalid inline term ID"),
            )
            .into();
        }
        self.reverse.get(&id).expect("unknown term ID").clone()
    }
}
impl Snapshot for MemorySnapshot {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = TripleBatch> + 'a> {
        Box::new(self.matching(perm, prefix).chunks(4096).map(|rows| {
            let mut b = TripleBatch::default();
            for r in rows {
                for (column, value) in b.cols.iter_mut().zip(r) {
                    column.push(*value);
                }
            }
            b
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 {
        self.matching(perm, prefix).len() as u64
    }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Option<TermId> {
        if prefix.len() >= 3 {
            return None;
        }
        let rows = self.matching(perm, prefix);
        let i = rows.partition_point(|row| row[prefix.len()] < target);
        rows.get(i).map(|row| row[prefix.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::NamedNode;
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
                let batches: Vec<_> = store.scan(perm, prefix).collect();
                let count: usize = batches.iter().map(|x| x.len()).sum();
                assert_eq!(store.count(perm, prefix), count as u64);
                assert!(count > 0);
                if n < 3 {
                    assert_eq!(store.seek(perm, prefix, ordered[n]), Some(ordered[n]));
                }
            }
            assert_eq!(store.count(perm, &[]), 3);
            assert_eq!(store.seek(perm, &ordered, ordered[0]), None);
        }
    }
    #[test]
    fn ntriples_loader_rejects_bad_input_and_deduplicates() {
        let row = "<urn:s> <urn:p> \"ok\" .\n";
        let data = format!("{row}{row}");
        let snap = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
        assert_eq!(snap.count(Perm::Spo, &[]), 1);
        assert!(MemorySnapshot::from_ntriples(format!("{row}broken").as_bytes()).is_err());
    }
    #[test]
    fn turtle_loader_handles_prefixes_and_errors() {
        let good = b"@prefix ex: <urn:example:> . ex:s ex:p \"hi\" .";
        assert_eq!(
            MemorySnapshot::from_turtle(good.as_slice())
                .unwrap()
                .count(Perm::Spo, &[]),
            1
        );
        assert!(
            MemorySnapshot::from_turtle(b"@prefix ex: <urn:example:> . ex:s".as_slice()).is_err()
        );
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
        }
    }
}
