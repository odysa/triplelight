//! Lossless Arrow projection of materialized RDF SELECT IDs.
use std::sync::Arc;
use arrow_array::{ArrayRef, RecordBatch, RecordBatchIterator, RecordBatchReader, RecordBatchOptions};
use arrow_array::builder::{ArrayBuilder, LargeStringBuilder, StringBuilder, StructBuilder};
use arrow_schema::{DataType, Field, Fields, Schema};
use oxrdf::Term;
use pyo3::prelude::*;
use rdfdb::exec::{QueryResult, UNBOUND};
use rdfdb::embedded::{DatabaseError, ReadSnapshot};
use arrow_pyarrow::IntoPyArrow;

const BATCH_ROWS: usize = 8192;

/// Each SPARQL binding is a nullable struct {kind, value, datatype, language}.
/// Null struct = unbound; literal lexical strings and datatype/language stay exact.
fn term_fields() -> Fields {
    vec![
        Field::new("kind", DataType::Utf8, false),
        Field::new("value", DataType::LargeUtf8, false),
        Field::new("datatype", DataType::LargeUtf8, true),
        Field::new("language", DataType::LargeUtf8, true),
    ].into()
}
fn builder() -> StructBuilder {
    let fields = term_fields();
    let builders: Vec<Box<dyn ArrayBuilder>> = vec![
        Box::new(StringBuilder::new()), Box::new(LargeStringBuilder::new()),
        Box::new(LargeStringBuilder::new()), Box::new(LargeStringBuilder::new()),
    ];
    StructBuilder::new(fields, builders)
}
fn append(b: &mut StructBuilder, term: Option<&Term>) -> Result<(), String> {
    let (kind, value, datatype, language) = match term {
        None => (None, None, None, None),
        Some(Term::NamedNode(n)) => (Some("iri"), Some(n.as_str()), None, None),
        Some(Term::BlankNode(n)) => (Some("bnode"), Some(n.as_str()), None, None),
        Some(Term::Literal(l)) => (Some("literal"), Some(l.value()), Some(l.datatype().as_str()), l.language()),
        #[allow(unreachable_patterns)]
        Some(_) => return Err("unsupported RDF term in Arrow result".into()),
    };
    b.field_builder::<StringBuilder>(0).unwrap().append_option(kind);
    b.field_builder::<LargeStringBuilder>(1).unwrap().append_option(value);
    b.field_builder::<LargeStringBuilder>(2).unwrap().append_option(datatype);
    b.field_builder::<LargeStringBuilder>(3).unwrap().append_option(language);
    b.append(term.is_some());
    Ok(())
}

/// SELECT results only. ID rows remain materialized; dictionary values are decoded once per distinct ID per batch.
pub fn to_batches(snapshot: &ReadSnapshot, result: QueryResult) -> Result<(Arc<Schema>, Vec<RecordBatch>), DatabaseError> {
    let QueryResult::Ids(result) = result else { return Err(DatabaseError::Unsupported("Arrow output supports SELECT only".into())) };
    let width = result.vars.len();
    let fields: Vec<Field> = result.vars.iter().map(|v| Field::new(v.as_str(), DataType::Struct(term_fields()), true)).collect();
    let schema = Arc::new(Schema::new(fields));
    let mut batches = Vec::new();
    if width == 0 { return Ok((schema, batches)); }
    let batch_cells = width.checked_mul(BATCH_ROWS).ok_or_else(|| DatabaseError::Storage(std::io::Error::other("result batch size overflow")))?;
    for chunk in result.ids.chunks(batch_cells) {
        if chunk.len() % width != 0 { return Err(DatabaseError::Storage(std::io::Error::other("result row width mismatch"))); }
        let mut ids: Vec<_> = chunk.iter().copied().filter(|id| *id != UNBOUND && !result.scratch.contains_key(id)).collect();
        ids.sort_unstable(); ids.dedup();
        let mut decoded: Vec<Option<Term>> = vec![None; ids.len()];
        snapshot.with_stored_terms(&ids, &mut |i, term| decoded[i] = Some(term.into_owned()))?;
        if decoded.iter().any(Option::is_none) { return Err(DatabaseError::Storage(std::io::Error::other("dictionary omitted a requested term"))); }
        let mut builders: Vec<_> = result.vars.iter().map(|_| builder()).collect();
        for row in chunk.chunks_exact(width) {
            for (b, id) in builders.iter_mut().zip(row) {
                let term = if *id == UNBOUND { None } else if let Some(term) = result.scratch.get(id) { Some(term) } else {
                    let i = ids.binary_search(id).map_err(|_| DatabaseError::Storage(std::io::Error::other("missing dictionary term")))?;
                    decoded[i].as_ref()
                };
                append(b, term).map_err(DatabaseError::Unsupported)?;
            }
        }
        let cols: Vec<ArrayRef> = builders.into_iter().map(|mut b| Arc::new(b.finish()) as ArrayRef).collect();
        let options = RecordBatchOptions::new().with_row_count(Some(chunk.len() / width));
        batches.push(RecordBatch::try_new_with_options(Arc::clone(&schema), cols, &options).map_err(|e| DatabaseError::Storage(std::io::Error::other(e)))?);
    }
    Ok((schema, batches))
}

/// Arrow's C stream interface transfers buffer ownership to PyArrow without copying the Arrow buffers.
pub fn to_reader(py: Python<'_>, snapshot: Arc<ReadSnapshot>, sparql: &str) -> PyResult<PyObject> {
    let (schema, batches) = py.allow_threads(|| {
        let result = snapshot.query_ids(sparql).map_err(crate::err)?;
        to_batches(&snapshot, result).map_err(crate::err)
    })?;
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(batches.into_iter().map(Ok), schema));
    reader.into_pyarrow(py)
}
