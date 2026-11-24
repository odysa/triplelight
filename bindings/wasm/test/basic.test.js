// Runs the WASM build under Node (same engine as in browsers): `npm test` after `npm run build`.
const test = require("node:test");
const assert = require("node:assert");
const { Database } = require("../pkg-node/triplelight_wasm.js");

const XSD = "http://www.w3.org/2001/XMLSchema#";
const DATA = '<urn:a> <urn:p> <urn:b> .\n<urn:b> <urn:p> <urn:c> .\n<urn:a> <urn:age> "30"^^<' + XSD + 'integer> .\n<urn:a> <urn:name> "Alice"@en .\n';

test("load N-Triples, query with RDF/JS-shaped terms", async () => {
  const db = new Database();
  assert.strictEqual(await db.load(DATA), 4);
  assert.strictEqual(db.size, 4);
  const r = await db.query("SELECT ?o WHERE { <urn:a> <urn:age> ?o }");
  assert.deepStrictEqual(r.variables, ["o"]);
  assert.deepStrictEqual(r.bindings[0].o, { termType: "Literal", value: "30", language: "", datatype: { termType: "NamedNode", value: XSD + "integer" } });
  assert.strictEqual(db.querySync("SELECT ?n WHERE { <urn:a> <urn:name> ?n }").bindings[0].n.language, "en");
});

test("property paths, aggregates, ASK, CONSTRUCT and JSON", () => {
  const db = new Database();
  db.loadSync(DATA);
  assert.strictEqual(db.querySync("SELECT ?x WHERE { <urn:a> <urn:p>+ ?x }").bindings.length, 2);
  assert.strictEqual(db.querySync("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }").bindings[0].n.value, "4");
  assert.strictEqual(db.querySync("ASK { <urn:b> <urn:p> <urn:c> }").boolean, true);
  assert.strictEqual(db.querySync("CONSTRUCT { ?o <urn:q> ?s } WHERE { ?s <urn:p> ?o }").triples.length, 2);
  assert.strictEqual(JSON.parse(db.queryJsonSync("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")).results.bindings[0].o.value, "urn:b");
});

test("updates, incremental loads, Turtle and snapshot isolation", async () => {
  const db = new Database();
  assert.strictEqual(await db.update("INSERT DATA { <urn:x> <urn:p> 1 . <urn:x> <urn:p> 2 }"), 2);
  assert.strictEqual(await db.update("INSERT DATA { <urn:x> <urn:p> 1 }"), 0);
  const snap = db.read();
  assert.strictEqual(db.loadSync("@prefix : <urn:> . :y :p 3 .", "turtle"), 1);
  assert.strictEqual(await db.update("DELETE WHERE { <urn:x> <urn:p> ?o }"), 2);
  assert.strictEqual(db.size, 1);
  assert.strictEqual(snap.querySync("SELECT * WHERE { ?s ?p ?o }").bindings.length, 2);
});

test("each update operation gets fresh blank nodes", () => {
  const db = new Database();
  db.updateSync("INSERT DATA { _:x <urn:p> 1 }");
  db.updateSync("INSERT DATA { _:x <urn:p> 1 }");
  assert.strictEqual(db.size, 2);
});

test("NOW, UUID and BNODE work without OS clocks or randomness", () => {
  const db = new Database();
  const b = db.querySync("SELECT (NOW() AS ?n) (STRUUID() AS ?u) (isBlank(BNODE()) AS ?b) WHERE {}").bindings[0];
  assert.match(b.n.value, /^\d{4}-\d\d-\d\dT/);
  assert.strictEqual(b.u.value.length, 36);
  assert.strictEqual(b.b.value, "true");
});

test("errors carry a kind; a bad load adds nothing", async () => {
  const db = new Database();
  await assert.rejects(db.query("SELEC"), (e) => e.kind === "SyntaxError");
  assert.throws(() => db.querySync("SELECT * WHERE { SERVICE <urn:remote> { ?s ?p ?o } }"), (e) => e.kind === "UnsupportedError");
  assert.throws(() => db.updateSync("DROP GRAPH <urn:absent>"), (e) => e.kind === "GraphStateError");
  assert.throws(() => db.loadSync("<urn:a> <urn:p> .\n"), (e) => e.kind === "SyntaxError");
  assert.strictEqual(db.size, 0);
  const b = db.querySync('SELECT ("x" AS ?__proto__) ("y" AS ?constructor) WHERE {}').bindings[0];
  assert.strictEqual(Object.getPrototypeOf(b), null);
  assert.deepStrictEqual(Object.keys(b).sort(), ["__proto__", "constructor"]);
  assert.strictEqual(b.constructor.value, "y");
});

test("queryJson rejects graph results; blank-node labels are scoped per load", async () => {
  const db = new Database();
  db.loadSync("_:b <urn:p> <urn:x> .\n");
  db.loadSync("_:b <urn:p> <urn:y> .\n");
  assert.strictEqual(db.querySync("SELECT DISTINCT ?s WHERE { ?s <urn:p> ?o }").bindings.length, 2);
  assert.throws(() => db.queryJsonSync("CONSTRUCT WHERE { ?s ?p ?o }"), (e) => e.kind === "UnsupportedError");
  await assert.rejects(db.read().queryJson("DESCRIBE <urn:x>"), (e) => e.kind === "UnsupportedError");
  await assert.rejects(db.queryJson("CONSTRUCT WHERE { ?s ?p ?o }"), (e) => e.kind === "UnsupportedError");
});

