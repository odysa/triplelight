# deslop-please baseline on main (task #106, umbrella #107)

`deslop-please` 0.1.3 (`cargo install --locked`, with the `rust-analyzer` component) on a clean checkout of main `141b0735`.
It reported **33 errors** and exited 1; indexing was verified (no "scip failed" in the output, summary line present, 315 lines of
output; see `results.txt`). Files: `lint.txt` is the raw output, `baseline.allow` (now `.deslop-please.allow` at the repository root) the allow file written by `--update-allow`
(the CI gate uses it as the baseline, and it may only shrink).

## Counts by rule
| Rule | Errors |
| --- | ---: |
| field-leak | 27 |
| module-cycle | 3 |
| tangle | 2 |
| wide-directory | 1 |
| shallow-module, long-file | 0 |

## By crate and owner (the lanes agreed in #107)
| Place | Findings | Owner |
| --- | --- | --- |
| `triplelight-exec` | 12 field-leak, all fields of `Ctx` in `lib.rs` (`base`, `char_pairs`, `count_pushdown`, `data`, `dataset`, `general_group`, `named`, `range_pushdown`, `range_reorderable`, `sorted_probe`, `stats`, `wcoj`), module-cycle `bgp` / `range`, tangle `agg` / `expr` (2 things across 3 modules) | Kaiming (plan, joins, paths), Jianlin (operators) |
| `triplelight-plan` | field-leak `PrefixModel::vars` (5 places) and `::degree` (4), both in `planner.rs` | Kaiming |
| `examples/` | field-leak `Stats::characteristic_sets` (2 places, `optimizer_*.rs`) | Kaiming |
| `triplelight-storage` | 10 field-leak: `Builder::{terms 5, orders 2, triples 2, dedup_rows 1, index_workers 1}`, `Cache::entries`, `PermIndex::keys`, `TermArena::direct`, `SeekMemo::0`, `Overlay::delta`; module-cycle `disk_snapshot` / `external_build` / `lazy_dictionary` / `term_block_codec` | Alex |
| `triplelight-core` | field-leak `Parser::batch` (2 places, `turtle.rs`); module-cycle `ntriples` / `rdf` / `turtle` | unassigned (parsers): proposed WindowsXP |
| `triplelight-rdf` | tangle: 31 things across 7 modules (`literal`, `parser`, ...) | unassigned (parsers): proposed WindowsXP |
| `src/embedded.rs` | field-leak `Overlay::base` (4 places) | WindowsXP (embedded API) |
| `tests/` (repository root) | wide-directory: 17 entries, more than 15 | WindowsXP |

No findings in the server (`src/bin`), the language bindings or `bench/`. Note on the counts: of the 27 field-leaks, 12 are fields of `Ctx`
and 15 belong to other structs; `Ctx` is the biggest single cluster.

## Gate notes
A successful zero-finding run prints `deslop-please: ok` (Linus), a run with errors prints `deslop-please: N errors, ...`; a run
that fails to index prints "scip failed" and still exits 0, so the gate must check the output text and not only the exit code.
