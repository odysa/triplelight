//! SELECT results as an Arrow C Stream (`tl_query_arrow`, task #34 step 3).
//!
//! Each projected variable is one nullable struct column `{kind: utf8, value: large_utf8, datatype: utf8?,
//! language: utf8?}`; a null struct is an unbound cell. Lexical forms, datatypes and languages are passed through
//! unchanged (no native-type conversion). Batches hold up to `BATCH_ROWS` solutions and are produced one at a time
//! from the query's ID result: each batch decodes its distinct stored IDs with one dictionary pass on the same
//! snapshot. This bounds the conversion work per batch, not memory: terms can be arbitrarily large. The query
//! result itself (term IDs) is still fully materialized before the stream starts.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use arrow_array::builder::{LargeStringBuilder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch, RecordBatchOptions, RecordBatchReader, StructArray};
use arrow_schema::{ArrowError, DataType, Field, Fields, Schema, SchemaRef};
use triplelight::embedded::ReadSnapshot;
use triplelight::exec::{IdSolutions, UNBOUND};
use triplelight::model::TermId;
use triplelight::rdf::TermRef;

/// Solutions per batch. A row-count bound only.
pub const BATCH_ROWS: usize = 8192;

pub fn term_fields() -> Fields {
    vec![
        Field::new("kind", DataType::Utf8, false),
        Field::new("value", DataType::LargeUtf8, false),
        Field::new("datatype", DataType::Utf8, true),
        Field::new("language", DataType::Utf8, true),
    ]
    .into()
}

pub fn schema_for(vars: &[String]) -> SchemaRef {
    Arc::new(Schema::new(vars.iter().map(|v| Field::new(v, DataType::Struct(term_fields()), true)).collect::<Vec<_>>()))
}

/// One decoded term (owned for the duration of one batch).
#[derive(Clone)]
struct Owned { kind: &'static str, value: String, datatype: Option<String>, language: Option<String> }

fn owned(t: TermRef<'_>) -> Result<Owned, ArrowError> {
    Ok(match t {
        TermRef::NamedNode(n) => Owned { kind: "iri", value: n.as_str().to_string(), datatype: None, language: None },
        TermRef::BlankNode(n) => Owned { kind: "bnode", value: n.as_str().to_string(), datatype: None, language: None },
        TermRef::Literal(l) => Owned {
            kind: "literal",
            value: l.value().to_string(),
            datatype: Some(l.datatype().as_str().to_string()),
            language: l.language().map(str::to_string),
        },
        #[allow(unreachable_patterns)]
        _ => return Err(ArrowError::NotYetImplemented("RDF-star terms are not supported".into())),
    })
}

/// Pulls batches from a materialized ID result. Owns its snapshot, so it outlives the database handle.
pub struct IdBatchReader {
    snap: ReadSnapshot,
    sol: IdSolutions,
    schema: SchemaRef,
    next_row: usize,
}

impl IdBatchReader {
    pub fn new(snap: ReadSnapshot, sol: IdSolutions) -> Self {
        let names: Vec<String> = sol.vars.iter().map(|v| v.as_str().to_string()).collect();
        Self { schema: schema_for(&names), snap, sol, next_row: 0 }
    }

