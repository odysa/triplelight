const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const tl = require("..");

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "triplelight-node-"));
const XSD = "http://www.w3.org/2001/XMLSchema#";

test("round trip with RDF/JS terms, persistence and ASK", async () => {
  const dir = path.join(tmp(), "db");
  let db = new tl.Database(dir);
  const n = await db.update('INSERT DATA { <urn:a> <urn:age> 30 . <urn:a> <urn:name> "Alice" . <urn:a> <urn:label> "x"@en . <urn:a> <urn:knows> _:b }');
  assert.strictEqual(n, 4);
  const r = await db.query("SELECT ?p ?o WHERE { <urn:a> ?p ?o } ORDER BY ?p");
  assert.strictEqual(r.type, "bindings");
  assert.deepStrictEqual(r.variables, ["p", "o"]);
  const byP = Object.fromEntries(r.bindings.map((b) => [b.p.value, b.o]));
  assert.deepStrictEqual(byP["urn:age"], { termType: "Literal", value: "30", language: "", datatype: { termType: "NamedNode", value: XSD + "integer" } });
  assert.strictEqual(byP["urn:label"].language, "en");
  assert.strictEqual(byP["urn:knows"].termType, "BlankNode");
  assert.strictEqual((await db.query("ASK { <urn:a> <urn:name> \"Alice\" }")).boolean, true);
  db.close();
  db = new tl.Database(dir);
  assert.strictEqual(db.querySync("SELECT * WHERE { ?s ?p ?o }").bindings.length, 4);
  db.close();
});

test("unbound variables are absent, CONSTRUCT returns triples, JSON fast path", async () => {
  const db = new tl.Database(path.join(tmp(), "db"));
  await db.update("INSERT DATA { <urn:a> <urn:p> <urn:b> }");
  const r = await db.query("SELECT ?o ?z WHERE { <urn:a> <urn:p> ?o OPTIONAL { <urn:a> <urn:none> ?z } }");
  assert.deepStrictEqual(Object.keys(r.bindings[0]), ["o"]);
  const c = await db.query("CONSTRUCT { ?o <urn:q> <urn:a> } WHERE { <urn:a> <urn:p> ?o }");
  assert.strictEqual(c.type, "triples");
  assert.strictEqual(c.triples[0].subject.value, "urn:b");
  const j = JSON.parse(await db.queryJson("SELECT ?o WHERE { <urn:a> <urn:p> ?o }"));
  assert.strictEqual(j.results.bindings[0].o.value, "urn:b");
  db.close();
});

test("snapshots are isolated from later updates", async () => {
  const db = new tl.Database(path.join(tmp(), "db"));
  await db.update("INSERT DATA { <urn:a> <urn:p> 1 }");
  const snap = db.read();
  await db.update("INSERT DATA { <urn:a> <urn:p> 2 }");
  assert.strictEqual((await snap.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")).bindings.length, 1);
  assert.strictEqual((await db.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")).bindings.length, 2);
  db.close();
});

test("typed errors: syntax, unsupported, locked", async () => {
  const dir = path.join(tmp(), "db");
  const db = new tl.Database(dir);
  await assert.rejects(db.query("SELEC nonsense"), (e) => e instanceof tl.SyntaxError && e.kind === "SyntaxError");
  await assert.rejects(db.query("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }"), tl.UnsupportedError);
  assert.throws(() => db.querySync("SELEC"), tl.TripleLightError);
  assert.throws(() => new tl.Database(dir), tl.LockedError);
  db.close();
});

test("load an N-Triples file", async () => {
  const d = tmp();
  const nt = path.join(d, "x.nt");
  fs.writeFileSync(nt, '<urn:s> <urn:p> "v" .\n<urn:s> <urn:q> <urn:o> .\n');
  const db = new tl.Database(path.join(d, "db"));
  assert.strictEqual(await db.load(nt), 2);
  assert.strictEqual((await db.query("SELECT * WHERE { ?s ?p ?o }")).bindings.length, 2);
  db.close();
});
