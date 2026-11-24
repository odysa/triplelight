#!/usr/bin/env bash
# SHACL Core conformance gate (task #108): runs the W3C SHACL Core test suite through `triplelight::shacl::validate` and checks the result.
#   ci/shacl-core.sh                       fetches the pinned suite revision, runs `cargo test --release --test shacl`, checks the summary
#   SHACL_TESTS=<dir> ci/shacl-core.sh     use an existing checkout's `data-shapes-test-suite/tests/core` (its revision is not checked)
#   ci/shacl-core.sh --check-log FILE      only check the summary of a finished run's output (used to test this script)
# Fails on:
#   - a run whose output has no `SHACL Core: N tests, ...` summary line (the suite did not run);
#   - a suite that is not the pinned size (a wrong directory or a truncated checkout), so the count cannot shrink unnoticed;
#   - any unexpected failure, and any excluded case that now passes (the runner prints it): `tests/shacl/w3c-exclusions.txt` only shrinks.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
PIN=eacdef4528d66ee116f0ac9118d968c5717c7233      # w3c/data-shapes
EXPECTED_TESTS=98                                  # sht:Validate cases under data-shapes-test-suite/tests/core at that revision

check_log() {
  local log=$1 line n unexpected
  line=$(grep -E '^SHACL Core: [0-9]+ tests' "$log" | tail -1)
  [ -n "$line" ] || { echo "shacl-core: no summary line; the suite did not run" >&2; return 2; }
  n=$(sed -E 's/^SHACL Core: ([0-9]+) tests.*/\1/' <<< "$line")
  unexpected=$(sed -E 's/.* ([0-9]+) unexpected failures.*/\1/' <<< "$line")
  if [ "$n" -ne "$EXPECTED_TESTS" ]; then echo "shacl-core: $n tests, expected $EXPECTED_TESTS at $PIN (wrong directory or truncated suite)" >&2; return 1; fi
  if [ "$unexpected" -ne 0 ] || grep -q '^NOW PASSES' "$log"; then
    grep -E '^(FAIL|NOW PASSES)' "$log" >&2; echo "shacl-core: unexpected failures or excluded cases that now pass" >&2; return 1
  fi
  echo "shacl-core: ok ($line)"
}

if [ "${1:-}" = "--check-log" ]; then check_log "$2"; exit $?; fi

if [ -z "${SHACL_TESTS:-}" ]; then
  dir="${TMPDIR:-/tmp}/tl-data-shapes-${PIN:0:8}"
  if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" != "$PIN" ]; then
    rm -rf "$dir"; git init -q "$dir"
    git -C "$dir" remote add origin https://github.com/w3c/data-shapes.git
    git -C "$dir" sparse-checkout set data-shapes-test-suite/tests/core
    git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$PIN" || { echo "shacl-core: cannot fetch $PIN" >&2; exit 2; }
    git -C "$dir" checkout -q --detach FETCH_HEAD
  fi
  [ "$(git -C "$dir" rev-parse HEAD)" = "$PIN" ] || { echo "shacl-core: the checkout is not $PIN" >&2; exit 2; }
  SHACL_TESTS="$dir/data-shapes-test-suite/tests/core"
fi
export TL_SHACL_TESTS="$SHACL_TESTS"
log=$(mktemp); trap 'rm -f "$log"' EXIT
CARGO_TERM_COLOR=never cargo test --locked --release --test shacl -- --ignored --nocapture > "$log" 2>&1; rc=$?
cat "$log"
[ $rc -eq 0 ] || { echo "shacl-core: cargo test failed (exit $rc)" >&2; exit 1; }
check_log "$log"
