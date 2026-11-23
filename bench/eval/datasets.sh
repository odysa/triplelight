#!/usr/bin/env bash
# Generate (or fetch) the evaluation datasets on the dedicated machine (docs/eval-plan/RUNBOOK.md; owner Alex).
# Run after provision.sh, as any user that can write $DATA:   . /data/eval.env; bench/eval/datasets.sh <name>...
# Names: lubm1000 lubm8000 bsbm100m bsbm1b watdiv-sf1000 wdbench=<archive path or direct URL>; for a qualification at a
# small size: lubm:<universities> bsbm:<products> watdiv:<scale factor>
# Each dataset becomes one N-Triples file $DATA/datasets/<name>.nt (one input file per dataset for every system, plan
# "Loading"), with a receipt $DATA/receipts/dataset-<name>.txt: generator, its archive hash, the exact command, wall
# time, bytes, lines and sha256. A finished dataset (receipt present) is skipped, so a rerun resumes.
set -euo pipefail
: "${DATA:?source /data/eval.env first}" "${TOOLS:?}"
OUT=$DATA/datasets; G=$TOOLS/gen; RIOT=$TOOLS/apache-jena-6.2.0/bin/riot
mkdir -p "$OUT" "$G" "$DATA/receipts"
JOBS=${JOBS:-$(nproc)}
UBA_URL=http://swat.cse.lehigh.edu/projects/lubm/uba1.7.zip
UBA_SHA=3d44f468e36b7f3cd532f0f5693a019d020877846397b1fa657367cbd53f380a
BSBM_URL=https://downloads.sourceforge.net/project/bsbmtools/bsbmtools/bsbmtools-0.2/bsbmtools-v0.2.zip
BSBM_SHA=40f5e59baadec3af0014b7647989d3e0fc0476af25e84a4bc9d7f8cd81520aaa
WATDIV_URL=https://dsg.uwaterloo.ca/watdiv/watdiv_v06.tar
WATDIV_SHA=fb8d930b74b3fbc8f948101bfaf658a90d2f74002f1fefda465c45ffd33a71d2   # = docs/optimizer-watdiv-sf1/provenance.json
WATDIV_SEED=20261112   # the two srand/time seeds patched as in docs/optimizer-watdiv-sf1/GENERATE.md
WDBENCH_TRIPLES=1257169959
ONTO=http://swat.cse.lehigh.edu/onto/univ-bench.owl

fetch() {  # fetch <url> <dest> <sha256>
  [ -f "$2" ] || { curl -fsSL --retry 3 -o "$2.part" "$1" && mv "$2.part" "$2"; }
  [ "$(sha256sum "$2" | cut -d' ' -f1)" = "$3" ] || { echo "checksum mismatch: $2" >&2; rm -f "$2"; exit 3; }
}
receipt() {  # receipt <name> <start epoch> <generator line> <command line>
  local f=$OUT/$1.nt r=$DATA/receipts/dataset-$1.txt
  { echo "dataset $1"; echo "generator $3"; echo "command $4"; echo "wall_s $(( $(date +%s) - $2 ))"
    echo "bytes $(stat -c %s "$f")"; echo "lines $(wc -l < "$f")"; echo "sha256 $(sha256sum "$f" | cut -d' ' -f1)"
    echo "finished $(date -u +%FT%TZ)"; } > "$r.part"
  mv "$r.part" "$r"; cat "$r"
}
done_already() { [ -f "$DATA/receipts/dataset-$1.txt" ] && { echo "$1: done ($DATA/receipts/dataset-$1.txt)"; }; }

