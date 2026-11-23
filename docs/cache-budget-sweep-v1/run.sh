#!/bin/bash
# #83 cache-budget sweep driver. Takes and releases /tmp/rdf-bench.lock itself.
set -u
OUT=${OUT:?set OUT; on the test laptop it was <workspace>/deliverables/cache-sweep-83}
REPO=${REPO:?set REPO; on the test laptop it was <workspace>/triplelight-named-storage}
DATA=${DATA:?set DATA; on the test laptop it was <workspace>/rdfdb/bench/tools}
SHA=a03774f2
WORK=/tmp/sweep83
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
mkdir /tmp/rdf-bench.lock || { log "lock held, abort"; exit 3; }
echo "Alex-Chi #83 cache sweep" > /tmp/rdf-bench.lock/owner
trap 'rm -rf /tmp/rdf-bench.lock' EXIT
rm -rf "$WORK"; git -C "$REPO" worktree prune; mkdir -p "$WORK"
git -C "$REPO" worktree add -q --detach "$WORK/tree" $SHA || exit 4
log "building binary at $SHA"
(cd "$WORK/tree" && CARGO_TARGET_DIR="$WORK/target" cargo build -q --release --locked --features server --bin triplelight) || { log "build failed"; exit 5; }
BIN="$WORK/triplelight"; cp "$WORK/target/release/triplelight" "$BIN"; shasum -a 256 "$BIN" > "$OUT/binary.sha256"
for d in lubm1000 bsbm100; do
  log "building $d store (default four orders)"
  /usr/bin/time -l "$BIN" build-disk "$DATA/$d.nt" "$WORK/$d" > "$OUT/build-$d.log" 2>&1 || { log "build $d failed"; exit 6; }
  "$BIN" verify "$WORK/$d" > "$OUT/verify-$d.txt" 2>&1 || { log "verify $d failed"; exit 7; }
  log "built $d: $(du -sh "$WORK/$d" | cut -f1)"
done
log "sweep lubm1000 (ascending)"
python3 -I "$OUT/sweep.py" --bin "$BIN" --store "$WORK/lubm1000" --suite "$WORK/tree/bench/lubm" --label lubm1000 \
  --budgets 0,512,1024,2048,10240,0 --out "$OUT" > "$OUT/lubm1000.txt" 2>&1; log "lubm1000 exit $?"
log "sweep bsbm100 (descending)"
python3 -I "$OUT/sweep.py" --bin "$BIN" --store "$WORK/bsbm100" --suite "$WORK/tree/bench/bsbm" --label bsbm100 \
  --budgets 0,10240,2048,1024,512,0 --out "$OUT" > "$OUT/bsbm100.txt" 2>&1; log "bsbm100 exit $?"
git -C "$REPO" worktree remove --force "$WORK/tree"; rm -rf "$WORK/target"
log "ALL-DONE (stores kept in $WORK)"
