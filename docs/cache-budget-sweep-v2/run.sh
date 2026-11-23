#!/bin/bash
# B4: the #83 cache-budget sweep re-run on main after #131. Caller holds /tmp/rdf-bench.lock.
set -u
OUT=<workspace>/deliverables/b4-sweep
REPO=<workspace>/triplelight-named-storage
DATA=<workspace>/rdfdb/bench/tools
SHA=a08e3440
WORK=/tmp/sweepb4
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
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
python3 -I "$WORK/tree/docs/cache-budget-sweep-v1/sweep.py" --bin "$BIN" --store "$WORK/lubm1000" --suite "$WORK/tree/bench/lubm" --label lubm1000 \
  --budgets 0,512,1024,2048,10240,0 --out "$OUT" > "$OUT/lubm1000.txt" 2>&1; log "lubm1000 exit $?"
log "sweep bsbm100 (descending)"
python3 -I "$WORK/tree/docs/cache-budget-sweep-v1/sweep.py" --bin "$BIN" --store "$WORK/bsbm100" --suite "$WORK/tree/bench/bsbm" --label bsbm100 \
  --budgets 0,10240,2048,1024,512,0 --out "$OUT" > "$OUT/bsbm100.txt" 2>&1; log "bsbm100 exit $?"
git -C "$REPO" worktree remove --force "$WORK/tree"; rm -rf "$WORK/target"
rm -rf "$WORK"; log "ALL-DONE"