lubm() {  # lubm <universities>: UBA 1.7, seed 0, sharded by start index. UBA reseeds every university from
  # (seed, index), so a shard of universities i..j writes the same files a single run would. UBA names its files
  # user.dir + "\\" + name (Windows separators), so user.dir is set to "u" and that prefix is stripped. Each file's
  # ontology header has the file's own URL as subject; it is rewritten to file:///lubm/<file>, so the output does not
  # depend on the shard directory.
  local n=$1 name=lubm$1 t0; t0=$(date +%s); done_already "$name" && return
  fetch "$UBA_URL" "$G/uba1.7.zip" "$UBA_SHA"; [ -d "$G/uba/classes" ] || unzip -q -o "$G/uba1.7.zip" -d "$G/uba"
  local W=$DATA/scratch/gen-$name per=$(( (n + JOBS - 1) / JOBS )) pids=(); rm -rf "$W"; mkdir -p "$W"
  for ((s = 0; s * per < n; s++)); do
    local i0=$(( s * per )) k=$(( n - s * per < per ? n - s * per : per ))
    ( mkdir -p "$W/$s" && cd "$W/$s" \
      && java -Duser.dir=u -cp "$G/uba/classes" edu.lehigh.swat.bench.uba.Generator -univ $k -index $i0 -seed 0 -onto "$ONTO" > gen.log 2>&1 \
      && for f in u\\*; do mv -- "$f" "${f#u\\}"; done \
      && printf '%s\n' University*.owl | sort -t_ -k1.11n -k2n | xargs -n 2000 "$RIOT" --output=ntriples \
         | sed "s#<file://$W/$s/#<file:///lubm/#g" > "$W/$s.nt" \
      && rm -f University*.owl ) &
    pids+=($!)
  done
  for s in "${!pids[@]}"; do wait "${pids[$s]}" || { echo "$name: shard $s failed ($W/$s/gen.log)" >&2; exit 4; }; done
  for ((s = 0; s * per < n; s++)); do cat "$W/$s.nt"; rm -f "$W/$s.nt"; done > "$OUT/$name.nt"
  rm -rf "$W"
  receipt "$name" "$t0" "uba1.7.zip sha256 $UBA_SHA, riot (Jena 6.2.0), $JOBS shards of $per universities" \
    "Generator -univ <k> -index <i0> -seed 0 -onto $ONTO; riot --output=ntriples, files in university/department order, header subjects file:///lubm/<file>"
}

bsbm() {  # bsbm <name> <product count>: BSBM tools 0.2, explore data and its td_data (the test driver needs it)
  local name=$1 pc=$2 t0; t0=$(date +%s); done_already "$name" && return
  fetch "$BSBM_URL" "$G/bsbmtools-v0.2.zip" "$BSBM_SHA"
  local W=$DATA/scratch/gen-$name; rm -rf "$W"; mkdir -p "$W"; unzip -q -o "$G/bsbmtools-v0.2.zip" -d "$W"
  ( cd "$W/bsbmtools-0.2" && chmod +x generate && ./generate -fc -pc "$pc" -s nt -fn "$name" -dir "td_data-$name" > gen.log 2>&1 )
  mv "$W/bsbmtools-0.2/$name.nt" "$OUT/$name.nt"; rm -rf "$OUT/td_data-$name"; mv "$W/bsbmtools-0.2/td_data-$name" "$OUT/"
  rm -rf "$W"
  receipt "$name" "$t0" "bsbmtools-v0.2.zip sha256 $BSBM_SHA" "generate -fc -pc $pc -s nt -fn $name -dir td_data-$name"
}

