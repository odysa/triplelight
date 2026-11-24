//! The constraint components of SHACL Core. This file is owned by the constraints lane: the types and signatures below are the
//! contract with the evaluator, and the placeholder `Constraint` has no variants until the components land.
use super::error::ValidationError;
use super::graph::Graph;
use super::report::{Outcome, ValidationResult};
use crate::rdf::Term;

/// What a constraint is evaluated against: the focus node, the value nodes of the shape's path (the focus itself for a node
/// shape), and the data graph.
pub struct ConstraintContext<'a> {
    pub focus: &'a Term,
    pub value_nodes: &'a [Term],
    pub data: &'a Graph,
}

/// Checks a focus node against another shape (`sh:node`, `sh:property`, `sh:not`, ...): `check_node(shape, focus)`. The
/// evaluator passes it in, so the constraint code does not depend on the evaluator.
pub type CheckNode<'a> = dyn FnMut(&Term, &Term) -> Result<Vec<ValidationResult>, ValidationError> + 'a;

#[derive(Clone, Debug)]
pub enum Constraint {}

/// Reads the constraint parameters of the shape `shape` from the shapes graph.
pub fn parse(_shape: &Term, _shapes: &Graph) -> Result<Vec<Constraint>, ValidationError> { Ok(Vec::new()) }

impl Constraint {
    pub fn evaluate(&self, _cx: &ConstraintContext<'_>, _check_node: &mut CheckNode<'_>) -> Result<Vec<Outcome>, ValidationError> {
        match *self {}
    }
}
