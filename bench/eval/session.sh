#!/usr/bin/env bash
# One evaluation session: for each system, one at a time, load if needed, serve, run the driver warm (and cold), stop.
#   session.sh <dataset> <queries-dir> <out-dir> <system>...        (the first system builds the reference answers if absent)
# Environment: TOOLS (default bench/tools), BUDGET ("--mem GB --cpus N"), COLD=1 to also run cold mode, RUNS (timed runs, 5),
#              TIMEOUT (seconds per query, 300). The machine lock is the caller's business (this script does not take it).
set -eu
D=$1; QDIR=$2; OUT=$3; shift 3
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
export TOOLS=${TOOLS:-$ROOT/bench/tools} OUT HERE
mkdir -p "$OUT"; . "$HERE/lib.sh"
BUDGET=${BUDGET:-}; RUNS=${RUNS:-5}; TIMEOUT=${TIMEOUT:-300}
trap 'for s in "$@"; do stop_system "$s" || true; done' EXIT
for s in "$@"; do
  if [ ! -d "$TOOLS/$s-$D" ]; then
    echo "== load $s $D"; start=$(date +%s)
    # shellcheck disable=SC2086
    /usr/bin/time -p "$HERE/systems.sh" "$s" load "$D" $BUDGET > "$OUT/$s.load.log" 2>&1
    echo "load wall $(( $(date +%s) - start )) s" >> "$OUT/$s.load.log"
  fi
done
for s in "$@"; do
  echo "== $s"
  # shellcheck disable=SC2086
  start_system "$s" "$D" $BUDGET
  if [ ! -f "$OUT/reference.json" ]; then
    [ "$s" = jena-tdb2 ] || { echo "the reference answers come from jena-tdb2: list it first" >&2; exit 2; }
    python3 "$HERE/driver.py" reference --queries "$QDIR" --url "$(url_of "$s")" --out "$OUT/reference.json" --timeout "$TIMEOUT" > "$OUT/reference.log"
    echo "reference answers from $s" >> "$OUT/reference.log"
  fi
  python3 "$HERE/driver.py" run --system "$s" --url "$(url_of "$s")" --queries "$QDIR" --reference "$OUT/reference.json" \
      --out "$OUT/$s.warm.json" --mode warm --runs "$RUNS" --timeout "$TIMEOUT" > "$OUT/$s.warm.log"
  if [ "${COLD:-0}" = 1 ]; then
    SYSTEM=$s DATASET=$D BUDGET=$BUDGET python3 "$HERE/driver.py" run --system "$s" --url "$(url_of "$s")" --queries "$QDIR" \
      --reference "$OUT/reference.json" --out "$OUT/$s.cold.json" --mode cold --timeout "$TIMEOUT" --cold-hook "$HERE/restart.sh" > "$OUT/$s.cold.log"
  fi
  stop_system "$s"
done
echo "session done: $OUT"
