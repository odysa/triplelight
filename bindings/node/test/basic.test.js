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
  await assert.rejects(db.query("SELECT * WHERE { SERVICE <urn:remote> { ?s ?p ?o } }"), tl.UnsupportedError);
  assert.throws(() => db.querySync("SELEC"), tl.TripleLightError);
  assert.throws(() => new tl.Database(dir), tl.LockedError);
  await db.update("CREATE GRAPH <urn:g>");
  await assert.rejects(db.update("CREATE GRAPH <urn:g>"), (e) => e instanceof tl.GraphStateError && e.kind === "GraphStateError");
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

test("a snapshot keeps the directory locked after close, including an in-flight query", async () => {
  const dir = path.join(tmp(), "db");
  const db = await tl.Database.open(dir);
  await db.update("INSERT DATA { <urn:a> <urn:p> 1 }");
  const snap = db.read();
  const pending = snap.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }");
  db.close();
  assert.throws(() => new tl.Database(dir), tl.LockedError);
  assert.strictEqual((await pending).bindings.length, 1);
  assert.strictEqual(snap.querySync("ASK { <urn:a> <urn:p> 1 }").boolean, true);
});

test("variables named like Object.prototype members are ordinary bindings", async () => {
  const db = await tl.Database.open(path.join(tmp(), "db"));
  const r = await db.query('SELECT ("x" AS ?__proto__) ("y" AS ?constructor) WHERE {}');
  const b = r.bindings[0];
  assert.strictEqual(Object.getPrototypeOf(b), null);
  assert.deepStrictEqual(Object.keys(b).sort(), ["__proto__", "constructor"]);
  assert.strictEqual(b["__proto__"].value, "x");
  assert.strictEqual(b.constructor.value, "y");
  db.close();
});

test("RDF 1.2 triple terms and base direction (skipped on a build without the rdf-12 feature)", async (t) => {
  const db = tl.Database.inMemory();
  try {
    await db.update('INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> "v" )>> . <urn:a> <urn:label> "שלום"@he--rtl }');
  } catch (e) {
    if (process.env.TRIPLELIGHT_EXPECT_RDF12) throw e; // the build is supposed to have rdf-12: a refusal is a failure
    t.skip("built without the rdf-12 feature");
    return;
  }
  const r = await db.query("SELECT ?p ?o WHERE { <urn:a> ?p ?o }");
  const byP = Object.fromEntries(r.bindings.map((b) => [b.p.value, b.o]));
  const q = byP["urn:says"];
  assert.strictEqual(q.termType, "Quad");
  assert.strictEqual(q.subject.value, "urn:b");
  assert.strictEqual(q.predicate.value, "urn:p");
  assert.strictEqual(q.object.value, "v");
  assert.strictEqual(q.graph.termType, "DefaultGraph");
  assert.strictEqual(byP["urn:label"].language, "he");
  assert.strictEqual(byP["urn:label"].direction, "rtl");
  db.close();
});

test("RDFS entailment is opt-in per query", async () => {
  const db = tl.Database.inMemory();
  await db.update("INSERT DATA { <urn:ann> a <urn:Student> . <urn:Student> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Person> }");
  const q = "SELECT ?x WHERE { ?x a <urn:Person> }";
  assert.strictEqual((await db.query(q)).bindings.length, 0);
  const r = await db.query(q, { entailment: "rdfs" });
  assert.deepStrictEqual(r.bindings.map((b) => b.x.value), ["urn:ann"]);
  assert.strictEqual(db.querySync(q, { entailment: "rdfs" }).bindings.length, 1);
  assert.ok((await db.queryJson(q, { entailment: "rdfs" })).includes("urn:ann"));
  assert.strictEqual(db.read().querySync(q, { entailment: "rdfs" }).bindings.length, 1);
  assert.throws(() => db.querySync(q, { entailment: "owl" }), /entailment must be/);
  db.close();
});

test("SHACL validate: reports, syntax errors and rejected shapes", () => {
  const shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ; sh:property [ sh:path <urn:name> ; sh:minCount 1 ] .";
  assert.strictEqual(tl.validate('<urn:ann> a <urn:Person> ; <urn:name> "Ann" .', shapes).conforms, true);
  const bad = tl.validate("<urn:bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .", shapes, { dataFormat: "ntriples" });
  assert.strictEqual(bad.conforms, false);
  const focus = bad.triples.filter((t) => t.predicate.value === "http://www.w3.org/ns/shacl#focusNode").map((t) => t.object);
  assert.deepStrictEqual(focus.map((o) => [o.termType, o.value]), [["NamedNode", "urn:bob"]]);
  assert.throws(() => tl.validate("<urn:bob> a", shapes), tl.SyntaxError);
  assert.throws(() => tl.validate("", "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:S> a sh:NodeShape ; sh:targetClass <urn:P> ; sh:property [ sh:minCount 1 ] ."), tl.ShapesError);
});

test("query limits: AbortSignal, timeoutMs and memoryMb", async () => {
  const db = tl.Database.inMemory();
  const data = Array.from({ length: 2000 }, (_, i) => `<urn:s${i}> <urn:p> "${i}" .`).join(" ");
  await db.update(`INSERT DATA { ${data} }`);
  const endless = "SELECT (COUNT(*) AS ?n) { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f FILTER(?b != ?d && ?d != ?f) }";
  const grouped = "SELECT ?a (COUNT(*) AS ?n) { ?a ?p ?b } GROUP BY ?a";
  // Aborting a running query rejects it, and the worker is free for the next query.
  const controller = new AbortController();
  const running = db.queryJson(endless, { signal: controller.signal });
  setTimeout(() => controller.abort(), 100);
  const started = Date.now();
  await assert.rejects(running, (e) => e instanceof tl.InterruptedError && e.timedOut === false);
  assert.ok(Date.now() - started < 10000);
  assert.strictEqual((await db.query("ASK { <urn:s1> <urn:p> \"1\" }")).boolean, true);
  // An aborted signal cancels at once.
  await assert.rejects(db.query(grouped, { signal: AbortSignal.abort() }), tl.InterruptedError);
  // A deadline, also on a snapshot and synchronously.
  await assert.rejects(db.read().query(endless, { timeoutMs: 50 }), (e) => e instanceof tl.InterruptedError && e.timedOut);
  assert.throws(() => db.querySync(endless, { timeoutMs: 50 }), tl.InterruptedError);
  // A memory budget.
  // Four million groups: far more than one megabyte of working memory.
  const wide = "SELECT ?a ?c (COUNT(*) AS ?n) { ?a ?p ?b . ?c ?q ?d } GROUP BY ?a ?c";
  await assert.rejects(db.query(wide, { memoryMb: 1 }), tl.ResourceLimitError);
  assert.strictEqual((await db.query(grouped, { memoryMb: 1024, timeoutMs: 60000 })).bindings.length, 2000);
  assert.throws(() => db.querySync(grouped, { memoryMb: 0 }), /memoryMb must be positive/);
  db.close();
});
