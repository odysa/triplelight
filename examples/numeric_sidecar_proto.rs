//! Task #85 part B: a value-sorted side index for BSBM q010, measured outside the engine.
//! Usage: numeric_sidecar_proto SNAPSHOT_DIR QUERY.rq OUTPUT.json
//!
//! q010 binds one product, takes its features, finds the other products with a shared feature, and keeps those whose
//! `productPropertyNumeric1` is within 120 of the bound product's and `productPropertyNumeric2` within 170. This program
//! (1) times q010 end to end and a copy of it without its numeric patterns and filters (`engine_numeric_part_ms` is the
//! difference of the two runs, a query-ablation difference, not a measurement of the numeric cost: removing the filters
//! can make the query slower); (2) times a hand-written candidate-and-probe plan (bind the candidates from the shared
//! features, then read and decode each candidate's numeric values; its output keys are `today_shape_*`, but it is not a
//! trace of the engine's plan); (3) builds a value-sorted
//! side index per numeric predicate, (value, subject ID), and times the plan "range lookup, then check the shared
//! feature". The two hand-written plans must return the same product IDs; the engine run is checked only by its row count.
use std::collections::HashSet;
use std::time::Instant;
use triplelight::exec::{Options, QueryResult, query_with};
use triplelight::model::{Perm, TermId};
use triplelight::opt::Stats;
use triplelight::storage::{DiskSnapshot, Dictionary, Snapshot};
use triplelight_rdf::{NamedNode, Term};

const NS: &str = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/";
const PRODUCT: &str = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/dataFromProducer204/Product9890";

type R<T> = Result<T, Box<dyn std::error::Error>>;

fn id(snap: &DiskSnapshot, iri: &str) -> R<TermId> {
    snap.lookup(&Term::NamedNode(NamedNode::new(iri)?))?.ok_or_else(|| format!("{iri} not in the dictionary").into())
}
/// All rows of `perm` under `prefix`, as (key columns 0..3).
fn rows(snap: &DiskSnapshot, perm: Perm, prefix: &[TermId]) -> R<Vec<[TermId; 3]>> {
    let mut out = Vec::new();
    for batch in snap.scan(perm, prefix) {
        let b = batch?;
        for i in 0..b.len() { out.push([b.cols[0][i], b.cols[1][i], b.cols[2][i]]); }
    }
    Ok(out)
}
fn int_value(snap: &DiskSnapshot, ids: &[TermId]) -> R<Vec<Option<i64>>> {
    let mut out = vec![None; ids.len()];
    snap.with_terms(ids, &mut |pos, t| {
        if let triplelight_rdf::TermRef::Literal(l) = t { out[pos] = l.value().parse::<i64>().ok(); }
    })?;
    Ok(out)
}
fn ms(t: Instant) -> f64 { t.elapsed().as_secs_f64() * 1e3 }
fn best<T>(n: usize, mut f: impl FnMut() -> R<T>) -> R<(f64, T)> {
    let mut best = f64::MAX; let mut last = None;
    for _ in 0..n { let t = Instant::now(); let v = f()?; best = best.min(ms(t)); last = Some(v); }
    Ok((best, last.unwrap()))
}
fn count_rows(r: &QueryResult) -> usize { match r { QueryResult::Solutions { rows, .. } => rows.len(), _ => 0 } }

