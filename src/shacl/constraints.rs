use std::cmp::Ordering;
use std::collections::HashSet;

use crate::rdf::{Literal, NamedNode, Term};

use super::error::ValidationError;
use super::graph::Graph;
use super::path::Path;
use super::report::{ConstraintViolation, Outcome, ValidationResult};

pub type CheckNode<'a> =
    dyn FnMut(&Term, &Term) -> Result<Vec<ValidationResult>, ValidationError> + 'a;

pub struct ConstraintContext<'a> {
    pub focus: &'a Term,
    pub value_nodes: &'a [Term],
    pub data: &'a Graph,
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

#[derive(Clone, Debug)]
pub enum Constraint {
    Class(Term),
    Datatype(NamedNode),
    NodeKind(NodeKind),
    MinCount(Limit),
    MaxCount(Limit),
    MinLength(Limit),
    MaxLength(Limit),
    In(Vec<Term>),
    HasValue(Term),
    LanguageIn(Vec<String>),
    UniqueLang(bool),
    Equals(NamedNode),
    Disjoint(NamedNode),
    Not(Term),
    And(Vec<Term>),
    Or(Vec<Term>),
    Xone(Vec<Term>),
    Node(Term),
    Property(Term),
    Closed(HashSet<NamedNode>),
    Qualified {
        shape: Term,
        siblings: Vec<Term>,
        min: Option<Limit>,
        max: Option<Limit>,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum NodeKind {
    Blank,
    Iri,
    Literal,
    BlankOrIri,
    BlankOrLiteral,
    IriOrLiteral,
}

impl NodeKind {
    fn accepts(self, value: &Term) -> bool {
        match value {
            Term::BlankNode(_) => {
                matches!(self, Self::Blank | Self::BlankOrIri | Self::BlankOrLiteral)
            }
            Term::NamedNode(_) => matches!(self, Self::Iri | Self::BlankOrIri | Self::IriOrLiteral),
            Term::Literal(_) => matches!(
                self,
                Self::Literal | Self::BlankOrLiteral | Self::IriOrLiteral
            ),
            #[cfg(feature = "rdf-12")]
            Term::Triple(_) => false,
        }
    }
}

/// An XSD integer compared only against a nonnegative size. Preserve the order
/// of bounds outside the platform's usize range without truncating them.
#[derive(Clone, Copy, Debug)]
pub enum Limit {
    Negative,
    Number(usize),
    AboveUsize,
}

impl Limit {
    fn compare(self, size: usize) -> Ordering {
        match self {
            Self::Negative => Ordering::Greater,
            Self::Number(bound) => size.cmp(&bound),
            Self::AboveUsize => Ordering::Less,
        }
    }
}

fn iri(namespace: &str, local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{namespace}{local}"))
}

fn invalid(shape: &Term, parameter: &str, expected: &str) -> ValidationError {
    ValidationError::MalformedShape(format!("{shape}: sh:{parameter} {expected}"))
}

fn literal(term: &Term) -> Option<&Literal> {
    if let Term::Literal(literal) = term {
        Some(literal)
    } else {
        None
    }
}

fn typed_value<'a>(term: &'a Term, datatype: &str) -> Option<&'a str> {
    let literal = literal(term)?;
    (literal.datatype().as_str().strip_prefix(XSD) == Some(datatype)).then(|| literal.value())
}

fn integer_text(text: &str) -> Option<(bool, &str)> {
    let text = text.trim_matches([' ', '\t', '\n', '\r']);
    let negative = text.starts_with('-');
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| (negative, digits.trim_start_matches('0')))
}

fn limit(term: &Term) -> Option<Limit> {
    let (negative, digits) = integer_text(typed_value(term, "integer")?)?;
    Some(if digits.is_empty() {
        Limit::Number(0)
    } else if negative {
        Limit::Negative
    } else {
        digits.parse().map_or(Limit::AboveUsize, Limit::Number)
    })
}

