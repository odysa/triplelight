//! Optional value-oriented Arrow columns. RDF structs remain the lossless default.
use std::sync::{Arc, OnceLock};
use arrow_array::{Array, ArrayRef, Date32Array, Decimal128Array, Float32Array, Float64Array, Int64Array, LargeStringArray, RecordBatch, StringArray, StructArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use regex::Regex;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
fn float_re() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").unwrap()) }
fn integer_re() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^[+-]?[0-9]+$").unwrap()) }
fn decimal_re() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)$").unwrap()) }
fn date_re() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap()) }
fn time_re() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,6})?(?:Z|[+-][0-9]{2}:[0-9]{2})?$").unwrap()) }

struct Parts<'a> { outer: &'a StructArray, kind: &'a StringArray, value: &'a LargeStringArray, datatype: &'a LargeStringArray, language: &'a LargeStringArray }
fn parts(a: &ArrayRef) -> Option<Parts<'_>> {
    let outer = a.as_any().downcast_ref::<StructArray>()?;
    Some(Parts { outer, kind: outer.column(0).as_any().downcast_ref()?, value: outer.column(1).as_any().downcast_ref()?, datatype: outer.column(2).as_any().downcast_ref()?, language: outer.column(3).as_any().downcast_ref()? })
}
fn value<'a>(p: &'a Parts<'_>, i: usize) -> Option<&'a str> { if p.outer.is_null(i) { None } else { Some(p.value.value(i)) } }
fn f64_value(s: &str) -> Option<f64> {
    match s { "INF" | "+INF" => Some(f64::INFINITY), "-INF" => Some(f64::NEG_INFINITY), "NaN" => Some(f64::NAN), _ if float_re().is_match(s) => s.parse().ok(), _ => None }
}
fn f32_value(s: &str) -> Option<f32> {
    match s { "INF" | "+INF" => Some(f32::INFINITY), "-INF" => Some(f32::NEG_INFINITY), "NaN" => Some(f32::NAN), _ if float_re().is_match(s) => s.parse().ok(), _ => None }
}
/// Returns the signed mantissa, fractional digits, and significant integer digits.
fn decimal_parts(s: &str) -> Option<(i128, u32, u32)> {
    if !decimal_re().is_match(s) { return None; }
    let neg = s.starts_with('-');
    let digits = s.trim_start_matches(['+', '-']);
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let scale = u32::try_from(fraction.len()).ok()?;
    if scale > 38 { return None; }
    let n_integer = u32::try_from(integer.trim_start_matches('0').len()).ok()?;
    let mut all = String::with_capacity(integer.len() + fraction.len());
    all.push_str(integer); all.push_str(fraction);
    let significant = all.trim_start_matches('0');
    let magnitude: i128 = if significant.is_empty() { 0 } else { significant.parse().ok()? };
    Some((if neg { -magnitude } else { magnitude }, scale, n_integer))
}
fn timestamp(s: &str) -> Option<(i64, bool)> {
    if !time_re().is_match(s) || s.starts_with("0000-") { return None; }
    let aware = s.ends_with('Z') || s.as_bytes().get(s.len().saturating_sub(6)).is_some_and(|b| *b == b'+' || *b == b'-');
    if aware {
        if !s.ends_with('Z') {
            let zone = &s[s.len()-5..];
            let h: u32 = zone[..2].parse().ok()?; let m: u32 = zone[3..].parse().ok()?;
            if h > 14 || m > 59 || (h == 14 && m != 0) { return None; }
        }
        let d = DateTime::parse_from_rfc3339(s).ok()?;
        Some((d.with_timezone(&Utc).timestamp_micros(), true))
    } else {
        let d = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
        Some((d.and_utc().timestamp_micros(), false))
    }
}

