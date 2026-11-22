//! Graph views the evaluator builds for a SPARQL dataset (FROM / FROM NAMED, task #36): an empty graph, and the
//! duplicate-free union of several graphs that share one dictionary.
use crate::model::{Perm, TermId, TripleBatch};
use crate::storage::{Dictionary, Snapshot, StorageError};
use triplelight_rdf::{Term, TermRef};

const BATCH: usize = 4096;

/// A graph with no triples whose dictionary is `dict` (FROM NAMED without FROM: the default graph is empty).
pub(crate) struct EmptyGraph<'a> { pub dict: &'a dyn Snapshot }

impl Dictionary for EmptyGraph<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.dict.lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.dict.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.dict.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { self.dict.with_terms(ids, f) }
}

impl Snapshot for EmptyGraph<'_> {
    fn scan<'b>(&'b self, _: Perm, _: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b> { Box::new(std::iter::empty()) }
    fn count(&self, _: Perm, _: &[TermId]) -> Result<u64, StorageError> { Ok(0) }
    fn seek(&self, _: Perm, _: &[TermId], _: TermId) -> Result<Option<TermId>, StorageError> { Ok(None) }
}

/// RDF merge of several graphs over one dictionary (the FROM default graph): every triple once, in key order.
/// Requires two or more members; a single FROM graph is used directly.
pub(crate) struct UnionGraph<'a> { pub members: Vec<&'a dyn Snapshot> }

impl Dictionary for UnionGraph<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.members[0].lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.members[0].resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.members[0].with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { self.members[0].with_terms(ids, f) }
}

/// One member's rows in key order, read batch by batch.
struct Cursor<'b> { batches: Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b>, batch: TripleBatch, at: usize }

impl Cursor<'_> {
    /// The current row, refilling from the next non-empty batch; `Ok(None)` at the end.
    fn peek(&mut self) -> Result<Option<[TermId; 3]>, StorageError> {
        while self.at >= self.batch.len() {
            match self.batches.next() {
                None => return Ok(None),
                Some(b) => { self.batch = b?; self.at = 0; }
            }
        }
        let c = &self.batch.cols;
        Ok(Some([c[0][self.at], c[1][self.at], c[2][self.at]]))
    }
}

impl Snapshot for UnionGraph<'_> {
    fn scan<'b>(&'b self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b> {
        let mut cursors: Vec<Cursor<'b>> = self.members.iter()
            .map(|m| Cursor { batches: m.scan(perm, prefix), batch: TripleBatch::default(), at: 0 }).collect();
        let mut done = false;
        Box::new(std::iter::from_fn(move || {
            if done { return None }
            let mut out = TripleBatch::default();
            while out.len() < BATCH {
                // Smallest current row across members; every member positioned on it advances (duplicates merge).
                let mut min: Option<[TermId; 3]> = None;
                for c in cursors.iter_mut() {
                    match c.peek() {
                        Err(e) => { done = true; return Some(Err(e)) }
                        Ok(Some(r)) if min.is_none_or(|m| r < m) => min = Some(r),
                        Ok(_) => {}
                    }
                }
                let Some(row) = min else { done = true; break };
                for c in cursors.iter_mut() {
                    if matches!(c.peek(), Ok(Some(r)) if r == row) { c.at += 1 }
                }
                for (col, v) in out.cols.iter_mut().zip(row) { col.push(v) }
            }
            (!out.is_empty()).then_some(Ok(out))
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        // Exact: overlapping members make per-member counts an overestimate, so count the merged scan.
        let mut n = 0;
        for b in self.scan(perm, prefix) { n += b?.len() as u64 }
        Ok(n)
    }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        let mut best: Option<TermId> = None;
        for m in &self.members {
            if let Some(v) = m.seek(perm, prefix, target)? { best = Some(best.map_or(v, |b| b.min(v))) }
        }
        Ok(best)
    }
}
