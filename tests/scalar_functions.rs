use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use triplelight::embedded::{Database, DatabaseError};
use triplelight::exec::{self, Entailment, Interrupt, QueryLimits, QueryResult};
use triplelight::extension::{Extension, ExtensionError, FunctionOptions, Registrar};
use triplelight::rdf::{Literal, Term};

const INC: &str = "urn:example:inc";

fn inc(args: &[Term]) -> Option<Term> {
    let [Term::Literal(value)] = args else {
        return None;
    };
    let value = value.value().parse::<i64>().ok()?.checked_add(1)?;
    Some(Literal::from(value).into())
}

#[test]
fn scalar_matches_arithmetic_and_respects_captured_registry() {
    let db = Database::in_memory();
    let previous = db.read();
    let options = FunctionOptions {
        arity: Some(1),
        pure: true,
    };
    db.register_function(INC, options, inc).unwrap();

    let select =
        "SELECT (<urn:example:inc>(?x) AS ?result) WHERE { VALUES ?x { 1 2 1 } } ORDER BY ?result";
    assert!(matches!(
        previous.query(select),
        Err(DatabaseError::Unsupported(_))
    ));
    assert!(matches!(
        previous.query_ids(select),
        Err(DatabaseError::Unsupported(_))
    ));
    assert!(matches!(
        previous.explain(select, Entailment::Simple),
        Err(DatabaseError::Unsupported(_))
    ));
    let current = db.read();
    assert!(matches!(current.query_ids(select), Ok(QueryResult::Ids(_))));
    assert!(current.explain(select, Entailment::Simple).is_ok());

    for query in [
        select,
        "SELECT ?x ?result WHERE { VALUES ?x { 1 2 } BIND(<urn:example:inc>(?x) AS ?result) FILTER(<urn:example:inc>(?x) > 2) } ORDER BY <urn:example:inc>(?x)",
        "SELECT ?key (COUNT(*) AS ?n) WHERE { VALUES ?x { 1 2 1 } } GROUP BY (<urn:example:inc>(?x) AS ?key) ORDER BY ?key",
    ] {
        assert_eq!(
            current.query_json(query).unwrap(),
            current
                .query_json(&query.replace("<urn:example:inc>(?x)", "(?x + 1)"))
                .unwrap(),
            "{query}",
        );
    }
    assert!(matches!(
        db.query("ASK { FILTER(<urn:unknown>(1)) }"),
        Err(DatabaseError::Unsupported(_))
    ));
    for iri in [
        INC,
        "relative",
        "http://www.w3.org/2001/XMLSchema#integer",
        "urn:triplelight:text:matches",
    ] {
        assert!(matches!(
            db.register_function(iri, options, inc),
            Err(ExtensionError::InvalidRegistration(_))
        ));
    }
}

struct Collision;
impl Extension for Collision {
    fn name(&self) -> &str {
        "collision"
    }

    fn register(&self, stage: &mut Registrar) -> Result<(), ExtensionError> {
        stage.function("urn:example:other", FunctionOptions::default(), inc)?;
        stage.function(INC, FunctionOptions::default(), inc)
    }
}

#[test]
fn conflicting_extension_publishes_none_of_its_functions() {
    let db = Database::in_memory();
    db.register_function(INC, FunctionOptions::default(), inc)
        .unwrap();
    assert!(matches!(
        db.register_extension(&Collision),
        Err(ExtensionError::InvalidRegistration(_))
    ));
    assert!(
        !db.list_extensions()
            .iter()
            .any(|ext| ext.name == "collision")
    );
    assert!(matches!(
        db.query("ASK { FILTER(<urn:example:other>(1) = 2) }"),
        Err(DatabaseError::Unsupported(_))
    ));
}

