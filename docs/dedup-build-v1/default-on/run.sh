#!/bin/bash
# #91 productizing checks on storage/dedup-parse. Takes and releases /tmp/rdf-bench.lock.
# 1. clippy + storage tests, feature off and rdf-12. 2. LUBM-1000 four-order builds: default (budget-derived map),
# 32 MB map per shard, default again (ABA); hashes must match. 3. Linux binary in Docker, LUBM-100 build under a 4 GiB
# cgroup (memory.max), default and dedup off: exit status and the cgroup's memory.peak.
set -u
OUT=${OUT:?set OUT; on the test laptop it was <workspace>/deliverables/dedup-productize}
REPO=${REPO:?set REPO; on the test laptop it was <workspace>/triplelight-named-storage}
DATA=${DATA:?set DATA; on the test laptop it was <workspace>/rdfdb/bench/tools}
SHA=$1; W=/tmp/dedupprod
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
mkdir /tmp/rdf-bench.lock || { log "lock held, abort"; exit 3; }
echo "Alex-Chi #91 dedup productizing checks" > /tmp/rdf-bench.lock/owner
trap 'rm -rf /tmp/rdf-bench.lock' EXIT
rm -rf $W; mkdir -p $W; git -C $REPO worktree prune; git -C $REPO worktree add -q --detach $W/tree $SHA || exit 4
cd $W/tree; export CARGO_TARGET_DIR=$W/target
cargo clippy --locked -p triplelight-storage --all-targets -- -D warnings > $OUT/clippy-off.log 2>&1; log "clippy off $?"
cargo clippy --locked -p triplelight-storage -p triplelight-core -p triplelight-exec --all-targets --features triplelight-storage/rdf-12,triplelight-exec/rdf-12 -- -D warnings > $OUT/clippy-on.log 2>&1; log "clippy on $?"
cargo test --locked -p triplelight-storage > $OUT/test-off.log 2>&1; log "test off $?: $(grep -E '^test result' $OUT/test-off.log | head -1)"
cargo test --locked -p triplelight-storage -p triplelight-exec --features triplelight-storage/rdf-12,triplelight-exec/rdf-12 > $OUT/test-on.log 2>&1; log "test on $?: $(grep -E '^test result: ok. [0-9]{3}' $OUT/test-on.log)"
grep -E 'dedup.* \.\.\. ' $OUT/test-off.log $OUT/test-on.log >> $OUT/progress.log
cargo build -q --release --locked --bin triplelight && cp $W/target/release/triplelight $W/bin; echo "$SHA $(shasum -a 256 $W/bin | cut -d' ' -f1)" > $OUT/binary.txt
i=0
for map in auto 32 auto; do
  i=$((i+1)); tag=r$i-map$map; dir=$W/$tag
  { date -u; docker ps --format '{{.Names}}'; } > $OUT/$tag.load.txt 2>&1
  log "build $tag"
  if [ $map = auto ]; then env TRIPLELIGHT_DEDUP_TRACE=1 /usr/bin/time -l $W/bin build-disk $DATA/lubm1000.nt $dir --build-external > $OUT/$tag.build.log 2>&1
  else env TRIPLELIGHT_DEDUP_TRACE=1 TRIPLELIGHT_DEDUP_MAP_MB=$map /usr/bin/time -l $W/bin build-disk $DATA/lubm1000.nt $dir --build-external > $OUT/$tag.build.log 2>&1; fi
  rc=$?; [ $rc -ne 0 ] && { log "$tag failed $rc"; exit 6; }
  $W/bin verify $dir > $OUT/$tag.verify.txt 2>&1 || { log "$tag verify failed"; exit 7; }
  (cd $dir && shasum -a 256 *) > $OUT/$tag.sha256; rm -rf $dir
  log "done $tag: $(grep -E ' real ' $OUT/$tag.build.log | awk '{print $1}') s, rss $(grep 'maximum resident' $OUT/$tag.build.log | awk '{print $1}'), $(grep -o 'flushes=[0-9]*' $OUT/$tag.build.log | sort | uniq -c | tr '\n' ' ')"
done
cmp -s $OUT/r1-mapauto.sha256 $OUT/r2-map32.sha256 && cmp -s $OUT/r1-mapauto.sha256 $OUT/r3-mapauto.sha256 && log "all three IDENTICAL" || log "hashes DIFFER"
log "linux binary (docker)"
docker run --rm -v $W/tree:/src:ro -v $W/linux:/out rust:latest sh -c "mkdir /b && cp -r /src/src /src/crates /src/Cargo.toml /src/Cargo.lock /src/rust-toolchain.toml /b/ && cd /b && CARGO_TARGET_DIR=/b/t cargo build -q --release --locked --bin triplelight && cp /b/t/release/triplelight /out/" > $OUT/linux-build.log 2>&1 || { log "linux build failed"; exit 8; }
for mode in default off; do
  envs=""; [ $mode = off ] && envs="-e TRIPLELIGHT_DEDUP_TERMS=0"
  docker run --rm --memory=4g --memory-swap=4g $envs -e TRIPLELIGHT_DEDUP_TRACE=1 -v $W/linux:/bin2:ro -v $DATA/lubm100.nt:/in.nt:ro debian:trixie-slim \
    sh -c "/bin2/triplelight build-disk /in.nt /tmp/s --build-external; rc=\$?; echo exit=\$rc; cat /sys/fs/cgroup/memory.peak; cat /sys/fs/cgroup/memory.max; /bin2/triplelight verify /tmp/s | tail -1" > $OUT/cgroup-$mode.log 2>&1
  log "cgroup 4g $mode: $(grep -E '^exit=' $OUT/cgroup-$mode.log) peak $(sed -n '/^exit=/{n;p;}' $OUT/cgroup-$mode.log) bytes"
done
cd /; git -C $REPO worktree remove --force $W/tree; rm -rf $W
log "ALL-DONE"
