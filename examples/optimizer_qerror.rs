//! Compare pure SELECT-BGP estimates to external, exact result counts.
//! Usage: cargo run --release --example optimizer_qerror -- data.nt queries/ reference_counts.tsv
//! Reference TSV: query-name<TAB>row-count, e.g. q1<TAB>4. No DISTINCT/FILTER/LIMIT semantics are silently ignored.
use oxrdf::Term;
use rdfdb::opt::plan::{JoinTree, Slot, TriplePattern};
use rdfdb::opt::{PlannerOptions, Stats, plan_bgp_with_options, q_error};
use rdfdb::storage::{MemorySnapshot, Snapshot};
use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern};
use spargebra::{Query, SparqlParser};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, BufReader};
use std::path::Path;
use std::time::Instant;

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
) -> Option<Slot> {
    match s {
        TermPattern::Variable(v) => Some(var(vars, format!("v:{}", v.as_str()))),
        TermPattern::BlankNode(b) => Some(var(vars, format!("b:{}", b.as_str()))),
        TermPattern::NamedNode(n) => snapshot
            .lookup(&Term::NamedNode(n.clone()))
            .map(Slot::Const),
        TermPattern::Literal(l) => snapshot.lookup(&Term::Literal(l.clone())).map(Slot::Const),
    }
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
        let s = term(&triple.subject, snapshot, &mut vars);
        let o = term(&triple.object, snapshot, &mut vars);
        let p = match triple.predicate {
            NamedNodePattern::NamedNode(n) => snapshot.lookup(&Term::NamedNode(n)).map(Slot::Const),
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
    let args: Vec<_> = std::env::args().collect();
    let verify = args.len() == 5 && args[4] == "--verify-executor";
    let bench = args.len() == 5 && args[4] == "--bench-planner";
    if args.len() != 4 && !verify && !bench {
        return Err(io::Error::other(
            "usage: optimizer_qerror DATA.nt QUERY_DIR REFERENCE_COUNTS.tsv",
        )
        .into());
    }
    let start = Instant::now();
    let input = BufReader::new(fs::File::open(&args[1])?);
    let snapshot = if args[1].ends_with(".snapshot") {
        MemorySnapshot::read_snapshot(input)?
    } else if args[1].ends_with(".ttl") {
        MemorySnapshot::from_turtle(input)?
    } else {
        MemorySnapshot::from_ntriples(input)?
    };
    eprintln!("load_s={:.6}", start.elapsed().as_secs_f64());
    let t = Instant::now();
    let stats = Stats::from_snapshot(&snapshot)?;
    eprintln!(
        "triples={} subjects={} characteristic_sets={} stats_s={:.6}",
        stats.triple_count,
        stats.subject_count,
        stats.characteristic_sets.len(),
        t.elapsed().as_secs_f64()
    );
    let mut reference = BTreeMap::new();
    for line in fs::read_to_string(&args[3])?.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| io::Error::other("invalid reference TSV"))?;
        if reference
            .insert(key.to_string(), value.parse::<u64>()?)
            .is_some()
        {
            return Err(io::Error::other("duplicate reference query").into());
        }
    }
    let mut files: Vec<_> = fs::read_dir(Path::new(&args[2]))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|p| p.extension().is_some_and(|x| x == "rq" || x == "sparql"));
    files.sort();
    let names: std::collections::BTreeSet<_> = files
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    if names != reference.keys().cloned().collect() {
        return Err(io::Error::other("query and reference cohorts differ").into());
    }
    let mut output = String::from(
        "query\testimate\tactual\tq_error\tsearch\tphysical\testimate_without_exact_prefixes\n",
    );
    for file in files {
        let name = file.file_stem().unwrap().to_string_lossy();
        let actual = reference[name.as_ref()];
        let query = fs::read_to_string(&file)?;
        let encoded = encode(&query, &snapshot)?;
        let (estimate, raw, search, kind) = if let Some(patterns) = encoded {
            if bench {
                let mut samples = Vec::new();
                for _ in 0..7 {
                    let start = Instant::now();
                    let mut cached = std::hint::black_box(&stats).clone();
                    let cloned = start.elapsed().as_nanos();
                    cached.cache_pattern_counts(&patterns, &snapshot)?;
                    let prepared = start.elapsed().as_nanos();
                    let plan = plan_bgp_with_options(&patterns, &cached, PlannerOptions::default());
                    let planned = start.elapsed().as_nanos();
                    std::hint::black_box(&plan);
                    samples.push((cloned, prepared - cloned, planned - prepared, planned));
                }
                let median = |column: usize| {
                    let mut ns: Vec<_> = samples
                        .iter()
                        .map(|r| [r.0, r.1, r.2, r.3][column])
                        .collect();
                    ns.sort();
                    ns[ns.len() / 2]
                };
                eprintln!(
                    "planner_bench\t{name}\t{}\t{}\t{}\t{}",
                    median(0),
                    median(1),
                    median(2),
                    median(3)
                );
            }
            let raw = stats.estimate_bgp(&patterns);
            let mut cached = stats.clone();
            cached.cache_pattern_counts(&patterns, &snapshot)?;
            let decision = plan_bgp_with_options(&patterns, &cached, PlannerOptions::default());
            let kind = match decision.tree {
                JoinTree::Scan { .. } => "scan",
                JoinTree::IndexNestedLoop { .. } => "index_loop",
                JoinTree::MergeJoin { .. } => "merge",
                JoinTree::HashJoin { .. } => "hash",
                JoinTree::Leapfrog { .. } => "leapfrog",
                JoinTree::Unit => "unit",
                JoinTree::Empty => "empty",
            };
            (cached.estimate_bgp(&patterns), raw, decision.search, kind)
        } else {
            if actual != 0 {
                return Err(io::Error::other("absent query constant but reference count is positive: data/reference mismatch").into());
            }
            (0., 0., "absent-constant", "empty")
        };
        if verify {
            use rdfdb::exec::{Options, QueryResult, query_with};
            let mut solutions = Vec::new();
            for mode in [None, Some(&stats)] {
                let QueryResult::Solutions { rows, .. } = query_with(
                    &snapshot,
                    &query,
                    &Options {
                        stats: mode,
                        wcoj: false,
                    },
                )?
                else {
                    unreachable!()
                };
                let mut normalized: Vec<Vec<Option<String>>> = rows
                    .into_iter()
                    .map(|r| r.into_iter().map(|t| t.map(|t| t.to_string())).collect())
                    .collect();
                normalized.sort();
                if normalized.len() as u64 != actual {
                    return Err(io::Error::other(format!(
                        "executor/reference count mismatch: {name}"
                    ))
                    .into());
                }
                solutions.push(normalized);
            }
            if solutions[0] != solutions[1] {
                return Err(io::Error::other(format!("executor multiset mismatch: {name}")).into());
            }
            eprintln!("executor_verified={name} rows={actual}");
        }
        output.push_str(&format!(
            "{name}\t{estimate:.9}\t{actual}\t{}\t{search}\t{kind}\t{raw:.9}\n",
            q_error(estimate, actual)
        ));
    }
    print!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_are_not_silently_compared_to_bgp_counts() {
        let snapshot =
            MemorySnapshot::from_ntriples("<urn:s> <urn:p> <urn:o> .".as_bytes()).unwrap();
        assert!(encode("SELECT DISTINCT ?s WHERE { ?s <urn:p> ?o }", &snapshot).is_err());
        assert!(encode("SELECT ?s WHERE { ?s <urn:p> ?o } LIMIT 1", &snapshot).is_err());
        assert!(
            encode(
                "SELECT ?s WHERE { ?s <urn:p> ?o FILTER(?s = <urn:s>) }",
                &snapshot
            )
            .is_err()
        );
    }
    #[test]
    fn absent_constant_is_reported() {
        let snapshot =
            MemorySnapshot::from_ntriples("<urn:s> <urn:p> <urn:o> .".as_bytes()).unwrap();
        assert!(
            encode("SELECT ?s WHERE { ?s <urn:missing> ?o }", &snapshot)
                .unwrap()
                .is_none()
        );
    }
}
