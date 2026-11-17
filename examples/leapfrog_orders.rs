//! Force all six orders through the actual production leapfrog kernel.
//! DATA.nt NEW_OUTPUT_DIR Q2.rq Q9.rq; memory backend, one load, no decoding in timed kernels.
use model::TermId;
use rdfdb::exec::{Options, QueryResult, query_with};
use rdfdb::opt::{
    Stats,
    plan::{Slot as OptSlot, TriplePattern as OptPattern},
};
use rdfdb::{model, storage};
use sha2::{Digest, Sha256};
use spargebra::{
    Query, SparqlParser,
    algebra::GraphPattern,
    term::{NamedNodePattern, TermPattern},
};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{self, Write},
    path::Path,
    time::Instant,
};
use storage::{Dictionary, MemorySnapshot};
const UNBOUND: TermId = TermId(0);
fn storage_failure(e: storage::StorageError) -> ! {
    std::panic::panic_any(e.to_string())
}
// Include the source, not a copied algorithm or a new production routing API.
#[allow(dead_code)]
#[path = "../src/exec/bgp.rs"]
mod kernel;
const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

fn slot(
    t: &TermPattern,
    snap: &MemorySnapshot,
    vars: &mut BTreeMap<String, usize>,
) -> Result<kernel::Slot, Box<dyn Error>> {
    Ok(match t {
        TermPattern::Variable(v) => {
            let next = vars.len();
            kernel::Slot::Var(*vars.entry(v.as_str().to_owned()).or_insert(next))
        }
        TermPattern::NamedNode(n) => kernel::Slot::Const(Some(
            snap.lookup(&n.clone().into())?
                .ok_or_else(|| io::Error::other("absent query constant"))?,
        )),
        TermPattern::Literal(l) => kernel::Slot::Const(Some(
            snap.lookup(&l.clone().into())?
                .ok_or_else(|| io::Error::other("absent query literal"))?,
        )),
        TermPattern::BlankNode(_) => {
            return Err("blank-node benchmark variables unsupported".into());
        }
    })
}
fn encode(
    text: &str,
    snap: &MemorySnapshot,
) -> Result<
    (
        Vec<[kernel::Slot; 3]>,
        Vec<OptPattern>,
        BTreeMap<String, usize>,
    ),
    Box<dyn Error>,
