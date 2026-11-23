#!/bin/bash
# #91 identity gates: one binary at perf/dedup-91 (9770ceb3); every store file must hash the same with the
# deduplicating parse off and on. LUBM-100 and BSBM-10M at four and six orders, plus LUBM-100 four orders with a 1 MB
# term map per shard (forces constant eviction). Caller holds /tmp/rdf-bench.lock.
set -u
OUT=<workspace>/deliverables/dedup-91-gates
REPO=<workspace>/triplelight-named-storage
DATA=<workspace>/rdfdb/bench/tools
SHA=9770ceb3; W=/tmp/gates91
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
rm -rf $W; mkdir -p $W; git -C $REPO worktree prune
git -C $REPO worktree add -q --detach $W/tree $SHA || exit 4
log "building at $SHA"
(cd $W/tree && CARGO_TARGET_DIR=$W/target cargo build -q --release --locked --bin triplelight) || { log "build failed"; exit 5; }
B=$W/target/release/triplelight; echo "$SHA $(shasum -a 256 $B | cut -d' ' -f1)" > $OUT/binary.txt
one() { # tag dataset orders env...
  local tag=$1 ds=$2 o=$3; shift 3
  env "$@" /usr/bin/time -l $B build-disk $DATA/$ds.nt $W/$tag --orders=$o --build-external > $OUT/$tag.build.log 2>&1 || { log "$tag build failed"; exit 6; }
  grep -q "strategy: external" $OUT/$tag.build.log || { log "$tag not external"; exit 8; }
  $B verify $W/$tag > $OUT/$tag.verify.txt 2>&1 || { log "$tag verify failed"; exit 7; }
  (cd $W/$tag && shasum -a 256 *) > $OUT/$tag.sha256; rm -rf $W/$tag
  log "done $tag $(grep -E ' real ' $OUT/$tag.build.log | awk '{print $1}')s"
}
cmpit() { cmp -s $OUT/$1.sha256 $OUT/$2.sha256 && log "$1 vs $2: IDENTICAL" || log "$1 vs $2: DIFFER"; }
for ds in lubm100 bsbm10; do for o in 4 6; do
  one $ds-o$o-off $ds $o TRIPLELIGHT_DEDUP_TERMS=0
  one $ds-o$o-on $ds $o TRIPLELIGHT_DEDUP_TERMS=1
  cmpit $ds-o$o-off $ds-o$o-on
done; done
one lubm100-o4-tiny lubm100 4 TRIPLELIGHT_DEDUP_TERMS=1 TRIPLELIGHT_DEDUP_MAP_MB=1
cmpit lubm100-o4-off lubm100-o4-tiny
git -C $REPO worktree remove --force $W/tree; rm -rf $W
log "ALL-DONE"
