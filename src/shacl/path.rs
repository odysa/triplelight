//! SHACL property paths: parsing from the shapes graph, evaluation over the data graph, and writing back as RDF.
use super::error::ValidationError;
use super::graph::Graph;
use super::vocab::{rdf, sh};
use crate::rdf::{BlankNode, NamedNode, Term, Triple};
use std::collections::HashSet;

/// The deepest nesting of path nodes `Path::parse` accepts.
const MAX_PATH_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Path {
    Predicate(NamedNode),
    Inverse(Box<Path>),
    /// Applied left to right.
    Sequence(Vec<Path>),
    Alternative(Vec<Path>),
    ZeroOrMore(Box<Path>),
    OneOrMore(Box<Path>),
    ZeroOrOne(Box<Path>),
}

impl Path {
    /// The path rooted at `node` in the shapes graph (SHACL 2.3.1): an IRI, or a blank node that is a list (a sequence)
    /// or has exactly one of `sh:inversePath`, `sh:alternativePath`, `sh:zeroOrMorePath`, `sh:oneOrMorePath`,
    /// `sh:zeroOrOnePath`.
    pub fn parse(node: &Term, shapes: &Graph) -> Result<Path, ValidationError> {
        Path::parse_at(node, shapes, &mut Vec::new())
    }

    /// `visiting` holds the blank path nodes being parsed, outermost first: a node that contains itself is a malformed path (and a
    /// path nested deeper than `MAX_PATH_DEPTH` is refused), so a hostile shapes graph cannot overflow the stack.
    fn parse_at(node: &Term, shapes: &Graph, visiting: &mut Vec<Term>) -> Result<Path, ValidationError> {
        let bad = |why: &str| ValidationError::MalformedShape(format!("path {node}: {why}"));
        match node {
            Term::NamedNode(p) => Ok(Path::Predicate(p.clone())),
            Term::BlankNode(_) => {
                if visiting.contains(node) { return Err(bad("a path contains itself")) }
                if visiting.len() >= MAX_PATH_DEPTH { return Err(bad("paths nested more than 64 deep are not supported")) }
                visiting.push(node.clone());
                let parsed = Path::parse_blank(node, shapes, visiting);
                visiting.pop();
                parsed
            }
            _ => Err(bad("a path is an IRI or a blank node")),
        }
    }

    fn parse_blank(node: &Term, shapes: &Graph, visiting: &mut Vec<Term>) -> Result<Path, ValidationError> {
        let bad = |why: &str| ValidationError::MalformedShape(format!("path {node}: {why}"));
        if !shapes.objects(node, &rdf("first")).is_empty() {
            let items = shapes.list_items(node)?;
            if items.len() < 2 { return Err(bad("a sequence path needs at least two members")) }
            return Ok(Path::Sequence(items.iter().map(|i| Path::parse_at(i, shapes, visiting)).collect::<Result<_, _>>()?));
        }
        let forms = ["inversePath", "alternativePath", "zeroOrMorePath", "oneOrMorePath", "zeroOrOnePath"];
        let present: Vec<(&str, &Term)> = forms.iter().flat_map(|f| shapes.objects(node, &sh(f)).iter().map(move |o| (*f, o))).collect();
        match present.as_slice() {
            [("inversePath", o)] => Ok(Path::Inverse(Box::new(Path::parse_at(o, shapes, visiting)?))),
            [("alternativePath", o)] => {
                let items = shapes.list_items(o)?;
                if items.len() < 2 { return Err(bad("an alternative path needs at least two members")) }
                Ok(Path::Alternative(items.iter().map(|i| Path::parse_at(i, shapes, visiting)).collect::<Result<_, _>>()?))
            }
            [("zeroOrMorePath", o)] => Ok(Path::ZeroOrMore(Box::new(Path::parse_at(o, shapes, visiting)?))),
            [("oneOrMorePath", o)] => Ok(Path::OneOrMore(Box::new(Path::parse_at(o, shapes, visiting)?))),
            [("zeroOrOnePath", o)] => Ok(Path::ZeroOrOne(Box::new(Path::parse_at(o, shapes, visiting)?))),
            _ => Err(bad("not a recognised path form")),
        }
    }