    fn batch(&mut self) -> Result<Option<RecordBatch>, ArrowError> {
        let rows = self.sol.rows;
        if self.next_row >= rows { return Ok(None) }
        let start = self.next_row;
        let n = (rows - start).min(BATCH_ROWS);
        self.next_row += n;
        let width = self.sol.vars.len();
        let options = RecordBatchOptions::new().with_row_count(Some(n));
        if width == 0 {
            // A zero-column SELECT keeps its cardinality: n solutions, no columns.
            return RecordBatch::try_new_with_options(Arc::clone(&self.schema), vec![], &options).map(Some);
        }
        let cells = &self.sol.ids[start * width..(start + n) * width];
        let mut distinct: Vec<TermId> = cells.iter().copied().filter(|id| *id != UNBOUND && !self.sol.scratch.contains_key(id)).collect();
        distinct.sort_unstable();
        distinct.dedup();
        let mut decoded: Vec<Option<Owned>> = vec![None; distinct.len()];
        let mut failure = None;
        self.snap.with_terms(&distinct, &mut |i, t| match owned(t) {
            Ok(o) => decoded[i] = Some(o),
            Err(e) => failure = Some(e),
        }).map_err(|e| ArrowError::ExternalError(Box::new(e)))?;
        if let Some(e) = failure { return Err(e) }
        let scratch: std::collections::HashMap<TermId, Owned> = self.sol.scratch.iter()
            .map(|(id, t)| Ok((*id, owned(t.as_ref())?)))
            .collect::<Result<_, ArrowError>>()?;
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(width);
        for col in 0..width {
            let mut kind = StringBuilder::new();
            let mut value = LargeStringBuilder::new();
            let mut datatype = StringBuilder::new();
            let mut language = StringBuilder::new();
            let mut nulls = Vec::with_capacity(n);
            // i32-offset columns must not overflow: check their byte totals before appending.
            let (mut dt_bytes, mut lang_bytes) = (0usize, 0usize);
            for r in 0..n {
                let id = cells[r * width + col];
                let term = if id == UNBOUND {
                    None
                } else if let Some(t) = scratch.get(&id) {
                    Some(t)
                } else {
                    let i = distinct.binary_search(&id).map_err(|_| ArrowError::ComputeError("undecoded term ID".into()))?;
                    Some(decoded[i].as_ref().ok_or_else(|| ArrowError::ComputeError("dictionary omitted a requested term".into()))?)
                };
                nulls.push(term.is_some());
                match term {
                    None => { kind.append_value(""); value.append_value(""); datatype.append_null(); language.append_null(); }
                    Some(t) => {
                        dt_bytes += t.datatype.as_ref().map_or(0, String::len);
                        lang_bytes += t.language.as_ref().map_or(0, String::len);
                        if dt_bytes > i32::MAX as usize || lang_bytes > i32::MAX as usize {
                            return Err(ArrowError::ComputeError("batch string data exceeds Utf8 offset range".into()));
                        }
                        kind.append_value(t.kind);
                        value.append_value(&t.value);
                        datatype.append_option(t.datatype.as_deref());
                        language.append_option(t.language.as_deref());
                    }
                }
            }
            let children: Vec<ArrayRef> = vec![Arc::new(kind.finish()), Arc::new(value.finish()), Arc::new(datatype.finish()), Arc::new(language.finish())];
            let array = StructArray::try_new(term_fields(), children, Some(nulls.into()))?;
            columns.push(Arc::new(array));
        }
        RecordBatch::try_new_with_options(Arc::clone(&self.schema), columns, &options).map(Some)
    }
}

impl Iterator for IdBatchReader {
    type Item = Result<RecordBatch, ArrowError>;
    fn next(&mut self) -> Option<Self::Item> {
        // No panic may reach the C stream callbacks: report it as a stream error and end the stream.
        match catch_unwind(AssertUnwindSafe(|| self.batch())) {
            Ok(Ok(Some(b))) => Some(Ok(b)),
            Ok(Ok(None)) => None,
            Ok(Err(e)) => { self.next_row = self.sol.rows; Some(Err(e)) }
            Err(_) => { self.next_row = self.sol.rows; Some(Err(ArrowError::ComputeError("internal panic while building a batch".into()))) }
        }
    }
}

impl RecordBatchReader for IdBatchReader {
    fn schema(&self) -> SchemaRef { Arc::clone(&self.schema) }
}

// The C stream moves the reader across threads only with the consumer's serialized pulls.
const _: fn() = || { fn send<T: Send>() {} send::<IdBatchReader>(); };
