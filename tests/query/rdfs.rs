//! RDFS entailment (`Entailment::Rdfs`, task #108) against an oracle: the graph closed by naive forward chaining over
//! the same rules (rdfs2, 3, 5, 7, 9, 11, no axioms), stored as plain triples and queried under simple entailment.
//! Every query must give the same multiset of solutions both ways.
use std::collections::BTreeSet;
use triplelight::exec::{query_with, Entailment, Options, QueryErrorKind, QueryResult};
use triplelight::storage::MemorySnapshot;

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SC: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const SP: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const DOM: &str = "<http://www.w3.org/2000/01/rdf-schema#domain>";
const RNG: &str = "<http://www.w3.org/2000/01/rdf-schema#range>";

/// A small university: a class hierarchy with a cycle, subproperties two deep, domains and ranges on super- and
/// subproperties, a literal-valued property with a range (no literal may become an instance), and asserted triples
/// that are also derivable (they must match once).
const DATA: &str = r#"
<http://ex/GradStudent> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Student> .
<http://ex/Student> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Person> .
<http://ex/Professor> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Faculty> .
<http://ex/Faculty> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Person> .
<http://ex/Person> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Agent> .
<http://ex/Agent> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Person> .
<http://ex/headOf> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> <http://ex/worksFor> .
<http://ex/worksFor> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> <http://ex/memberOf> .
<http://ex/advisor> <http://www.w3.org/2000/01/rdf-schema#domain> <http://ex/Student> .
<http://ex/advisor> <http://www.w3.org/2000/01/rdf-schema#range> <http://ex/Professor> .
<http://ex/memberOf> <http://www.w3.org/2000/01/rdf-schema#domain> <http://ex/Person> .
<http://ex/memberOf> <http://www.w3.org/2000/01/rdf-schema#range> <http://ex/Organization> .
<http://ex/headOf> <http://www.w3.org/2000/01/rdf-schema#range> <http://ex/Department> .
<http://ex/name> <http://www.w3.org/2000/01/rdf-schema#range> <http://ex/Text> .
<http://ex/Department> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Organization> .
<http://ex/ann> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/GradStudent> .
<http://ex/ann> <http://ex/advisor> <http://ex/bob> .
<http://ex/ann> <http://ex/memberOf> <http://ex/cs> .
<http://ex/ann> <http://ex/name> "Ann" .
<http://ex/bob> <http://ex/headOf> <http://ex/cs> .
<http://ex/bob> <http://ex/worksFor> <http://ex/cs> .
<http://ex/bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Person> .
<http://ex/cat> <http://ex/advisor> _:b1 .
_:b1 <http://ex/name> "Carl" .
<http://ex/dan> <http://ex/likes> <http://ex/ann> .
"#;

type Triple = (String, String, String);

/// The closure by naive forward chaining over N-Triples text.
fn closure() -> BTreeSet<Triple> {
    let mut g: BTreeSet<Triple> = DATA.lines().filter(|l| !l.trim().is_empty()).map(|l| {
        let l = l.trim().trim_end_matches('.').trim();
        let (s, rest) = l.split_once(' ').unwrap();
        let (p, o) = rest.split_once(' ').unwrap();
        (s.to_string(), p.to_string(), o.to_string())
    }).collect();
    loop {
        let mut add = Vec::new();
        for (a, p, b) in &g {
            for (c, q, d) in &g {
                // rdfs5, rdfs11: transitivity.
                if (p == SC || p == SP) && q == p && b == c { add.push((a.clone(), p.clone(), d.clone())) }
                // rdfs7: (a p b), p sp d.
                if q == SP && c == p { add.push((a.clone(), d.clone(), b.clone())) }
                // rdfs9: (a type b), b sc d.
                if p == RDF_TYPE && q == SC && c == b { add.push((a.clone(), RDF_TYPE.into(), d.clone())) }
                // rdfs2: (a p b), p dom d.
                if q == DOM && c == p { add.push((a.clone(), RDF_TYPE.into(), d.clone())) }
                // rdfs3: (a p b), p rng d, b not a literal.
                if q == RNG && c == p && !b.starts_with('"') { add.push((b.clone(), RDF_TYPE.into(), d.clone())) }
            }
        }
        let before = g.len();
        g.extend(add);
        if g.len() == before { return g }
    }
}

