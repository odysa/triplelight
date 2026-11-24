use triplelight::embedded::Database;
use triplelight::extension::{Extension, ExtensionError, Registrar, Validation, Validator};
use triplelight::rdf::Triple;

#[test]
fn official_extension_availability_matches_the_build() {
    let db = Database::in_memory();
    let entries = db.list_extensions();
    for (name, compiled_in) in [
        ("rdfs", cfg!(feature = "rdfs")),
        ("shacl", cfg!(feature = "shacl")),
    ] {
        let entry = entries.iter().find(|entry| entry.name == name).unwrap();
        assert_eq!(entry.compiled_in, compiled_in);
        assert!(!entry.loaded);
        if !compiled_in {
            assert!(matches!(
                db.load_extension(name),
                Err(ExtensionError::Unsupported(message)) if message.contains(name) && message.contains("Cargo feature"),
            ));
        }
    }
    assert!(
        matches!(db.load_extension("missing"), Err(ExtensionError::Unsupported(message)) if message.contains("missing"))
    );
}

#[cfg(feature = "rdfs")]
#[test]
fn rdfs_autoload_is_local_to_the_database_and_preserves_existing_snapshots() {
    use triplelight::embedded::DatabaseError;
    use triplelight::exec::{Entailment, Interrupt, QueryLimits, QueryResult};

    let db = Database::in_memory();
    let other = Database::in_memory();
    db.update("INSERT DATA { <urn:p> <http://www.w3.org/2000/01/rdf-schema#domain> <urn:Class> . <urn:s> <urn:p> <urn:o> }").unwrap();
    let old = db.read();
    let ask = "ASK { <urn:s> a <urn:Class> }";
    assert!(matches!(db.query(ask), Ok(QueryResult::Boolean(false))));
    assert!(matches!(
        old.query_with_entailment(ask, Entailment::Rdfs),
        Ok(QueryResult::Boolean(true))
    ));
    assert!(old.explain(ask, Entailment::Rdfs).is_ok());
    assert!(!db.list_extensions().iter().any(|entry| entry.loaded));
    let json = db
        .query_json_with_limits(ask, Entailment::Rdfs, QueryLimits::default())
        .unwrap();
    assert!(json.contains("\"boolean\":true"));
    assert!(matches!(
        db.query_with_entailment(ask, Entailment::Rdfs),
        Ok(QueryResult::Boolean(true))
    ));
    db.load_extension("rdfs").unwrap();
    assert_eq!(
        db.list_extensions()
            .iter()
            .filter(|entry| entry.name == "rdfs" && entry.loaded)
            .count(),
        1
    );
    assert!(!old.list_extensions().iter().any(|entry| entry.loaded));
    assert!(!other.list_extensions().iter().any(|entry| entry.loaded));
    assert!(matches!(
        old.query_with_extension(ask, "rdfs"),
        Ok(QueryResult::Boolean(true))
    ));
    assert!(matches!(
        db.read().query_with_extension(ask, "rdfs"),
        Ok(QueryResult::Boolean(true))
    ));
    let interrupt = Interrupt::new();
    interrupt.cancel();
    let limits = QueryLimits {
        interrupt: Some(&interrupt),
        memory: None,
    };
    assert!(matches!(
        db.read()
            .query_with_extension_and_limits(ask, "rdfs", limits),
        Err(DatabaseError::Interrupted { timed_out: false })
    ));
}

#[cfg(feature = "shacl")]
#[test]
fn shacl_autoload_returns_a_report() {
    use triplelight::rdf::{Format, parse_triples};

    let data = parse_triples(b"<urn:s> <urn:p> <urn:o> .", Format::Turtle).unwrap();
    let shapes = parse_triples(
        br#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        <urn:shape> a sh:NodeShape; sh:targetNode <urn:s>;
            sh:property [ sh:path <urn:required>; sh:minCount 1 ] .
    "#,
        Format::Turtle,
    )
    .unwrap();
    let db = Database::in_memory();
    let result = db.validate_with_extension("shacl", &data, &shapes).unwrap();
    assert!(!result.conforms);
    assert!(!result.report.is_empty());
    assert!(
        db.list_extensions()
            .iter()
            .any(|entry| entry.name == "shacl" && entry.loaded)
    );
}

struct FixtureValidator;
impl Validator for FixtureValidator {
    fn validate(&self, data: &[Triple], _: &[Triple]) -> Result<Validation, ExtensionError> {
        Ok(Validation {
            conforms: data.is_empty(),
            report: Vec::new(),
        })
    }
}

struct FixtureExtension {
    fail: bool,
}
impl Extension for FixtureExtension {
    fn name(&self) -> &str {
        "fixture"
    }
    fn register(&self, registrar: &mut Registrar) -> Result<(), ExtensionError> {
        registrar.validator(FixtureValidator)?;
        if self.fail {
            return Err(ExtensionError::InvalidRegistration(
                "fixture rejected".into(),
            ));
        }
        Ok(())
    }
}

#[test]
fn a_failed_registration_publishes_no_capability() {
    let db = Database::in_memory();
    assert!(
        db.register_extension(&FixtureExtension { fail: true })
            .is_err()
    );
    assert!(db.validate_with_extension("fixture", &[], &[]).is_err());
    db.register_extension(&FixtureExtension { fail: false })
        .unwrap();
    assert!(matches!(
        db.register_extension(&FixtureExtension { fail: false }),
        Err(ExtensionError::InvalidRegistration(message)) if message.contains("already registered"),
    ));
    assert!(
        db.validate_with_extension("fixture", &[], &[])
            .unwrap()
            .conforms
    );
}