fn boolean(term: &Term) -> Option<bool> {
    match typed_value(term, "boolean")?.trim_matches([' ', '\t', '\n', '\r']) {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn lexical(value: &Term) -> Option<&str> {
    match value {
        Term::NamedNode(node) => Some(node.as_str()),
        Term::Literal(literal) => Some(literal.value()),
        _ => None,
    }
}

fn datatype_matches(value: &Term, datatype: &NamedNode) -> bool {
    let Some(literal) = literal(value) else {
        return false;
    };
    if literal.datatype().as_str() != datatype.as_str() {
        return false;
    }
    let text = literal.value().trim_matches([' ', '\t', '\n', '\r']);
    match datatype.as_str().strip_prefix(XSD) {
        Some("integer") => integer_text(text).is_some(),
        Some("boolean") => boolean(value).is_some(),
        _ => true,
    }
}

fn language_matches(tag: &str, range: &str) -> bool {
    range == "*"
        || tag.eq_ignore_ascii_case(range)
        || tag.get(..range.len()).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case(range) && tag.as_bytes().get(range.len()) == Some(&b'-')
        })
}

fn lang_range(term: &Term) -> Option<String> {
    let text = typed_value(term, "string")?;
    let mut parts = text.split('-');
    let first = parts.next()?;
    if first == "*" && parts.next().is_none() {
        return Some(text.to_owned());
    }
    if !(1..=8).contains(&first.len()) || !first.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    parts
        .all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .then(|| text.to_owned())
}

fn shape_reference(shape: &Term, parameter: &str, term: &Term) -> Result<Term, ValidationError> {
    if matches!(term, Term::NamedNode(_) | Term::BlankNode(_)) {
        Ok(term.clone())
    } else {
        Err(invalid(shape, parameter, "must name a shape"))
    }
}

fn optional<'a>(
    shapes: &'a Graph,
    shape: &Term,
    parameter: &str,
) -> Result<Option<&'a Term>, ValidationError> {
    match shapes.objects(shape, &iri(SH, parameter)) {
        [] => Ok(None),
        [term] => Ok(Some(term)),
        _ => Err(invalid(shape, parameter, "has more than one value")),
    }
}

fn closed(shape: &Term, shapes: &Graph) -> Result<Option<Constraint>, ValidationError> {
    let Some(flag) = optional(shapes, shape, "closed")? else {
        return Ok(None);
    };
    let enabled =
        boolean(flag).ok_or_else(|| invalid(shape, "closed", "must be an xsd:boolean"))?;
    let mut allowed = HashSet::new();
    if let Some(head) = optional(shapes, shape, "ignoredProperties")? {
        for item in shapes.list_items(head)? {
            let Term::NamedNode(predicate) = item else {
                return Err(invalid(shape, "ignoredProperties", "must list IRIs"));
            };
            allowed.insert(predicate);
        }
    }
    if !enabled {
        return Ok(None);
    }
    for subshape in shapes.objects(shape, &iri(SH, "property")) {
        if let Some(Term::NamedNode(path)) = optional(shapes, subshape, "path")? {
            allowed.insert(path.clone());
        }
    }
    Ok(Some(Constraint::Closed(allowed)))
}

