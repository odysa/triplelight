# BSBM 100M DuckDB reference import

The retry imported **98,861,444 raw statements and unique graph triples**, matching the independently collected Jena graph count in `../jena100-reference-count-v1/`. The streaming input SHA-256 matched `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`; source size/mtime and copied parser/helper hashes also passed. Count agreement is not a full graph-equivalence proof. No 100M BI output qualification, numeric-policy freeze or performance comparison is claimed here.

The input was read in place without copying it. DuckDB 2.0.0 and PyArrow 26.0.1 built the same plain lossless typed triples table as the 10M preparation, with `SELECT DISTINCT` graph semantics. There are no property tables, numeric indexes or altered lexical types. The new optional status file reports parsed input, not resumable committed checkpoints. An interrupted attempt must restart in a new directory.

## Attempts, both retained

| Attempt | Outcome | Memory setting | Spill allowance | Parsed statements | Final database bytes |
|---|---|---|---|---:|---:|
| v1 | Failed at configured spill limit | 8 GB | 8 GB | 60,096,512 | 12,288 (invalid partial) |
| v2 | Success, input/count checks passed | 8 GB | 24 GB | 98,861,444 | 5,975,060,480 |

The successful importer ran about 2,127 seconds, with process peak RSS 9,639,428,096 bytes and sampled spill file lengths peaking at 19,008,978,944 bytes. These are preparation observations, **not comparative load timings or a memory hard-limit claim**. Python and Arrow are included in process RSS and are outside DuckDB's memory setting. Both attempts use four DuckDB threads. No failed attempt was discarded or relabelled.

An independent supervisor sampled total allocated scratch and free disk every two seconds. It would stop only its owned importer process group at 40 GiB scratch or 48 GiB free space, reserving headroom against the approved 48 GiB scratch cap and 40 GiB free-space floor. Sampling is not an instantaneous filesystem quota. No guard fired. `summary.json` contains sampled extrema and all retained reports. The runner waited for the supervised importer before checking source/count identities and releasing its exact owner lock. All owned processes exited.

## Validation and evidence

Two guard unit tests passed (threshold/lock cases and owned-process termination). A supervised three-statement fixture produced two unique triples, checking duplicate statements and plain/xsd:string equivalence across one-row Arrow batches. Exact source copies, commands, progress snapshots, supervisor samples, errors, reports and completion receipts are archived under each attempt. The final database, original 26 GB NT source, and spill data are deliberately not in Git.

`file-sha256.json` binds every retained evidence file except itself. The repository helper removes redundant progress-phase writes after the archived run; the archived helper is the exact code used for both import attempts. No parser, schema, input, or graph-set logic changed in that cleanup.

The original numeric and COUNT policies and their protected sources were not changed. Next steps are independent per-group input/count anchors and a separately versioned 100M numeric freeze before BI qualification and timing.
