//! SPARQL expression evaluation over resolved terms. `None` is the SPARQL "error" value.
use super::{Ctx, Row};
use crate::storage::Snapshot;
use oxrdf::vocab::xsd;
use oxrdf::{Literal, NamedNode, Term};
use spargebra::algebra::{Expression, Function};
use std::cmp::Ordering;

const NUMERIC: [&str; 16] = [
    "integer", "decimal", "double", "float", "int", "long", "short", "byte", "nonNegativeInteger",
    "positiveInteger", "negativeInteger", "nonPositiveInteger", "unsignedInt", "unsignedLong", "unsignedShort", "unsignedByte",
];

fn xsd_local(l: &Literal) -> Option<&str> { l.datatype().as_str().strip_prefix("http://www.w3.org/2001/XMLSchema#") }

#[derive(Copy, Clone, PartialEq, PartialOrd, Debug)]
/// SPARQL numeric value. Promotion order integer < decimal < float < double (XPath op:numeric-*). `Flt` holds an
/// f32-rounded value so float arithmetic and output stay single precision.
pub(super) enum Num { Int(i64), Dec(f64), Flt(f64), Dbl(f64) }
impl Num {
    /// The value at a promoted type rank (1 decimal, 2 float, 3 double); a float result is rounded to binary32, as
    /// every XPath float operation is.
    pub(super) fn of_rank(rank: u8, v: f64) -> Num {
        match rank { 0 | 1 => Num::Dec(v), 2 => Num::Flt(v as f32 as f64), _ => Num::Dbl(v) }
    }
    pub(super) fn f(self) -> f64 { match self { Num::Int(i) => i as f64, Num::Dec(d) | Num::Flt(d) | Num::Dbl(d) => d } }
    pub(super) fn rank(self) -> u8 { match self { Num::Int(_) => 0, Num::Dec(_) => 1, Num::Flt(_) => 2, Num::Dbl(_) => 3 } }
    pub(super) fn term(self) -> Term {
        match self {
            Num::Int(i) => Literal::from(i).into(),
            Num::Dec(d) => Literal::new_typed_literal(fmt_dec(d), xsd::DECIMAL).into(),
            Num::Flt(d) => Literal::new_typed_literal(java_float_lexical(d as f32), xsd::FLOAT).into(),
            Num::Dbl(d) => Literal::from(d).into(),
        }
    }
}
fn fmt_dec(d: f64) -> String {
    let s = format!("{d}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") }
}

/// Integers compare exactly (as i64; f64 would conflate neighbours above 2^53); otherwise by promoted f64.
fn num_cmp(x: Num, y: Num) -> Option<Ordering> {
    if let (Num::Int(i), Num::Int(j)) = (x, y) { return Some(i.cmp(&j)) }
    x.f().partial_cmp(&y.f())
}

fn num(t: &Term) -> Option<Num> {
    let Term::Literal(l) = t else { return None };
    let local = xsd_local(l)?;
    if !NUMERIC.contains(&local) { return None }
    let v = l.value().trim();
    match local {
        "double" => parse_xsd_float(v).map(Num::Dbl),
        "float" => parse_xsd_float(v).map(|d| Num::Flt(d as f32 as f64)),
        "decimal" => v.parse().ok().map(Num::Dec),
        _ => v.trim_start_matches('+').parse().ok().map(Num::Int),
    }
}