fn qualified(
    shape: &Term,
    shapes: &Graph,
    property_shape: bool,
) -> Result<Option<Constraint>, ValidationError> {
    let parse_limit = |name| {
        optional(shapes, shape, name)?
            .map(|value| limit(value).ok_or_else(|| invalid(shape, name, "must be an xsd:integer")))
            .transpose()
    };
    let min = parse_limit("qualifiedMinCount")?;
    let max = parse_limit("qualifiedMaxCount")?;
    let target = optional(shapes, shape, "qualifiedValueShape")?;
    if min.is_none() && max.is_none() {
        return Ok(None);
    }
    if !property_shape {
        return Err(invalid(
            shape,
            "qualifiedValueShape",
            "requires a property shape",
        ));
    }
    let target = target.ok_or_else(|| {
        invalid(
            shape,
            "qualifiedValueShape",
            "must be present with a qualified count",
        )
    })?;
    let target = shape_reference(shape, "qualifiedValueShape", target)?;
    let disjoint = optional(shapes, shape, "qualifiedValueShapesDisjoint")?
        .map(|value| {
            boolean(value).ok_or_else(|| {
                invalid(
                    shape,
                    "qualifiedValueShapesDisjoint",
                    "must be an xsd:boolean",
                )
            })
        })
        .transpose()?
        .unwrap_or(false);
    let mut siblings = Vec::new();
    if disjoint {
        let property = iri(SH, "property");
        let qualified = iri(SH, "qualifiedValueShape");
        for parent in shapes.subjects(&property, shape) {
            for sibling in shapes.objects(&parent, &property) {
                for candidate in shapes.objects(sibling, &qualified) {
                    if *candidate != target && !siblings.contains(candidate) {
                        siblings.push(shape_reference(sibling, "qualifiedValueShape", candidate)?);
                    }
                }
            }
        }
    }
    Ok(Some(Constraint::Qualified {
        shape: target,
        siblings,
        min,
        max,
    }))
}

pub fn parse(shape: &Term, shapes: &Graph) -> Result<Vec<Constraint>, ValidationError> {
    let mut constraints = Vec::new();
    let property_shape = !shapes.objects(shape, &iri(SH, "path")).is_empty();
    for (predicate, value) in shapes.outgoing(shape) {
        let Some(name) = predicate.as_str().strip_prefix(SH) else {
            continue;
        };
        let required_limit =
            || limit(value).ok_or_else(|| invalid(shape, name, "must be an xsd:integer"));
        let named = || match value {
            Term::NamedNode(node) => Ok(node.clone()),
            _ => Err(invalid(shape, name, "must be an IRI")),
        };
        let shape_list = || {
            shapes
                .list_items(value)?
                .iter()
                .map(|term| shape_reference(shape, name, term))
                .collect::<Result<Vec<_>, _>>()
        };
        let constraint = match name {
            "class" => Constraint::Class(named()?.into()),
            "datatype" => Constraint::Datatype(named()?),
            "nodeKind" => {
                let node_kind = match named()?.as_str().strip_prefix(SH) {
                    Some("BlankNode") => NodeKind::Blank,
                    Some("IRI") => NodeKind::Iri,
                    Some("Literal") => NodeKind::Literal,
                    Some("BlankNodeOrIRI") => NodeKind::BlankOrIri,
                    Some("BlankNodeOrLiteral") => NodeKind::BlankOrLiteral,
                    Some("IRIOrLiteral") => NodeKind::IriOrLiteral,
                    _ => return Err(invalid(shape, name, "must name a SHACL node kind")),
                };
                Constraint::NodeKind(node_kind)
            }
            "minCount" => Constraint::MinCount(required_limit()?),
            "maxCount" => Constraint::MaxCount(required_limit()?),
            "minLength" => Constraint::MinLength(required_limit()?),
            "maxLength" => Constraint::MaxLength(required_limit()?),
            "hasValue" => Constraint::HasValue(value.clone()),
            "in" => Constraint::In(shapes.list_items(value)?),
            "languageIn" => {
                let ranges = shapes
                    .list_items(value)?
                    .iter()
                    .map(|member| {
                        lang_range(member)
                            .ok_or_else(|| invalid(shape, name, "must list basic language ranges"))
                    })
                    .collect::<Result<_, _>>()?;
                Constraint::LanguageIn(ranges)
            }
            "uniqueLang" => Constraint::UniqueLang(
                boolean(value).ok_or_else(|| invalid(shape, name, "must be an xsd:boolean"))?,
            ),
            "equals" => Constraint::Equals(named()?),
            "disjoint" => Constraint::Disjoint(named()?),
            "not" => Constraint::Not(shape_reference(shape, name, value)?),
            "and" => Constraint::And(shape_list()?),
            "or" => Constraint::Or(shape_list()?),
            "xone" => Constraint::Xone(shape_list()?),
            "node" | "property" => {
                let target = shape_reference(shape, name, value)?;
                let is_property = !shapes.objects(&target, &iri(SH, "path")).is_empty();
                match (name, is_property) {
                    ("node", false) => Constraint::Node(target),
                    ("property", true) => Constraint::Property(target),
                    ("node", true) => return Err(invalid(shape, name, "must name a node shape")),
                    _ => return Err(invalid(shape, name, "must name a property shape")),
                }
            }
            _ => continue,
        };
        if matches!(name, "minCount" | "maxCount" | "uniqueLang") && !property_shape {
            return Err(invalid(shape, name, "requires a property shape"));
        }
        if !matches!(
            name,
            "class"
                | "hasValue"
                | "equals"
                | "disjoint"
                | "not"
                | "and"
                | "or"
                | "xone"
                | "node"
                | "property"
        ) && shapes.objects(shape, predicate).len() > 1
        {
            return Err(invalid(shape, name, "has more than one value"));
        }
        constraints.push(constraint);
    }
    constraints.extend(closed(shape, shapes)?);
    constraints.extend(qualified(shape, shapes, property_shape)?);
    Ok(constraints)
}

