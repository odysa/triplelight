# Inline integer tag: statistics compatibility review

Task #14; source audit of main **02323e5**, 2026-11-15. The proposed 0xC encoding is not implemented in this reviewed revision. This is a design/source review, not a runtime validation of the future branch. No production code, tag layout, or storage format changed in this task.

## Statistics outcome

**No literal-tag classification needs extending in the current statistics code.** All scanned object IDs enter `Property::objects` and `Property::frequencies`. The NDV sketch hashes the complete 64-bit ID with wrapping SplitMix64 operations before selecting a register. Frequencies and exact pattern/object caches key on complete `TermId` values. Distinct canonical and noncanonical RDF terms therefore remain separate if the backend preserves their distinct IDs.

Source anchors at the reviewed revision:

- [Stats sketches and Property](https://github.com/odysa/rdfdb/blob/02323e5/src/opt/stats.rs#L25): the HLL input is the full ID, not its tag or payload alone; frequencies use `BTreeMap<TermId, u64>`.
- [Stats fields](https://github.com/odysa/rdfdb/blob/02323e5/src/opt/stats.rs#L108): no per-tag or literal-only object counters exist.
- [Query-star grouping](https://github.com/odysa/rdfdb/blob/02323e5/src/opt/stats.rs#L319): its `(u8, u64)` key separates variable slots from constant IDs; the u8 is not an RDF term-kind bucket.
- [Count execution](https://github.com/odysa/rdfdb/blob/02323e5/src/exec/count.rs): equality and successor seeks treat IDs as opaque identities and sorted keys. They do not classify literal tags.

This does not guarantee unchanged approximate estimates: relabeling IDs changes the sketch input and may change estimate/cost values. Correctness must not depend on an approximate estimate proving absence.

## Missing proof for the proposed range shortcut

The existing `Property` fields cannot prove that a predicate has only inline integer objects. Neither a zero from the bounded 64-counter frequency summary nor a value inferred from HLL licenses omitting all non-0xC objects.

Skipping the fallback requires an **exact, snapshot-scoped per-predicate non-inline count**, accumulated from every triple. Unknown coverage must retain the fallback. Updates, old snapshots, and compaction must maintain or invalidate that proof. [serve-store already gates statistics](https://github.com/odysa/rdfdb/blob/02323e5/src/bin/rdfdb.rs#L164) on both an empty delta and matching base identity; retain that guard for correctness-sensitive coverage facts.

## Identity, decoding, and precision

1. **One RDF term, one ID within a snapshot.** An old base dictionary ID for canonical integer `5` and a new 0xC ID cannot coexist as two representations of that same term in an overlay. Legacy lookup must prefer the actually stored ID; a full re-encoding must also rebuild ID-keyed caches. Lexically distinct `5`, `+5`, `05` and different datatypes remain distinct RDF terms.
2. **Aggregate results need decoding too.** COUNT can create a canonical integer absent from the stored graph. Both scalar `resolve` and batched `with_terms` must decode a computed 0xC ID without requiring a dictionary entry, or retain the scratch-ID fallback. Test the raw-ID/JSON result path as well as decoded queries.
3. **The existing comparator narrows integers to f64.** Both [general comparison](https://github.com/odysa/rdfdb/blob/02323e5/src/exec/expr.rs#L85) and [compiled comparison](https://github.com/odysa/rdfdb/blob/02323e5/src/exec/expr.rs#L320) call `Num::f`. The integers 9007199254740992 and 9007199254740993 map to the same f64. An integer-exact range covering ±2^55 therefore needs exact Int/Int comparison and adjacent-integer tests around 2^53. This is a pre-existing limitation, not a bug demonstrated in the unimplemented new tag.

## Handoff

Kaiming adopted the coverage, identity and count-result constraints in Raft message `be36234c`: keep non-inline fallback until exact coverage is available; consult stored dictionary IDs first; consistently re-encode and rebuild caches at compaction; support unstored count results. The comparison precision finding was sent separately in `09a80438`.

Implementation tests, old-store/WAL reopening, mixed datatype/lexical forms, range boundaries and the A/B remain the encoding owner's next steps. The audit has no remaining statistics-code edit to apply. No builds, benchmarks or model runs were needed for this source-only review.

## Subsequent resolution

After the findings were sent, Kaiming fixed the pre-existing Int/Int precision issue in main **bef6811**. Source inspection confirms that both general and compiled comparisons now share `num_cmp`, using i64 ordering for two Int operands; mixed types still use the existing f64 path. This closes the specific adjacent-integer finding, not a general claim of arbitrary decimal precision. No independent runtime tests were run in this audit.

In message `a0a54a16`, Kaiming also confirmed that the exact all-inline proof will retain the empty-delta/matching-base guard. The new tag and coverage-counter implementation still require the encoding branch's own compatibility tests and A/B.
