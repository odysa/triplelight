//! SPARQL expression evaluation over resolved terms. `None` is the SPARQL "error" value.
use super::{Ctx, Row};
use crate::storage::Snapshot;
use triplelight_rdf::vocab::xsd;
use triplelight_rdf::{Literal, NamedNode, Term};
#[cfg(feature = "rdf-12")]
use triplelight_rdf::{BaseDirection, NamedOrBlankNode, Triple};
use crate::sparql::{Expression, Function};
use std::cmp::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};
// std's clock panics on wasm32-unknown-unknown; web-time reads the JS clock there.
#[cfg(target_arch = "wasm32")]
use web_time::{SystemTime, UNIX_EPOCH};
use std::str::FromStr;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};

const NUMERIC: [&str; 16] = [
    "integer", "decimal", "double", "float", "int", "long", "short", "byte", "nonNegativeInteger",
    "positiveInteger", "negativeInteger", "nonPositiveInteger", "unsignedInt", "unsignedLong", "unsignedShort", "unsignedByte",
];

fn xsd_local(l: &Literal) -> Option<&str> { l.datatype().as_str().strip_prefix("http://www.w3.org/2001/XMLSchema#") }

#[derive(Copy, Clone, PartialEq, PartialOrd, Debug)]
/// SPARQL numeric value. Promotion order integer < decimal < float < double (XPath op:numeric-*). `Dec` is an exact
/// decimal (28 significant digits; overflow is a SPARQL error). `Flt` holds an f32-rounded value so float arithmetic
/// and output stay single precision.
pub(super) enum Num { Int(i64), Dec(Decimal), Flt(f64), Dbl(f64) }
impl Num {
    /// The value at a promoted type rank (1 decimal, 2 float, 3 double); a float result is rounded to binary32, as
    /// every XPath float operation is.
    pub(super) fn f(self) -> f64 { match self { Num::Int(i) => i as f64, Num::Dec(d) => d.to_f64().unwrap_or(f64::NAN), Num::Flt(d) | Num::Dbl(d) => d } }
    /// The exact decimal value of an integer or decimal (`None` for float/double).
    fn dec(self) -> Option<Decimal> { match self { Num::Int(i) => Some(Decimal::from(i)), Num::Dec(d) => Some(d), _ => None } }
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
/// Decimal lexical form, Jena style: no exponent, at least one fractional digit ("3.0", "11.1").
fn fmt_dec(d: Decimal) -> String {
    let s = d.normalize().to_string();
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// Integers and decimals compare exactly; float/double by promoted f64.
fn num_cmp(x: Num, y: Num) -> Option<Ordering> {
    if let (Num::Int(i), Num::Int(j)) = (x, y) { return Some(i.cmp(&j)) }
    if let (Some(a), Some(b)) = (x.dec(), y.dec()) { return Some(a.cmp(&b)) }
    // Promotion converts each operand to the common type first: compared as float, a decimal is rounded to binary32.
    if x.rank().max(y.rank()) == 2 { return (x.f() as f32).partial_cmp(&(y.f() as f32)) }
    x.f().partial_cmp(&y.f())
}

/// Exact decimal operations: the exact result is computed on integer mantissas (arbitrary precision) and accepted only
/// if it fits `Decimal` (mantissa below 2^96, at most 28 fractional digits once trailing zeros are removed); otherwise it
/// is a SPARQL error, never a rounded value. Only a non-terminating quotient (e.g. 1/3) is rounded to 28 digits, as
/// XPath allows for decimal division.
fn dec_exact(op: char, a: Decimal, b: Decimal) -> Option<Decimal> {
    use num_bigint::BigInt;
    let (ma, sa, mb, sb) = (BigInt::from(a.mantissa()), a.scale(), BigInt::from(b.mantissa()), b.scale());
    let p10 = |k: u32| BigInt::from(10u8).pow(k);
    match op {
        '+' | '-' => {
            let s = sa.max(sb);
            let (x, y) = (ma * p10(s - sa), mb * p10(s - sb));
            fit_decimal(if op == '+' { x + y } else { x - y }, s)
        }
        '*' => fit_decimal(ma * mb, sa + sb),
        _ => {
            if b.is_zero() { return None }
            // a / b = (ma * 10^sb) / (mb * 10^sa), reduced to lowest terms.
            let (mut num, mut den) = (ma * p10(sb), mb * p10(sa));
            if den < BigInt::from(0) { num = -num; den = -den; }
            let g = num_gcd(num.clone(), den.clone());
            let (num, den) = (num / &g, den / &g);
            // It terminates iff the reduced denominator has no prime factors other than 2 and 5.
            let (mut d, mut twos, mut fives) = (den.clone(), 0u32, 0u32);
            let (two, five, zero) = (BigInt::from(2u8), BigInt::from(5u8), BigInt::from(0u8));
            while &d % &two == zero { d /= &two; twos += 1 }
            while &d % &five == zero { d /= &five; fives += 1 }
            if d == BigInt::from(1u8) {
                let k = twos.max(fives);
                fit_decimal(num * p10(k) / den, k)
            } else {
                a.checked_div(b)
            }
        }
    }
}

fn num_gcd(mut a: num_bigint::BigInt, mut b: num_bigint::BigInt) -> num_bigint::BigInt {
    use num_bigint::Sign;
    if a.sign() == Sign::Minus { a = -a }
    if b.sign() == Sign::Minus { b = -b }
    while b != num_bigint::BigInt::from(0u8) { let r = &a % &b; a = b; b = r; }
    a
}

/// `m * 10^-s` as a Decimal if it is exactly representable.
fn fit_decimal(mut m: num_bigint::BigInt, mut s: u32) -> Option<Decimal> {
    use num_bigint::BigInt;
    let (ten, zero) = (BigInt::from(10u8), BigInt::from(0u8));
    while s > 0 && &m % &ten == zero { m /= &ten; s -= 1 }
    if s > 28 { return None }
    let limit = BigInt::from(1u8) << 96;
    if m >= limit || m <= -limit { return None }
    let m: i128 = m.try_into().ok()?;
    Decimal::try_from_i128_with_scale(m, s).ok()
}

/// XPath op:numeric-add/subtract/multiply/divide with type promotion. Integer arithmetic is checked; integer and
/// decimal arithmetic is exact (integer / integer is a decimal); overflow and decimal division by zero are errors;
/// float rounds to binary32 per operation; double is IEEE. `None` = SPARQL error.
pub(super) fn num_op(op: char, x: Num, y: Num) -> Option<Num> {
    let rank = x.rank().max(y.rank());
    if rank == 0 && op != '/' {
        let (Num::Int(i), Num::Int(j)) = (x, y) else { unreachable!() };
        return match op { '+' => i.checked_add(j), '-' => i.checked_sub(j), _ => i.checked_mul(j) }.map(Num::Int);
    }
    if rank <= 1 { return dec_exact(op, x.dec()?, y.dec()?).map(Num::Dec) }
    if rank == 2 {
        // Float: each operand is converted to binary32 first, then the operation is done in binary32.
        let (i, j) = (x.f() as f32, y.f() as f32);
        return Some(Num::Flt(match op { '+' => i + j, '-' => i - j, '*' => i * j, _ => i / j } as f64));
    }
    let (i, j) = (x.f(), y.f());
    Some(Num::Dbl(match op { '+' => i + j, '-' => i - j, '*' => i * j, _ => i / j }))
}

fn num(t: &Term) -> Option<Num> {
    let Term::Literal(l) = t else { return None };
    let local = xsd_local(l)?;
    if !NUMERIC.contains(&local) { return None }
    let v = l.value().trim();
    match local {
        "double" => parse_xsd_float(v).map(Num::Dbl),
        "float" => parse_xsd_float(v).map(|d| Num::Flt(d as f32 as f64)),
        "decimal" => parse_decimal(v).map(Num::Dec),
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
    if l.datatype() == xsd::BOOLEAN {
        return match l.value() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ if cfg!(feature = "rdf-12") => None,
            _ => Some(false),
        }
    }
    if let Some(n) = num(t) { return Some(n.f() != 0.0 && !n.f().is_nan()) }
    if l.datatype() == xsd::STRING || (!cfg!(feature = "rdf-12") && l.language().is_some()) {
        return Some(!l.value().is_empty())
    }
    None
}

pub(super) fn arith(op: char, a: &Term, b: &Term) -> Option<Term> {
    num_op(op, num(a)?, num(b)?).map(Num::term)
}

/// Value comparison for `=`, `<` etc. `None` = type error.
fn compare(a: &Term, b: &Term) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (num(a), num(b)) { return num_cmp(x, y) }
    match (a, b) {
        (Term::Literal(x), Term::Literal(y)) => {
            let comparable = (x.datatype() == y.datatype() && x.language() == y.language() && same_direction(x, y))
                && (is_stringish(x) || matches!(xsd_local(x), Some("dateTime" | "date" | "time" | "boolean")));
            if comparable { Some(x.value().cmp(y.value())) } else { None }
        }
        _ => None,
    }
}

/// Two literals have the same base direction (SPARQL 1.2): `"a"@en--ltr` and `"a"@en--rtl` are different values.
#[cfg(feature = "rdf-12")]
fn same_direction(x: &Literal, y: &Literal) -> bool { x.direction() == y.direction() }
#[cfg(not(feature = "rdf-12"))]
fn same_direction(_: &Literal, _: &Literal) -> bool { true }

fn equal(a: &Term, b: &Term) -> Option<bool> {
    if a == b { return Some(true) }
    // SPARQL 1.2 sameValue (§17.4.2.2) on two triple terms: component-wise; true if every pair is equal, an error if
    // any pair is an error, otherwise false. (A triple term and any other term are simply unequal, below.)
    #[cfg(feature = "rdf-12")]
    if let (Term::Triple(x), Term::Triple(y)) = (a, b) {
        let parts = [
            equal(&x.subject.clone().into(), &y.subject.clone().into()),
            Some(x.predicate == y.predicate),
            equal(&x.object, &y.object),
        ];
        if parts.iter().all(|p| *p == Some(true)) { return Some(true) }
        if parts.iter().any(Option::is_none) { return None }
        return Some(false);
    }
    match compare(a, b) {
        Some(o) => Some(o == Ordering::Equal),
        None => match (a, b) {
            (Term::Literal(x), Term::Literal(y)) if xsd_local(x).is_some() && xsd_local(y).is_some() && x.datatype() != y.datatype() && !is_stringish(x) && !is_stringish(y) => None,
            (Term::Literal(_), Term::Literal(_)) => Some(false),
            _ => Some(false),
        },
    }
}

/// Total order used by ORDER BY / MIN / MAX: unbound < blank < IRI < literal (< triple term, SPARQL 1.2 §15.1).
pub(super) fn order_cmp(a: Option<&Term>, b: Option<&Term>) -> Ordering {
    fn class(t: Option<&Term>) -> u8 {
        match t {
            None => 0, Some(Term::BlankNode(_)) => 1, Some(Term::NamedNode(_)) => 2,
            #[cfg(feature = "rdf-12")]
            Some(Term::Triple(_)) => 4,
            _ => 3,
        }
    }
    match (a, b) {
        (Some(x), Some(y)) if class(a) == class(b) => {
            if let Some(o) = compare(x, y) { return o }
            match (x, y) {
                (Term::NamedNode(m), Term::NamedNode(n)) => m.as_str().cmp(n.as_str()),
                (Term::BlankNode(m), Term::BlankNode(n)) => m.as_str().cmp(n.as_str()),
                #[cfg(not(feature = "rdf-12"))]
                (Term::Literal(m), Term::Literal(n)) => m.value().cmp(n.value()).then_with(|| m.datatype().as_str().cmp(n.datatype().as_str())),
                // Language tag and base direction break the remaining ties, so `"a"@en--ltr`, `"a"@en--rtl` and
                // `"a"@fr--ltr` never sort as equal.
                #[cfg(feature = "rdf-12")]
                (Term::Literal(m), Term::Literal(n)) => m.value().cmp(n.value())
                    .then_with(|| m.datatype().as_str().cmp(n.datatype().as_str()))
                    .then_with(|| m.language().cmp(&n.language()))
                    .then_with(|| direction_rank(m.direction()).cmp(&direction_rank(n.direction()))),
                // The draft leaves two triple terms unordered; order them component-wise (subject, predicate,
                // object), recursively, so distinct triple terms never tie.
                #[cfg(feature = "rdf-12")]
                (Term::Triple(m), Term::Triple(n)) => triple_cmp(m, n),
                _ => Ordering::Equal,
            }
        }
        _ => class(a).cmp(&class(b)),
    }
}

#[cfg(feature = "rdf-12")]
fn direction_rank(d: Option<BaseDirection>) -> u8 { match d { None => 0, Some(BaseDirection::Ltr) => 1, Some(BaseDirection::Rtl) => 2 } }

#[cfg(feature = "rdf-12")]
fn triple_cmp(m: &Triple, n: &Triple) -> Ordering {
    let subject = |t: &Triple| -> Term { t.subject.clone().into() };
    order_cmp(Some(&subject(m)), Some(&subject(n)))
        .then_with(|| m.predicate.as_str().cmp(n.predicate.as_str()))
        .then_with(|| order_cmp(Some(&m.object), Some(&n.object)))
}

fn simple(s: impl Into<String>) -> Term { Literal::new_simple_literal(s).into() }

/// Whether two string arguments have the same base direction (SPARQL 1.2 argument compatibility); always true for
/// RDF 1.1 terms, which have none.
fn same_dir(_a: &Term, _b: &Term) -> bool {
    #[cfg(feature = "rdf-12")]
    {
        let d = |t: &Term| if let Term::Literal(l) = t { l.direction() } else { None };
        d(_a) == d(_b)
    }
    #[cfg(not(feature = "rdf-12"))]
    { true }
}

/// String result that keeps the language tag of the first argument (as SPARQL string functions require).
fn like(src: &Term, s: String) -> Term {
    match src {
        // SPARQL 1.2: the base direction is kept along with the language tag.
        #[cfg(feature = "rdf-12")]
        Term::Literal(l) if l.direction().is_some() => Literal::new_directional_language_tagged_literal_unchecked(s, l.language().unwrap(), l.direction().unwrap()).into(),
        Term::Literal(l) if l.language().is_some() => Literal::new_language_tagged_literal_unchecked(s, l.language().unwrap()).into(),
        _ => simple(s),
    }
}

fn lit(t: &Term) -> Option<&Literal> { if let Term::Literal(l) = t { Some(l) } else { None } }
/// A string literal (simple, xsd:string or language-tagged), the argument type of SPARQL string functions.
fn strlit(t: &Term) -> Option<&Literal> { lit(t).filter(|l| is_stringish(l)) }

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
        Expression::FunctionCall(f, args) => call(f, args, &|e| v(e), ctx.base.as_deref(), &ctx.now),
    }
}

