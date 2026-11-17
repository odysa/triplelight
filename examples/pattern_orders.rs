//! Bounded connected pattern-order sensitivity through the complete evaluator.
use rdfdb::{
    exec::{Options, QueryResult, diagnostic::query_with_pattern_order, query_with},
    opt::Stats,
    storage::MemorySnapshot,
};
use sha2::{Digest, Sha256};
use spargebra::{
    Query, SparqlParser,
    algebra::GraphPattern,
    term::{NamedNodePattern, TermPattern},
};
use std::{collections::BTreeSet, error::Error, fs, io::Write, path::Path, time::Instant};

fn shapes(p: &GraphPattern) -> Result<Vec<BTreeSet<String>>, Box<dyn Error>> {
    match p {
        GraphPattern::Bgp { patterns } => Ok(patterns
            .iter()
            .map(|t| {
                let mut out = BTreeSet::new();
                for term in [&t.subject, &t.object] {
                    match term {
                        TermPattern::Variable(v) => {
                            out.insert(v.as_str().to_string());
                        }
                        TermPattern::BlankNode(b) => {
                            out.insert(format!("_b{}", b.as_str()));
                        }
                        _ => {}
                    }
                }
                if let NamedNodePattern::Variable(v) = &t.predicate {
                    out.insert(v.as_str().to_string());
                }
                out
            })
            .collect()),
        GraphPattern::Filter { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => shapes(inner),
        _ => Err("Expected one BGP with ordinary modifiers".into()),
    }
}
fn connected_orders(shapes: &[BTreeSet<String>]) -> Vec<Vec<usize>> {
    fn visit(
        shapes: &[BTreeSet<String>],
        order: &mut Vec<usize>,
        bound: &BTreeSet<String>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if order.len() == shapes.len() {
            out.push(order.clone());
            return;
        }
        let connected =
            (0..shapes.len()).any(|i| !order.contains(&i) && !shapes[i].is_disjoint(bound));
        for i in 0..shapes.len() {
            if order.contains(&i) || (connected && shapes[i].is_disjoint(bound)) {
                continue;
            }
            let mut next = bound.clone();
            next.extend(shapes[i].iter().cloned());
            order.push(i);
            visit(shapes, order, &next, out);
            order.pop();
        }
    }
    let mut out = Vec::new();
    visit(shapes, &mut Vec::new(), &BTreeSet::new(), &mut out);
    out
}
fn candidates(all: &[Vec<usize>], chosen: &[usize], salt: &[u8]) -> Vec<Vec<usize>> {
    let n = chosen.len();
    let mut buckets = vec![Vec::new(); n];
    for order in all {
        if order.as_slice() != chosen {
            buckets[order[0]].push(order.clone())
        }
    }
    for bucket in &mut buckets {
        bucket.sort_by_cached_key(|order| {
            let mut h = Sha256::new();
            h.update(salt);
            h.update(order.iter().map(|&i| i as u8).collect::<Vec<_>>());
            h.finalize().to_vec()
        });
    }
    let mut out = vec![chosen.to_vec()];
    for depth in 0..all.len() {
        for bucket in &buckets {
            if out.len() == 24 {
                return out;
            }
            if let Some(order) = bucket.get(depth) {
                out.push(order.clone())
            }
        }
        if buckets.iter().all(|b| depth >= b.len()) {
            break;
        }
    }
    out
}
#[derive(PartialEq, Eq)]
struct Canonical {
    vars: Vec<String>,
    rows: Vec<Vec<u64>>,
}
fn canonical(result: QueryResult) -> Result<Canonical, Box<dyn Error>> {
    let QueryResult::Ids(sol) = result else {
        return Err("Raw IDs expected".into());
    };
    if !sol.scratch.is_empty() {
        return Err("Computed projection terms need a separate canonical contract".into());
    }
    let width = sol.width();
    if width == 0 {
        return Err("Nonempty projection required".into());
    }
    let mut rows: Vec<_> = sol
        .ids
        .chunks_exact(width)
        .map(|r| r.iter().map(|t| t.0).collect::<Vec<_>>())
        .collect();
    rows.sort_unstable();
    Ok(Canonical {
        vars: sol.vars.iter().map(|v| v.as_str().to_owned()).collect(),
        rows,
    })
}
fn signature(rows: &Canonical) -> String {
    let mut h = Sha256::new();
    for name in &rows.vars {
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
    }
    for r in &rows.rows {
        for x in r {
            h.update(x.to_le_bytes());
        }
    }
    format!("{:x}", h.finalize())
}
fn main() -> Result<(), Box<dyn Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() < 4 {
        return Err("usage: pattern_orders DATA.nt NEW_OUTPUT_DIR QUERY.rq...".into());
    }
    let output = Path::new(&a[2]);
    fs::create_dir(output)?;
    let mut metadata = fs::File::create(output.join("metadata.txt"))?;
    let load = Instant::now();
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let snap = MemorySnapshot::from_ntriples_parallel_file(&a[1], workers)?;
    writeln!(
        metadata,
        "data={} workers={workers} load_s={:.9}",
        a[1],
        load.elapsed().as_secs_f64()
    )?;
    metadata.flush()?;
    let build = Instant::now();
    let stats = Stats::from_snapshot(&snap)?;
    writeln!(
        metadata,
        "triples={} stats_s={:.9}",
        stats.triple_count,
        build.elapsed().as_secs_f64()
    )?;
    metadata.flush()?;
    let options = Options {
        stats: Some(&stats),
        wcoj: true,
        raw_ids: true,
    };
    let mut samples = fs::File::create(output.join("samples.tsv"))?;
    writeln!(
        samples,
        "query\tround\tposition\tmode\torder\tchosen\twarmup\tevaluation_ns\trows\tid_multiset_sha256"
    )?;
    samples.flush()?;
    for file in &a[3..] {
        let text = fs::read_to_string(file)?;
        let name = Path::new(file).file_stem().unwrap().to_string_lossy();
        let Query::Select { pattern, .. } = SparqlParser::new().parse_query(&text)? else {
            return Err("SELECT expected".into());
        };
        let shape = shapes(&pattern)?;
        if !(2..=7).contains(&shape.len()) {
            return Err("Benchmark supports two to seven patterns".into());
        }
        let ordinary = Instant::now();
        let ordinary_result = query_with(&snap, &text, &options)?;
        let ordinary_total_ns = ordinary.elapsed().as_nanos();
        let reference = canonical(ordinary_result)?;
        let normal = query_with_pattern_order(&snap, &text, &options, None)?;
        let chosen = normal.pattern_order;
        if canonical(normal.result)? != reference {
            return Err("Diagnostic normal mode differs from ordinary query".into());
        }
        if chosen.len() != shape.len()
            || chosen.iter().copied().collect::<BTreeSet<_>>().len() != shape.len()
        {
            return Err("Ordinary execution stopped before a complete chosen order; report a censored comparison".into());
        }
        let all = connected_orders(&shape);
        let orders = candidates(&all, &chosen, text.as_bytes());
        let hash = signature(&reference);
        let source_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        writeln!(
            metadata,
            "query={name} source_sha256={source_hash} shapes={shape:?} chosen={chosen:?} chosen_connected={} connected_orders={} tested={} ordinary_query_ns={ordinary_total_ns} rows={} id_hash={hash}",
            all.contains(&chosen),
            all.len(),
            orders.len(),
            reference.rows.len()
        )?;
        writeln!(metadata, "query={name} candidates={orders:?}")?;
        metadata.flush()?;
        for round in 0..7 {
            let normal_control = |samples: &mut fs::File| -> Result<(), Box<dyn Error>> {
                let start = Instant::now();
                let result = query_with(&snap, &text, &options)?;
                let ns = start.elapsed().as_nanos();
                if canonical(result)? != reference {
                    return Err("Normal control output changed".into());
                }
                writeln!(
                    samples,
                    "{name}\t{round}\t-1\tnormal\tauto\tfalse\t{}\t{ns}\t{}\t{hash}",
                    round == 0,
                    reference.rows.len()
                )?;
                samples.flush()?;
                Ok(())
            };
            if round % 2 == 0 {
                normal_control(&mut samples)?;
            }
            for position in 0..orders.len() {
                let offset = (round * 7) % orders.len();
                let index = if round % 2 == 0 {
                    (position + offset) % orders.len()
                } else {
                    (orders.len() - 1 - position + offset) % orders.len()
                };
                let order = &orders[index];
                let start = Instant::now();
                let run = query_with_pattern_order(&snap, &text, &options, Some(order))?;
                let ns = start.elapsed().as_nanos();
                if run.pattern_order != *order {
                    return Err("Forced order was not fully executed".into());
                }
                let result = canonical(run.result)?;
                if result != reference {
                    return Err(format!("Full-query output differs: {name} {order:?}; investigate ORDER/LIMIT ties before interpreting").into());
                }
                let id = order
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    samples,
                    "{name}\t{round}\t{position}\tfixed\t{id}\t{}\t{}\t{ns}\t{}\t{hash}",
                    *order == chosen,
                    round == 0,
                    result.rows.len()
                )?;
                samples.flush()?;
            }
            if round % 2 == 1 {
                normal_control(&mut samples)?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_cap_preserves_chosen_and_balances_first_patterns() {
        let shape: Vec<_> = (0..6)
            .map(|i| BTreeSet::from(["s".to_owned(), format!("v{i}")]))
            .collect();
        let all = connected_orders(&shape);
        assert_eq!(all.len(), 720);
        let chosen = vec![0, 1, 2, 3, 4, 5];
        let selected = candidates(&all, &chosen, b"fixed");
        assert_eq!(selected.len(), 24);
        assert_eq!(selected[0], chosen);
        assert_eq!(selected.iter().collect::<BTreeSet<_>>().len(), 24);
        assert_eq!(
            selected.iter().map(|o| o[0]).collect::<BTreeSet<_>>().len(),
            6
        );
        assert_eq!(selected, candidates(&all, &chosen, b"fixed"));
    }
    #[test]
    fn connected_order_avoids_cartesian_extension_when_a_connected_step_exists() {
        let shape = vec![
            BTreeSet::from(["a".into(), "b".into()]),
            BTreeSet::from(["b".into(), "c".into()]),
            BTreeSet::from(["c".into(), "d".into()]),
        ];
        let all = connected_orders(&shape);
        assert!(!all.contains(&vec![0, 2, 1]));
        assert!(all.contains(&vec![0, 1, 2]));
    }
}