fn main() -> R<()> {
    let a: Vec<String> = std::env::args().collect();
    let snap = DiskSnapshot::open(&a[1])?;
    let stats = Stats::from_snapshot(&snap)?;
    let opts = Options { stats: Some(&stats), wcoj: true, raw_ids: false, general_group: false };
    let q = std::fs::read_to_string(&a[2])?;
    // (1) the engine: q010, and q010 without the two numeric patterns and their filters.
    let without: String = q.lines().filter(|l| !l.contains("productPropertyNumeric") && !l.contains("simProperty")).collect::<Vec<_>>().join("\n");
    let (full_ms, full_rows) = best(5, || Ok(count_rows(&query_with(&snap, &q, &opts)?)))?;
    let (nonum_ms, nonum_rows) = best(5, || Ok(count_rows(&query_with(&snap, &without, &opts)?)))?;

    let (n1, n2, feat) = (id(&snap, &format!("{NS}productPropertyNumeric1"))?, id(&snap, &format!("{NS}productPropertyNumeric2"))?, id(&snap, &format!("{NS}productFeature"))?);
    let p0 = id(&snap, PRODUCT)?;
    let bound = |p: TermId| -> R<i64> {
        let r = rows(&snap, Perm::Pso, &[p, p0])?;
        let v = int_value(&snap, &[r[0][2]])?[0].ok_or("bound product has no integer value")?;
        Ok(v)
    };
    let (v1, v2) = (bound(n1)?, bound(n2)?);
    let (lo1, hi1, lo2, hi2) = (v1 - 119, v1 + 119, v2 - 169, v2 + 169);

    // candidates sharing a feature with the bound product (also what the engine's plan starts from)
    let candidates = |snap: &DiskSnapshot| -> R<HashSet<TermId>> {
        let mut set = HashSet::new();
        for f in rows(snap, Perm::Pso, &[feat, p0])? {
            for r in rows(snap, Perm::Pos, &[feat, f[2]])? { if r[2] != p0 { set.insert(r[2]); } }
        }
        Ok(set)
    };
    // (2) today's shape: for each candidate, probe its numeric values and decode them
    let (today_ms, today) = best(5, || {
        let cand = candidates(&snap)?;
        let mut keep = Vec::new();
        for &s in &cand {
            let a = rows(&snap, Perm::Pso, &[n1, s])?; let b = rows(&snap, Perm::Pso, &[n2, s])?;
            if a.is_empty() || b.is_empty() { continue; }
            let va = int_value(&snap, &[a[0][2]])?[0]; let vb = int_value(&snap, &[b[0][2]])?[0];
            if let (Some(x), Some(y)) = (va, vb) && (lo1..=hi1).contains(&x) && (lo2..=hi2).contains(&y) { keep.push(s); }
        }
        keep.sort(); Ok(keep)
    })?;
    // (3) the side index: build once per predicate, then range lookup + feature check
    let build = |p: TermId| -> R<Vec<(i64, TermId)>> {
        let r = rows(&snap, Perm::Pso, &[p])?;
        let objects: Vec<TermId> = r.iter().map(|x| x[2]).collect();
        let vals = int_value(&snap, &objects)?;
        let mut v: Vec<(i64, TermId)> = r.iter().zip(vals).filter_map(|(x, v)| v.map(|v| (v, x[1]))).collect();
        v.sort_unstable();
        Ok(v)
    };
    let t = Instant::now(); let side1 = build(n1)?; let build1_ms = ms(t);
    let t = Instant::now(); let side2 = build(n2)?; let build2_ms = ms(t);
    let range = |side: &[(i64, TermId)], lo: i64, hi: i64| -> HashSet<TermId> {
        let a = side.partition_point(|x| x.0 < lo); let b = side.partition_point(|x| x.0 <= hi);
        side[a..b].iter().map(|x| x.1).collect()
    };
    let (side_ms, sided) = best(5, || {
        let cand = candidates(&snap)?;
        let r1 = range(&side1, lo1, hi1); let r2 = range(&side2, lo2, hi2);
        let mut keep: Vec<TermId> = cand.into_iter().filter(|s| r1.contains(s) && r2.contains(s)).collect();
        keep.sort(); Ok(keep)
    })?;
    // the range lookups alone, many repetitions
    let reps = 20_000;
    let t = Instant::now(); let mut sink = 0usize;
    for i in 0..reps { let d = (i % 7) as i64; let a = side1.partition_point(|x| x.0 < lo1 + d); let b = side1.partition_point(|x| x.0 <= hi1 + d); sink += b - a; }
    let lookup_ns = t.elapsed().as_nanos() as f64 / reps as f64;
    // decode-and-filter over the whole predicate, for reference only (not what the engine does)
    let (whole_ms, whole) = best(3, || {
        let r = rows(&snap, Perm::Pso, &[n1])?; let objects: Vec<TermId> = r.iter().map(|x| x[2]).collect();
        Ok(int_value(&snap, &objects)?.iter().flatten().filter(|v| (lo1..=hi1).contains(v)).count())
    })?;
    let cands = candidates(&snap)?.len();
    let fields: Vec<(&str, String)> = vec![
        ("snapshot", format!("\"{}\"", a[1])), ("bound_values", format!("[{v1}, {v2}]")), ("windows", format!("[[{lo1}, {hi1}], [{lo2}, {hi2}]]")),
        ("engine_q010_ms", format!("{full_ms:.3}")), ("engine_q010_rows", full_rows.to_string()),
        ("engine_without_numeric_ms", format!("{nonum_ms:.3}")), ("engine_without_numeric_rows", nonum_rows.to_string()),
        ("engine_numeric_part_ms", format!("{:.3}", full_ms - nonum_ms)),
        ("candidates_sharing_a_feature", cands.to_string()),
        ("today_shape_ms", format!("{today_ms:.3}")), ("today_shape_matches", today.len().to_string()),
        ("sidecar_plan_ms", format!("{side_ms:.3}")), ("sidecar_plan_matches", sided.len().to_string()), ("same_products", (today == sided).to_string()),
        ("sidecar_rows", format!("[{}, {}]", side1.len(), side2.len())), ("sidecar_build_ms", format!("[{build1_ms:.1}, {build2_ms:.1}]")),
        ("sidecar_bytes_per_row", std::mem::size_of::<(i64, TermId)>().to_string()),
        ("sidecar_bytes", ((side1.len() + side2.len()) * std::mem::size_of::<(i64, TermId)>()).to_string()),
        ("range_lookup_ns", format!("{lookup_ns:.1}")), ("range_lookup_sink", sink.to_string()),
        ("whole_predicate_decode_filter_ms", format!("{whole_ms:.3}")), ("whole_predicate_matches", whole.to_string()),
    ];
    let json = format!("{{\n{}\n}}\n", fields.iter().map(|(k, v)| format!("  \"{k}\": {v}")).collect::<Vec<_>>().join(",\n"));
    std::fs::write(&a[3], &json)?;
    print!("{json}");
    Ok(())
}
