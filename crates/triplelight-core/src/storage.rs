//! The storage interface the query side is written against (snapshots and the term dictionary).
use crate::model::{Perm, TermId, TripleBatch};
use triplelight_rdf::{NamedNodeRef, Term, TermRef};

pub type StorageError = std::io::Error;

pub trait Dictionary {
    /// Return the existing ID, or None if absent; storage failures are errors.
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError>;
    fn resolve(&self, id: TermId) -> Result<Term, StorageError>;
    /// Borrow a term only for the duration of the callback, allowing cached disk blocks.
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError>;
    /// Callbacks carry input positions and may be delivered in any order. On error,
    /// callers must discard partial output already produced by the callback.
    fn with_terms(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, TermRef<'_>),
    ) -> Result<(), StorageError> {
        for (position, &id) in ids.iter().enumerate() {
            self.with_term(id, &mut |term| f(position, term))?;
        }
        Ok(())
    }
    /// For stores whose stored literal IDs (tag `id_tag::LITERAL`) ascend with the literals' N-Triples text
    /// (`Term::to_string()`, the unified disk dictionary): the smallest stored literal ID whose text is `>= text`, or
    /// `> text` when `strict`. When no stored literal qualifies it is `TermId((id_tag::LITERAL + 1) << 60)`, one past
    /// every literal ID. `Ok(None)` means the store makes no such ordering promise (the default), and callers must
    /// not derive ID ranges from term text. IDs of other tags (overlay deltas, inline terms) are never covered.
    fn literal_text_bound(&self, _text: &str, _strict: bool) -> Result<Option<TermId>, StorageError> { Ok(None) }
    /// The IDs of the subject, predicate and object of triple term `id` (RDF 1.2), or `None` if `id` is not a triple
    /// term. Every part of a stored triple term is a stored term. The default resolves the term and looks up its
    /// parts; stores with a parts table answer from it.
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> {
        let Term::Triple(t) = self.resolve(id)? else { return Ok(None) };
        let t = *t;
        let part = |term: Term| {
            self.lookup(&term)?.ok_or_else(|| StorageError::new(std::io::ErrorKind::InvalidData, "triple-term part missing from the dictionary"))
        };
        Ok(Some([part(t.subject.into())?, part(t.predicate.into())?, part(t.object)?]))
    }
}

/// `Snapshot::first_keys` by one full scan of `perm`: for views that override it only in some states.
pub fn first_keys_by_scan(snap: &(impl Snapshot + ?Sized), perm: Perm) -> Result<Vec<TermId>, StorageError> {
    let (mut keys, mut sorted) = (Vec::<TermId>::new(), true);
    for batch in snap.scan(perm, &[]) {
        for &k in &batch?.cols[0] {
            match keys.last() {
                Some(&last) if last == k => {}
                last => { sorted &= last.is_none_or(|&l| l < k); keys.push(k) }
            }
        }
    }
    // A scan is sorted by contract; a view that breaks it still gets the documented answer.
    if !sorted { keys.sort_unstable(); keys.dedup() }
    Ok(keys)
}

/// `Snapshot::scan_from` by a full prefix scan that skips the rows below `from`.
pub fn scan_from_by_scan<'a>(snap: &'a (impl Snapshot + ?Sized), perm: Perm, prefix: &[TermId], from: TermId) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
    let k = prefix.len();
    if k >= 3 { return snap.scan(perm, prefix) }
    Box::new(snap.scan(perm, prefix).filter_map(move |b| match b {
        Err(e) => Some(Err(e)),
        Ok(b) => {
            let start = b.cols[k].partition_point(|&v| v < from);
            if start == b.len() { return None }
            let mut out = TripleBatch::default();
            for (dst, src) in out.cols.iter_mut().zip(&b.cols) { dst.extend_from_slice(&src[start..]) }
            Some(Ok(out))
        }
    }))
}

