//! Immutable, in-memory reference snapshot. Bulk construction deduplicates RDF triples.
use super::{Dictionary, Snapshot, StorageError};
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::{Term, Triple};
use std::collections::HashMap;

const INLINE: u64 = 0xf000_0000_0000_0000;
fn inline(term: &Term) -> Option<TermId> {
    inline_ref(term.as_ref())
}
fn inline_ref(term: oxrdf::TermRef<'_>) -> Option<TermId> {
    if let oxrdf::TermRef::Literal(l) = term {
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

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
/// A term as stored: kind (1 IRI, 2 blank node, 3 xsd:string literal, 4 language-tagged literal, 5 other typed
/// literal), lexical value, and language tag or datatype IRI (empty for kinds 1-3). Distinct terms have distinct parts.
fn parts(term: oxrdf::TermRef<'_>) -> (u8, &str, &str) {
    match term {
        oxrdf::TermRef::NamedNode(v) => (1, v.as_str(), ""),
        oxrdf::TermRef::BlankNode(v) => (2, v.as_str(), ""),
        oxrdf::TermRef::Literal(v) => match v.language() {
            Some(lang) => (4, v.value(), lang),
            None if v.datatype().as_str() == XSD_STRING => (3, v.value(), ""),
            None => (5, v.value(), v.datatype().as_str()),
        },
    }
}
/// The ID tag (top four bits) that a non-inline term of `kind` carries.
fn tag_of(kind: u8) -> u64 {
    u64::from(kind.min(3))
}
fn is_inline(id: TermId) -> bool {
    id.0 >> 60 == 15
}
fn direct_index(id: TermId) -> usize {
    (id.0 & 0x0fff_ffff_ffff_ffff) as usize
}

/// Location of one term's text in the arena; `kind == 0` marks an unused ID slot.
#[derive(Clone, Copy, Default)]
struct ArenaEntry {
    start: u64,
    len: u32,
    aux_len: u32,
    kind: u8,
}
/// The single copy of every term's text, addressed by ID, plus a hash index from term text to ID.
/// The index stores only `(hash, id)`: terms are compared against the arena, never held a second time.
#[derive(Default)]
struct TermArena {
    text: String,
    direct: Vec<ArenaEntry>,
    inline: HashMap<TermId, ArenaEntry>,
    by_term: hashbrown::HashTable<(u64, TermId)>,
    hasher: std::hash::RandomState,
    len: usize,
}
impl TermArena {
    fn hash(&self, (kind, value, aux): (u8, &str, &str)) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = self.hasher.build_hasher();
        h.write_u8(kind);
        h.write(value.as_bytes());
        h.write_u8(0xff); // never occurs in UTF-8, so (value, aux) splits unambiguously
        h.write(aux.as_bytes());
        h.finish()
    }
    fn entry(&self, id: TermId) -> Option<ArenaEntry> {
        let entry = if is_inline(id) {
            *self.inline.get(&id)?
        } else {
            let entry = *self.direct.get(direct_index(id))?;
            if entry.kind == 0 || tag_of(entry.kind) != id.0 >> 60 {
                return None;
            }
            entry
        };
        Some(entry)
    }
    fn entry_parts(&self, e: ArenaEntry) -> (u8, &str, &str) {
        let start = e.start as usize;
        let mid = start + e.len as usize;
        (e.kind, &self.text[start..mid], &self.text[mid..mid + e.aux_len as usize])
    }
    /// The ID of a stored term, if any.
    fn find(&self, term: oxrdf::TermRef<'_>) -> Option<TermId> {
        if let Some(id) = inline_ref(term) {
            return self.inline.contains_key(&id).then_some(id);
        }
        let key = parts(term);
        self.find_hashed(key, self.hash(key))
    }
    fn find_hashed(&self, key: (u8, &str, &str), hash: u64) -> Option<TermId> {
        self.by_term
            .find(hash, |&(h, id)| h == hash && self.entry(id).is_some_and(|e| self.entry_parts(e) == key))
            .map(|&(_, id)| id)
    }
    /// Stores a term under `id`. `false` (and no change) if that ID or that term is already stored.
    fn insert(&mut self, id: TermId, term: oxrdf::TermRef<'_>) -> bool {
        let key @ (_, value, aux) = parts(term);
        if is_inline(id) {
            if self.inline.contains_key(&id) {
                return false;
            }
            let entry = self.entry_at_end(key);
            self.text.push_str(value);
            self.text.push_str(aux);
            self.inline.insert(id, entry);
            self.len += 1;
            return true;
        }
        let index = direct_index(id);
        let hash = self.hash(key);
        if self.direct.get(index).is_some_and(|e| e.kind != 0) || self.find_hashed(key, hash).is_some() {
            return false;
        }
        self.push_direct(index, id, key, hash);
        true
    }
    /// The entry for `key` if its text is appended to the arena next.
    fn entry_at_end(&self, (kind, value, aux): (u8, &str, &str)) -> ArenaEntry {
        let len = |s: &str| u32::try_from(s.len()).expect("RDF term component longer than 4 GiB");
        ArenaEntry { start: self.text.len() as u64, len: len(value), aux_len: len(aux), kind }
    }
    /// Stores a non-inline term in free slot `index`; the caller has already looked it up by `hash` and found it absent.
    fn push_direct(&mut self, index: usize, id: TermId, key: (u8, &str, &str), hash: u64) {
        let entry = self.entry_at_end(key);
        self.text.push_str(key.1);
        self.text.push_str(key.2);
        if self.direct.len() <= index {
            self.direct.resize(index + 1, ArenaEntry::default());
        }
        self.direct[index] = entry;
        self.by_term.insert_unique(hash, (hash, id), |&(h, _)| h);
        self.len += 1;
    }
    fn get(&self, id: TermId) -> Option<oxrdf::TermRef<'_>> {
        let (kind, value, aux) = self.entry_parts(self.entry(id)?);
        Some(match kind {
            1 => oxrdf::NamedNodeRef::new_unchecked(value).into(),
            2 => oxrdf::BlankNodeRef::new_unchecked(value).into(),
            3 => oxrdf::LiteralRef::new_simple_literal(value).into(),
            4 => oxrdf::LiteralRef::new_language_tagged_literal_unchecked(value, aux).into(),
            5 => oxrdf::LiteralRef::new_typed_literal(value, oxrdf::NamedNodeRef::new_unchecked(aux)).into(),
            _ => return None,
        })
    }
    /// All stored terms in ascending ID order.
    fn sorted_entries(&self) -> Vec<(TermId, oxrdf::TermRef<'_>)> {
        let mut out: Vec<_> = self
            .direct
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind != 0)
            .map(|(i, e)| TermId((tag_of(e.kind) << 60) | i as u64))
            .chain(self.inline.keys().copied())
            .map(|id| (id, self.get(id).expect("stored ID resolves")))
            .collect();
        out.sort_unstable_by_key(|(id, _)| *id);
        out
    }
    /// Drops growth slack once loading is complete.
    fn shrink(&mut self) {
        self.text.shrink_to_fit();
        self.direct.shrink_to_fit();
        self.by_term.shrink_to_fit(|&(h, _)| h);
    }
}

