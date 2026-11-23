# Blazegraph and TripleLight on Java 25, LUBM-100 qualification

Blazegraph 2.1.5 and TripleLight each matched the saved Jena TDB2 answers on all 30 registered LUBM-100 labels, covering 26 distinct SPARQL query texts. Both engines reopened with 13,409,395 triples on the same Linux ARM64 Temurin 25.0.5.1+2 runtime. All nine stages exited successfully.

| Embedded engine | Reopened triples | Exact typed checks | Apparent store bytes |
| --- | ---: | ---: | ---: |
| Blazegraph DiskRW journal | 13,409,395 | 30/30 | 1,714,159,616 |
| TripleLight four-order persistent store | 13,409,395 | 30/30 | 312,057,889 |

Both runs used the same 2,377,133,547-byte N-Triples input. The Blazegraph explicit count equaled its total at load and reopen, and its journal digest matched across all three stages. TripleLight used the retained CLI and C ABI from commit `5e753fd5` with the SPO, PSO, POS and OSP orders. Its apparent size includes the generation-zero statistics sidecar. The sidecar matched the base manifest and the Java binding reported readiness after a fresh reopen.

The qualification used four CPUs and a 12 GiB container memory limit with swap disabled. The Java stages used an 8 GiB maximum heap. This run collected no query timings.

Replay the saved evidence with Python 3.12 or later. The packet keeps the query and reference bytes, both engines’ native answer streams, and the stage and store receipts. The provenance record describes the local-path redactions and retains the original receipt digests.

```sh
python3 -IB replay.py packet.tar.gz
```

[Evidence archive](packet.tar.gz) · [Archive SHA-256](PACKET.sha256) · [Replay output](AUDIT.json) · [Earlier nine-triple qualification](../blazegraph-matched-java25-qualification-v1/README.md)
