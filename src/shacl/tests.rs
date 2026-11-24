//! Tests of the graph adapter, paths, targets and the report. The constraint components have their own tests in `constraints`.
use super::*;
use crate::rdf::{Format, Term, Triple};

fn parse(ttl: &str) -> Vec<Triple> {
    let doc = format!(
        "@prefix ex: <http://ex/> . @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
         @prefix sh: <http://www.w3.org/ns/shacl#> . {ttl}");
    crate::rdf::parse_triples(doc.as_bytes(), Format::Turtle).expect("the test document parses")
}
fn iri(s: &str) -> Term { Term::from(crate::rdf::NamedNode::new_unchecked(format!("http://ex/{s}"))) }
fn pred(s: &str) -> crate::rdf::NamedNode { crate::rdf::NamedNode::new_unchecked(format!("http://ex/{s}")) }

#[test]
fn objects_subjects_and_outgoing_index_the_same_triples() {
    let g = Graph::new(&parse("ex:a ex:p ex:b, ex:c . ex:d ex:p ex:b . ex:a ex:q ex:b ."));
    assert_eq!(g.objects(&iri("a"), &pred("p")), [iri("b"), iri("c")]);
    assert_eq!(g.subjects(&pred("p"), &iri("b")), [iri("a"), iri("d")]);
    assert_eq!(g.outgoing(&iri("a")).len(), 3);
    assert_eq!(g.subjects_of(&pred("p")), [iri("a"), iri("d")]);
    assert!(g.objects(&iri("zzz"), &pred("p")).is_empty());
}

#[test]
fn duplicate_triples_count_once() {
    let g = Graph::new(&parse("ex:a ex:p ex:b . ex:a ex:p ex:b ."));
    assert_eq!(g.objects(&iri("a"), &pred("p")).len(), 1);
}

#[test]
fn class_membership_follows_subclass_and_survives_cycles() {
    let g = Graph::new(&parse("ex:Dog rdfs:subClassOf ex:Animal . ex:Animal rdfs:subClassOf ex:Dog . ex:rex a ex:Dog . ex:cat a ex:Animal ."));
    assert!(g.is_instance(&iri("rex"), &iri("Animal")));
    assert!(g.is_instance(&iri("cat"), &iri("Dog")), "a subclass cycle makes the classes equivalent");
    assert!(!g.is_instance(&iri("rex"), &iri("Plant")));
    let mut inst = g.instances_of(&iri("Animal"));
    inst.sort_by_key(|t| t.to_string());
    assert_eq!(inst, [iri("cat"), iri("rex")]);
}

#[test]
fn lists_are_read_and_malformed_ones_are_errors() {
    let g = Graph::new(&parse("ex:s ex:l (ex:a ex:b ex:c) . ex:t ex:l _:x . _:x rdf:first ex:a . "));
    let head = g.objects(&iri("s"), &pred("l"))[0].clone();
    assert_eq!(g.list_items(&head).unwrap(), [iri("a"), iri("b"), iri("c")]);
    let broken = g.objects(&iri("t"), &pred("l"))[0].clone();
    assert!(matches!(g.list_items(&broken), Err(ValidationError::MalformedList(_))));
}

#[test]
fn path_forms_evaluate_as_sets() {
    let data = Graph::new(&parse("ex:a ex:p ex:b . ex:b ex:p ex:c . ex:c ex:p ex:a . ex:a ex:q ex:z . ex:y ex:p ex:a ."));
    let p = Path::Predicate(pred("p"));
    assert_eq!(p.values(&iri("a"), &data), [iri("b")]);
    assert_eq!(Path::Inverse(Box::new(p.clone())).values(&iri("a"), &data), [iri("c"), iri("y")]);
    assert_eq!(Path::Sequence(vec![p.clone(), p.clone()]).values(&iri("a"), &data), [iri("c")]);
    assert_eq!(Path::Alternative(vec![p.clone(), Path::Predicate(pred("q"))]).values(&iri("a"), &data), [iri("b"), iri("z")]);
    assert_eq!(Path::ZeroOrOne(Box::new(p.clone())).values(&iri("a"), &data), [iri("a"), iri("b")]);
    // a cycle a -> b -> c -> a: one or more steps reach all three (a again through the cycle), zero or more adds nothing new
    let mut one_plus = Path::OneOrMore(Box::new(p.clone())).values(&iri("a"), &data);
    one_plus.sort_by_key(|t| t.to_string());
    assert_eq!(one_plus, [iri("a"), iri("b"), iri("c")]);
    let zero_plus = Path::ZeroOrMore(Box::new(p)).values(&iri("a"), &data);
    assert_eq!(zero_plus.len(), 3);
    assert_eq!(zero_plus[0], iri("a"), "the focus node comes first");
}

#[test]
fn paths_parse_from_the_shapes_graph() {
    let ttl = "ex:shape ex:pathI [ sh:inversePath ex:p ] ; ex:pathS ( ex:p ex:q ) ; ex:pathA [ sh:alternativePath ( ex:p ex:q ) ] ; \
               ex:pathZ [ sh:zeroOrMorePath ex:p ] ; ex:pathBad [ sh:inversePath ex:p ; sh:zeroOrOnePath ex:p ] .";
    let g = Graph::new(&parse(ttl));
    let get = |name: &str| Path::parse(&g.objects(&iri("shape"), &pred(name))[0], &g);
    assert_eq!(get("pathI").unwrap(), Path::Inverse(Box::new(Path::Predicate(pred("p")))));
    assert_eq!(get("pathS").unwrap(), Path::Sequence(vec![Path::Predicate(pred("p")), Path::Predicate(pred("q"))]));
    assert_eq!(get("pathA").unwrap(), Path::Alternative(vec![Path::Predicate(pred("p")), Path::Predicate(pred("q"))]));
    assert_eq!(get("pathZ").unwrap(), Path::ZeroOrMore(Box::new(Path::Predicate(pred("p")))));
    assert!(matches!(get("pathBad"), Err(ValidationError::MalformedShape(_))), "two path forms on one node is malformed");
}

#[test]
fn targets_select_focus_nodes() {
    let shapes = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:targetClass ex:C ; sh:targetSubjectsOf ex:p ; sh:targetObjectsOf ex:q .");
    let data = Graph::new(&parse("ex:x a ex:C . ex:y ex:p ex:o1 . ex:o2 ex:q ex:z . ex:Sub rdfs:subClassOf ex:C . ex:w a ex:Sub ."));
    let sg = ShapesGraph::new(Graph::new(&shapes)).unwrap();
    let shape = sg.shape(&iri("S")).unwrap();
    let mut foci = sg.focus_nodes(&shape, &data);
    foci.sort_by_key(|t| t.to_string());
    let mut want = vec![iri("n"), iri("x"), iri("w"), iri("y"), iri("z")];
    want.sort_by_key(|t| t.to_string());
    assert_eq!(foci, want);
    let work: Vec<_> = sg.work_list(&data).collect::<Result<_, _>>().unwrap();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].0, iri("S"));
    assert_eq!(work[0].1.len(), 5);
}