fn solutions(snap: &MemorySnapshot, q: &str, e: Entailment) -> Vec<Vec<String>> {
    let opts = Options { entailment: e, ..Options::default() };
    match query_with(snap, q, &opts).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
            v.sort();
            v
        }
        QueryResult::Boolean(b) => vec![vec![b.to_string()]],
        r => panic!("{r:?}"),
    }
}

/// Blank node labels differ between the two stores; compare with every label replaced.
fn unlabel(v: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let mut v: Vec<Vec<String>> = v.into_iter().map(|r| r.into_iter().map(|t| if t.starts_with("_:") { "_:".into() } else { t }).collect()).collect();
    v.sort();
    v
}

#[test]
fn rdfs_answers_match_the_materialized_closure() {
    let base = MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap();
    let text: String = closure().into_iter().map(|(s, p, o)| format!("{s} {p} {o} .\n")).collect();
    let closed = MemorySnapshot::from_ntriples(text.as_bytes()).unwrap();
    let queries = [
        "SELECT * WHERE { ?s ?p ?o }",
        "SELECT ?x WHERE { ?x a <http://ex/Person> }",
        "SELECT ?x WHERE { ?x a <http://ex/Agent> }",
        "SELECT ?x WHERE { ?x a <http://ex/Organization> }",
        "SELECT ?x WHERE { ?x a <http://ex/Text> }",
        "SELECT ?x ?c WHERE { ?x a ?c }",
        "SELECT ?c WHERE { <http://ex/ann> a ?c }",
        "SELECT ?c WHERE { _:x a ?c }",
        "SELECT ?x ?y WHERE { ?x <http://ex/memberOf> ?y }",
        "SELECT ?x ?y WHERE { ?x <http://ex/worksFor> ?y }",
        "SELECT ?p ?o WHERE { <http://ex/bob> ?p ?o }",
        "SELECT ?s ?p WHERE { ?s ?p <http://ex/cs> }",
        "SELECT ?a ?b WHERE { ?a <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?b }",
        "SELECT ?b WHERE { <http://ex/GradStudent> <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?b }",
        "SELECT ?a ?b WHERE { ?a <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> ?b }",
        "SELECT ?x ?d WHERE { ?x a <http://ex/Student> . ?x <http://ex/memberOf> ?d . ?d a <http://ex/Organization> }",
        "SELECT ?x ?y WHERE { ?x <http://ex/advisor> ?y . ?y a <http://ex/Faculty> }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?x a <http://ex/Person> }",
        "SELECT ?c (COUNT(?x) AS ?n) WHERE { ?x a ?c } GROUP BY ?c",
        "SELECT ?x WHERE { ?x <http://ex/memberOf>+ ?y }",
        "ASK { <http://ex/bob> a <http://ex/Agent> }",
        "ASK { \"Ann\" a <http://ex/Text> }",
        "SELECT ?x WHERE { ?x a <http://ex/Person> FILTER NOT EXISTS { ?x a <http://ex/Student> } }",
        "SELECT ?x WHERE { ?x <http://ex/likes> ?y OPTIONAL { ?y a ?t } }",
    ];
    for q in queries {
        let rdfs = unlabel(solutions(&base, q, Entailment::Rdfs));
        let oracle = unlabel(solutions(&closed, q, Entailment::Simple));
        assert_eq!(rdfs, oracle, "{q}");
    }
    // The default regime is unchanged.
    let plain = solutions(&base, "SELECT ?x WHERE { ?x a <http://ex/Person> }", Entailment::Simple);
    assert_eq!(plain, vec![vec!["<http://ex/bob>".to_string()]]);
}

/// GRAPH without a dataset stays refused under RDFS as under simple entailment.
#[test]
fn rdfs_refuses_graph_without_a_dataset() {
    let base = MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap();
    let opts = Options { entailment: Entailment::Rdfs, ..Options::default() };
    let e = query_with(&base, "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }", &opts).unwrap_err();
    assert_eq!(e.kind, QueryErrorKind::Unsupported, "{e:?}");
}