test("named graphs: updates, management, quad loads and dataset queries", () => {
  const db = new Database();
  db.updateSync("INSERT DATA { GRAPH <urn:g> { <urn:a> <urn:p> 1 } <urn:a> <urn:p> 2 } ; COPY <urn:g> TO <urn:h>");
  const graphs = db.querySync("SELECT ?g WHERE { GRAPH ?g { } }").bindings.map((b) => b.g.value).sort();
  assert.deepStrictEqual(graphs, ["urn:g", "urn:h"]);
  assert.strictEqual(db.size, 1); // the default graph only
  assert.strictEqual(db.querySync("SELECT ?o FROM <urn:h> WHERE { <urn:a> <urn:p> ?o }").bindings[0].o.value, "1");
  // A quad load into a database that already has named graphs keeps them (no bulk base replacement).
  assert.strictEqual(db.loadSync("_:b <urn:q> \"x\" <urn:k> .\n_:b <urn:q> \"y\" <urn:g> .\n", "nquads"), 2);
  assert.strictEqual(db.loadSync("@prefix : <urn:> . :t { :a :p 3 }", "trig"), 1);
  const shared = db.querySync("SELECT ?y WHERE { GRAPH <urn:k> { ?b <urn:q> \"x\" } GRAPH <urn:g> { ?b <urn:q> ?y } }");
  assert.strictEqual(shared.bindings[0].y.value, "y");
  assert.deepStrictEqual(db.querySync("SELECT ?g WHERE { GRAPH ?g { } }").bindings.map((b) => b.g.value).sort(), ["urn:g", "urn:h", "urn:k", "urn:t"]);
  assert.throws(() => db.loadSync("<urn:a> <urn:p> \"1\" _:g .\n", "nquads"), (e) => e.kind === "UnsupportedError");
  assert.throws(() => db.updateSync("CREATE GRAPH <urn:g>"), (e) => e.kind === "GraphStateError");
  // A snapshot keeps the dataset as it was.
  const snap = db.read();
  db.updateSync("DROP ALL");
  assert.strictEqual(snap.querySync("ASK { GRAPH <urn:t> { ?s ?p ?o } }").boolean, true);
  assert.strictEqual(db.querySync("ASK { GRAPH ?g { ?s ?p ?o } }").boolean, false);
});

test("a triple load into a named-only database keeps its graphs", async () => {
  // Only named graphs, the default graph empty: the bulk path must not replace the base.
  for (const setup of [
    (db) => db.loadSync("<urn:a> <urn:p> \"1\" <urn:g> .\n", "nquads"),
    (db) => db.updateSync("CREATE GRAPH <urn:g>"),
  ]) {
    const db = new Database();
    setup(db);
    assert.strictEqual(db.size, 0);
    assert.strictEqual(db.loadSync("<urn:a> <urn:p> \"2\" .\n"), 1);
    assert.strictEqual(db.size, 1);
    assert.deepStrictEqual(db.querySync("SELECT ?g WHERE { GRAPH ?g { } }").bindings.map((b) => b.g.value), ["urn:g"]);
  }
  const db = new Database();
  db.loadSync("<urn:a> <urn:p> \"1\" <urn:g> .\n", "nquads");
  const json = JSON.parse(await db.read().queryJson("SELECT ?o WHERE { GRAPH <urn:g> { <urn:a> <urn:p> ?o } }"));
  assert.strictEqual(json.results.bindings[0].o.value, "1");
  assert.strictEqual(JSON.parse(db.queryJsonSync("ASK { GRAPH <urn:g> { ?s ?p ?o } }")).boolean, true);
});

test("RDFS entailment is opt-in per query", async () => {
  const db = new Database();
  await db.load("<urn:ann> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Student> .\n<urn:Student> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Person> .\n");
  const q = "SELECT ?x WHERE { ?x a <urn:Person> }";
  assert.strictEqual(db.querySync(q).bindings.length, 0);
  assert.deepStrictEqual(db.querySync(q, { entailment: "rdfs" }).bindings.map((b) => b.x.value), ["urn:ann"]);
  assert.ok(db.queryJsonSync(q, { entailment: "rdfs" }).includes("urn:ann"));
  assert.strictEqual((await db.read().query(q, { entailment: "rdfs" })).bindings.length, 1);
  assert.throws(() => db.querySync(q, { entailment: "owl" }), /entailment must be/);
});

test("SHACL validate: reports, syntax errors and rejected shapes", () => {
  const { validate } = require("../pkg-node/triplelight_wasm.js");
  const shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ; sh:property [ sh:path <urn:name> ; sh:minCount 1 ] .";
  assert.strictEqual(validate('<urn:ann> a <urn:Person> ; <urn:name> "Ann" .', shapes).conforms, true);
  const bad = validate("<urn:bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .", shapes, { dataFormat: "ntriples" });
  assert.strictEqual(bad.conforms, false);
  assert.ok(bad.triples.some((t) => t.predicate.value === "http://www.w3.org/ns/shacl#focusNode" && t.object.value === "urn:bob"));
  assert.throws(() => validate("<urn:bob> a", shapes), (e) => e.kind === "SyntaxError");
  assert.throws(() => validate("", "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:S> a sh:NodeShape ; sh:targetClass <urn:P> ; sh:property [ sh:minCount 1 ] ."), (e) => e.kind === "ShapesError");
});

test("extension availability, autoload and load", () => {
  const db = new Database();
  try {
    assert.ok(db.listExtensions().every(({ compiledIn, loaded }) => compiledIn && !loaded));
    assert.strictEqual(db.querySync("ASK {}", { entailment: "rdfs" }).boolean, true);
    db.loadExtension("shacl");
    assert.deepStrictEqual(db.listExtensions().filter(({ loaded }) => loaded).map(({ name }) => name), ["rdfs", "shacl"]);
    assert.throws(() => db.loadExtension("missing"), (error) => error.kind === "UnsupportedError");
  } finally {
    db.free();
  }
});
