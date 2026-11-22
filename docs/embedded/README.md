# Embedded Rust API

`triplelight::embedded::Database` opens a local persistent disk store in-process. It does not require a server or HTTP. Add triplelight as a Rust dependency and use:

```rust,no_run
use triplelight::embedded::Database;
use triplelight::exec::QueryResult;

let db = Database::open("./example.triplelight")?;
db.update("INSERT DATA { <https://example/s> <https://example/p> <https://example/o> }")?;
let read = db.read();
match read.query("ASK { <https://example/s> <https://example/p> <https://example/o> }")? {
    QueryResult::Boolean(found) => assert!(found),
    _ => unreachable!(),
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`DatabaseOptions` can initialize a new directory from N-Triples. `query` returns owned results; `read` creates a stable view retained across later updates and compactions. `update` commits atomically and synchronizes the WAL before returning. `load` imports `.nt` or `.ttl` into an existing store atomically, parsing the entire input in memory before committing. `query_json` batches dictionary decoding for SELECT/ASK into SPARQL Results JSON. `compact` creates a new disk generation. The query and update implementation supports a subset of SPARQL; unsupported operations return an error. Results are currently materialized in memory.

One process may open a directory at a time; share one database among application threads, for example through `Arc<Database>`. An advisory OS file lock rejects concurrent opens before recovery and remains held until the store drops. Programs and older TripleLight builds that ignore the lock must not open the directory concurrently; do not delete or replace its `LOCK` file. File-lock semantics depend on the filesystem. This interface currently supports Unix platforms and Rust 1.89 or later. It is an initial Rust interface, not a stable ABI, C/Python binding, full SPARQL implementation, or a proven multi-process server.

## Async

With the `async` feature, `triplelight::async_embedded` offers the same database as an async API that works on any
executor (tokio, async-std, smol or your own). It doesn't depend on any runtime.

```toml
[dependencies]
triplelight = { git = "https://github.com/odysa/triplelight", features = ["async"] }
```

```rust
use triplelight::async_embedded::Database;

let db = Database::open("./example.db").await?;
db.update("INSERT DATA { <https://ex/s> <https://ex/p> <https://ex/o> }").await?;
let json = db.query_json("ASK { <https://ex/s> <https://ex/p> <https://ex/o> }").await?;
let snapshot = db.read().await?; // a stable view, unaffected by later updates
```

- **How it runs:** query execution and storage are synchronous. Each call runs on a worker thread and returns its
  result through an [AsyncBand](https://github.com/apache/asyncband) oneshot, so the executor thread never blocks.
- **Concurrency:** at most `min(cores, 8)` calls of one database run at once, and clones share that limit. Change it
  with `Database::with_concurrency(n)`.
- **Cancellation:** dropping a future that is still waiting for its turn cancels it. Once its work has started, it
  runs to the end: an update whose future you drop may still commit.
- **Wrapping a sync handle:** `Database::from_sync` wraps a database you opened synchronously, and `as_sync` returns
  the sync handle for calls the async API doesn't wrap.
- **Limits:** this is not native async disk I/O, and it isn't available in the WebAssembly build.

## Bulk disk build

`triplelight build-disk source.nt new-directory --build-external` uses a scratch-backed N-Triples builder. `--parallel-load=N` sets ingestion workers (default: available cores; `1` selects the serial path). The external builder uses at most eight parsing workers and three concurrent index builders, reducing worker count if the configured budget cannot fit their run buffers. Turtle uses the existing in-memory path.

`--build-memory-mb=N` is an **advisory** strategy and run-buffer target, not a process RSS limit. File buffers, metadata, allocator overhead, concurrent work and caches consume additional memory; the serial fallback keeps its existing 128 MiB run buffer even if a smaller target is requested. For example, one exploratory LUBM-100 session observed up to 2.62 GB maximum RSS at a 1 GiB setting. Scratch disk use is not included in the setting.
