//! `tl_query_arrow` against the existing `tl_query` view: identical kinds, lexical forms, datatypes and languages
//! per cell, unbound cells, empty and zero-column results, batch boundaries, lifetimes and error paths.
use std::ffi::c_char;
use std::ptr;

use arrow_array::ffi_stream::{RecordBatchStreamReader, FFI_ArrowArrayStream};
use arrow_array::{Array, LargeStringArray, RecordBatch, RecordBatchReader, StringArray, StructArray};

/// The C layout of `struct ArrowArrayStream`, to call and inspect its callbacks directly.
#[repr(C)]
struct RawStream {
    get_schema: Option<unsafe extern "C" fn(*mut RawStream, *mut std::ffi::c_void) -> i32>,
    get_next: Option<unsafe extern "C" fn(*mut RawStream, *mut std::ffi::c_void) -> i32>,
    get_last_error: Option<unsafe extern "C" fn(*mut RawStream) -> *const c_char>,
    release: Option<unsafe extern "C" fn(*mut RawStream)>,
    private_data: *mut std::ffi::c_void,
}
fn raw(s: &mut FFI_ArrowArrayStream) -> &mut RawStream { unsafe { &mut *(s as *mut FFI_ArrowArrayStream as *mut RawStream) } }
use triplelight_c::*;

/// (kind, value, datatype, language) or None for unbound.
type Cell = Option<(String, String, Option<String>, Option<String>)>;

fn db_with(nt: &str) -> *mut TlDb {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tl-arrow-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let f = dir.join("data.nt");
    std::fs::write(&f, nt).unwrap();
    let mut db = ptr::null_mut();
    unsafe {
        assert_eq!(tl_open_in_memory(&mut db, ptr::null_mut()), TL_OK);
        let p = f.to_str().unwrap();
        assert_eq!(tl_load(db, p.as_ptr() as *const c_char, p.len(), ptr::null_mut(), ptr::null_mut()), TL_OK);
    }
    db
}

fn via_view(db: *mut TlDb, q: &str) -> (Vec<String>, Vec<Vec<Cell>>) {
    unsafe {
        let mut r = ptr::null_mut();
        assert_eq!(tl_query(db, q.as_ptr() as *const c_char, q.len(), &mut r, ptr::null_mut()), TL_OK);
        let v = &*tl_result_get_view(r);
        let bytes = std::slice::from_raw_parts(v.bytes, v.n_bytes as usize);
        let s = |x: TlStr| String::from_utf8(bytes[x.off as usize..(x.off + x.len) as usize].to_vec()).unwrap();
        let vars: Vec<String> = std::slice::from_raw_parts(v.vars, v.n_vars as usize).iter().map(|x| s(*x)).collect();
        let terms = std::slice::from_raw_parts(v.terms, v.n_terms as usize);
        let cells = std::slice::from_raw_parts(v.cells, (v.n_rows * v.n_vars) as usize);
        let w = v.n_vars as usize;
        let mut rows = Vec::new();
        for r in 0..v.n_rows as usize {
            rows.push((0..w).map(|c| {
                let i = cells[r * w + c];
                if i == 0 { return None }
                let t = terms[i as usize - 1];
                let kind = match t.kind { TL_TERM_IRI => "iri", TL_TERM_BLANK => "bnode", _ => "literal" }.to_string();
                let opt = |x: TlStr| (x.len > 0).then(|| s(x));
                Some((kind, s(t.value), if t.kind == TL_TERM_LITERAL { Some(s(t.datatype)) } else { None }, opt(t.language)))
            }).collect());
        }
        tl_result_free(r);
        (vars, rows)
    }
}

fn stream(db: *mut TlDb, q: &str) -> Result<RecordBatchStreamReader, i32> {
    unsafe {
        let mut s = FFI_ArrowArrayStream::empty();
        let mut err = ptr::null_mut();
        let rc = tl_query_arrow(db, q.as_ptr() as *const c_char, q.len(), &mut s, &mut err);
        if rc != TL_OK { tl_error_free(err); return Err(rc) }
        Ok(RecordBatchStreamReader::try_new(s).unwrap())
    }
}

fn cells_of(batch: &RecordBatch) -> Vec<Vec<Cell>> {
    let mut rows = vec![vec![]; batch.num_rows()];
    for col in batch.columns() {
        let st = col.as_any().downcast_ref::<StructArray>().unwrap();
        let kind = st.column(0).as_any().downcast_ref::<StringArray>().unwrap();
        let value = st.column(1).as_any().downcast_ref::<LargeStringArray>().unwrap();
        let dt = st.column(2).as_any().downcast_ref::<StringArray>().unwrap();
        let lang = st.column(3).as_any().downcast_ref::<StringArray>().unwrap();
        for (r, row) in rows.iter_mut().enumerate() {
            row.push(st.is_valid(r).then(|| (kind.value(r).to_string(), value.value(r).to_string(),
                dt.is_valid(r).then(|| dt.value(r).to_string()), lang.is_valid(r).then(|| lang.value(r).to_string()))));
        }
    }
    rows
}

