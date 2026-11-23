# WatDiv and BSBM BI DuckDB layout qualification

DuckDB 2.0.0 on Linux ARM64 built a one-table control and six sorted copies of that table for each dataset. Each copy reopened with the same schema and row multiset as the control. The check used `EXCEPT ALL` in both directions and inspected adjacent sort keys for inversions. The control and six-copy files kept their identities during the query checks.

The three query arms used the one-table control, the SPO copy for every pattern, and the copy with the longest constant prefix for each pattern. Each layout ran 42 WatDiv requests and 60 BSBM BI requests. The BI workload used the frozen value and ranking policy. Both runs measured answer correctness. The separate [LUBM-100 qualification](../duckdb-layout-lubm100-v1/README.md) used the same DuckDB version and layout checks.

## Results

| Workload | Triples per copy | One-table bytes | Six-copy bytes | Exact typed per arm | Primary per arm | Compatibility per arm |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| WatDiv | 10,914,314 | 268,709,888 | 964,702,208 | 42/42 | 42/42 | 42/42 COUNT-compatible |
| BSBM BI | 10,102,032 | 618,672,128 | 2,508,206,080 | 24/60 | 60/60 | 60/60 value and ranking |

Each BI arm had 36 answers outside the exact typed bag comparison. They occurred in templates 4, 5, 6, 7 and 8, with 4, 12, 15, 3 and 2 requests respectively in each arm. The registered BI gate uses the value and ranking policy. Both workloads passed their registered laptop gates. The timed comparison will requalify every DuckDB layout on 2.0.1.

## Replay

Run `python3 -IB replay.py watdiv/packet.tar.gz` and `python3 -IB replay.py bi10m/packet.tar.gz` from this directory. Each replay verifies the archive, runtime and store receipts, then compares saved answers with the frozen Jena references. Exact typed, primary and compatibility results are reported separately. The BSBM BI archive removes machine-specific paths from its manifest and three diagnostic receipts, and records the original and portable hashes. All query texts, SQL, reference answers, numeric inputs used for comparisons, and validator source are retained byte for byte.
