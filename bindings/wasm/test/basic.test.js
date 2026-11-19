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
  assert.throws(() => db.querySync("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }"), (e) => e.kind === "UnsupportedError");
  assert.throws(() => db.loadSync("<urn:a> <urn:p> .\n"), (e) => e.kind === "SyntaxError");
  assert.strictEqual(db.size, 0);
  const b = db.querySync('SELECT ("x" AS ?__proto__) ("y" AS ?constructor) WHERE {}').bindings[0];
  assert.strictEqual(Object.getPrototypeOf(b), null);
  assert.deepStrictEqual(Object.keys(b).sort(), ["__proto__", "constructor"]);
  assert.strictEqual(b.constructor.value, "y");
});