fn via_stream(db: *mut TlDb, q: &str) -> (Vec<String>, Vec<Vec<Cell>>, Vec<usize>) {
    let reader = stream(db, q).unwrap();
    let vars: Vec<String> = reader.schema().fields().iter().map(|f| f.name().clone()).collect();
    let (mut rows, mut sizes) = (Vec::new(), Vec::new());
    for b in reader {
        let b = b.unwrap();
        sizes.push(b.num_rows());
        rows.extend(cells_of(&b));
    }
    (vars, rows, sizes)
}

const TYPED: &str = r#"<urn:s> <urn:v> <urn:iri> .
<urn:s> <urn:v> "urn:iri" .
<urn:s> <urn:v> "hello"@en .
<urn:s> <urn:v> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:s> <urn:v> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:s> <urn:v> "true"^^<http://www.w3.org/2001/XMLSchema#boolean> .
<urn:s> <urn:v> "" .
<urn:s> <urn:v> "INF"^^<http://www.w3.org/2001/XMLSchema#double> .
<urn:s> <urn:v> "1.25"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<urn:s> <urn:v> _:b0 .
"#;

#[test]
fn typed_terms_and_unbound_cells_match_the_view() {
    let db = db_with(TYPED);
    for q in ["SELECT ?o WHERE { <urn:s> <urn:v> ?o }",
              "SELECT ?o ?missing WHERE { <urn:s> <urn:v> ?o OPTIONAL { ?o <urn:absent> ?missing } }",
              "SELECT ?o (STRLEN(STR(?o)) AS ?n) WHERE { <urn:s> <urn:v> ?o FILTER(isLiteral(?o)) }"] {
        let (v1, r1) = via_view(db, q);
        let (v2, r2, _) = via_stream(db, q);
        assert_eq!(v1, v2, "{q}");
        let mut a = r1.clone(); a.sort(); let mut b = r2.clone(); b.sort();
        assert_eq!(a, b, "{q}");
        assert_eq!(r1, r2, "row order preserved: {q}");
    }
    // Spot checks on distinctions a string rendering could hide.
    let (_, rows, _) = via_stream(db, "SELECT ?o WHERE { <urn:s> <urn:v> ?o }");
    let all: Vec<_> = rows.into_iter().map(|r| r[0].clone().unwrap()).collect();
    assert!(all.contains(&("iri".into(), "urn:iri".into(), None, None)));
    assert!(all.contains(&("literal".into(), "urn:iri".into(), Some("http://www.w3.org/2001/XMLSchema#string".into()), None)));
    assert!(all.contains(&("literal".into(), "".into(), Some("http://www.w3.org/2001/XMLSchema#string".into()), None)));
    assert!(all.contains(&("literal".into(), "hello".into(), Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString".into()), Some("en".into()))));
    assert!(all.contains(&("literal".into(), "01".into(), Some("http://www.w3.org/2001/XMLSchema#int".into()), None)));
    assert!(all.iter().any(|t| t.0 == "bnode"));
    unsafe { tl_close(db) }
}

#[test]
fn batch_boundaries_keep_every_row_and_typed_edge_cases() {
    for n in [BATCH_ROWS - 1, BATCH_ROWS, BATCH_ROWS + 1, 2 * BATCH_ROWS + 3] {
        let mut nt = String::new();
        for i in 0..n {
            // Every 4th row has an empty string, every 7th a language tag, others noncanonical ints; some unbound.
            let o = match i % 7 { 0 => format!("\"t{i}\"@en"), 3 => "\"\"".to_string(), _ => format!("\"0{i}\"^^<http://www.w3.org/2001/XMLSchema#int>") };
            nt.push_str(&format!("<urn:r{i:06}> <urn:p> {o} .\n"));
            if i % 5 == 0 { nt.push_str(&format!("<urn:r{i:06}> <urn:q> <urn:x{i}> .\n")) }
        }
        let db = db_with(&nt);
        let q = "SELECT ?s ?o ?x WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:q> ?x } } ORDER BY ?s";
        let (v1, r1) = via_view(db, q);
        let (v2, r2, sizes) = via_stream(db, q);
        assert_eq!(v1, v2);
        assert_eq!(r1.len(), n);
        assert_eq!(r1, r2, "n={n}");
        assert!(sizes.iter().all(|&s| s <= BATCH_ROWS) && sizes.iter().sum::<usize>() == n, "{sizes:?}");
        unsafe { tl_close(db) }
    }
}

#[test]
fn empty_and_zero_column_results_keep_cardinality() {
    let db = db_with(TYPED);
    let (vars, rows, _) = via_stream(db, "SELECT ?o WHERE { <urn:s> <urn:v> ?o FILTER(false) }");
    assert_eq!((vars, rows.len()), (vec!["o".to_string()], 0));
    // A projected but never-bound variable: ten rows, all null.
    let (vars, rows, _) = via_stream(db, "SELECT ?nothing WHERE { <urn:s> <urn:v> ?o }");
    assert_eq!(vars, vec!["nothing".to_string()]);
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().all(|r| r == &vec![None]));
    // No projected variables at all: the solution count must survive (one match, then ten matches).
    for (q, n) in [("SELECT * WHERE { <urn:s> <urn:v> <urn:iri> }", 1usize), ("SELECT * WHERE { <urn:s> <urn:v> [] }", 10)] {
        let (v1, r1) = via_view(db, q);
        assert_eq!((v1.len(), r1.len()), (0, n), "view: {q}");
        let reader = stream(db, q).unwrap();
        assert_eq!(reader.schema().fields().len(), 0, "{q}");
        let total: usize = reader.map(|b| { let b = b.unwrap(); assert_eq!(b.num_columns(), 0); b.num_rows() }).sum();
        assert_eq!(total, n, "{q}");
    }
    unsafe { tl_close(db) }
}

#[test]
fn errors_lifetimes_and_release() {
    let db = db_with(TYPED);
    unsafe {
        // Null out is rejected before anything runs; a failed call leaves *out untouched.
        let q = "SELECT ?o WHERE { <urn:s> <urn:v> ?o }";
        let mut err = ptr::null_mut();
        assert_eq!(tl_query_arrow(db, q.as_ptr() as *const c_char, q.len(), ptr::null_mut(), &mut err), TL_ERR_INVALID_ARGUMENT);
        tl_error_free(err);
        for (q, rc) in [("ASK { ?s ?p ?o }", TL_ERR_UNSUPPORTED), ("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }", TL_ERR_UNSUPPORTED), ("SELEKT", TL_ERR_SYNTAX)] {
            let mut s = FFI_ArrowArrayStream::empty();
            raw(&mut s).private_data = 0x5eed as *mut std::ffi::c_void; // sentinel; release stays NULL so drop is a no-op
            let mut err = ptr::null_mut();
            assert_eq!(tl_query_arrow(db, q.as_ptr() as *const c_char, q.len(), &mut s, &mut err), rc, "{q}");
            let r = raw(&mut s);
            assert!(r.release.is_none() && r.get_next.is_none() && r.private_data as usize == 0x5eed, "out untouched on failure");
            tl_error_free(err);
        }
        tl_close(db);
    }
    // Stream and batches outlive the database handle; release with a batch still unread.
    let mut nt = String::new();
    let n = BATCH_ROWS + 100;
    for i in 0..n { nt.push_str(&format!("<urn:r{i:06}> <urn:p> \"0{i}\"^^<http://www.w3.org/2001/XMLSchema#int> .\n")) }
    let db = db_with(&nt);
    let q = "SELECT ?s ?o WHERE { ?s <urn:p> ?o } ORDER BY ?s";
    let (_, expected) = via_view(db, q);
    let mut reader = stream(db, q).unwrap();
    unsafe { tl_close(db) };
    let first = reader.next().unwrap().unwrap();
    assert_eq!(first.num_rows(), BATCH_ROWS, "a second batch is still unread");
    drop(reader); // release the stream before reading its second batch
    assert_eq!(cells_of(&first), expected[..BATCH_ROWS].to_vec(), "retained batch outlives stream and database");
}

#[test]
fn double_release_is_harmless() {
    let db = db_with(TYPED);
    unsafe {
        let q = "SELECT ?o WHERE { <urn:s> <urn:v> ?o }";
        let mut s = FFI_ArrowArrayStream::empty();
        assert_eq!(tl_query_arrow(db, q.as_ptr() as *const c_char, q.len(), &mut s, ptr::null_mut()), TL_OK);
        let release = raw(&mut s).release.unwrap();
        release(raw(&mut s));
        assert!(raw(&mut s).release.is_none(), "release marks the stream released");
        // Releasing a released stream must not run the callback again (the consumer checks release == NULL);
        // dropping the Rust wrapper afterwards must also be a no-op.
        drop(s);
        tl_close(db);
    }
}
