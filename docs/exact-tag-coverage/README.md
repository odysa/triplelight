# Exact predicate object-tag coverage

Task #15 supports task #11's numeric range pushdown. Functional regression passed on base bef6811; integrated all-target validation later passed 93 tests on d3e18cd plus coverage/diagnostics. The serialized statistics-build comparison is in [MEASUREMENTS.md](MEASUREMENTS.md).

## API and contract

`Stats::exact_non_inline_objects(predicate) -> Option<u64>` counts distinct graph triples with that predicate whose object ID is **not tag 0xC**. Repeated values in different triples count separately. Repeated identical input triples are ignored by the existing sorted-SPO builder. Other inline kinds, including 0xF strings, are non-0xC for this purpose.

- `Some(0)` certifies that all objects of a **known predicate in the statistics' input** have tag 0xC.
- `Some(n)` with n > 0 requires the non-0xC fallback.
- `None` is unknown coverage (including an unknown predicate); retain the fallback.

This method does not receive the current snapshot and cannot detect a caller supplying stale statistics. The consumer must first establish the valid statistics scope. For `serve-store`, require an empty delta and identity of the source base. The current `base_id` is a raw Arc address; a correctness-critical consumer must also prevent address reuse across base generations (for example, pin the allocation with a Weak reference or use a unique generation). No address-reuse failure was reproduced here. A planning clone or query-specific exact-count refinement does not rebind the tag coverage to a different input snapshot. `from_spo` coverage describes its complete supplied stream, not a larger graph it might have been sampled from.

```rust
let outside = valid_stats.and_then(|st| st.exact_non_inline_objects(predicate));
if outside == Some(0) {
    // Valid proof permits omitting the non-0xC branch.
} else {
    // Includes unknown coverage: retain the exact fallback scan.
}
```

## Implementation

Only the global per-predicate summary contains a 16-counter tag array. Characteristic-set properties retain their old layout and sketches. Each unique triple increments its object's top-four-bit bucket at the existing global predicate-map lookup; no additional per-triple map lookup or term decoding is introduced. The proof is derived from exact total minus exact 0xC count, never from HLL or bounded frequency entries. Statistics clones share the global map through Arc.

This changes neither dictionary/tag layout nor query routing. It supplies a future consumer with exact metadata. The consumer still owns datatype promotion, legacy-ID compatibility, and numeric-filter semantics.

## Validation

Three new unit tests cover every tag, weighted row counts and duplicate suppression, predicate isolation/unknown coverage, a rare non-0xC object evicted from the bounded frequency sketch, and bit-for-bit preservation of the existing NDV/frequency/equality sketch inputs. The full `nice -n 19 cargo test -j1 -- --test-threads=1` run passed 88 tests, with no failures; see `functional-tests.log`. This was the explicitly authorized functional window during dataset loading, not a performance comparison. Tag tests use synthetic IDs; future 0xC encoder integration belongs to the consumer.

`examples/stats_build_probe.rs` is the identical baseline/candidate harness. It rebuilds Stats three times on the same disk snapshot (first pass followed by two warmed passes), then records exact float bits and plan hashes on the saved pure BGP queries. A timed comparison must use a quiet, serialized machine window; dataset generation overlap is not valid evidence of negligible overhead.

The measured warm-build ratios were 1.047 on LUBM-100 and 0.977 on WatDiv SF100 in one shared-host ABBA session, with identical estimate bits and plans for all 34 queries. This does not establish zero overhead. A mainline merge and numeric consumer remain the owner's decision.