fn violation(component: &str, value: Option<&Term>) -> Outcome {
    result(component, value, None)
}

fn result(component: &str, value: Option<&Term>, path_override: Option<Path>) -> Outcome {
    Outcome::Violation(ConstraintViolation {
        component: iri(SH, component),
        value: value.cloned(),
        message: None,
        details: Vec::new(),
        path_override,
    })
}

fn check_values(component: &str, values: &[Term], accepts: impl Fn(&Term) -> bool) -> Vec<Outcome> {
    values
        .iter()
        .filter(|value| !accepts(value))
        .map(|value| violation(component, Some(value)))
        .collect()
}

fn check_set(component: &str, passes: bool) -> Vec<Outcome> {
    if passes {
        Vec::new()
    } else {
        vec![violation(component, None)]
    }
}

fn check_conformance(
    check_node: &mut CheckNode<'_>,
    component: &str,
    shapes: &[Term],
    values: &[Term],
    accepts: impl Fn(usize) -> bool,
) -> Result<Vec<Outcome>, ValidationError> {
    let mut violations = Vec::new();
    for value in values {
        let mut conforming = 0;
        for shape in shapes {
            if check_node(shape, value)?.is_empty() {
                conforming += 1;
            }
        }
        if !accepts(conforming) {
            violations.push(violation(component, Some(value)));
        }
    }
    Ok(violations)
}

