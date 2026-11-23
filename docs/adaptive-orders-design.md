# Usage-driven index orders (design; owner: Alex)

**Status:** this is a design, with no code yet. Implementation waits for the first round of #105. The owner's request
is that users should not have to choose index orders, and that orders should be built and evicted according to usage
(`#sota-rdf-db` msg 13d5c812 and a454ac44; Kaiming's scope is in #105).

## 1. What exists today

- **Disk base:** a base is an immutable directory, `base-<n>/`. It holds a MANIFEST, one block file per kept order and
  the dictionary. `CURRENT` names the active base, and switching bases is write-tmp, fsync, rename, fsync dir
  (`store.rs`).
- **Order set:** RDFSNAP6 records the kept orders as a bit set over `Perm::ALL` = [SPO, SOP, PSO, POS, OSP, OPS].
  `REQUIRED_ORDERS` = {SPO, PSO, POS, OSP} today. `build-disk --orders=6` keeps all six.
- **Missing orders:** a request on a missing order is answered exactly, through one of two routes
  (`derived_order.rs`):
  - its *twin*, when two key columns are bound: the same rows with the first two columns swapped, a pure remap at
    kept-order cost;
  - its *sibling*, when zero or one column is bound: the kept order with the same first column, regrouped one group at
    a time. Groups above the spill threshold are spilled to `<store>/scratch/`.

  A one-column seek uses a per-snapshot memo of one group's distinct values.
- **Pinning:** a reader holds an `Arc<Overlay>` → `Arc<dyn Snapshot>` → `DiskIndex` with an open `File`. A base that
  `CURRENT` no longer names is deleted only at the next `Store::open`, so a running query never loses its files.
- **Counters:** `REMAP_CALLS`, `REGROUP_SCANS`, `REGROUP_ROWS`, `MAX_GROUP`, `MEMO_HITS`, `MEMO_MISSES`, `MEMO_ROWS`,
  `SORT_NS`, `REGROUP_SPILLS`, `SPILL_ROWS` and `PEAK_SPILL_BYTES` exist, but only when `TRIPLELIGHT_TRACE_DERIVED`
  is set, and they are process-global, not per order.

The mechanism a policy needs, answering any order from a covering subset, is already in place. This design adds the
policy, a per-order cost ledger, and a way to add or drop one order without rewriting the base.

## 2. Which order sets are safe (the floor)

A set *K* of kept orders can serve all six exactly if every missing order has its sibling (same first column) in *K*.
The sibling route handles every prefix length. The twin route is only a faster path for two bound columns. The
smallest such sets have one order per first column, three orders in all, for example {SPO, PSO, OSP}.

Some covering sets are still bad. Serving PSO from POS regroups whole predicate groups, and `rdf:type` alone is more
than 10% of LUBM. Serving OPS from OSP regroups object groups, which is cheap except for class objects (#187 sized
these). So the floor is a cost question as well as a coverage question:

- **The floor is user-visible.** Default {SPO, PSO, POS, OSP}, as today. The policy may evict down to the floor and
  never below it.
- **A smaller floor (3 orders) is an option, never the default.** The same ledger (§3) could justify it on a workload
  where the predicate is always bound. That is the owner's second question: nothing forces SPO(C) storage. It must be
  measured first.

Routing then generalizes from the hard-coded SOP and OPS to any missing order. Sibling = the kept order with the same
first column. Twin = the order with the first two columns swapped, if kept. Prefer the twin when two columns are
bound. Otherwise prefer the sibling whose regroup is cheapest, by the group-size statistics the planner already has.

## 3. Ledger: what an order costs and what it saves

The ledger is per order and per store, kept in memory and persisted beside `CURRENT` as `orders.ledger`. The ledger is
advisory, so a lost or corrupt ledger only resets the policy, never correctness.

| Field | Updated by |
| --- | --- |
| `direct_rows` | rows read from this order while kept (scans, counts by range, seeks) |
| `derived_rows` | rows read and regrouped to serve this order while missing (`REGROUP_ROWS` + `MEMO_ROWS` + `SPILL_ROWS`, attributed to the requested order) |
| `derived_ns` | time spent in those regroups and spills, including sort time |
| `last_used` | logical clock (requests served) of the last direct or derived use |
| `bytes` | on-disk size of the order file if kept, or the estimate `table_rows × bytes_per_row` of the order it would come from |

The counters become per order and always on: a relaxed `fetch_add` per batch, not per row, so the cost is in the
noise. A batch is 4096 rows. The process-global trace keeps its current meaning for the per-query log lines.

Costs are kept as an exponentially decayed sum with a half-life of *H* requests (default 10,000), so that old workload
phases fade out.

## 4. Build policy: ski rental

Building order *X* costs about one pass over the table: read the sibling and sort it externally
(`external_build::from_snapshot_keeping` already does this per order). Measured on the laptop, a LUBM-1000 order is
about 20–40 s by my estimate, to be measured before thresholds are fixed. Serving *X* derived costs `derived_ns` per
use.

The rule: build *X* once its decayed `derived_ns` reaches *α* × `build_ns_estimate(X)`, with *α* = 1. This is the
classic rent-or-buy rule. With *α* = 1 the total cost is at most twice that of an offline policy that knew the future,
and the same 2-competitive argument backs database cracking and adaptive indexing (Idreos et al.). `build_ns_estimate`
starts from the measured build throughput of the base (S3 receipts give rows per second per order) and is updated
after every build.

Other conditions:
- **One build at a time,** at background priority. It gets one rayon worker by default, so query throughput is not
  taken over, and its threads come from a dedicated pool, not the query pool.
- **A build is cancellable.** A commit that triggers compaction cancels a running build, and the build restarts on the
  new base if still justified, because compaction rewrites every kept order.
- **Disk budget:** an `index_budget_bytes` setting (default: 2× the floor's bytes). A build that would exceed the
  budget evicts first (§5). If even that cannot make room, the build does not start, and the ledger records
  "skipped: budget".

## 5. Evict policy

An order above the floor is a candidate when its decayed `direct_rows` saving is below what keeping it costs. The
saving is the derived cost it avoids, estimated as `direct_rows × derived_ns_per_row` of its route. The cost of keeping
it is its share of the disk budget.

In practice:
1. **Only under pressure.** Evict when a build needs room or the budget shrinks. Never evict an order that was used
   in the last *H* requests.
2. **Least saving first.** Among the candidates, evict the one with the smallest decayed saving per byte.
3. **Never below the floor.**

Eviction is cheap, but rebuilding is not. So hysteresis applies: an order evicted in the last 2*H* requests must reach
2*α* before it is rebuilt.

## 6. Adding and dropping one order: on-disk protocol and crash safety

Both operations make a new base generation that reuses unchanged files by hard link, then switch `CURRENT` the same
way compaction does. The base stays immutable, and recovery stays "the base `CURRENT` names, plus the WAL".

**Add *X* to `base-n`, producing `base-(n+1)`:**
1. Build `X` into `base-n+1.tmp/` from the snapshot pinned at start (which is base `n` and its delta). Rows come from
   the base only, not the delta: the WAL still covers the delta, as it does today.
2. Hard-link every file of `base-n` into `base-n+1.tmp/` (dictionary, kept orders). Write a new MANIFEST with the
   order bit set and the new file's identity. Fsync the files, then the directory.
3. Open and validate `base-n+1.tmp` the way `compact` does, then rename it to `base-(n+1)`, fsync the parent, and
   switch `CURRENT` (write tmp, fsync, rename, fsync dir).
4. Publish a new `Overlay` with the new base and the *same* delta. Readers that started before the switch keep the old
   `Arc`.

The WAL is not touched: `base-(n+1)` has the same logical content as `base-n`, so replaying the WAL on either gives
the same state. No change to the WAL's base-sequence bookkeeping is needed, but that must be checked against
`compact`'s WAL handling.

**Drop *X*:** the same steps, with no build and without linking `X`'s file. The MANIFEST clears the bit. Check that
the remaining set covers the floor before step 3.

**Crash cases:**

| Crash point | State at next open |
| --- | --- |
| During steps 1–2 | `base-n+1.tmp` is not named by `CURRENT` and is removed at open, the same as an abandoned compaction |
| After the rename, before `CURRENT` switches | `base-(n+1)` is unnamed and removed at open, and `base-n` is served |
| After `CURRENT` switches | `base-(n+1)` is served and `base-n` is removed. Hard links keep the shared files alive |

`Store::open` already removes every directory whose name starts with `base-` and that `CURRENT` doesn't name
(`store.rs`, at open), so `base-n+1.tmp` is covered.

**Concurrent commit and compaction:** an add or drop holds the same compaction mutex for step 3 only. Step 1, the
build, runs without it. If a compaction committed in the meantime, the add is rebased: the build restarts against the
new base, or is dropped if no longer justified.

**What a pinned reader sees:**
- When an order appears, readers that started earlier keep the derived route on their old snapshot, which is still
  correct. New readers use the new order.
- When an order disappears, an earlier reader keeps its open `File`. The inode lives while it is open, and
  `base-n`'s directory is not removed until the next open. New readers route through the twin or sibling.

No reader ever sees a partially built order: an order is only visible through a MANIFEST that names a complete,
validated file.

**Cache budget:** the block cache splits its budget into shares per kept order (`open_with_share`). After a switch,
the new base's indexes take new shares, and the old base's cache is freed when its last reader drops it.

## 7. Frozen mode for benchmarks

The paper must not let an index policy move during a measurement. #72's index-matched question depends on this.

- **`--index-policy=adaptive|frozen`.** The default is `adaptive` for users. Every benchmark arm runs `frozen`.
- **Receipts.** The kept order set is recorded in every receipt, as the MANIFEST bits, which `driver.py` already
  reads. A frozen run with the adaptive set from a warm-up phase is a separate, labelled arm.
- **The adaptive policy's evaluation is its own experiment:**
  - a workload shift (for example, LUBM queries, then WDBench-style variable-predicate paths);
  - time to adapt, requests until the build triggers, space used, and throughput before, during and after;
  - against the fixed four and fixed six orders.

## 8. Open questions (measure before fixing)

1. **Build time and query interference:** build time per order on LUBM-1000 and BSBM-100M, and query interference
   with one background worker, to set `build_ns_estimate` and the pool size.
2. **Cost of the derived routes:** the regroup cost per row and per group size, for each route (SOP via SPO, OPS via
   OSP, and the hypothetical PSO via POS), to decide whether a 3-order floor is ever worth offering.
3. **Half-life *H*:** whether one global *H* is enough, or whether it should be per suite.
4. **Hard links:** whether every target filesystem supports them (ext4 and APFS do). If not, fall back to a copy.

## 9. Work plan (after the first #105 round)

Each step is its own PR, with the #105 rules: identical answers, full CI, and an ABBA where timing is involved.
1. Per-order ledger and always-on counters. No policy yet. Measure the overhead (expected to be in the noise).
2. Generalized routing (any missing order through its sibling or twin), with tests over all 2⁶ order sets that cover
   the floor.
3. Add and drop one order: the protocol in §6, with fail-point tests at each crash point and a pinned-reader test.
4. The policy (§4–§5) behind `--index-policy=adaptive`, with frozen mode and receipt fields.
5. The adaptive experiment (§7), on the laptop first.

Related work to cite and compare:
- adaptive indexing and database cracking (Idreos, Kersten, Manegold);
- RDF-3X and Hexastore, which keep all six permutations;
- QLever, which keeps two by default;
- emergent schemas and characteristic sets (Pham and Boncz). Kaiming is measuring this. If it pans out, the ledger
  extends to characteristic-set tables.