#[test]
fn a_shape_that_is_also_a_class_targets_its_own_instances() {
    let shapes = parse("ex:Person a rdfs:Class, sh:NodeShape .");
    let sg = ShapesGraph::new(Graph::new(&shapes)).unwrap();
    let shape = sg.shape(&iri("Person")).unwrap();
    let data = Graph::new(&parse("ex:ann a ex:Person ."));
    assert_eq!(sg.focus_nodes(&shape, &data), [iri("ann")]);
}

#[test]
fn a_shape_without_constraints_conforms_and_a_deactivated_shape_is_skipped() {
    let shapes = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n . ex:T a sh:NodeShape ; sh:deactivated true ; sh:targetNode ex:n .");
    let report = validate(&parse("ex:n ex:p ex:o ."), &shapes).unwrap();
    assert!(report.conforms && report.results.is_empty());
    let triples = report.to_triples();
    assert!(triples.iter().any(|t| t.predicate.as_str().ends_with("#conforms")));
}

#[test]
fn the_report_is_written_as_rdf() {
    let result = ValidationResult {
        focus_node: iri("n"), result_path: Some(Path::Inverse(Box::new(Path::Predicate(pred("p"))))), value: Some(iri("v")),
        source_shape: iri("S"), source_constraint_component: crate::rdf::NamedNode::new_unchecked("http://www.w3.org/ns/shacl#MinCountConstraintComponent"),
        severity: Severity::Warning, messages: vec![crate::rdf::Literal::new_simple_literal("too few")], details: Vec::new(),
    };
    let report = ValidationReport::new(vec![result]);
    assert!(!report.conforms);
    let t = report.to_triples();
    let has = |p: &str| t.iter().any(|x| x.predicate.as_str() == format!("http://www.w3.org/ns/shacl#{p}"));
    for p in ["conforms", "result", "focusNode", "resultPath", "value", "sourceShape", "sourceConstraintComponent", "resultSeverity", "resultMessage", "inversePath"] {
        assert!(has(p), "missing sh:{p}");
    }
}

