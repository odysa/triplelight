//! The constraint components of SHACL Core. This file is owned by the constraints lane: the types and signatures below are the
//! contract with the evaluator, and the placeholder `Constraint` has no variants until the components land.
use super::error::ValidationError;
use super::evaluator::Evaluator;
use super::graph::Graph;
use super::report::Outcome;
use super::shapes::Shape;
use crate::rdf::Term;

/// What a constraint is evaluated against: the focus node, the value nodes of the shape's path (the focus itself for a node
/// shape), the shape the constraint belongs to, and the data graph.
pub struct ConstraintContext<'a> {
    pub focus: &'a Term,
    pub value_nodes: &'a [Term],
    pub shape: &'a Shape,
    pub data: &'a Graph,
}

#[derive(Clone, Debug)]
pub enum Constraint {}

/// Reads the constraint parameters of the shape `shape` from the shapes graph.
pub fn parse(_shape: &Term, _shapes: &Graph) -> Result<Vec<Constraint>, ValidationError> { Ok(Vec::new()) }

impl Constraint {
    pub fn evaluate(&self, _cx: &ConstraintContext<'_>, _ev: &mut Evaluator<'_>) -> Result<Vec<Outcome>, ValidationError> {
        match *self {}
    }
}
