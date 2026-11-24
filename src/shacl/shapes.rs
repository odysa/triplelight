//! The shapes graph: which nodes are shapes, their targets, paths, severity and messages, parsed once and cached.
use super::constraints::{self, Constraint};
use super::error::ValidationError;
use super::graph::Graph;
use super::path::Path;
use super::report::Severity;
use super::vocab::{rdfs, sh};
use crate::rdf::{Literal, NamedNode, Term};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Target {
    Node(Term),
    Class(Term),
    SubjectsOf(NamedNode),
    ObjectsOf(NamedNode),
}

#[derive(Debug)]
pub struct Shape {
    pub id: Term,
    pub targets: Vec<Target>,
    /// `Some` for a property shape.
    pub path: Option<Path>,
    pub deactivated: bool,
    pub severity: Severity,
    pub messages: Vec<Literal>,
    pub constraints: Vec<Constraint>,
}

pub struct ShapesGraph {
    graph: Graph,
    cache: RefCell<HashMap<Term, Rc<Shape>>>,
    declared: Vec<Term>,
}

impl ShapesGraph {
    /// Finds the shapes that have targets (the ones validation starts from); other shapes are parsed when a constraint refers to them.
    pub fn new(graph: Graph) -> Result<ShapesGraph, ValidationError> {
        let (node_shape, property_shape) = (Term::from(sh("NodeShape")), Term::from(sh("PropertyShape")));
        let rdfs_class = Term::from(rdfs("Class"));
        let mut candidates: Vec<Term> = Vec::new();
        let mut seen = HashSet::new();
        for p in ["targetNode", "targetClass", "targetSubjectsOf", "targetObjectsOf"] {
            for s in graph.subjects_of(&sh(p)) { if seen.insert(s.clone()) { candidates.push(s.clone()) } }
        }
        // An implicit class target: a shape that is also an rdfs:Class (SHACL 2.1.3.1).
        for shape_class in [&node_shape, &property_shape] {
            for s in graph.instances_of(shape_class) {
                if graph.is_instance(&s, &rdfs_class) && seen.insert(s.clone()) { candidates.push(s) }
            }
        }
        candidates.sort_by_key(|t| t.to_string());
        let shapes = ShapesGraph { graph, cache: RefCell::new(HashMap::new()), declared: candidates };
        shapes.refuse_recursion()?;
        Ok(shapes)
    }

    pub fn graph(&self) -> &Graph { &self.graph }

    /// SHACL leaves a shape that (through `sh:node`, `sh:property`, `sh:not`, `sh:and`, `sh:or`, `sh:xone` or `sh:qualifiedValueShape`) refers
    /// back to itself undefined, so it is refused here, before validation: otherwise the answer and the cost would depend on the data (a
    /// long chain overflows the stack, a ladder of two references per level takes exponential time). Walks every shape reachable from the
    /// shapes validation starts from, iteratively, so a long reference chain cannot overflow the stack either.
    fn refuse_recursion(&self) -> Result<(), ValidationError> {
        #[derive(PartialEq)]
        enum Mark { Active, Done }
        let mut marks: HashMap<Term, Mark> = HashMap::new();
        for root in &self.declared {
            if marks.contains_key(root) { continue }
            // each frame: the shape and the references of it not yet visited
            let mut stack: Vec<(Term, std::vec::IntoIter<Term>)> = Vec::new();
            let refs = |id: &Term| -> Result<std::vec::IntoIter<Term>, ValidationError> {
                let shape = self.shape(id)?;
                let all: Vec<Term> = shape.constraints.iter().flat_map(|c| c.referenced_shapes().into_iter().cloned()).collect();
                Ok(all.into_iter())
            };
            marks.insert(root.clone(), Mark::Active);
            stack.push((root.clone(), refs(root)?));
            while let Some((id, it)) = stack.last_mut() {
                match it.next() {
                    Some(next) => match marks.get(&next) {
                        Some(Mark::Active) => return Err(ValidationError::UndefinedRecursion(format!("{id} reaches {next}, which is already being evaluated"))),
                        Some(Mark::Done) => {}
                        None => {
                            marks.insert(next.clone(), Mark::Active);
                            let nested = refs(&next)?;
                            stack.push((next, nested));
                        }
                    },
                    None => { marks.insert(id.clone(), Mark::Done); stack.pop(); }
                }
            }
        }
        Ok(())
    }

