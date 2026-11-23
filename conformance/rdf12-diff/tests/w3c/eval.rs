use super::terms::{isomorphic, parse_native, quad};
use super::w3c::{MF, fixture, iri, node, property};
use oxrdf::{BlankNode, Dataset, Graph, GraphName, NamedNode, NamedNodeRef, Quad, Term, Triple};
use sparesults::{QueryResultsFormat, QueryResultsParser, SliceQueryResultsParserOutput};
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use triplelight::exec::{Options, QueryResult, query_dataset_with, results::serialize};
use triplelight::model::Perm;
use triplelight::storage::{DatasetView, Dictionary, MemorySnapshot, Snapshot};
use triplelight::store::{Overlay, dataset_update, quad_load_changes};
use triplelight_rdf as tl;

const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const UT: &str = "http://www.w3.org/2009/sparql/tests/test-update#";
type Fallible<T> = Result<T, Box<dyn Error>>;

fn load_fixture(root: &Path, path: &str) -> Fallible<Vec<tl::Quad>> {
    let format = match path.rsplit('.').next() {
        Some("nt") => "NTriples",
        Some("nq") => "NQuads",
        Some("ttl") => "Turtle",
        Some("trig") => "Trig",
        _ => return Err(format!("unrecognized RDF fixture format {path}").into()),
    };
    Ok(parse_native(&fixture(root, path), format, path)?)
}

fn reference_graph(root: &Path, path: &str) -> Fallible<Dataset> {
    let data = fixture(root, path);
    match path.rsplit('.').next() {
        Some("nt") => Ok(oxttl::NTriplesParser::new()
            .for_slice(&data)
            .map(|result| result.map(|triple| triple.in_graph(GraphName::DefaultGraph)))
            .collect::<Result<_, _>>()?),
        Some("nq") => Ok(oxttl::NQuadsParser::new()
            .for_slice(&data)
            .collect::<Result<_, _>>()?),
        Some("ttl") => Ok(oxttl::TurtleParser::new()
            .with_base_iri(path)?
            .for_slice(&data)
            .map(|result| result.map(|triple| triple.in_graph(GraphName::DefaultGraph)))
            .collect::<Result<_, _>>()?),
        Some("trig") => Ok(oxttl::TriGParser::new()
            .with_base_iri(path)?
            .for_slice(&data)
            .collect::<Result<_, _>>()?),
        _ => Err(format!("unrecognized RDF reference format {path}").into()),
    }
}

fn input_dataset(
    root: &Path,
    graph: &Graph,
    action: oxrdf::NamedOrBlankNodeRef<'_>,
    prefix: &str,
) -> Fallible<Overlay> {
    let mut dataset = Overlay::from_base(Arc::new(MemorySnapshot::from_triples([])));
    let data = format!("{prefix}data");
    let named = format!("{prefix}graphData");
    if graph
        .objects_for_subject_predicate(action, NamedNodeRef::new_unchecked(&named))
        .next()
        .is_some()
    {
        return Err("unexpected manifest graphData in the pinned suite".into());
    }
    for path in graph.objects_for_subject_predicate(action, NamedNodeRef::new_unchecked(&data)) {
        let changes = quad_load_changes(load_fixture(root, iri(path))?)?;
        (dataset, _) = dataset.with_dataset_changes(&changes)?;
    }
    Ok(dataset)
}

fn read_output(dataset: &Overlay) -> Fallible<Dataset> {
    let mut rows = Vec::new();
    let mut read = |snapshot: &dyn Snapshot, graph: tl::GraphName| -> Fallible<()> {
        for batch in snapshot.scan(Perm::Spo, &[]) {
            let [s, p, o] = batch?.cols;
            for ((s, p), o) in s.into_iter().zip(p).zip(o) {
                rows.push(quad(tl::Quad::new(
                    tl::NamedOrBlankNode::try_from(snapshot.resolve(s)?)?,
                    tl::NamedNode::try_from(snapshot.resolve(p)?)?,
                    snapshot.resolve(o)?,
                    graph.clone(),
                )));
            }
        }
        Ok(())
    };
    read(dataset.default_graph(), tl::GraphName::DefaultGraph)?;
    for id in dataset.graph_names()? {
        let name = tl::NamedNode::try_from(dataset.resolve(id)?)?;
        let snapshot = dataset
            .graph(id)?
            .ok_or("missing named graph from catalog")?;
        read(snapshot, tl::GraphName::NamedNode(name))?;
    }
    Ok(rows.into_iter().collect())
}

