const test = require("node:test");
const assert = require("node:assert/strict");
const { Database } = require("../pkg-node/triplelight_wasm.js");

test("scalar terms are identical across live and snapshot query APIs", async () => {
  const db = new Database();
  const earlier = db.read();
  db.registerFunction("urn:echo", (term) => term, 1, true);
  const snap = db.read();
  const query = `SELECT (<urn:echo>(?value) AS ?result) WHERE {
    VALUES ?value { <urn:item> "hello"@en "003"^^<urn:custom> }
  }`;
  assert.throws(() => earlier.querySync(query), { kind: "UnsupportedError" });
  assert.throws(() => db.registerFunction("urn:echo", () => null), { kind: "RegistrationError" });
  assert.throws(() => db.registerFunction("urn:invalid", () => null, 0.5), { kind: "RegistrationError" });
  const live = db.querySync(query);
  assert.deepEqual(await db.query(query), live);
  assert.deepEqual(live.bindings.map(({ result }) => result.value).sort(), ["003", "hello", "urn:item"]);
  const byValue = new Map(live.bindings.map(({ result }) => [result.value, result]));
  assert.equal(byValue.get("003").datatype.value, "urn:custom");
  assert.equal(byValue.get("hello").language, "en");
  assert.equal(byValue.get("urn:item").termType, "NamedNode");
  const json = db.queryJsonSync(query);
  assert.equal(await db.queryJson(query), json);
  db.free();
  assert.deepEqual(snap.querySync(query), live);
  assert.equal(await snap.queryJson(query), json);
  snap.free();
  earlier.free();
});

test("callback errors become unbound expressions without poisoning other rows", () => {
  const db = new Database();
  db.registerFunction("urn:bad", () => { throw new Error("callback failed"); }, 0);
  db.registerFunction("urn:getter", () => ({ get value() { throw new Error("getter failed"); } }), 0);
  db.registerFunction("urn:invalid", (term) => ({ ...term, language: "en" }), 1);
  db.registerFunction("urn:promise", async (term) => term, 1);
  db.registerFunction("urn:rejected", async () => { throw new Error("rejected"); }, 0);
  db.registerFunction("urn:echo", (term) => term, 1);
  db.loadSync("_:person <urn:p> <urn:o> .");
  const result = db.querySync(`SELECT ?s (<urn:echo>(?s) AS ?echo)
    (<urn:bad>() AS ?bad) (<urn:getter>() AS ?getter)
    (<urn:invalid>("x") AS ?invalid) (<urn:promise>("x") AS ?promise)
    (<urn:rejected>() AS ?rejected)
    (<urn:echo>() AS ?wrongArity) WHERE { ?s <urn:p> <urn:o> }`);
  assert.deepEqual(Object.keys(result.bindings[0]).sort(), ["echo", "s"]);
  assert.deepEqual(result.bindings[0].s, result.bindings[0].echo);
  assert.equal(result.bindings[0].s.termType, "BlankNode");
  db.free();
});
