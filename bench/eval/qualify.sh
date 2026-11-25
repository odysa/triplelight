#!/usr/bin/env bash
# The laptop qualification of the evaluation driver and system scripts (task #93). Correctness only, no timing claims.
#   qualify.sh <out-dir> <source-tools-dir>      Needs the machine lock held by the caller, Docker, Java, python3.
set -eu
OUT=$1; SRC=$2
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
mkdir -p "$OUT"; export TOOLS=$OUT/tools
{ echo "git $(git -C "$ROOT" rev-parse HEAD)"; date -u +%FT%TZ; uname -a; docker --version; java -version 2>&1 | head -1; python3 --version; } > "$OUT/environment.txt"
step() { n=$1; shift; echo "== $n"; ( "$@" ) > "$OUT/$n.log" 2>&1 && echo "$n exit 0" | tee -a "$OUT/results.txt" || { echo "$n exit $?" | tee -a "$OUT/results.txt"; true; }; }
step selftest python3 "$HERE/driver.py" selftest
step manifest python3 "$HERE/manifest.py" selftest
step drop-caches bash "$HERE/test-drop-caches.sh"
step session-status bash "$HERE/test-session-status.sh"
[ -n "${SKIP_SELFTEST_BIG:-}" ] || step selftest-big /usr/bin/time -l python3 "$HERE/driver.py" selftest-big --rows 5000000
step prep "$HERE/prep.sh" "$TOOLS" "$SRC"
# always build with the server feature: a plain release binary has no `serve` (found in the first qualification run)
step build-triplelight bash -c "cd $ROOT && cargo build --release --features server"
LUBM_Q=$ROOT/bench/lubm; BSBM_Q=$ROOT/bench/bsbm
step build-millenniumdb "$HERE/build-millenniumdb.sh" "$OUT/millenniumdb-src"
# jena-tdb2 first: it builds the frozen reference answers.
for pair in "lubm10:$LUBM_Q" "bsbm1:$BSBM_Q"; do
  D=${pair%%:*}; Q=${pair#*:}
  step "session-$D" env COLD=1 RUNS=3 TIMEOUT=300 BUDGET="--mem 12 --cpus 8" "$HERE/session.sh" "$D" "$Q" "$OUT/$D" jena-tdb2 triplelight oxigraph qlever qlever-two-order virtuoso rdf4j-native rdf4j-native-default millenniumdb
done
echo DONE >> "$OUT/results.txt"
