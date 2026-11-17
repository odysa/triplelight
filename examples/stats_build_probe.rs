//! Shared baseline/candidate harness; no dependency on new coverage APIs.
use oxrdf::Term;
use rdfdb::opt::plan::{Slot, TriplePattern};
use rdfdb::opt::{PlannerOptions, Stats, plan_bgp_with_options};
use rdfdb::storage::{DiskSnapshot, Snapshot};
use sha2::{Digest, Sha256};
use spargebra::{
    Query, SparqlParser,
    algebra::GraphPattern,
    term::{NamedNodePattern, TermPattern},
};
use std::{collections::BTreeMap, error::Error, fs, io, time::Instant};
fn bgp(pattern: &GraphPattern) -> Result<Vec<spargebra::term::TriplePattern>, String> {
    match pattern{
 GraphPattern::Bgp{patterns}=>Ok(patterns.clone()),
 GraphPattern::Project{inner,..}=>bgp(inner),
 GraphPattern::Join{left,right}=>{let mut out=bgp(left)?;out.extend(bgp(right)?);Ok(out)},
 _=>Err("only pure BGP/Join/Project supported; query modifiers require a different reference-count contract".into()),
}
}
fn var(vars: &mut BTreeMap<String, u32>, name: String) -> Slot {
    let n = vars.len() as u32;
    Slot::Var(*vars.entry(name).or_insert(n))
}
fn term(
    s: &TermPattern,
    snapshot: &dyn Snapshot,
    vars: &mut BTreeMap<String, u32>,
) -> io::Result<Option<Slot>> {
    Ok(match s {
        TermPattern::Variable(v) => Some(var(vars, format!("v:{}", v.as_str()))),
        TermPattern::BlankNode(b) => Some(var(vars, format!("b:{}", b.as_str()))),
        TermPattern::NamedNode(n) => snapshot
            .lookup(&Term::NamedNode(n.clone()))?
            .map(Slot::Const),
        TermPattern::Literal(l) => snapshot.lookup(&Term::Literal(l.clone()))?.map(Slot::Const),
    })
}

fn encode(
    text: &str,
    snapshot: &dyn Snapshot,
) -> Result<Option<Vec<TriplePattern>>, Box<dyn Error>> {
    let query = SparqlParser::new().parse_query(text)?;
    let Query::Select { pattern, .. } = query else {
        return Err(io::Error::other("expected SELECT").into());
    };
    let triples = bgp(&pattern).map_err(io::Error::other)?;
    let mut vars = BTreeMap::new();
    let mut encoded = Vec::new();
    for triple in triples {
        let s = term(&triple.subject, snapshot, &mut vars)?;
        let o = term(&triple.object, snapshot, &mut vars)?;
        let p = match triple.predicate {
            NamedNodePattern::NamedNode(n) => {
                snapshot.lookup(&Term::NamedNode(n))?.map(Slot::Const)
            }
            NamedNodePattern::Variable(v) => Some(var(&mut vars, format!("v:{}", v.as_str()))),
        };
        let (Some(s), Some(p), Some(o)) = (s, p, o) else {
            return Ok(None);
        };
        encoded.push(TriplePattern { s, p, o });
    }
    Ok(Some(encoded))
}

fn main() -> Result<(), Box<dyn Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 4 {
        return Err("usage: stats_build_probe SNAPSHOT_DIR QUERY_DIR OUTPUT.tsv".into());
    }
    let snap = DiskSnapshot::open(&a[1])?;
    let mut output = String::from("build\tnanoseconds\ttriples\tsubjects\tcharacteristic_sets\n");
    let mut last = None;
    for iteration in 0..3 {
        drop(last.take());
        let start = Instant::now();
        let stats = Stats::from_snapshot(&snap)?;
        std::hint::black_box(&stats);
        let ns = start.elapsed().as_nanos();
        use std::fmt::Write;
        writeln!(
            output,
            "{iteration}\t{ns}\t{}\t{}\t{}",
            stats.triple_count,
            stats.subject_count,
            stats.characteristic_sets.len()
        )?;
        fs::write(&a[3], &output)?;
        last = Some(stats);
    }
    let stats = last.unwrap();
    let mut queries: Vec<_> = fs::read_dir(&a[2])?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    queries.retain(|p| p.extension().is_some_and(|e| e == "rq"));
    queries.sort();
    let mut signature = String::from("query\traw_rows_bits\tprepared_rows_bits\tplan_sha256\n");
    for p in queries {
        let name = p.file_stem().unwrap().to_string_lossy();
        let text = fs::read_to_string(&p)?;
        let (raw, prepared, tree) = if let Some(patterns) = encode(&text, &snap)? {
            let raw = stats.estimate_bgp(&patterns).to_bits();
            let mut cached = stats.clone();
            cached.cache_pattern_counts(&patterns, &snap)?;
            let estimate = cached.estimate_bgp(&patterns).to_bits();
            let plan = plan_bgp_with_options(&patterns, &cached, PlannerOptions::default());
            (raw, estimate, format!("{:?}", plan.tree))
        } else {
            (0, 0, "absent-constant".to_owned())
        };
        use std::fmt::Write;
        writeln!(
            signature,
            "{name}\t{raw:016x}\t{prepared:016x}\t{:x}",
            Sha256::digest(tree.as_bytes())
        )?;
    }
    fs::write(format!("{}.plans.tsv", a[3]), signature)?;
    Ok(())
}