/// A consistent read view (snapshot) of the store.
pub trait Snapshot: Dictionary + Send + Sync {
    /// Sorted scan of `perm` where the first `prefix.len()` key components (0..=3) equal `prefix`.
    /// Yields batches of at most ~4096 rows, sorted by the full key.
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>;
    /// Full ordered scan intended for one-pass statistics; backends may bypass their query cache.
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        self.scan(Perm::Spo, &[])
    }
    /// The distinct values of `perm`'s first key column, ascending (every subject for `Spo`, every object for `Osp`).
    /// The default is one full scan; backends may answer it from their block layout instead.
    fn first_keys(&self, perm: Perm) -> Result<Vec<TermId>, StorageError> { first_keys_by_scan(self, perm) }
    /// The rows of `scan(perm, prefix)` from the first whose key column `prefix.len()` is `>= from`, in key order; the
    /// consumer may stop early. The default scans and skips; backends that can position a scan should.
    fn scan_from<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
        from: TermId,
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        scan_from_by_scan(self, perm, prefix, from)
    }
    /// Exact number of triples matching the prefix (from block index / aggregated counts, no full scan).
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError>;
    /// Seek support for leapfrog joins: smallest value >= `target` in key position `prefix.len()` under `prefix`.
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, StorageError>;
    /// Zero-copy view of the rows `scan` would yield (key order of `perm`), for stores that keep the permutation as one
    /// in-memory sorted array. Lets an index nested-loop join probe without allocating a batch per input row.
    fn matching_rows(&self, _perm: Perm, _prefix: &[TermId]) -> Option<&[[TermId; 3]]> { None }
    /// Zero-copy view of the rows matching a non-empty `prefix` without their first key column (which equals
    /// `prefix[0]`): element `t[k - 1]` is key position `k` of `perm`. For stores that keep each permutation's leading
    /// column only once per distinct value. `None` for an empty prefix or when the store has no such layout.
    fn matching_tail(&self, _perm: Perm, _prefix: &[TermId]) -> Option<&[[TermId; 2]]> { None }
    /// Whether this view may hold an RDF 1.2 term (a triple term or a directional literal). A store-level flag, not a
    /// scan: `false` guarantees there is none, so outputs can stay exactly RDF 1.1. Stores without 1.2 support
    /// return `false`.
    fn has_rdf12_terms(&self) -> bool { false }
    /// Sorted literal IDs containing every token. An absent or unusable index returns `None`.
    /// The caller limits candidates to bound working memory; the default uses scalar filtering.
    fn text_candidates(&self, _tokens: &[&str], _limit: usize) -> Option<Vec<TermId>> { None }
    /// A memo the executor may fill with facts that depend only on this view's contents (range pushdown's
    /// per-predicate check), shared by every query on it. Only an immutable view may return one; `None` (the default)
    /// makes the executor recompute per query.
    fn object_memo(&self) -> Option<&ObjectMemo> { None }
}

/// A query-time entailment provider over a read-only RDF graph.
pub trait EntailmentProvider: Send + Sync {
    /// Predicates the provider can infer even when their IRIs are absent from the store dictionary.
    fn virtual_predicates(&self) -> &'static [NamedNodeRef<'static>];

    /// Wraps one graph. Each missing predicate has a stable ID assigned by the executor for this query.
    fn graph<'a>(
        &self,
        source: &'a dyn Snapshot,
        virtual_ids: &[(NamedNodeRef<'static>, TermId)],
    ) -> Result<Box<dyn Snapshot + 'a>, StorageError>;
}

/// See `Snapshot::object_memo`: for a (predicate, datatype IRI), the predicate's stored-literal objects ascending if
/// every one of that datatype is in canonical form, else `None`; for (predicate, "iri-only objects"), `Some` (empty)
/// if every object of the predicate is an IRI.
#[derive(Default)]
pub struct ObjectMemo(pub std::sync::Mutex<std::collections::HashMap<(TermId, &'static str), Option<std::sync::Arc<[TermId]>>>>);


/// An RDF dataset read view: the default graph plus a catalog of named graphs, all sharing one dictionary and one
/// consistent snapshot (provisional query-side contract for named graphs, task #36).
///
/// An absent graph (`graph` returns `Ok(None)`) differs from an empty one (`Ok(Some(view))` with no triples); the
/// catalog lists every named graph, empty ones included. Graph names are dictionary IDs of IRIs.
pub trait DatasetView: Send + Sync {
    /// The stored default graph (distinct from every named graph).
    fn default_graph(&self) -> &dyn Snapshot;
    /// The named graph `name`, or `None` if the catalog does not contain it.
    fn graph(&self, name: TermId) -> Result<Option<&dyn Snapshot>, StorageError>;
    /// Every named graph in the catalog, in ascending ID order, empty graphs included.
    fn graph_names(&self) -> Result<Vec<TermId>, StorageError>;
    /// Whether some blank node occurs in more than one of `graphs`. A FROM default graph is the RDF merge of its
    /// sources, which standardizes blank nodes apart; a plain union over shared IDs is only that merge when no blank
    /// node is shared. `None` means the store cannot tell cheaply, and the evaluator then refuses such a FROM.
    fn shares_blank_nodes(&self, _graphs: &[TermId]) -> Result<Option<bool>, StorageError> { Ok(None) }
}
