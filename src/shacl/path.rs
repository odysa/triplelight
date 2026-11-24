//! SHACL property paths: parsing from the shapes graph, evaluation over the data graph, and writing back as RDF.
//!
//! A path node that several paths share (the same blank node reached twice) is parsed once and shared (`Rc`), and evaluating and writing a
//! path memoize per shared node, so a shapes graph cannot make a path cost exponential time or memory in its nesting depth.
use super::error::ValidationError;
use super::graph::Graph;
use super::vocab::{rdf, sh};
use crate::rdf::{BlankNode, NamedNode, Term, Triple};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The deepest nesting of path nodes `Path::parse` accepts.
const MAX_PATH_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Path {
    Predicate(NamedNode),
    Inverse(Rc<Path>),
    /// Applied left to right.
    Sequence(Vec<Rc<Path>>),
    Alternative(Vec<Rc<Path>>),
    ZeroOrMore(Rc<Path>),
    OneOrMore(Rc<Path>),
    ZeroOrOne(Rc<Path>),
}

impl Path {
    /// The path rooted at `node` in the shapes graph (SHACL 2.3.1): an IRI, or a blank node that is a list (a sequence)
    /// or has exactly one of `sh:inversePath`, `sh:alternativePath`, `sh:zeroOrMorePath`, `sh:oneOrMorePath`,
    /// `sh:zeroOrOnePath`.
    pub fn parse(node: &Term, shapes: &Graph) -> Result<Path, ValidationError> {
        let mut parser = PathParser { shapes, visiting: Vec::new(), done: HashMap::new() };
        let root = parser.parse_at(node)?;
        Ok((*root).clone())
    }

    /// The value nodes reached from `focus`: a set, in the order first reached.
    pub fn values(&self, focus: &Term, data: &Graph) -> Vec<Term> {
        let mut walk = Walk { data, memo: HashMap::new() };
        dedupe(walk.eval(self, focus, false))
    }

    /// Writes the path as RDF (a validation result's `sh:resultPath`) and returns its root: the IRI itself for a predicate
    /// path, a fresh blank node (with its triples appended to `out`) for the others. A node shared inside the path is written once.
    pub fn to_rdf(&self, out: &mut Vec<Triple>) -> Term {
        let mut writer = PathWriter { out, done: HashMap::new() };
        writer.write(self)
    }
}

fn dedupe(values: Vec<Term>) -> Vec<Term> {
    let mut seen = HashSet::new();
    values.into_iter().filter(|v| seen.insert(v.clone())).collect()
}

/// Parses path nodes, sharing each blank node's parsed path.
struct PathParser<'a> {
    shapes: &'a Graph,
    /// The blank path nodes being parsed, outermost first: a node that contains itself is a malformed path.
    visiting: Vec<Term>,
    done: HashMap<Term, Rc<Path>>,
}

impl PathParser<'_> {
    fn parse_at(&mut self, node: &Term) -> Result<Rc<Path>, ValidationError> {
        let bad = |why: &str| ValidationError::MalformedShape(format!("path {node}: {why}"));
        match node {
            Term::NamedNode(p) => Ok(Rc::new(Path::Predicate(p.clone()))),
            Term::BlankNode(_) => {
                if let Some(done) = self.done.get(node) { return Ok(Rc::clone(done)) }
                if self.visiting.contains(node) { return Err(bad("a path contains itself")) }
                if self.visiting.len() >= MAX_PATH_DEPTH { return Err(bad("paths nested more than 64 deep are not supported")) }
                self.visiting.push(node.clone());
                let parsed = self.parse_blank(node);
                self.visiting.pop();
                let parsed = Rc::new(parsed?);
                self.done.insert(node.clone(), Rc::clone(&parsed));
                Ok(parsed)
            }
            _ => Err(bad("a path is an IRI or a blank node")),
        }
    }

    fn parse_blank(&mut self, node: &Term) -> Result<Path, ValidationError> {
        let bad = |why: &str| ValidationError::MalformedShape(format!("path {node}: {why}"));
        let shapes = self.shapes;
        if !shapes.objects(node, &rdf("first")).is_empty() {
            let items = shapes.list_items(node)?;
            if items.len() < 2 { return Err(bad("a sequence path needs at least two members")) }
            return Ok(Path::Sequence(items.iter().map(|i| self.parse_at(i)).collect::<Result<_, _>>()?));
        }
        let forms = ["inversePath", "alternativePath", "zeroOrMorePath", "oneOrMorePath", "zeroOrOnePath"];
        let present: Vec<(&str, &Term)> = forms.iter().flat_map(|f| shapes.objects(node, &sh(f)).iter().map(move |o| (*f, o))).collect();
        match present.as_slice() {
            [("inversePath", o)] => Ok(Path::Inverse(self.parse_at(o)?)),
            [("alternativePath", o)] => {
                let items = shapes.list_items(o)?;
                if items.len() < 2 { return Err(bad("an alternative path needs at least two members")) }
                Ok(Path::Alternative(items.iter().map(|i| self.parse_at(i)).collect::<Result<_, _>>()?))
            }
            [("zeroOrMorePath", o)] => Ok(Path::ZeroOrMore(self.parse_at(o)?)),
            [("oneOrMorePath", o)] => Ok(Path::OneOrMore(self.parse_at(o)?)),
            [("zeroOrOnePath", o)] => Ok(Path::ZeroOrOne(self.parse_at(o)?)),
            _ => Err(bad("not a recognised path form")),
        }
    }
}

