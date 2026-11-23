#!/bin/bash
# B4 follow-up: is LUBM-1000 warm-query time slower on main than at a03774f2 (the #83 sweep binary)? One store built by
# main (four orders; readable by both), then ABBA at the default cache budget with sweep.py: old, main, main, old.
# Records `docker ps` and the top CPU processes before each arm. Takes and releases /tmp/rdf-bench.lock.
set -u
OUT=/Users/odysa/.slock/agents/78888e08-1e35-4024-96d3-04d2a9dfaf6c/deliverables/ab-b4
REPO=/Users/odysa/.slock/agents/78888e08-1e35-4024-96d3-04d2a9dfaf6c/triplelight-named-storage
DATA=/Users/odysa/.slock/agents/b438313f-6338-4d86-ab3e-5995a1bde6c3/rdfdb/bench/tools
OLD=a03774f2; NEW=${NEW:-$(git -C $REPO rev-parse --short=8 origin/main)}; W=/tmp/abb4
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
mkdir /tmp/rdf-bench.lock || { log "lock held, abort"; exit 3; }
echo "Alex-Chi B4 A/B ($OLD vs $NEW)" > /tmp/rdf-bench.lock/owner
trap 'rm -rf /tmp/rdf-bench.lock' EXIT
rm -rf $W; mkdir -p $W; git -C $REPO worktree prune
for sha in $OLD $NEW; do
  git -C $REPO worktree add -q --detach $W/tree-$sha $sha || exit 4
  log "building $sha"
  (cd $W/tree-$sha && CARGO_TARGET_DIR=$W/target-$sha cargo build -q --release --locked --features server --bin triplelight) || { log "build $sha failed"; exit 5; }
  cp $W/target-$sha/release/triplelight $W/bin-$sha; rm -rf $W/target-$sha
  echo "$sha $(shasum -a 256 $W/bin-$sha | cut -d' ' -f1)" >> $OUT/binaries.txt
done
log "building lubm1000 store with $NEW"
$W/bin-$NEW build-disk $DATA/lubm1000.nt $W/store > $OUT/build.log 2>&1 || { log "store build failed"; exit 6; }
$W/bin-$OLD verify $W/store > $OUT/verify-old.txt 2>&1 || { log "old binary cannot verify the store"; exit 7; }
i=0
for sha in $OLD $NEW $NEW $OLD; do
  i=$((i+1)); tag=r$i-$sha
  { date -u; docker ps --format '{{.Names}} {{.Status}}'; ps -Ao pcpu,comm -r | head -6; } > $OUT/$tag.load.txt 2>&1
  log "arm $tag"
  python3 -I $W/tree-$NEW/docs/cache-budget-sweep-v1/sweep.py --bin $W/bin-$sha --store $W/store --suite $W/tree-$NEW/bench/lubm \
    --label $tag --budgets 0,0,0 --out $OUT > $OUT/$tag.txt 2>&1; log "$tag exit $?: $(tail -1 $OUT/$tag.txt)"
done
for sha in $OLD $NEW; do git -C $REPO worktree remove --force $W/tree-$sha; done; rm -rf $W
log "ALL-DONE"
