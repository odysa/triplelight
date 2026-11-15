//! Opt-in fixed-work sampling + measured engine calibration. No serving-path changes.
use oxrdf::Term;
use rdfdb::opt::plan::{Slot, TriplePattern};
use rdfdb::opt::{Stats, q_error, sampling::*};
use rdfdb::storage::{DiskSnapshot, MemorySnapshot, Snapshot};
use spargebra::{
    Query, SparqlParser,
    algebra::GraphPattern,
    term::{NamedNodePattern, TermPattern},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::{self, BufReader},
    time::{Duration, Instant},
};
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
        return Err("usage: sampling_probe DATA(.nt|disk-dir) QUERY_DIR OUTPUT.tsv".into());
    }
    if std::path::Path::new(&a[1]).is_dir() {
        run(&DiskSnapshot::open(&a[1])?, &a)
    } else {
        run(
            &MemorySnapshot::from_ntriples(BufReader::new(fs::File::open(&a[1])?))?,
            &a,
        )
    }
}
fn run<S: Snapshot>(s: &S, a: &[String]) -> Result<(), Box<dyn Error>> {
    let stats = Stats::from_snapshot(s)?;
    let mut files: Vec<_> = fs::read_dir(&a[2])?
        .map(|p| p.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|p| p.extension().is_some_and(|x| x == "rq"));
    files.sort();
    let mut output = String::from(
        "query\tseed\tactual\tstatic_rows\tstatic_qerror\tsample_rows\tsample_qerror\tse\tsuccesses\tsample_us\tprobes\tgate_complete\tgate_us\twcoj_off_us\twcoj_on_us\n",
    );
    for f in files {
        let text = fs::read_to_string(&f)?;
        let name = f.file_stem().unwrap().to_string_lossy();
        let Some(patterns) = encode(&text, s)? else {
            eprintln!("{name}: absent constant, sampling skipped");
            continue;
        };
        let sizes: Vec<_> = patterns
            .iter()
            .map(|p| {
                let slots = [p.s, p.p, p.o];
                let perm = rdfdb::model::Perm::ALL
                    .into_iter()
                    .max_by_key(|p| {
                        p.order()
                            .iter()
                            .take_while(|&&i| matches!(slots[i], Slot::Const(_)))
                            .count()
                    })
                    .unwrap();
                let prefix: Vec<_> = perm
                    .order()
                    .iter()
                    .map_while(|&i| {
                        if let Slot::Const(id) = slots[i] {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect();
                s.count(perm, &prefix)
            })
            .collect::<Result<_, _>>()?;
        let mut order = Vec::new();
        let mut bound = BTreeSet::new();
        while order.len() < patterns.len() {
            let i = (0..patterns.len())
                .filter(|i| !order.contains(i))
                .min_by_key(|&i| {
                    let p = patterns[i];
                    let connected = [p.s, p.p, p.o]
                        .iter()
                        .filter(|x| matches!(x,Slot::Var(v) if bound.contains(v)))
                        .count();
                    (std::cmp::Reverse(connected), sizes[i], i)
                })
                .unwrap();
            order.push(i);
            let p = patterns[i];
            for x in [p.s, p.p, p.o] {
                if let Slot::Var(v) = x {
                    bound.insert(v);
                }
            }
        }
        let mut cached = stats.clone();
        cached.cache_pattern_counts(&patterns, s)?;
        let static_rows = cached.estimate_bgp(&patterns);
        let mut samples = Vec::new();
        let mut gated = Vec::new();
        for seed in 0..5 {
            let options = WalkOptions {
                seed: 20261115 + seed,
                max_time: Duration::from_secs(5),
                max_probes: 65536,
                ..Default::default()
            };
            if let WalkOutcome::Complete(e) = estimate_walks(s, &patterns, &order, options)? {
                samples.push(e);
            } else {
                return Err(format!("research sample failed for {name}").into());
            }
            match estimate_walks(
                s,
                &patterns,
                &order,
                WalkOptions {
                    seed: 20261115 + seed,
                    ..Default::default()
                },
            )? {
                WalkOutcome::Complete(e) => gated.push((true, e.elapsed.as_micros())),
                WalkOutcome::BudgetExceeded { elapsed, .. } => {
                    gated.push((false, elapsed.as_micros()))
                }
                WalkOutcome::UnsupportedRank => return Err("rank not supported".into()),
            }
        }
        let mut times = [Vec::new(), Vec::new()];
        let mut canonical = None;
        let mut actual = 0;
        for repetition in 0..5 {
            for mode in if repetition % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                let result = rdfdb::exec::query_with(
                    s,
                    &text,
                    &rdfdb::exec::Options {
                        stats: Some(&stats),
                        wcoj: mode == 1,
                        raw_ids: true,
                    },
                )?;
                let us = start.elapsed().as_micros();
                let rdfdb::exec::QueryResult::Ids(sol) = result else {
                    return Err("expected raw ID solutions".into());
                };
                let mut rows: Vec<_> = sol
                    .ids
                    .chunks_exact(sol.width())
                    .map(|r| r.to_vec())
                    .collect();
                rows.sort();
                actual = rows.len();
                if let Some(ref expected) = canonical {
                    if expected != &rows {
                        return Err(format!("engine disagreement for {name}").into());
                    }
                } else {
                    canonical = Some(rows);
                }
                times[mode].push(us);
            }
        }
        for (seed, e) in samples.iter().enumerate() {
            use std::fmt::Write;
            writeln!(
                output,
                "{name}\t{seed}\t{actual}\t{static_rows}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                q_error(static_rows, actual as u64),
                e.rows,
                q_error(e.rows, actual as u64),
                e.standard_error,
                e.successful_walks,
                e.elapsed.as_micros(),
                e.probes,
                gated[seed].0,
                gated[seed].1,
                times[0][seed],
                times[1][seed]
            )?;
        }
        eprintln!(
            "{name}: actual={actual} binary/leapfrog {:?}/{:?}",
            times[0], times[1]
        );
        fs::write(&a[3], &output)?;
    }
    Ok(())
}