    /// The work validation starts from, one shape at a time and in a fixed order: each shape that has targets with its focus nodes in
    /// `data`. It is lazy, so only one shape's focus nodes are held at once, and a malformed shape surfaces when its turn comes.
    pub fn work_list<'a>(&'a self, data: &'a Graph) -> impl Iterator<Item = Result<(Term, Vec<Term>), ValidationError>> + 'a {
        self.declared.iter().map(move |id| Ok((id.clone(), self.focus_nodes(&*self.shape(id)?, data))))
    }

    /// The shape with this id, parsed on first use. A node that has no shape properties is still a (trivial) shape.
    pub fn shape(&self, id: &Term) -> Result<Rc<Shape>, ValidationError> {
        if let Some(s) = self.cache.borrow().get(id) { return Ok(Rc::clone(s)) }
        let shape = Rc::new(self.parse_shape(id)?);
        self.cache.borrow_mut().insert(id.clone(), Rc::clone(&shape));
        Ok(shape)
    }

    fn parse_shape(&self, id: &Term) -> Result<Shape, ValidationError> {
        let g = &self.graph;
        let mut targets: Vec<Target> = Vec::new();
        targets.extend(g.objects(id, &sh("targetNode")).iter().cloned().map(Target::Node));
        targets.extend(g.objects(id, &sh("targetClass")).iter().cloned().map(Target::Class));
        for (name, make) in [("targetSubjectsOf", Target::SubjectsOf as fn(NamedNode) -> Target), ("targetObjectsOf", Target::ObjectsOf)] {
            for o in g.objects(id, &sh(name)) {
                let Term::NamedNode(p) = o else { return Err(ValidationError::MalformedShape(format!("{id}: sh:{name} needs an IRI, got {o}"))) };
                targets.push(make(p.clone()));
            }
        }
        let is_shape_class = |c: &str| g.is_instance(id, &Term::from(sh(c)));
        if (is_shape_class("NodeShape") || is_shape_class("PropertyShape")) && g.is_instance(id, &Term::from(rdfs("Class"))) {
            targets.push(Target::Class(id.clone()));
        }
        let path = match g.object(id, &sh("path"))? {
            Some(p) => Some(Path::parse(p, g)?),
            None => None,
        };
        if g.objects(id, &sh("path")).len() > 1 { return Err(ValidationError::MalformedShape(format!("{id} has several sh:path"))) }
        let deactivated = match g.object(id, &sh("deactivated"))? {
            Some(Term::Literal(l)) => l.value() == "true",
            _ => false,
        };
        let severity = match g.object(id, &sh("severity"))? {
            Some(Term::NamedNode(n)) => Severity::from_iri(n),
            Some(other) => return Err(ValidationError::MalformedShape(format!("{id}: severity {other} is not an IRI"))),
            None => Severity::Violation,
        };
        let messages = g.objects(id, &sh("message")).iter().filter_map(|m| if let Term::Literal(l) = m { Some(l.clone()) } else { None }).collect();
        let constraints = constraints::parse(id, g)?;
        Ok(Shape { id: id.clone(), targets, path, deactivated, severity, messages, constraints })
    }

    /// The focus nodes of a shape: the union of its targets over the data graph, each once, in first-reached order.
    pub fn focus_nodes(&self, shape: &Shape, data: &Graph) -> Vec<Term> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut add = |t: Term| if seen.insert(t.clone()) { out.push(t) };
        for target in &shape.targets {
            match target {
                Target::Node(n) => add(n.clone()),
                Target::Class(c) => data.instances_of(c).into_iter().for_each(&mut add),
                Target::SubjectsOf(p) => data.subjects_of(p).iter().cloned().for_each(&mut add),
                Target::ObjectsOf(p) => data.objects_of(p).iter().cloned().for_each(&mut add),
            }
        }
        out
    }
}
