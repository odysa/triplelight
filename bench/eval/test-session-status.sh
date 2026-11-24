#!/usr/bin/env bash
# session.sh keeps going when a system fails, but the session's exit status must say that something failed.
set -u
HERE=$(cd "$(dirname "$0")" && pwd); T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
mkdir -p "$T/q" "$T/tools"
echo "SELECT * WHERE {}" > "$T/q/q1.rq"
TOOLS="$T/tools" "$HERE/session.sh" nosuchdataset "$T/q" "$T/out" nosuchsystem > "$T/log" 2>&1; rc=$?
if [ "$rc" -ne 0 ] && grep -q "nosuchsystem" "$T/out/failures.txt" 2>/dev/null; then echo "ok   a failed system fails the session (exit $rc) and is listed in failures.txt"; exit 0; fi
echo "FAIL the session exited $rc, failures.txt: $(cat "$T/out/failures.txt" 2>/dev/null)"; sed -n 1,20p "$T/log"; exit 1
