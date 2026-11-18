# Planned matched-VM BI measurement

Status: plan; no BI timings have been collected. The running host cohort is
correctness preparation, separate from this Linux/VM performance comparison.
The numeric acceptance source and inputs remain fixed by freeze
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.

## Work and result accounting

Use the existing 60-instance manifest: one complete 15-query warmup mix, followed
by the three measured mixes. These are the declared bounded-cohort settings,
not the official BSBM 50/500-mix throughput procedure. Keep every scheduled
query, failure, timeout and result. No complete-cohort speed ranking is issued
unless every required output qualifies. Mix 0 remains exploratory and unscored.

Run one engine/process group at a time under the shared machine lock, with a
reversed engine order for a second session pass. Record the individual passes
and parameter-instance measurements, not just template averages. Do not infer
statistical equivalence or a global SOTA result from two passes. Compare sums of
complete measured-mix elapsed times and also show per-template distributions.

End-to-end request timing includes HTTP transport and complete body consumption.
Use the same client process and host-to-VM path for all endpoints. Validate the
fully parsed response outside the request timer, preserve its raw bytes and
record size. Identify this as HTTP product latency, distinct from native
embedded API timings. Record setup/build/import separately.

## Runtime and data identities

Pin Linux ARM64 binaries, source commits, image digests, library versions,
query hashes, input SHA-256, graph triple count and engine flags. Verify the
actual running server owns the selected port and returns the pinned graph's
answers. Never reuse a pre-existing service or silently substitute an old
binary after a failed build. Build inside copied Linux source, following the
project's stale-shared-mount precaution.

Rdfdb variants are main with ID aggregation, the same binary with
RDFDB_LEGACY_GROUP=1, and the separately validated wider-accumulation candidate.
Use a common source base before attributing a change to wider accumulation.
Keep input IDs/store contents and source controls fixed across these variants.
Wait for statistics readiness before requests. Report each variant separately.

DuckDB uses the existing lossless plain-triples schema and hand-written SQL
translations, with no property tables, materialized joins or aggregate cache.
SQL parsing/planning/execution belongs inside its timed request. The protocol
adapter's mapping from a registered SPARQL instance to its SQL text is a declared
workload adapter, not a general SPARQL frontend. Use explicit memory/spill/thread
settings and preserve actual RSS/spill observations separately from targets.

Virtuoso queries must select the intended benchmark graph explicitly through
SPARQL protocol dataset parameters. Its row cap must not truncate answers.
Qualify graph identity and all output columns before timing.

## QLever configuration and query-result caching

The existing helper's positive 1 MB cache limits do not establish disabled
caching for small BI results. The official
[server options](https://github.com/ad-freiburg/qlever/blob/master/src/ServerMain.cpp)
describe them as entry/capacity limits. Verify the pinned executable's actual
configuration and observe its cache behavior before measuring.

The primary execution profile clears query/intermediate-result cache between
requests, outside the timer, while retaining normal within-query execution and
storage-buffer warming. Validate the clear operation on our own server and
preserve its responses. Do not clear another person's service or filesystem
cache. No whole-answer cache hits may silently count as fresh execution.

If supported by the pinned image, additionally record the explicit cache-bypass
profile. The upstream
[cache-bypass change](https://github.com/ad-freiburg/qlever/pull/2758)
introduces `disable-caching=true`, which bypasses reads/writes and key creation.
That differs from a cache-cold query with ordinary within-query reuse; report
profiles separately instead of conflating their costs.

Set QLever’s working-memory limit to at least the 10 GB rdfdb cache budget,
and retain its exact flag/value. This is an execution-memory setting, not an
assertion about total RSS or identical memory accounting across engines.

Record QLever's default grouping configuration and an explicit
`group-by-hash-map-enabled=true` profile if supported. The inspected upstream
RuntimeParameters.h defaults that option to false (file blob
44127456ead05678e4bb2d2dcae35e12d961322b). Preserve both configurations rather
than reporting only whichever result favors rdfdb. Current upstream source is
planning evidence; the actual pinned image/settings are authoritative at run time.

## Resource and publication boundaries

Observe the shared host's 40 GB free-space floor. Current owned RDF scratch was
about 9.5 GiB before Linux competitor staging. Measure additional allocations;
request infrastructure approval before exceeding 20 GB. The 100M track follows
10M sizing and needs its own explicit resource plan. Do not infer larger-than-RAM
support from configured cache sizes.

Retain quality status beside latency. Datatype, membership, integer-count or
numeric-envelope failures are not successful zero-cost queries, and cannot be
removed from a headline total. A separate exploratory timing with visible
failures may help diagnosis, but does not satisfy the complete BI comparison.

## Prepared request runner (not yet integration-qualified)

`http_measure.py` consumes one explicitly configured endpoint, the pinned
workload, retained Jena outputs and immutable numeric inputs. It times through
complete HTTP body consumption and validates afterward. Raw success/error bodies
and failed attempts are retained; any scheduled failure suppresses the complete
measured total. QLever profiles require separate evidence that their cache
control works, plus explicit clear acknowledgements for the cache-cold profile.
This runner neither provisions a server nor verifies a claimed configuration.

Seven transport/adapter unit checks cover the timer/body-read boundary, raw HTTP error retention,
timeout classification, q7 RDF-kind validation, repeated SQL execution and engine-error handling. `duck_http.py` is a serial registered-workload adapter with read-only storage and explicit resource settings. Real matched-VM integration,
cache-control verification, and memory/spill observation remain pending; these unit checks do not qualify a competitor comparison.

For cache-clear preparation, upstream Server.cpp blob
`9a70a4adbf83ce0eac87b1ebfd82631d552dd1d8` returns cache statistics after
`cmd=clear-cache` (unpinned entries only); complete clearing requires access-token
authorization. Verify this against the actual pinned executable. A dedicated
benchmark server must have no pinned results before using the unprivileged
clear operation. The [official settings documentation](https://docs.qlever.dev/qleverfile/)
also documents the full cache bypass. No cache-control command has yet been
issued to a live QLever process in this BI track.
