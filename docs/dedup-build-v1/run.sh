#!/bin/bash
# #91 LUBM-1000 ABBA: one binary at perf/dedup-91 (9770ceb3), four-order external builds with the deduplicating
# parse off, on, on, off. Records wall/user/RSS (/usr/bin/time -l), the build directory's peak size and free space
# (2 s sampler), the per-shard dedup lines (flushes, record bytes to sort), the containers before each build, and
# the file hashes (each "on" build must equal each "off" build). Takes and releases /tmp/rdf-bench.lock.
set -u
OUT=<workspace>/deliverables/dedup-91-abba
REPO=<workspace>/triplelight-named-storage
DATA=<workspace>/rdfdb/bench/tools
SHA=9770ceb3; W=/tmp/abba91
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
mkdir /tmp/rdf-bench.lock || { log "lock held, abort"; exit 3; }
echo "Alex-Chi #91 LUBM-1000 ABBA" > /tmp/rdf-bench.lock/owner
trap 'rm -rf /tmp/rdf-bench.lock; [ -n "${SAMPLER:-}" ] && kill $SAMPLER 2>/dev/null' EXIT
rm -rf $W; mkdir -p $W; git -C $REPO worktree prune
git -C $REPO worktree add -q --detach $W/tree $SHA || exit 4
log "building at $SHA"
(cd $W/tree && CARGO_TARGET_DIR=$W/target cargo build -q --release --locked --bin triplelight) || { log "build failed"; exit 5; }
B=$W/bin; cp $W/target/release/triplelight $B; rm -rf $W/target; echo "$SHA $(shasum -a 256 $B | cut -d' ' -f1)" > $OUT/binary.txt
i=0
for mode in 0 1 1 0; do
  i=$((i+1)); tag=r$i-dedup$mode; dir=$W/$tag
  { date -u; docker ps --format '{{.Names}} {{.Status}}'; ps -Ao pcpu,comm -r | head -6; } > $OUT/$tag.load.txt 2>&1
  log "build $tag"
  ( peak=0; low=999999999999
    while :; do
      s=$(du -sk "$dir" 2>/dev/null | cut -f1); s=${s:-0}; f=$(df -k /tmp | awk 'NR==2{print $4}')
      [ "$s" -gt "$peak" ] && peak=$s; [ "$f" -lt "$low" ] && low=$f
      echo "$peak $low" > "$OUT/$tag.disk"
      if [ "$f" -lt $((15 * 1024 * 1024)) ]; then pkill -f "build-disk $DATA/lubm1000.nt $dir"; echo LOW-DISK >> "$OUT/$tag.disk"; fi
      sleep 2
    done ) & SAMPLER=$!
  TRIPLELIGHT_DEDUP_TERMS=$mode /usr/bin/time -l $B build-disk $DATA/lubm1000.nt $dir --build-external > $OUT/$tag.build.log 2>&1; rc=$?
  kill $SAMPLER 2>/dev/null; wait $SAMPLER 2>/dev/null; SAMPLER=
  [ $rc -ne 0 ] && { log "$tag build failed ($rc)"; exit 6; }
  $B verify $dir > $OUT/$tag.verify.txt 2>&1 || { log "$tag verify failed"; exit 7; }
  (cd $dir && shasum -a 256 *) > $OUT/$tag.sha256; rm -rf $dir
  log "done $tag: $(grep -E ' real ' $OUT/$tag.build.log | awk '{print $1}') s, disk $(head -1 $OUT/$tag.disk)"
done
for p in "r1-dedup0 r2-dedup1" "r4-dedup0 r3-dedup1"; do
  a=${p% *}; b=${p#* }
  cmp -s $OUT/$a.sha256 $OUT/$b.sha256 && log "$a vs $b: IDENTICAL" || log "$a vs $b: DIFFER"
done
git -C $REPO worktree remove --force $W/tree; rm -rf $W
log "ALL-DONE"
