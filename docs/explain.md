# Inspect the joins a SPARQL query ran

TripleLight chooses some access paths during execution, when it sees input
bindings and index counts. EXPLAIN executes the query once and reports the
observed basic graph pattern (BGP) steps. Use it for read-only diagnostics.
It discards query answers and returns plain text with the join strategy,
pattern order, access paths, available estimates, actual row counts, and
observed time. Timings include trace overhead. Paths, filters, aggregation,
and sorting outside the recorded BGP steps have no separate plan entries.

## Rust

```rust
use triplelight::embedded::Database;
use triplelight::exec::Entailment;

let db = Database::in_memory();
let report = db.explain("SELECT * WHERE { ?s ?p ?o }", Entailment::Simple)?;
println!("{report}");
# Ok::<(), triplelight::embedded::DatabaseError>(())
```

`ReadSnapshot::explain` uses the same pinned view and statistics selection
as its query method. Use the same lightweight or dataset executor entry
point as the query you want to diagnose.

## CLI

```sh
triplelight explain data.nt 'SELECT * WHERE { ?s ?p ?o }'
```

The input rules are shared with `query`. Add `--entailment=rdfs` to use the
RDFS query view. The output goes to stdout, and a query error produces a
nonzero exit and a diagnostic on stderr.

## HTTP

Send a raw SPARQL query to `POST /explain` on either server mode.

```sh
curl --data-binary 'SELECT * WHERE { ?s ?p ?o }' \
  -H 'Content-Type: application/sparql-query' \
  http://127.0.0.1:7878/explain
```

Specify `?entailment=simple` or `?entailment=rdfs` to override the server's
default. The endpoint uses the same graph scope and execution options as
the HTTP query handler, including the configured query timeout. Syntax,
unsupported feature, storage, and timeout errors return 400, 501, 500,
and 503 respectively. The response includes a truncation
notice if the trace exceeds 256 KiB, and an empty-plan notice if no BGP
was traced. The text is intended for people and may change between versions.
