//! The shape evaluator: for a shape and a focus node it finds the value nodes, runs the shape's constraint components, and turns
//! their violations into validation results. It owns the recursion guard.
use super::constraints::ConstraintContext;
use super::error::ValidationError;
use super::graph::Graph;
use super::report::{ValidationReport, ValidationResult};
use super::shapes::ShapesGraph;
use crate::rdf::{Term, Triple};

pub struct Evaluator<'a> {
    shapes: &'a ShapesGraph,
    data: &'a Graph,
    /// The (shape, focus node) pairs being evaluated, outermost first: meeting a pair again is a recursive reference.
    active: Vec<(Term, Term)>,
}

impl<'a> Evaluator<'a> {
    pub fn new(shapes: &'a ShapesGraph, data: &'a Graph) -> Evaluator<'a> { Evaluator { shapes, data, active: Vec::new() } }

    /// Checks `focus` against `shape` and returns its validation results. Used for the shapes validation starts from, and by
    /// constraints that refer to other shapes (`sh:node`, `sh:property`, `sh:not`, ...).
    pub fn check_node(&mut self, shape: &Term, focus: &Term) -> Result<Vec<ValidationResult>, ValidationError> {
        let parsed = self.shapes.shape(shape)?;
        if parsed.deactivated { return Ok(Vec::new()) }
        let key = (shape.clone(), focus.clone());
        if self.active.contains(&key) {
            return Err(ValidationError::UndefinedRecursion(format!("{shape} reaches itself at focus node {focus}")));
        }
        self.active.push(key);
        let outcome = self.run(&parsed, focus);
        self.active.pop();
        outcome
    }

    fn run(&mut self, shape: &super::shapes::Shape, focus: &Term) -> Result<Vec<ValidationResult>, ValidationError> {
        let data = self.data;
        let value_nodes = match &shape.path {
            Some(path) => path.values(focus, data),
            None => vec![focus.clone()],
        };
        let cx = ConstraintContext { focus, value_nodes: &value_nodes, shape, data };
        let mut results = Vec::new();
        for constraint in &shape.constraints {
            for violation in constraint.evaluate(&cx, self)? {
                let messages = if shape.messages.is_empty() { violation.message.into_iter().collect() } else { shape.messages.clone() };
                results.push(ValidationResult {
                    focus_node: focus.clone(),
                    result_path: shape.path.clone(),
                    value: violation.value,
                    source_shape: shape.id.clone(),
                    source_constraint_component: violation.component,
                    severity: shape.severity,
                    messages,
                    details: violation.details,
                });
            }
        }
        Ok(results)
    }
}

/// Validates `data` against `shapes`: every shape with targets is checked at each of its focus nodes.
pub fn validate(data: &[Triple], shapes: &[Triple]) -> Result<ValidationReport, ValidationError> {
    let data = Graph::new(data);
    let shapes = ShapesGraph::new(Graph::new(shapes))?;
    let mut ev = Evaluator::new(&shapes, &data);
    let mut results = Vec::new();
    for id in shapes.declared() {
        let shape = shapes.shape(id)?;
        for focus in shapes.focus_nodes(&shape, &data) { results.extend(ev.check_node(id, &focus)?) }
    }
    Ok(ValidationReport::new(results))
}
