const test = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { Database, UnsupportedError } = require("..");

const echo = `SELECT (<urn:echo>(?x) AS ?y) WHERE {
  VALUES ?x { "003"^^<urn:type> "he\u0301"@fr <urn:item> }
}`;

test("scalar functions run on the JavaScript thread from all query surfaces", async () => {
  const db = Database.inMemory();
  const earlier = db.read();
  const hostValues = [];
  db.registerFunction("urn:echo", (value) => {
    hostValues.push(value.termType);
    return value;
  }, 1);
  assert.throws(() => db.registerFunction("urn:echo", () => null), /already registered/);
  assert.throws(() => db.registerFunction("urn:f", () => null, 0.5), /arity/);
  assert.throws(() => earlier.querySync(echo), UnsupportedError);
  const snapshot = db.read();
  const expected = db.querySync(echo);
  assert.deepEqual(await db.query(echo), expected);
  const byValue = new Map(expected.bindings.map(({ y }) => [y.value, y]));
  assert.equal(byValue.get("003").datatype.value, "urn:type");
  assert.equal(byValue.get("he\u0301").language, "fr");
  assert.equal(byValue.get("urn:item").termType, "NamedNode");
  const json = await db.queryJson(echo);
  const inFlight = snapshot.query(echo);
  db.close();
  assert.deepEqual(await inFlight, expected);
  assert.deepEqual(snapshot.querySync(echo), expected);
  assert.equal(await snapshot.queryJson(echo), json);
  assert.equal(hostValues.length, 18);
});

test("callback exceptions, Promise results and malformed RDF are expression errors", async () => {
  const db = Database.inMemory();
  db.registerFunction("urn:throws", () => { throw new Error("throws"); });
  db.registerFunction("urn:getter", () => ({ get value() { throw new Error("getter"); } }));
  db.registerFunction("urn:promise", async () => { throw new Error("rejected"); });
  db.registerFunction("urn:wrongType", (x) => ({ ...x, language: "en" }));
  db.registerFunction("urn:empty", () => null);
  db.registerFunction("urn:echo", (x) => x, 1);
  const q = `SELECT (<urn:throws>() AS ?a) (<urn:getter>() AS ?b) (<urn:promise>() AS ?c)
    (<urn:wrongType>("x") AS ?d) (<urn:empty>() AS ?e) (<urn:echo>() AS ?f)
    (<urn:echo>("x") AS ?ok) WHERE {}`;
  for (const result of [db.querySync(q), await db.query(q)]) {
    assert.deepEqual(Object.keys(result.bindings[0]), ["ok"]);
    assert.equal(result.bindings[0].ok.value, "x");
  }
  db.close();
});

test("a worker waiting for a callback shares the query interrupt and deadline", async () => {
  const db = Database.inMemory();
  const controller = new AbortController();
  db.registerFunction("urn:abort", () => {
    controller.abort();
    return null;
  });
  await assert.rejects(db.query("SELECT (<urn:abort>() AS ?v) WHERE {}", { signal: controller.signal }),
    { kind: "InterruptedError", timedOut: false });

  db.registerFunction("urn:slow", () => {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
    return null;
  });
  await assert.rejects(db.query("SELECT (<urn:slow>() AS ?v) WHERE {}", { timeoutMs: 100 }),
    { kind: "InterruptedError", timedOut: true });
  db.close();
});

test("terminating a Node worker with scalar calls in flight does not hang", () => {
  // Make an environment teardown deadlock fail this test rather than hanging npm.
  const script = `
    const { Worker } = require("node:worker_threads");
    const worker = new Worker(\`
      const { parentPort, workerData } = require("node:worker_threads");
      const { Database } = require(workerData);
      const db = Database.inMemory();
      db.registerFunction("urn:slow", () => {
        parentPort.postMessage("invoked");
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
        return null;
      });
      for (let i = 0; i < 4; i++) db.query("SELECT (<urn:slow>() AS ?v) WHERE {}");
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

test("scalar callbacks preserve RDF 1.2 terms", async (t) => {
  const db = Database.inMemory();
  try {
    try {
      await db.update('INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> "v" )>> . <urn:a> <urn:label> "שלום"@he--rtl }');
    } catch (error) {
      if (process.env.TRIPLELIGHT_EXPECT_RDF12 || !["UnsupportedError", "SyntaxError"].includes(error.kind)) throw error;
      t.skip("built without rdf-12");
      return;
    }
    db.registerFunction("urn:echo", (value) => value, 1);
    const result = await db.query("SELECT ?o (<urn:echo>(?o) AS ?copied) WHERE { <urn:a> ?p ?o }");
    assert.equal(result.bindings.length, 2);
    for (const row of result.bindings) assert.deepEqual(row.copied, row.o);
  } finally {
    db.close();
  }
});
