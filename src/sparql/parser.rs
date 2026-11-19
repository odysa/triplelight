//! Recursive-descent parser for SPARQL 1.1 queries and updates (grammar §19), translating directly to the algebra
//! of §18.2. Static checks required by the grammar notes (§19.8) are enforced here: blank-node label scope, BIND and
//! SELECT-expression variable reuse, grouping of projected variables, aggregate placement and DATA restrictions.
use super::algebra::*;
use super::lexer::{tokenize, Spanned, Tok};
use crate::rdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Variable};
use std::collections::{HashMap, HashSet};
use std::fmt;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// A syntax error with its 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError { pub message: String, pub line: usize, pub column: usize }

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "syntax error at line {}, column {}: {}", self.line, self.column, self.message)
    }
}
impl std::error::Error for ParseError {}

type R<T> = Result<T, ParseError>;

/// Parses a SPARQL 1.1 query. `base_iri` resolves relative IRIs when the query has no `BASE`.
pub fn parse_query(text: &str, base_iri: Option<&str>) -> R<Query> {
    let mut p = Parser::new(text, base_iri)?;
    p.prologue()?;
    let q = p.query()?;
    p.expect_eof()?;
    Ok(q)
}

/// Parses a SPARQL 1.1 update request.
pub fn parse_update(text: &str, base_iri: Option<&str>) -> R<Update> {
    let mut p = Parser::new(text, base_iri)?;
    let mut operations = Vec::new();
    loop {
        p.prologue()?;
        if p.at_eof() { break }
        operations.push(p.update_operation()?);
        if !p.eat_punct(";") { break }
    }
    p.expect_eof()?;
    Ok(Update { operations, base_iri: p.base.clone() })
}

/// One element of a group graph pattern, before translation (§18.2.2.5).
enum Elem {
    Triples(Vec<TriplePattern>),
    Path(TermPattern, PropertyPathExpression, TermPattern),
    Filter(Expression),
    Optional(GraphPattern),
    Minus(GraphPattern),
    Bind(Expression, Variable),
    Other(GraphPattern),
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Spanned>,
    i: usize,
    base: Option<String>,
    prefixes: HashMap<String, String>,
    /// Salt for generated blank nodes and variables, so they cannot collide with names written in the query.
    salt: String,
    counter: usize,
    /// Blank-node label -> the basic graph pattern (block number) it was first used in.
    bnode_block: HashMap<String, usize>,
    block: usize,
    /// Labels used by earlier `INSERT DATA` operations of the same request.
    data_bnodes: HashSet<String>,
    /// Parsing a CONSTRUCT or INSERT/DELETE template: its blank nodes are not pattern variables, so they are outside
    /// the basic-graph-pattern label scope check.
    template: bool,
    /// Aggregates collected while parsing SELECT / HAVING / ORDER BY expressions; `None` where aggregates are invalid.
    aggregates: Option<Vec<(Variable, AggregateExpression)>>,
    in_aggregate: bool,
}

