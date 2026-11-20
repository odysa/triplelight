//! Named graphs on the query side (task #36): GRAPH, FROM, FROM NAMED over a `DatasetView`. The dataset here is a
//! test double sharing one dictionary; expected answers follow SPARQL 1.1 §13 (dataset) and are to be re-checked
//! against Jena on the shared fixture list before the semantics are frozen.
use oxrdf::{NamedNode, Term, TermRef, Triple};
use triplelight::exec::{query_dataset_with, query_with, Options, QueryErrorKind, QueryResult};
use triplelight::model::{Perm, TermId, TripleBatch};
use triplelight::storage::{DatasetView, Dictionary, MemorySnapshot, Snapshot, StorageError};

/// One graph as sorted ID triples per permutation, over a shared dictionary.
struct TestGraph<'a> { dict: &'a MemorySnapshot, rows: Vec<Vec<[TermId; 3]>> }

impl<'a> TestGraph<'a> {
    fn new(dict: &'a MemorySnapshot, triples: &[Triple]) -> Self {
        let ids: Vec<[TermId; 3]> = triples.iter().map(|t| {
            let id = |x: Term| dict.lookup(&x).unwrap().unwrap();
            [id(t.subject.clone().into()), id(t.predicate.clone().into()), id(t.object.clone())]
        }).collect();
        let rows = Perm::ALL.iter().map(|p| {
            let o = p.order();
            let mut v: Vec<[TermId; 3]> = ids.iter().map(|t| [t[o[0]], t[o[1]], t[o[2]]]).collect();
            v.sort(); v.dedup(); v
        }).collect();
        TestGraph { dict, rows }
    }
    fn matching(&self, perm: Perm, prefix: &[TermId]) -> impl Iterator<Item = &[TermId; 3]> {
        let i = Perm::ALL.iter().position(|p| *p == perm).unwrap();
        let prefix = prefix.to_vec();
        self.rows[i].iter().filter(move |r| r[..prefix.len()] == prefix[..])
    }
}

impl Dictionary for TestGraph<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.dict.lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.dict.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.dict.with_term(id, f) }
}

impl Snapshot for TestGraph<'_> {
    fn scan<'b>(&'b self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b> {
        let mut b = TripleBatch::default();
        for r in self.matching(perm, prefix) { for (c, v) in b.cols.iter_mut().zip(r) { c.push(*v) } }
        Box::new((!b.is_empty()).then_some(Ok(b)).into_iter())
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { Ok(self.matching(perm, prefix).count() as u64) }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        Ok(self.matching(perm, prefix).map(|r| r[prefix.len()]).filter(|&v| v >= target).min())
    }
}

struct TestDataset<'a> { default: TestGraph<'a>, named: Vec<(TermId, TestGraph<'a>)> }

impl DatasetView for TestDataset<'_> {
    fn default_graph(&self) -> &dyn Snapshot { &self.default }
    fn graph(&self, name: TermId) -> Result<Option<&dyn Snapshot>, StorageError> {
        Ok(self.named.iter().find(|(n, _)| *n == name).map(|(_, g)| g as &dyn Snapshot))
    }
    fn graph_names(&self) -> Result<Vec<TermId>, StorageError> {
        let mut v: Vec<TermId> = self.named.iter().map(|(n, _)| *n).collect();
        v.sort();
        Ok(v)
    }
}

fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
fn t(s: &str, o: &str) -> Triple { Triple::new(iri(s), iri("p"), oxrdf::Literal::new_simple_literal(o)) }

/// default: a p "d" | g1: a p "1", b p "1" | g2: a p "2", a p "1" | g3: empty (in the catalog).
/// `absent` occurs in the dictionary but is not a graph of the dataset.
fn data() -> (MemorySnapshot, Vec<Triple>, Vec<(&'static str, Vec<Triple>)>) {
    let default = vec![t("a", "d")];
    let graphs = vec![("g1", vec![t("a", "1"), t("b", "1")]), ("g2", vec![t("a", "2"), t("a", "1")]), ("g3", vec![])];
    let mut all: Vec<Triple> = default.iter().chain(graphs.iter().flat_map(|(_, g)| g)).cloned().collect();
    // Graph names need dictionary IDs (shared namespace); these triples belong to no graph view.
    for n in ["g1", "g2", "g3", "absent"] { all.push(Triple::new(iri(n), iri("name"), iri(n))) }
    (MemorySnapshot::from_triples(all), default, graphs)
}

fn run(q: &str) -> Vec<Vec<String>> {
    let (dict, default, graphs) = data();
    let ds = TestDataset {
        default: TestGraph::new(&dict, &default),
        named: graphs.iter().map(|(n, g)| (dict.lookup(&iri(n).into()).unwrap().unwrap(), TestGraph::new(&dict, g))).collect(),
    };
    match query_dataset_with(&ds.default, &ds, q, &Options::default()).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    }
}