/// Convert only homogeneous supported literal columns, or keep the whole RDF struct unchanged.
/// Inference examines every batch before any output schema is chosen.
fn convert_column(cols: &[ArrayRef]) -> Option<Vec<ArrayRef>> {
    let ps: Vec<_> = cols.iter().map(parts).collect::<Option<_>>()?;
    let mut datatype: Option<&str> = None;
    for p in &ps { for i in 0..p.outer.len() {
        if p.outer.is_null(i) { continue; }
        if p.kind.is_null(i) || p.kind.value(i) != "literal" || p.value.is_null(i) || p.datatype.is_null(i) || !p.language.is_null(i) { return None; }
        let t = p.datatype.value(i).strip_prefix(XSD)?;
        if datatype.is_some_and(|old| old != t) { return None; }
        datatype = Some(t);
    }}
    let datatype = datatype?;
    let mut decimal_scale = 0u32; let mut decimal_int = 0u32; let mut aware: Option<bool> = None;
    for p in &ps { for i in 0..p.outer.len() {
        let Some(v) = value(p, i) else { continue };
        match datatype {
            "string" => (),
            "integer" => { if !integer_re().is_match(v) || v.parse::<i64>().is_err() { return None; } },
            "decimal" => { let (_, scale, integer) = decimal_parts(v)?; decimal_scale = decimal_scale.max(scale); decimal_int = decimal_int.max(integer); },
            "float" => { f32_value(v)?; }, "double" => { f64_value(v)?; },
            "date" => { if !date_re().is_match(v) || v.starts_with("0000-") || NaiveDate::parse_from_str(v, "%Y-%m-%d").is_err() { return None; } },
            "dateTime" => { let (_, zone) = timestamp(v)?; if aware.is_some_and(|x| x != zone) { return None; } aware = Some(zone); },
            _ => return None,
        }
    }}
    let precision = (decimal_int + decimal_scale).max(1);
    if datatype == "decimal" && precision > 38 { return None; }
    let mut output = Vec::with_capacity(ps.len());
    for p in &ps {
        let a: ArrayRef = match datatype {
            "string" => Arc::clone(p.outer.column(1)),
            "integer" => Arc::new(Int64Array::from((0..p.outer.len()).map(|i| value(p,i).map(|v| v.parse::<i64>().unwrap())).collect::<Vec<_>>())),
            "float" => Arc::new(Float32Array::from((0..p.outer.len()).map(|i| value(p,i).map(|v| f32_value(v).unwrap())).collect::<Vec<_>>())),
            "double" => Arc::new(Float64Array::from((0..p.outer.len()).map(|i| value(p,i).map(|v| f64_value(v).unwrap())).collect::<Vec<_>>())),
            "decimal" => {
                let values = (0..p.outer.len()).map(|i| match value(p,i) {
                    None => Some(None),
                    Some(v) => {
                        let (mantissa, scale, _) = decimal_parts(v)?;
                        Some(Some(mantissa.checked_mul(10i128.checked_pow(decimal_scale - scale)?)?))
                    }
                }).collect::<Option<Vec<_>>>()?;
                Arc::new(Decimal128Array::from(values).with_precision_and_scale(precision as u8, decimal_scale as i8).ok()?)
            },
            "date" => {
                let epoch = NaiveDate::from_ymd_opt(1970,1,1).unwrap();
                Arc::new(Date32Array::from((0..p.outer.len()).map(|i| value(p,i).map(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").unwrap().signed_duration_since(epoch).num_days() as i32)).collect::<Vec<_>>()))
            },
            "dateTime" => {
                let times = (0..p.outer.len()).map(|i| value(p,i).map(|v| timestamp(v).unwrap().0)).collect::<Vec<_>>();
                let arr = TimestampMicrosecondArray::from(times);
                if aware == Some(true) { Arc::new(arr.with_timezone("UTC")) } else { Arc::new(arr) }
            },
            _ => return None,
        };
        output.push(a);
    }
    Some(output)
}

pub fn convert(schema: Arc<Schema>, batches: Vec<RecordBatch>) -> Result<(Arc<Schema>, Vec<RecordBatch>), String> {
    if batches.is_empty() { return Ok((schema, batches)); }
    let mut columns = Vec::with_capacity(schema.fields().len());
    let mut fields = Vec::with_capacity(schema.fields().len());
    for i in 0..schema.fields().len() {
        let original: Vec<ArrayRef> = batches.iter().map(|b| Arc::clone(b.column(i))).collect();
        let converted = convert_column(&original).unwrap_or(original);
        fields.push(Field::new(schema.field(i).name(), converted[0].data_type().clone(), true));
        columns.push(converted);
    }
    let new_schema = Arc::new(Schema::new(fields));
    let mut out = Vec::with_capacity(batches.len());
    for row in 0..batches.len() {
        let arrays = columns.iter().map(|c| Arc::clone(&c[row])).collect();
        let options = arrow_array::RecordBatchOptions::new().with_row_count(Some(batches[row].num_rows()));
        out.push(RecordBatch::try_new_with_options(Arc::clone(&new_schema), arrays, &options).map_err(|e| e.to_string())?);
    }
    Ok((new_schema, out))
}
