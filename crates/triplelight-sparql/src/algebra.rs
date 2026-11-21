//! SPARQL 1.1 algebra (W3C SPARQL 1.1 Query Language §18), produced by `parser` and evaluated by `exec`.
//!
//! The node names follow the specification's algebra operators. Terms come from `crate::rdf`.
use crate::rdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Variable};
use std::fmt;

/// A parsed query: one of the four query forms over a graph pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Select { dataset: Option<QueryDataset>, pattern: GraphPattern, base_iri: Option<String> },
    Construct { template: Vec<TriplePattern>, dataset: Option<QueryDataset>, pattern: GraphPattern, base_iri: Option<String> },
    Describe { dataset: Option<QueryDataset>, pattern: GraphPattern, base_iri: Option<String> },
    Ask { dataset: Option<QueryDataset>, pattern: GraphPattern, base_iri: Option<String> },
}

/// `FROM` / `FROM NAMED` (or `USING` / `USING NAMED` in updates).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct QueryDataset {
    pub default: Vec<NamedNode>,
    /// `None` when no `FROM NAMED` clause is given.
    pub named: Option<Vec<NamedNode>>,
}

/// A parsed update request: operations applied in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update { pub operations: Vec<GraphUpdateOperation>, pub base_iri: Option<String> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphUpdateOperation {
    InsertData { data: Vec<Quad> },
    DeleteData { data: Vec<GroundQuad> },
    /// `DELETE { … } INSERT { … } [USING …] WHERE { … }`, also `DELETE WHERE { … }` and `INSERT { … } WHERE { … }`.
    DeleteInsert {
        delete: Vec<GroundQuadPattern>,
        insert: Vec<QuadPattern>,
        /// `WITH <g>`: the graph the templates and WHERE clause default to.
        with: Option<NamedNode>,
        using: Option<QueryDataset>,
        pattern: Box<GraphPattern>,
    },
    /// `LOAD [SILENT] <source> [INTO GRAPH <g>]`.
    Load { silent: bool, source: NamedNode, destination: GraphName },
    /// `CLEAR [SILENT] (GRAPH <g> | DEFAULT | NAMED | ALL)`.
    Clear { silent: bool, graph: ClearTarget },
    /// `CREATE [SILENT] GRAPH <g>`.
    Create { silent: bool, graph: NamedNode },
    /// `DROP [SILENT] (GRAPH <g> | DEFAULT | NAMED | ALL)`.
    Drop { silent: bool, graph: ClearTarget },
    /// `ADD`, `MOVE` or `COPY [SILENT] (DEFAULT | [GRAPH] <g>) TO (DEFAULT | [GRAPH] <g>)`.
    Transfer { kind: TransferKind, silent: bool, from: GraphName, to: GraphName },
}

/// The graphs a `CLEAR` or `DROP` applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClearTarget { NamedNode(NamedNode), DefaultGraph, NamedGraphs, AllGraphs }

/// The kind of a graph-to-graph management operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind { Add, Move, Copy }

impl TransferKind {
    pub fn keyword(self) -> &'static str { match self { TransferKind::Add => "ADD", TransferKind::Move => "MOVE", TransferKind::Copy => "COPY" } }
}

/// A data quad of `INSERT DATA`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quad { pub subject: NamedOrBlankNode, pub predicate: NamedNode, pub object: Term, pub graph_name: GraphName }

/// A data quad of `DELETE DATA` (no blank nodes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundQuad { pub subject: NamedNode, pub predicate: NamedNode, pub object: GroundTerm, pub graph_name: GraphName }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphName { NamedNode(NamedNode), DefaultGraph }

/// An `INSERT` template quad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuadPattern { pub subject: TermPattern, pub predicate: NamedNodePattern, pub object: TermPattern, pub graph_name: GraphNamePattern }

/// A `DELETE` template quad (no blank nodes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundQuadPattern {
    pub subject: GroundTermPattern,
    pub predicate: NamedNodePattern,
    pub object: GroundTermPattern,
    pub graph_name: GraphNamePattern,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphNamePattern { NamedNode(NamedNode), DefaultGraph, Variable(Variable) }

/// A term of a triple pattern. Blank nodes in query patterns act as variables scoped to their basic graph pattern.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TermPattern { NamedNode(NamedNode), BlankNode(BlankNode), Literal(Literal), Variable(Variable) }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NamedNodePattern { NamedNode(NamedNode), Variable(Variable) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroundTermPattern { NamedNode(NamedNode), Literal(Literal), Variable(Variable) }

/// A constant that may appear in `VALUES` or `DELETE DATA`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GroundTerm { NamedNode(NamedNode), Literal(Literal) }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TriplePattern { pub subject: TermPattern, pub predicate: NamedNodePattern, pub object: TermPattern }

