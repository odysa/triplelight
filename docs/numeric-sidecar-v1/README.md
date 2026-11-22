# Numbers in the ID, and a value-sorted side index for BSBM q010 (task #85)

Two measurements from the 2026-11 survey of recent database papers (#74). Registered before running (task #85 on the
board); every prediction is scored below, including the ones that missed. Single session, one host, descriptive.

## Setup

- **Code:** main at `e44cd704` (raw/versions.txt). The prototype is `examples/numeric_sidecar_proto.rs`, outside the
  engine. `count_numeric.py` is the literal counter.
- **Data:** `bsbm100.nt` (98,861,444 lines, 51,738,533 literals) and `lubm1000.nt` (138,318,414 lines, 44,351,183
  literals).
- **Store (part B):** the BSBM-100M store that task #83 built from `bsbm100.nt` on main with the default four orders:
  manifest `RDFSNAP6`, 6.1 GB on disk (dictionary 4.62 GB, `spo` 1.10 GB, `pso` 286 MB, `osp` 299 MB, `pos` 253 MB),
  opened read-only. The earlier "7.94 GB" store is the six-order build.
- **Timing:** warm, serial, best of five per cell, two full runs (raw/proto.json, raw/proto2.json).

## Part A: numeric literals and canonical form

"Canonical" means the form the engine would print from the stored value is byte-identical to the original, since RDF
term identity is lexical. Two rules were counted: a plain rule (integers without sign, leading zeros or `-0`;
decimals without trailing fraction zeros) and the XSD rule (a decimal keeps a digit on each side of the point; doubles
and floats in `E` form).

| Data set | Numeric literals | Share of all literals | Datatypes | Canonical | Fit in 60 bits | Distinct values |
|---|---:|---:|---|---:|---:|---:|
| BSBM-100M | 14,880,794 | 28.8% | `xsd:integer` only | 14,880,794 (100%) | 14,880,794 | 2,000 |
| LUBM-1000 | 0 | 0% | none | n/a | n/a | n/a |

No form fails to round-trip under either rule (the failing-form counts and examples are in `raw/`; they are empty). BSBM
has no decimal, double or float literals in this file, so the decimal and float rules were not exercised on real data; the
counter's regexes were checked on a small hand-made file. Literals with a non-numeric datatype, plain strings that look
like numbers and language-tagged strings are not counted.

**Predictions:** A1 (at least 95% of BSBM's numeric literals canonical) held: 100%. A2 (LUBM has almost no numeric
literals) held, and the count is exactly zero.

What it means: for BSBM, every numeric literal could be carried inside the ID with no loss of lexical identity, and
there are only 2,000 distinct values, so the dictionary already holds them once. LUBM would gain nothing. That says
nothing yet about how much speed an inline encoding would buy.

## Part B: value-sorted side index

q010 binds one product (value 960 for `productPropertyNumeric1`, 570 for `productPropertyNumeric2`), takes the 29,437
other products that share a feature with it, and keeps those within ±119 and ±169 of its two values (929 products), then
labels, sorts and limits to 5 rows. Times in milliseconds, two runs:

| What | Run 1 | Run 2 |
|---|---:|---:|
| Engine, q010 end to end (5 rows) | 23.9 | 25.5 |
| Engine, q010 without its numeric patterns and filters (5 rows) | 74.5 | 71.8 |
| Hand-written plan of q010's shape, as today: for each candidate, probe both numeric values and decode them | 60.4 | 58.7 |
| Same plan with the side index: range lookups, then the shared-feature check | 3.8 | 4.3 |
| Range lookup alone (two binary searches), ns | 37.9 | 38.8 |
| Reference: decode and filter the whole `productPropertyNumeric1` predicate (284,826 rows) | 47.1 | 46.4 |

The three plans return the same 929 products. The side index is two arrays of (value, subject ID), 284,826 rows each,
16 bytes per row in this prototype, 9.1 MB in all, built in about 54 ms per predicate.

**Predictions:**
- **B1** (the range lookup is at least 100× faster than decode-and-filter): true against the whole-predicate reference
  (38 ns against 47 ms), but that is not q010's baseline. Against the plan shaped like today's, the side-index plan is 14
  to 16× faster (3.8 ms against 60 ms). The engine itself is faster than my hand-written baseline, at 24 ms end to end.
- **B2** (about 12 bytes per row, a few hundred MB in all): missed. The prototype uses 16 bytes per row (64-bit value and
  64-bit subject ID), and BSBM-100M has only 284,826 products, so the side index is 9.1 MB, about 0.15% of the 6.1 GB store.

**The registered way to isolate the engine's numeric cost did not work.** Removing the numeric patterns and filters made
q010 slower (74 ms against 24 ms), because the filters prune the join early. So the engine's numeric part cannot be
read as a difference of the two runs, and nothing here shows how much of the engine's 24 ms the side index would save.
The 3.8 ms plan produces only the product set, without the label join, the sort or the limit, so it is not comparable
with the 24 ms end to end.

## Limits

- The side index lives outside the engine; no end-to-end q010 speed-up is claimed, and the cost of keeping it current
  under updates was not measured.
- One query (q010), one data set, two runs on a shared host.
- Only `productPropertyNumeric1/2` were indexed; q013 and the other numeric-range templates were not run.
- Part A cannot say how many numeric-looking values are stored as plain strings.
