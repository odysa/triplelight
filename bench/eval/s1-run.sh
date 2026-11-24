#!/usr/bin/env bash
# S1 memory-limited runs for TripleLight (docs/eval-plan/PLAN.md "S1"; owner Alex): one session per memory limit, each
# with the cgroup sampler (s1-sample.sh) and the server's block-cache and phase traces, then the per-query reports.
#   s1-run.sh <dataset> <queries-dir> <out-dir> <limit-GB>...      e.g. s1-run.sh lubm8000 bench/lubm out/s1 8 16
# Per limit, <out-dir>/<limit>g/ holds the session files, triplelight.s1.json (per query: peak anon and file memory,
# device bytes read, major faults, OOM; s1-report.py) and triplelight.s1-cache.json (per query: block-cache hits,
# misses, hit rate, store bytes read; s1-cache.py). On Linux the server runs in a systemd scope with MemoryMax=<limit>G
# and no swap (lib.sh start_system); elsewhere (macOS) the limit is not enforced and s1-sample.sh records why, so a laptop
# run checks the layout only. Environment as for session.sh (TOOLS, RUNS, TIMEOUT); CPUS sets --cpus (default 8).
# The machine lock is the caller's business. No timing claims are made here: the times stay in the driver's JSON.
set -eu
D=$1; QDIR=$2; OUT=$3; shift 3
[ $# -gt 0 ] || { echo "usage: s1-run.sh <dataset> <queries-dir> <out-dir> <limit-GB>..." >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
for g in "$@"; do
  o="$OUT/${g}g"; mkdir -p "$o"
  # the reference answers come from Jena; reuse one if the caller put it in place, else the session needs jena-tdb2
  [ -f "$OUT/reference.json" ] && cp "$OUT/reference.json" "$o/reference.json"
  echo "== S1 $D at ${g} GB -> $o"
  S1_SAMPLE=1 TRIPLELIGHT_TRACE_BLOCK_CACHE=1 TRIPLELIGHT_TRACE_PHASES=1 BUDGET="--mem $g --cpus ${CPUS:-8}" \
    "$HERE/session.sh" "$D" "$QDIR" "$o" triplelight
  if [ -f "$o/triplelight.warm.json" ]; then
    python3 -I "$HERE/s1-report.py" "$o/triplelight.warm.json" "$o/triplelight.s1.tsv" > "$o/triplelight.s1.json" || echo "s1-report failed for ${g} GB" >> "$o/failures.txt"
    python3 -I "$HERE/s1-cache.py" "$QDIR" "$o/triplelight.warm.json" "$o/triplelight.server.log" > "$o/triplelight.s1-cache.json"
    python3 -I -c "import json,sys; r=json.load(open(sys.argv[1])); print('  block-cache join:', 'ok' if r['joined'] else 'NOT joined: ' + r.get('why', ''))" "$o/triplelight.s1-cache.json"
  fi
done
