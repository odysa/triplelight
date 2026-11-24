//! The shape evaluator: for a shape and a focus node it finds the value nodes, runs the shape's constraint components, and turns
//! their violations into validation results. It owns the recursion guard.
use super::constraints::{CheckNode, ConstraintContext};
use super::error::ValidationError;
use super::graph::Graph;
use super::report::{Outcome, ValidationReport, ValidationResult};
use super::shapes::{MAX_SHAPE_DEPTH, ShapesGraph};
use crate::rdf::{Term, Triple};
use std::collections::HashMap;

pub struct Evaluator<'a> {
    shapes: &'a ShapesGraph,
    data: &'a Graph,
    /// The (shape, focus node) pairs being evaluated, outermost first: meeting a pair again is a recursive reference.
    active: Vec<(Term, Term)>,
    conformance: HashMap<Term, HashMap<Term, bool>>,
}

impl<'a> Evaluator<'a> {
    pub fn new(shapes: &'a ShapesGraph, data: &'a Graph) -> Evaluator<'a> {
        Evaluator {
            shapes,
            data,
            active: Vec::new(),
            conformance: HashMap::new(),
        }
    }

    /// Checks `focus` against `shape` and returns its validation results. Used for the shapes validation starts from, and by
    /// constraints that refer to other shapes (`sh:node`, `sh:property`, `sh:not`, ...).
    pub fn check_node(
        &mut self,
        shape: &Term,
        focus: &Term,
    ) -> Result<Vec<ValidationResult>, ValidationError> {
        let mut results = Vec::new();
        self.evaluate(shape, focus, Some(&mut results))?;
        Ok(results)
    }

    fn evaluate(
        &mut self,
        shape: &Term,
        focus: &Term,
        report: Option<&mut Vec<ValidationResult>>,
    ) -> Result<bool, ValidationError> {
        if let Some(&conforms) = self
            .conformance
            .get(shape)
            .and_then(|nodes| nodes.get(focus))
            && (conforms || report.is_none())
        {
            return Ok(conforms);
        }
        let parsed = self.shapes.shape(shape)?;
        if parsed.deactivated {
            return Ok(true);
        }
        let key = (shape.clone(), focus.clone());
        if self.active.contains(&key) {
            return Err(ValidationError::UndefinedRecursion(format!(
                "{shape} reaches itself at focus node {focus}"
            )));
        }
        if self.active.len() >= MAX_SHAPE_DEPTH {
            return Err(ValidationError::Unsupported(format!(
                "shape references nested more than {MAX_SHAPE_DEPTH} deep (at {shape})"
            )));
        }
        let nested = !self.active.is_empty();
        self.active.push(key);
        let outcome = self.run(&parsed, focus, report);
        self.active.pop();
        if nested && let Ok(conforms) = outcome {
            self.conformance
                .entry(shape.clone())
                .or_default()
                .insert(focus.clone(), conforms);
        }
        outcome
    }

    fn run(
        &mut self,
        shape: &super::shapes::Shape,
        focus: &Term,
        report: Option<&mut Vec<ValidationResult>>,
    ) -> Result<bool, ValidationError> {
        let data = self.data;
        let value_nodes = match &shape.path {
            Some(path) => path.values(focus, data),
            None => vec![focus.clone()],
        };
        let cx = ConstraintContext {
            focus,
            value_nodes: &value_nodes,
            data,
        };
        let Some(results) = report else {
            let mut conforms = true;
            for constraint in &shape.constraints {
                conforms &= constraint.conforms(&cx, self)?;
            }
            return Ok(conforms);
        };
        for constraint in &shape.constraints {
            for outcome in constraint.evaluate(&cx, self)? {
                let violation = match outcome {
                    Outcome::Violation(v) => v,
                    Outcome::Forwarded(nested) => {
                        results.extend(nested);
                        continue;
                    }
                };
                let messages = if shape.messages.is_empty() {
                    violation.message.into_iter().collect()
                } else {
                    shape.messages.clone()
                };
                results.push(ValidationResult {
                    focus_node: focus.clone(),
                    result_path: violation.path_override.or_else(|| shape.path.clone()),
                    value: violation.value,
                    source_shape: shape.id.clone(),
                    source_constraint_component: violation.component,
                    severity: shape.severity.clone(),
                    messages,
                    details: violation.details,
                });
            }
        }
        Ok(results.is_empty())
    }
}

impl CheckNode for Evaluator<'_> {
    fn conforms(&mut self, shape: &Term, focus: &Term) -> Result<bool, ValidationError> {
        self.evaluate(shape, focus, None)
    }

    fn report(
        &mut self,
        shape: &Term,
        focus: &Term,
    ) -> Result<Vec<ValidationResult>, ValidationError> {
        self.check_node(shape, focus)
    }
}

/// Validates `data` against `shapes`: every shape with targets is checked at each of its focus nodes.
pub fn validate(data: &[Triple], shapes: &[Triple]) -> Result<ValidationReport, ValidationError> {
    // the shapes graph first: a rejected one (malformed, recursive, too deep) exits before the data graph is indexed
    let shapes = ShapesGraph::new(Graph::new(shapes))?;
    let data = Graph::new(data);
    let mut ev = Evaluator::new(&shapes, &data);
    let mut results = Vec::new();
    for item in shapes.work_list(&data) {
        let (id, foci) = item?;
        for focus in foci {
            results.extend(ev.check_node(&id, &focus)?)
        }
    }
    Ok(ValidationReport::new(results))
}
