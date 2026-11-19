# TripleLight for Node.js

The embedded TripleLight RDF database in a Node.js process: SPARQL 1.1 queries and updates on a local directory, no
server. Queries run on worker threads and return Promises; terms follow the [RDF/JS data model](https://rdf.js.org/data-model-spec/).

```js
const { Database } = require("triplelight");

const db = new Database("./my.db");
await db.update('INSERT DATA { <urn:alice> <urn:age> 30 }');
const r = await db.query("SELECT ?who ?age WHERE { ?who <urn:age> ?age }");
for (const b of r.bindings) console.log(b.who.value, b.age.value); // urn:alice 30
db.close();
```

Build from source (Rust 1.89+, Node 18+, Linux or macOS): `npm run build`, then `npm test`.
Types are in `index.d.ts`. Errors are `TripleLightError` subclasses: `SyntaxError`, `UnsupportedError`,
`StorageError` and `LockedError` (the directory is open in another process).
