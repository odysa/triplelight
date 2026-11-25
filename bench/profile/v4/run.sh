#!/bin/bash
# Profile v4 from one command: bench/profile/v4/run.sh OUT_DIR [make-plans.py options]
# Writes the plans for this machine, then runs each through bench/eval/plan.py (which takes the machine lock,
# requires a fresh machine-check receipt, scores every session and writes report.md per plan), then the scoreboard.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../../.." && pwd)
OUT=${1:?usage: run.sh OUT_DIR [make-plans options]}; shift
[ ! -e "$OUT" ] || [ -z "$(ls -A "$OUT")" ] || { echo "run.sh: $OUT is not empty" >&2; exit 2; }
mkdir -p "$OUT/plans"
python3 "$HERE/make-plans.py" "$OUT/plans" "$@"
rc=0
for p in "$OUT"/plans/*.json; do
  s=$(basename "$p" .json)
  python3 "$ROOT/bench/eval/plan.py" run "$p" --out "$OUT/$s" || { echo "plan $s failed (rc $?)" | tee -a "$OUT/failures.txt"; rc=1; }
done
exit $rc