/// One sorted permutation stored without its leading column: each distinct first key appears once in `keys`, with the
/// offset of its group in `tails`, so a row costs 16 bytes instead of 24.
#[derive(Default)]
struct PermIndex {
    keys: Vec<(TermId, usize)>,
    tails: Vec<[TermId; 2]>,
}
impl PermIndex {
    /// From rows sorted by the full key (and deduplicated).
    fn from_sorted(rows: &[[TermId; 3]]) -> Self {
        let mut keys: Vec<(TermId, usize)> = Vec::new();
        let mut tails = Vec::with_capacity(rows.len());
        for row in rows {
            if keys.last().is_none_or(|&(k, _)| k != row[0]) {
                keys.push((row[0], tails.len()));
            }
            tails.push([row[1], row[2]]);
        }
        keys.shrink_to_fit();
        Self { keys, tails }
    }
    fn len(&self) -> usize {
        self.tails.len()
    }
    fn group_end(&self, group: usize) -> usize {
        self.keys.get(group + 1).map_or(self.tails.len(), |&(_, start)| start)
    }
    /// Every group as (first key, its tails), in key order.
    fn groups(&self) -> impl Iterator<Item = (TermId, &[[TermId; 2]])> + '_ {
        self.keys.iter().enumerate().map(|(g, &(k, start))| (k, &self.tails[start..self.group_end(g)]))
    }
    /// All rows in key order.
    fn rows(&self) -> impl Iterator<Item = [TermId; 3]> + '_ {
        self.groups().flat_map(|(k, tails)| tails.iter().map(move |t| [k, t[0], t[1]]))
    }
    /// The tails of the rows that start with `prefix` (1 to 3 IDs), still sorted. An empty prefix returns every tail
    /// with its first key dropped, which is only meaningful for its length (use `rows` for whole rows).
    fn tail(&self, prefix: &[TermId]) -> &[[TermId; 2]] {
        let Some((&first, rest)) = prefix.split_first() else { return &self.tails };
        if rest.len() > 2 {
            return &[];
        }
        let Ok(group) = self.keys.binary_search_by_key(&first, |&(k, _)| k) else { return &[] };
        let tails = &self.tails[self.keys[group].1..self.group_end(group)];
        if rest.is_empty() {
            return tails;
        }
        // Pad to full tails so each probe is a fixed-size array comparison.
        let (mut low, mut high) = ([TermId(0); 2], [TermId(u64::MAX); 2]);
        low[..rest.len()].copy_from_slice(rest);
        high[..rest.len()].copy_from_slice(rest);
        let lo = tails.partition_point(|t| *t < low);
        let hi = lo + tails[lo..].partition_point(|t| *t <= high);
        &tails[lo..hi]
    }
}

