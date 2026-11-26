const test = require("node:test");
const assert = require("node:assert/strict");
const { Database } = require("../pkg-node/triplelight_wasm.js");

function count() {
  return {
    total: 0,
    step(value) {
      if (value.termType === "Literal") this.total++;
    },
    finish() {
      return {
        termType: "Literal",
        value: String(this.total),
        language: "",
        datatype: { termType: "NamedNode", value: "http://www.w3.org/2001/XMLSchema#integer" },
      };
    },
  };
}

test("aggregate group state and registration belong to the captured snapshot", async () => {
  const db = new Database();
  const earlier = db.read();
  const grouped = `SELECT ?g (<urn:count>(DISTINCT ?v) AS ?n) WHERE {
    VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
  } GROUP BY ?g ORDER BY ?g`;
  const empty = "SELECT (<urn:count>(?v) AS ?n) WHERE { FILTER(false) }";
  db.registerAggregateFunction("urn:count", count);
  assert.throws(() => db.registerFunction("urn:count", () => null), { kind: "RegistrationError" });
  assert.throws(() => earlier.querySync(grouped), { kind: "UnsupportedError" });
  earlier.free();
  const current = db.read();
  db.free();
  for (const query of [grouped, empty]) {
    const builtin = query.replace("<urn:count>", "COUNT");
    assert.deepEqual(await current.query(query), current.querySync(builtin));
  }
  current.free();
});

test("invalid aggregate callbacks leave unbound results", async () => {
  const db = new Database();
  db.registerAggregateFunction("urn:create", async () => { throw new Error("async factory"); });
  db.registerAggregateFunction("urn:step", () => ({
    async step() { throw new Error("async step"); },
    finish() { return count().finish(); },
  }));
  db.registerAggregateFunction("urn:finish", () => ({
    step() {},
    async finish() { throw new Error("async finish"); },
  }));
  const result = await db.query(`SELECT (<urn:create>(?v) AS ?a) (<urn:step>(?v) AS ?b)
    (<urn:finish>(?v) AS ?c) WHERE { VALUES ?v { "x" } }`);
  assert.deepEqual(result.bindings.map(Object.keys), [[]]);
  db.free();
});