#[test]
fn callback_errors_and_panics_follow_expression_semantics() {
    let db = Database::in_memory();
    db.register_function(
        INC,
        FunctionOptions {
            arity: Some(1),
            pure: false,
        },
        inc,
    )
    .unwrap();
    db.register_function(
        "urn:example:panic",
        FunctionOptions::default(),
        |_: &[Term]| -> Option<Term> { panic!("callback failed") },
    )
    .unwrap();

    let bind =
        "SELECT ?x ?y WHERE { VALUES ?x { 1 UNDEF \"wrong\" } BIND(<urn:example:inc>(?x) AS ?y) }";
    assert_eq!(
        db.query_json(bind).unwrap(),
        db.query_json(&bind.replace("<urn:example:inc>(?x)", "(?x + 1)"))
            .unwrap()
    );
    assert!(matches!(
        db.query("ASK { FILTER(<urn:example:inc>() = 2) }"),
        Ok(QueryResult::Boolean(false))
    ));
    assert!(matches!(
        db.query("ASK { FILTER(<urn:example:panic>()) }"),
        Ok(QueryResult::Boolean(false))
    ));
    assert!(matches!(
        db.query("ASK { FILTER(IF(true, true, <urn:example:panic>())) }"),
        Ok(QueryResult::Boolean(true))
    ));
}

#[test]
fn cancellation_propagates_after_a_running_callback_returns() {
    let db = Database::in_memory();
    let interrupt = Interrupt::new();
    let (entered, wait_entered) = std::sync::mpsc::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let wait_release = std::sync::Mutex::new(wait_release);
    db.register_function(
        "urn:example:slow",
        FunctionOptions::default(),
        move |_: &[Term]| {
            entered.send(()).unwrap();
            wait_release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Some(Literal::from(true).into())
        },
    )
    .unwrap();
    std::thread::scope(|scope| {
        let result = scope.spawn(|| {
            db.query_with_limits(
                "ASK { FILTER(<urn:example:slow>()) }",
                Entailment::Simple,
                QueryLimits {
                    interrupt: Some(&interrupt),
                    memory: None,
                },
            )
        });
        wait_entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        interrupt.cancel();
        release.send(()).unwrap();
        assert!(matches!(
            result.join().unwrap(),
            Err(DatabaseError::Interrupted { timed_out: false })
        ));
    });
}

#[test]
fn pure_memo_is_optional_and_impure_calls_are_evaluated_per_row() {
    use exec::udf::{FunctionLookup, QueryExtensions, RegisteredFunction};
    use triplelight::storage::MemorySnapshot;

    struct Functions(RegisteredFunction);
    impl FunctionLookup for Functions {
        fn scalar(&self, iri: &str) -> Option<&RegisteredFunction> {
            (iri == INC).then_some(&self.0)
        }
    }

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_in_callback = Arc::clone(&counter);
    let functions = Functions(RegisteredFunction::new(
        FunctionOptions {
            arity: Some(1),
            pure: true,
        },
        move |args: &[Term]| {
            counter_in_callback.fetch_add(1, Ordering::Relaxed);
            inc(args)
        },
    ));
    let snapshot = MemorySnapshot::from_ntriples(
        &b"<urn:s> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n"[..],
    )
    .unwrap();
    let query = "SELECT (<urn:example:inc>(?x) AS ?n) WHERE { ?s <urn:p> ?x . VALUES ?k { 1 2 3 4 } } ORDER BY ?k";
    let with = |disable_function_memo| {
        let extensions = QueryExtensions {
            functions: Some(&functions),
            disable_function_memo,
            ..Default::default()
        };
        let result = exec::query_with_extensions(&snapshot, None, query, &Default::default(), extensions).unwrap();
        exec::results::serialize(&result).1
    };
    let memoized = with(false);
    assert_eq!(counter.swap(0, Ordering::Relaxed), 1);
    assert_eq!(with(true), memoized);
    assert_eq!(counter.swap(0, Ordering::Relaxed), 4);

    let db = Database::in_memory();
    let counter_in_callback = Arc::clone(&counter);
    db.register_function(INC, FunctionOptions::default(), move |args: &[Term]| {
        counter_in_callback.fetch_add(1, Ordering::Relaxed);
        inc(args)
    })
    .unwrap();
    db.query("SELECT (SUM(<urn:example:inc>(?x)) AS ?n) WHERE { VALUES ?x { 1 1 1 1 } }")
        .unwrap();
    assert_eq!(counter.load(Ordering::Relaxed), 4);
}
