# TripleLight for Node.js

The embedded TripleLight RDF database in a Node.js process: SPARQL 1.1 queries and updates on a local directory, no
server. Query evaluation, updates and `Database.open` run on worker threads and return Promises; converting a result
to JS objects happens on the event loop. Terms are plain objects shaped like the
[RDF/JS data model](https://rdf.js.org/data-model-spec/) (`termType`, `value`, `language`, `datatype`), without
`equals()` methods.

```js
const { Database } = require("triplelight");

const db = await Database.open("./my.db");
await db.update('INSERT DATA { <urn:alice> <urn:age> 30 }');
const r = await db.query("SELECT ?who ?age WHERE { ?who <urn:age> ?age }");
for (const b of r.bindings) console.log(b.who.value, b.age.value); // urn:alice 30
db.close();
```

Install: `npm install triplelight`. The package carries one prebuilt addon per platform and loads the one that matches
`process.platform` and `process.arch`:

| Platform | Requirement |
|---|---|
| macOS arm64 (Apple Silicon) | none beyond Node 18+ |
| Linux x64 and Linux arm64 | glibc 2.31 or newer (Debian 11, Ubuntu 20.04, RHEL 9 and later); older systems such as RHEL 8, and musl (Alpine), are not supported |

The download is 5.9 MB and the unpacked package 15.3 MB (three addons of 4.5 to 5.7 MB each). On any other platform, build from source.

Build from source (Rust 1.100, Node 18+, Linux or macOS): `npm run build`, then `npm test`.
Types are in `index.d.ts`. Errors are `TripleLightError` subclasses: `SyntaxError`, `UnsupportedError`,
`StorageError`, `LockedError` (the directory is open in another process) and `GraphStateError` (an update names a
graph in the wrong state, such as `CREATE` of an existing graph).
