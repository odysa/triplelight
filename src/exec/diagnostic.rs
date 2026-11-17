//! Explicit offline pattern-order comparisons. Not an HTTP request option.
//! The complete ordinary evaluator still handles filters, projection and modifiers.
use super::{
    Options, QueryError, QueryErrorKind, QueryResult, StorageFailure, bgp, run_query_controlled,
};
use crate::model::TermId;
use crate::storage::Snapshot;
use spargebra::Query;
use spargebra::algebra::{Expression, GraphPattern, OrderExpression};
use spargebra::term::{NamedNodePattern, TermPattern};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(super) struct OrderControl {
    pub forced: Option<Vec<usize>>,
    pub trace: Arc<Mutex<Vec<usize>>>,
}
#[derive(Debug)]
pub struct RecordedExecution {
    pub result: QueryResult,
    /// Actual ordinary-join steps, including only the prefix executed before an empty result.
    pub pattern_order: Vec<usize>,
}
fn exists(e: &Expression) -> bool {
    use Expression::*;
    match e {
        Exists(_) => true,
        Or(a, b)
        | And(a, b)
        | Equal(a, b)
        | SameTerm(a, b)
        | Greater(a, b)
        | GreaterOrEqual(a, b)
        | Less(a, b)
        | LessOrEqual(a, b)
        | Add(a, b)
        | Subtract(a, b)
        | Multiply(a, b)
        | Divide(a, b) => exists(a) || exists(b),
        UnaryPlus(a) | UnaryMinus(a) | Not(a) => exists(a),
        In(a, l) => exists(a) || l.iter().any(exists),
        If(a, b, c) => exists(a) || exists(b) || exists(c),
        Coalesce(l) | FunctionCall(_, l) => l.iter().any(exists),
        NamedNode(_) | Literal(_) | Variable(_) | Bound(_) => false,
    }
}
fn single_bgp(p: &GraphPattern) -> Result<&[spargebra::term::TriplePattern], QueryError> {
    use GraphPattern::*;
    match p {
        Bgp { patterns } => Ok(patterns),
        Filter { expr, inner } if !exists(expr) => single_bgp(inner),
        OrderBy { expression, inner }
            if expression.iter().all(|o| match o {
                OrderExpression::Asc(e) | OrderExpression::Desc(e) => !exists(e),
            }) =>
        {
            single_bgp(inner)
        }
        Project { inner, .. } | Distinct { inner } | Reduced { inner } | Slice { inner, .. } => {
            single_bgp(inner)
        }
        _ => Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "Diagnostic orders require one BGP without nested graph/subquery evaluation",
        )),
    }
}
fn acyclic(patterns: &[spargebra::term::TriplePattern]) -> bool {
    let mut variables = HashMap::<String, usize>::new();
    let mut variable = |name: String| {
        let next = variables.len();
        bgp::Slot::Var(*variables.entry(name).or_insert(next))
    };
    let mut slot = |p: &TermPattern| match p {
        TermPattern::Variable(v) => variable(v.as_str().to_string()),
        TermPattern::BlankNode(b) => variable(format!("_b{}", b.as_str())),
        _ => bgp::Slot::Const(Some(TermId(1))),
    };
    let mut rows = Vec::new();
    for p in patterns {
        let s = slot(&p.subject);
        let o = slot(&p.object);
        rows.push((s, o, &p.predicate));
    }
    drop(slot);
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(s, o, p)| {
            let p = match p {
                NamedNodePattern::Variable(v) => variable(v.as_str().to_owned()),
                _ => bgp::Slot::Const(Some(TermId(1))),
            };
            [s, p, o]
        })
        .collect();
    !bgp::cyclic(&rows)
}

