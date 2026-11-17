# Embedded Rust API

`rdfdb::embedded::Database` opens a local persistent disk store in-process. It does not require a server or HTTP. Add rdfdb as a Rust dependency and use:

```rust,no_run
use rdfdb::embedded::Database;
use rdfdb::exec::QueryResult;

let db = Database::open("./example.rdfdb")?;
db.update("INSERT DATA { <https://example/s> <https://example/p> <https://example/o> }")?;
let read = db.read();
match read.query("ASK { <https://example/s> <https://example/p> <https://example/o> }")? {
    QueryResult::Boolean(found) => assert!(found),
    _ => unreachable!(),
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`DatabaseOptions` can initialize a new directory from N-Triples. `query` returns owned results; `read` creates a stable view retained across later updates and compactions. `update` commits atomically and synchronizes the WAL before returning. `load` imports `.nt` or `.ttl` into an existing store atomically, parsing the entire input in memory before committing. `query_json` batches dictionary decoding for SELECT/ASK into SPARQL Results JSON. `compact` creates a new disk generation. The query and update implementation supports a subset of SPARQL; unsupported operations return an error. Results are currently materialized in memory.

One process may open a directory at a time; share one database among application threads, for example through `Arc<Database>`. An advisory OS file lock rejects concurrent opens before recovery and remains held until the store drops. Programs and older rdfdb builds that ignore the lock must not open the directory concurrently; do not delete or replace its `LOCK` file. File-lock semantics depend on the filesystem. This interface currently supports Unix platforms and Rust 1.89 or later. It is an initial Rust interface, not a stable ABI, C/Python binding, full SPARQL implementation, or a proven multi-process server.

## Bulk disk build

`rdfdb build-disk source.nt new-directory --build-external` uses a scratch-backed N-Triples builder. `--parallel-load=N` sets ingestion workers (default: available cores; `1` selects the serial path). The external builder uses at most eight parsing workers and three concurrent index builders, reducing worker count if the configured budget cannot fit their run buffers. Turtle uses the existing in-memory path.

`--build-memory-mb=N` is an **advisory** strategy and run-buffer target, not a process RSS limit. File buffers, metadata, allocator overhead, concurrent work and caches consume additional memory; the serial fallback keeps its existing 128 MiB run buffer even if a smaller target is requested. For example, one exploratory LUBM-100 session observed up to 2.62 GB maximum RSS at a 1 GiB setting. Scratch disk use is not included in the setting.
