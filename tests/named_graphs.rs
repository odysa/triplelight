//! Named graphs on the query side (task #36): GRAPH, FROM, FROM NAMED over a `DatasetView`. The dataset here is a
//! test double sharing one dictionary; expected answers follow SPARQL 1.1 §13 (dataset) and are to be re-checked
//! against Jena on the shared fixture list before the semantics are frozen.
use triplelight_rdf::{NamedNode, Term, TermRef, Triple};
use triplelight::exec::{query_dataset_with, query_with, Options, QueryErrorKind, QueryResult};
use triplelight::model::{Perm, TermId, TripleBatch};
use triplelight::model::{Change, DatasetChange, GraphTarget};
use triplelight::storage::{DatasetView, Dictionary, MemorySnapshot, Snapshot, StorageError};
use triplelight::store::Overlay;

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
    fn shares_blank_nodes(&self, graphs: &[TermId]) -> Result<Option<bool>, StorageError> {
        let mut seen = std::collections::HashMap::new();
        for g in graphs {
            let (_, view) = self.named.iter().find(|(n, _)| n == g).unwrap();
            let mut mine = std::collections::HashSet::new();
            for r in &view.rows[0] {
                for id in [r[0], r[2]] {
                    if matches!(view.dict.resolve(id)?, Term::BlankNode(_)) { mine.insert(id); }
                }
            }
            for id in mine { if seen.insert(id, *g).is_some() { return Ok(Some(true)) } }
        }
        Ok(Some(false))
    }
}

fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
fn t_pq(s: &str, p: &str, o: &str) -> Triple { Triple::new(iri(s), iri(p), triplelight_rdf::Literal::new_simple_literal(o)) }
fn t(s: &str, o: &str) -> Triple { Triple::new(iri(s), iri("p"), triplelight_rdf::Literal::new_simple_literal(o)) }

/// default: a p "d" | g1: a p "1", b p "1" | g2: a p "2", a p "1" | g3: empty (in the catalog) | g4: _:x p "4" | g5: _:x q "5".
/// `absent` occurs in the dictionary but is not a graph of the dataset.
fn data() -> (MemorySnapshot, Vec<Triple>, Vec<(&'static str, Vec<Triple>)>) {
    let default = vec![t("a", "d")];
    // g4 / g5 share the blank node _:x (one dataset input, so the same node in both graphs).
    let x = triplelight_rdf::BlankNode::new("x").unwrap();
    let bn = |p: &str, o: &str| Triple::new(x.clone(), iri(p), triplelight_rdf::Literal::new_simple_literal(o));
    let graphs = vec![("g1", vec![t("a", "1"), t("b", "1")]), ("g2", vec![t("a", "2"), t("a", "1")]), ("g3", vec![]),
                      ("g4", vec![bn("p", "4"), t_pq("iri", "p", "4")]), ("g5", vec![bn("q", "5"), t_pq("iri", "q", "5")])];
    let mut all: Vec<Triple> = default.iter().chain(graphs.iter().flat_map(|(_, g)| g)).cloned().collect();
    // Graph names need dictionary IDs (shared namespace); these triples belong to no graph view.
    for n in ["g1", "g2", "g3", "g4", "g5", "absent"] { all.push(Triple::new(iri(n), iri("name"), iri(n))) }
    (MemorySnapshot::from_triples(all), default, graphs)
}

fn with_dataset<T>(f: impl FnOnce(&TestDataset<'_>) -> T) -> T {
    let (dict, default, graphs) = data();
    let ds = TestDataset {
        default: TestGraph::new(&dict, &default),
        named: graphs.iter().map(|(n, g)| (dict.lookup(&iri(n).into()).unwrap().unwrap(), TestGraph::new(&dict, g))).collect(),
    };
    f(&ds)
}

/// The same dataset as a real store (task #36, step 1): an overlay built by one dataset update. With `in_base`, the
/// default graph sits in the base and the named graphs mix base and delta IDs; without, every term is a delta term.
fn store_dataset(in_base: bool) -> Overlay {
    let (_, default, graphs) = data();
    let base = MemorySnapshot::from_triples(if in_base { default.clone() } else { Vec::new() });
    let mut changes: Vec<DatasetChange> = Vec::new();
    if !in_base {
        changes.extend(default.iter().map(|t| DatasetChange::Quad(GraphTarget::Default, Change::Insert, t.clone())));
    }
    for (n, g) in &graphs {
        changes.push(DatasetChange::Create { graph: iri(n), silent: false });
        changes.extend(g.iter().map(|t| DatasetChange::Quad(GraphTarget::Named(iri(n)), Change::Insert, t.clone())));
    }
    // As in the double, `:absent` is a known IRI that is not a graph: interned by a default-graph triple that the same
    // update deletes again, so it is in the dictionary but in no graph view.
    let mention = Triple::new(iri("absent"), iri("name"), iri("absent"));
    changes.push(DatasetChange::Quad(GraphTarget::Default, Change::Insert, mention.clone()));
    changes.push(DatasetChange::Quad(GraphTarget::Default, Change::Delete, mention));
    let store = Overlay::from_base(std::sync::Arc::new(base)).with_dataset_changes(&changes).unwrap().0;
    let absent = store.lookup(&iri("absent").into()).unwrap().expect(":absent is known to the store dictionary");
    assert!(store.graph(absent).unwrap().is_none() && !store.graph_names().unwrap().contains(&absent));
    assert_eq!(store.count(Perm::Spo, &[]).unwrap(), default.len() as u64, "the mention left no triple behind");
    store
}

/// The answer (sorted rows, or the error kind and message) of `q` on the test double and on both store layouts,
/// which must agree.
fn answer(q: &str) -> Result<Vec<Vec<String>>, (QueryErrorKind, String)> {
    fn rows(r: Result<QueryResult, triplelight::exec::QueryError>) -> Result<Vec<Vec<String>>, (QueryErrorKind, String)> {
        match r {
            Ok(QueryResult::Solutions { rows, .. }) => {
                let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
                v.sort();
                Ok(v)
            }
            Ok(r) => panic!("{r:?}"),
            Err(e) => Err((e.kind, e.message)),
        }
    }
    let double = with_dataset(|ds| rows(query_dataset_with(&ds.default, ds, q, &Options::default())));
    for in_base in [false, true] {
        let store = store_dataset(in_base);
        assert_eq!(rows(query_dataset_with(&store, &store, q, &Options::default())), double, "store (in_base={in_base}) differs: {q}");
    }
    double
}

fn run_err(q: &str) -> triplelight::exec::QueryError {
    let (kind, message) = answer(q).unwrap_err();
    triplelight::exec::QueryError { kind, message }
}

fn run(q: &str) -> Vec<Vec<String>> { answer(q).unwrap() }

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
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ }} }}")), v(&[&["<http://ex/g1>"], &["<http://ex/g2>"], &["<http://ex/g3>"], &["<http://ex/g4>"], &["<http://ex/g5>"]]));
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
    // A present but empty FROM graph contributes nothing.
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g1 FROM :g3 {{ :a :p ?o }}")), v(&[&["\"1\""]]));
}

