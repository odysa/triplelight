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

SUMMARY='^SHACL Core: ([0-9]+) tests, ([0-9]+) passed, ([0-9]+) excluded, ([0-9]+) unexpected failures$'
check_log() {
  local log=$1 line n passed excluded unexpected
  line=$(grep -E '^SHACL Core:' "$log" | tail -1)
  [ -n "$line" ] || { echo "shacl-core: no summary line; the suite did not run" >&2; return 2; }
  # the whole line must have the runner's exact shape before any field is read
  [[ "$line" =~ $SUMMARY ]] || { echo "shacl-core: malformed summary line: $line" >&2; return 2; }
  n=${BASH_REMATCH[1]}; passed=${BASH_REMATCH[2]}; excluded=${BASH_REMATCH[3]}; unexpected=${BASH_REMATCH[4]}
  if [ "$n" -ne "$EXPECTED_TESTS" ]; then echo "shacl-core: $n tests, expected $EXPECTED_TESTS at $PIN (wrong directory or truncated suite)" >&2; return 1; fi
  if [ "$unexpected" -ne 0 ] || grep -q '^NOW PASSES' "$log"; then
    grep -E '^(FAIL|NOW PASSES)' "$log" >&2; echo "shacl-core: unexpected failures or excluded cases that now pass" >&2; return 1
  fi
  # every case either passed or is a listed exclusion that still fails (a stale exclusion entry breaks this)
  if [ "$passed" -ne $((n - excluded)) ]; then echo "shacl-core: $passed passed + $excluded excluded does not add up to $n: $line" >&2; return 1; fi
  echo "shacl-core: ok ($line)"
}

if [ "${1:-}" = "--check-log" ]; then check_log "$2"; exit $?; fi

dir=
if [ -z "${SHACL_TESTS:-}" ]; then
  # a fresh private checkout every time, removed on exit: another invocation's checkout is never touched
  dir=$(mktemp -d "${TMPDIR:-/tmp}/tl-data-shapes.XXXXXX") || { echo "shacl-core: cannot create a temporary directory" >&2; exit 2; }
  trap 'rm -rf "$dir"' EXIT
  git init -q "$dir"
  git -C "$dir" remote add origin https://github.com/w3c/data-shapes.git
  git -C "$dir" sparse-checkout set data-shapes-test-suite/tests/core
  git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$PIN" || { echo "shacl-core: cannot fetch $PIN" >&2; exit 2; }
  git -C "$dir" checkout -q --detach FETCH_HEAD
  [ "$(git -C "$dir" rev-parse HEAD)" = "$PIN" ] || { echo "shacl-core: the checkout is not $PIN" >&2; exit 2; }
  SHACL_TESTS="$dir/data-shapes-test-suite/tests/core"
fi
export TL_SHACL_TESTS="$SHACL_TESTS"
log=$(mktemp) || { echo "shacl-core: cannot create a log file" >&2; exit 2; }
trap 'rm -f "$log"; [ -z "${dir:-}" ] || rm -rf "$dir"' EXIT
CARGO_TERM_COLOR=never cargo test --locked --release --test shacl -- --ignored --nocapture > "$log" 2>&1; rc=$?
cat "$log"
[ $rc -eq 0 ] || { echo "shacl-core: cargo test failed (exit $rc)" >&2; exit 1; }
check_log "$log"