pub struct MemorySnapshot {
    arena: TermArena,
    indexes: [PermIndex; 6],
}
impl MemorySnapshot {
    /// Parse N-Triples and construct a snapshot only if the entire input is valid.
    pub fn from_ntriples(reader: impl std::io::Read) -> std::io::Result<Self> {
        let triples = triplelight_core::ntriples::read_ntriples(std::io::BufReader::new(reader)).collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_triples(triples))
    }
    /// Parse Turtle and reject the entire load on syntax or I/O errors.
    pub fn from_turtle(reader: impl std::io::Read) -> std::io::Result<Self> {
        let triples = triplelight_core::turtle::read_turtle(std::io::BufReader::new(reader)).collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_triples(triples))
    }
    /// Parse an N-Triples file in bounded worker threads, then sort the six indexes in parallel.
    /// Dictionary construction remains serial and the full parsed dataset remains in memory.
    pub fn from_ntriples_parallel_file(
        path: impl AsRef<std::path::Path>,
        workers: usize,
    ) -> std::io::Result<Self> {
        use rayon::prelude::*;
        if workers == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workers must be nonzero",
            ));
        }
        let readers = triplelight_core::ntriples::split_ntriples_file(path.as_ref(), workers)?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .map_err(std::io::Error::other)?;
        pool.install(|| {
            let chunks: Result<Vec<Vec<Triple>>, _> = readers
                .into_par_iter()
                .map(|reader| reader.collect())
                .collect();
            let chunks = chunks.map_err(std::io::Error::from)?;
            Ok(Self::build_triples(chunks.into_iter().flatten(), true))
        })
    }
    /// Constructs a complete immutable snapshot from parsed RDF triples.
    pub fn from_triples(triples: impl IntoIterator<Item = Triple>) -> Self {
        Self::build_triples(triples, false)
    }
    /// Build a snapshot incrementally from a fallible stream. A failed stream drops the partial snapshot.
    pub fn try_from_triples<E>(triples: impl IntoIterator<Item = Result<Triple, E>>) -> Result<Self, E> {
        Self::try_build_triples(triples, false)
    }
    fn build_triples(triples: impl IntoIterator<Item = Triple>, parallel_sort: bool) -> Self {
        match Self::try_build_triples(triples.into_iter().map(Ok::<_, std::convert::Infallible>), parallel_sort) {
            Ok(store) => store,
            Err(never) => match never {},
        }
    }
    fn try_build_triples<E>(triples: impl IntoIterator<Item = Result<Triple, E>>, parallel_sort: bool) -> Result<Self, E> {
        let mut store = Self { arena: TermArena::default(), indexes: Default::default() };
        // Only the subject-predicate-object rows grow during the load; the other five orders are derived once the
        // deduplicated triple count is known, so no permutation carries growth slack or duplicates.
        let mut spo = Vec::new();
        for triple in triples {
            let triple = triple?;
            spo.push([
                store.intern(triple.subject.as_ref().into()),
                store.intern(triple.predicate.as_ref().into()),
                store.intern(triple.object.as_ref()),
            ]);
        }
        store.arena.shrink();
        // Each derived order is sorted in a temporary full-row vector, then kept only in its compact form.
        let permute = |spo: &[[TermId; 3]], perm: Perm| {
            let order = perm.order();
            let mut rows: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
            rows.sort_unstable();
            PermIndex::from_sorted(&rows)
        };
        debug_assert_eq!(Perm::Spo.order(), [0, 1, 2]);
        if parallel_sort {
            use rayon::prelude::*;
            spo.par_sort_unstable();
            spo.dedup();
            spo.shrink_to_fit();
            let derived: Vec<_> = Perm::ALL.par_iter().filter(|&&p| p != Perm::Spo).map(|&p| (p, permute(&spo, p))).collect();
            for (perm, rows) in derived {
                store.indexes[slot(perm)] = rows;
            }
        } else {
            spo.sort_unstable();
            spo.dedup();
            spo.shrink_to_fit();
            for perm in Perm::ALL.into_iter().filter(|&p| p != Perm::Spo) {
                store.indexes[slot(perm)] = permute(&spo, perm);
            }
        }
        store.indexes[slot(Perm::Spo)] = PermIndex::from_sorted(&spo);
        Ok(store)
    }
    /// The rows of permutation `index` (in `Perm::ALL` order), sorted by its full key.
    pub(super) fn index_rows(&self, index: usize) -> impl ExactSizeIterator<Item = [TermId; 3]> + '_ {
        /// A row iterator that knows how many rows remain (writers record the count up front).
        struct Counted<I> {
            rows: I,
            left: usize,
        }
        impl<I: Iterator<Item = [TermId; 3]>> Iterator for Counted<I> {
            type Item = [TermId; 3];
            fn next(&mut self) -> Option<[TermId; 3]> {
                let row = self.rows.next()?;
                self.left -= 1;
                Some(row)
            }
            fn size_hint(&self) -> (usize, Option<usize>) {
                (self.left, Some(self.left))
            }
        }
        impl<I: Iterator<Item = [TermId; 3]>> ExactSizeIterator for Counted<I> {}
        let index = &self.indexes[index];
        Counted { rows: index.rows(), left: index.len() }
    }
    /// Writes a portable snapshot stream, including dictionary IDs and all six indexes.
    /// The caller owns atomic file replacement and durability (flush/fsync).
    pub fn write_snapshot(&self, mut out: impl std::io::Write) -> std::io::Result<()> {
        out.write_all(b"RDFSNP01")?;
        out.write_all(&(self.arena.len as u64).to_le_bytes())?;
        for (id, term) in self.terms() {
            let text = term.to_string();
            out.write_all(&id.0.to_le_bytes())?;
            out.write_all(&(text.len() as u64).to_le_bytes())?;
            out.write_all(text.as_bytes())?;
        }
        for index in &self.indexes {
            out.write_all(&(index.len() as u64).to_le_bytes())?;
            for row in index.rows() {
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
        let mut snapshot = Self { arena: TermArena::default(), indexes: Default::default() };
        let mut full: [Vec<[TermId; 3]>; 6] = Default::default();
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
            // The index bound comes first, so a corrupt ID cannot size the arena; `insert` rejects a repeated ID or term.
            if !valid_tag
                || (!is_inline(id) && direct_index(id) as u64 >= n)
                || !snapshot.arena.insert(id, term.as_ref())
            {
                return Err(bad());
            }
        }
        for index in &mut full {
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
                if row.iter().any(|&id| snapshot.arena.entry(id).is_none())
                    || index.last().is_some_and(|prev| prev >= &row)
                {
                    return Err(bad());
                }
                index.push(row);
            }
        }
        // Ensure every permutation represents exactly the same RDF triple set.
        let base = &full[0];
        for perm in Perm::ALL {
            let order = perm.order();
            let mut expected: Vec<_> = base
                .iter()
                .map(|r| [r[order[0]], r[order[1]], r[order[2]]])
                .collect();
            expected.sort_unstable();
            if expected != full[slot(perm)] {
                return Err(bad());
            }
        }
        let mut trailing = [0];
        if input.read(&mut trailing)? != 0 {
            return Err(bad());
        }
        snapshot.arena.shrink();
        for (index, rows) in snapshot.indexes.iter_mut().zip(full) {
            *index = PermIndex::from_sorted(&rows);
        }
        Ok(snapshot)
    }
    fn intern(&mut self, term: oxrdf::TermRef<'_>) -> TermId {
        if let Some(id) = inline_ref(term) {
            self.arena.insert(id, term); // no-op if already stored
            return id;
        }
        // One hash per term: the same hash serves the lookup and, for a new term, the insertion.
        let key = parts(term);
        let hash = self.arena.hash(key);
        if let Some(id) = self.arena.find_hashed(key, hash) {
            return id;
        }
        // Non-inline IDs are dense: the next one is the next arena slot.
        let index = self.arena.direct.len();
        let id = TermId((tag_of(key.0) << 60) | index as u64);
        self.arena.push_direct(index, id, key, hash);
        id
    }
    /// Every stored term with its ID, in ascending ID order (for snapshot and dictionary writers).
    pub(super) fn terms(&self) -> Vec<(TermId, oxrdf::TermRef<'_>)> {
        self.arena.sorted_entries()
    }
    fn index(&self, perm: Perm) -> &PermIndex {
        &self.indexes[slot(perm)]
    }
}
impl Dictionary for MemorySnapshot {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        Ok(self.arena.find(term.as_ref()))
    }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        Ok(self
            .arena
            .get(id)
            .ok_or_else(|| StorageError::new(std::io::ErrorKind::InvalidData, "unknown term ID"))?
            .into_owned())
    }
    fn with_term(
        &self,
        id: TermId,
        f: &mut dyn FnMut(oxrdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        f(self.arena.get(id).ok_or_else(|| {
            StorageError::new(std::io::ErrorKind::InvalidData, "unknown term ID")
        })?);
        Ok(())
    }
}

