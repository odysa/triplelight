// Compile-only check of index.d.ts: `npx tsc --noEmit --strict test/types-check.ts`.
import { Database, QueryResult, Term, UnsupportedError } from "..";

async function example(): Promise<string[]> {
  const db = await Database.open("./db", { initial: "data.nt" });
  new Database("./db2").close();
  const n: number = await db.update("INSERT DATA { <urn:a> <urn:p> 1 }");
  const r: QueryResult = await db.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }");
  const out: string[] = [String(n)];
  if (r.type === "bindings") {
    for (const b of r.bindings) {
      const t: Term | undefined = b["o"];
      if (t && t.termType === "Literal") out.push(t.datatype.value, t.language);
    }
  } else if (r.type === "boolean") {
    out.push(String(r.boolean));
  } else {
    out.push(r.triples[0].predicate.value);
  }
  const json: string = await db.queryJson("ASK {}");
  out.push(json, String(db.statsReady()));
  try { db.querySync("SELECT * WHERE { SERVICE <urn:remote> { ?s ?p ?o } }"); } catch (e) { if (e instanceof UnsupportedError) out.push(e.kind); }
  db.read().querySync("ASK {}");
  await db.compact();
  db.close();
  return out;
}
void example;
