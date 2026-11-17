# Embedded Python comparison

Status: harness draft, **no embedded performance measurements yet**. The Rust
facade and Python package are being validated separately. The earlier Ladybug
report measures HTTP and cannot answer this embedded comparison.

Both systems run in their own fresh Python processes, calling public in-process
APIs. `run.py` starts no HTTP server. Import the pinned modules before the open
timer. Use a dedicated, populated durable rdfdb Store (with `CURRENT`), never an
immutable build-disk snapshot or a shared reference directory: open may recover
the WAL and owns the writer lock. Creation/import/compaction are separate
workloads, excluded from this query cohort.

## Measurement contract

Each sample measures these consecutive phases with one monotonic clock:

1. `execute_return_ms`: the public query call, including any eager allocation,
   conversion, parsing and planning it performs. A returned handle alone is not
   evidence of complete work. rdfdb's current Python query builds owned rows here.
2. `materialize_ms`: consume Ladybug's entire Arrow table and convert columns to
   Python lists. rdfdb already owns its Python rows; accessing them adds no fake
   consumption loop to this phase.
3. `semantic_decode_ms`: traverse every row into common RDF-term tuples. Ladybug
   JSON-valued `.term` columns are decoded; rdfdb's documented lossless Python
   conversions are inverted. Allocation of all common result rows is included.
4. **Primary `fully_consumed_ms`**: total of those phases. It includes binding and
   mapping overhead, rather than claiming engine-only execution. All outputs are
   read; no COUNT surrogate, unconsumed cursor or factorized result substitutes.

`--rdfdb-result json` is a separately labeled public API configuration:
`query_json()` runs inside API return, Python JSON parsing is materialization,
and every parsed RDF value is traversed into the same common tuples. Comparing
only its returned JSON string to fully decoded objects would omit required work.

Sorting and complete bag hashing happen afterward. Destruction is also outside
the query clock. Report process memory separately: owning common rows alongside
backend results can have a large peak. This is not a streaming-memory benchmark.
The native internal compile counter can repeat the cached plan's original cost;
it is deliberately not added to wall time.

Record `open_ms`, `ready_ms` (including rdfdb statistics construction), the first
query sample (`warmup=true`), and warmed medians separately. Ladybug uses its
parameterized execute API with its implicit prepared-plan cache. rdfdb uses
background statistics only when the facade's base-generation/delta guard allows
it. Query preparation for the native mapping is outside query timings, just as
conversion/import is; its cost belongs in the build report. OS caches are not
cleared, so label all startup numbers as process-cold, not storage-cold.

Use the same Python version, thread setting and resource window, record exact
binding commits/wheel hashes, data hashes, schema/mapping and database provenance.
Alternate complete A/B/B/A processes in one exclusive session. `--threads` sets
Ladybug and `RAYON_NUM_THREADS`; `--cache-mb` currently applies **only to Ladybug**.
The rdfdb Python draft does not expose a cache option, so report its actual
default instead of claiming matched budgets or a memory hard limit. Generic
and tuned Ladybug schemas remain distinct configurations.

## Correctness

Every repetition, including warmup, must match the complete reference bag:
projected variable names, duplicates, URI/blank/literal kind, exact lexical text,
datatype, language and unbound values. xsd:string/plain and language case are
normalized RDF identities. Distinct lexical integers and derived integer types
stay distinct. Do not map arbitrary numeric terms into Python float. Existing
LUBM/BSBM cohorts contain no blank nodes; this checker does **not** solve blank
node graph isomorphism for a new dataset.

`reference.py` converts independent Jena SELECT JSON exports to the reference
format. A run without `--reference` is explicitly not externally validated.
Capture those exports and their source/data versions; do not use a candidate's
own first result as an independent oracle. Any result/materialization/decoding
error aborts the process and records `complete=false`; it is never treated as an
empty or partial result. A reference mismatch also aborts.

ORDER BY requires an explicit `--order-spec` keyed by query name. Supported keys
are projected, homogeneous lexical or finite numeric terms, ascending or
descending. `bsbm-order.json` also declares the exact `xsd:double(str(?price))`
cast and timezone-free, second-resolution xsd:dateTime keys; unexpected types,
non-finite values or datetime shapes fail. The checker verifies monotonic order while permitting different row
orders within equal keys. Review the spec against the actual SPARQL: hidden
expressions, mixed types, NaN, date timezone comparison and unstable LIMIT tie
selection need a separate comparator/cohort. Do not remove ORDER BY just to run.
An example for `ORDER BY DESC(?price) ?product`:

```json
{"q010": [{"var": "price", "kind": "numeric", "descending": true},
          {"var": "product", "kind": "lexical", "term_kind": "uri"}]}
```

## Execution

Use the fixed task18 `bench/ladybug` mappings/compiled queries, validating their
source hashes against `.rq` inputs. This harness only supports SELECT cohorts.
Acquire the shared benchmark lock **before** opening a database or running database tests,
fresh-read the current owner's directive, and pass the exact owner string. The
script checks ownership before startup and every sample; it does not acquire or
release someone else's lock. It refuses to overwrite an existing output file.

The pure standard-library term/order regressions do not open a database, build
code or collect performance samples: `python -m unittest discover -s bench/embedded`.
`smoke.py --lock-owner 'your exact owner string'` opens real databases and checks
objects/JSON/Arrow parity on lexical identity, language tags, duplicates and
unbound values. Run it in the benchmark window before large datasets.
`export_jena.py` captures independent TDB2 results through Jena's CLI, retaining
compressed JSON, complete signatures and per-query commands; it is untimed and
must finish before any timed process starts.

```sh
python bench/embedded/run.py --system ladybug --database /path/native.db \
  --queries /path/rq --compiled /path/compiled.json --reference /path/reference.json \
  --binding-source ladybug-0.22.3-wheel-SHA256 --lock-owner 'your exact owner string' \
  --out /path/abba-a1.json
python bench/embedded/run.py --system rdfdb --database /path/dedicated-store \
  --queries /path/rq --compiled /path/compiled.json --reference /path/reference.json \
  --binding-source exact-source-and-wheel-SHA256 --lock-owner 'your exact owner string' \
  --out /path/abba-b1.json
```

`test_contract.py` covers identity loss, bag multiplicity, variable alignment,
numeric ordering, ties, descending multi-key ordering and unbound keys. It is not
a replacement for real database parity and complete cohort measurements.