impl Snapshot for MemorySnapshot {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        let index = self.index(perm);
        let mut rows: Box<dyn Iterator<Item = [TermId; 3]> + 'a> = match prefix.first() {
            None => Box::new(index.rows()),
            Some(&first) => Box::new(index.tail(prefix).iter().map(move |t| [first, t[0], t[1]])),
        };
        Box::new(std::iter::from_fn(move || {
            let mut b = TripleBatch::default();
            for r in rows.by_ref().take(4096) {
                for (column, value) in b.cols.iter_mut().zip(r) {
                    column.push(value);
                }
            }
            (!b.is_empty()).then_some(Ok(b))
        }))
    }
    fn matching_tail(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> {
        (!prefix.is_empty()).then(|| self.index(perm).tail(prefix))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        let index = self.index(perm);
        if let [a, b, c] = prefix {
            return Ok(u64::from(index.tail(&[*a]).binary_search(&[*b, *c]).is_ok()));
        }
        Ok(index.tail(prefix).len() as u64)
    }
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, StorageError> {
        let index = self.index(perm);
        Ok(match prefix.len() {
            0 => {
                let i = index.keys.partition_point(|&(k, _)| k < target);
                index.keys.get(i).map(|&(k, _)| k)
            }
            1 | 2 => {
                let tails = index.tail(prefix);
                let col = prefix.len() - 1;
                let i = tails.partition_point(|t| t[col] < target);
                tails.get(i).map(|t| t[col])
            }
            _ => None,
        })
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
    fn unknown_id_is_error_and_does_not_invoke_callback() {
        let store = MemorySnapshot::from_triples([]);
        assert!(store.lookup(&triple("urn:s", "urn:p", "v").object).unwrap().is_none());
        assert!(store.resolve(TermId(42)).is_err());
        let mut called = false;
        assert!(store.with_term(TermId(42), &mut |_| called = true).is_err());
        assert!(!called);
    }
    #[test]
    fn six_permutations_deduplicate_count_and_seek() {
        let a = triple("urn:s1", "urn:p1", "a");
        let b = triple("urn:s1", "urn:p2", "long value");
        let c = triple("urn:s2", "urn:p1", "a");
        let store = MemorySnapshot::from_triples([a.clone(), b, c, a.clone()]);
        let values = [
            store.lookup(&a.subject.clone().into()).unwrap().unwrap(),
            store.lookup(&a.predicate.clone().into()).unwrap().unwrap(),
            store.lookup(&a.object).unwrap().unwrap(),
        ];
        assert_eq!(store.resolve(values[2]).unwrap(), a.object);
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
            let all: Vec<[TermId; 3]> = store.index_rows(slot(perm)).collect();
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
        let path =
            std::env::temp_dir().join(format!("rdf-parallel-load-{}.nt", std::process::id()));
        let mut text = String::new();
        for i in 0..4000 {
            text.push_str(&format!(
                "_:node{} <urn:p> \"value{}\" .\n",
                i % 100,
                i % 113
            ));
        }
        std::fs::write(&path, &text).unwrap();
        let serial = MemorySnapshot::from_ntriples(text.as_bytes()).unwrap();
        let parallel = MemorySnapshot::from_ntriples_parallel_file(&path, 4).unwrap();
        for perm in Perm::ALL {
            let rows = |s: &MemorySnapshot| -> Vec<[TermId; 3]> {
                s.scan(perm, &[])
                    .map(|b| b.unwrap())
                    .flat_map(|b| {
                        (0..b.len())
                            .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };
            assert_eq!(rows(&serial), rows(&parallel));
        }
        assert!(MemorySnapshot::from_ntriples_parallel_file(&path, 0).is_err());
        text.push_str("not valid");
        std::fs::write(&path, text).unwrap();
        assert!(MemorySnapshot::from_ntriples_parallel_file(&path, 4).is_err());
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
        let ids: Vec<_> = terms.iter().map(|t| store.intern(t.as_ref())).collect();
        for (id, term) in ids.iter().zip(&terms) {
            store
                .with_term(*id, &mut |v| assert_eq!(v.into_owned(), *term))
                .unwrap();
        }
        let mut bytes = Vec::new();
        store.write_snapshot(&mut bytes).unwrap();
        let reopened = MemorySnapshot::read_snapshot(bytes.as_slice()).unwrap();
        for (id, term) in ids.iter().zip(&terms) {
            reopened
                .with_term(*id, &mut |v| assert_eq!(v.into_owned(), *term))
                .unwrap();
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
        let x_id = s.intern(x.as_ref());
        let y_id = s.intern(y.as_ref());
        assert_ne!(x_id, TermId(0));
        assert_ne!(y_id, TermId(0));
        assert_ne!(x_id, y_id);
        for v in ["", "é", "1234567", "12345678"] {
            let t: Term = Literal::new_simple_literal(v).into();
            let id = s.intern(t.as_ref());
            assert_ne!(id, TermId(0));
            assert_eq!(s.resolve(id).unwrap(), t);
            s.with_term(id, &mut |v| assert_eq!(v.into_owned(), t))
                .unwrap();
        }
    }
    #[test]
    fn same_text_in_different_term_kinds_stays_distinct_and_found() {
        let value = "http://example/a-long-value";
        let terms: Vec<Term> = vec![
            NamedNode::new(value).unwrap().into(),
            BlankNode::new("b1").unwrap().into(),
            Literal::new_simple_literal(value).into(),
            Literal::new_language_tagged_literal(value, "en").unwrap().into(),
            Literal::new_typed_literal(value, NamedNode::new("http://example/dt").unwrap()).into(),
            Literal::new_typed_literal("", NamedNode::new(value).unwrap()).into(),
            Literal::new_simple_literal("").into(),
        ];
        let mut s = MemorySnapshot::from_triples([]);
        let ids: Vec<_> = terms.iter().map(|t| s.intern(t.as_ref())).collect();
        let mut distinct = ids.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), terms.len());
        for (id, t) in ids.iter().zip(&terms) {
            assert_eq!(s.lookup(t).unwrap(), Some(*id));
            assert_eq!(s.intern(t.as_ref()), *id, "re-interning returns the stored ID");
            assert_eq!(s.resolve(*id).unwrap(), *t);
        }
        assert_eq!(s.lookup(&Literal::new_simple_literal("absent value").into()).unwrap(), None);
        assert_eq!(s.terms().len(), terms.len());
    }
    #[test]
    fn snapshot_reader_rejects_one_term_under_two_ids() {
        let term = "<http://example/dup>";
        let mut bytes = b"RDFSNP01".to_vec();
        bytes.extend(2u64.to_le_bytes());
        for index in [0u64, 1] {
            bytes.extend(((1u64 << 60) | index).to_le_bytes());
            bytes.extend((term.len() as u64).to_le_bytes());
            bytes.extend(term.as_bytes());
        }
        for _ in 0..6 {
            bytes.extend(0u64.to_le_bytes());
        }
        assert!(MemorySnapshot::read_snapshot(bytes.as_slice()).is_err());
    }
    #[test]
    fn tail_view_count_and_seek_match_a_full_row_model() {
        let triples = (0..1500).map(|i| triple(&format!("urn:s{}", i % 41), &format!("urn:p{}", i % 5), &format!("value {}", i % 29)));
        let store = MemorySnapshot::from_triples(triples);
        for perm in Perm::ALL {
            let all: Vec<[TermId; 3]> = store.index_rows(slot(perm)).collect();
            assert!(all.windows(2).all(|w| w[0] < w[1]), "rows stay sorted and distinct");
            assert!(store.matching_tail(perm, &[]).is_none());
            for row in all.iter().step_by(7) {
                for n in 1..=3 {
                    let prefix = &row[..n];
                    let expected: Vec<[TermId; 2]> = all.iter().filter(|r| r[..n] == *prefix).map(|r| [r[1], r[2]]).collect();
                    assert_eq!(store.matching_tail(perm, prefix).unwrap(), expected.as_slice());
                    assert_eq!(store.count(perm, prefix).unwrap(), expected.len() as u64);
                }
                for n in 0..3 {
                    let prefix = &row[..n];
                    for target in [TermId(0), row[n], TermId(row[n].0 + 1), TermId(u64::MAX)] {
                        let expected = all.iter().filter(|r| r[..n] == *prefix && r[n] >= target).map(|r| r[n]).min();
                        assert_eq!(store.seek(perm, prefix, target).unwrap(), expected, "{perm:?} n={n}");
                    }
                }
            }
            assert_eq!(store.count(perm, &[TermId(1), TermId(2), TermId(3)]).unwrap(), 0);
        }
    }
}
