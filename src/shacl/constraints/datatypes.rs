use std::cmp::Ordering;

use oxsdatatypes::{
    Boolean, Date, DateTime, DayTimeDuration, Double, Duration, Float, Time, YearMonthDuration,
};

use crate::rdf::{Literal, Term};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn xsd_name(literal: &Literal) -> Option<&str> {
    literal.datatype().as_str().strip_prefix(XSD)
}

fn space(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\n', '\r'])
}

pub(super) fn integer_text(text: &str) -> Option<(bool, &str)> {
    let text = space(text);
    let negative = text.starts_with('-');
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    Some((negative && !digits.is_empty(), digits))
}

fn is_integer(datatype: &str) -> bool {
    matches!(
        datatype,
        "integer"
            | "long"
            | "int"
            | "short"
            | "byte"
            | "nonPositiveInteger"
            | "negativeInteger"
            | "nonNegativeInteger"
            | "positiveInteger"
            | "unsignedLong"
            | "unsignedInt"
            | "unsignedShort"
            | "unsignedByte"
    )
}

fn integer_in_range(datatype: &str, text: &str) -> bool {
    let Some((negative, digits)) = integer_text(text) else {
        return false;
    };
    let amount = || {
        if digits.is_empty() {
            Some(0)
        } else {
            digits.parse::<u64>().ok()
        }
    };
    let signed = |max: u64| amount().is_some_and(|value| value <= max + u64::from(negative));
    let unsigned = |max: u64| !negative && amount().is_some_and(|value| value <= max);
    match datatype {
        "integer" => true,
        "long" => signed(i64::MAX as u64),
        "int" => signed(i32::MAX as u64),
        "short" => signed(i16::MAX as u64),
        "byte" => signed(i8::MAX as u64),
        "nonPositiveInteger" => negative || digits.is_empty(),
        "negativeInteger" => negative,
        "nonNegativeInteger" => !negative,
        "positiveInteger" => !negative && !digits.is_empty(),
        "unsignedLong" => unsigned(u64::MAX),
        "unsignedInt" => unsigned(u64::from(u32::MAX)),
        "unsignedShort" => unsigned(u64::from(u16::MAX)),
        "unsignedByte" => unsigned(u64::from(u8::MAX)),
        _ => false,
    }
}

/// XSD decimals and integers are not limited to machine precision.
struct ExactNumber<'a> {
    negative: bool,
    integral: &'a str,
    fractional: &'a str,
}

impl<'a> ExactNumber<'a> {
    fn new(text: &'a str) -> Option<Self> {
        let text = space(text);
        let negative = text.starts_with('-');
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        let (integral, fractional) = digits.split_once('.').unwrap_or((digits, ""));
        if (integral.is_empty() && fractional.is_empty())
            || !integral
                .bytes()
                .chain(fractional.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let integral = integral.trim_start_matches('0');
        let fractional = fractional.trim_end_matches('0');
        Some(Self {
            negative: negative && (!integral.is_empty() || !fractional.is_empty()),
            integral,
            fractional,
        })
    }

    fn cmp(&self, other: &Self) -> Ordering {
        if self.negative != other.negative {
            return other.negative.cmp(&self.negative);
        }
        let magnitude = self
            .integral
            .len()
            .cmp(&other.integral.len())
            .then_with(|| self.integral.cmp(other.integral))
            .then_with(|| self.fractional.cmp(other.fractional));
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

pub(super) fn valid_lexical(literal: &Literal) -> bool {
    if literal.datatype().as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString" {
        return literal.language().is_some();
    }
    let text = space(literal.value());
    match xsd_name(literal) {
        Some(name) if is_integer(name) => integer_in_range(name, text),
        Some("decimal") => ExactNumber::new(text).is_some(),
        Some("boolean") => text.parse::<Boolean>().is_ok(),
        Some("float") => text.parse::<Float>().is_ok(),
        Some("double") => text.parse::<Double>().is_ok(),
        Some("dateTime") => text.parse::<DateTime>().is_ok(),
        Some("date") => text.parse::<Date>().is_ok(),
        Some("time") => text.parse::<Time>().is_ok(),
        Some("duration") => text.parse::<Duration>().is_ok(),
        Some("dayTimeDuration") => text.parse::<DayTimeDuration>().is_ok(),
        Some("yearMonthDuration") => text.parse::<YearMonthDuration>().is_ok(),
        _ => true,
    }
}

fn numeric_rank(datatype: &str) -> Option<u8> {
    match datatype {
        "double" => Some(2),
        "float" => Some(1),
        "decimal" => Some(0),
        name if is_integer(name) => Some(0),
        _ => None,
    }
}

fn numeric_key(literal: &Literal) -> Option<f64> {
    let text = space(literal.value());
    match xsd_name(literal)? {
        "float" => Some(f64::from(f32::from(text.parse::<Float>().ok()?))),
        "double" => Some(f64::from(text.parse::<Double>().ok()?)),
        _ => text.parse().ok(),
    }
}

fn float_key(literal: &Literal) -> Option<f32> {
    let text = space(literal.value());
    if xsd_name(literal) == Some("float") {
        Some(f32::from(text.parse::<Float>().ok()?))
    } else {
        text.parse().ok()
    }
}

pub(super) fn compare(left: &Term, right: &Term) -> Option<Ordering> {
    let (Term::Literal(left), Term::Literal(right)) = (left, right) else {
        return None;
    };
    let l_name = xsd_name(left)?;
    let r_name = xsd_name(right)?;
    if !valid_lexical(left) || !valid_lexical(right) {
        return None;
    }
    let l = space(left.value());
    let r = space(right.value());
    if let (Some(a), Some(b)) = (numeric_rank(l_name), numeric_rank(r_name)) {
        return match a.max(b) {
            0 => Some(ExactNumber::new(l)?.cmp(&ExactNumber::new(r)?)),
            1 => float_key(left)?.partial_cmp(&float_key(right)?),
            _ => numeric_key(left)?.partial_cmp(&numeric_key(right)?),
        };
    }
    match (l_name, r_name) {
        ("string", "string") => Some(left.value().cmp(right.value())),
        ("boolean", "boolean") => l.parse::<Boolean>().ok()?.partial_cmp(&r.parse().ok()?),
        ("dateTime", "dateTime") => l.parse::<DateTime>().ok()?.partial_cmp(&r.parse().ok()?),
        ("date", "date") => l.parse::<Date>().ok()?.partial_cmp(&r.parse().ok()?),
        ("time", "time") => l.parse::<Time>().ok()?.partial_cmp(&r.parse().ok()?),
        ("dayTimeDuration", "dayTimeDuration") => l
            .parse::<DayTimeDuration>()
            .ok()?
            .partial_cmp(&r.parse::<DayTimeDuration>().ok()?),
        ("yearMonthDuration", "yearMonthDuration") => l
            .parse::<YearMonthDuration>()
            .ok()?
            .partial_cmp(&r.parse::<YearMonthDuration>().ok()?),
        _ => None,
    }
}
