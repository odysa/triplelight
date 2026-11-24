//! TripleLight compiled to WebAssembly: the full SPARQL engine over an in-memory store, for browsers, Deno, edge
//! workers and Node without a native addon. The API mirrors the Node binding (`query`, `queryJson`, `update`, `load`,
//! RDF/JS-shaped terms, errors with a `kind`), except that data lives in memory and is loaded from a string. Every
//! method, including the Promise-returning ones, runs to completion on the calling thread before it returns.
use std::sync::Arc;

use js_sys::{Array, Object, Promise, Reflect};
use triplelight::rdf::{Format, Literal, QuadFormat, ReadError, Term, Triple};
use triplelight::exec::{self, QueryErrorKind, QueryResult};
use triplelight::storage::MemorySnapshot;
use triplelight::store::{Change, Overlay};
use wasm_bindgen::prelude::*;

/// A JS `Error` whose `name` and `kind` say what went wrong: `SyntaxError`, `UnsupportedError`, `StorageError` or
/// `GraphStateError` (an update names a graph in the wrong state).
fn error(kind: &str, message: &str) -> JsValue {
    let e = js_sys::Error::new(message);
    e.set_name(kind);
    let _ = Reflect::set(&e, &"kind".into(), &kind.into());
    e.into()
}

fn query_error(e: exec::QueryError) -> JsValue {
    let kind = match e.kind {
        QueryErrorKind::Syntax => "SyntaxError",
        QueryErrorKind::Unsupported => "UnsupportedError",
        QueryErrorKind::Storage => "StorageError",
        QueryErrorKind::GraphState => "GraphStateError",
    };
    error(kind, &e.message)
}

fn set(o: &Object, k: &str, v: impl Into<JsValue>) { let _ = Reflect::set(o, &k.into(), &v.into()); }

fn named(iri: &str) -> Object {
    let o = Object::new();
    set(&o, "termType", "NamedNode");
    set(&o, "value", iri);
    o
}

fn literal(l: &Literal) -> Object {
    let o = Object::new();
    set(&o, "termType", "Literal");
    set(&o, "value", l.value());
    set(&o, "language", l.language().unwrap_or(""));
    set(&o, "datatype", named(l.datatype().as_str()));
    o
}

fn term(t: &Term) -> Object {
    match t {
        Term::NamedNode(n) => named(n.as_str()),
        Term::BlankNode(b) => { let o = Object::new(); set(&o, "termType", "BlankNode"); set(&o, "value", b.as_str()); o }
        Term::Literal(l) => literal(l),
        #[allow(unreachable_patterns)]
        _ => Object::new(),
    }
}

/// Same shape as the Node binding: `{type: "bindings" | "boolean" | "triples", ...}`.
fn convert(r: QueryResult) -> Result<JsValue, JsValue> {
    let out = Object::new();
    match r {
        QueryResult::Solutions { vars, rows } => {
            set(&out, "type", "bindings");
            set(&out, "variables", vars.iter().map(|v| JsValue::from_str(v.as_str())).collect::<Array>());
            let list = Array::new();
            for row in &rows {
                // No prototype: a variable named `__proto__` must become an own property, not replace the prototype.
                let b = Object::create(JsValue::NULL.unchecked_ref());
                for (v, t) in vars.iter().zip(row) { if let Some(t) = t { set(&b, v.as_str(), term(t)); } }
                list.push(&b);
            }
            set(&out, "bindings", list);
        }
        QueryResult::Boolean(x) => { set(&out, "type", "boolean"); set(&out, "boolean", x); }
        QueryResult::Graph(triples) => {
            set(&out, "type", "triples");
            let list = Array::new();
            for t in &triples {
                let o = Object::new();
                set(&o, "subject", term(&t.subject.clone().into()));
                set(&o, "predicate", named(t.predicate.as_str()));
                set(&o, "object", term(&t.object));
                list.push(&o);
            }
            set(&out, "triples", list);
        }
        QueryResult::Ids(_) => return Err(error("StorageError", "internal: undecoded result")),
    }
    Ok(out.into())
}

/// SPARQL Results JSON for SELECT and ASK. Like the Node binding, CONSTRUCT/DESCRIBE results are rejected here
/// (their graph has no SPARQL JSON form); use `query` for them.
fn results_json(r: QueryResult) -> Result<String, JsValue> {
    match r {
        QueryResult::Graph(_) => Err(error("UnsupportedError", "queryJson supports SELECT and ASK; use query() for CONSTRUCT and DESCRIBE")),
        r => Ok(exec::results::serialize(&r).1),
    }
}

