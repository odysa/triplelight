#!/bin/bash
# #88 same-session build comparison: --orders=6 vs --orders=4, ABBA per dataset (registered in (internal discussion)).
# Takes and releases /tmp/rdf-bench.lock itself.
set -u
OUT=${OUT_DIR:?set OUT_DIR to the directory for the logs and results}
DATA=${DATA_DIR:?set DATA_DIR to the directory holding the .nt datasets}
BIN=/tmp/sweep83/triplelight
WORK=/tmp/build88
MIN_FREE_KB=$((15 * 1024 * 1024))
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/progress.log"; }
mkdir /tmp/rdf-bench.lock || { log "lock held, abort"; exit 3; }
echo "Alex-Chi #88 build comparison" > /tmp/rdf-bench.lock/owner
trap 'rm -rf /tmp/rdf-bench.lock; [ -n "${SAMPLER:-}" ] && kill $SAMPLER 2>/dev/null' EXIT
shasum -a 256 "$BIN" > "$OUT/binary.sha256"
[ -z "${ONLY:-}" ] && rm -rf "$WORK"; mkdir -p "$WORK"
for d in lubm1000 bsbm100; do
  i=0
  for orders in 6 4 4 6; do
    i=$((i + 1)); base="$d-o$orders-r$i"; tag="$base${SUFFIX:-}"; dir="$WORK/$tag"
    # ONLY=<tag> reruns one build (after the 2-hour session limit stopped the full run).
    if [ -n "${ONLY:-}" ] && ! [[ ",$ONLY," == *",$base,"* ]]; then continue; fi
    log "build $tag"
    # Sampler: output directory size and volume free space every 2 s; stops the build if free space runs low.
    ( peak=0; low=999999999999
      while :; do
        s=$(du -sk "$dir" 2>/dev/null | cut -f1); s=${s:-0}; f=$(df -k /tmp | awk 'NR==2{print $4}')
        [ "$s" -gt "$peak" ] && peak=$s; [ "$f" -lt "$low" ] && low=$f
        echo "$peak $low" > "$OUT/$tag.disk"
        # Coarse phase timeline: when each file in the output directory first appears and its size each sample.
        (cd "$dir" 2>/dev/null && du -sk .[!.]* * 2>/dev/null) | awk -v t="$(date -u +%H:%M:%S)" '{print t, $1*1024, $2}' >> "$OUT/$tag.files"
        if [ "$f" -lt "$MIN_FREE_KB" ]; then pkill -f "build-disk $DATA/$d.nt $dir"; echo "LOW-DISK" >> "$OUT/$tag.disk"; fi
        sleep 2
      done ) & SAMPLER=$!
    /usr/bin/time -l "$BIN" build-disk "$DATA/$d.nt" "$dir" --orders=$orders > "$OUT/$tag.build.log" 2>&1; rc=$?
    kill $SAMPLER 2>/dev/null; wait $SAMPLER 2>/dev/null; SAMPLER=
    if [ $rc -ne 0 ]; then log "$tag build failed ($rc)"; exit 5; fi
    "$BIN" verify "$dir" > "$OUT/$tag.verify.txt" 2>&1 || { log "$tag verify failed"; exit 6; }
    (cd "$dir" && shasum -a 256 dictionary.unified spo pso pos osp) > "$OUT/$tag.kept.sha256"
    log "done $tag: $(du -sh "$dir" | cut -f1), disk $(cat "$OUT/$tag.disk" | head -1)"
    rm -rf "$dir"
  done
done
log "ALL-DONE"
