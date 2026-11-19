# Virtuoso BI q5: projected unbound join variable reduction

The pinned Virtuoso image still returns no rows for the retained original BSBM BI q5, where Jena returns 14. Five small VALUES-only controls narrow the discrepancy to joining a projected, unbound variable: the standalone grouped subquery works, but joining it to bindings of that variable drops all matches. The same discrepancy occurs without aggregation. This establishes an observable reproduction, not the internal implementation cause or a claim about other Virtuoso versions.

| Control | Jena rows | Virtuoso rows | Exact RDF-term bag |
|---|---:|---:|---|
| Standalone grouped subquery, country unbound | 2 | 2 | Pass |
| Grouped subquery joined to bound country | 3 | 0 | Fail |
| Reversed syntactic join order | 3 | 0 | Fail |
| Ungrouped projection of unbound country, joined | 3 | 0 | Fail |
| Remove always-unbound country from inner projection/grouping | 3 | 3 | Pass |
| Original real q5 | 14 | 0 | Fail |
| Diagnostic rewrite of real q5 | 14 | 14 | Fail; country/product multiset passes |

The simplest failing control is controls/join-projection-unbound.rq:

```sparql
PREFIX ex:<urn:ex:>
SELECT ?country ?product WHERE {
  { SELECT ?country ?product WHERE { VALUES ?product { ex:p1 ex:p2 } } }
  VALUES (?country ?product) {
    (ex:US ex:p1) (ex:DE ex:p1) (ex:US ex:p2)
  }
}
```

The three expected mappings are (US,p1), (DE,p1), (US,p2). In SPARQL, compatibility checks equality only on variables bound in both solution mappings; an absent country binding therefore does not reject a bound country on the other side. This follows from the [compatible-mapping and Join definitions](https://www.w3.org/TR/sparql11-query/#defn_algJoin). Jena 6.2 returns precisely these mappings. The standalone grouped subquery also verifies that the unbound country has not become an explicit RDF term.

The original q5 price subquery projects and groups by ?country although its input patterns never bind it. The diagnostic rewrite removes only that variable from that subquery's projection/grouping. On the existing 10M store this restores the 14 country/product bindings, including multiplicity. This is not full result qualification: the remaining numerical/count term differences were not classified or accepted under a new gate here. The rewrite is exploratory and is not substituted for the official query in any comparative timing or historical correctness score.

## Scope and provenance

This is a short serial correctness run under the shared-machine lock, not a performance benchmark. The Virtuoso container uses four CPU quota units, 12 GiB memory and 12 GiB total memory-plus-swap (no extra swap). Image: openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a. It reopens the existing agent-owned matched-vm-10m/virtuoso-v3 store and makes read-only HTTP SPARQL requests against graph http://bench/bsbm10. Before and after counts both equal 10,102,032. Reopen/checkpoint may modify administrative database files; no byte-immutable store claim is made. Query hashes were fixed before execution and rechecked afterward.

Each small control is independently run through native Jena 6.2's arq CLI over the included irrelevant one-triple fixture, then through Virtuoso. The real q5 uses the saved historical Jena reference from docs/analytical-benchmark/value-diagnostic-v1/virtuoso-q5. Expected three-row control results also follow directly from the finite VALUES tables, so no large-data inference is needed. Raw responses, command receipts, original/rewrite queries and complete exact-bag outcomes are retained. The post-run checker independently replays the exact bags, checks completed supervisor state and verifies the owned container and supervisor PID are absent before releasing the lock. The current runner's process had already returned to its parent supervisor.

The whole owned scratch directory remains under the approved 48 GiB allocation, with the inherited two-second sampled abort guards; these are not hard quotas. No shared input/store was written. No database files, keys, credentials, environment dumps or comparative latency samples are included. Credential diagnostic lines in the captured server log were redacted before persistence. SHA256SUMS.json covers every published file except itself.

The unchanged original q5 remains a correctness failure for this pinned configuration; there is still no qualified Virtuoso BI total. The reduction can support a later version/configuration test or an upstream issue, but no external issue was filed and no product engine change is requested by this report.
