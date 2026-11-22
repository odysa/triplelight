# Parallel N-Triples loading

**In memory.** `triplelight serve data.nt 7878` and `triplelight query data.nt ...` load N-Triples into memory with
one worker per CPU (`available_parallelism`). The file is split at line boundaries and the chunks are parsed in
parallel. Terms then enter the dictionary serially, deduplicated triples go into SPO, and the other five orders are
derived from SPO with a parallel sort. Everything is held in memory, so this is not an external-memory loader.
`--parallel-load=N` sets the worker count, and `--parallel-load=1` takes the serial path. Turtle is parsed serially,
and asking for more than one worker with a `.ttl` input exits with an error. The code is
`MemorySnapshot::from_ntriples_parallel_file` in `crates/triplelight-storage/src/memory.rs`.

**Disk builds.** `triplelight build-disk` uses the same loader when it takes the in-memory path (see
[persistence.md](persistence.md#building-a-base)). The external builder parses the file in parallel with at most 8
workers and builds the triple orders with at most 3 workers at a time; `--parallel-load=N` caps the parse workers.

**Benchmarks.** `bench/suite.sh lubm100 triplelight-parallel` runs the server with `--parallel-load=4`. Its RSS figure
is a sample taken after the queries, not a peak.