/// Evaluates a path over the data graph, in either direction, remembering what each shared path node reaches from each data node.
struct Walk<'a> {
    data: &'a Graph,
    memo: HashMap<(usize, bool, Term), Rc<Vec<Term>>>,
}

impl Walk<'_> {
    /// What the shared path node `p` reaches from `node` (walked backwards when `inverse`), computed once.
    fn go(&mut self, p: &Rc<Path>, node: &Term, inverse: bool) -> Rc<Vec<Term>> {
        let key = (Rc::as_ptr(p) as usize, inverse, node.clone());
        if let Some(hit) = self.memo.get(&key) { return Rc::clone(hit) }
        let value = Rc::new(self.eval(p, node, inverse));
        self.memo.insert(key, Rc::clone(&value));
        value
    }

    fn eval(&mut self, p: &Path, node: &Term, inverse: bool) -> Vec<Term> {
        match p {
            Path::Predicate(pred) => if inverse { self.data.subjects(pred, node) } else { self.data.objects(node, pred).to_vec() },
            Path::Inverse(inner) => self.go(inner, node, !inverse).to_vec(),
            Path::Sequence(parts) => {
                let mut frontier = vec![node.clone()];
                // forwards left to right; backwards, the last part first
                let ordered: Vec<&Rc<Path>> = if inverse { parts.iter().rev().collect() } else { parts.iter().collect() };
                for part in ordered {
                    let mut next = Vec::new();
                    let mut seen = HashSet::new();
                    for n in &frontier { for v in self.go(part, n, inverse).iter() { if seen.insert(v.clone()) { next.push(v.clone()) } } }
                    frontier = next;
                }
                frontier
            }
            Path::Alternative(parts) => {
                let mut out = Vec::new();
                for part in parts { out.extend(self.go(part, node, inverse).iter().cloned()) }
                dedupe(out)
            }
            Path::ZeroOrOne(inner) => { let mut v = vec![node.clone()]; v.extend(self.go(inner, node, inverse).iter().cloned()); dedupe(v) }
            Path::ZeroOrMore(inner) => self.closure(inner, node, inverse, true),
            Path::OneOrMore(inner) => self.closure(inner, node, inverse, false),
        }
    }

    /// Reachability through `inner` (zero or more steps when `reflexive`, one or more otherwise), cycle-safe.
    fn closure(&mut self, inner: &Rc<Path>, from: &Term, inverse: bool, reflexive: bool) -> Vec<Term> {
        let mut out = Vec::new();
        let mut seen: HashSet<Term> = HashSet::new();
        if reflexive { seen.insert(from.clone()); out.push(from.clone()) }
        let mut stack = vec![from.clone()];
        while let Some(n) = stack.pop() {
            for v in self.go(inner, &n, inverse).iter() {
                if seen.insert(v.clone()) { out.push(v.clone()); stack.push(v.clone()) }
            }
        }
        out
    }
}

/// Writes a path as RDF; a shared path node is written once and reused.
struct PathWriter<'a> {
    out: &'a mut Vec<Triple>,
    done: HashMap<usize, Term>,
}

impl PathWriter<'_> {
    fn write_shared(&mut self, p: &Rc<Path>) -> Term {
        let key = Rc::as_ptr(p) as usize;
        if let Some(t) = self.done.get(&key) { return t.clone() }
        let t = self.write(p);
        self.done.insert(key, t.clone());
        t
    }

    fn wrap(&mut self, kind: &str, inner: Term) -> Term {
        let b = Term::from(BlankNode::default());
        self.out.push(Triple::new(as_subject(&b), sh(kind), inner));
        b
    }

    fn write(&mut self, p: &Path) -> Term {
        match p {
            Path::Predicate(n) => Term::from(n.clone()),
            Path::Inverse(i) => { let t = self.write_shared(i); self.wrap("inversePath", t) }
            Path::ZeroOrMore(i) => { let t = self.write_shared(i); self.wrap("zeroOrMorePath", t) }
            Path::OneOrMore(i) => { let t = self.write_shared(i); self.wrap("oneOrMorePath", t) }
            Path::ZeroOrOne(i) => { let t = self.write_shared(i); self.wrap("zeroOrOnePath", t) }
            Path::Sequence(parts) => self.list(parts),
            Path::Alternative(parts) => { let l = self.list(parts); self.wrap("alternativePath", l) }
        }
    }

    fn list(&mut self, parts: &[Rc<Path>]) -> Term {
        let mut tail = Term::from(rdf("nil"));
        for part in parts.iter().rev() {
            let item = self.write_shared(part);
            let cell = Term::from(BlankNode::default());
            self.out.push(Triple::new(as_subject(&cell), rdf("first"), item));
            self.out.push(Triple::new(as_subject(&cell), rdf("rest"), tail));
            tail = cell;
        }
        tail
    }
}

fn as_subject(t: &Term) -> crate::rdf::NamedOrBlankNode {
    crate::rdf::NamedOrBlankNode::try_from(t.clone()).expect("a path node is an IRI or a blank node")
}
