# Rental runbook (owner: Alex)

This runbook covers day one on the dedicated machine and the run order under [Amendment 2](PLAN.md#amendment-2-2026-11-23-before-any-rental-measurement-scope-under-the-rental-budget).
The machine is `i4i.4xlarge`: 16 vCPU, 128 GB RAM and 3.75 TB of local NVMe. Every time below is a planning estimate
from task #100, not a measurement. Where an estimate is mine, it says so. Revise the schedule after the first tier-1
dataset, as Jianlin asked, and record each change in the session log with its reason.

## 1. Day one: provisioning (about 1 h)

The repository is private, so the default puts no GitHub credential on the rented host. On the laptop, bundle
the commit under test and copy the bundle over:

```sh
# laptop
git bundle create tl.bundle main && git bundle verify tl.bundle && scp tl.bundle root@<host>:
# host, as root, on a fresh Debian trixie
git clone ~/tl.bundle ~/tl && cd ~/tl && git checkout --detach <sha>
lsblk                                   # pick the instance-store NVMe, not the root volume
bench/eval/provision.sh --device /dev/nvme1n1 --data /data --repo ~/tl.bundle --commit <sha>
. /data/eval.env
```

For a later commit, copy a new bundle and rerun with the new `--commit`. Cloning from GitHub (the script's default
without `--repo`) is only an alternative, and it needs the owner's own access on the host, which only the owner sets
up. Agents never put a credential on the machine.

The script is idempotent: rerun it after a failure. It writes every step, version, checksum and setting to
`/data/receipts/provision.txt`, and stops on any mismatch. It does the following:

- **Disk:** formats the NVMe as ext4 (`nodiscard`, no reserved blocks), mounts it at `/data` with `noatime`, and lays
  out `tools/`, `datasets/`, `stores/`, `scratch/{duckdb,triplelight}/`, `results/` and `receipts/`.
- **CPU:** sets the governor to `performance`, turns turbo off (`intel_pstate/no_turbo=1` or `cpufreq/boost=0`), sets
  `vm.swappiness=1` and disables swap. It records the values it finds; on instance types that hide `cpufreq`, the
  receipt says `n/a`, and that is expected.
- **Packages:** Docker (`docker.io`, plus `docker-cli` where it is a separate package), Maven, Boost headers (for
  WatDiv), `wamerican` (WatDiv's word list), `lbzip2`, `time`, `lsof`, `jq` and `iproute2`.
- **Docker:** it must report `systemd` as the cgroup driver on cgroup v2, or the script stops. That is what
  `s3-build.sh` and `s1-sample.sh` assume when they look for `/sys/fs/cgroup/system.slice/docker-<id>.scope`.
- **Toolchains:**
  - Rust 1.100.0, under `/opt/rustup` and `/opt/cargo`;
  - Temurin 25.0.5.1+2, under `/opt/jdk25`, checked by sha256;
  - a Python 3.13 virtual environment at `/opt/eval-venv`, from [`requirements-eval.txt`](../../bench/eval/requirements-eval.txt)
    with `--require-hashes --only-binary :all:`. It holds DuckDB 2.0.1, PyArrow 26.0.1, PyOxigraph 0.5.13 and maturin
    1.16.0, pinned by PyPI's wheel hashes for x86_64 and aarch64.
- **Systems:**
  - Jena and Fuseki 6.2.0 (sha512), the Oxigraph 0.5.13 binary (sha256) and the Blazegraph 2.1.5 jar (sha256; it
    matches Maven Central's sha1);
  - RDF Fusion: `rdf-fusion-cli` 0.3.1, built with `--locked`;
  - the signed DuckDB `rdf` extension, installed by DuckDB itself with signature checks on into
    `tools/duckdb-ext`, then compared with the pinned per-architecture hash. If upstream has published a new build,
    the script stops; re-pin it with Jianlin, and never turn signature checks off.
- **TripleLight at `<sha>`:**
  - the server binary (`--features server`);
  - the Python wheel, built with maturin into the virtual environment;
  - `libtriplelight_c.so` and the Java classes, from the same commit, kept together in `tools/triplelight-java/`.
  The sha256 of every artifact goes into the receipt.
- **Images:** QLever, Virtuoso and RDF4J are pulled by the digests in `VERSIONS`, and the MillenniumDB image is built
  at its pinned commit.
- **`/data/eval.env`:** `PATH`, `JAVA_HOME`, `TRIPLELIGHT_LIBRARY`, `BLAZEGRAPH_JAR`, `DUCKDB_EXTENSION_DIRECTORY`,
  `DUCKDB_TEMP_DIRECTORY=/data/scratch/duckdb`, and `TRIPLELIGHT_SCRATCH_DIR` and `TMPDIR` on the NVMe. DuckDB spill
  and every other temporary file stay on the NVMe, never on the root volume or a tmpfs.

### Day-one checks (the script runs them; read the receipt)

| Check | Expected receipt line |
| --- | --- |
| Native scope | `native scope ok: memory.max 268435456 swap.max 0 cpuset 0`, from a `systemd-run --scope -p MemoryMax -p MemorySwapMax=0 -p AllowedCPUs` probe, the same call `lib.sh start_system` makes |
| Docker cgroup | `docker cgroup ok: memory.max 268435456 swap.max 0 cpuset 0` at `/sys/fs/cgroup/system.slice/docker-<id>.scope`, with the `--memory`, `--memory-swap` and `--cpuset-cpus` flags that `systems.sh` passes |
| Cold cache | `drop_caches ok` (S2 cold runs and S3's cold-cache builds need it) |
| Listeners | the list of listening sockets before any server starts. After each server starts, `ss -Hltn` must show it on `127.0.0.1` only; `systems.sh` binds every server and every `docker -p` to 127.0.0.1 |
| cgroup v2 | `cgroup: cgroup2fs` |

Then confirm by hand that `docker run --network host` is not used for any timed arm unless the W3 control (section 4)
chose it.

## 2. Datasets (about 6–8 h of wall time on day one, before any timed run)

```sh
. /data/eval.env
nohup bench/eval/datasets.sh bsbm1b > /data/receipts/gen-bsbm1b.log 2>&1 &          # longest, single-threaded: start first
bench/eval/datasets.sh lubm1000 bsbm100m watdiv-sf1000                                # tier 1, about 1–2 h
bench/eval/datasets.sh lubm8000                                                       # 16 shards
bench/eval/datasets.sh wdbench=/data/datasets/<archive from Figshare>                 # or wdbench=<direct URL>
```

Generate every dataset before the first timed run, so that no generator competes for CPUs or disk with a
measurement. Each dataset becomes one N-Triples file in `/data/datasets/`, with a receipt in
`/data/receipts/dataset-<name>.txt`. The receipt holds the generator archive's sha256, the exact command, wall time,
bytes, lines and the file's sha256. A finished dataset is skipped on a rerun.

| Dataset | Generator (archive sha256 checked) | Command | Size (estimate) | Wall (my estimate) |
| --- | --- | --- | ---: | ---: |
| LUBM-1000 | UBA 1.7 (`3d44f468…`) | `-univ 1000 -seed 0`, then `riot --output=ntriples` | 22.8 GB, 138.3M lines (laptop) | 0.5 h |
| LUBM-8000 | UBA 1.7 | `-univ 8000 -seed 0`, 16 shards by `-index` | about 182 GB, about 1.1B lines | 1–2 h |
| BSBM-100M | BSBM tools 0.2 (`40f5e59b…`) | `generate -fc -pc 284826 -s nt` | 24.8 GB (laptop) | 0.5–1 h |
| BSBM-1B | BSBM tools 0.2 | `generate -fc -pc 2848260 -s nt` | about 250 GB | 4–6 h, single thread |
| WatDiv SF1000 (about 110M triples) | WatDiv v0.6 (`fb8d930b…`), seeds patched to 20261112 | `watdiv -d model/wsdbm-data-model.txt 1000` | about 15 GB, about 110M lines | 1 h |
| WDBench | the Figshare item `50b7544ad6b1f51de060` (2021-06-23 direct properties) | stream-decompressed; exactly 1,257,169,959 lines, or the script stops | about 165 GB, plus the archive | download plus 0.5 h |

Notes:

- **LUBM sharding:** UBA reseeds each university from (seed, index), so shards by `-index` write the same files as one
  run. Files are converted in university and department order, so the output is deterministic.
- **BSBM:** the test driver's `td_data-<name>/` is kept next to the data. BSBM's query constants come from it, and the
  BI constants are generated on each graph (plan, analytical section).
- **WatDiv:** scale factor 1000, about 110M triples. Label it by its scale factor everywhere, never "WatDiv-100M"
  (Kaiming). The laptop's `watdiv100.nt` is SF100 (10.9M triples), so its queries are re-instantiated with `watdiv -q` against the SF1000 saved state, which `datasets.sh` keeps as
  `watdiv-saved-sf1000.txt`. Debian's word list differs from the laptop's macOS one. Its hash is in the receipt.
  WatDiv-1B is not generated (Amendment 2).
- **WDBench:** Figshare's private link needs a browser session. The owner downloads the archive (or gets its direct
  URL) once, and the script verifies the graph by its line count, then records both hashes. Delete the archive once
  the graph's receipt exists. Queries are pinned at upstream `6767ee26`.
- **Verification:** the sha256 and line counts take about 10 min per 200 GB, and they are part of the wall times
  above.

## 3. Disk budget and delete order

Usable space after formatting is about 3.4 TiB. One dataset's full set of stores is on disk at a time (Kaiming, confirming the reading below). Keep at least **300 GB free** at all times; this floor is my choice and
covers logs, result files and an unexpected spill. Before every build, compare `df` with the remaining inputs plus the
next store and its scratch allowance plus the floor, as Jianlin asked, and write that line into the session log.

| Item | Size (estimate) | Kept until |
| --- | ---: | --- |
| Tier-1 inputs (LUBM-1000, BSBM-100M, WatDiv SF1000) | about 63 GB | the end of tier 3, or the end of the rental if tier 3 is not run |
| Tier-2 inputs (LUBM-8000, BSBM-1B, WDBench) | about 600 GB | the end of their last tier |
| A 100M store, any system | 10–60 GB | that dataset's last block and archive |
| A 1B store | 60–600 GB (QLever and TripleLight smallest; TDB2 and Oxigraph largest) | the same |
| Build scratch | up to 2.1× the input for TripleLight before dedup, about 1× with it (#88, `docs/dedup-build-v1/`), so up to 525 GB at 1B; the rivals' loaders vary | the end of that build |
| DuckDB control and six-copy files plus spill | 150 GB at 100M, 1 TB at 1B (Jianlin) | that dataset's DuckDB arms |

A5 runs the blocks in a balanced rotation over the arms, so every arm's store of the dataset being measured must exist
during that dataset's blocks. I read B6's "one system's store per dataset" as one copy of each system's store, with
no duplicates and no second dataset's stores alongside. At 100M, all seven stores of one dataset fit easily (about
0.4 TB). At 1B, all seven stores of BSBM-1B come to about 1.4 TB (my estimate, about 5.5× the input across the seven
systems). With 0.66 TB of inputs and one build's scratch, that is about 2.6 TB, which fits with the floor to spare.

**Delete order:**
1. A build's scratch, as soon as the build's receipt is written. TripleLight removes `<store>/scratch/` on open, and the
   other loaders' temporary directories are removed by `s3-build.sh`'s caller.
2. DuckDB query spill, after each DuckDB pass.
3. The DuckDB shared six-copy file, after both the fixed and routed arms on that dataset finish.
4. The DuckDB control file, after every planned comparator or mapping use on that dataset finishes.
5. A system's store, after its last block on that dataset, and only once its receipts, answer hashes and results are
   archived into `results/` and copied off the machine. Delete the longest-loading arm's store last, in case a block
   has to be rerun.
6. The WDBench archive, once the graph's receipt exists.
7. Dataset files, after their last tier. Keep the frozen Jena reference answers and every receipt to the end.

If the free-space check fails before a 1B build, delete in this order and recheck. Never delete a store whose
results are not archived.

## 4. Run order

The machine lock and the session log work as on the laptop: one measured job at a time, with nothing else running.

### Tier 1: 100M, the paper's main tables (about 6–7 days, B5)

| # | Block | Estimate | Owner |
| --- | --- | ---: | --- |
| 1 | W3 Docker-versus-native control: LUBM-1000 suite, QLever and Virtuoso, native and Docker with host networking and a local volume, one block. Its outcome decides whether every Docker system runs native, so it goes first. | 2–3 h | WindowsXP |
| 2 | Loads at 100M: seven systems × three datasets, each through `s3-build.sh`, so every load is also an S3 receipt; cold page cache; 24 h time-box (B4). Jena's TDB2 store also serves as the reference store (Jianlin). | about 0.5 day | WindowsXP, Alex for S3 |
| 3 | S3 TripleLight variants: four orders against `--orders=6`, ABBA three times on LUBM-1000 and BSBM-100M | 2–3 h (laptop: LUBM-1000 builds in 239 s with dedup) | Alex |
| 4 | Jena reference answers and BI parameter and numeric-anchor generation, unscored | 1–2 h per dataset | Jianlin |
| 5 | A7 thread scaling: TripleLight, 1, 2, 4, 8 and 16 threads, one suite, six blocks | about 5 h | WindowsXP |
| 6 | HTTP suites: seven systems, six warm blocks over the three datasets, with cold runs in blocks 1 and 4 only (B2) | about 16 h per block, about 4 days | WindowsXP |
| 7 | Four against six orders TOST: 12 paired blocks, TripleLight-only HTTP, LUBM-1000 and BSBM-100M (B3) | 6–10 h (my estimate) | WindowsXP, Alex |
| 8 | S1 small curve: TripleLight at 2 and 4 GB on LUBM-1000 and BSBM-100M, via `S1_SAMPLE=1 session.sh` | 4–6 h (my estimate) | Alex |
| 9 | Embedded and DuckDB arms, per dataset: DuckDB typed control, six-copy file and single, fixed and routed passes (2–4 h); embedded Python, TripleLight and PyOxigraph (2–4 h); embedded Java 25, TripleLight and Blazegraph where qualified (2–4 h); RDF Fusion and DuckDB `rdf` qualification (1–2 h) | 8–16 h per dataset, 24–48 h in total | Jianlin |

The sum of the high ends is about 7.5 days, so tier 1 alone can fill a one-week rental. If tier 1 runs long, cut in
the order Amendment 2 gives (B4 for loads); do not cut a tier-1 block.

### Tier 2: 1B for the HTTP cohort and TripleLight's storage claims (about 9 days, B5)

Run the datasets in this order: LUBM-8000, then WDBench, then BSBM-1B (Kaiming, within B1's tier). Real data answers
the synthetic-benchmark limitation, and BSBM's ranking claim is already decided at 100M. Finish each dataset before
starting the next, so that a rental that ends early leaves whole datasets rather than half of each.

| # | Block | Estimate | Owner |
| --- | --- | ---: | --- |
| 1 | Loads, seven systems, through `s3-build.sh`, 24 h time-box each. TripleLight about 0.6–1.5 h, QLever 1–2 h, MillenniumDB 3–10 h, Virtuoso 4–10 h, TDB2 6–15 h, Oxigraph 6–20 h, RDF4J possibly days. | about 60 h per dataset if every rival loads | WindowsXP, Alex for S3 |
| 2 | S3 TripleLight variants, four against six orders, one each | 2–3 h per dataset | Alex |
| 3 | HTTP suites, three warm blocks, with cold runs in block 1 only | about 17 h per block per dataset | WindowsXP |
| 4 | S1: TripleLight at 8 and 16 GB, `S1_SAMPLE=1` | 3–5 h per dataset (my estimate) | Alex |
| 5 | W9 / A8 derived-order stress, `derived_stress --top 50` on the four- and six-order stores | 1–2 h per dataset (my estimate; the laptop's full LUBM-1000 OPS scan takes 5.1 s) | Alex |
| 6 | WDBench (second dataset): loads, the 30-request gate on Jena's references (3–6 h, unscored), then the suite | not in B5's tier-2 estimate (see below) | Jianlin (workload), Alex (load), WindowsXP (suite) |
| 7 | Extra, only if time remains after BSBM-1B: QLever under S1 at 8 GB on LUBM-8000. Otherwise the paper says S1 covers TripleLight only. | 3–5 h (my estimate) | Alex, WindowsXP |

**A gap to flag:** WindowsXP's tier-2 load estimate (about 5 days) covers LUBM-8000 and BSBM-1B only. WDBench's
seven loads would add up to 2–3 days at the same rates, and that is not in B5's nine days. Because WDBench now runs before BSBM-1B, the shortfall falls on BSBM-1B. Every load keeps the 24 h time-box, an arm that does not finish is reported as "did not finish", and BSBM-1B reports the blocks it completed, with the block count stated (B4).

### Tier 3: 1B for the embedded and DuckDB arms (only if time remains)

These take about 30–60 h per admitted dataset (Jianlin). Run them on LUBM-8000 first, after that dataset's HTTP stores
have been deleted: DuckDB's 1 TB reservation does not fit next to all seven 1B stores. WatDiv is never run at 1B.

### Calendar (planning, from B5)

| Days | Work |
| --- | --- |
| 0 | Provisioning, all datasets, the day-one checks |
| 1–7 | Tier 1 |
| 8–12 | Tier 2: LUBM-8000 (loads, S3, three blocks, S1, W9) |
| 12–16 | Tier 2: WDBench, then BSBM-1B |
| after 16 | Tier 3, if the rental allows |

A one-week rental covers day 0 and most of tier 1. Two weeks cover tier 1, LUBM-8000 at 1B and most of WDBench,
with BSBM-1B likely partial.

## 5. Coordination

- **WindowsXP:** the HTTP suites, the W3 control and A7. They need only `curl`, `python3` (stdlib), `lsof` and `awk`,
  plus Docker with the systemd cgroup driver, and provisioning covers all of it.
- **Jianlin:** the embedded, DuckDB and reference arms. Store preparation for Jena is shared with the HTTP load, and
  reference runs stay unscored. Jianlin confirms the DuckDB `rdf` extension pin for the rental's architecture before
  tier 1.
- **Alex:** provisioning, datasets, loads under S3, S1 and W9, and disk.
- **Between blocks:** whoever finishes a block checks `df` against the floor, archives its results, and names the
  next owner in the session log.

## 6. Laptop dry run (2026-11-23; receipts in [`provision-dryrun-v1/`](provision-dryrun-v1/))

`provision.sh --dry-run --skip-images --repo <bundle> --commit <sha>` ran in a privileged Debian trixie container with
systemd on the laptop (ARM64, 10 CPUs, Docker 26.1.5 inside). It skipped the disk format, the CPU tuning and the image
step.

**What the dry run fixed:**
- `libssl-dev` and `cmake` were missing for the RDF Fusion build.
- `archive.apache.org` served about 50 KB/s, so Jena and Fuseki now come from Maven Central. The bytes are the same:
  the sha512 values match.
- The repository is private, so the script now takes `--repo`. On the rental, either set up GitHub access first or
  copy a `git bundle` of the commit under test.

**Results:**
- **Provisioning:** the final pass exited 0, and a rerun exited 0 in 8 s with nothing left to do.
- **Pins:** all verified on aarch64, including the DuckDB `rdf` extension (v2.9.0).
- **Checks:** all five day-one checks passed. The native scope and the Docker container each got `memory.max`
  268435456, `swap.max` 0 and cpuset 0, `drop_caches` worked, and the only listener was on 127.0.0.1.

**Datasets:** `datasets.sh lubm:4 bsbm:500 watdiv:1 lubm:10` found and fixed two LUBM problems:
- UBA writes `user.dir + "\" + name`, a single file with a backslash in its name on Linux.
- riot gives each file's ontology header the file's own path as its subject.

After the fixes, the runs showed the following:
- **LUBM:** the output is byte-identical with one shard and with three. LUBM-10 has 1,316,700 lines, the same count
  as the laptop's file.
- **BSBM and WatDiv:** each rerun reproduced its file's sha256.
- **WDBench:** the guard rejects an archive with the wrong line count and removes the partial file.

**Differences from the rental:**
- The WatDiv SF1 file differs from the laptop's (`docs/optimizer-watdiv-sf1/provenance.json`) because Debian's word
  list differs from macOS's. The rental generates its own files, and the receipts record them.
- The dry run checked the aarch64 pins, not the x86_64 ones the rental uses. Those are checked for the first time on
  day one, and a mismatch stops the script.
