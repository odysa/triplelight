"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { Database } = require("..");

function count() {
  return {
    total: 0,
    step(term) {
      if (term.termType === "Literal") this.total++;
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

test("aggregate state belongs to each group, query and captured registry", async () => {
  const db = Database.inMemory();
  const earlier = db.read();
  const grouped = `SELECT ?g (<urn:node:count>(DISTINCT ?v) AS ?n) WHERE {
    VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
  } GROUP BY ?g ORDER BY ?g`;
  db.registerAggregateFunction("urn:node:count", count);
  assert.throws(() => db.registerFunction("urn:node:count", () => null), /already registered/);
  assert.throws(() => earlier.querySync(grouped), { kind: "UnsupportedError" });
  const current = db.read();
  db.close();
  const empty = "SELECT (<urn:node:count>(?v) AS ?n) WHERE { FILTER(false) }";
  for (const query of [grouped, empty]) {
    const expected = current.querySync(query.replace("<urn:node:count>", "COUNT"));
    assert.deepEqual(current.querySync(query), expected);
    assert.deepEqual(await Promise.all([current.query(query), current.query(query)]), [expected, expected]);
  }
});

test("contains callback failures and cancels an active aggregate", async () => {
  const db = Database.inMemory();
  db.registerAggregateFunction("urn:node:create", async () => { throw new Error("async factory"); });
  db.registerAggregateFunction("urn:node:step", () => ({
    async step() { throw new Error("async step"); },
    finish() { return count().finish(); },
  }));
  db.registerAggregateFunction("urn:node:finish", () => ({
    step() {},
    finish() { throw new Error("finish failed"); },
  }));
  const result = await db.query(`SELECT (<urn:node:create>(?v) AS ?a) (<urn:node:step>(?v) AS ?b)
    (<urn:node:finish>(?v) AS ?c) WHERE { VALUES ?v { "x" } }`);
  assert.deepEqual(result.bindings.map(Object.keys), [[]]);

  const controller = new AbortController();
  let finished = false;
  db.registerAggregateFunction("urn:node:abort", () => ({
    step() { controller.abort(); },
    finish() { finished = true; return null; },
  }));
  await assert.rejects(db.query('SELECT (<urn:node:abort>(?v) AS ?n) WHERE { VALUES ?v { "x" } }',
    { signal: controller.signal }), { kind: "InterruptedError", timedOut: false });
  assert.equal(finished, false);
  assert.equal(db.querySync("ASK {}").boolean, true);
  db.close();
});

test("terminating a Node worker releases in-flight aggregate callbacks", () => {
  const script = `
    const { Worker } = require("node:worker_threads");
    const worker = new Worker(\`
      const { parentPort, workerData } = require("node:worker_threads");
      const { Database } = require(workerData);
      const db = Database.inMemory();
      db.registerAggregateFunction("urn:slow", () => ({
        step() {
          parentPort.postMessage("stepped");
          Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
        },
        finish() { return null; },
      }));
      for (let i = 0; i < 4; i++) {
        db.query("SELECT (<urn:slow>(?v) AS ?n) WHERE { VALUES ?v { 1 2 3 } }");
      }
    \`, { eval: true, workerData: ${JSON.stringify(require.resolve(".."))} });
    worker.once("message", () => worker.terminate().then(() => console.log("terminated")));
  `;
  const child = spawnSync(process.execPath, ["-e", script], {
    encoding: "utf8", timeout: 10000, killSignal: "SIGKILL",
  });
  assert.ifError(child.error);
  assert.equal(child.status, 0, child.stderr);
  assert.match(child.stdout, /terminated/);
});
