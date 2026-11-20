//! Fault injection at the Arrow C Stream callback boundary, and batch-local conversion of computed terms.
use std::ffi::CStr;
use std::sync::Arc;

use arrow_array::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::{Array, LargeStringArray, RecordBatch, RecordBatchOptions, RecordBatchReader, StructArray};
use arrow_schema::{ArrowError, Schema, SchemaRef};
use triplelight::embedded::Database;
use triplelight::exec::QueryResult;
use triplelight_c::arrow::IdBatchReader;
use triplelight_c::export::export;
use triplelight_c::BATCH_ROWS;

enum Fault { SchemaPanics, NextPanicsSecond, NextErrsWithNul }

struct Faulty { fault: Fault, pulls: usize, schema: SchemaRef }

impl Iterator for Faulty {
    type Item = Result<RecordBatch, ArrowError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.pulls += 1;
        match self.fault {
            Fault::NextPanicsSecond if self.pulls == 2 => panic!("injected next panic"),
            Fault::NextErrsWithNul => Some(Err(ArrowError::ComputeError("bad\0term".into()))),
            _ => Some(RecordBatch::try_new_with_options(Arc::clone(&self.schema), vec![], &RecordBatchOptions::new().with_row_count(Some(3)))),
        }
    }
}

impl RecordBatchReader for Faulty {
    fn schema(&self) -> SchemaRef {
        if let Fault::SchemaPanics = self.fault { panic!("injected schema panic") }
        Arc::clone(&self.schema)
    }
}

fn exported(fault: Fault) -> FFI_ArrowArrayStream {
    let mut s = FFI_ArrowArrayStream::empty();
    unsafe { export(Box::new(Faulty { fault, pulls: 0, schema: Arc::new(Schema::empty()) }), &mut s) };
    s
}

fn last_error(s: &mut FFI_ArrowArrayStream) -> String {
    let p = unsafe { s.get_last_error().unwrap()(s) };
    assert!(!p.is_null());
    unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string()
}

fn pull(s: &mut FFI_ArrowArrayStream) -> (i32, FFI_ArrowArray) {
    let mut a = FFI_ArrowArray::empty();
    let rc = unsafe { s.get_next().unwrap()(s, &mut a) };
    (rc, a)
}

#[test]
fn schema_panic_becomes_an_error() {
    let mut s = exported(Fault::SchemaPanics);
    let mut schema = FFI_ArrowSchema::empty();
    let rc = unsafe { s.get_schema().unwrap()(&mut s, &mut schema) };
    assert_ne!(rc, 0);
    assert!(last_error(&mut s).contains("injected schema panic"));
    drop(s); // release after a caught panic
}

#[test]
fn next_panic_poisons_the_stream() {
    let mut s = exported(Fault::NextPanicsSecond);
    let (rc, a) = pull(&mut s);
    assert_eq!(rc, 0);
    assert_eq!(a.len(), 3, "zero-column batch keeps its row count");
    drop(a);
    let (rc, _) = pull(&mut s);
    assert_ne!(rc, 0);
    let first = last_error(&mut s);
    assert!(first.contains("injected next panic"), "{first}");
    let (again, a) = pull(&mut s);
    assert_eq!(again, rc, "a poisoned stream repeats its error");
    assert!(a.is_released());
    assert_eq!(last_error(&mut s), first);
}

#[test]
fn error_with_interior_nul_is_reported() {
    let mut s = exported(Fault::NextErrsWithNul);
    let (rc, _) = pull(&mut s);
    assert_ne!(rc, 0);
    assert!(last_error(&mut s).contains("bad\\0term"));
}

#[test]
fn released_stream_rejects_calls() {
    let mut s = exported(Fault::NextPanicsSecond);
    unsafe { s.release().unwrap()(&mut s) };
    assert!(s.release().is_none() && s.private_data().is_null());
}

#[test]
fn computed_terms_are_converted_per_batch() {
    let n = 2 * BATCH_ROWS + 3;
    let db = Database::in_memory();
    let mut update = String::from("INSERT DATA {");
    for i in 0..n { update.push_str(&format!(" <urn:s{i:06}> <urn:p> {i} .")) }
    db.update(&(update + " }")).unwrap();
    let snap = db.read();
    let q = r#"SELECT ?s (CONCAT("c", STR(?o)) AS ?c) WHERE { ?s <urn:p> ?o } ORDER BY ?s"#;
    let QueryResult::Ids(ids) = snap.query_ids(q).unwrap() else { panic!("SELECT expected") };
    let computed = ids.scratch.len();
    assert!(computed >= n, "each row has its own computed value ({computed})");
    let mut reader = IdBatchReader::new(snap, ids);
    let mut seen = Vec::new();
    for b in reader.by_ref() {
        let b = b.unwrap();
        let c = b.column(1).as_any().downcast_ref::<StructArray>().unwrap();
        let v = c.column(1).as_any().downcast_ref::<LargeStringArray>().unwrap();
        seen.extend((0..b.num_rows()).map(|r| v.value(r).to_string()));
    }
    assert_eq!(seen, (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>());
    // Every computed value appears in exactly one batch, so each is converted once in total.
    assert_eq!(reader.scratch_converted(), n);
}