/// Local-only dataset policy: a FROM / FROM NAMED source missing from the catalog is an error, while a present
/// empty graph is accepted (and listed by FROM NAMED). `GRAPH <missing>` without a dataset clause is an empty match.
#[test]
fn missing_dataset_sources_are_refused_present_empty_ones_accepted() {
    for q in [format!("{P} SELECT ?o FROM :g1 FROM :absent {{ :a :p ?o }}"),
              format!("{P} SELECT ?g FROM NAMED :g1 FROM NAMED :absent {{ GRAPH ?g {{ }} }}"),
              format!("{P} SELECT ?o FROM <http://ex/never-seen> {{ :a :p ?o }}")] {
        let e = run_err(&q);
        assert_eq!(e.kind, QueryErrorKind::Unsupported, "{q}");
        assert!(e.message.contains("not a graph of this database"), "{q}: {}", e.message);
    }
    assert_eq!(run(&format!("{P} SELECT ?g FROM NAMED :g1 FROM NAMED :g3 {{ GRAPH ?g {{ }} }}")), v(&[&["<http://ex/g1>"], &["<http://ex/g3>"]]));
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g3 {{ :a :p ?o }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ GRAPH :absent {{ ?s ?p ?o }} }}")), v(&[]));
}

#[test]
fn from_named_only_has_an_empty_default_graph() {
    assert_eq!(run(&format!("{P} SELECT ?o FROM NAMED :g1 {{ :a :p ?o }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?g FROM NAMED :g1 {{ GRAPH ?g {{ }} }}")), v(&[&["<http://ex/g1>"]]));
}

#[test]
fn from_without_from_named_has_no_named_graphs() {
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ ?s ?p ?o }} }}")).len(), 8);
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

