# TripleLight for WebAssembly

The TripleLight SPARQL engine compiled to WebAssembly, with an in-memory store. It runs in browsers, Deno, edge
workers and Node without a native addon. The API matches the Node binding (`query`, `queryJson`, `update`, `load`,
terms shaped like RDF/JS, errors with a `kind`), with two differences: data lives in memory and is loaded from a
string, and every method also has a `...Sync` form.

```js
import init, { Database } from "./pkg/triplelight_wasm.js";
await init();

const db = new Database();
await db.load(await (await fetch("/data.nt")).text());           // or db.load(text, "turtle" | "nquads" | "trig")
const r = await db.query("SELECT ?s (COUNT(?o) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s");
for (const b of r.bindings) console.log(b.s.value, b.n.value);
```

Install: `npm install triplelight-wasm`. In Node, `require("triplelight-wasm")` loads the CommonJS build and needs no
`init()`; bundlers and browsers get the ES module build, where `await init()` comes first as in the example.

Build: `npm run build` (Rust 1.100, `rustup target add wasm32-unknown-unknown`, and
`cargo install wasm-bindgen-cli --version 0.2.100`). Test: `npm test` (runs the WASM build under Node).

All work (`load`, `update`, `query`, `queryJson`) runs to completion on the calling thread before the method
returns, including the Promise-returning forms; use a Web Worker for large data in a browser. `queryJson` answers
SELECT and ASK; CONSTRUCT and DESCRIBE go through `query`. Blank-node labels are scoped to each `load` call.

Register a synchronous SPARQL function with `db.registerFunction(iri, callback, arity?, pure?)`.
The callback receives one RDF/JS term per argument. Return an RDF/JS term, or return `null` to signal a
SPARQL expression error. Thrown exceptions and invalid return values also become expression errors.
Set `pure` only for functions whose result depends only on their arguments, since the query can reuse those results.

```js
db.registerFunction("urn:echo", (term) => term, 1, true);
const result = await db.query('SELECT (<urn:echo>("hello"@en) AS ?value) WHERE {}');
```

A snapshot retains the registrations it saw when `db.read()` was called. Callbacks run on the calling JavaScript
thread in both query forms. Keep callbacks synchronous and avoid reentering the database they are evaluating.
Call `free()` on databases and snapshots when finished, particularly if a callback retains either object.

Not in this version: persistence (OPFS / IndexedDB), the HTTP server, and multithreaded WebAssembly.
