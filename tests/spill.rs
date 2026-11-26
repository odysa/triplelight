//! GROUP BY spilling under the memory budget (#111): what happens when a spilled query is stopped, when the scratch
//! directory is unusable, and when the grouped answer itself does not fit.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use triplelight::embedded::{Database, DatabaseError};
use triplelight::exec::{Entailment, Interrupt, MemoryBudget, QueryLimits};
use triplelight::extension::{AggregateState, FunctionOptions};
use triplelight::rdf::{Literal, Term};

/// A fresh, empty scratch directory for one test, removed when it drops.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Scratch {
        let d = std::env::temp_dir().join(format!("tl-spill-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn entries(&self) -> usize { std::fs::read_dir(&self.0).unwrap().count() }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn db_with(rows: u32) -> Database {
    let db = Database::in_memory();
    let dir = std::env::temp_dir().join(format!("tl-spill-data-{rows}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let nt = dir.join("d.nt");
    let mut text = String::new();
    for i in 0..rows {
        text.push_str(&format!("<http://ex/s{i}> <http://ex/p> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i % 97));
    }
    std::fs::write(&nt, text).unwrap();
    db.load(&nt).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    db
}

/// A query cancelled while its spilled partitions are being grouped stops with `Cancelled` and leaves no run files.
/// The aggregate calls a function that cancels the query, so the stop lands inside `group_partitions`.
#[test]
fn a_query_cancelled_while_grouping_partitions_leaves_nothing_behind() {
    let db = db_with(5000);
    let interrupt = Arc::new(Interrupt::new());
    let stop = Arc::clone(&interrupt);
    db.register_function("urn:example:stop", FunctionOptions { arity: Some(1), pure: false }, move |args: &[Term]| {
        stop.cancel();
        Some(args[0].clone())
    }).unwrap();
    let scratch = Scratch::new("cancel");
    let budget = MemoryBudget::new(64 << 20).with_scratch_dir(&scratch.0).with_spill_rows(100);
    let q = "SELECT ?o (SUM(<urn:example:stop>(?o)) AS ?t) WHERE { ?s <http://ex/p> ?o } GROUP BY ?o";
    let got = db.query_with_limits(q, Entailment::Simple, QueryLimits { interrupt: Some(&interrupt), memory: Some(&budget) });
    assert!(matches!(got, Err(DatabaseError::Interrupted { timed_out: false })), "{got:?}");
    assert_eq!(scratch.entries(), 0, "the run directory is removed by the unwind");
    assert_eq!(budget.used(), 0);
}

/// A scratch path that is a file, not a directory: the spill fails as a storage error and creates nothing.
#[test]
fn an_unusable_scratch_directory_is_a_storage_error() {
    let db = db_with(2000);
    let scratch = Scratch::new("file");
    let file = scratch.0.join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let budget = MemoryBudget::new(64 << 20).with_scratch_dir(&file).with_spill_rows(100);
    // SUM, not COUNT(*): a COUNT(*) GROUP BY is answered from the index counts without grouping rows at all
    let q = "SELECT ?o (SUM(?o) AS ?t) WHERE { ?s <http://ex/p> ?o } GROUP BY ?o";
    let got = db.query_with_limits(q, Entailment::Simple, QueryLimits { interrupt: None, memory: Some(&budget) });
    assert!(matches!(got, Err(DatabaseError::Storage(_))), "{got:?}");
    assert_eq!(scratch.entries(), 1, "only the file itself");
    assert_eq!(budget.used(), 0);
}

#[test]
fn spilled_custom_aggregates_evaluate_each_group_once() {
    #[derive(Default)]
    struct Count(i64);
    impl AggregateState for Count {
        fn step(&mut self, _: &Term) -> Option<()> {
            self.0 += 1;
            Some(())
        }

        fn finish(&mut self) -> Option<Term> {
            Some(Literal::from(self.0).into())
        }
    }

    let db = db_with(4000);
    let scratch = Scratch::new("custom-aggregate");
    let partition_dir = scratch.0.clone();
    let states = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&states);
    db.register_aggregate_function(
        "urn:example:count",
        move || -> Option<Box<dyn AggregateState>> {
            std::fs::read_dir(&partition_dir).ok()?.next()?.ok()?;
            started.fetch_add(1, Ordering::Relaxed);
            Some(Box::new(Count::default()))
        },
    )
    .unwrap();
    let query = "SELECT ?o (<urn:example:count>(?s) AS ?total) \
                 (<urn:example:count>(DISTINCT ?o) AS ?unique) \
                 WHERE { ?s <http://ex/p> ?o } GROUP BY ?o ORDER BY ?o";
    let expected = db
        .query_json(&query.replace("<urn:example:count>", "COUNT"))
        .unwrap();
    let budget = MemoryBudget::new(64 << 20).with_scratch_dir(&scratch.0).with_spill_rows(100);
    let spilled = db
        .query_json_with_limits(
            query,
            Entailment::Simple,
            QueryLimits {
                interrupt: None,
                memory: Some(&budget),
            },
        )
        .unwrap();
    assert_eq!(spilled, expected);
    assert_eq!(states.load(Ordering::Relaxed), 2 * 97);
    assert_eq!(scratch.entries(), 0);
    assert_eq!(budget.used(), 0);
}

/// One group per row and no HAVING: the grouped answer is as large as the input, and the budget sees it. The groups
/// are built by spilling, but the merged answer does not fit, so the query ends with `ResourceLimit` (and cleans up).
#[test]
fn a_grouped_answer_too_large_for_the_budget_is_refused_after_spilling() {
    let db = Database::in_memory();
    let dir = Scratch::new("big-data");
    let nt = dir.0.join("d.nt");
    let text: String = (0..40_000u32).map(|i| format!("<http://ex/s{i}> <http://ex/p> \"v{i}\" .\n")).collect();
    std::fs::write(&nt, text).unwrap();
    db.load(&nt).unwrap();
    let scratch = Scratch::new("big");
    let budget = MemoryBudget::new(3 << 20).with_scratch_dir(&scratch.0);
    let q = "SELECT ?s (GROUP_CONCAT(?o) AS ?all) WHERE { ?s <http://ex/p> ?o } GROUP BY ?s";
    let got = db.query_with_limits(q, Entailment::Simple, QueryLimits { interrupt: None, memory: Some(&budget) });
    assert!(matches!(got, Err(DatabaseError::ResourceLimit(_))), "{got:?}");
    assert_eq!(scratch.entries(), 0);
    assert_eq!(budget.used(), 0);
}

/// A single-key COUNT with few groups runs in memory even when twice its input would not fit what is free: only the
/// group table is held, and it is small. The scratch path is a file, so any spill attempt would fail the query.
#[test]
fn a_count_with_few_groups_does_not_spill() {
    let db = db_with(20_000);
    let scratch = Scratch::new("few");
    let file = scratch.0.join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    // about 20k rows of 3 IDs (48 bytes each, about 1 MB): 2x the input exceeds 1.5 MB, the 97-group table does not
    let budget = MemoryBudget::new(1536 << 10).with_scratch_dir(&file);
    for q in [
        "SELECT ?o (COUNT(*) AS ?n) WHERE { ?s <http://ex/p> ?o } GROUP BY ?o",
        "SELECT ?o (SUM(?o) AS ?t) (MAX(?s) AS ?m) WHERE { ?s <http://ex/p> ?o } GROUP BY ?o",
    ] {
        let got = db.query_with_limits(q, Entailment::Simple, QueryLimits { interrupt: None, memory: Some(&budget) });
        assert!(got.is_ok(), "{q}: {got:?}");
    }
}
