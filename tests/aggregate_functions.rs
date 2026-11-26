use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use triplelight::embedded::{Database, DatabaseError};
use triplelight::exec::{Entailment, Interrupt, QueryLimits, QueryResult};
use triplelight::extension::{
    AggregateState, Extension, ExtensionError, FunctionOptions, Registrar,
};
use triplelight::rdf::{Literal, Term};

const IRI: &str = "urn:test:bound";

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

fn count() -> Option<Box<dyn AggregateState>> {
    Some(Box::new(Count::default()))
}

#[test]
fn custom_calls_use_one_state_per_group_and_the_captured_registry() {
    let db = Database::in_memory();
    let previous = db.read();
    db.register_aggregate_function(IRI, count).unwrap();
    let query = r#"
        PREFIX test: <urn:test:>
        SELECT ?g (test:bound(?x) AS ?n) (test:bound(DISTINCT ?x) AS ?unique)
        WHERE { VALUES (?g ?x) { (1 1) (1 1) (1 UNDEF) (1 2) (2 1) (2 UNDEF) } }
        GROUP BY ?g HAVING (test:bound(?x) > 0) ORDER BY DESC(test:bound(?x))
    "#;
    let current = db.read();
    let builtin = query.replace("test:bound(", "COUNT(");
    assert_eq!(
        current.query_json(query).unwrap(),
        current.query_json(&builtin).unwrap()
    );
    assert!(matches!(current.query_ids(query), Ok(QueryResult::Ids(_))));
    assert!(current.explain(query, Entailment::Simple).is_ok());
    assert!(matches!(
        previous.query("SELECT (<urn:test:bound>(?x) AS ?n) WHERE { VALUES ?x { 1 } }"),
        Err(DatabaseError::Unsupported(_))
    ));
    let empty = "SELECT (<urn:test:bound>(?x) AS ?n) WHERE { VALUES ?x {} }";
    assert_eq!(
        db.query_json(empty).unwrap(),
        db.query_json(&empty.replace("<urn:test:bound>", "COUNT"))
            .unwrap()
    );
}

#[test]
fn known_aggregates_are_unary_and_do_not_nest() {
    let db = Database::in_memory();
    db.register_aggregate_function(IRI, count).unwrap();
    for expression in [
        "<urn:test:bound>()",
        "<urn:test:bound>(1, 2)",
        "<urn:test:bound>(COUNT(1))",
        "COUNT(<urn:test:bound>(DISTINCT 1))",
    ] {
        let query = format!("SELECT ({expression} AS ?n) WHERE {{}}");
        assert!(db.query(&query).is_err(), "{expression}");
    }
    assert!(db.query("ASK { FILTER(<urn:test:bound>(1) > 0) }").is_err());
    assert!(matches!(
        db.query("SELECT (<urn:test:missing>(DISTINCT 1) AS ?n) WHERE {}"),
        Err(DatabaseError::Unsupported(_))
    ));
}

#[test]
fn a_custom_aggregate_in_update_rejects_the_whole_request() {
    let db = Database::in_memory();
    db.register_aggregate_function(IRI, count).unwrap();
    for call in ["<urn:test:bound>(?x)", "<urn:test:bound>(DISTINCT ?x)"] {
        let update = format!(
            "INSERT DATA {{ <urn:before> <urn:p> 1 }}; \
             INSERT {{ <urn:after> <urn:p> ?n }} WHERE {{ \
                 SELECT ({call} AS ?n) WHERE {{ VALUES ?x {{ 1 }} }} \
             }}"
        );
        assert!(
            matches!(db.update(&update), Err(DatabaseError::Unsupported(_))),
            "{call}"
        );
        assert!(
            matches!(
                db.query("ASK { ?s <urn:p> ?o }").unwrap(),
                QueryResult::Boolean(false)
            ),
            "{call}"
        );
    }
}

struct Collision;

impl Extension for Collision {
    fn name(&self) -> &str {
        "aggregate-collision"
    }

    fn register(&self, stage: &mut Registrar) -> Result<(), ExtensionError> {
        stage.aggregate_function("urn:test:unpublished", count)?;
        stage.function(IRI, FunctionOptions::default(), |_: &[Term]| None)
    }
}

#[test]
fn scalar_and_aggregate_registrations_have_one_namespace() {
    let db = Database::in_memory();
    db.register_aggregate_function(IRI, count).unwrap();
    assert!(matches!(
        db.register_extension(&Collision),
        Err(ExtensionError::InvalidRegistration(_))
    ));
    assert!(matches!(
        db.query("SELECT (<urn:test:unpublished>(1) AS ?n) WHERE {}"),
        Err(DatabaseError::Unsupported(_))
    ));
    db.register_function(
        "urn:test:scalar",
        FunctionOptions::default(),
        |_: &[Term]| None,
    )
    .unwrap();
    assert!(matches!(
        db.register_aggregate_function("urn:test:scalar", count),
        Err(ExtensionError::InvalidRegistration(_))
    ));
    assert!(matches!(
        db.register_aggregate_function("http://www.w3.org/2001/XMLSchema#integer", count),
        Err(ExtensionError::InvalidRegistration(_))
    ));
}

struct CancelOnStep {
    interrupt: Arc<Interrupt>,
    drops: Arc<AtomicUsize>,
}

impl AggregateState for CancelOnStep {
    fn step(&mut self, _: &Term) -> Option<()> {
        self.interrupt.cancel();
        Some(())
    }

    fn finish(&mut self) -> Option<Term> {
        panic!("cancelled group must not finish");
    }
}

impl Drop for CancelOnStep {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn cancellation_after_step_drops_the_accumulator_and_stops_the_query() {
    let db = Database::in_memory();
    let interrupt = Arc::new(Interrupt::new());
    let drops = Arc::new(AtomicUsize::new(0));
    let stop = Arc::clone(&interrupt);
    let state_drops = Arc::clone(&drops);
    db.register_aggregate_function(IRI, move || -> Option<Box<dyn AggregateState>> {
        Some(Box::new(CancelOnStep {
            interrupt: Arc::clone(&stop),
            drops: Arc::clone(&state_drops),
        }))
    })
    .unwrap();
    let q = "SELECT (<urn:test:bound>(?x) AS ?n) WHERE { VALUES ?x { 1 2 } }";
    let result = db.query_with_limits(
        q,
        Entailment::Simple,
        QueryLimits {
            interrupt: Some(&interrupt),
            memory: None,
        },
    );
    assert!(matches!(
        result,
        Err(DatabaseError::Interrupted { timed_out: false })
    ));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}