fn xsd(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{XSD}{local}")) }
fn rdf(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{RDF}{local}")) }

impl<'a> Parser<'a> {
    fn new(src: &'a str, base: Option<&str>) -> R<Self> {
        let toks = tokenize(src).map_err(|e| Self::error_at(src, e.pos, e.message))?;
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(src.len());
        let salt = format!("{:012x}", h.finish() & 0xffff_ffff_ffff);
        if let Some(b) = base { if oxiri::Iri::parse(b.to_string()).is_err() { return Err(Self::error_at(src, 0, format!("invalid base IRI {b}"))) } }
        Ok(Parser {
            src, toks, i: 0, base: base.map(str::to_string), prefixes: HashMap::new(), salt, counter: 0,
            bnode_block: HashMap::new(), block: 0, data_bnodes: HashSet::new(), template: false, aggregates: None, in_aggregate: false,
        })
    }

    // ---------- token helpers ----------

    fn error_at(src: &str, pos: usize, message: impl Into<String>) -> ParseError {
        let before = &src[..pos.min(src.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        ParseError { message: message.into(), line, column }
    }
    fn err<T>(&self, message: impl Into<String>) -> R<T> { Err(Self::error_at(self.src, self.toks[self.i].pos, message)) }
    fn peek(&self) -> &Tok { &self.toks[self.i].tok }
    fn peek_n(&self, n: usize) -> &Tok { &self.toks[(self.i + n).min(self.toks.len() - 1)].tok }
    fn next(&mut self) -> Tok { let t = self.toks[self.i].tok.clone(); if self.i + 1 < self.toks.len() { self.i += 1 } t }
    fn at_eof(&self) -> bool { matches!(self.peek(), Tok::Eof) }
    fn expect_eof(&self) -> R<()> { if self.at_eof() { Ok(()) } else { self.err(format!("unexpected {}", self.peek())) } }
    fn is_punct(&self, p: &str) -> bool { matches!(self.peek(), Tok::Punct(x) if *x == p) }
    fn eat_punct(&mut self, p: &str) -> bool { if self.is_punct(p) { self.next(); true } else { false } }
    fn expect_punct(&mut self, p: &str) -> R<()> {
        if self.eat_punct(p) { Ok(()) } else { self.err(format!("expected '{p}', found {}", self.peek())) }
    }
    fn is_kw(&self, k: &str) -> bool { matches!(self.peek(), Tok::Name(n) if n.eq_ignore_ascii_case(k)) }
    fn is_kw_n(&self, n: usize, k: &str) -> bool { matches!(self.peek_n(n), Tok::Name(x) if x.eq_ignore_ascii_case(k)) }
    fn eat_kw(&mut self, k: &str) -> bool { if self.is_kw(k) { self.next(); true } else { false } }
    fn expect_kw(&mut self, k: &str) -> R<()> {
        if self.eat_kw(k) { Ok(()) } else { self.err(format!("expected {k}, found {}", self.peek())) }
    }

    fn fresh_label(&mut self) -> String { self.counter += 1; format!("tl{}x{}", self.salt, self.counter) }
    fn fresh_bnode(&mut self) -> BlankNode { BlankNode::new_unchecked(self.fresh_label()) }
    fn fresh_var(&mut self) -> Variable { Variable::new_unchecked(format!("_{}", self.fresh_label())) }

    // ---------- prologue and IRIs ----------

    fn prologue(&mut self) -> R<()> {
        loop {
            if self.eat_kw("BASE") {
                let Tok::Iri(i) = self.next() else { return self.err("expected an IRI after BASE") };
                let iri = self.resolve(&i)?;
                self.base = Some(iri.into_string());
            } else if self.eat_kw("PREFIX") {
                let Tok::PName(p, l) = self.next() else { return self.err("expected a prefix name after PREFIX") };
                if !l.is_empty() { return self.err("a prefix declaration must end with ':'") }
                let Tok::Iri(i) = self.next() else { return self.err("expected an IRI in PREFIX") };
                let iri = self.resolve(&i)?;
                self.prefixes.insert(p, iri.into_string());
            } else { return Ok(()) }
        }
    }

    fn resolve(&self, iri: &str) -> R<NamedNode> {
        let full = match &self.base {
            Some(b) => match oxiri::Iri::parse(b.as_str()).and_then(|b| b.resolve(iri)) {
                Ok(r) => r.into_inner(),
                Err(e) => return self.err(format!("invalid IRI <{iri}>: {e}")),
            },
            None => match oxiri::Iri::parse(iri.to_string()) {
                Ok(r) => r.into_inner(),
                Err(e) => return self.err(format!("invalid IRI <{iri}> (relative IRIs need a BASE): {e}")),
            },
        };
        Ok(NamedNode::new_unchecked(full))
    }

    fn pname(&self, p: &str, l: &str) -> R<NamedNode> {
        let Some(ns) = self.prefixes.get(p) else { return self.err(format!("undeclared prefix {p}:")) };
        let full = format!("{ns}{l}");
        match oxiri::Iri::parse(full.clone()) { Ok(_) => Ok(NamedNode::new_unchecked(full)), Err(e) => self.err(format!("invalid IRI {p}:{l}: {e}")) }
    }

    fn try_iri(&mut self) -> R<Option<NamedNode>> {
        match self.peek().clone() {
            Tok::Iri(i) => { self.next(); Ok(Some(self.resolve(&i)?)) }
            Tok::PName(p, l) => { self.next(); Ok(Some(self.pname(&p, &l)?)) }
            _ => Ok(None),
        }
    }
    fn iri(&mut self) -> R<NamedNode> {
        match self.try_iri()? { Some(n) => Ok(n), None => self.err(format!("expected an IRI, found {}", self.peek())) }
    }

    // ---------- literals ----------

    fn try_literal(&mut self) -> R<Option<Literal>> {
        let lit = match self.peek().clone() {
            Tok::Str(s) => {
                self.next();
                match self.peek().clone() {
                    Tok::LangTag(t) => {
                        self.next();
                        match Literal::new_language_tagged_literal(s, t.clone()) { Ok(l) => l, Err(_) => return self.err(format!("invalid language tag @{t}")) }
                    }
                    Tok::Punct("^^") => { self.next(); let dt = self.iri()?; Literal::new_typed_literal(s, dt) }
                    _ => Literal::new_simple_literal(s),
                }
            }
            Tok::Integer(n) => { self.next(); Literal::new_typed_literal(n, xsd("integer")) }
            Tok::Decimal(n) => { self.next(); Literal::new_typed_literal(n, xsd("decimal")) }
            Tok::Double(n) => { self.next(); Literal::new_typed_literal(n, xsd("double")) }
            Tok::Punct(sign @ ("+" | "-")) if matches!(self.peek_n(1), Tok::Integer(_) | Tok::Decimal(_) | Tok::Double(_)) => {
                self.next();
                let (lex, dt) = match self.next() {
                    Tok::Integer(n) => (n, "integer"), Tok::Decimal(n) => (n, "decimal"), Tok::Double(n) => (n, "double"),
                    _ => unreachable!(),
                };
                Literal::new_typed_literal(format!("{sign}{lex}"), xsd(dt))
            }
            Tok::Name(n) if n == "true" || n == "false" => { self.next(); Literal::new_typed_literal(n, xsd("boolean")) }
            _ => return Ok(None),
        };
        Ok(Some(lit))
    }

    fn var(&mut self) -> R<Variable> {
        match self.next() { Tok::Var(v) => Ok(Variable::new_unchecked(v)), t => { self.i -= 1; self.err(format!("expected a variable, found {t}")) } }
    }

    // ---------- query forms ----------

    fn query(&mut self) -> R<Query> {
        let base = |p: &Self| p.base.clone();
        if self.is_kw("SELECT") {
            let sel = self.select_clause()?;
            let dataset = self.dataset_clauses()?;
            let pattern = self.where_and_modifiers(Form::Select(sel))?;
            Ok(Query::Select { dataset, pattern, base_iri: base(self) })
        } else if self.eat_kw("CONSTRUCT") {
            if self.is_punct("{") {
                let template = self.construct_template()?;
                let dataset = self.dataset_clauses()?;
                let pattern = self.where_and_modifiers(Form::Star)?;
                Ok(Query::Construct { template, dataset, pattern, base_iri: base(self) })
            } else {
                // CONSTRUCT WHERE { triples }: the template is the pattern.
                let dataset = self.dataset_clauses()?;
                self.expect_kw("WHERE")?;
                self.expect_punct("{")?;
                self.block += 1;
                let template = if self.is_punct("}") { Vec::new() } else { self.triples_template()? };
                self.expect_punct("}")?;
                let inner = GraphPattern::Bgp { patterns: template.clone() };
                let pattern = self.modifiers(inner, Form::Star)?;
                Ok(Query::Construct { template, dataset, pattern, base_iri: base(self) })
            }
        } else if self.eat_kw("DESCRIBE") {
            let mut items = Vec::new();
            if !self.eat_punct("*") {
                loop {
                    if let Tok::Var(_) = self.peek() { items.push(NamedNodePattern::Variable(self.var()?)) }
                    else if let Some(i) = self.try_iri()? { items.push(NamedNodePattern::NamedNode(i)) }
                    else { break }
                }
                if items.is_empty() { return self.err("expected '*', a variable or an IRI after DESCRIBE") }
            }
            let dataset = self.dataset_clauses()?;
            let inner = if self.is_kw("WHERE") || self.is_punct("{") { self.where_clause()? } else { GraphPattern::Bgp { patterns: vec![] } };
            let form = if items.is_empty() { Form::Star } else { Form::Describe(items) };
            let pattern = self.modifiers(inner, form)?;
            Ok(Query::Describe { dataset, pattern, base_iri: base(self) })
        } else if self.eat_kw("ASK") {
            let dataset = self.dataset_clauses()?;
            let pattern = self.where_and_modifiers(Form::Ask)?;
            Ok(Query::Ask { dataset, pattern, base_iri: base(self) })
        } else {
            self.err(format!("expected SELECT, CONSTRUCT, DESCRIBE or ASK, found {}", self.peek()))
        }
    }

    fn dataset_clauses(&mut self) -> R<Option<QueryDataset>> {
        let mut ds: Option<QueryDataset> = None;
        while self.eat_kw("FROM") {
            let d = ds.get_or_insert_with(Default::default);
            if self.eat_kw("NAMED") { let g = self.iri()?; d.named.get_or_insert_with(Vec::new).push(g) } else { let g = self.iri()?; d.default.push(g) }
        }
        Ok(ds)
    }

    fn where_clause(&mut self) -> R<GraphPattern> {
        self.eat_kw("WHERE");
        self.group_graph_pattern()
    }

    fn where_and_modifiers(&mut self, form: Form) -> R<GraphPattern> {
        let inner = self.where_clause()?;
        self.modifiers(inner, form)
    }

    fn select_clause(&mut self) -> R<Select> {
        self.expect_kw("SELECT")?;
        let modifier = if self.eat_kw("DISTINCT") { 1 } else if self.eat_kw("REDUCED") { 2 } else { 0 };
        let mut items = Vec::new();
        if self.eat_punct("*") { return Ok(Select { modifier, items: None }) }
        // Aggregates found in SELECT expressions are collected for the GROUP built in `modifiers`.
        self.aggregates = Some(Vec::new());
        loop {
            match self.peek() {
                Tok::Var(_) => items.push(SelectItem::Var(self.var()?)),
                Tok::Punct("(") => {
                    self.next();
                    let e = self.expression()?;
                    self.expect_kw("AS")?;
                    let v = self.var()?;
                    self.expect_punct(")")?;
                    items.push(SelectItem::Expr(e, v));
                }
                _ => break,
            }
        }
        if items.is_empty() { return self.err("expected '*', variables or (expression AS ?var) after SELECT") }
        Ok(Select { modifier, items: Some(items) })
    }

    /// GROUP BY / HAVING / ORDER BY / LIMIT / OFFSET / VALUES, and the projection (§18.2.4, §18.2.5).
    fn modifiers(&mut self, mut p: GraphPattern, form: Form) -> R<GraphPattern> {
        let mut aggregates = self.aggregates.take().unwrap_or_default();
        // GROUP BY
        let mut keys: Option<Vec<Variable>> = None;
        if self.eat_kw("GROUP") {
            self.expect_kw("BY")?;
            let mut ks = Vec::new();
            loop {
                if let Tok::Var(_) = self.peek() { ks.push(self.var()?); continue }
                if self.is_punct("(") {
                    self.next();
                    let e = self.expression()?;
                    let v = if self.eat_kw("AS") { self.var()? } else { self.fresh_var() };
                    self.expect_punct(")")?;
                    p = GraphPattern::Extend { inner: Box::new(p), variable: v.clone(), expression: e };
                    ks.push(v);
                    continue;
                }
                match self.try_call_expression()? {
                    Some(e) => { let v = self.fresh_var(); p = GraphPattern::Extend { inner: Box::new(p), variable: v.clone(), expression: e }; ks.push(v) }
                    None => break,
                }
            }
            if ks.is_empty() { return self.err("expected a GROUP BY condition") }
            keys = Some(ks);
        }
        // HAVING (aggregates allowed)
        let mut having = Vec::new();
        self.aggregates = Some(std::mem::take(&mut aggregates));
        if self.eat_kw("HAVING") {
            loop {
                match self.try_constraint()? { Some(e) => having.push(e), None => break }
            }
            if having.is_empty() { return self.err("expected a HAVING condition") }
        }
        // ORDER BY (aggregates allowed)
        let mut order = Vec::new();
        if self.eat_kw("ORDER") {
            self.expect_kw("BY")?;
            loop {
                if self.eat_kw("ASC") { order.push(OrderExpression::Asc(self.bracketted()?)); continue }
                if self.eat_kw("DESC") { order.push(OrderExpression::Desc(self.bracketted()?)); continue }
                if let Tok::Var(_) = self.peek() { let v = self.var()?; order.push(OrderExpression::Asc(Expression::Variable(v))); continue }
                match self.try_constraint()? { Some(e) => order.push(OrderExpression::Asc(e)), None => break }
            }
            if order.is_empty() { return self.err("expected an ORDER BY condition") }
        }
        aggregates = self.aggregates.take().unwrap_or_default();
        // LIMIT / OFFSET
        let (mut limit, mut offset) = (None, None);
        for _ in 0..2 {
            if self.eat_kw("LIMIT") { if limit.is_some() { return self.err("duplicate LIMIT") } limit = Some(self.integer()?) }
            else if self.eat_kw("OFFSET") { if offset.is_some() { return self.err("duplicate OFFSET") } offset = Some(self.integer()?) }
        }
        // Trailing VALUES
        let values = if self.is_kw("VALUES") { Some(self.inline_data()?) } else { None };

        let select_items = match &form { Form::Select(s) => s.items.clone(), _ => None };
        let grouped = keys.is_some() || !aggregates.is_empty();
        if grouped {
            let keys = keys.unwrap_or_default();
            // Every variable read outside an aggregate must be a group key (or an earlier SELECT alias).
            let mut allowed: HashSet<Variable> = keys.iter().cloned().collect();
            allowed.extend(aggregates.iter().map(|(v, _)| v.clone()));
            // A trailing VALUES is joined after the grouping, so its variables are in scope too.
            if let Some(GraphPattern::Values { variables, .. }) = &values { allowed.extend(variables.iter().cloned()) }
            match (&form, &select_items) {
                (Form::Select(_), None) => return self.err("SELECT * cannot be used with GROUP BY or aggregates"),
                (_, Some(items)) => for it in items {
                    match it {
                        SelectItem::Var(v) => if !allowed.contains(v) { return self.err(format!("variable ?{} is projected but not grouped", v.as_str())) },
                        SelectItem::Expr(e, v) => {
                            let mut vs = Vec::new();
                            expr_vars(e, &mut vs);
                            if let Some(bad) = vs.iter().find(|x| !allowed.contains(*x)) {
                                return self.err(format!("variable ?{} is used in a SELECT expression but not grouped", bad.as_str()));
                            }
                            allowed.insert(v.clone());
                        }
                    }
                },
                _ => {}
            }
            p = GraphPattern::Group { inner: Box::new(p), variables: keys, aggregates };
        }
        if !having.is_empty() {
            let e = having.into_iter().reduce(|a, b| Expression::And(Box::new(a), Box::new(b))).unwrap();
            p = GraphPattern::Filter { expr: e, inner: Box::new(p) };
        }
        if let Some(v) = values { p = join(p, v) }
        // SELECT expressions and projection
        let projection: Option<Vec<Variable>> = match form {
            Form::Select(sel) => match sel.items {
                None => Some(in_scope(&p)),
                Some(items) => {
                    let mut seen: HashSet<Variable> = HashSet::new();
                    let mut vars = Vec::new();
                    for it in items {
                        match it {
                            SelectItem::Var(v) => { if !seen.insert(v.clone()) { return self.err(format!("variable ?{} is projected twice", v.as_str())) } vars.push(v) }
                            SelectItem::Expr(e, v) => {
                                if seen.contains(&v) || in_scope(&p).contains(&v) {
                                    return self.err(format!("variable ?{} is already in scope and cannot be bound by AS", v.as_str()));
                                }
                                seen.insert(v.clone());
                                p = GraphPattern::Extend { inner: Box::new(p), variable: v.clone(), expression: e };
                                vars.push(v);
                            }
                        }
                    }
                    Some(vars)
                }
            }.map(|vars| {
                let p2 = std::mem::replace(&mut p, GraphPattern::Bgp { patterns: vec![] });
                p = if order.is_empty() { p2 } else { GraphPattern::OrderBy { inner: Box::new(p2), expression: std::mem::take(&mut order) } };
                let distinct = sel.modifier;
                let mut q = GraphPattern::Project { inner: Box::new(std::mem::replace(&mut p, GraphPattern::Bgp { patterns: vec![] })), variables: vars.clone() };
                if distinct == 1 { q = GraphPattern::Distinct { inner: Box::new(q) } } else if distinct == 2 { q = GraphPattern::Reduced { inner: Box::new(q) } }
                p = q;
                vars
            }),
            Form::Describe(items) => {
                let mut vars = Vec::new();
                for it in items {
                    match it {
                        NamedNodePattern::Variable(v) => vars.push(v),
                        NamedNodePattern::NamedNode(n) => {
                            let v = self.fresh_var();
                            p = GraphPattern::Extend { inner: Box::new(p), variable: v.clone(), expression: Expression::NamedNode(n) };
                            vars.push(v);
                        }
                    }
                }
                if !order.is_empty() { p = GraphPattern::OrderBy { inner: Box::new(p), expression: std::mem::take(&mut order) } }
                p = GraphPattern::Project { inner: Box::new(p), variables: vars.clone() };
                Some(vars)
            }
            Form::Star | Form::Ask => None,
        };
        if projection.is_none() && !order.is_empty() { p = GraphPattern::OrderBy { inner: Box::new(p), expression: order } }
        if limit.is_some() || offset.is_some() {
            p = GraphPattern::Slice { inner: Box::new(p), start: offset.unwrap_or(0), length: limit };
        }
        Ok(p)
    }

    fn integer(&mut self) -> R<usize> {
        match self.next() { Tok::Integer(n) => n.parse().or_else(|_| self.err("integer too large")), t => { self.i -= 1; self.err(format!("expected an integer, found {t}")) } }
    }

    // ---------- group graph patterns ----------

    fn group_graph_pattern(&mut self) -> R<GraphPattern> {
        self.expect_punct("{")?;
        if self.is_kw("SELECT") {
            // Subquery: its own aggregates and solution modifiers; the outer collection state is saved around it.
            let saved = self.aggregates.take();
            let sel = self.select_clause()?;
            let inner = self.where_clause()?;
            let p = self.modifiers(inner, Form::Select(sel))?;
            self.aggregates = saved;
            self.expect_punct("}")?;
            return Ok(p);
        }
        // Aggregates are not allowed in WHERE; save the SELECT-level collection while inside.
        let saved = self.aggregates.take();
        self.block += 1;
        let mut elems: Vec<Elem> = Vec::new();
        loop {
            if self.eat_punct("}") { break }
            if self.is_kw("OPTIONAL") { self.next(); let g = self.group_graph_pattern()?; self.block += 1; elems.push(Elem::Optional(g)); }
            else if self.is_kw("MINUS") { self.next(); let g = self.group_graph_pattern()?; self.block += 1; elems.push(Elem::Minus(g)); }
            else if self.is_kw("GRAPH") {
                self.next();
                let name = self.var_or_iri()?;
                let g = self.group_graph_pattern()?;
                self.block += 1;
                elems.push(Elem::Other(GraphPattern::Graph { name, inner: Box::new(g) }));
            } else if self.is_kw("SERVICE") {
                self.next();
                let silent = self.eat_kw("SILENT");
                let name = self.var_or_iri()?;
                let g = self.group_graph_pattern()?;
                self.block += 1;
                elems.push(Elem::Other(GraphPattern::Service { name, inner: Box::new(g), silent }));
            } else if self.is_kw("FILTER") {
                self.next();
                match self.try_constraint()? { Some(e) => elems.push(Elem::Filter(e)), None => return self.err("expected a constraint after FILTER") }
            } else if self.is_kw("BIND") {
                self.next();
                self.expect_punct("(")?;
                let e = self.expression()?;
                self.expect_kw("AS")?;
                let v = self.var()?;
                self.expect_punct(")")?;
                self.block += 1;
                elems.push(Elem::Bind(e, v));
            } else if self.is_kw("VALUES") {
                let v = self.inline_data()?;
                self.block += 1;
                elems.push(Elem::Other(v));
            } else if self.is_punct("{") {
                let mut g = self.group_graph_pattern()?;
                while self.eat_kw("UNION") {
                    let r = self.group_graph_pattern()?;
                    g = GraphPattern::Union { left: Box::new(g), right: Box::new(r) };
                }
                self.block += 1;
                elems.push(Elem::Other(g));
            } else {
                // A triples block, possibly with property paths.
                let mut out = Vec::new();
                self.triples_same_subject_path(&mut out)?;
                elems.extend(out);
                if !self.eat_punct(".") && !self.is_punct("}") && !self.starts_not_triples() {
                    return self.err(format!("expected '.', '}}' or a graph pattern, found {}", self.peek()));
                }
                continue;
            }
            self.eat_punct(".");
        }
        self.aggregates = saved;
        self.translate_group(elems)
    }

    fn starts_not_triples(&self) -> bool {
        ["OPTIONAL", "MINUS", "GRAPH", "SERVICE", "FILTER", "BIND", "VALUES"].iter().any(|k| self.is_kw(k)) || self.is_punct("{")
    }

    /// §18.2.2.5: FILTERs apply to the whole group; OPTIONAL/MINUS/BIND fold left; triple patterns between other
    /// elements (FILTERs do not separate them) form one basic graph pattern.
    fn translate_group(&mut self, elems: Vec<Elem>) -> R<GraphPattern> {
        let mut g = GraphPattern::Bgp { patterns: vec![] };
        let mut filters = Vec::new();
        let mut bgp: Vec<TriplePattern> = Vec::new();
        let flush = |g: &mut GraphPattern, bgp: &mut Vec<TriplePattern>| {
            if !bgp.is_empty() {
                let b = GraphPattern::Bgp { patterns: std::mem::take(bgp) };
                *g = join(std::mem::replace(g, GraphPattern::Bgp { patterns: vec![] }), b);
            }
        };
        for e in elems {
            match e {
                Elem::Triples(ts) => bgp.extend(ts),
                Elem::Filter(f) => filters.push(f),
                Elem::Path(s, path, o) => {
                    flush(&mut g, &mut bgp);
                    g = join(g, GraphPattern::Path { subject: s, path, object: o });
                }
                Elem::Optional(a) => {
                    flush(&mut g, &mut bgp);
                    let (right, expression) = match a { GraphPattern::Filter { expr, inner } => (*inner, Some(expr)), a => (a, None) };
                    g = GraphPattern::LeftJoin { left: Box::new(g), right: Box::new(right), expression };
                }
                Elem::Minus(a) => { flush(&mut g, &mut bgp); g = GraphPattern::Minus { left: Box::new(g), right: Box::new(a) } }
                Elem::Bind(expr, v) => {
                    flush(&mut g, &mut bgp);
                    if in_scope(&g).contains(&v) {
                        return self.err(format!("BIND variable ?{} is already in scope", v.as_str()));
                    }
                    g = GraphPattern::Extend { inner: Box::new(g), variable: v, expression: expr };
                }
                Elem::Other(a) => { flush(&mut g, &mut bgp); g = join(g, a) }
            }
        }
        flush(&mut g, &mut bgp);
        if let Some(f) = filters.into_iter().reduce(|a, b| Expression::And(Box::new(a), Box::new(b))) {
            g = GraphPattern::Filter { expr: f, inner: Box::new(g) };
        }
        Ok(g)
    }

    fn var_or_iri(&mut self) -> R<NamedNodePattern> {
        if let Tok::Var(_) = self.peek() { return Ok(NamedNodePattern::Variable(self.var()?)) }
        Ok(NamedNodePattern::NamedNode(self.iri()?))
    }

    fn inline_data(&mut self) -> R<GraphPattern> {
        self.expect_kw("VALUES")?;
        let mut variables = Vec::new();
        let mut bindings = Vec::new();
        if let Tok::Var(_) = self.peek() {
            variables.push(self.var()?);
            self.expect_punct("{")?;
            while !self.eat_punct("}") { bindings.push(vec![self.data_block_value()?]) }
        } else {
            if !matches!(self.peek(), Tok::Nil) {
                self.expect_punct("(")?;
                while !self.eat_punct(")") { variables.push(self.var()?) }
            } else { self.next(); }
            self.expect_punct("{")?;
            while !self.eat_punct("}") {
                let mut row = Vec::new();
                if matches!(self.peek(), Tok::Nil) { self.next(); } else {
                    self.expect_punct("(")?;
                    while !self.eat_punct(")") { row.push(self.data_block_value()?) }
                }
                if row.len() != variables.len() { return self.err("VALUES row length differs from the number of variables") }
                bindings.push(row);
            }
        }
        Ok(GraphPattern::Values { variables, bindings })
    }

    fn data_block_value(&mut self) -> R<Option<GroundTerm>> {
        if self.eat_kw("UNDEF") { return Ok(None) }
        if let Some(i) = self.try_iri()? { return Ok(Some(GroundTerm::NamedNode(i))) }
        if let Some(l) = self.try_literal()? { return Ok(Some(GroundTerm::Literal(l))) }
        self.err(format!("expected a VALUES term, found {}", self.peek()))
    }

    // ---------- triples ----------

    fn note_bnode(&mut self, label: &str) -> R<()> {
        if self.template { return Ok(()) }
        match self.bnode_block.get(label) {
            Some(&b) if b != self.block => self.err(format!("blank node label _:{label} is used in two different graph patterns")),
            Some(_) => Ok(()),
            None => { self.bnode_block.insert(label.to_string(), self.block); Ok(()) }
        }
    }

    /// VarOrTerm (§19.8 [106]) as a pattern term; `[]`, `[...]` and collections are handled by the callers.
    fn try_var_or_term(&mut self) -> R<Option<TermPattern>> {
        match self.peek().clone() {
            Tok::Var(v) => { self.next(); Ok(Some(TermPattern::Variable(Variable::new_unchecked(v)))) }
            Tok::BNode(b) => { self.next(); self.note_bnode(&b)?; Ok(Some(TermPattern::BlankNode(BlankNode::new_unchecked(b)))) }
            Tok::Anon => { self.next(); Ok(Some(TermPattern::BlankNode(self.fresh_bnode()))) }
            Tok::Nil => { self.next(); Ok(Some(TermPattern::NamedNode(rdf("nil")))) }
            _ => {
                if let Some(i) = self.try_iri()? { return Ok(Some(TermPattern::NamedNode(i))) }
                if let Some(l) = self.try_literal()? { return Ok(Some(TermPattern::Literal(l))) }
                Ok(None)
            }
        }
    }

    /// A graph node: a term, a `[ property list ]` or a collection `( … )`. Generated triples go to `out`.
    fn graph_node(&mut self, out: &mut Vec<Elem>, paths: bool) -> R<TermPattern> {
        if let Some(t) = self.try_var_or_term()? { return Ok(t) }
        if self.eat_punct("[") {
            let b = TermPattern::BlankNode(self.fresh_bnode());
            self.property_list_not_empty(&b, out, paths)?;
            self.expect_punct("]")?;
            return Ok(b);
        }
        if self.eat_punct("(") {
            let mut items = Vec::new();
            while !self.eat_punct(")") { items.push(self.graph_node(out, paths)?) }
            if items.is_empty() { return self.err("empty collection") }
            let head = TermPattern::BlankNode(self.fresh_bnode());
            let mut cur = head.clone();
            let n = items.len();
            for (k, item) in items.into_iter().enumerate() {
                out.push(Elem::Triples(vec![TriplePattern { subject: cur.clone(), predicate: NamedNodePattern::NamedNode(rdf("first")), object: item }]));
                let next = if k + 1 == n { TermPattern::NamedNode(rdf("nil")) } else { TermPattern::BlankNode(self.fresh_bnode()) };
                out.push(Elem::Triples(vec![TriplePattern { subject: cur.clone(), predicate: NamedNodePattern::NamedNode(rdf("rest")), object: next.clone() }]));
                cur = next;
            }
            return Ok(head);
        }
        self.err(format!("expected an RDF term, found {}", self.peek()))
    }

    fn triples_same_subject_path(&mut self, out: &mut Vec<Elem>) -> R<()> {
        let starts_node = matches!(self.peek(), Tok::Punct("[") | Tok::Punct("("));
        let s = self.graph_node(out, true)?;
        if starts_node && (self.is_punct(".") || self.is_punct("}") || self.starts_not_triples()) { return Ok(()) }
        self.property_list_not_empty(&s, out, true)
    }

    fn property_list_not_empty(&mut self, s: &TermPattern, out: &mut Vec<Elem>, paths: bool) -> R<()> {
        loop {
            let verb = if let Tok::Var(_) = self.peek() { Verb::Var(self.var()?) }
                else if paths { Verb::Path(self.path()?) }
                else if self.eat_kw_a() { Verb::Path(PropertyPathExpression::NamedNode(rdf("type"))) }
                else { Verb::Path(PropertyPathExpression::NamedNode(self.iri()?)) };
            loop {
                let o = self.graph_node(out, paths)?;
                match &verb {
                    Verb::Var(v) => out.push(Elem::Triples(vec![TriplePattern { subject: s.clone(), predicate: NamedNodePattern::Variable(v.clone()), object: o }])),
                    Verb::Path(p) => self.translate_path(s.clone(), p.clone(), o, out),
                }
                if !self.eat_punct(",") { break }
            }
            if !self.eat_punct(";") { return Ok(()) }
            while self.eat_punct(";") {}
            if !(matches!(self.peek(), Tok::Var(_) | Tok::Iri(_) | Tok::PName(..)) || self.is_kw("a")
                || (paths && (self.is_punct("^") || self.is_punct("!") || self.is_punct("(")))) { return Ok(()) }
        }
    }

    fn eat_kw_a(&mut self) -> bool { if matches!(self.peek(), Tok::Name(n) if n == "a") { self.next(); true } else { false } }

    /// §18.2.2.4: IRIs, `^IRI` and sequences become triple patterns; other paths stay `Path`.
    fn translate_path(&mut self, s: TermPattern, p: PropertyPathExpression, o: TermPattern, out: &mut Vec<Elem>) {
        match p {
            PropertyPathExpression::NamedNode(n) => out.push(Elem::Triples(vec![TriplePattern { subject: s, predicate: NamedNodePattern::NamedNode(n), object: o }])),
            PropertyPathExpression::Reverse(q) if matches!(*q, PropertyPathExpression::NamedNode(_)) => self.translate_path(o, *q, s, out),
            PropertyPathExpression::Sequence(a, b) => {
                let mid = TermPattern::BlankNode(self.fresh_bnode());
                self.translate_path(s, *a, mid.clone(), out);
                self.translate_path(mid, *b, o, out);
            }
            p => out.push(Elem::Path(s, p, o)),
        }
    }

    fn path(&mut self) -> R<PropertyPathExpression> {
        let mut p = self.path_sequence()?;
        while self.eat_punct("|") { p = PropertyPathExpression::Alternative(Box::new(p), Box::new(self.path_sequence()?)) }
        Ok(p)
    }
    fn path_sequence(&mut self) -> R<PropertyPathExpression> {
        let mut p = self.path_elt_or_inverse()?;
        while self.eat_punct("/") { p = PropertyPathExpression::Sequence(Box::new(p), Box::new(self.path_elt_or_inverse()?)) }
        Ok(p)
    }
    fn path_elt_or_inverse(&mut self) -> R<PropertyPathExpression> {
        if self.eat_punct("^") { return Ok(PropertyPathExpression::Reverse(Box::new(self.path_elt()?))) }
        self.path_elt()
    }
    fn path_elt(&mut self) -> R<PropertyPathExpression> {
        let p = self.path_primary()?;
        Ok(if self.eat_punct("?") { PropertyPathExpression::ZeroOrOne(Box::new(p)) }
            else if self.eat_punct("*") { PropertyPathExpression::ZeroOrMore(Box::new(p)) }
            else if self.eat_punct("+") { PropertyPathExpression::OneOrMore(Box::new(p)) }
            else { p })
    }
    fn path_primary(&mut self) -> R<PropertyPathExpression> {
        if self.eat_kw_a() { return Ok(PropertyPathExpression::NamedNode(rdf("type"))) }
        if self.eat_punct("!") {
            let mut fwd = Vec::new();
            let mut inv = Vec::new();
            let mut one = |p: &mut Self| -> R<()> {
                let reverse = p.eat_punct("^");
                let iri = if p.eat_kw_a() { rdf("type") } else { p.iri()? };
                if reverse { inv.push(iri) } else { fwd.push(iri) }
                Ok(())
            };
            if matches!(self.peek(), Tok::Nil) { self.next(); }
            else if self.eat_punct("(") {
                one(self)?;
                while self.eat_punct("|") { one(self)? }
                self.expect_punct(")")?;
            } else { one(self)? }
            return Ok(match (fwd.is_empty(), inv.is_empty()) {
                (_, true) => PropertyPathExpression::NegatedPropertySet(fwd),
                (true, false) => PropertyPathExpression::Reverse(Box::new(PropertyPathExpression::NegatedPropertySet(inv))),
                (false, false) => PropertyPathExpression::Alternative(
                    Box::new(PropertyPathExpression::NegatedPropertySet(fwd)),
                    Box::new(PropertyPathExpression::Reverse(Box::new(PropertyPathExpression::NegatedPropertySet(inv))))),
            });
        }
        if self.eat_punct("(") { let p = self.path()?; self.expect_punct(")")?; return Ok(p) }
        Ok(PropertyPathExpression::NamedNode(self.iri()?))
    }

    /// Triples without paths (CONSTRUCT templates, update templates).
    fn triples_template(&mut self) -> R<Vec<TriplePattern>> {
        let mut out = Vec::new();
        loop {
            if self.is_punct("}") { break }
            let starts_node = matches!(self.peek(), Tok::Punct("[") | Tok::Punct("("));
            let s = self.graph_node(&mut out, false)?;
            if !(starts_node && (self.is_punct(".") || self.is_punct("}"))) { self.property_list_not_empty(&s, &mut out, false)?; }
            if !self.eat_punct(".") { break }
        }
        let mut triples = Vec::new();
        for e in out { if let Elem::Triples(t) = e { triples.extend(t) } }
        Ok(triples)
    }

    fn construct_template(&mut self) -> R<Vec<TriplePattern>> {
        self.expect_punct("{")?;
        self.template = true;
        let t = self.triples_template();
        self.template = false;
        let t = t?;
        self.expect_punct("}")?;
        Ok(t)
    }

    // ---------- expressions ----------

    fn bracketted(&mut self) -> R<Expression> {
        self.expect_punct("(")?;
        let e = self.expression()?;
        self.expect_punct(")")?;
        Ok(e)
    }

    /// Constraint: a bracketted expression, a built-in call or a function call.
    fn try_constraint(&mut self) -> R<Option<Expression>> {
        if self.is_punct("(") { return Ok(Some(self.bracketted()?)) }
        self.try_call_expression()
    }

    fn try_call_expression(&mut self) -> R<Option<Expression>> {
        if let Tok::Name(_) = self.peek() { if let Some(e) = self.try_builtin()? { return Ok(Some(e)) } }
        if matches!(self.peek(), Tok::Iri(_) | Tok::PName(..)) {
            let iri = self.iri()?;
            return Ok(Some(self.function_call(iri)?));
        }
        Ok(None)
    }

    fn expression(&mut self) -> R<Expression> {
        let mut e = self.and_expression()?;
        while self.eat_punct("||") { e = Expression::Or(Box::new(e), Box::new(self.and_expression()?)) }
        Ok(e)
    }
    fn and_expression(&mut self) -> R<Expression> {
        let mut e = self.relational()?;
        while self.eat_punct("&&") { e = Expression::And(Box::new(e), Box::new(self.relational()?)) }
        Ok(e)
    }
    fn relational(&mut self) -> R<Expression> {
        let a = self.additive()?;
        let b = |p: &mut Self| -> R<Box<Expression>> { Ok(Box::new(p.additive()?)) };
        Ok(match self.peek() {
            Tok::Punct("=") => { self.next(); Expression::Equal(Box::new(a), b(self)?) }
            Tok::Punct("!=") => { self.next(); Expression::Not(Box::new(Expression::Equal(Box::new(a), b(self)?))) }
            Tok::Punct("<") => { self.next(); Expression::Less(Box::new(a), b(self)?) }
            Tok::Punct(">") => { self.next(); Expression::Greater(Box::new(a), b(self)?) }
            Tok::Punct("<=") => { self.next(); Expression::LessOrEqual(Box::new(a), b(self)?) }
            Tok::Punct(">=") => { self.next(); Expression::GreaterOrEqual(Box::new(a), b(self)?) }
            _ if self.is_kw("IN") => { self.next(); Expression::In(Box::new(a), self.expression_list()?) }
            _ if self.is_kw("NOT") && self.is_kw_n(1, "IN") => {
                self.next(); self.next();
                Expression::Not(Box::new(Expression::In(Box::new(a), self.expression_list()?)))
            }
            _ => a,
        })
    }
    fn additive(&mut self) -> R<Expression> {
        let mut e = self.multiplicative()?;
        loop {
            if self.eat_punct("+") { e = Expression::Add(Box::new(e), Box::new(self.multiplicative()?)) }
            else if self.eat_punct("-") { e = Expression::Subtract(Box::new(e), Box::new(self.multiplicative()?)) }
            else { return Ok(e) }
        }
    }
    fn multiplicative(&mut self) -> R<Expression> {
        let mut e = self.unary()?;
        loop {
            if self.eat_punct("*") { e = Expression::Multiply(Box::new(e), Box::new(self.unary()?)) }
            else if self.eat_punct("/") { e = Expression::Divide(Box::new(e), Box::new(self.unary()?)) }
            else { return Ok(e) }
        }
    }
    fn unary(&mut self) -> R<Expression> {
        if self.eat_punct("!") { return Ok(Expression::Not(Box::new(self.primary()?))) }
        if self.eat_punct("+") { return Ok(Expression::UnaryPlus(Box::new(self.primary()?))) }
        if self.eat_punct("-") { return Ok(Expression::UnaryMinus(Box::new(self.primary()?))) }
        self.primary()
    }
    fn primary(&mut self) -> R<Expression> {
        if self.is_punct("(") { return self.bracketted() }
        if let Tok::Var(_) = self.peek() { return Ok(Expression::Variable(self.var()?)) }
        if let Some(l) = self.try_literal()? { return Ok(Expression::Literal(l)) }
        if matches!(self.peek(), Tok::Iri(_) | Tok::PName(..)) {
            let iri = self.iri()?;
            if self.is_punct("(") || matches!(self.peek(), Tok::Nil) { return self.function_call(iri) }
            return Ok(Expression::NamedNode(iri));
        }
        if let Some(e) = self.try_builtin()? { return Ok(e) }
        self.err(format!("expected an expression, found {}", self.peek()))
    }

    fn expression_list(&mut self) -> R<Vec<Expression>> {
        if matches!(self.peek(), Tok::Nil) { self.next(); return Ok(Vec::new()) }
        self.expect_punct("(")?;
        let mut v = vec![self.expression()?];
        while self.eat_punct(",") { v.push(self.expression()?) }
        self.expect_punct(")")?;
        Ok(v)
    }

    /// `iri(args)`: an extension function, or a custom aggregate when written with DISTINCT in an aggregate context.
    fn function_call(&mut self, iri: NamedNode) -> R<Expression> {
        if matches!(self.peek(), Tok::Nil) { self.next(); return Ok(Expression::FunctionCall(Function::Custom(iri), vec![])) }
        self.expect_punct("(")?;
        if self.eat_kw("DISTINCT") {
            let e = self.expression()?;
            self.expect_punct(")")?;
            return self.register_aggregate(AggregateExpression::FunctionCall { name: AggregateFunction::Custom(iri), expr: e, distinct: true });
        }
        let mut args = vec![self.expression()?];
        while self.eat_punct(",") { args.push(self.expression()?) }
        self.expect_punct(")")?;
        Ok(Expression::FunctionCall(Function::Custom(iri), args))
    }

    fn register_aggregate(&mut self, a: AggregateExpression) -> R<Expression> {
        if self.aggregates.is_none() { return self.err("aggregates are only allowed in SELECT, HAVING and ORDER BY") }
        let v = self.fresh_var();
        self.aggregates.as_mut().unwrap().push((v.clone(), a));
        Ok(Expression::Variable(v))
    }

    fn aggregate(&mut self, name: &str) -> R<Expression> {
        if self.in_aggregate { return self.err("aggregates cannot be nested") }
        if self.aggregates.is_none() { return self.err("aggregates are only allowed in SELECT, HAVING and ORDER BY") }
        self.expect_punct("(")?;
        let distinct = self.eat_kw("DISTINCT");
        self.in_aggregate = true;
        let saved = self.aggregates.take();
        let result = (|| -> R<AggregateExpression> {
            if name == "COUNT" && self.eat_punct("*") { return Ok(AggregateExpression::CountSolutions { distinct }) }
            let expr = self.expression()?;
            let func = match name {
                "COUNT" => AggregateFunction::Count, "SUM" => AggregateFunction::Sum, "AVG" => AggregateFunction::Avg,
                "MIN" => AggregateFunction::Min, "MAX" => AggregateFunction::Max, "SAMPLE" => AggregateFunction::Sample,
                _ => {
                    let mut separator = None;
                    if self.eat_punct(";") {
                        self.expect_kw("SEPARATOR")?;
                        self.expect_punct("=")?;
                        match self.next() { Tok::Str(s) => separator = Some(s), _ => return self.err("expected a string SEPARATOR") }
                    }
                    AggregateFunction::GroupConcat { separator }
                }
            };
            Ok(AggregateExpression::FunctionCall { name: func, expr, distinct })
        })();
        self.aggregates = saved;
        self.in_aggregate = false;
        let a = result?;
        self.expect_punct(")")?;
        self.register_aggregate(a)
    }

    fn try_builtin(&mut self) -> R<Option<Expression>> {
        let Tok::Name(n) = self.peek().clone() else { return Ok(None) };
        let up = n.to_ascii_uppercase();
        if matches!(up.as_str(), "COUNT" | "SUM" | "MIN" | "MAX" | "AVG" | "SAMPLE" | "GROUP_CONCAT") {
            self.next();
            return Ok(Some(self.aggregate(&up)?));
        }
        let (f, arity): (Function, (usize, usize)) = match up.as_str() {
            "STR" => (Function::Str, (1, 1)), "LANG" => (Function::Lang, (1, 1)), "LANGMATCHES" => (Function::LangMatches, (2, 2)),
            "DATATYPE" => (Function::Datatype, (1, 1)), "IRI" | "URI" => (Function::Iri, (1, 1)), "BNODE" => (Function::BNode, (0, 1)),
            "RAND" => (Function::Rand, (0, 0)), "ABS" => (Function::Abs, (1, 1)), "CEIL" => (Function::Ceil, (1, 1)),
            "FLOOR" => (Function::Floor, (1, 1)), "ROUND" => (Function::Round, (1, 1)), "CONCAT" => (Function::Concat, (0, usize::MAX)),
            "SUBSTR" => (Function::SubStr, (2, 3)), "STRLEN" => (Function::StrLen, (1, 1)), "REPLACE" => (Function::Replace, (3, 4)),
            "UCASE" => (Function::UCase, (1, 1)), "LCASE" => (Function::LCase, (1, 1)), "ENCODE_FOR_URI" => (Function::EncodeForUri, (1, 1)),
            "CONTAINS" => (Function::Contains, (2, 2)), "STRSTARTS" => (Function::StrStarts, (2, 2)), "STRENDS" => (Function::StrEnds, (2, 2)),
            "STRBEFORE" => (Function::StrBefore, (2, 2)), "STRAFTER" => (Function::StrAfter, (2, 2)), "YEAR" => (Function::Year, (1, 1)),
            "MONTH" => (Function::Month, (1, 1)), "DAY" => (Function::Day, (1, 1)), "HOURS" => (Function::Hours, (1, 1)),
            "MINUTES" => (Function::Minutes, (1, 1)), "SECONDS" => (Function::Seconds, (1, 1)), "TIMEZONE" => (Function::Timezone, (1, 1)),
            "TZ" => (Function::Tz, (1, 1)), "NOW" => (Function::Now, (0, 0)), "UUID" => (Function::Uuid, (0, 0)),
            "STRUUID" => (Function::StrUuid, (0, 0)), "MD5" => (Function::Md5, (1, 1)), "SHA1" => (Function::Sha1, (1, 1)),
            "SHA256" => (Function::Sha256, (1, 1)), "SHA384" => (Function::Sha384, (1, 1)), "SHA512" => (Function::Sha512, (1, 1)),
            "STRLANG" => (Function::StrLang, (2, 2)), "STRDT" => (Function::StrDt, (2, 2)), "ISIRI" | "ISURI" => (Function::IsIri, (1, 1)),
            "ISBLANK" => (Function::IsBlank, (1, 1)), "ISLITERAL" => (Function::IsLiteral, (1, 1)), "ISNUMERIC" => (Function::IsNumeric, (1, 1)),
            "REGEX" => (Function::Regex, (2, 3)),
            "BOUND" => {
                self.next();
                self.expect_punct("(")?;
                let v = self.var()?;
                self.expect_punct(")")?;
                return Ok(Some(Expression::Bound(v)));
            }
            "COALESCE" => { self.next(); return Ok(Some(Expression::Coalesce(self.expression_list()?))) }
            "IF" => {
                self.next();
                let a = self.call_args(3, 3)?;
                let mut it = a.into_iter();
                let (x, y, z) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
                return Ok(Some(Expression::If(Box::new(x), Box::new(y), Box::new(z))));
            }
            "SAMETERM" => {
                self.next();
                let a = self.call_args(2, 2)?;
                let mut it = a.into_iter();
                return Ok(Some(Expression::SameTerm(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))));
            }
            "EXISTS" => { self.next(); return Ok(Some(Expression::Exists(Box::new(self.exists_pattern()?)))) }
            "NOT" if self.is_kw_n(1, "EXISTS") => {
                self.next(); self.next();
                return Ok(Some(Expression::Not(Box::new(Expression::Exists(Box::new(self.exists_pattern()?))))));
            }
            _ => return Ok(None),
        };
        self.next();
        let args = self.call_args(arity.0, arity.1)?;
        Ok(Some(Expression::FunctionCall(f, args)))
    }

    /// EXISTS { … }: a group pattern in its own blank-node scope, where aggregates are not allowed.
    fn exists_pattern(&mut self) -> R<GraphPattern> {
        let saved = self.aggregates.take();
        let g = self.group_graph_pattern();
        self.aggregates = saved;
        g
    }

    fn call_args(&mut self, min: usize, max: usize) -> R<Vec<Expression>> {
        let args = self.expression_list()?;
        if args.len() < min || args.len() > max { return self.err(format!("wrong number of arguments ({})", args.len())) }
        Ok(args)
    }

    // ---------- updates ----------

    fn update_operation(&mut self) -> R<GraphUpdateOperation> {
        for (kw, name) in [("LOAD", "LOAD"), ("CLEAR", "CLEAR"), ("DROP", "DROP"), ("CREATE", "CREATE"), ("ADD", "ADD"), ("MOVE", "MOVE"), ("COPY", "COPY")] {
            if self.eat_kw(kw) {
                self.eat_kw("SILENT");
                match name {
                    "LOAD" => { self.iri()?; if self.eat_kw("INTO") { self.expect_kw("GRAPH")?; self.iri()?; } }
                    "CLEAR" | "DROP" => {
                        if !(self.eat_kw("DEFAULT") || self.eat_kw("NAMED") || self.eat_kw("ALL")) { self.expect_kw("GRAPH")?; self.iri()?; }
                    }
                    "CREATE" => { self.expect_kw("GRAPH")?; self.iri()?; }
                    _ => {
                        let graph_or_default = |p: &mut Self| -> R<()> { if !p.eat_kw("DEFAULT") { p.eat_kw("GRAPH"); p.iri()?; } Ok(()) };
                        graph_or_default(self)?;
                        self.expect_kw("TO")?;
                        graph_or_default(self)?;
                    }
                }
                return Ok(GraphUpdateOperation::Management { operation: name });
            }
        }
        if self.is_kw("INSERT") && self.is_kw_n(1, "DATA") {
            self.next(); self.next();
            let quads = self.quads(QuadMode::InsertData)?;
            let mut data = Vec::new();
            for (s, p, o, g) in quads {
                let subject = match s { TermPattern::NamedNode(n) => NamedOrBlankNode::NamedNode(n), TermPattern::BlankNode(b) => NamedOrBlankNode::BlankNode(b), _ => return self.err("invalid subject in INSERT DATA") };
                let NamedNodePattern::NamedNode(predicate) = p else { return self.err("variables are not allowed in INSERT DATA") };
                let object: Term = match o { TermPattern::NamedNode(n) => n.into(), TermPattern::BlankNode(b) => b.into(), TermPattern::Literal(l) => l.into(), TermPattern::Variable(_) => return self.err("variables are not allowed in INSERT DATA") };
                let graph_name = match g { GraphNamePattern::NamedNode(n) => GraphName::NamedNode(n), GraphNamePattern::DefaultGraph => GraphName::DefaultGraph, GraphNamePattern::Variable(_) => return self.err("variables are not allowed in INSERT DATA") };
                data.push(Quad { subject, predicate, object, graph_name });
            }
            return Ok(GraphUpdateOperation::InsertData { data });
        }
        if self.is_kw("DELETE") && self.is_kw_n(1, "DATA") {
            self.next(); self.next();
            let quads = self.quads(QuadMode::DeleteData)?;
            let mut data = Vec::new();
            for (s, p, o, g) in quads {
                let TermPattern::NamedNode(subject) = s else { return self.err("invalid subject in DELETE DATA") };
                let NamedNodePattern::NamedNode(predicate) = p else { return self.err("variables are not allowed in DELETE DATA") };
                let object = match o { TermPattern::NamedNode(n) => GroundTerm::NamedNode(n), TermPattern::Literal(l) => GroundTerm::Literal(l), _ => return self.err("blank nodes and variables are not allowed in DELETE DATA") };
                let graph_name = match g { GraphNamePattern::NamedNode(n) => GraphName::NamedNode(n), GraphNamePattern::DefaultGraph => GraphName::DefaultGraph, GraphNamePattern::Variable(_) => return self.err("variables are not allowed in DELETE DATA") };
                data.push(GroundQuad { subject, predicate, object, graph_name });
            }
            return Ok(GraphUpdateOperation::DeleteData { data });
        }
        if self.is_kw("DELETE") && self.is_kw_n(1, "WHERE") {
            self.next(); self.next();
            let quads = self.quads(QuadMode::Delete)?;
            let delete = self.ground_patterns(&quads)?;
            let pattern = quads_to_pattern(&quads);
            return Ok(GraphUpdateOperation::DeleteInsert { delete, insert: vec![], with: None, using: None, pattern: Box::new(pattern) });
        }
        // Modify: [WITH iri] (DELETE {…} [INSERT {…}] | INSERT {…}) USING* WHERE {…}
        let with = if self.eat_kw("WITH") { Some(self.iri()?) } else { None };
        let mut delete = Vec::new();
        let mut insert = Vec::new();
        let mut any = false;
        if self.eat_kw("DELETE") { let q = self.quads(QuadMode::Delete)?; delete = self.ground_patterns(&q)?; any = true; }
        if self.eat_kw("INSERT") {
            let q = self.quads(QuadMode::Insert)?;
            insert = q.into_iter().map(|(s, p, o, g)| QuadPattern { subject: s, predicate: p, object: o, graph_name: g }).collect();
            any = true;
        }
        if !any { return self.err(format!("expected an update operation, found {}", self.peek())) }
        let mut using: Option<QueryDataset> = None;
        while self.eat_kw("USING") {
            let d = using.get_or_insert_with(Default::default);
            if self.eat_kw("NAMED") { let g = self.iri()?; d.named.get_or_insert_with(Vec::new).push(g) } else { let g = self.iri()?; d.default.push(g) }
        }
        self.expect_kw("WHERE")?;
        let pattern = self.group_graph_pattern()?;
        Ok(GraphUpdateOperation::DeleteInsert { delete, insert, with, using, pattern: Box::new(pattern) })
    }

    fn ground_patterns(&self, quads: &[(TermPattern, NamedNodePattern, TermPattern, GraphNamePattern)]) -> R<Vec<GroundQuadPattern>> {
        let g = |t: &TermPattern| -> R<GroundTermPattern> {
            match t {
                TermPattern::NamedNode(n) => Ok(GroundTermPattern::NamedNode(n.clone())),
                TermPattern::Literal(l) => Ok(GroundTermPattern::Literal(l.clone())),
                TermPattern::Variable(v) => Ok(GroundTermPattern::Variable(v.clone())),
                TermPattern::BlankNode(_) => self.err("blank nodes are not allowed in DELETE templates"),
            }
        };
        quads.iter().map(|(s, p, o, gr)| Ok(GroundQuadPattern { subject: g(s)?, predicate: p.clone(), object: g(o)?, graph_name: gr.clone() })).collect()
    }

    /// `{ triples (GRAPH g { triples })* }` of update data and templates.
    fn quads(&mut self, mode: QuadMode) -> R<Vec<(TermPattern, NamedNodePattern, TermPattern, GraphNamePattern)>> {
        // DELETE WHERE's quads are also its pattern, so they keep the scope check; templates and data do not.
        self.template = !matches!(mode, QuadMode::Delete);
        let r = self.quads_inner(mode);
        self.template = false;
        r
    }

    fn quads_inner(&mut self, mode: QuadMode) -> R<Vec<(TermPattern, NamedNodePattern, TermPattern, GraphNamePattern)>> {
        self.expect_punct("{")?;
        self.block += 1;
        let first_label_count = self.bnode_block.len();
        let mut out = Vec::new();
        loop {
            if self.eat_punct("}") { break }
            if self.eat_kw("GRAPH") {
                let g = match self.var_or_iri()? { NamedNodePattern::NamedNode(n) => GraphNamePattern::NamedNode(n), NamedNodePattern::Variable(v) => GraphNamePattern::Variable(v) };
                self.expect_punct("{")?;
                for t in self.triples_template()? { out.push((t.subject, t.predicate, t.object, g.clone())) }
                self.expect_punct("}")?;
                self.eat_punct(".");
                continue;
            }
            for t in self.triples_template()? { out.push((t.subject, t.predicate, t.object, GraphNamePattern::DefaultGraph)) }
            if !self.is_punct("}") && !self.is_kw("GRAPH") { return self.err(format!("expected '}}' or GRAPH, found {}", self.peek())) }
        }
        let _ = first_label_count;
        for (s, p, o, g) in &out {
            let data = matches!(mode, QuadMode::InsertData | QuadMode::DeleteData);
            let has_var = |t: &TermPattern| matches!(t, TermPattern::Variable(_));
            if data && (has_var(s) || has_var(o) || matches!(p, NamedNodePattern::Variable(_)) || matches!(g, GraphNamePattern::Variable(_))) {
                return self.err("variables are not allowed in DATA blocks");
            }
            let bnodes = [s, o].into_iter().filter_map(|t| if let TermPattern::BlankNode(b) = t { Some(b.as_str().to_string()) } else { None });
            for b in bnodes {
                match mode {
                    QuadMode::DeleteData | QuadMode::Delete => return self.err("blank nodes are not allowed in DELETE"),
                    QuadMode::InsertData => {
                        // A label may not be shared by two INSERT DATA operations of one request.
                        if self.data_bnodes.contains(&b) && !out_labels_this_op(&out).contains(&b) { return self.err(format!("blank node _:{b} is reused across operations")) }
                    }
                    QuadMode::Insert => {}
                }
            }
        }
        if matches!(mode, QuadMode::InsertData) {
            let labels = out_labels_this_op(&out);
            for l in &labels { if self.data_bnodes.contains(l) { return self.err(format!("blank node _:{l} is reused across operations")) } }
            self.data_bnodes.extend(labels);
        }
        Ok(out)
    }
}

fn out_labels_this_op(out: &[(TermPattern, NamedNodePattern, TermPattern, GraphNamePattern)]) -> HashSet<String> {
    let mut s = HashSet::new();
    for (a, _, b, _) in out { for t in [a, b] { if let TermPattern::BlankNode(x) = t { s.insert(x.as_str().to_string()); } } }
    s
}

#[derive(Clone, Copy)]
enum QuadMode { InsertData, DeleteData, Delete, Insert }

enum Verb { Var(Variable), Path(PropertyPathExpression) }

#[derive(Clone)]
enum SelectItem { Var(Variable), Expr(Expression, Variable) }

struct Select { modifier: u8, items: Option<Vec<SelectItem>> }

enum Form { Select(Select), Describe(Vec<NamedNodePattern>), Star, Ask }

/// `Join` with the empty group as identity (§18.2.2.8 simplification).
fn join(a: GraphPattern, b: GraphPattern) -> GraphPattern {
    let empty = |p: &GraphPattern| matches!(p, GraphPattern::Bgp { patterns } if patterns.is_empty());
    if empty(&a) { return b }
    if empty(&b) { return a }
    // Adjacent basic graph patterns form one BGP.
    if let (GraphPattern::Bgp { patterns: x }, GraphPattern::Bgp { patterns: y }) = (&a, &b) {
        let mut patterns = x.clone();
        patterns.extend(y.iter().cloned());
        return GraphPattern::Bgp { patterns };
    }
    GraphPattern::Join { left: Box::new(a), right: Box::new(b) }
}

fn quads_to_pattern(quads: &[(TermPattern, NamedNodePattern, TermPattern, GraphNamePattern)]) -> GraphPattern {
    let mut default = Vec::new();
    let mut graphs: Vec<(NamedNodePattern, Vec<TriplePattern>)> = Vec::new();
    for (s, p, o, g) in quads {
        let t = TriplePattern { subject: s.clone(), predicate: p.clone(), object: o.clone() };
        match g {
            GraphNamePattern::DefaultGraph => default.push(t),
            GraphNamePattern::NamedNode(n) => push_graph(&mut graphs, NamedNodePattern::NamedNode(n.clone()), t),
            GraphNamePattern::Variable(v) => push_graph(&mut graphs, NamedNodePattern::Variable(v.clone()), t),
        }
    }
    let mut p = GraphPattern::Bgp { patterns: default };
    for (name, ts) in graphs { p = join(p, GraphPattern::Graph { name, inner: Box::new(GraphPattern::Bgp { patterns: ts }) }) }
    p
}

fn push_graph(graphs: &mut Vec<(NamedNodePattern, Vec<TriplePattern>)>, name: NamedNodePattern, t: TriplePattern) {
    match graphs.iter_mut().find(|(n, _)| *n == name) { Some((_, ts)) => ts.push(t), None => graphs.push((name, vec![t])) }
}

/// Variables of an expression outside aggregates (aggregates are already replaced by their variables).
fn expr_vars(e: &Expression, out: &mut Vec<Variable>) {
    match e {
        Expression::Variable(v) | Expression::Bound(v) => out.push(v.clone()),
        Expression::NamedNode(_) | Expression::Literal(_) | Expression::Exists(_) => {}
        Expression::Or(a, b) | Expression::And(a, b) | Expression::Equal(a, b) | Expression::SameTerm(a, b)
        | Expression::Greater(a, b) | Expression::GreaterOrEqual(a, b) | Expression::Less(a, b) | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b) | Expression::Subtract(a, b) | Expression::Multiply(a, b) | Expression::Divide(a, b) => { expr_vars(a, out); expr_vars(b, out) }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => expr_vars(a, out),
        Expression::In(a, l) => { expr_vars(a, out); for x in l { expr_vars(x, out) } }
        Expression::If(a, b, c) => { expr_vars(a, out); expr_vars(b, out); expr_vars(c, out) }
        Expression::Coalesce(l) | Expression::FunctionCall(_, l) => for x in l { expr_vars(x, out) },
    }
}

/// In-scope variables of a pattern (§18.2.1), in order of first appearance; blank-node variables are not included.
pub fn in_scope(p: &GraphPattern) -> Vec<Variable> {
    let mut out: Vec<Variable> = Vec::new();
    fn add(out: &mut Vec<Variable>, v: &Variable) { if !out.contains(v) { out.push(v.clone()) } }
    fn term(out: &mut Vec<Variable>, t: &TermPattern) { if let TermPattern::Variable(v) = t { add(out, v) } }
    fn walk(p: &GraphPattern, out: &mut Vec<Variable>) {
        match p {
            GraphPattern::Bgp { patterns } => for t in patterns {
                term(out, &t.subject);
                if let NamedNodePattern::Variable(v) = &t.predicate { add(out, v) }
                term(out, &t.object);
            },
            GraphPattern::Path { subject, object, .. } => { term(out, subject); term(out, object) }
            GraphPattern::Join { left, right } | GraphPattern::LeftJoin { left, right, .. } | GraphPattern::Union { left, right } => { walk(left, out); walk(right, out) }
            GraphPattern::Minus { left, .. } => walk(left, out),
            GraphPattern::Filter { inner, .. } | GraphPattern::OrderBy { inner, .. } | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner } | GraphPattern::Slice { inner, .. } => walk(inner, out),
            GraphPattern::Graph { name, inner } | GraphPattern::Service { name, inner, .. } => {
                if let NamedNodePattern::Variable(v) = name { add(out, v) }
                walk(inner, out)
            }
            GraphPattern::Extend { inner, variable, .. } => { walk(inner, out); add(out, variable) }
            GraphPattern::Values { variables, .. } => for v in variables { add(out, v) },
            GraphPattern::Project { variables, .. } => for v in variables { add(out, v) },
            GraphPattern::Group { variables, aggregates, .. } => {
                for v in variables { add(out, v) }
                for (v, _) in aggregates { add(out, v) }
            }
        }
    }
    walk(p, &mut out);
    out.retain(|v| !v.as_str().starts_with('_'));
    out
}
