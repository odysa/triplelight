//! Validation results and the validation report (SHACL 3.6).
use super::path::Path;
use super::vocab::{rdf, sh};
use crate::rdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};

/// A result's severity: the three SHACL ones, or any other IRI a shape names with `sh:severity` (SHACL allows user-defined severities).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Severity { Info, Warning, Violation, Custom(NamedNode) }

impl Severity {
    pub fn iri(&self) -> NamedNode {
        match self {
            Severity::Info => sh("Info"),
            Severity::Warning => sh("Warning"),
            Severity::Violation => sh("Violation"),
            Severity::Custom(iri) => iri.clone(),
        }
    }
    pub fn from_iri(iri: &NamedNode) -> Severity {
        [Severity::Info, Severity::Warning, Severity::Violation].into_iter().find(|s| &s.iri() == iri).unwrap_or_else(|| Severity::Custom(iri.clone()))
    }
}

/// What evaluating one constraint yields: a violation of the constraint itself, or results of a nested shape that are reported
/// as they are (`sh:property` creates no result of its own).
#[derive(Clone, Debug)]
pub enum Outcome {
    Violation(ConstraintViolation),
    Forwarded(Vec<ValidationResult>),
}

/// What a constraint component reports for one failed check: the evaluator adds the focus node, the path, the source
/// shape and the severity.
#[derive(Clone, Debug)]
pub struct ConstraintViolation {
    pub value: Option<Term>,
    /// A path to report in place of the shape's own (`sh:closed` reports the offending predicate).
    pub path_override: Option<Path>,
    pub component: NamedNode,
    pub message: Option<Literal>,
    pub details: Vec<ValidationResult>,
}

#[derive(Clone, Debug)]
pub struct ValidationResult {
    pub focus_node: Term,
    pub result_path: Option<Path>,
    pub value: Option<Term>,
    pub source_shape: Term,
    pub source_constraint_component: NamedNode,
    pub severity: Severity,
    pub messages: Vec<Literal>,
    pub details: Vec<ValidationResult>,
}

#[derive(Clone, Debug)]
pub struct ValidationReport {
    /// True when there are no validation results at all (a result of any severity makes the data graph non-conforming).
    pub conforms: bool,
    pub results: Vec<ValidationResult>,
}

impl ValidationReport {
    pub fn new(results: Vec<ValidationResult>) -> ValidationReport { ValidationReport { conforms: results.is_empty(), results } }

    /// The report as RDF: a `sh:ValidationReport` node with `sh:conforms` and one `sh:result` per result. Blank nodes are fresh.
    pub fn to_triples(&self) -> Vec<Triple> {
        let mut out = Vec::new();
        let report = fresh();
        out.push(Triple::new(report.clone(), rdf("type"), Term::from(sh("ValidationReport"))));
        out.push(Triple::new(report.clone(), sh("conforms"), Term::from(Literal::from(self.conforms))));
        for r in &self.results {
            let node = result_to_triples(r, &mut out);
            out.push(Triple::new(report.clone(), sh("result"), Term::from(node)));
        }
        out
    }
}

fn fresh() -> NamedOrBlankNode { NamedOrBlankNode::from(BlankNode::default()) }

fn result_to_triples(r: &ValidationResult, out: &mut Vec<Triple>) -> NamedOrBlankNode {
    let node = fresh();
    out.push(Triple::new(node.clone(), rdf("type"), Term::from(sh("ValidationResult"))));
    out.push(Triple::new(node.clone(), sh("focusNode"), r.focus_node.clone()));
    if let Some(p) = &r.result_path {
        let root = p.to_rdf(out);
        out.push(Triple::new(node.clone(), sh("resultPath"), root));
    }
    if let Some(v) = &r.value { out.push(Triple::new(node.clone(), sh("value"), v.clone())) }
    out.push(Triple::new(node.clone(), sh("sourceShape"), r.source_shape.clone()));
    out.push(Triple::new(node.clone(), sh("sourceConstraintComponent"), Term::from(r.source_constraint_component.clone())));
    out.push(Triple::new(node.clone(), sh("resultSeverity"), Term::from(r.severity.iri())));
    for m in &r.messages { out.push(Triple::new(node.clone(), sh("resultMessage"), Term::from(m.clone()))) }
    for d in &r.details {
        let child = result_to_triples(d, out);
        out.push(Triple::new(node.clone(), sh("detail"), Term::from(child)));
    }
    node
}