fn v(rows: &[&[&str]]) -> Vec<Vec<String>> {
    let mut v: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect();
    v.sort();
    v
}

const P: &str = "PREFIX : <http://ex/> ";

#[test]
fn default_graph_without_dataset_clause_is_the_stored_default() {
    assert_eq!(run(&format!("{P} SELECT ?o {{ :a :p ?o }}")), v(&[&["\"d\""]]));
}

#[test]
fn graph_variable_ranges_over_named_graphs_only() {
    assert_eq!(run(&format!("{P} SELECT ?g ?o {{ GRAPH ?g {{ :a :p ?o }} }}")),
               v(&[&["<http://ex/g1>", "\"1\""], &["<http://ex/g2>", "\"2\""], &["<http://ex/g2>", "\"1\""]]));
}

#[test]
fn empty_group_in_graph_binds_every_catalog_graph_including_empty() {
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ }} }}")), v(&[&["<http://ex/g1>"], &["<http://ex/g2>"], &["<http://ex/g3>"]]));
}

#[test]
fn graph_iri_empty_absent_and_bound_variable() {
    assert_eq!(run(&format!("{P} SELECT ?s {{ GRAPH :g3 {{ ?s ?p ?o }} }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ GRAPH :absent {{ ?s ?p ?o }} }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ GRAPH :g1 {{ ?s :p \"1\" }} }}")), v(&[&["<http://ex/a>"], &["<http://ex/b>"]]));
    // ?g bound before GRAPH: only that graph contributes.
    assert_eq!(run(&format!("{P} SELECT ?o {{ VALUES ?g {{ :g2 }} GRAPH ?g {{ :a :p ?o }} }}")), v(&[&["\"1\""], &["\"2\""]]));
}

#[test]
fn from_merges_graphs_without_duplicates() {
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g1 FROM :g2 {{ :a :p ?o }}")), v(&[&["\"1\""], &["\"2\""]]));
    assert_eq!(run(&format!("{P} SELECT ?s FROM :g1 FROM :g2 {{ ?s :p \"1\" }}")), v(&[&["<http://ex/a>"], &["<http://ex/b>"]]));
    assert_eq!(run(&format!("{P} SELECT (COUNT(*) AS ?n) FROM :g1 FROM :g2 {{ ?s ?p ?o }}")),
               v(&[&["\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>"]]));
    // An absent FROM graph contributes nothing.
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g1 FROM :absent {{ :a :p ?o }}")), v(&[&["\"1\""]]));
}

#[test]
fn from_named_only_has_an_empty_default_graph() {
    assert_eq!(run(&format!("{P} SELECT ?o FROM NAMED :g1 {{ :a :p ?o }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?g FROM NAMED :g1 FROM NAMED :absent {{ GRAPH ?g {{ }} }}")), v(&[&["<http://ex/g1>"]]));
}

#[test]
fn from_without_from_named_has_no_named_graphs() {
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ ?s ?p ?o }} }}")).len(), 4);
    assert_eq!(run(&format!("{P} SELECT ?g FROM :g1 {{ GRAPH ?g {{ ?s ?p ?o }} }}")), v(&[]));
}

#[test]
fn exists_memo_is_per_graph() {
    // The same EXISTS pattern is evaluated in g1, g2 and g3; a memo shared across graphs would leak g1's answer.
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ FILTER EXISTS {{ :a :p \"2\" }} }} }}")), v(&[&["<http://ex/g2>"]]));
}

#[test]
fn count_and_paths_use_the_active_graph() {
    assert_eq!(run(&format!("{P} SELECT (COUNT(*) AS ?n) {{ GRAPH :g2 {{ ?s ?p ?o }} }}")),
               v(&[&["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"]]));
    assert_eq!(run(&format!("{P} SELECT ?o {{ GRAPH :g1 {{ :b :p+ ?o }} }}")), v(&[&["\"1\""]]));
}

#[test]
fn plain_snapshot_queries_still_reject_datasets() {
    let (dict, _, _) = data();
    for q in [format!("{P} SELECT * {{ GRAPH ?g {{ ?s ?p ?o }} }}"), format!("{P} SELECT * FROM :g1 {{ ?s ?p ?o }}")] {
        let e = query_with(&dict, &q, &Options::default()).unwrap_err();
        assert_eq!(e.kind, QueryErrorKind::Unsupported, "{q}");
    }
}