#[test]
fn a_user_defined_severity_is_kept() {
    let shape = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:severity ex:Bogus .");
    let sg = ShapesGraph::new(Graph::new(&shape)).unwrap();
    assert_eq!(sg.shape(&iri("S")).unwrap().severity, Severity::Custom(pred("Bogus")));
    assert!(matches!(Severity::from_iri(&pred("Bogus")), Severity::Custom(_)));
    assert_eq!(Severity::from_iri(&crate::rdf::NamedNode::new_unchecked("http://www.w3.org/ns/shacl#Warning")), Severity::Warning);
}

#[test]
fn a_path_that_contains_itself_is_malformed_not_a_crash() {
    let shapes = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path _:p ] . _:p sh:inversePath _:p .");
    assert!(matches!(validate(&[], &shapes), Err(ValidationError::MalformedShape(_))));
    let alt = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path _:p ] . _:p sh:alternativePath ( _:p ex:q ) .");
    assert!(matches!(validate(&[], &alt), Err(ValidationError::MalformedShape(_))));
}

#[test]
fn a_very_deeply_nested_path_is_refused() {
    let mut ttl = String::from("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path _:p0 ] .\n");
    for i in 0..300 { ttl.push_str(&format!("_:p{i} sh:inversePath _:p{} .\n", i + 1)); }
    ttl.push_str("_:p300 sh:inversePath ex:q .\n");
    assert!(matches!(validate(&[], &parse(&ttl)), Err(ValidationError::MalformedShape(_))));
}

#[test]
fn a_shape_that_refers_to_itself_is_refused_before_any_data_is_read() {
    let shapes = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n0 ; sh:property [ sh:path ex:next ; sh:node ex:S ] .");
    let mut chain = String::new();
    for i in 0..100_000 { chain.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1)); }
    assert!(matches!(validate(&parse(&chain), &shapes), Err(ValidationError::UndefinedRecursion(_))));
}

#[test]
fn a_ladder_of_double_references_to_the_same_shape_is_refused_not_evaluated_exponentially() {
    let shapes = parse("ex:S a sh:NodeShape ; sh:targetNode ex:l0 ; sh:property [ sh:path ex:a ; sh:node ex:S ] , [ sh:path ex:b ; sh:node ex:S ] .");
    let mut ladder = String::new();
    for i in 0..40 { ladder.push_str(&format!("ex:l{i} ex:a ex:l{} ; ex:b ex:l{} .\n", i + 1, i + 1)); }
    assert!(matches!(validate(&parse(&ladder), &shapes), Err(ValidationError::UndefinedRecursion(_))));
}

#[test]
fn recursion_through_logical_and_qualified_references_is_refused_too() {
    let via_not = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:not ex:T . ex:T a sh:NodeShape ; sh:and ( ex:S ) .");
    assert!(matches!(validate(&[], &via_not), Err(ValidationError::UndefinedRecursion(_))));
    let q = parse("ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:qualifiedValueShape ex:S ; sh:qualifiedMinCount 1 ] .");
    assert!(matches!(validate(&[], &q), Err(ValidationError::UndefinedRecursion(_))));
}

#[test]
fn nested_shapes_that_do_not_recurse_still_validate() {
    let shapes = parse(
        "ex:T a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:node ex:U ] . \
         ex:U a sh:NodeShape ; sh:datatype <http://www.w3.org/2001/XMLSchema#string> .");
    let report = validate(&parse("ex:n ex:p \"a\" , 1 ."), &shapes).unwrap();
    assert!(!report.conforms);
    assert_eq!(report.results.len(), 1, "the integer value fails U, the string passes");
}

#[test]
fn a_deep_acyclic_chain_of_shapes_is_refused_with_unsupported_before_any_data_is_read() {
    let mut ttl = String::from("ex:s0 a sh:NodeShape ; sh:targetNode ex:n ; sh:node ex:s1 .\n");
    for i in 1..300 { ttl.push_str(&format!("ex:s{i} a sh:NodeShape ; sh:node ex:s{} .\n", i + 1)); }
    assert!(matches!(validate(&[], &parse(&ttl)), Err(ValidationError::Unsupported(_))));
    // a chain within the limit validates
    let mut ok = String::from("ex:s0 a sh:NodeShape ; sh:targetNode ex:n ; sh:node ex:s1 .\n");
    for i in 1..100 { ok.push_str(&format!("ex:s{i} a sh:NodeShape ; sh:node ex:s{} .\n", i + 1)); }
    assert!(validate(&[], &parse(&ok)).unwrap().conforms);
}
