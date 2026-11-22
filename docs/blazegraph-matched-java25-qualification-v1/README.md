# Matched Java 25 embedded qualification

Blazegraph 2.1.5 and TripleLight at commit `5e753fd5` each loaded and reopened nine triples. Each engine returned the expected typed object bag, the integer count of nine, and an empty result with the expected projection. Both rejected the malformed-query control. All six stages completed in separate Linux ARM64 containers using Temurin 25.0.5.1+2, four JVM processors, a 1 GiB heap, a 4 GiB cgroup with no swap, and disabled networking.

The object bag comes from the archived Jena grouped reference. The checker verifies nine distinct groups with one occurrence each, then projects their objects. The count uses the alias from the frozen query. The check preserves lexical forms, datatypes and language tags.

| Post-query store | Apparent bytes | Allocated bytes |
| --- | ---: | ---: |
| Blazegraph DiskRW journal | 10,485,760 | 10,485,760 |
| TripleLight RDFSNAP6 store including WAL | 1,241 | 36,864 |

TripleLight kept SPO, PSO, POS and OSP. The receipt sums the regular store files, including the WAL populated by the Java load. The Blazegraph journal occupies its preallocated 10 MiB initial extent. These measurements describe the nine-triple fixture. Full-workload qualification and timing are separate. [Blazegraph 2.1.5 journal constants](https://blazegraph.com/database/apidocs/constant-values.html#com.bigdata.journal.Options.DEFAULT_INITIAL_EXTENT)

The first run stopped after the Blazegraph load when the host launcher received a file-size signal despite its two captured logs being below the limit. The second run fixed capture and completed all three Blazegraph JVMs. Its checker applied a singleton rule to the inference marker printed for each query, and its expected headers did not match the frozen projection and COUNT alias. Both attempts are retained. The third run corrected the checker, preserved all fixture and engine bytes, and passed both engines on registration `6949b2aac297a975ec9e32d42b8ac33926352aa5e71ca1ea44cddb03bbc50006`.

Extract the evidence archive and run the recorded answer checker with Python 3.12 or newer.

```sh
tar -xzf packet-v1.tar.gz
python3 -B harness-v3/check_fixture.py run-v3 --java-version 25.0.5.1+2-LTS --processors 4 | python3 -m json.tool --sort-keys --indent 2 > replay.json
cmp replay.json run-v3/AUDIT.json
```

The output must match `run-v3/AUDIT.json`. The packet contains stage and store receipts, console and answer traces, the frozen checkers and fixture, and build identities. It omits compiled binaries and store data.