enum Answer {
    Boolean(bool),
    Solutions {
        variables: BTreeSet<String>,
        rows: Dataset,
    },
}

fn normalize_blank(label: BlankNode, assigned: &mut HashMap<BlankNode, BlankNode>) -> BlankNode {
    let id = assigned.len();
    assigned
        .entry(label)
        .or_insert_with(|| BlankNode::new_unchecked(format!("b{id}")))
        .clone()
}

fn scope_term(term: Term, assigned: &mut HashMap<BlankNode, BlankNode>) -> Term {
    match term {
        Term::BlankNode(label) => normalize_blank(label, assigned).into(),
        Term::Triple(triple) => {
            let triple = *triple;
            let subject = match triple.subject {
                oxrdf::NamedOrBlankNode::BlankNode(label) => {
                    normalize_blank(label, assigned).into()
                }
                named => named,
            };
            Triple::new(
                subject,
                triple.predicate,
                scope_term(triple.object, assigned),
            )
            .into()
        }
        scalar => scalar,
    }
}

fn result_answer(data: &[u8], format: QueryResultsFormat, ordered: bool) -> Fallible<Answer> {
    match QueryResultsParser::from_format(format).for_slice(data)? {
        SliceQueryResultsParserOutput::Boolean(value) => Ok(Answer::Boolean(value)),
        SliceQueryResultsParserOutput::Solutions(solutions) => {
            let variables = solutions
                .variables()
                .iter()
                .map(|var| var.as_str().to_owned())
                .collect();
            let mut rows = Vec::new();
            let mut assigned = HashMap::new();
            for (i, solution) in solutions.enumerate() {
                let solution = solution?;
                // One blank node per row distinguishes duplicate solutions and empty solutions.
                let row = BlankNode::new_unchecked(format!("r{i}"));
                rows.push(Quad::new(
                    NamedNode::new_unchecked("urn:tl:w3c:answer"),
                    NamedNode::new_unchecked("urn:tl:w3c:row"),
                    row.clone(),
                    GraphName::DefaultGraph,
                ));
                if ordered {
                    rows.push(Quad::new(
                        row.clone(),
                        NamedNode::new_unchecked("urn:tl:w3c:position"),
                        oxrdf::Literal::from(i as u64),
                        GraphName::DefaultGraph,
                    ));
                }
                for (variable, value) in &solution {
                    rows.push(Quad::new(
                        row.clone(),
                        NamedNode::new_unchecked(format!("urn:tl:w3c:var:{}", variable.as_str())),
                        scope_term(value.clone(), &mut assigned),
                        GraphName::DefaultGraph,
                    ));
                }
            }
            Ok(Answer::Solutions {
                variables,
                rows: rows.into_iter().collect(),
            })
        }
    }
}

fn compare_results(expected: Answer, actual: Answer) -> bool {
    match (expected, actual) {
        (Answer::Boolean(left), Answer::Boolean(right)) => left == right,
        (
            Answer::Solutions {
                variables: left_vars,
                rows: left,
            },
            Answer::Solutions {
                variables: right_vars,
                rows: right,
            },
        ) => left_vars == right_vars && isomorphic(left, right),
        _ => false,
    }
}