/// Run an acyclic single-BGP SELECT with its normal pattern order (None),
/// or an explicitly supplied complete permutation. All original filters,
/// projection, DISTINCT, ORDER BY and LIMIT remain in the ordinary evaluator.
/// Default query_with does not enable this override or allocate a trace.
pub fn query_with_pattern_order<S: Snapshot>(
    snapshot: &S,
    text: &str,
    options: &Options<'_>,
    forced: Option<&[usize]>,
) -> Result<RecordedExecution, QueryError> {
    let query = spargebra::SparqlParser::new()
        .parse_query(text)
        .map_err(|e| QueryError::new(QueryErrorKind::Syntax, e.to_string()))?;
    let Query::Select {
        pattern, dataset, ..
    } = &query
    else {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "SELECT expected",
        ));
    };
    if dataset.is_some() {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "Diagnostic orders use one default-graph snapshot",
        ));
    }
    let patterns = single_bgp(pattern)?;
    if !acyclic(patterns) {
        return Err(QueryError::new(
            QueryErrorKind::Unsupported,
            "Forcing binary order on a cyclic BGP would change the runtime algorithm",
        ));
    }
    if let Some(order) = forced {
        if order.len() != patterns.len()
            || order.iter().any(|&i| i >= patterns.len())
            || order.iter().copied().collect::<HashSet<_>>().len() != patterns.len()
        {
            return Err(QueryError::new(
                QueryErrorKind::Unsupported,
                "Forced order must be a complete pattern permutation",
            ));
        }
    }
    let control = OrderControl {
        forced: forced.map(<[usize]>::to_vec),
        trace: Arc::new(Mutex::new(Vec::new())),
    };
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_query_controlled(snapshot, &query, options, Some(&control))
    })) {
        Ok(r) => r?,
        Err(p) => match p.downcast::<StorageFailure>() {
            Ok(error) => {
                return Err(QueryError::new(
                    QueryErrorKind::Storage,
                    format!("storage error: {}", error.0),
                ));
            }
            Err(p) => std::panic::resume_unwind(p),
        },
    };
    let pattern_order = control
        .trace
        .lock()
        .expect("diagnostic trace poisoned")
        .clone();
    Ok(RecordedExecution {
        result,
        pattern_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemorySnapshot;
    const QUERY: &str = "SELECT DISTINCT ?s WHERE { ?s <urn:p> ?x . ?s <urn:q> ?name FILTER(?x > 1) } ORDER BY ?s LIMIT 1";
    fn data() -> MemorySnapshot {
        MemorySnapshot::from_ntriples(b"<urn:a> <urn:p> \"0\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n<urn:a> <urn:q> \"zero\" .\n<urn:b> <urn:p> \"2\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n<urn:b> <urn:q> \"two\" .".as_slice()).unwrap()
    }
    #[test]
    fn both_orders_keep_filter_distinct_order_and_limit_semantics() {
        let snap = data();
        let options = Options::default();
        let normal = query_with_pattern_order(&snap, QUERY, &options, None).unwrap();
        assert_eq!(normal.pattern_order.len(), 2);
        let QueryResult::Solutions { rows: expected, .. } = normal.result else {
            panic!("solutions expected")
        };
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0][0].as_ref().unwrap().to_string(), "<urn:b>");
        for order in [[0, 1], [1, 0]] {
            let forced = query_with_pattern_order(&snap, QUERY, &options, Some(&order)).unwrap();
            assert_eq!(forced.pattern_order, order);
            let QueryResult::Solutions { rows, .. } = forced.result else {
                panic!("solutions expected")
            };
            assert_eq!(rows, expected);
        }
    }
    #[test]
    fn invalid_orders_cycles_and_expression_subqueries_fail_closed() {
        let snap = data();
        let options = Options::default();
        for order in [vec![0], vec![0, 0], vec![0, 2]] {
            assert_eq!(
                query_with_pattern_order(&snap, QUERY, &options, Some(&order))
                    .unwrap_err()
                    .kind,
                QueryErrorKind::Unsupported
            );
        }
        for query in [
            "SELECT * WHERE { ?a <urn:p> ?b . ?b <urn:q> ?c . ?c <urn:r> ?a }",
            "SELECT * WHERE { ?a <urn:p> ?b FILTER(EXISTS { ?a <urn:q> ?c }) }",
        ] {
            assert_eq!(
                query_with_pattern_order(&snap, query, &options, None)
                    .unwrap_err()
                    .kind,
                QueryErrorKind::Unsupported
            );
        }
    }
    struct BrokenCount(MemorySnapshot);
    impl crate::storage::Dictionary for BrokenCount {
        fn lookup(&self, t: &oxrdf::Term) -> std::io::Result<Option<TermId>> {
            self.0.lookup(t)
        }
        fn resolve(&self, id: TermId) -> std::io::Result<oxrdf::Term> {
            self.0.resolve(id)
        }
        fn with_term(
            &self,
            id: TermId,
            f: &mut dyn FnMut(oxrdf::TermRef<'_>),
        ) -> std::io::Result<()> {
            self.0.with_term(id, f)
        }
    }
    impl Snapshot for BrokenCount {
        fn scan<'a>(
            &'a self,
            p: crate::model::Perm,
            prefix: &[TermId],
        ) -> Box<dyn Iterator<Item = std::io::Result<crate::model::TripleBatch>> + 'a> {
            self.0.scan(p, prefix)
        }
        fn count(&self, _: crate::model::Perm, _: &[TermId]) -> std::io::Result<u64> {
            Err(std::io::Error::other("planted count failure"))
        }
        fn seek(
            &self,
            p: crate::model::Perm,
            prefix: &[TermId],
            target: TermId,
        ) -> std::io::Result<Option<TermId>> {
            self.0.seek(p, prefix, target)
        }
    }
    #[test]
    fn storage_failure_keeps_storage_error_class_in_diagnostic_path() {
        let error =
            query_with_pattern_order(&BrokenCount(data()), QUERY, &Options::default(), None)
                .unwrap_err();
        assert_eq!(error.kind, QueryErrorKind::Storage);
        assert!(error.message.contains("planted count failure"));
    }
}