/// Each graph of a dataset is entailed on its own triples: a named graph with the schema infers, one without does
/// not (the default graph's schema does not leak in), and a FROM merge is entailed as the merge.
#[test]
fn rdfs_entails_each_graph_of_a_dataset_on_its_own() {
    use triplelight::exec::query_dataset_with;
    use triplelight::model::{Change, DatasetChange, GraphTarget};
    use triplelight::store::Overlay;
    let n = |x: &str| triplelight_rdf::NamedNode::new(x.trim_matches(|c| c == '<' || c == '>')).unwrap();
    let t = |a: &str, p: &str, o: &str| triplelight_rdf::Triple::new(n(a), n(p), n(o));
    let schema = [t("http://ex/GradStudent", SC, "http://ex/Student"), t("http://ex/advisor", DOM, "http://ex/Student")];
    let data = [t("http://ex/ann", RDF_TYPE, "http://ex/GradStudent"), t("http://ex/cat", "http://ex/advisor", "http://ex/bob")];
    let g = |x: &str| n(&format!("http://ex/{x}"));
    let mut changes = Vec::new();
    for t in schema { changes.push(DatasetChange::Quad(GraphTarget::Default, Change::Insert, t.clone())); changes.push(DatasetChange::Quad(GraphTarget::Named(g("g1")), Change::Insert, t)) }
    for t in data {
        changes.push(DatasetChange::Quad(GraphTarget::Named(g("g1")), Change::Insert, t.clone()));
        changes.push(DatasetChange::Quad(GraphTarget::Named(g("g2")), Change::Insert, t));
    }
    let ds = Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_dataset_changes(&changes).unwrap().0;
    let opts = Options { entailment: Entailment::Rdfs, ..Options::default() };
    let students = |q: &str| -> Vec<String> {
        match query_dataset_with(&ds, &ds, q, &opts).unwrap() {
            QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect::<Vec<_>>().join(" ")).collect(); v.sort(); v }
            r => panic!("{r:?}"),
        }
    };
    let both = vec!["<http://ex/ann>".to_string(), "<http://ex/cat>".to_string()];
    assert_eq!(students("SELECT ?x WHERE { GRAPH <http://ex/g1> { ?x a <http://ex/Student> } }"), both);
    assert!(students("SELECT ?x WHERE { GRAPH <http://ex/g2> { ?x a <http://ex/Student> } }").is_empty());
    assert_eq!(students("SELECT ?g ?x WHERE { GRAPH ?g { ?x a <http://ex/Student> } }"), vec!["<http://ex/g1> <http://ex/ann>".to_string(), "<http://ex/g1> <http://ex/cat>".to_string()]);
    // The default graph holds only the schema: no instances.
    assert!(students("SELECT ?x WHERE { ?x a <http://ex/Student> }").is_empty());
    // FROM g2 alone has no schema; the merge of g1 and g2 has g1's.
    assert!(students("SELECT ?x FROM <http://ex/g2> WHERE { ?x a <http://ex/Student> }").is_empty());
    assert_eq!(students("SELECT ?x FROM <http://ex/g1> FROM <http://ex/g2> WHERE { ?x a <http://ex/Student> }"), both);
}

/// W3C `rdfs03`: domain through a superproperty in a graph with no `rdf:type` triple at all.
#[test]
fn rdfs_types_by_domain_without_any_stored_type_triple() {
    let data = "<http://ex/a> <http://ex/b1> <http://ex/c1> .\n<http://ex/b1> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> <http://ex/b2> .\n<http://ex/b2> <http://www.w3.org/2000/01/rdf-schema#domain> <http://ex/c2> .\n";
    let base = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    assert_eq!(solutions(&base, "SELECT ?x WHERE { ?x a <http://ex/c2> }", Entailment::Rdfs), vec![vec!["<http://ex/a>".to_string()]]);
    assert_eq!(solutions(&base, "SELECT ?t WHERE { <http://ex/a> a ?t }", Entailment::Rdfs), vec![vec!["<http://ex/c2>".to_string()]]);
    // The unbound-predicate view names the type triple with the right IRI.
    assert!(solutions(&base, "SELECT ?p ?o WHERE { <http://ex/a> ?p ?o }", Entailment::Rdfs).contains(&vec!["<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string(), "<http://ex/c2>".to_string()]));
    assert!(solutions(&base, "SELECT ?x WHERE { ?x a <http://ex/c2> }", Entailment::Simple).is_empty());
}