pub fn evaluate(
    root: &Path,
    graph: &Graph,
    entry: oxrdf::NamedOrBlankNodeRef<'_>,
    update: bool,
) -> Fallible<()> {
    let (prefix, operation) = if update {
        (UT, "request")
    } else {
        (QT, "query")
    };
    let action = node(property(graph, entry, MF, "action"));
    let request = iri(property(graph, action, prefix, operation));
    let initial = input_dataset(root, graph, action, prefix)?;
    let text = format!(
        "BASE <{request}>\n{}",
        String::from_utf8(fixture(root, request))?
    );
    let reference = property(graph, entry, MF, "result");

    let matches = if update {
        let result = node(reference);
        let path = iri(property(graph, result, UT, "data"));
        let (updated, _, _) = dataset_update(&initial, &text)?;
        isomorphic(
            reference_graph(root, path)?,
            read_output(updated.as_ref().unwrap_or(&initial))?,
        )
    } else {
        let path = iri(reference);
        let result = query_dataset_with(&initial, &initial, &text, &Options::default())?;
        match path.rsplit('.').next() {
            Some("srj" | "srx") => {
                if matches!(result, QueryResult::Graph(_) | QueryResult::Ids(_)) {
                    return Err("expected a tuple or boolean result".into());
                }
                let reference_format = if path.ends_with(".srj") {
                    QueryResultsFormat::Json
                } else {
                    QueryResultsFormat::Xml
                };
                let (_, actual_json) = serialize(&result);
                // These four pinned queries sort on ?x, a unique, projected IRI.
                // The triple-term sort test checks the selected offsets in the bindings.
                let ordered = request
                    .split_once("/lang-basedir/")
                    .is_some_and(|(_, file)| {
                        matches!(
                            file,
                            "haslang.rq" | "haslangdir.rq" | "lang.rq" | "langdir.rq"
                        )
                    });
                compare_results(
                    result_answer(&fixture(root, path), reference_format, ordered)?,
                    result_answer(actual_json.as_bytes(), QueryResultsFormat::Json, ordered)?,
                )
            }
            _ => {
                let QueryResult::Graph(graph) = result else {
                    return Err("expected a graph result".into());
                };
                let actual = graph
                    .into_iter()
                    .map(|triple| quad(triple.in_graph(tl::GraphName::DefaultGraph)))
                    .collect();
                isomorphic(reference_graph(root, path)?, actual)
            }
        }
    };
    if matches {
        Ok(())
    } else {
        Err("answer differs from the pinned reference".into())
    }
}

#[test]
fn solution_comparison_checks_duplicates_and_shared_nested_blank_nodes() {
    let row = r#"{"x":{"type":"bnode","value":"a"},"t":{"type":"triple","value":{
        "subject":{"type":"bnode","value":"a"},
        "predicate":{"type":"uri","value":"urn:p"},
        "object":{"type":"literal","value":"text","xml:lang":"en","its:dir":"ltr"}}}}"#;
    let with_order = |rows: &str, ordered| {
        let json = format!(r#"{{"head":{{"vars":["x","t"]}},"results":{{"bindings":[{rows}]}}}}"#);
        result_answer(json.as_bytes(), QueryResultsFormat::Json, ordered).unwrap()
    };
    let answer = |rows: &str| with_order(rows, false);
    assert!(compare_results(
        answer(row),
        answer(&row.replace("\"a\"", "\"b\""))
    ));
    assert!(!compare_results(
        answer(row),
        answer(&row.replace("ltr", "rtl"))
    ));
    assert!(!compare_results(
        answer(row),
        answer(&format!("{row},{row}"))
    ));
    let nested = "\"subject\":{\"type\":\"bnode\",\"value\":\"a\"}";
    assert!(!compare_results(
        answer(row),
        answer(&row.replace(nested, &nested.replace("\"a\"", "\"b\"")))
    ));
    let other = row.replace("text", "other");
    let left = format!("{row},{other}");
    let right = format!("{other},{row}");
    assert!(compare_results(answer(&left), answer(&right)));
    assert!(!compare_results(
        with_order(&left, true),
        with_order(&right, true)
    ));
}
