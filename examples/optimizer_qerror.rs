//! Compare pure SELECT-BGP estimates to external, exact result counts.
//! Usage: cargo run --release --example optimizer_qerror -- data.nt queries/ reference_counts.tsv
//! Reference TSV: query-name<TAB>row-count, e.g. q1<TAB>4. No DISTINCT/FILTER/LIMIT semantics are silently ignored.
use triplelight_rdf::Term;
use triplelight::opt::plan::{JoinTree, Slot, TriplePattern};
use triplelight::opt::{PlannerOptions, Stats, plan_bgp_with_options, q_error};
use triplelight::storage::{MemorySnapshot, Snapshot};
use triplelight::sparql::GraphPattern;
use triplelight::sparql::{NamedNodePattern, TermPattern};
use triplelight::sparql::Query;
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, BufReader};
use std::path::Path;
use std::time::Instant;

fn bgp(pattern: &GraphPattern) -> Result<Vec<triplelight::sparql::TriplePattern>, String> {
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
        // Triple-term patterns need the executor's per-solution unification, which this reference-count probe lacks.
        #[cfg(feature = "rdf-12")]
        TermPattern::Triple(_) => return Err(io::Error::new(io::ErrorKind::Unsupported, "triple-term patterns are not supported by this probe")),
    })
}

fn encode(
    text: &str,
    snapshot: &dyn Snapshot,
) -> Result<Option<Vec<TriplePattern>>, Box<dyn Error>> {
    let query = triplelight::sparql::parse_query(text, None)?;
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
    let args: Vec<_> = std::env::args().collect();
    let verify_wcoj = args.len() == 5 && args[4] == "--verify-wcoj";
    let verify = args.len() == 5 && args[4] == "--verify-executor" || verify_wcoj;
    let bench = args.len() == 5 && args[4] == "--bench-planner";
    if args.len() != 4 && !verify && !bench {
        return Err(io::Error::other(
            "usage: optimizer_qerror DATA.nt QUERY_DIR REFERENCE_COUNTS.tsv [--verify-executor|--verify-wcoj|--bench-planner]",
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
        stats.characteristic_set_count(),
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
            use triplelight::exec::{Options, QueryResult, query_with};
            let mut solutions = Vec::new();
            let modes = if verify_wcoj {
                vec![(None, false), (None, true), (Some(&stats), true)]
            } else {
                vec![(None, false), (Some(&stats), false)]
            };
            for (mode, wcoj) in modes {
                let QueryResult::Solutions { rows, .. } = query_with(
                    &snapshot,
                    &query,
                    &Options {
                        stats: mode,
                        wcoj,
                        raw_ids: false,
                        general_group: false,
                        count_pushdown: true,
                        range_pushdown: true,
                        char_pairs: true,
                        sorted_probe: true,
                        csr_paths: true,
                        entailment: Default::default(),
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
            if solutions.windows(2).any(|pair| pair[0] != pair[1]) {
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
    use triplelight::storage::Dictionary;
    #[derive(Debug)]
    struct PlantedReadFailure;
    impl std::fmt::Display for PlantedReadFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("planted dictionary read failure")
        }
    }
    impl std::error::Error for PlantedReadFailure {}
    struct FailingLookup<'a> {
        inner: &'a MemorySnapshot,
        fail: Term,
    }
    impl triplelight::storage::Dictionary for FailingLookup<'_> {
        fn lookup(&self, t: &Term) -> io::Result<Option<triplelight::model::TermId>> {
            if *t == self.fail {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    PlantedReadFailure,
                ))
            } else {
                self.inner.lookup(t)
            }
        }
        fn resolve(&self, id: triplelight::model::TermId) -> io::Result<Term> {
            self.inner.resolve(id)
        }
        fn with_term(
            &self,
            id: triplelight::model::TermId,
            f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>),
        ) -> io::Result<()> {
            self.inner.with_term(id, f)
        }
    }
    impl Snapshot for FailingLookup<'_> {
        fn scan<'a>(
            &'a self,
            p: triplelight::model::Perm,
            prefix: &[triplelight::model::TermId],
        ) -> Box<dyn Iterator<Item = io::Result<triplelight::model::TripleBatch>> + 'a> {
            self.inner.scan(p, prefix)
        }
        fn count(&self, p: triplelight::model::Perm, prefix: &[triplelight::model::TermId]) -> io::Result<u64> {
            self.inner.count(p, prefix)
        }
        fn seek(
            &self,
            p: triplelight::model::Perm,
            prefix: &[triplelight::model::TermId],
            target: triplelight::model::TermId,
        ) -> io::Result<Option<triplelight::model::TermId>> {
            self.inner.seek(p, prefix, target)
        }
    }
    #[test]
    fn lookup_failures_preserve_original_io_error_in_every_constant_position() {
        let snapshot =
            MemorySnapshot::from_ntriples("<urn:s> <urn:p> <urn:o> .".as_bytes()).unwrap();
        let named = |s| Term::NamedNode(triplelight_rdf::NamedNode::new(s).unwrap());
        let cases = [
            (named("urn:s"), "SELECT * WHERE { <urn:s> ?p ?o }"),
            (named("urn:p"), "SELECT * WHERE { ?s <urn:p> ?o }"),
            (named("urn:o"), "SELECT * WHERE { ?s ?p <urn:o> }"),
            (
                Term::Literal(triplelight_rdf::Literal::new_language_tagged_literal("broken", "en").unwrap()),
                "SELECT * WHERE { ?s ?p \"broken\"@en }",
            ),
        ];
        for (fail, query) in cases {
            let snapshot = FailingLookup {
                inner: &snapshot,
                fail,
            };
            let error = encode(query, &snapshot).unwrap_err();
            let io = error
                .downcast_ref::<io::Error>()
                .expect("original I/O error type");
            assert_eq!(io.kind(), io::ErrorKind::InvalidData);
            assert!(
                io.get_ref()
                    .unwrap()
                    .downcast_ref::<PlantedReadFailure>()
                    .is_some(),
                "original source lost"
            );
        }
        assert!(snapshot.lookup(&named("urn:missing")).unwrap().is_none());
        assert!(
            encode("SELECT * WHERE { ?s <urn:p> <urn:o> }", &snapshot)
                .unwrap()
                .is_some()
        );
    }
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