> {
    let Query::Select { pattern, .. } = SparqlParser::new().parse_query(text)? else {
        return Err("SELECT expected".into());
    };
    let node = match &pattern {
        GraphPattern::Project { inner, .. } => inner.as_ref(),
        p => p,
    };
    let GraphPattern::Bgp { patterns } = node else {
        return Err("Only unmodified single BGP SELECT supported".into());
    };
    let mut vars = BTreeMap::new();
    let mut pats = Vec::new();
    for t in patterns {
        let s = slot(&t.subject, snap, &mut vars)?;
        let o = slot(&t.object, snap, &mut vars)?;
        let NamedNodePattern::NamedNode(p) = &t.predicate else {
            return Err("Variable predicate unsupported".into());
        };
        let p = kernel::Slot::Const(Some(
            snap.lookup(&p.clone().into())?
                .ok_or_else(|| io::Error::other("absent predicate"))?,
        ));
        pats.push([s, p, o]);
    }
    if vars.len() != 3 || !kernel::cyclic(&pats) || !pats.iter().all(kernel::no_repeated_var) {
        return Err("Benchmark requires exactly three variables in a cyclic BGP without repeated-variable triples".into());
    }
    let cv = |s: kernel::Slot| match s {
        kernel::Slot::Var(v) => OptSlot::Var(v as u32),
        kernel::Slot::Const(Some(t)) => OptSlot::Const(t),
        _ => unreachable!(),
    };
    let opt = pats
        .iter()
        .map(|p| OptPattern {
            s: cv(p[0]),
            p: cv(p[1]),
            o: cv(p[2]),
        })
        .collect();
    Ok((pats, opt, vars))
}
fn normalized(flat: &[TermId]) -> Vec<[u64; 3]> {
    assert_eq!(flat.len() % 3, 0);
    let mut rows: Vec<_> = flat
        .chunks_exact(3)
        .map(|r| [r[0].0, r[1].0, r[2].0])
        .collect();
    rows.sort_unstable();
    rows
}
fn hash(rows: &[[u64; 3]]) -> String {
    let mut h = Sha256::new();
    for r in rows {
        for x in r {
            h.update(x.to_le_bytes());
        }
    }
    format!("{:x}", h.finalize())
}
fn main() -> Result<(), Box<dyn Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() < 4 {
        return Err("usage: leapfrog_orders DATA.nt NEW_OUTPUT_DIR QUERY.rq...".into());
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
    let mut raw = fs::File::create(output.join("samples.tsv"))?;
    writeln!(
        raw,
        "query\tround\tposition\torder\tchosen\twarmup\tkernel_ns\trows\tid_multiset_sha256"
    )?;
    raw.flush()?;
    for file in &a[3..] {
        let text = fs::read_to_string(file)?;
        let name = Path::new(file).file_stem().unwrap().to_string_lossy();
        let (pats, opt, vars) = encode(&text, &snap)?;
        use rdfdb::opt::planner::{
            MAX_RUNTIME_DP_PATTERNS, MIN_OPTIMIZER_WORK, choose_variable_order,
            estimated_greedy_work,
        };
        let work = estimated_greedy_work(&opt, &stats);
        if opt.len() > MAX_RUNTIME_DP_PATTERNS || work < MIN_OPTIMIZER_WORK {
            return Err("Runtime does not choose the statistics order for this query; do not mislabel a forced order".into());
        }
        let chosen: Vec<usize> = choose_variable_order(&opt, &stats)
            .into_iter()
            .map(|v| v as usize)
            .collect();
        if chosen.len() != 3 || !ORDERS.iter().any(|o| o.as_slice() == chosen.as_slice()) {
            return Err("Invalid chosen order".into());
        }
        let full = Instant::now();
        let QueryResult::Ids(sol) = query_with(
            &snap,
            &text,
            &Options {
                stats: Some(&stats),
                wcoj: true,
                raw_ids: true,
            },
        )?
        else {
            return Err("Expected ordinary raw-ID SELECT results".into());
        };
        let query_ns = full.elapsed().as_nanos();
        if sol.vars.len() != 3 {
            return Err("Projection must retain all three variables".into());
        }
        let mut canonical = Vec::with_capacity(sol.ids.len());
        let mut indices = vec![0; 3];
        for (name, v) in &vars {
            indices[*v] = sol
                .vars
                .iter()
                .position(|q| q.as_str() == name)
                .ok_or_else(|| io::Error::other("Projection omitted a BGP variable"))?;
        }
        for r in sol.ids.chunks_exact(3) {
            for &i in &indices {
                canonical.push(r[i]);
            }
        }
        let reference = normalized(&canonical);
        drop(sol);
        drop(canonical);
        let expected_hash = hash(&reference);
        writeln!(
            metadata,
            "query={name} vars={vars:?} chosen={chosen:?} gate_work={work:.9} normal_query_ns={query_ns} rows={} id_hash={expected_hash}",
            reference.len()
        )?;
        metadata.flush()?;
        let bgp = kernel::Bgp {
            snap: &snap,
            w: 3,
            profile_plan: false,
        };
        // One complete warm-up pass, then six rotated/reversed measured passes.
        // Keep all completed rows flushed in case an order is unexpectedly expensive.
        for round in 0..7 {
            for position in 0..6 {
                let index = if round % 2 == 0 {
                    (position + round) % 6
                } else {
                    (5 - position + round) % 6
                };
                let order = ORDERS[index];
                let start = Instant::now();
                let result = bgp.leapfrog(&pats, &[UNBOUND; 3], Some(order.to_vec()));
                let ns = start.elapsed().as_nanos();
                let result = normalized(&result);
                if result != reference {
                    return Err(format!("Complete ID multiset mismatch: {name} {order:?}").into());
                }
                let order_text = format!("{}{}{}", order[0], order[1], order[2]);
                writeln!(
                    raw,
                    "{name}\t{round}\t{position}\t{order_text}\t{}\t{}\t{ns}\t{}\t{expected_hash}",
                    order.as_slice() == chosen.as_slice(),
                    round == 0,
                    result.len()
                )?;
                raw.flush()?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_forced_orders_match_ordinary_multiset_on_a_small_cycle() {
        let data =
            b"<urn:a> <urn:p> <urn:b> .\n<urn:b> <urn:q> <urn:c> .\n<urn:c> <urn:r> <urn:a> .\n";
        let snap = MemorySnapshot::from_ntriples(&data[..]).unwrap();
        let (pats, _, _) = encode(
            "SELECT * WHERE {?a <urn:p> ?b . ?b <urn:q> ?c . ?c <urn:r> ?a}",
            &snap,
        )
        .unwrap();
        let bgp = kernel::Bgp {
            snap: &snap,
            w: 3,
            profile_plan: false,
        };
        let QueryResult::Ids(ordinary) = query_with(
            &snap,
            "SELECT ?a ?b ?c WHERE {?a <urn:p> ?b . ?b <urn:q> ?c . ?c <urn:r> ?a}",
            &Options {
                stats: None,
                wcoj: true,
                raw_ids: true,
            },
        )
        .unwrap() else {
            panic!("IDs expected")
        };
        let reference = normalized(&ordinary.ids);
        assert_eq!(reference.len(), 1);
        for order in ORDERS {
            assert_eq!(
                normalized(&bgp.leapfrog(&pats, &[UNBOUND; 3], Some(order.to_vec()))),
                reference
            );
        }
    }
    #[test]
    fn modifiers_are_rejected_for_forced_kernel_comparison() {
        let snap = MemorySnapshot::from_ntriples(b"<urn:a> <urn:p> <urn:b> .".as_slice()).unwrap();
        assert!(encode("SELECT DISTINCT * WHERE {?a <urn:p> ?b}", &snap).is_err());
        assert!(encode("SELECT * WHERE {?a <urn:p> ?b} LIMIT 1", &snap).is_err());
    }
}