fn call(f: &Function, args: &[Expression], v: &dyn Fn(&Expression) -> Option<Term>, base: Option<&str>, now: &str) -> Option<Term> {
    let a = |i: usize| v(args.get(i)?);
    // SPARQL string functions take string literals only (simple, xsd:string or language-tagged); anything else,
    // including numbers and other typed literals, is a type error (§17.4.3).
    let sl = |i: usize| -> Option<(String, Option<String>)> {
        let t = a(i)?;
        let l = strlit(&t)?;
        Some((l.value().to_string(), l.language().map(str::to_string)))
    };
    let s = |i: usize| -> Option<String> { sl(i).map(|x| x.0) };
    // A plain string: simple or xsd:string, no language tag (STRDT / STRLANG lexical forms).
    let plain = |i: usize| -> Option<String> { match sl(i)? { (x, None) => Some(x), _ => None } };
    // §17.4.3.1.1 argument compatibility: both plain, the same language tag, or a tagged first and a plain second.
    let compat = |l1: &Option<String>, l2: &Option<String>| match (l1, l2) {
        (_, None) => true,
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        (None, Some(_)) => false,
    };
    // A result string carrying the first argument's language tag (or plain).
    match f {
        Function::Str => Some(simple(string_value(&a(0)?)?)),
        Function::Lang => Some(simple(lit(&a(0)?)?.language().unwrap_or(""))),
        Function::LangMatches => {
            let (tag, range) = (s(0)?.to_lowercase(), s(1)?.to_lowercase());
            Some(bool_term(if range == "*" { !tag.is_empty() } else { tag == range || tag.starts_with(&format!("{range}-")) }))
        }
        Function::Datatype => Some(lit(&a(0)?)?.datatype().into_owned().into()),
        // IRI()/URI(): an IRI argument is returned as is; a simple string is resolved against the query's BASE.
        Function::Iri => match a(0)? {
            Term::NamedNode(n) => Some(n.into()),
            t => {
                let x = plain(0).or_else(|| strlit(&t).filter(|l| l.language().is_none()).map(|l| l.value().to_string()))?;
                let resolved = match base { Some(b) => triplelight_iri::Iri::parse(b).ok()?.resolve(&x).ok()?.into_string(), None => x };
                NamedNode::new(resolved).ok().map(Into::into)
            }
        },
        Function::IsIri => Some(bool_term(matches!(a(0)?, Term::NamedNode(_)))),
        Function::IsBlank => Some(bool_term(matches!(a(0)?, Term::BlankNode(_)))),
        Function::IsLiteral => Some(bool_term(matches!(a(0)?, Term::Literal(_)))),
        Function::IsNumeric => Some(bool_term(num(&a(0)?).is_some())),
        Function::StrLen => Some(Literal::from(s(0)?.chars().count() as i64).into()),
        Function::UCase => { let t = a(0)?; let x = strlit(&t)?.value().to_uppercase(); Some(like(&t, x)) }
        Function::LCase => { let t = a(0)?; let x = strlit(&t)?.value().to_lowercase(); Some(like(&t, x)) }
        Function::Contains | Function::StrStarts | Function::StrEnds => {
            let ((x, lx), (y, ly)) = (sl(0)?, sl(1)?);
            if !compat(&lx, &ly) { return None }
            Some(bool_term(match f { Function::Contains => x.contains(&y), Function::StrStarts => x.starts_with(&y), _ => x.ends_with(&y) }))
        }
        // No match gives the empty simple literal; a match keeps the first argument's language tag (§17.4.3.7/8).
        Function::StrBefore => {
            let ((x, lx), (y, ly)) = (sl(0)?, sl(1)?);
            if !compat(&lx, &ly) || !same_dir(&a(0)?, &a(1)?) { return None }
            Some(match x.find(&y) { Some(i) => like(&a(0)?, x[..i].to_string()), None => simple("") })
        }
        Function::StrAfter => {
            let ((x, lx), (y, ly)) = (sl(0)?, sl(1)?);
            if !compat(&lx, &ly) || !same_dir(&a(0)?, &a(1)?) { return None }
            Some(match x.find(&y) { Some(i) => like(&a(0)?, x[i + y.len()..].to_string()), None => simple("") })
        }
        // The result keeps a language tag only if every argument has that same tag (§17.4.3.12).
        Function::Concat => {
            let mut out = String::new();
            let mut lang: Option<Option<String>> = None;
            // SPARQL 1.2: the tag is kept only if every argument also has the same base direction.
            let mut dir_same = true;
            for i in 0..args.len() {
                let (x, l) = sl(i)?;
                out.push_str(&x);
                if i > 0 { dir_same &= same_dir(&a(0)?, &a(i)?) }
                lang = Some(match lang { None => l, Some(prev) if prev.as_deref().map(str::to_lowercase) == l.as_deref().map(str::to_lowercase) => prev, Some(_) => None });
            }
            match lang.flatten() {
                Some(_) if dir_same && !args.is_empty() => Some(like(&a(0)?, out)),
                _ => Some(simple(out)),
            }
        }
        Function::SubStr => {
            let t = a(0)?;
            let x: Vec<char> = strlit(&t)?.value().chars().collect();
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
            let out = regex::Regex::new(&pat).ok()?.replace_all(strlit(&t)?.value(), s(2)?.as_str()).into_owned();
            Some(like(&t, out))
        }
        Function::Abs => { let n = num(&a(0)?)?; Some(match n { Num::Int(i) => Num::Int(i.abs()), Num::Dec(d) => Num::Dec(d.abs()), Num::Flt(d) => Num::Flt(d.abs()), Num::Dbl(d) => Num::Dbl(d.abs()) }.term()) }
        Function::Ceil | Function::Floor | Function::Round => {
            let n = num(&a(0)?)?;
            let g = |d: f64| match f { Function::Ceil => d.ceil(), Function::Floor => d.floor(), _ => (d + 0.5).floor() };
            // fn:round rounds halves towards positive infinity.
            let gd = |d: Decimal| match f {
                Function::Ceil => d.ceil(),
                Function::Floor => d.floor(),
                _ => if d.fract().is_zero() { d } else { d.checked_add(Decimal::new(5, 1)).map(|x| x.floor()).unwrap_or(d.round()) },
            };
            Some(match n { Num::Int(i) => Num::Int(i), Num::Dec(d) => Num::Dec(gd(d)), Num::Flt(d) => Num::Flt(g(d)), Num::Dbl(d) => Num::Dbl(g(d)) }.term())
        }
        Function::Year | Function::Month | Function::Day | Function::Hours | Function::Minutes => {
            // Date/time accessors take xsd:dateTime (and xsd:date) literals, not strings.
            let t = a(0)?;
            let l = lit(&t).filter(|l| matches!(xsd_local(l), Some("dateTime" | "date")))?;
            let x = l.value().to_string();
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
        Function::StrLang => Some(Literal::new_language_tagged_literal(plain(0)?, plain(1)?).ok()?.into()),
        Function::StrDt => match a(1)? { Term::NamedNode(n) => Some(Literal::new_typed_literal(plain(0)?, n).into()), _ => None },
        Function::EncodeForUri => {
            let x = s(0)?;
            Some(simple(x.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>()))
        }
        Function::Seconds => {
            let t = a(0)?;
            let l = lit(&t).filter(|l| xsd_local(l) == Some("dateTime"))?;
            let (_, time) = l.value().split_once('T')?;
            let (clock, _) = split_timezone(time);
            Some(Num::Dec(Decimal::from_str(clock.get(6..)?).ok()?).term())
        }
        Function::Timezone | Function::Tz => {
            let t = a(0)?;
            let l = lit(&t).filter(|l| xsd_local(l) == Some("dateTime"))?;
            let (_, time) = l.value().split_once('T')?;
            let (_, tz) = split_timezone(time);
            if matches!(f, Function::Tz) { return Some(simple(tz)) }
            if tz.is_empty() { return None } // TIMEZONE of a value without a timezone is an error
            let minutes = if tz == "Z" { 0 } else {
                let sign = if tz.starts_with('-') { -1 } else { 1 };
                sign * (tz.get(1..3)?.parse::<i64>().ok()? * 60 + tz.get(4..6)?.parse::<i64>().ok()?)
            };
            let (h, m) = (minutes.abs() / 60, minutes.abs() % 60);
            let mut d = String::from(if minutes < 0 { "-PT" } else { "PT" });
            if minutes == 0 { d.push_str("0S") }
            if h > 0 { d.push_str(&format!("{h}H")) }
            if m > 0 { d.push_str(&format!("{m}M")) }
            Some(Literal::new_typed_literal(d, NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dayTimeDuration")).into())
        }
        Function::Now => Some(Literal::new_typed_literal(now, xsd::DATE_TIME).into()),
        Function::Rand => Some(Num::Dbl((random_u64() >> 11) as f64 / (1u64 << 53) as f64).term()),
        Function::Uuid => Some(NamedNode::new_unchecked(format!("urn:uuid:{}", uuid_v4())).into()),
        Function::StrUuid => Some(simple(uuid_v4())),
        Function::BNode if args.is_empty() => Some(triplelight_rdf::BlankNode::default().into()),
        Function::Md5 | Function::Sha1 | Function::Sha256 | Function::Sha384 | Function::Sha512 => {
            use sha2::Digest;
            let x = plain(0)?;
            let bytes: Vec<u8> = match f {
                Function::Md5 => md5::Md5::digest(x.as_bytes()).to_vec(),
                Function::Sha1 => sha1::Sha1::digest(x.as_bytes()).to_vec(),
                Function::Sha256 => sha2::Sha256::digest(x.as_bytes()).to_vec(),
                Function::Sha384 => sha2::Sha384::digest(x.as_bytes()).to_vec(),
                _ => sha2::Sha512::digest(x.as_bytes()).to_vec(),
            };
            Some(simple(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
        }
        Function::Custom(f) => xsd_cast(f.as_str(), &a(0)?),
        // SPARQL 1.2 §17.4.6: TRIPLE errors unless (s, p, o) is an RDF triple (IRI or blank-node subject, IRI
        // predicate); SUBJECT / PREDICATE / OBJECT error on anything but a triple term.
        #[cfg(feature = "rdf-12")]
        Function::Triple => {
            let s = match a(0)? { Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n), Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b), _ => return None };
            let Term::NamedNode(p) = a(1)? else { return None };
            Some(Triple::new(s, p, a(2)?).into())
        }
        #[cfg(feature = "rdf-12")]
        Function::Subject => match a(0)? { Term::Triple(t) => { let t = *t; Some(t.subject.into()) } _ => None },
        #[cfg(feature = "rdf-12")]
        Function::Predicate => match a(0)? { Term::Triple(t) => { let t = *t; Some(t.predicate.into()) } _ => None },
        #[cfg(feature = "rdf-12")]
        Function::Object => match a(0)? { Term::Triple(t) => { let t = *t; Some(t.object) } _ => None },
        #[cfg(feature = "rdf-12")]
        Function::IsTriple => Some(bool_term(matches!(a(0)?, Term::Triple(_)))),
        // §17.4.2.9-11: LANGDIR takes a literal ("" without a direction; a non-literal is an error); hasLANG and
        // hasLANGDIR are false for anything that is not a literal with a language tag / base direction.
        #[cfg(feature = "rdf-12")]
        Function::LangDir => Some(simple(lit(&a(0)?)?.direction().map_or_else(String::new, |d| d.to_string()))),
        #[cfg(feature = "rdf-12")]
        Function::HasLang => Some(bool_term(matches!(a(0)?, Term::Literal(l) if l.language().is_some()))),
        #[cfg(feature = "rdf-12")]
        Function::HasLangDir => Some(bool_term(matches!(a(0)?, Term::Literal(l) if l.direction().is_some()))),
        // §17.4.2.17: plain-string arguments; a non-empty, well-formed language tag; direction exactly "ltr" or "rtl"
        // (case-sensitive).
        #[cfg(feature = "rdf-12")]
        Function::StrLangDir => {
            let dir = match plain(2)?.as_str() { "ltr" => BaseDirection::Ltr, "rtl" => BaseDirection::Rtl, _ => return None };
            let lang = plain(1)?;
            if lang.is_empty() { return None }
            Some(Literal::new_directional_language_tagged_literal(plain(0)?, lang, dir).ok()?.into())
        }
        _ => None, // BNODE(string) is rejected up front by `support`
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
                Ok(num_op(*op, x, y))
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
    let typed = |v: String, dt: triplelight_rdf::NamedNodeRef<'_>| -> Option<Term> { Some(Literal::new_typed_literal(v, dt).into()) };
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
            let d = match source_num {
                Some(n) => match n.dec() { Some(d) => d, None => { let f = n.f(); if !f.is_finite() { return None } Decimal::from_f64(f)? } },
                None if is_bool => if matches!(lexical.as_str(), "true" | "1") { Decimal::ONE } else { Decimal::ZERO },
                None => { if lexical.contains(['e', 'E']) || lexical.is_empty() { return None } parse_decimal(&lexical)? } };
            typed(fmt_dec(d), xsd::DECIMAL)
        }
        "integer" | "int" | "long" | "short" | "byte" => {
            let i: i64 = match source_num { Some(Num::Int(i)) => i, Some(n) => { let f = n.f(); if !f.is_finite() { return None } f.trunc() as i64 }
                None if is_bool => i64::from(matches!(lexical.as_str(), "true" | "1")),
                None => lexical.trim_start_matches('+').parse().ok()? };
            typed(i.to_string(), triplelight_rdf::NamedNodeRef::new_unchecked(iri))
        }
        "boolean" => {
            let b = match source_num { Some(n) => { let f = n.f(); f != 0.0 && !f.is_nan() }
                None => match lexical.as_str() { "true" | "1" => true, "false" | "0" => false, _ => return None } };
            Some(bool_term(b))
        }
        "dateTime" | "date" | "time" => typed(lexical, triplelight_rdf::NamedNodeRef::new_unchecked(iri)),
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
    match a.rank().max(b.rank()) {
        0 | 1 => num_op('+', a, b),
        // Operands are promoted to float (binary32) first; the accumulation itself stays wide (rounded once later).
        2 => Some(Num::Flt(if a.rank() == 2 { a.f() } else { a.f() as f32 as f64 } + if b.rank() == 2 { b.f() } else { b.f() as f32 as f64 })),
        _ => Some(Num::Dbl(a.f() + b.f())),
    }
}
/// `sum / n` with exactly `arith('/')`'s typing (integer / integer is decimal).
pub(super) fn num_div_count(sum: Num, n: i64) -> Option<Num> {
    let r = sum.rank();
    if r <= 1 { return num_op('/', sum, Num::Int(n)) }
    let v = sum.f() / n as f64;
    Some(if r == 2 { Num::Flt(v) } else { Num::Dbl(v) })
}

/// An xsd:decimal lexical form ("+1.50", "-.5", "3"); no exponent.
fn parse_decimal(v: &str) -> Option<Decimal> {
    let v = v.trim().trim_start_matches('+');
    if v.is_empty() || v.contains(['e', 'E']) { return None }
    let v = if let Some(rest) = v.strip_prefix("-.") { format!("-0.{rest}") } else if let Some(rest) = v.strip_prefix('.') { format!("0.{rest}") } else { v.to_string() };
    // Trailing fractional zeros do not change the value but can push the mantissa past 96 bits ("…335.0").
    let v = if v.contains('.') { v.trim_end_matches('0').trim_end_matches('.').to_string() } else { v };
    let v = if v.is_empty() || v == "-" { "0".to_string() } else { v };
    // Exact or not at all: a lexical form with more precision than Decimal holds is out of the supported range.
    Decimal::from_str_exact(&v).ok()
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

/// `("12:30:05.5", "-05:00")` from the time part of an xsd:dateTime; the timezone is `""` when absent.
fn split_timezone(time: &str) -> (&str, &str) {
    if let Some(c) = time.strip_suffix('Z') { return (c, "Z") }
    if time.len() > 6 {
        let (c, tz) = time.split_at(time.len() - 6);
        if (tz.starts_with('+') || tz.starts_with('-')) && tz.as_bytes()[3] == b':' { return (c, tz) }
    }
    (time, "")
}

/// A random 64-bit value from the standard library's randomly keyed hasher (no RNG dependency); good enough for
/// RAND() and UUID(), not for cryptography.
pub(crate) fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, AtomicOrdering::Relaxed));
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

/// A random (version 4) UUID in lowercase hex.
fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&random_u64().to_le_bytes());
    b[8..].copy_from_slice(&random_u64().to_le_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The current time as an xsd:dateTime lexical form in UTC with milliseconds (NOW() is fixed per query).
pub(super) fn now_lexical() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem / 60 % 60, rem % 60, d.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::{Literal, ebv, xsd};

    #[test]
    fn ebv_of_language_tagged_strings() {
        for (lexical, legacy_value) in [("hello", true), ("", false)] {
            let term = Literal::new_language_tagged_literal(lexical, "en").unwrap().into();
            let expected = if cfg!(feature = "rdf-12") { None } else { Some(legacy_value) };
            assert_eq!(ebv(&term), expected);
            assert_eq!(ebv(&Literal::new_simple_literal(lexical).into()), Some(legacy_value));
        }
    }

    #[test]
    fn ebv_of_boolean_lexicals() {
        for (lexical, value) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            let term = Literal::new_typed_literal(lexical, xsd::BOOLEAN).into();
            assert_eq!(ebv(&term), Some(value));
        }
        let invalid = Literal::new_typed_literal("z", xsd::BOOLEAN).into();
        let expected = if cfg!(feature = "rdf-12") { None } else { Some(false) };
        assert_eq!(ebv(&invalid), expected);
    }
}
