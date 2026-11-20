# Virtuoso100M: copied-store correctness screen

The registered 60-request attempt completed, but it does **not qualify for a complete-workload comparison**. The unchanged frozen gates pass 42/60 primary results (including registered numeric bounds), 42/60 COUNT-compatible results, and 59/60 value-compatible results. No comparative timing or ranking total is reported.

| Gate | Passed | Required |
|---|---:|---:|
| Primary gate (registered numeric bounds) | 42 | 60 |
| COUNT-compatible gate | 42 | 60 |
| Value-compatible gate | 59 | 60 |

The only value-compatible failure is `m000_p08_q5.rq`: this candidate returns **0 rows** against the frozen Jena reference's **25 rows**. It occurs in warmup mix 0. The registered eligibility policy requires all 60 requests, including warmups, so dropping that request or reporting only the 45 later requests would change the policy after seeing the answer. Every overall ranking total remains null. No request was rerun or rewritten.

The other eleven q5 instances have empty reference answers and empty candidate answers. Their passing results do not demonstrate correct handling of the nonempty q5 case. The mismatch is consistent with the previously investigated projected-unbound-variable pattern, but this screen does not independently isolate the cause or establish a general Virtuoso defect.

## Candidate and scope

- Registration SHA-256: `84a6d66ff6704bc9a55dd0200fa7c07d46ad99b3505b7ff67fab7ca9c604297c`.
- Reviewed runner SHA-256: `c012fbd071a278f37b1dd41fef0da747577625a22fff76250023c4c6ce83ce46`.
- Runtime image: `openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a`. This pins the verification runtime; it does not reconstruct the original import image.
- A private copy of the existing 100M store was opened. The shared original was never mounted or changed by this job.
- The 25,965,260,343-byte input's SHA-256 is `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`, matching both recorded TripleLight build100 input hashes. Loader state is 2 with NULL error; the selected graph has 98,861,444 triples before and after the attempt.
- Original import-time input hash, error receipt and image digest are missing, and later shared-store checkpoints occurred. Retrospective input/count/loader checks do not prove full graph identity or reconstruct that lineage. This limitation remains even apart from the failed answer gate.
- Four CPU quota, 12 GiB cgroup limit and no additional swap. `ThreadsPerQuery=4` and `AsyncQueueMaxThreads=4` differ from the historical ten-thread configuration. This is an untimed correctness screen, not a configuration-matched performance comparison.
- Both workload and graph counts explicitly select `http://bench/bsbm100`; Virtuoso system graphs are excluded. The complete rendered ini digest was registered and checked, without publishing private configuration or database keys.

All 60 HTTP responses completed with status 200. The server exited cleanly with exit code 0, no OOM/high/max events were recorded, the exact owned container was removed, and its lock was released. Frozen auxiliary identities were unchanged. These checks do not supply a comparative memory result.

The audit JSON retains the implementation field name `strict` for the primary gate; this is not a claim of exact RDF-term identity for all primary passes.

## Evidence and replay

[`evidence.tar.gz`](evidence.tar.gz) contains the registration, all 60 response bodies and request records, frozen query/reference/numeric-policy/validator files, source and audit scripts, loader/count/resource/cleanup receipts, and a portable replay. It excludes the database, SSL material and raw ini.

After extraction, run `python3 virtuoso100-qualification-v1/replay.py`. The portable replay verifies all **353 registered auxiliary files** and all **60 response bodies**, replays the frozen three gates, confirms the 42/42/59 totals and the 0-versus-25 mismatch, and checks that all ranking totals remain null. It was run successfully against the exported packet; no database execution is needed.

The previous 100M TripleLight/QLever/DuckDB performance cohort remains a separate historical experiment. This screen adds no Virtuoso speed ratio to that table.
