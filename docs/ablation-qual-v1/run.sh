#!/bin/bash
# #92 ablation qualification: every answer-preserving switch must give the default's answers. Correctness only.
set -uo pipefail
WT=${WT:-$(git rev-parse --show-toplevel)}; T=${TRIPLELIGHT_DATA:?set to the directory holding lubm100.nt, bsbm10.nt, watdiv100.nt}
OUT=/tmp/abl92/out; S=/tmp/abl92/stores; LOCK=/tmp/rdf-bench.lock
if ! mkdir "$LOCK" 2>/dev/null; then grep -q "Kaiming-He #92" "$LOCK/owner" 2>/dev/null || { echo "lock busy: $(cat $LOCK/owner 2>/dev/null)"; exit 3; }; fi
echo "Kaiming-He #92 ablation check" > "$LOCK/owner"
cleanup() { pkill -f "abl92/stores" 2>/dev/null; rm -rf "$S"; if grep -q "Kaiming-He #92" "$LOCK/owner" 2>/dev/null; then rm -rf "$LOCK"; fi; echo "lock released"; }
trap cleanup EXIT
rm -rf "$OUT" "$S"; mkdir -p "$OUT" "$S"
cd "$WT" || exit 1
git rev-parse HEAD > "$OUT/head"
rtk proxy cargo test --release --test exec > "$OUT/test-exec.log" 2>&1 || { echo TEST FAIL; tail -30 "$OUT/test-exec.log"; exit 1; }
rtk proxy cargo build --release --features server --bin triplelight > "$OUT/build.log" 2>&1 || { echo BUILD FAIL; tail -30 "$OUT/build.log"; exit 1; }
B=$WT/target/release/triplelight
$B serve-store /nonexistent 1 2>&1 | grep -q "needs the HTTP server" && { echo "SERVER FEATURE MISSING"; exit 1; }
CONFIGS=("default|" "wcoj|--ablate=wcoj" "count|--ablate=count" "fastgroup|--ablate=fast-group" "nostats|--no-stats" "all|--ablate=wcoj,count,fast-group --no-stats")
run_ds() { # name nt suites...
  local ds=$1 nt=$2; shift 2
  # build-disk writes a bare base; serve-store needs a store directory (CURRENT + base-<n>), else it serves an empty store.
  mkdir -p "$S/$ds-4" "$S/$ds-6"
  /usr/bin/time -l $B build-disk "$nt" "$S/$ds-4/base-1" --build-external > "$OUT/$ds-build4.log" 2>&1 || { echo "$ds build4 FAIL"; return 1; }
  /usr/bin/time -l $B build-disk "$nt" "$S/$ds-6/base-1" --build-external --orders=6 > "$OUT/$ds-build6.log" 2>&1 || { echo "$ds build6 FAIL"; return 1; }
  echo base-1 > "$S/$ds-4/CURRENT"; echo base-1 > "$S/$ds-6/CURRENT"
  local want; want=$($B query "$S/$ds-4/base-1" 'SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }' --disk 2>/dev/null | grep -o '"value":"[0-9]*"')
  echo "$ds triples $want"
  local port=7990 eps=() pids=()
  for c in "${CONFIGS[@]}"; do
    name=${c%%|*}; flags=${c#*|}
    cp -cR "$S/$ds-4" "$S/$ds-4-$name"   # APFS clone: one store directory per process (file lock)
    env $( [ "$name" = default ] && echo TRIPLELIGHT_TRACE_DERIVED=1 ) $B serve-store "$S/$ds-4-$name" $port --disk --read-only $flags > "$OUT/$ds-$name.server.log" 2>&1 & pids+=($!)
    eps+=("$name=http://127.0.0.1:$port/sparql"); port=$((port+1))
  done
  $B serve-store "$S/$ds-6" $port --disk --read-only > "$OUT/$ds-six.server.log" 2>&1 & pids+=($!)
  eps+=("six=http://127.0.0.1:$port/sparql")
  for p in $(seq 7990 $port); do for i in $(seq 1 120); do /usr/bin/curl -s -o /dev/null "http://127.0.0.1:$p/sparql?query=ASK%7B%7D" && break; sleep 1; done; done
  for p in $(seq 7990 $port); do
    got=$(/usr/bin/curl -s "http://127.0.0.1:$p/sparql" --data-urlencode 'query=SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }' | grep -o '"value":"[0-9]*"')
    [ "$got" = "$want" ] || { echo "$ds port $p COUNT MISMATCH got=$got want=$want FAIL"; kill "${pids[@]}"; return 1; }
  done
  echo "$ds all endpoints serve $want"
  for suite in "$@"; do
    (cd "$OUT" && python3 /tmp/abl92/run1.py "$WT/$suite" "${eps[@]}" > "$OUT/$ds-$(echo $suite | tr / _).txt" 2>&1)
  done
  kill "${pids[@]}" 2>/dev/null; wait "${pids[@]}" 2>/dev/null
  rm -rf "$S/$ds-4" "$S/$ds-4-"* "$S/$ds-6"
}
run_ds lubm100 "$T/lubm100.nt" bench/lubm docs/count-pushdown/queries/lubm
run_ds bsbm10 "$T/bsbm10.nt" bench/bsbm
run_ds watdiv100 "$T/watdiv100.nt" bench/watdiv docs/count-pushdown/queries/watdiv
echo "== summary"; grep -l "MISMATCH\|ERR\|ORDER?" "$OUT"/*.txt; grep -c "MISMATCH\|ERR " "$OUT"/*.txt
echo DONE
