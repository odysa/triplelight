#!/usr/bin/env bash
# One evaluation session: for each system, one at a time, load if needed, serve, run the driver warm (and cold), stop.
#   session.sh <dataset> <queries-dir> <out-dir> <system>...        (the first system builds the reference answers if absent)
# Environment: EXPECT_UNSUPPORTED_<SYSTEM> (case IDs that system's configuration is known not to answer), TOOLS (default bench/tools), BUDGET ("--mem GB --cpus N"), COLD=1 to also run cold mode, RUNS (timed runs, 5),
#              TIMEOUT (seconds per query, 300). The machine lock is the caller's business (this script does not take it).
set -eu
D=$1; QDIR=$2; OUT=$3; shift 3
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
export TOOLS=${TOOLS:-$ROOT/bench/tools} OUT HERE
mkdir -p "$OUT"; . "$HERE/lib.sh"
BUDGET=${BUDGET:-}; RUNS=${RUNS:-5}; TIMEOUT=${TIMEOUT:-300}
trap 'for s in "$@"; do stop_system "$s" || true; done' EXIT
# A failing system must not stop the others: every failure is recorded in $OUT/failures.txt and the session goes on.
OK=()
for s in "$@"; do
  if [ ! -d "$TOOLS/$s-$D" ]; then
    echo "== load $s $D"; start=$(date +%s)
    # shellcheck disable=SC2086
    if ! /usr/bin/time -p "$HERE/systems.sh" "$s" load "$D" $BUDGET > "$OUT/$s.load.log" 2>&1; then
      echo "$s: load failed (see $s.load.log)" >> "$OUT/failures.txt"; continue
    fi
    echo "load wall $(( $(date +%s) - start )) s" >> "$OUT/$s.load.log"
  fi
  OK+=("$s")
done
for s in "${OK[@]}"; do
  echo "== $s"
  # Cases this system configuration is known not to answer, from EXPECT_UNSUPPORTED_<SYSTEM> (upper case, - as _); never shared across systems.
  var=EXPECT_UNSUPPORTED_$(echo "$s" | tr 'a-z-' 'A-Z_'); EXPECT=${!var:-}
  CLEAR=$(clear_cache_url_of "$s")   # QLever only: its result cache is emptied before every request (see lib.sh)
  # shellcheck disable=SC2086
  if ! start_system "$s" "$D" $BUDGET; then echo "$s: start failed (see $s.server.log)" >> "$OUT/failures.txt"; stop_system "$s" || true; continue; fi
  # S1: sample the server's cgroup (memory, page cache, device reads, major faults) for the whole measurement.
  SAMPLER=
  if [ "${S1_SAMPLE:-0}" = 1 ]; then "$HERE/s1-sample.sh" "$s" "$OUT/$s.s1.tsv" & SAMPLER=$!; fi
  if [ ! -f "$OUT/reference.json" ]; then
    [ "$s" = jena-tdb2 ] || { echo "the reference answers come from jena-tdb2: list it first" >&2; exit 2; }
    python3 "$HERE/driver.py" reference --queries "$QDIR" --url "$(url_of "$s")" --out "$OUT/reference.json" --timeout "$TIMEOUT" > "$OUT/reference.log"
    echo "reference answers from $s" >> "$OUT/reference.log"
  fi
  if ! SYSTEM=$s DATASET=$D BUDGET=$BUDGET python3 "$HERE/driver.py" run --system "$s" --url "$(url_of "$s")" ${CLEAR:+--clear-cache-url "$CLEAR"} ${EXPECT:+--expect-unsupported "$EXPECT"} --queries "$QDIR" --reference "$OUT/reference.json" \
      --out "$OUT/$s.warm.json" --mode warm --runs "$RUNS" --timeout "$TIMEOUT" --restart-hook "$HERE/restart.sh" > "$OUT/$s.warm.log" 2>&1; then
    echo "$s: warm run failed (see $s.warm.log)" >> "$OUT/failures.txt"
  fi
  if [ "${COLD:-0}" = 1 ]; then
    if ! SYSTEM=$s DATASET=$D BUDGET=$BUDGET python3 "$HERE/driver.py" run --system "$s" --url "$(url_of "$s")" ${CLEAR:+--clear-cache-url "$CLEAR"} ${EXPECT:+--expect-unsupported "$EXPECT"} --queries "$QDIR" \
      --reference "$OUT/reference.json" --out "$OUT/$s.cold.json" --mode cold --timeout "$TIMEOUT" --cold-hook "'$HERE/restart.sh' --cold" > "$OUT/$s.cold.log" 2>&1; then
      echo "$s: cold run failed (see $s.cold.log)" >> "$OUT/failures.txt"
    fi
  fi
  [ -n "$SAMPLER" ] && { kill "$SAMPLER" 2>/dev/null || true; wait "$SAMPLER" 2>/dev/null || true; }
  stop_system "$s"
done
echo "session done: $OUT"
# Every engine got its turn, but a failed load, start or run, or a run that was ineligible for a comparison, fails the session.
if [ -s "$OUT/failures.txt" ]; then echo "session finished with failures, see $OUT/failures.txt" >&2; exit 1; fi