/// Blank-node labels are scoped to the document they appear in: `_:b` in two `load` calls are two different nodes.
fn fresh_blank_nodes(triples: Vec<Triple>) -> Vec<Triple> {
    use std::collections::HashMap;
    use triplelight::rdf::{BlankNode, NamedOrBlankNode};
    let mut map: HashMap<BlankNode, BlankNode> = HashMap::new();
    let mut fresh = |b: &BlankNode| map.entry(b.clone()).or_default().clone();
    triples.into_iter().map(|t| {
        let subject = match t.subject { NamedOrBlankNode::BlankNode(b) => NamedOrBlankNode::BlankNode(fresh(&b)), s => s };
        let object = match t.object { Term::BlankNode(b) => Term::BlankNode(fresh(&b)), o => o };
        Triple::new(subject, t.predicate, object)
    }).collect()
}

/// Runs a query over the whole dataset: the default graph plus the named graphs (`GRAPH`, `FROM`, `FROM NAMED`).
fn run(cur: &Overlay, sparql: &str, options: Option<Object>) -> Result<QueryResult, JsValue> {
    let opts = exec::Options { entailment: entailment(options)?, ..Default::default() };
    exec::query_dataset_with(cur, cur, sparql, &opts).map_err(query_error)
}

/// The `entailment` query option (#108): "simple" (the default) or "rdfs", which matches the default graph's RDFS
/// closure computed at query time.
fn entailment(options: Option<Object>) -> Result<exec::Entailment, JsValue> {
    let Some(options) = options else { return Ok(exec::Entailment::Simple) };
    let value = Reflect::get(&options, &JsValue::from_str("entailment"))?;
    match value.as_string().as_deref() {
        _ if value.is_undefined() || value.is_null() => Ok(exec::Entailment::Simple),
        Some("simple") => Ok(exec::Entailment::Simple),
        Some("rdfs") => Ok(exec::Entailment::Rdfs),
        _ => Err(js_sys::TypeError::new("entailment must be \"simple\" or \"rdfs\"").into()),
    }
}

fn settle(r: Result<JsValue, JsValue>) -> Promise {
    match r { Ok(v) => Promise::resolve(&v), Err(e) => Promise::reject(&e) }
}

/// An in-memory RDF database. Not persistent: export with `CONSTRUCT` or `queryJson` to keep data.
#[wasm_bindgen]
pub struct Database { cur: Arc<Overlay> }

#[wasm_bindgen]
impl Database {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Database {
        Database { cur: Arc::new(Overlay::from_base(Arc::new(MemorySnapshot::from_triples(Vec::new())))) }
    }

    /// Parses N-Triples (`format` = "ntriples", the default), Turtle ("turtle"), N-Quads ("nquads") or TriG ("trig")
    /// and adds every triple or quad, atomically: on an error nothing is added. Quads go into their named graphs;
    /// blank-node graph names are refused. Returns the number of triples added.
    #[wasm_bindgen(js_name = loadSync)]
    pub fn load_sync(&mut self, data: &str, format: Option<String>) -> Result<f64, JsValue> {
        let name = format.as_deref().unwrap_or("ntriples");
        let quads = match name { "nquads" | "nq" => Some(QuadFormat::NQuads), "trig" => Some(QuadFormat::TriG), _ => None };
        if let Some(format) = quads {
            let quads = triplelight::rdf::read_quads(data.as_bytes(), format).map_err(|e| match e {
                ReadError::Io(e) => error("StorageError", &e.to_string()),
                ReadError::Syntax(m) => error("SyntaxError", &m),
            })?;
            let changes = triplelight::store::quad_load_changes(quads).map_err(query_error)?;
            let (next, n) = self.cur.with_dataset_changes(&changes).map_err(|e| query_error(triplelight::store::storage_error(e)))?;
            self.cur = Arc::new(next);
            return Ok(n as f64);
        }
        let format = Format::from_name(name).ok_or_else(|| error("UnsupportedError",
            &format!("unknown format {name:?}: use \"ntriples\", \"turtle\", \"nquads\" or \"trig\"")))?;
        let triples = triplelight::rdf::parse_triples(data.as_bytes(), format).map_err(|e| error("SyntaxError", &e))?;
        let triples = fresh_blank_nodes(triples);
        // The bulk path replaces the base, so it applies only while the database holds nothing, named graphs included.
        if self.cur.delta_len() == 0 && !self.cur.has_named_graphs() && self.is_empty()? {
            // Bulk path: build sorted indexes once instead of inserting triple by triple.
            let base = MemorySnapshot::from_triples(triples);
            let added = base_len(&base); // duplicates in the input count once
            self.cur = Arc::new(Overlay::from_base(Arc::new(base)));
            return Ok(added as f64);
        }
        let changes: Vec<(Change, Triple)> = triples.into_iter().map(|t| (Change::Insert, t)).collect();
        let (next, n) = self.cur.with_changes(&changes).map_err(|e| error("StorageError", &e.to_string()))?;
        self.cur = Arc::new(next);
        Ok(n as f64)
    }

