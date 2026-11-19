//! Binding-overhead baseline: times the native Rust `Database::query` (decoded terms) and the C ABI `tl_query`
//! (the same query plus encoding the shared result buffer) on one in-memory database.
//!   cargo run --release --example overhead -- <data.nt> <reps>
use std::time::Instant;
use triplelight_c::*;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (nt, reps): (&str, usize) = (&a[1], a.get(2).map_or(20, |r| r.parse().unwrap()));
    let db = triplelight::embedded::Database::in_memory();
    db.load(nt).unwrap();
    while !db.stats_ready() { std::thread::sleep(std::time::Duration::from_millis(20)) }
    let mut h = std::ptr::null_mut();
    unsafe { tl_open_in_memory(&mut h, std::ptr::null_mut()) };
    unsafe { tl_load(h, nt.as_ptr() as _, nt.len(), std::ptr::null_mut(), std::ptr::null_mut()) };
    while unsafe { tl_stats_ready(h) } == 0 { std::thread::sleep(std::time::Duration::from_millis(20)) }
    for (name, q) in queries() {
        let rust = median(reps, || { let r = db.query(q).unwrap(); std::hint::black_box(&r); });
        let ids_only = median(reps, || { let r = db.read().query_ids(q).unwrap(); std::hint::black_box(&r); });
        let enc_terms = median(reps, || { std::hint::black_box(bench_encode(&db, q, false)); });
        let enc_ids = median(reps, || { std::hint::black_box(bench_encode(&db, q, true)); });
        let mut shape = (0u64, 0u64, 0u64);
        let cabi = median(reps, || unsafe {
            let mut r = std::ptr::null_mut();
            assert_eq!(tl_query(h, q.as_ptr() as _, q.len(), &mut r, std::ptr::null_mut()), TL_OK);
            let v = &*tl_result_get_view(r);
            shape = (v.n_rows, v.n_vars, v.n_terms);
            tl_result_free(r);
        });
        println!("{{\"lang\":\"rust\",\"query\":\"{name}\",\"rows\":{},\"cols\":{},\"distinct_terms\":{},\"native_query_ms\":{rust:.3},\"query_ids_ms\":{ids_only:.3},\"encode_via_terms_ms\":{enc_terms:.3},\"encode_via_ids_ms\":{enc_ids:.3},\"tl_query_ms\":{cabi:.3}}}", shape.0, shape.1, shape.2);
    }
    unsafe { tl_close(h) };
}

fn median(reps: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..3 { f() }
    let mut t: Vec<f64> = (0..reps).map(|_| { let s = Instant::now(); f(); s.elapsed().as_secs_f64() * 1e3 }).collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t[t.len() / 2]
}

pub fn queries() -> [(&'static str, &'static str); 4] {
    [
        ("lubm_q1", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:GraduateStudent . ?X ub:takesCourse <http://www.Department0.University0.edu/GraduateCourse0> }"),
        ("lubm_q14", "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> SELECT ?X WHERE { ?X rdf:type ub:UndergraduateStudent }"),
        ("wide_200k", "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 200000"),
        ("group_by_p", "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p"),
    ]
}