#[test]
fn from_merge_with_shared_blank_nodes_is_refused() {
    // Controls after Jianlin's bnode-merge-controls: g4 = {_:x p "4", :iri p "4"}, g5 = {_:x q "5", :iri q "5"}.
    // A real RDF merge would answer only :iri (the blank node standardized apart); a shared-ID union would also return
    // _:x. Neither is computed: the FROM merge of graphs sharing a blank node is refused (scoped limitation).
    let e = run_err(&format!("{P} SELECT ?x FROM :g4 FROM :g5 {{ ?x :p \"4\" ; :q \"5\" }}"));
    assert_eq!(e.kind, QueryErrorKind::Unsupported);
    // A single FROM graph, or several graphs without shared blank nodes, is fine.
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g4 {{ ?x :p ?o }}")), v(&[&["\"4\""], &["\"4\""]]));
    assert_eq!(run(&format!("{P} SELECT ?o FROM :g1 FROM :g4 {{ ?x :p ?o }}")).len(), 4);
    // GRAPH keeps the dataset's own blank node identity across graphs (no merge): both :iri and the shared _:x join.
    let rows = run(&format!("{P} SELECT ?x {{ GRAPH :g4 {{ ?x :p \"4\" }} GRAPH :g5 {{ ?x :q \"5\" }} }}"));
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r[0] == "<http://ex/iri>") && rows.iter().any(|r| r[0].starts_with("_:")), "{rows:?}");
}

#[test]
fn graph_inside_exists_with_an_inner_only_variable() {
    // ?g appears only inside EXISTS; the default graph holds :a :p "d", and only g2 holds :a :p "2".
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p ?o FILTER EXISTS {{ GRAPH ?g {{ ?s :p \"2\" }} }} }}")), v(&[&["<http://ex/a>"]]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p ?o FILTER NOT EXISTS {{ GRAPH ?g {{ ?s :p \"2\" }} }} }}")), v(&[]));
}

#[test]
fn graph_inside_exists_with_an_outer_bound_graph_variable() {
    // GRAPH ?g {} binds every catalog graph; EXISTS then evaluates in that same graph only.
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ }} FILTER EXISTS {{ GRAPH ?g {{ :b :p \"1\" }} }} }}")), v(&[&["<http://ex/g1>"]]));
    assert_eq!(run(&format!("{P} SELECT ?g {{ GRAPH ?g {{ }} FILTER NOT EXISTS {{ GRAPH ?g {{ :a :p ?x }} }} }}")),
               v(&[&["<http://ex/g3>"], &["<http://ex/g4>"], &["<http://ex/g5>"]]));
}

#[test]
fn graph_inside_exists_with_constant_absent_and_empty_graphs() {
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p \"d\" FILTER EXISTS {{ GRAPH :g2 {{ ?s :p \"2\" }} }} }}")), v(&[&["<http://ex/a>"]]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p \"d\" FILTER EXISTS {{ GRAPH :g1 {{ ?s :p \"2\" }} }} }}")), v(&[]));
    // An absent graph and an empty graph both make EXISTS false.
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p \"d\" FILTER EXISTS {{ GRAPH :absent {{ ?x ?y ?z }} }} }}")), v(&[]));
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p \"d\" FILTER EXISTS {{ GRAPH :g3 {{ ?x ?y ?z }} }} }}")), v(&[]));
}

#[test]
fn graph_inside_exists_is_still_refused_without_a_dataset() {
    let store = MemorySnapshot::from_triples([t("a", "d")]);
    let err = triplelight::exec::query(&store, &format!("{P} SELECT ?s {{ ?s :p ?o FILTER EXISTS {{ GRAPH ?g {{ ?s :p ?o }} }} }}"))
        .unwrap_err();
    assert_eq!(err.kind, triplelight::exec::QueryErrorKind::Unsupported);
}

#[test]
fn graph_variable_only_inside_exists_in_order_by_and_aggregates() {
    // ?h appears only inside EXISTS in ORDER BY / an aggregate argument; it must still get a variable slot.
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p ?o }} ORDER BY DESC(EXISTS {{ GRAPH ?h {{ ?s :p \"2\" }} }})")),
               v(&[&["<http://ex/a>"]]));
    assert_eq!(run(&format!("{P} SELECT (SUM(IF(EXISTS {{ GRAPH ?h {{ ?s :p \"2\" }} }}, 1, 0)) AS ?n) {{ ?s :p ?o }}")),
               v(&[&["\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"]]));
}

#[test]
fn graph_variable_only_in_a_sort_inside_a_nested_exists_subquery() {
    // ?h appears only in an ORDER BY key of a subquery inside FILTER EXISTS.
    assert_eq!(run(&format!("{P} SELECT ?s {{ ?s :p ?o FILTER EXISTS {{ SELECT ?s {{ ?s :p ?x }} ORDER BY DESC(EXISTS {{ GRAPH ?h {{ ?s :p \"2\" }} }}) }} }}")),
               v(&[&["<http://ex/a>"]]));
    // Same inside BIND and OPTIONAL's filter.
    assert_eq!(run(&format!("{P} SELECT ?s ?b {{ ?s :p ?o BIND(EXISTS {{ SELECT ?s {{ ?s :p ?x }} ORDER BY (EXISTS {{ GRAPH ?h {{ ?s :p \"2\" }} }}) }} AS ?b) }}")),
               v(&[&["<http://ex/a>", "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"]]));
}
