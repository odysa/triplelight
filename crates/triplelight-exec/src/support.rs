//! Up-front rejection of SPARQL features the evaluator does not implement.
//!
//! Principle 1 is "correct answers first": a construct we do not evaluate must fail with `Unsupported`, never be
//! ignored (a `GRAPH` pattern answered from the default graph) or turned into a per-row evaluation error (a `NOW()`
//! that makes every FILTER false). The check runs on the parsed query before any evaluation.
use crate::sparql::{AggregateExpression, Expression, Function, GraphPattern, OrderExpression};
use crate::sparql::Query;

/// `Some(reason)` if `q` uses a feature the evaluator does not support.
/// `dataset`: the query runs against a `DatasetView`, so FROM / FROM NAMED and GRAPH are evaluated.
pub(super) fn unsupported(q: &Query, dataset_view: bool) -> Option<String> {
    let (dataset, pattern) = match q {
        Query::Select { dataset, pattern, .. } | Query::Ask { dataset, pattern, .. } | Query::Construct { dataset, pattern, .. }
        | Query::Describe { dataset, pattern, .. } => (dataset, pattern),
    };
    if dataset.is_some() && !dataset_view { return Some("FROM / FROM NAMED datasets are not supported (only the default graph)".into()) }
    pattern_unsupported(pattern, dataset_view)
}

pub(super) fn pattern_unsupported(p: &GraphPattern, ds: bool) -> Option<String> {
    match p {
        GraphPattern::Bgp { .. } => None,
        GraphPattern::Path { .. } => None,
        GraphPattern::Graph { inner, .. } if ds => pattern_unsupported(inner, ds),
        GraphPattern::Graph { .. } => Some("GRAPH patterns (named graphs) are not supported (only the default graph)".into()),
        GraphPattern::Service { .. } => Some("SERVICE is not supported".into()),
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right } => {
            pattern_unsupported(left, ds).or_else(|| pattern_unsupported(right, ds))
        }
        GraphPattern::LeftJoin { left, right, expression } => pattern_unsupported(left, ds)
            .or_else(|| pattern_unsupported(right, ds))
            .or_else(|| expression.as_ref().and_then(expr_unsupported)),
        GraphPattern::Filter { expr, inner } => expr_unsupported(expr).or_else(|| pattern_unsupported(inner, ds)),
        GraphPattern::Extend { inner, expression, .. } => pattern_unsupported(inner, ds).or_else(|| expr_unsupported(expression)),
        GraphPattern::OrderBy { inner, expression } => pattern_unsupported(inner, ds).or_else(|| expression.iter().find_map(|o| match o {
            OrderExpression::Asc(e) | OrderExpression::Desc(e) => expr_unsupported(e),
        })),
        GraphPattern::Group { inner, aggregates, .. } => pattern_unsupported(inner, ds).or_else(|| aggregates.iter().find_map(|(_, a)| match a {
            AggregateExpression::FunctionCall { expr, name, .. } => match name {
                crate::sparql::AggregateFunction::Custom(f) => Some(format!("custom aggregate <{}> is not supported", f.as_str())),
                _ => expr_unsupported(expr),
            },
            AggregateExpression::CountSolutions { .. } => None,
        })),
        GraphPattern::Values { .. } => None,
        GraphPattern::Project { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => pattern_unsupported(inner, ds),
    }
}

/// XSD constructor casts implemented by `expr::xsd_cast`.
const XSD_CASTS: &[&str] = &["string", "double", "float", "decimal", "integer", "int", "long", "short", "byte", "boolean", "dateTime", "date", "time"];

fn expr_unsupported(e: &Expression) -> Option<String> {
    match e {
        Expression::NamedNode(_) | Expression::Literal(_) | Expression::Variable(_) | Expression::Bound(_) => None,
        Expression::Or(a, b) | Expression::And(a, b) | Expression::Equal(a, b) | Expression::SameTerm(a, b)
        | Expression::Greater(a, b) | Expression::GreaterOrEqual(a, b) | Expression::Less(a, b) | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b) | Expression::Subtract(a, b) | Expression::Multiply(a, b) | Expression::Divide(a, b) => {
            expr_unsupported(a).or_else(|| expr_unsupported(b))
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => expr_unsupported(a),
        Expression::In(a, l) => expr_unsupported(a).or_else(|| l.iter().find_map(expr_unsupported)),
        Expression::If(a, b, c) => expr_unsupported(a).or_else(|| expr_unsupported(b)).or_else(|| expr_unsupported(c)),
        Expression::Coalesce(l) => l.iter().find_map(expr_unsupported),
        // Conservative: GRAPH inside EXISTS is rejected even for dataset queries (not yet covered by fixtures).
        Expression::Exists(p) => pattern_unsupported(p, false),
        Expression::FunctionCall(Function::BNode, args) if !args.is_empty() => Some("BNODE(string) is not supported".into()),
        Expression::FunctionCall(f, args) => function_unsupported(f).or_else(|| args.iter().find_map(expr_unsupported)),
    }
}

fn function_unsupported(f: &Function) -> Option<String> {
    use Function::*;
    match f {
        Str | Lang | LangMatches | Datatype | Iri | IsIri | IsBlank | IsLiteral | IsNumeric | StrLen | UCase | LCase
        | Contains | StrStarts | StrEnds | StrBefore | StrAfter | Concat | SubStr | Regex | Replace | Abs | Ceil
        | Floor | Round | Year | Month | Day | Hours | Minutes | Seconds | Timezone | Tz | Now | Rand | Uuid | StrUuid
        | BNode | Md5 | Sha1 | Sha256 | Sha384 | Sha512 | StrLang | StrDt | EncodeForUri => None,
        Custom(iri) => match iri.as_str().strip_prefix("http://www.w3.org/2001/XMLSchema#") {
            Some(t) if XSD_CASTS.contains(&t) => None,
            _ => Some(format!("function <{}> is not supported", iri.as_str())),
        },
        #[allow(unreachable_patterns)]
        other => Some(format!("function {other} is not supported")),
    }
}
