# Frozen BSBM BI-100M answer policy

This archive contains the typed Jena references, Q7 cutoff diagnostics and numeric input evidence for the 60 BI-100M cases used by profile v4. The adapter in `bench/eval/bi_case_policy.py` applies the frozen answer and order checks, including exact-anchor intervals for Q4–Q6 and the inner LIMIT cutoff check for Q7. It verifies the dataset, query bytes and folded reference records before a run can score.

`PATH-SCRUB.json` records both SHA-256 digests for each of the nine changed files. Five files had machine paths replaced in provenance strings. Four files record the resulting digest changes. The other 256 files are byte-identical to the original registry, including all 60 result documents, eight Q7 diagnostics, 28 numeric group files and every validator module. The count-verification receipt changes only its plan digest. Machine paths in historical receipts are descriptive placeholders. Generic guarded lock paths in validator source are part of the archived code.

## Prepare the bundle

From the repository root, extract the registry into a fresh directory and add the adapter at that directory's root. Use the directory as the policy bundle in the plan and name `bi_case_policy.py` as its policy module. The runner binds the digest of the resulting directory when it writes the scored run manifest.

```sh
bundle=$(mktemp -d)
tar -xzf docs/eval-plan/bi100m-policy-v1/registry.tar.gz -C "$bundle"
cp bench/eval/bi_case_policy.py "$bundle/"
```

For BI-10M, extract the [separate BI-10M packet](../../index-layout-watdiv-bi-laptop-v1/bi10m/packet.tar.gz), copy both `bi_case_policy.py` and `bi10m_case_policy.py` into its root, and select `bi10m_case_policy.py`. Each entry point declares its own dataset digest.

The published replay binds the profile v4 query and reference files and checks the 360 saved answers from both historical sessions for each engine. It does not start a server or time queries.

```sh
python3 -B docs/eval-plan/bi100m-policy-v1/replay.py
```

`REPLAY.json` records the pinned commits and replay result. All 360 saved answers pass their registered check. Each session has 40 exact and 20 value-compatible DuckDB answers, 23 exact and 37 value-compatible QLever answers, and 58 exact and two value-compatible TripleLight answers.
