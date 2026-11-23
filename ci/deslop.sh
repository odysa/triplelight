#!/usr/bin/env bash
# Structure gate (task #107): deslop-please, deep modules and narrow interfaces. `AGENTS.md` has the rule.
#   ci/deslop.sh                (needs deslop-please 0.1.3 on PATH and the rust-analyzer component)
# Fails on:
#   - any finding not in .deslop-please.allow;
#   - an allow file that still lists findings that are gone (fixing one means deleting its line, so the file only shrinks);
#   - a run that did not index the workspace. deslop-please 0.1.3 prints "rust-analyzer scip failed" and still exits 0,
#     and a clean run prints "deslop-please: ok", so the summary line is required.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT"
VERSION=0.1.3
# deslop-please has no --version flag: check what cargo installed.
cargo install --list | grep -q "^deslop-please v$VERSION:" || { echo "deslop-please $VERSION required: cargo install --locked deslop-please --version $VERSION" >&2; exit 2; }
out=$(mktemp); now=$(mktemp); trap 'rm -f "$out" "$now"' EXIT
deslop-please --allow .deslop-please.allow > "$out" 2>&1; rc=$?
if grep -q "scip failed" "$out"; then cat "$out"; echo "deslop: rust-analyzer indexing failed; the run checked nothing" >&2; exit 2; fi
grep -Eq '^deslop-please: ([0-9]+ errors?|ok)' "$out" || { cat "$out"; echo "deslop: no summary line; the run did not complete" >&2; exit 2; }
if [ $rc -ne 0 ]; then cat "$out"; echo "deslop: new structure findings (fix them from first principles, see AGENTS.md)" >&2; exit 1; fi
deslop-please --update-allow "$now" > /dev/null 2>&1
if ! diff <(grep -v '^#' .deslop-please.allow | sort) <(grep -v '^#' "$now" | sort) > /dev/null; then
  echo "deslop: .deslop-please.allow lists findings that no longer exist; delete these lines:" >&2
  comm -23 <(grep -v '^#' .deslop-please.allow | sort) <(grep -v '^#' "$now" | sort) >&2
  exit 1
fi
echo "deslop: ok ($(grep -vc '^#' .deslop-please.allow) known findings left)"
