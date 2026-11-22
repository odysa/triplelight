# One dictionary copy at BSBM-100M (task #55)

Registered comparison of the disk build before and after PR #83 (`RDFSNAP4`, one front-coded dictionary file in ID
order instead of two). Single session, one host, no other jobs (the shared benchmark lock was held).

- **Data:** BSBM, 98.9M triples, N-Triples file, built with `build-disk --build-external`.
- **Builds:** `d9a47980` (main before #83) and `3ce08bd8` (#83 merged), release binaries with `--features server`; the
  per-phase `[phase]` lines come from a local logging patch applied to both. Order old, new, new, old.
- **Measured per build:** wall time, store size, peak `du` of the build directory, max RSS, SHA-256 of every output
  file, and the `[phase]` timestamps (`raw/`).
- **Check:** `verify` of the new store (`raw/verify-new.log`).

## Results

| | old (`d9a47980`) | new (`3ce08bd8`) |
|---|---|---|
| Store size (two builds each) | 12,533.1 MB (11.67 GiB) | 7,936.3 MB (7.39 GiB), −36.7% |
| Dictionary files | `dictionary.term` 4.62 GB + `dictionary.id` 4.60 GB | `dictionary.unified` 4.62 GB |
| Dictionary conversion phase | 190.9 s, 195.9 s | 131.0 s, 139.5 s |
| Wall time | 684.3 s, 775.5 s | 706.3 s, 704.3 s |
| Peak build-directory size | 45.4 GB, 45.4 GB | 43.9 GB, 45.3 GB |
| Max RSS | 1,886 MB, 3,347 MB | 2,664 MB, 2,636 MB |

- The six permutation files are byte-identical between old and new (same SHA-256); only the dictionary file(s) and
  the manifest differ.
- The conversion phase (`triples_raw_written_assignments_removed` to `dictionary_converted`) is about 58 s shorter
  on average (193.4 s to 135.3 s, −30%), and the two ranges do not overlap.
- Total wall time overlaps between old and new (the host cannot resolve a few percent end to end), so no end-to-end
  speed-up is claimed. Peak scratch and RSS are not different beyond run-to-run spread.
- `verify` of the new 100M store passes: `snapshot verified`, 8 min 41 s.

## Limits

- Two builds per side, one host, descriptive comparison only.
- Phase times come from a logging patch that is not in the repository; it only prints timestamps.
- The store sizes are from file sizes in the build directory, not `du` blocks.
