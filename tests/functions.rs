//! SPARQL 1.1 functions added for hashes, NOW, RAND, UUID/STRUUID, BNODE() and the timezone/seconds accessors.
use triplelight::exec::{query, QueryResult};
use triplelight::storage::MemorySnapshot;

fn one(expr: &str) -> String {
    let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> <urn:b> .\n"[..]).unwrap();
    let q = format!("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ({expr} AS ?x) WHERE {{}}");
    match query(&s, &q).unwrap() {
        QueryResult::Solutions { rows, .. } => rows[0][0].as_ref().map_or("-".into(), |t| t.to_string()),
        r => panic!("{r:?}"),
    }
}

#[test]
fn hashes_match_reference_digests() {
    assert_eq!(one("MD5(\"abc\")"), "\"900150983cd24fb0d6963f7d28e17f72\"");
    assert_eq!(one("SHA1(\"abc\")"), "\"a9993e364706816aba3e25717850c26c9cd0d89d\"");
    assert_eq!(one("SHA256(\"abc\")"), "\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\"");
    assert!(one("SHA384(\"abc\")").starts_with("\"cb00753f45a35e8b"));
    assert!(one("SHA512(\"abc\")").starts_with("\"ddaf35a193617aba"));
    // Language-tagged and non-string arguments are type errors (unbound result).
    assert_eq!(one("MD5(\"abc\"@en)"), "-");
    assert_eq!(one("MD5(1)"), "-");
}

#[test]
fn timezone_tz_and_seconds() {
    let dt = |s: &str| format!("\"{s}\"^^xsd:dateTime");
    assert_eq!(one(&format!("TZ({})", dt("2011-01-10T14:45:13.815-05:00"))), "\"-05:00\"");
    assert_eq!(one(&format!("TZ({})", dt("2011-01-10T14:45:13Z"))), "\"Z\"");
    assert_eq!(one(&format!("TZ({})", dt("2011-01-10T14:45:13"))), "\"\"");
    assert_eq!(one(&format!("TIMEZONE({})", dt("2011-01-10T14:45:13.815-05:00"))), "\"-PT5H\"^^<http://www.w3.org/2001/XMLSchema#dayTimeDuration>");
    assert_eq!(one(&format!("TIMEZONE({})", dt("2011-01-10T14:45:13Z"))), "\"PT0S\"^^<http://www.w3.org/2001/XMLSchema#dayTimeDuration>");
    assert_eq!(one(&format!("TIMEZONE({})", dt("2011-01-10T14:45:13+05:30"))), "\"PT5H30M\"^^<http://www.w3.org/2001/XMLSchema#dayTimeDuration>");
    assert_eq!(one(&format!("TIMEZONE({})", dt("2011-01-10T14:45:13"))), "-");
    assert_eq!(one(&format!("SECONDS({})", dt("2011-01-10T14:45:13.815-05:00"))), "\"13.815\"^^<http://www.w3.org/2001/XMLSchema#decimal>");
}

#[test]
fn now_rand_uuid_bnode() {
    let now = one("NOW()");
    assert!(now.ends_with("Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>") && now.len() > 30, "{now}");
    let r = one("RAND()");
    assert!(r.ends_with("^^<http://www.w3.org/2001/XMLSchema#double>"), "{r}");
    let u = one("UUID()");
    assert!(u.starts_with("<urn:uuid:") && u.len() == "<urn:uuid:>".len() + 36 && u.as_bytes()[24] == b'4', "{u}");
    assert_eq!(one("STRLEN(STRUUID())"), "\"36\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    assert_eq!(one("isBlank(BNODE())"), "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>");
}

#[test]
fn volatile_functions_differ_per_row_and_now_is_constant() {
    let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> <urn:x> .\n<urn:b> <urn:p> <urn:x> .\n<urn:c> <urn:p> <urn:x> .\n"[..]).unwrap();
    // Same ?o in every row: a per-ID memo would wrongly give all three rows one UUID.
    match query(&s, "SELECT (CONCAT(STR(?o), STRUUID()) AS ?u) (NOW() AS ?n) WHERE { ?s <urn:p> ?o }").unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let us: std::collections::HashSet<String> = rows.iter().map(|r| r[0].as_ref().unwrap().to_string()).collect();
            let ns: std::collections::HashSet<String> = rows.iter().map(|r| r[1].as_ref().unwrap().to_string()).collect();
            assert_eq!((us.len(), ns.len()), (3, 1));
        }
        r => panic!("{r:?}"),
    }
}