/// Graph patterns (§18.2). `Bgp { patterns: [] }` is the empty group, the identity of `Join`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GraphPattern {
    Bgp { patterns: Vec<TriplePattern> },
    /// A property path that is not a plain IRI, `^IRI` or a sequence of those (§18.2.2.4).
    Path { subject: TermPattern, path: PropertyPathExpression, object: TermPattern },
    Join { left: Box<GraphPattern>, right: Box<GraphPattern> },
    LeftJoin { left: Box<GraphPattern>, right: Box<GraphPattern>, expression: Option<Expression> },
    Filter { expr: Expression, inner: Box<GraphPattern> },
    Union { left: Box<GraphPattern>, right: Box<GraphPattern> },
    Graph { name: NamedNodePattern, inner: Box<GraphPattern> },
    Extend { inner: Box<GraphPattern>, variable: Variable, expression: Expression },
    Minus { left: Box<GraphPattern>, right: Box<GraphPattern> },
    Values { variables: Vec<Variable>, bindings: Vec<Vec<Option<GroundTerm>>> },
    OrderBy { inner: Box<GraphPattern>, expression: Vec<OrderExpression> },
    Project { inner: Box<GraphPattern>, variables: Vec<Variable> },
    Distinct { inner: Box<GraphPattern> },
    Reduced { inner: Box<GraphPattern> },
    Slice { inner: Box<GraphPattern>, start: usize, length: Option<usize> },
    /// `GROUP BY` keys (already bound by `Extend` when they were expressions) and the aggregates, each bound to a
    /// variable that the expressions above the group read.
    Group { inner: Box<GraphPattern>, variables: Vec<Variable>, aggregates: Vec<(Variable, AggregateExpression)> },
    Service { name: NamedNodePattern, inner: Box<GraphPattern>, silent: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PropertyPathExpression {
    NamedNode(NamedNode),
    Reverse(Box<PropertyPathExpression>),
    Sequence(Box<PropertyPathExpression>, Box<PropertyPathExpression>),
    Alternative(Box<PropertyPathExpression>, Box<PropertyPathExpression>),
    ZeroOrMore(Box<PropertyPathExpression>),
    OneOrMore(Box<PropertyPathExpression>),
    ZeroOrOne(Box<PropertyPathExpression>),
    /// `!(p1 | … | pn)` over forward links; inverse members become `^!(…)` in an `Alternative`.
    NegatedPropertySet(Vec<NamedNode>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expression {
    NamedNode(NamedNode),
    Literal(Literal),
    Variable(Variable),
    Or(Box<Expression>, Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Equal(Box<Expression>, Box<Expression>),
    SameTerm(Box<Expression>, Box<Expression>),
    Greater(Box<Expression>, Box<Expression>),
    GreaterOrEqual(Box<Expression>, Box<Expression>),
    Less(Box<Expression>, Box<Expression>),
    LessOrEqual(Box<Expression>, Box<Expression>),
    In(Box<Expression>, Vec<Expression>),
    Add(Box<Expression>, Box<Expression>),
    Subtract(Box<Expression>, Box<Expression>),
    Multiply(Box<Expression>, Box<Expression>),
    Divide(Box<Expression>, Box<Expression>),
    UnaryPlus(Box<Expression>),
    UnaryMinus(Box<Expression>),
    Not(Box<Expression>),
    Exists(Box<GraphPattern>),
    Bound(Variable),
    If(Box<Expression>, Box<Expression>, Box<Expression>),
    Coalesce(Vec<Expression>),
    FunctionCall(Function, Vec<Expression>),
}

impl Expression {
    /// Direct sub-expressions in source order (left operand first; for `IN`, the needle then the list).
    /// `EXISTS` has none here: its pattern is not an expression, and each caller treats it specially.
    pub fn children(&self) -> impl Iterator<Item = &Expression> {
        let (fixed, rest): ([Option<&Expression>; 3], &[Expression]) = match self {
            Expression::Or(a, b) | Expression::And(a, b) | Expression::Equal(a, b) | Expression::SameTerm(a, b)
            | Expression::Greater(a, b) | Expression::GreaterOrEqual(a, b) | Expression::Less(a, b) | Expression::LessOrEqual(a, b)
            | Expression::Add(a, b) | Expression::Subtract(a, b) | Expression::Multiply(a, b) | Expression::Divide(a, b) => ([Some(a), Some(b), None], &[]),
            Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => ([Some(a), None, None], &[]),
            Expression::In(a, l) => ([Some(a), None, None], l),
            Expression::If(a, b, c) => ([Some(a), Some(b), Some(c)], &[]),
            Expression::Coalesce(l) | Expression::FunctionCall(_, l) => ([None; 3], l),
            Expression::NamedNode(_) | Expression::Literal(_) | Expression::Variable(_) | Expression::Bound(_) | Expression::Exists(_) => ([None; 3], &[]),
        };
        fixed.into_iter().flatten().chain(rest.iter())
    }
}

/// Built-in functions (§17.4), plus calls to an IRI (`Custom`: XSD casts or extension functions).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Function {
    Str, Lang, LangMatches, Datatype, Iri, BNode, Rand, Abs, Ceil, Floor, Round, Concat, SubStr, StrLen, Replace,
    UCase, LCase, EncodeForUri, Contains, StrStarts, StrEnds, StrBefore, StrAfter, Year, Month, Day, Hours, Minutes,
    Seconds, Timezone, Tz, Now, Uuid, StrUuid, Md5, Sha1, Sha256, Sha384, Sha512, StrLang, StrDt, IsIri, IsBlank,
    IsLiteral, IsNumeric, Regex,
    Custom(NamedNode),
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Function::Str => "STR", Function::Lang => "LANG", Function::LangMatches => "LANGMATCHES",
            Function::Datatype => "DATATYPE", Function::Iri => "IRI", Function::BNode => "BNODE", Function::Rand => "RAND",
            Function::Abs => "ABS", Function::Ceil => "CEIL", Function::Floor => "FLOOR", Function::Round => "ROUND",
            Function::Concat => "CONCAT", Function::SubStr => "SUBSTR", Function::StrLen => "STRLEN",
            Function::Replace => "REPLACE", Function::UCase => "UCASE", Function::LCase => "LCASE",
            Function::EncodeForUri => "ENCODE_FOR_URI", Function::Contains => "CONTAINS", Function::StrStarts => "STRSTARTS",
            Function::StrEnds => "STRENDS", Function::StrBefore => "STRBEFORE", Function::StrAfter => "STRAFTER",
            Function::Year => "YEAR", Function::Month => "MONTH", Function::Day => "DAY", Function::Hours => "HOURS",
            Function::Minutes => "MINUTES", Function::Seconds => "SECONDS", Function::Timezone => "TIMEZONE",
            Function::Tz => "TZ", Function::Now => "NOW", Function::Uuid => "UUID", Function::StrUuid => "STRUUID",
            Function::Md5 => "MD5", Function::Sha1 => "SHA1", Function::Sha256 => "SHA256", Function::Sha384 => "SHA384",
            Function::Sha512 => "SHA512", Function::StrLang => "STRLANG", Function::StrDt => "STRDT",
            Function::IsIri => "isIRI", Function::IsBlank => "isBLANK", Function::IsLiteral => "isLITERAL",
            Function::IsNumeric => "isNUMERIC", Function::Regex => "REGEX",
            Function::Custom(iri) => return write!(f, "<{}>", iri.as_str()),
        };
        f.write_str(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AggregateExpression {
    /// `COUNT(*)` / `COUNT(DISTINCT *)`.
    CountSolutions { distinct: bool },
    FunctionCall { name: AggregateFunction, expr: Expression, distinct: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AggregateFunction {
    Count, Sum, Avg, Min, Max, Sample,
    /// `GROUP_CONCAT`; `None` separator means the default single space.
    GroupConcat { separator: Option<String> },
    Custom(NamedNode),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderExpression { Asc(Expression), Desc(Expression) }

#[cfg(test)]
mod children_tests {
    use super::*;
    fn v(n: &str) -> Expression { Expression::Variable(Variable::new_unchecked(n)) }
    fn names(e: &Expression) -> Vec<String> {
        e.children().map(|c| match c { Expression::Variable(x) => x.as_str().to_string(), _ => "?".into() }).collect()
    }
    #[test]
    fn children_in_source_order() {
        assert_eq!(names(&Expression::Less(Box::new(v("a")), Box::new(v("b")))), ["a", "b"]);
        assert_eq!(names(&Expression::Not(Box::new(v("a")))), ["a"]);
        assert_eq!(names(&Expression::In(Box::new(v("a")), vec![v("b"), v("c")])), ["a", "b", "c"]);
        assert_eq!(names(&Expression::If(Box::new(v("a")), Box::new(v("b")), Box::new(v("c")))), ["a", "b", "c"]);
        assert_eq!(names(&Expression::Coalesce(vec![v("a"), v("b")])), ["a", "b"]);
        assert!(v("a").children().next().is_none());
        assert!(Expression::Exists(Box::new(GraphPattern::Bgp { patterns: vec![] })).children().next().is_none());
    }
}
