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
            .or_else(|| expression.as_ref().and_then(|e| expr_unsupported(e, ds))),
        GraphPattern::Filter { expr, inner } => expr_unsupported(expr, ds).or_else(|| pattern_unsupported(inner, ds)),
        GraphPattern::Extend { inner, expression, .. } => pattern_unsupported(inner, ds).or_else(|| expr_unsupported(expression, ds)),
        GraphPattern::OrderBy { inner, expression } => pattern_unsupported(inner, ds).or_else(|| expression.iter().find_map(|o| match o {
            OrderExpression::Asc(e) | OrderExpression::Desc(e) => expr_unsupported(e, ds),
        })),
        GraphPattern::Group { inner, aggregates, .. } => pattern_unsupported(inner, ds).or_else(|| aggregates.iter().find_map(|(_, a)| match a {
            AggregateExpression::FunctionCall { expr, name, .. } => match name {
                crate::sparql::AggregateFunction::Custom(f) => Some(format!("custom aggregate <{}> is not supported", f.as_str())),
                _ => expr_unsupported(expr, ds),
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

/// `ds`: the query runs against a `DatasetView`, so GRAPH is evaluated, including inside EXISTS.
fn expr_unsupported(e: &Expression, ds: bool) -> Option<String> {
    match e {
        Expression::Exists(p) => pattern_unsupported(p, ds),
        Expression::FunctionCall(f, args) => function_unsupported(f).or_else(|| args.iter().find_map(|a| expr_unsupported(a, ds))),
        e => e.children().find_map(|c| expr_unsupported(c, ds)),
    }
}

fn function_unsupported(f: &Function) -> Option<String> {
    use Function::*;
    match f {
        Str | Lang | LangMatches | Datatype | Iri | IsIri | IsBlank | IsLiteral | IsNumeric | StrLen | UCase | LCase
        | Contains | StrStarts | StrEnds | StrBefore | StrAfter | Concat | SubStr | Regex | Replace | Abs | Ceil
        | Floor | Round | Year | Month | Day | Hours | Minutes | Seconds | Timezone | Tz | Now | Rand | Uuid | StrUuid
        | BNode | Md5 | Sha1 | Sha256 | Sha384 | Sha512 | StrLang | StrDt | EncodeForUri => None,
        // SPARQL 1.2 triple-term and base-direction functions (`expr::call`).
        #[cfg(feature = "rdf-12")]
        Triple | Subject | Predicate | Object | IsTriple | LangDir | HasLang | HasLangDir | StrLangDir => None,
        Custom(iri) => match iri.as_str().strip_prefix("http://www.w3.org/2001/XMLSchema#") {
            Some(t) if XSD_CASTS.contains(&t) => None,
            _ => Some(format!("function <{}> is not supported", iri.as_str())),
        },
    }
}
