//! Lossless Arrow projection of materialized RDF SELECT rows.
use std::sync::Arc;
use arrow_array::{ArrayRef, RecordBatch, RecordBatchIterator, RecordBatchReader, RecordBatchOptions};
use arrow_array::builder::{ArrayBuilder, LargeStringBuilder, StringBuilder, StructBuilder};
use arrow_schema::{DataType, Field, Fields, Schema};
use oxrdf::Term;
use pyo3::prelude::*;
use rdfdb::exec::QueryResult;
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

/// SELECT results only. Eager query execution currently materializes all rows before batching.
pub fn to_batches(result: QueryResult) -> Result<(Arc<Schema>, Vec<RecordBatch>), String> {
    let QueryResult::Solutions { vars, rows } = result else { return Err("Arrow output supports SELECT only".into()) };
    let fields: Vec<Field> = vars.iter().map(|v| Field::new(v.as_str(), DataType::Struct(term_fields()), true)).collect();
    let schema = Arc::new(Schema::new(fields));
    let mut batches = Vec::new();
    for chunk in rows.chunks(BATCH_ROWS) {
        let mut builders: Vec<_> = vars.iter().map(|_| builder()).collect();
        for row in chunk {
            if row.len() != builders.len() { return Err("result row width mismatch".into()); }
            for (b, term) in builders.iter_mut().zip(row) { append(b, term.as_ref())?; }
        }
        let cols: Vec<ArrayRef> = builders.into_iter().map(|mut b| Arc::new(b.finish()) as ArrayRef).collect();
        let options = RecordBatchOptions::new().with_row_count(Some(chunk.len()));
        batches.push(RecordBatch::try_new_with_options(Arc::clone(&schema), cols, &options).map_err(|e| e.to_string())?);
    }
    Ok((schema, batches))
}

/// Arrow's C stream interface transfers buffer ownership to PyArrow without copying the Arrow buffers.
pub fn to_reader(py: Python<'_>, result: QueryResult) -> PyResult<PyObject> {
    let (schema, batches) = to_batches(result).map_err(crate::RdfdbError::new_err)?;
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(batches.into_iter().map(Ok), schema));
    reader.into_pyarrow(py)
}