impl Constraint {
    pub fn evaluate(
        &self,
        cx: &ConstraintContext<'_>,
        check_node: &mut CheckNode<'_>,
    ) -> Result<Vec<Outcome>, ValidationError> {
        let values = cx.value_nodes;
        let results = match self {
            Self::Class(class) => check_values("ClassConstraintComponent", values, |value| {
                !matches!(value, Term::Literal(_)) && cx.data.is_instance(value, class)
            }),
            Self::Datatype(datatype) => {
                check_values("DatatypeConstraintComponent", values, |value| {
                    datatype_matches(value, datatype)
                })
            }
            Self::NodeKind(kind) => check_values("NodeKindConstraintComponent", values, |value| {
                kind.accepts(value)
            }),
            Self::MinCount(limit) => check_set(
                "MinCountConstraintComponent",
                limit.compare(values.len()) != Ordering::Less,
            ),
            Self::MaxCount(limit) => check_set(
                "MaxCountConstraintComponent",
                limit.compare(values.len()) != Ordering::Greater,
            ),
            Self::MinLength(limit) => {
                check_values("MinLengthConstraintComponent", values, |value| {
                    lexical(value)
                        .is_some_and(|text| limit.compare(text.chars().count()) != Ordering::Less)
                })
            }
            Self::MaxLength(limit) => {
                check_values("MaxLengthConstraintComponent", values, |value| {
                    lexical(value).is_some_and(|text| {
                        limit.compare(text.chars().count()) != Ordering::Greater
                    })
                })
            }
            Self::In(allowed) => check_values("InConstraintComponent", values, |value| {
                allowed.contains(value)
            }),
            Self::HasValue(required) => {
                check_set("HasValueConstraintComponent", values.contains(required))
            }
            Self::LanguageIn(ranges) => {
                check_values("LanguageInConstraintComponent", values, |value| {
                    literal(value)
                        .and_then(Literal::language)
                        .is_some_and(|tag| ranges.iter().any(|range| language_matches(tag, range)))
                })
            }
            Self::UniqueLang(enabled) => {
                let mut seen = HashSet::new();
                let mut duplicates = HashSet::new();
                if *enabled {
                    for tag in values
                        .iter()
                        .filter_map(|value| literal(value).and_then(Literal::language))
                    {
                        let tag = tag.to_ascii_lowercase();
                        if !seen.insert(tag.clone()) {
                            duplicates.insert(tag);
                        }
                    }
                }
                duplicates
                    .iter()
                    .map(|_| violation("UniqueLangConstraintComponent", None))
                    .collect()
            }
            Self::Equals(predicate) => {
                let others = cx.data.objects(cx.focus, predicate);
                let mut violations = check_values("EqualsConstraintComponent", values, |value| {
                    others.contains(value)
                });
                violations.extend(check_values("EqualsConstraintComponent", others, |value| {
                    values.contains(value)
                }));
                violations
            }
            Self::Disjoint(predicate) => {
                let others = cx.data.objects(cx.focus, predicate);
                check_values("DisjointConstraintComponent", values, |value| {
                    !others.contains(value)
                })
            }
            Self::Not(shape) => {
                return check_conformance(
                    check_node,
                    "NotConstraintComponent",
                    std::slice::from_ref(shape),
                    values,
                    |count| count == 0,
                );
            }
            Self::And(shapes) => {
                return check_conformance(
                    check_node,
                    "AndConstraintComponent",
                    shapes,
                    values,
                    |count| count == shapes.len(),
                );
            }
            Self::Or(shapes) => {
                return check_conformance(
                    check_node,
                    "OrConstraintComponent",
                    shapes,
                    values,
                    |count| count > 0,
                );
            }
            Self::Xone(shapes) => {
                return check_conformance(
                    check_node,
                    "XoneConstraintComponent",
                    shapes,
                    values,
                    |count| count == 1,
                );
            }
            Self::Node(shape) => {
                return check_conformance(
                    check_node,
                    "NodeConstraintComponent",
                    std::slice::from_ref(shape),
                    values,
                    |count| count == 1,
                );
            }
            Self::Property(shape) => {
                let mut violations = Vec::new();
                for value in values {
                    violations.push(Outcome::Forwarded(check_node(shape, value)?));
                }
                violations
            }
            Self::Closed(allowed) => {
                let mut violations = Vec::new();
                for value in values {
                    for (predicate, object) in cx.data.outgoing(value) {
                        if !allowed.contains(predicate) {
                            violations.push(result(
                                "ClosedConstraintComponent",
                                Some(object),
                                Some(Path::Predicate(predicate.clone())),
                            ));
                        }
                    }
                }
                violations
            }
            Self::Qualified {
                shape,
                siblings,
                min,
                max,
            } => {
                let mut count = 0;
                for value in values {
                    let conforms = check_node(shape, value)?.is_empty();
                    let mut excludes = false;
                    for sibling in siblings {
                        excludes |= check_node(sibling, value)?.is_empty();
                    }
                    if conforms && !excludes {
                        count += 1;
                    }
                }
                let mut violations = Vec::new();
                if min.is_some_and(|limit| limit.compare(count) == Ordering::Less) {
                    violations.push(violation("QualifiedMinCountConstraintComponent", None));
                }
                if max.is_some_and(|limit| limit.compare(count) == Ordering::Greater) {
                    violations.push(violation("QualifiedMaxCountConstraintComponent", None));
                }
                violations
            }
        };
        Ok(results)
    }
}
