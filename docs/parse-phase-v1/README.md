# The parse phase of a disk build: time split, duplication, and a design note (task #91)

Part 3 of the survey experiments on build speed (after `docs/build-profile-v1`). Registered before running (task #91);
scored below. One instrumented build, one scan, one host; descriptive.

## Setup

- **Code:** main at `7a2e6d20` plus `timing.patch` (timers and counters only: per-shard time for parsing, formatting,
  pushing and the final merge, and process-wide time spent sorting runs, writing runs and merging).
- **Build:** `triplelight build-disk lubm1000.nt DIR --build-external`, four orders, under `/usr/bin/time -l`: 435.1 s
  wall, 686.5 s user, 260.6 s system, max RSS 2.31 GB, build-directory peak 41.4 GB (at 93 s, inside the parse phase).
  **This run is much faster than the Part 1 build of the same file (654.8 s, peak 46.7 GB at 244 s) and I have no
  explanation for the difference** (same command and inputs, same exclusive lock, different main by docs and a server
  change): the two builds have different absolute times, so only ratios are compared between them below.
- **Scan:** `term_dup.py`, one process, over the same file with the builder's 8-way byte split, counting term tokens
  (subject, predicate, object as written in the file).

## Measurement 1: where a shard's time goes

Eight shards, each about 17.3M triples and 51.8M term records (414,955,242 records and 27.35 GB of record bytes in all).
Per shard, averaged (seconds, and share of the shard's 111.3 s wall time; the slowest shard took 118.4 s):

| Part | Seconds | Share |
|---|---:|---:|
| Read and parse the N-Triples (`read_ntriples` iterator) | 40.1 | 36.1% |
| Format the three terms (`to_string()`) | 8.0 | 7.2% |
| Build and push the records (outside run flushes) | 2.3 | 2.1% |
| Sort a full run (`sort_unstable` on `Vec<Vec<u8>>`) | 10.3 | 9.3% |
| Write a run | 10.8 | 9.7% |
| Merge the shard's runs (the final merge round) | 36.8 | 33.0% |

416 runs in all across the 8 shards (52 per shard). The shard stage lasted until the slowest shard finished (118 s); the
fastest shards then waited, since the term merge needs every shard.

**Predictions:**
- **1a (parsing proper is under 25% of a shard's time):** missed. It is 36%, the largest single piece.
- **1b (sorting, writing and merging runs together are at least 50%):** held, barely: 10.3 + 10.8 + 36.8 = 57.9 s, 52.0%.
- **1c (the timers cost under 5% of wall time):** not resolved. The run is not comparable with Part 1's wall time, so the
  cost of the timers cannot be read off. Each triple takes about six clock reads at roughly 30 ns each, about 3 s per
  shard (2.8%) by arithmetic, not by measurement. Kaiming's note, to read the clock once per 4,096 triples, came after the
  run started and was not applied.

## Measurement 2: duplication

| | Value |
|---|---:|
| Term occurrences (3 per triple) | 414,955,242 |
| Distinct terms, global | 32,905,188 (7.93% of occurrences) |
| Sum of per-shard distinct terms | 32,920,468 (7.93%) |
| Record bytes if every occurrence is sorted | 27.35 GB |
| Record bytes if each shard sorted only its distinct terms | 2.38 GB (8.7%) |
| Occurrences that are IRIs / literals | 370.6M / 44.4M (no blank nodes) |

Occurrences by how often their term occurs: once 11.2M; 2 to 9 times 99.0M; 10 to 99 times 84.6M; 100 to 999 times
11.6M; 1,000 to 99,999 times 23.5M; 100,000 times or more 185.1M (44.6% of all occurrences, from a few hundred terms:
predicates, `rdf:type`, class IRIs).

Per-shard and global distinct counts are almost equal because LUBM's data is generated university by university, so a
term is local to the shard that holds its university. Other datasets will share more terms across shards, and the scan
should be rerun on BSBM and WatDiv before relying on these ratios there (about 6 minutes each, single core).

**Predictions:**
- **2a (global distinct at most 15% of occurrences):** held, 7.9%.
- **2b (sum of per-shard distinct at most 25%):** held, 7.9%.
- **2c (at least 80% of occurrences belong to terms that occur 10 times or more):** missed, 73.5%. Heavy terms are
  concentrated at the very top: 44.6% of occurrences come from terms seen at least 100,000 times.

## Design note (estimates, not measurements; no code)

**Today.** Each of the 415M term occurrences becomes its own sort record: term text, a zero byte, the triple ID and the
position, written to a run on disk, then merged twice (per shard, then across shards) and read back to assign a global
ID and emit a (triple, position) to ID mapping record, which a second sort (the mapping sort) puts back in triple order.
Because each record carries the triple ID, equal terms are never equal records, so the sorter cannot merge them.

**Option A: deduplicate in a per-shard hash map before sorting.** In each shard thread, look each term up in a
`HashMap<term, local id>`, and write the triple as three 8-byte local IDs to a per-shard rows file instead of pushing
three records. At the end of the shard, sort only the shard's distinct terms (4.1M, about 300 MB, which fits in memory) and
write them as one sorted run with their local IDs. The term merge then merges 8 sorted lists of distinct terms (32.9M
records, 2.4 GB) and assigns global IDs exactly as today, in global text order, so the dictionary and the IDs come out
unchanged. It writes, for each shard, an array local ID to global ID (4.1M × 8 B = 33 MB). The triples are then remapped by
a sequential pass over each shard's rows file with that array in memory, producing the `.triples.raw` file the order builds
read today. That pass replaces the mapping sort and the 17-byte mapping records.

- **Time (estimate):** per shard, parsing (40.1 s) and formatting (8.0 s) stay; the run sorts, run writes and merge (57.9 s)
  are replaced by 17.3M × 3 hash lookups (about 8 s at 150 ns each), writing 417 MB of rows (about 1 s), and an in-memory
  sort of 4.1M terms (about 5 s). That takes a shard from about 111 s to about 66 s, 40% less. The term merge sorts 12.6×
  fewer records (32.9M against 415M) and the mapping sort disappears into a remap pass over 133.6M rows. Applied to Part 1's
  breakdown (parse stage 38.7%, term merge 14.4%, mapping sort 11.8% of the wall time): about −15%, −12% and −10%, so
  roughly 35% of the build, within a plausible range of 25 to 40%.
- **Scratch (estimate):** the shard runs, 27.4 GB today and the reason for the 41 to 47 GB peak, become about 2.4 GB of
  term runs plus about 3.3 GB of local-ID rows. The build directory would then peak in the order phase, at about 20 GB (Part 1),
  roughly half of today's peak.
- **Memory (estimate):** the map holds the shard's 4.1M distinct terms, about 72 bytes of text and framing each. A
  `HashMap<String, u64>` costs the string header (24 B), the heap text, 8 B for the ID and about 30% table overhead, so
  about 120 bytes per entry, 0.5 GB per shard and about 4 GB across 8 shards, on top of today's 2.3 GB: peak RSS of about
  6 GB for LUBM-1000. That is within a 64 GB machine but a regression against today's flat 2.3 GB.
- **When a shard's map exceeds its budget:** flush it as a partial dictionary run (sorted terms with their local IDs),
  clear it, and continue with a new run number. Row IDs then carry (run, local ID), the term merge assigns a global ID per
  (run, local ID), and the remap arrays are per run. A dataset of mostly unique terms makes every run nearly as large as
  today's records but without the triple ID and position, so it should not be slower than today, only no longer faster.
- **Risks:** the first pass must also keep the order of triples inside a shard (the triple IDs, `shard << 56 | n`); the
  global ID assignment must stay byte-identical (gate: identical stores against main, four and six orders, as for
  #90); a term over 16 MiB must still be refused; and the sorted run of a partial map reuses the unique-ID rule for kinds.

**Option B: keep the record layout, hash the term, and use the fixed-width sorter (#90).** Replace the term text in
the record with its 64-bit hash and sort fixed-width (hash, triple, position) rows with the new sorter, and sort the distinct
terms separately. It also removes the variable-length sort of 415M records, but it still sorts and merges 415M rows
(fixed-width, so about 4× cheaper than today per row by the Part 2 benchmark) and adds a hash-collision rule. It is
smaller to build on top of #90 but saves less than A: it keeps the 415M-row sort and the mapping sort.

**Option C: dictionary-encode while parsing from a shared concurrent map.** One global map shared by all shards avoids the
cross-shard merge of duplicate terms, but needs a concurrent map and a global ID order that is not text order (the unified
dictionary requires ID order to equal text order within a kind), so the IDs would have to be reassigned after a final
sort anyway. Not recommended.

**Suggested first experiment for A:** build it only for the shard stage on LUBM-1000 behind a flag, measure the shard
stage's time and the build directory's peak, and compare stores for identity before changing the term merge and the mapping
sort.

## Limits

- One build and one scan on one host; the LUBM duplication ratios (7.9%, the per-shard equal to the global) are specific to
  LUBM's generation. BSBM and WatDiv were not scanned.
- The two runs of the same command differed by 1.5× in wall time (655 s and 435 s) for a reason I could not find, so
  absolute times here are not comparable with Part 1; the shares inside this run are.
- The estimates in the design note come from this profile and the Part 2 benchmark; no prototype of Option A exists.