    /// `loadSync` as a Promise, for API parity with the Node binding.
    pub fn load(&mut self, data: &str, format: Option<String>) -> Promise {
        settle(self.load_sync(data, format).map(JsValue::from))
    }

    /// Applies a SPARQL Update atomically. Returns the number of triples changed.
    #[wasm_bindgen(js_name = updateSync)]
    pub fn update_sync(&mut self, sparql: &str) -> Result<f64, JsValue> {
        let (next, _, n) = triplelight::store::dataset_update(&self.cur, sparql).map_err(query_error)?;
        if let Some(next) = next { self.cur = Arc::new(next) }
        Ok(n as f64)
    }

    pub fn update(&mut self, sparql: &str) -> Promise { settle(self.update_sync(sparql).map(JsValue::from)) }

    /// Runs a SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE query.
    #[wasm_bindgen(js_name = querySync)]
    pub fn query_sync(&self, sparql: &str, options: Option<Object>) -> Result<JsValue, JsValue> {
        convert(run(&self.cur, sparql, options)?)
    }

    pub fn query(&self, sparql: &str, options: Option<Object>) -> Promise { settle(self.query_sync(sparql, options)) }

    /// SPARQL 1.1 Query Results JSON for SELECT and ASK, as a string.
    #[wasm_bindgen(js_name = queryJsonSync)]
    pub fn query_json_sync(&self, sparql: &str, options: Option<Object>) -> Result<String, JsValue> {
        results_json(run(&self.cur, sparql, options)?)
    }

    #[wasm_bindgen(js_name = queryJson)]
    pub fn query_json(&self, sparql: &str, options: Option<Object>) -> Promise { settle(self.query_json_sync(sparql, options).map(JsValue::from)) }

    /// A consistent view for several queries; later updates do not change it.
    pub fn read(&self) -> Snapshot { Snapshot { cur: Arc::clone(&self.cur) } }

    /// Number of triples in the default graph (named graphs are not counted).
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> Result<f64, JsValue> {
        match exec::query(&*self.cur, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }").map_err(query_error)? {
            QueryResult::Solutions { rows, .. } => Ok(rows.first().and_then(|r| r[0].as_ref())
                .and_then(|t| if let Term::Literal(l) = t { l.value().parse::<f64>().ok() } else { None }).unwrap_or(0.0)),
            _ => Ok(0.0),
        }
    }

    fn is_empty(&self) -> Result<bool, JsValue> { Ok(self.size()? == 0.0) }
}

impl Default for Database { fn default() -> Self { Self::new() } }

fn base_len(base: &MemorySnapshot) -> usize {
    use triplelight::storage::Snapshot as _;
    base.count(triplelight::model::Perm::Spo, &[]).map(|n| n as usize).unwrap_or(0)
}

/// An immutable view of the database, unaffected by later updates.
#[wasm_bindgen]
pub struct Snapshot { cur: Arc<Overlay> }

#[wasm_bindgen]
impl Snapshot {
    #[wasm_bindgen(js_name = querySync)]
    pub fn query_sync(&self, sparql: &str, options: Option<Object>) -> Result<JsValue, JsValue> {
        convert(run(&self.cur, sparql, options)?)
    }
    pub fn query(&self, sparql: &str, options: Option<Object>) -> Promise { settle(self.query_sync(sparql, options)) }
    #[wasm_bindgen(js_name = queryJson)]
    pub fn query_json(&self, sparql: &str, options: Option<Object>) -> Promise {
        settle(run(&self.cur, sparql, options).and_then(results_json).map(JsValue::from))
    }
}
