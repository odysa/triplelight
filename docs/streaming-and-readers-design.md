# Streaming results and multi-process readers (design, task #112)

Status: proposal, read from the code at main `15514671`+; nothing here is implemented. It is written for review by the executor
(Kaiming) and storage (Alex) lanes before any code.

## 1. Streaming query results

**What happens today.** `query_with` returns `QueryResult::Ids(IdSolutions)`: every result row as term IDs, in memory. The server
(`src/bin/server.rs`, `answer`) then calls `serialize_ids_v`, which builds the whole SPARQL-JSON body as one `String` (parallel 4,096-row
chunks, decoded in bounded windows, but appended into one buffer), and sends it with `Response::from_string`, so with a `Content-Length`.
Nothing reaches the client until execution and serialization have both finished. For LUBM-1000 Q6 (7,924,765 rows) the server spends 0.46 s
executing and 0.40 s serializing, and the body is 761,668,080 bytes (`docs/large-answer-timing-v1`), so peak memory is the IDs (about 63 MB for
one column) plus a 761 MB body.

**What streaming can and cannot change.** The executor is not pipelined (BGP extension is level by level over flat row buffers), so the first
row cannot leave before the last join level has produced its rows. What can change is everything after execution:

1. *Serialize into a writer, not a `String`.* `serialize_ids_to(&IdSolutions, dict, &mut impl Write)`: the same windowed parallel chunks, each
   written in order as soon as it is ready. Peak memory drops by the size of the body; first byte moves from "execution + full
   serialization" to "execution + first window" (for Q6, about 0.46 s instead of about 0.86 s on the laptop run), and serialization overlaps
   the socket write.
2. *HTTP with chunked transfer encoding.* A producer thread serializes windows into a bounded channel (two windows) and the HTTP thread
   reads it as a `Read` for `tiny_http::Response::new(...)`; the bound is the back-pressure, so a slow client holds at most two windows.
   The first window is produced before the status line is sent, so the common errors (a dictionary read failure on the first rows) still
   answer 500.
3. *Failure after the first byte.* The status is already sent. The stream is ended without the terminating chunk and without the closing
   `]}}`, so the client sees a truncated chunked body (a client or the benchmark driver reports it as incomplete, as it does for
   Virtuoso's row cap), never a complete-looking wrong answer. The server logs the error.
4. *Formats.* JSON first (it is the only id-based path); CSV and TSV and CONSTRUCT keep the buffered path until measured. HTTP/1.0 clients
   and `HEAD` keep the buffered `Content-Length` response.
5. *Embedded API.* `Database::query_to_writer(sparql, format, &mut impl Write)` (and `ReadSnapshot::...`) for callers that want bounded
   memory without HTTP; `query` is unchanged.

**A second step, not promised here.** For a plain `SELECT` over one BGP without `ORDER BY`, `DISTINCT` or grouping, the last extension level
could hand each morsel to the serializer as it is produced, and `LIMIT` could stop the executor early. How much that buys depends on the
share of time spent in the last level; measure with the phase trace before building.

**Tests.** Byte-identical output to the buffered path for every query of the existing suites (a differential over `serialize_ids_v`); a
failure injected after the first window ends the stream without the closing braces; a slow reader keeps memory bounded (peak RSS on Q6
against the buffered path); empty and one-row results; `ASK` and `CONSTRUCT` unchanged; `bytecheck.sh` still sees a constant size.

## 2. Multi-process readers of one store

**What happens today** (`crates/triplelight-storage/src/store.rs`). Opening a store directory takes an exclusive `LOCK` (retried briefly, then
`WouldBlock`, "database directory is already open"). Under that lock the open replays the WAL, truncates a torn tail, deletes `base-*`
directories that `CURRENT` does not name, and may rewrite `CURRENT` (the RDF 1.2 and named-graph fences). The writer appends WAL records
and fsyncs; compaction writes `base-<n+1>`, atomically replaces `CURRENT`, truncates the WAL, and deletes the old base. So a second process
cannot open the store at all, and even a read-only second open would be destructive today (it would delete bases and truncate the WAL).

**Proposal, in two steps.**

*Step A: read-only open, readers or one writer.* `open_read_only(dir)` takes a shared `flock` on `LOCK`; it never writes: it reads `CURRENT`,
opens the base, reads the WAL up to the last complete record (a record ends with the line `C`; a torn tail is ignored, not truncated), and
deletes nothing. Many processes can serve the same store (read replicas), each with its own cache budget. A writer's exclusive lock fails
while any reader is open, and the reverse, with a clear message in both directions. CLI and server: `--read-only` on `serve-store`.

*Step B: readers during writes.* The writer keeps the exclusive lock on a separate `WRITER` file; readers take `READER` shared. A reader
refreshes at the start of a query: it compares `CURRENT` (compaction switched bases) and the committed WAL length, and applies new
records to a fresh overlay. Two points for Alex to decide, because they touch the format:
- *Visibility vs durability.* A reader must read only fsynced records. A small `WAL.committed` file (the length after each fsync, written
  atomically) gives a safe upper bound without changing the WAL record format.
- *Compaction under a reader.* On POSIX an open base survives its deletion, so a reader keeps its snapshot; a reader that read `CURRENT`
  but not yet opened the base retries. Windows cannot delete open files, so Step B is Unix first.

**Failure modes to test.** Writer killed mid-append (readers see only complete records); torn tail; compaction while a reader runs a long
query; many readers; reader opened before the first update; a store with named graphs or RDF 1.2 terms (the fence must be honoured read-only:
an old reader must refuse, not rewrite `CURRENT`); a lock held by a crashed process (`flock` releases on exit).

**Out of scope.** Network file systems (advisory locks are unreliable there; document, do not claim), multiple writers, and replication.

## 3. Order of work and what each needs
1. Streaming serialization and HTTP (executor-neutral; me). 2. `open_read_only` and `--read-only` (needs Alex to review the open path).
3. Step B only after Alex agrees to `WAL.committed` and the reader refresh.