    /// The value nodes reached from `focus`: a set, in the order first reached.
    pub fn values(&self, focus: &Term, data: &Graph) -> Vec<Term> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for v in self.step(focus, data) { if seen.insert(v.clone()) { out.push(v) } }
        out
    }

    fn step(&self, from: &Term, data: &Graph) -> Vec<Term> {
        match self {
            Path::Predicate(p) => data.objects(from, p).to_vec(),
            Path::Inverse(inner) => inner.inverse_step(from, data),
            Path::Sequence(parts) => {
                let mut frontier = vec![from.clone()];
                for part in parts {
                    let mut next = Vec::new();
                    let mut seen = HashSet::new();
                    for n in &frontier { for v in part.step(n, data) { if seen.insert(v.clone()) { next.push(v) } } }
                    frontier = next;
                }
                frontier
            }
            Path::Alternative(parts) => parts.iter().flat_map(|p| p.step(from, data)).collect(),
            Path::ZeroOrOne(inner) => { let mut v = vec![from.clone()]; v.extend(inner.step(from, data)); v }
            Path::ZeroOrMore(inner) => inner.closure(from, data, true),
            Path::OneOrMore(inner) => inner.closure(from, data, false),
        }
    }

    /// The nodes from which `self` reaches `to` (the path walked backwards).
    fn inverse_step(&self, to: &Term, data: &Graph) -> Vec<Term> {
        match self {
            Path::Predicate(p) => data.subjects(p, to),
            Path::Inverse(inner) => inner.step(to, data),
            Path::Sequence(parts) => {
                let mut frontier = vec![to.clone()];
                for part in parts.iter().rev() {
                    let mut next = Vec::new();
                    let mut seen = HashSet::new();
                    for n in &frontier { for v in part.inverse_step(n, data) { if seen.insert(v.clone()) { next.push(v) } } }
                    frontier = next;
                }
                frontier
            }
            Path::Alternative(parts) => parts.iter().flat_map(|p| p.inverse_step(to, data)).collect(),
            Path::ZeroOrOne(inner) => { let mut v = vec![to.clone()]; v.extend(inner.inverse_step(to, data)); v }
            Path::ZeroOrMore(inner) => Path::closure_by(to, data, true, &|n, d| inner.inverse_step(n, d)),
            Path::OneOrMore(inner) => Path::closure_by(to, data, false, &|n, d| inner.inverse_step(n, d)),
        }
    }

    /// Reachability through `self` (zero or more steps when `reflexive`, one or more otherwise), cycle-safe.
    fn closure(&self, from: &Term, data: &Graph, reflexive: bool) -> Vec<Term> {
        Path::closure_by(from, data, reflexive, &|n, d| self.step(n, d))
    }

    fn closure_by(from: &Term, data: &Graph, reflexive: bool, step: &dyn Fn(&Term, &Graph) -> Vec<Term>) -> Vec<Term> {
        let mut out = Vec::new();
        let mut seen: HashSet<Term> = HashSet::new();
        if reflexive { seen.insert(from.clone()); out.push(from.clone()) }
        let mut stack = vec![from.clone()];
        while let Some(n) = stack.pop() {
            for v in step(&n, data) {
                if seen.insert(v.clone()) { out.push(v.clone()); stack.push(v) }
            }
        }
        out
    }

    /// Writes the path as RDF (a validation result's `sh:resultPath`) and returns its root: the IRI itself for a predicate
    /// path, a fresh blank node (with its triples appended to `out`) for the others.
    pub fn to_rdf(&self, out: &mut Vec<Triple>) -> Term {
        let wrap = |kind: &str, inner: Term, out: &mut Vec<Triple>| -> Term {
            let b = Term::from(BlankNode::default());
            out.push(Triple::new(as_subject(&b), sh(kind), inner));
            b
        };
        match self {
            Path::Predicate(p) => Term::from(p.clone()),
            Path::Inverse(i) => { let t = i.to_rdf(out); wrap("inversePath", t, out) }
            Path::ZeroOrMore(i) => { let t = i.to_rdf(out); wrap("zeroOrMorePath", t, out) }
            Path::OneOrMore(i) => { let t = i.to_rdf(out); wrap("oneOrMorePath", t, out) }
            Path::ZeroOrOne(i) => { let t = i.to_rdf(out); wrap("zeroOrOnePath", t, out) }
            Path::Sequence(parts) => list_to_rdf(parts, out),
            Path::Alternative(parts) => { let l = list_to_rdf(parts, out); wrap("alternativePath", l, out) }
        }
    }
}

fn as_subject(t: &Term) -> crate::rdf::NamedOrBlankNode {
    crate::rdf::NamedOrBlankNode::try_from(t.clone()).expect("a path node is an IRI or a blank node")
}

fn list_to_rdf(parts: &[Path], out: &mut Vec<Triple>) -> Term {
    let mut tail = Term::from(rdf("nil"));
    for part in parts.iter().rev() {
        let item = part.to_rdf(out);
        let cell = Term::from(BlankNode::default());
        out.push(Triple::new(as_subject(&cell), rdf("first"), item));
        out.push(Triple::new(as_subject(&cell), rdf("rest"), tail));
        tail = cell;
    }
    tail
}