watdiv() {  # watdiv <name> <scale factor>: WatDiv v0.6 with the registered seed patch; keeps the saved state, which
  # query instantiation (watdiv -q) reads, next to the data
  local name=$1 sf=$2 t0; t0=$(date +%s); done_already "$name" && return
  fetch "$WATDIV_URL" "$G/watdiv_v06.tar" "$WATDIV_SHA"
  local B=$G/watdiv
  if [ ! -x "$B/bin/Release/watdiv" ]; then
    rm -rf "$B"; tar -xf "$G/watdiv_v06.tar" -C "$G"
    sed -i "s/static_cast<unsigned> (time(0))/static_cast<unsigned> ($WATDIV_SEED)/; s/srand (time(NULL))/srand ($WATDIV_SEED)/" "$B/src/model.cpp"
    [ "$(grep -c "$WATDIV_SEED" "$B/src/model.cpp")" = 2 ] || { echo "watdiv seed patch did not apply" >&2; exit 4; }
    mkdir -p "$B/bin/Release"
    g++ -std=c++14 -O2 -w -I"$B/include" "$B"/src/{dictionary,model,statistics,volatility_gen}.cpp -o "$B/bin/Release/watdiv"
  fi
  ( cd "$B/bin/Release" && ./watdiv -d ../../model/wsdbm-data-model.txt "$sf" > "$OUT/$name.nt" )
  cp "$B/bin/Release/saved.txt" "$OUT/watdiv-saved-sf$sf.txt"
  receipt "$name" "$t0" "watdiv_v06.tar sha256 $WATDIV_SHA, model.cpp seeds $WATDIV_SEED, g++ $(g++ -dumpfullversion), words sha256 $(sha256sum /usr/share/dict/words | cut -d' ' -f1)" \
    "watdiv -d model/wsdbm-data-model.txt $sf; saved state $OUT/watdiv-saved-sf$sf.txt sha256 $(sha256sum "$OUT/watdiv-saved-sf$sf.txt" | cut -d' ' -f1)"
}

wdbench() {  # wdbench <archive path or URL>: the 2021-06-23 direct-property graph from the WDBench Figshare item,
  # streamed into one N-Triples file; the archive is kept until the graph is verified, then may be deleted (RUNBOOK)
  local src=$1 name=wdbench t0; t0=$(date +%s); done_already "$name" && return
  local A=$src
  if [[ $src == http* ]]; then  # a direct download URL; keep its file name (and so its archive type)
    local base=${src%%\?*}; A=$OUT/${base##*/}
    if [ ! -f "$A" ]; then curl -fL --retry 3 -o "$A.part" "$src"; mv "$A.part" "$A"; fi
  fi
  [ -f "$A" ] || { echo "wdbench: no archive at $A" >&2; exit 2; }
  local ash; ash=$(sha256sum "$A" | cut -d' ' -f1)
  case $A in
    *.zip) unzip -p "$A" '*.nt' ;; *.bz2) (command -v lbzip2 >/dev/null && lbzip2 -dc "$A" || bzip2 -dc "$A") ;;
    *.gz) gzip -dc "$A" ;; *.nt) cat "$A" ;; *) echo "wdbench: unknown archive type $A" >&2; exit 2 ;;
  esac > "$OUT/$name.nt.part"
  local n; n=$(wc -l < "$OUT/$name.nt.part")
  [ "$n" = "$WDBENCH_TRIPLES" ] || { echo "wdbench: $n lines, the registered graph has $WDBENCH_TRIPLES" >&2; rm -f "$OUT/$name.nt.part"; exit 5; }
  mv "$OUT/$name.nt.part" "$OUT/$name.nt"
  receipt "$name" "$t0" "WDBench Figshare item 50b7544ad6b1f51de060, archive $(basename "$A") sha256 $ash" "stream-decompressed, $n lines checked"
}

[ $# -gt 0 ] || { sed -n 2,7p "$0"; exit 2; }
for d in "$@"; do case $d in
  lubm1000) lubm 1000;; lubm8000) lubm 8000;;
  bsbm100m) bsbm bsbm100m 284826;; bsbm1b) bsbm bsbm1b 2848260;;
  watdiv-sf1000) watdiv watdiv-sf1000 1000;;   # about 110M triples; label it by scale factor (the laptop file is SF100, 10.9M)
  wdbench=*) wdbench "${d#wdbench=}";;
  lubm:*) lubm "${d#lubm:}";; bsbm:*) bsbm "bsbm-pc${d#bsbm:}" "${d#bsbm:}";; watdiv:*) watdiv "watdiv-sf${d#watdiv:}" "${d#watdiv:}";;  # small qualification sizes
  *) echo "unknown dataset $d" >&2; exit 2;; esac; done