pub(super) fn string_value(t: &Term) -> Option<String> {
    match t {
        Term::Literal(l) => Some(l.value().to_string()),
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn is_stringish(l: &Literal) -> bool { l.datatype() == xsd::STRING || l.language().is_some() }

fn bool_term(b: bool) -> Term { Literal::from(b).into() }

pub(super) fn ebv(t: &Term) -> Option<bool> {
    let Term::Literal(l) = t else { return None };
    if l.datatype() == xsd::BOOLEAN { return Some(matches!(l.value(), "true" | "1")) }
    if let Some(n) = num(t) { return Some(n.f() != 0.0 && !n.f().is_nan()) }
    if is_stringish(l) { return Some(!l.value().is_empty()) }
    None
}

pub(super) fn arith(op: char, a: &Term, b: &Term) -> Option<Term> {
    let (x, y) = (num(a)?, num(b)?);
    let rank = x.rank().max(y.rank());
    if op == '/' && rank == 0 {
        if y.f() == 0.0 { return None }
        return Some(Num::Dec(x.f() / y.f()).term());
    }
    if rank == 0 {
        let (Num::Int(i), Num::Int(j)) = (x, y) else { unreachable!() };
        let r = match op { '+' => i.checked_add(j), '-' => i.checked_sub(j), _ => i.checked_mul(j) }?;
        return Some(Num::Int(r).term());
    }
    let (i, j) = (x.f(), y.f());
    let r = match op { '+' => i + j, '-' => i - j, '*' => i * j, _ => { if rank == 1 && j == 0.0 { return None } i / j } };
    Some(Num::of_rank(rank, r).term())
}

/// Value comparison for `=`, `<` etc. `None` = type error.
fn compare(a: &Term, b: &Term) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (num(a), num(b)) { return num_cmp(x, y) }
    match (a, b) {
        (Term::Literal(x), Term::Literal(y)) => {
            let comparable = (x.datatype() == y.datatype() && x.language() == y.language())
                && (is_stringish(x) || matches!(xsd_local(x), Some("dateTime" | "date" | "time" | "boolean")));
            if comparable { Some(x.value().cmp(y.value())) } else { None }
        }
        _ => None,
    }
}

fn equal(a: &Term, b: &Term) -> Option<bool> {
    if a == b { return Some(true) }
    match compare(a, b) {
        Some(o) => Some(o == Ordering::Equal),
        None => match (a, b) {
            (Term::Literal(x), Term::Literal(y)) if xsd_local(x).is_some() && xsd_local(y).is_some() && x.datatype() != y.datatype() && !is_stringish(x) && !is_stringish(y) => None,
            (Term::Literal(_), Term::Literal(_)) => Some(false),
            _ => Some(false),
        },
    }
}

/// Total order used by ORDER BY / MIN / MAX: unbound < blank < IRI < literal.
pub(super) fn order_cmp(a: Option<&Term>, b: Option<&Term>) -> Ordering {
    fn class(t: Option<&Term>) -> u8 { match t { None => 0, Some(Term::BlankNode(_)) => 1, Some(Term::NamedNode(_)) => 2, _ => 3 } }
    match (a, b) {
        (Some(x), Some(y)) if class(a) == class(b) => {
            if let Some(o) = compare(x, y) { return o }
            match (x, y) {
                (Term::NamedNode(m), Term::NamedNode(n)) => m.as_str().cmp(n.as_str()),
                (Term::BlankNode(m), Term::BlankNode(n)) => m.as_str().cmp(n.as_str()),
                (Term::Literal(m), Term::Literal(n)) => m.value().cmp(n.value()).then_with(|| m.datatype().as_str().cmp(n.datatype().as_str())),
                _ => Ordering::Equal,
            }
        }
        _ => class(a).cmp(&class(b)),
    }
}

fn simple(s: impl Into<String>) -> Term { Literal::new_simple_literal(s).into() }

/// String result that keeps the language tag of the first argument (as SPARQL string functions require).
fn like(src: &Term, s: String) -> Term {
    match src {
        Term::Literal(l) if l.language().is_some() => Literal::new_language_tagged_literal_unchecked(s, l.language().unwrap()).into(),
        _ => simple(s),
    }
}

fn lit(t: &Term) -> Option<&Literal> { if let Term::Literal(l) = t { Some(l) } else { None } }

pub(super) fn eval<S: Snapshot + ?Sized>(ctx: &Ctx<'_, S>, e: &Expression, r: &Row) -> Option<Term> {
    let v = |e: &Expression| eval(ctx, e, r);
    match e {
        Expression::NamedNode(n) => Some(n.clone().into()),
        Expression::Literal(l) => Some(l.clone().into()),
        Expression::Variable(x) => ctx.var_value(x, r),
        Expression::Bound(x) => Some(bool_term(ctx.var_value(x, r).is_some())),
        Expression::Or(a, b) => match (v(a).and_then(|t| ebv(&t)), v(b).and_then(|t| ebv(&t))) {
            (Some(true), _) | (_, Some(true)) => Some(bool_term(true)),
            (Some(false), Some(false)) => Some(bool_term(false)),
            _ => None,
        },
        Expression::And(a, b) => match (v(a).and_then(|t| ebv(&t)), v(b).and_then(|t| ebv(&t))) {
            (Some(false), _) | (_, Some(false)) => Some(bool_term(false)),
            (Some(true), Some(true)) => Some(bool_term(true)),
            _ => None,
        },
        Expression::Not(a) => Some(bool_term(!ebv(&v(a)?)?)),
        Expression::Equal(a, b) => Some(bool_term(equal(&v(a)?, &v(b)?)?)),
        Expression::SameTerm(a, b) => Some(bool_term(v(a)? == v(b)?)),
        Expression::Greater(a, b) => Some(bool_term(compare(&v(a)?, &v(b)?)? == Ordering::Greater)),
        Expression::GreaterOrEqual(a, b) => Some(bool_term(compare(&v(a)?, &v(b)?)? != Ordering::Less)),
        Expression::Less(a, b) => Some(bool_term(compare(&v(a)?, &v(b)?)? == Ordering::Less)),
        Expression::LessOrEqual(a, b) => Some(bool_term(compare(&v(a)?, &v(b)?)? != Ordering::Greater)),
        Expression::In(a, list) => {
            let x = v(a)?;
            let mut err = false;
            for y in list {
                match v(y).and_then(|y| equal(&x, &y)) { Some(true) => return Some(bool_term(true)), None => err = true, _ => {} }
            }
            if err { None } else { Some(bool_term(false)) }
        }
        Expression::Add(a, b) => arith('+', &v(a)?, &v(b)?),
        Expression::Subtract(a, b) => arith('-', &v(a)?, &v(b)?),
        Expression::Multiply(a, b) => arith('*', &v(a)?, &v(b)?),
        Expression::Divide(a, b) => arith('/', &v(a)?, &v(b)?),
        Expression::UnaryPlus(a) => { let t = v(a)?; num(&t)?; Some(t) }
        Expression::UnaryMinus(a) => arith('*', &v(a)?, &Literal::from(-1i64).into()),
        Expression::Exists(p) => Some(bool_term(ctx.exists(p, r))),
        Expression::If(c, a, b) => if ebv(&v(c)?)? { v(a) } else { v(b) },
        Expression::Coalesce(l) => l.iter().find_map(v),
        Expression::FunctionCall(f, args) => call(f, args, &|e| v(e)),
    }
}

fn call(f: &Function, args: &[Expression], v: &dyn Fn(&Expression) -> Option<Term>) -> Option<Term> {
    let a = |i: usize| v(args.get(i)?);
    let s = |i: usize| -> Option<String> { let t = a(i)?; Some(lit(&t)?.value().to_string()) };
    match f {
        Function::Str => Some(simple(string_value(&a(0)?)?)),
        Function::Lang => Some(simple(lit(&a(0)?)?.language().unwrap_or(""))),
        Function::LangMatches => {
            let (tag, range) = (s(0)?.to_lowercase(), s(1)?.to_lowercase());
            Some(bool_term(if range == "*" { !tag.is_empty() } else { tag == range || tag.starts_with(&format!("{range}-")) }))
        }
        Function::Datatype => Some(lit(&a(0)?)?.datatype().into_owned().into()),
        Function::Iri => match a(0)? { Term::NamedNode(n) => Some(n.into()), t => NamedNode::new(lit(&t)?.value()).ok().map(Into::into) },
        Function::IsIri => Some(bool_term(matches!(a(0)?, Term::NamedNode(_)))),
        Function::IsBlank => Some(bool_term(matches!(a(0)?, Term::BlankNode(_)))),
        Function::IsLiteral => Some(bool_term(matches!(a(0)?, Term::Literal(_)))),
        Function::IsNumeric => Some(bool_term(num(&a(0)?).is_some())),
        Function::StrLen => Some(Literal::from(s(0)?.chars().count() as i64).into()),
        Function::UCase => { let t = a(0)?; let x = lit(&t)?.value().to_uppercase(); Some(like(&t, x)) }
        Function::LCase => { let t = a(0)?; let x = lit(&t)?.value().to_lowercase(); Some(like(&t, x)) }
        Function::Contains => Some(bool_term(s(0)?.contains(&s(1)?))),
        Function::StrStarts => Some(bool_term(s(0)?.starts_with(&s(1)?))),
        Function::StrEnds => Some(bool_term(s(0)?.ends_with(&s(1)?))),
        Function::StrBefore => { let (x, y) = (s(0)?, s(1)?); Some(simple(x.find(&y).map(|i| &x[..i]).unwrap_or(""))) }
        Function::StrAfter => { let (x, y) = (s(0)?, s(1)?); Some(simple(x.find(&y).map(|i| &x[i + y.len()..]).unwrap_or(""))) }
        Function::Concat => {
            let mut out = String::new();
            for i in 0..args.len() { out.push_str(&s(i)?) }
            Some(simple(out))
        }
        Function::SubStr => {
            let t = a(0)?;
            let x: Vec<char> = lit(&t)?.value().chars().collect();
            let start = num(&a(1)?)?.f().round() as i64;
            let len = match args.get(2) { Some(_) => num(&a(2)?)?.f().round() as i64, None => i64::MAX / 2 };
            let (lo, hi) = (start.max(1), start.saturating_add(len));
            let sub: String = x.iter().enumerate().filter(|(i, _)| { let p = *i as i64 + 1; p >= lo && p < hi }).map(|(_, c)| c).collect();
            Some(like(&t, sub))
        }
        Function::Regex => {
            let flags = if args.len() > 2 { s(2)? } else { String::new() };
            let pat = if flags.contains('i') { format!("(?i){}", s(1)?) } else { s(1)? };
            Some(bool_term(regex::Regex::new(&pat).ok()?.is_match(&s(0)?)))
        }
        Function::Replace => {
            let flags = if args.len() > 3 { s(3)? } else { String::new() };
            let pat = if flags.contains('i') { format!("(?i){}", s(1)?) } else { s(1)? };
            let t = a(0)?;
            let out = regex::Regex::new(&pat).ok()?.replace_all(lit(&t)?.value(), s(2)?.as_str()).into_owned();
            Some(like(&t, out))
        }
        Function::Abs => { let n = num(&a(0)?)?; Some(match n { Num::Int(i) => Num::Int(i.abs()), Num::Dec(d) => Num::Dec(d.abs()), Num::Flt(d) => Num::Flt(d.abs()), Num::Dbl(d) => Num::Dbl(d.abs()) }.term()) }
        Function::Ceil | Function::Floor | Function::Round => {
            let n = num(&a(0)?)?;
            let g = |d: f64| match f { Function::Ceil => d.ceil(), Function::Floor => d.floor(), _ => (d + 0.5).floor() };
            Some(match n { Num::Int(i) => Num::Int(i), Num::Dec(d) => Num::Dec(g(d)), Num::Flt(d) => Num::Flt(g(d)), Num::Dbl(d) => Num::Dbl(g(d)) }.term())
        }
        Function::Year | Function::Month | Function::Day | Function::Hours | Function::Minutes => {
            let x = s(0)?;
            let (date, time) = x.split_once('T').unwrap_or((x.as_str(), ""));
            let neg = date.starts_with('-');
            let parts: Vec<&str> = date.trim_start_matches('-').split('-').collect();
            let field = match f {
                Function::Year => { let y: i64 = parts.first()?.parse().ok()?; return Some(Literal::from(if neg { -y } else { y }).into()) }
                Function::Month => parts.get(1)?.to_string(),
                Function::Day => parts.get(2)?.get(..2)?.to_string(),
                Function::Hours => time.get(..2)?.to_string(),
                _ => time.get(3..5)?.to_string(),
            };
            Some(Literal::from(field.parse::<i64>().ok()?).into())
        }
        Function::StrLang => Some(Literal::new_language_tagged_literal(s(0)?, s(1)?).ok()?.into()),
        Function::StrDt => match a(1)? { Term::NamedNode(n) => Some(Literal::new_typed_literal(s(0)?, n).into()), _ => None },
        Function::EncodeForUri => {
            let x = s(0)?;
            Some(simple(x.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>()))
        }
        Function::Custom(f) => xsd_cast(f.as_str(), &a(0)?),
        _ => None, // hashes, NOW/RAND/UUID, BNODE and RDF-1.2 functions: not supported in P1
    }
}

/// Compiled numeric filter (comparisons over `+ - *` of variables and numeric constants, combined with `&& || !`).
/// Each distinct term ID is decoded to a number once and memoized, so a filter over many rows does no term
/// materialization. A variable whose value is unbound or not numeric makes the row fall back to the general evaluator,
/// so results are identical to `eval` by construction (comparison is by `f64`, integer arithmetic is checked).
pub(super) enum NumFilter { Cmp(Ordering, bool, NumExpr, NumExpr), And(Box<NumFilter>, Box<NumFilter>), Or(Box<NumFilter>, Box<NumFilter>), Not(Box<NumFilter>) }
pub(super) enum NumExpr { Var(usize), Const(Num), Op(char, Box<NumExpr>, Box<NumExpr>) }
/// `Err(())`: this row needs the general evaluator.
type Fast<T> = Result<T, ()>;

impl NumExpr {
    fn compile(e: &Expression, vars: &std::collections::HashMap<String, usize>) -> Option<NumExpr> {
        Some(match e {
            Expression::Variable(v) => NumExpr::Var(*vars.get(v.as_str())?),
            Expression::Literal(l) => NumExpr::Const(num(&l.clone().into())?),
            Expression::Add(a, b) => NumExpr::Op('+', Box::new(Self::compile(a, vars)?), Box::new(Self::compile(b, vars)?)),
            Expression::Subtract(a, b) => NumExpr::Op('-', Box::new(Self::compile(a, vars)?), Box::new(Self::compile(b, vars)?)),
            Expression::Multiply(a, b) => NumExpr::Op('*', Box::new(Self::compile(a, vars)?), Box::new(Self::compile(b, vars)?)),
            _ => return None,
        })
    }
    /// `Ok(None)`: SPARQL error (integer overflow), as in `arith`.
    fn eval(&self, r: &[crate::model::TermId], val: &mut dyn FnMut(crate::model::TermId) -> Option<Num>) -> Fast<Option<Num>> {
        match self {
            NumExpr::Var(i) => { let id = *r.get(*i).ok_or(())?; if id == super::UNBOUND { return Err(()) } val(id).map(Some).ok_or(()) }
            NumExpr::Const(c) => Ok(Some(*c)),
            NumExpr::Op(op, a, b) => {
                let (Some(x), Some(y)) = (a.eval(r, val)?, b.eval(r, val)?) else { return Ok(None) };
                let rank = x.rank().max(y.rank());
                if let (Num::Int(i), Num::Int(j)) = (x, y) {
                    let v = match op { '+' => i.checked_add(j), '-' => i.checked_sub(j), _ => i.checked_mul(j) };
                    return Ok(v.map(Num::Int));
                }
                let (i, j) = (x.f(), y.f());
                let v = match op { '+' => i + j, '-' => i - j, _ => i * j };
                Ok(Some(Num::of_rank(rank, v)))
            }
        }
    }
}

impl NumFilter {
    pub(super) fn compile(e: &Expression, vars: &std::collections::HashMap<String, usize>) -> Option<NumFilter> {
        let cmp = |a: &Expression, b: &Expression, o: Ordering, eq: bool| Some(NumFilter::Cmp(o, eq, NumExpr::compile(a, vars)?, NumExpr::compile(b, vars)?));
        match e {
            Expression::Less(a, b) => cmp(a, b, Ordering::Less, false),
            Expression::LessOrEqual(a, b) => cmp(a, b, Ordering::Less, true),
            Expression::Greater(a, b) => cmp(a, b, Ordering::Greater, false),
            Expression::GreaterOrEqual(a, b) => cmp(a, b, Ordering::Greater, true),
            Expression::And(a, b) => Some(NumFilter::And(Box::new(Self::compile(a, vars)?), Box::new(Self::compile(b, vars)?))),
            Expression::Or(a, b) => Some(NumFilter::Or(Box::new(Self::compile(a, vars)?), Box::new(Self::compile(b, vars)?))),
            Expression::Not(a) => Some(NumFilter::Not(Box::new(Self::compile(a, vars)?))),
            _ => None,
        }
    }
    /// Three-valued result (`None` = error), mirroring `eval`; `Err` = fall back for this row.
    pub(super) fn eval(&self, r: &[crate::model::TermId], val: &mut dyn FnMut(crate::model::TermId) -> Option<Num>) -> Fast<Option<bool>> {
        Ok(match self {
            NumFilter::Cmp(o, eq, a, b) => {
                let (Some(x), Some(y)) = (a.eval(r, val)?, b.eval(r, val)?) else { return Ok(None) };
                num_cmp(x, y).map(|c| c == *o || (*eq && c == Ordering::Equal))
            }
            NumFilter::And(a, b) => match (a.eval(r, val)?, b.eval(r, val)?) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            NumFilter::Or(a, b) => match (a.eval(r, val)?, b.eval(r, val)?) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            NumFilter::Not(a) => a.eval(r, val)?.map(|b| !b),
        })
    }
}

/// Numeric value of a term (for the memoized filter path).
pub(super) fn term_num(t: &Term) -> Option<Num> { num(t) }

/// XPath constructor functions (SPARQL 1.1 §17.5 casts), e.g. `xsd:double(str(?price))`. `None` = type error.
fn xsd_cast(iri: &str, t: &Term) -> Option<Term> {
    let target = iri.strip_prefix("http://www.w3.org/2001/XMLSchema#")?;
    let typed = |v: String, dt: oxrdf::NamedNodeRef<'_>| -> Option<Term> { Some(Literal::new_typed_literal(v, dt).into()) };
    // Source: a numeric value, a boolean, or a lexical form (plain/xsd:string literal, or any literal's string).
    let source_num = num(t);
    let lexical = match t {
        Term::Literal(l) => Some(l.value().trim().to_string()),
        Term::NamedNode(n) if target == "string" => Some(n.as_str().to_string()),
        _ => None,
    }?;
    let is_bool = matches!(t, Term::Literal(l) if l.datatype() == xsd::BOOLEAN);
    match target {
        "string" => Some(simple(lexical)),
        "double" | "float" => {
            let d = match source_num { Some(n) => n.f(), None if is_bool => if matches!(lexical.as_str(), "true" | "1") { 1.0 } else { 0.0 },
                None => match lexical.as_str() { "INF" | "+INF" => f64::INFINITY, "-INF" => f64::NEG_INFINITY, "NaN" => f64::NAN, s => s.parse().ok()? } };
            if target == "float" { return Some(Num::Flt(d as f32 as f64).term()) }
            typed(format_double(d), xsd::DOUBLE)
        }
        "decimal" => {
            let d = match source_num { Some(n) => { let f = n.f(); if !f.is_finite() { return None } f }
                None if is_bool => if matches!(lexical.as_str(), "true" | "1") { 1.0 } else { 0.0 },
                None => { if lexical.contains(['e', 'E']) || lexical.is_empty() { return None } lexical.parse().ok()? } };
            typed(fmt_dec(d), xsd::DECIMAL)
        }
        "integer" | "int" | "long" | "short" | "byte" => {
            let i: i64 = match source_num { Some(Num::Int(i)) => i, Some(n) => { let f = n.f(); if !f.is_finite() { return None } f.trunc() as i64 }
                None if is_bool => i64::from(matches!(lexical.as_str(), "true" | "1")),
                None => lexical.trim_start_matches('+').parse().ok()? };
            typed(i.to_string(), oxrdf::NamedNodeRef::new_unchecked(iri))
        }
        "boolean" => {
            let b = match source_num { Some(n) => { let f = n.f(); f != 0.0 && !f.is_nan() }
                None => match lexical.as_str() { "true" | "1" => true, "false" | "0" => false, _ => return None } };
            Some(bool_term(b))
        }
        "dateTime" | "date" | "time" => typed(lexical, oxrdf::NamedNodeRef::new_unchecked(iri)),
        _ => None,
    }
}

fn format_double(d: f64) -> String {
    if d.is_nan() { "NaN".into() } else if d.is_infinite() { if d > 0.0 { "INF".into() } else { "-INF".into() } } else { format!("{d:E}") }
}

/// SUM/AVG accumulation step: `arith('+')`'s typing (checked integer add; promote to decimal/float/double), but a
/// float accumulator keeps f64 precision and is rounded to binary32 once, when the result becomes a term. SUM is over a
/// multiset with unspecified order; one final rounding is much more accurate than rounding after every addition, whose
/// error grows with the group size. Ordinary expression arithmetic (`arith`, compiled filters) still rounds per operation.
pub(super) fn num_add(a: Num, b: Num) -> Option<Num> {
    if let (Num::Int(i), Num::Int(j)) = (a, b) { return i.checked_add(j).map(Num::Int) }
    let v = a.f() + b.f();
    Some(match a.rank().max(b.rank()) { 0 | 1 => Num::Dec(v), 2 => Num::Flt(v), _ => Num::Dbl(v) })
}
/// `sum / n` with exactly `arith('/')`'s typing (integer / integer is decimal).
pub(super) fn num_div_count(sum: Num, n: i64) -> Option<Num> {
    let r = sum.rank();
    if n == 0 && r <= 1 { return None }
    let v = sum.f() / n as f64;
    Some(match r.max(1) { 1 => Num::Dec(v), 2 => Num::Flt(v), _ => Num::Dbl(v) })
}

/// xsd:double / xsd:float lexical forms, including INF, -INF and NaN (Rust's parser spells them differently).
fn parse_xsd_float(v: &str) -> Option<f64> {
    match v { "INF" | "+INF" => Some(f64::INFINITY), "-INF" => Some(f64::NEG_INFINITY), "NaN" => Some(f64::NAN),
        _ if v.eq_ignore_ascii_case("inf") || v.eq_ignore_ascii_case("infinity") || v.eq_ignore_ascii_case("nan") => None,
        _ => v.parse().ok() }
}

/// xsd:float lexical form as Java's `Float.toString` writes it (what Jena returns): plain decimal with at least one
/// fractional digit for 1e-3 <= |x| < 1e7 ("6.0", "0.25"), otherwise scientific with an uppercase E ("1.0E10").
pub(super) fn java_float_lexical(x: f32) -> String {
    if x.is_nan() { return "NaN".into() }
    if x.is_infinite() { return if x > 0.0 { "INF".into() } else { "-INF".into() } }
    if x == 0.0 { return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() } }
    let a = x.abs();
    if (1e-3..1e7).contains(&a) {
        let s = format!("{x}");
        if s.contains('.') { s } else { format!("{s}.0") }
    } else {
        let s = format!("{x:e}");
        let (m, e) = s.split_once('e').unwrap();
        let m = if m.contains('.') { m.to_string() } else { format!("{m}.0") };
        format!("{m}E{e}")
    }
}
