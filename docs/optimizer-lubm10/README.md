# LUBM-10 optimizer validation

The generated dataset has 1,316,700 input statements and 1,272,953 unique triples.
No inference is enabled. Data is read in place and is not committed. Checksums,
query hashes and compiler version are in `provenance.json`.

`reference_counts.tsv` comes from `bench/results_lubm_1794494976.json`; its Jena,
Oxigraph and rdfdb counts and answer hashes agree. `prefix_baseline.tsv` is the
original synopsis with exact prefix counts; `refined.tsv` adds capped rare-object
CS refinement. `bind_join.tsv` uses the same refinement and the new default index
nested-loop cost model. Estimates do not change with the cost-model switch.

Reproduce from the repository root:

```sh
cargo run --release --example optimizer_qerror -- DATA.nt bench/lubm docs/optimizer-lubm10/reference_counts.tsv --verify-executor
```

The verification executes both greedy and cost-ordered plans, compares complete
RDF-term result multisets, and checks their counts against the reference. All 14
queries pass. This is correctness validation, not a latency benchmark. Q1/Q10 and
Q6/Q14 repeat shapes in this explicit-type suite; these are not 14 independent
query shapes. Repeated-variable and empty-pattern semantics are tested separately.

Median q-error improves from 1.385772 to 1.000000. Rare-constant Q1/Q3/Q10/Q11/Q13 become exact. Q9 remains severely
underestimated: 1.678 estimated versus 422 actual, q-error 251.447. Uniform chain
selectivity and within-CS degree correlations remain limitations. No timing or
state-of-the-art accuracy claim follows from these results. WatDiv is pending.
