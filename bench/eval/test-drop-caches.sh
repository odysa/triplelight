#!/usr/bin/env bash
# The cold-start hook must fail when the page cache cannot be dropped, and the warm-timeout restart must not (lib.sh drop_caches).
# Skipped where the cache can really be dropped: the point is the no-permission path.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -w /proc/sys/vm/drop_caches ]; then echo "drop_caches test skipped: this machine can drop the page cache"; exit 0; fi
fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got $2, want $3)"; fail=1; fi; }
# sudo is stubbed to refuse, so a machine with passwordless `purge` is tested the same way
run() ( sudo() { return 1; }; . "$HERE/lib.sh"; "$@" >/dev/null 2>&1; echo $? )
check "warm path: a page cache that cannot be dropped is not fatal" "$(run drop_caches)" 0
check "cold path: REQUIRE_CACHE_DROP=1 makes it fatal" "$(REQUIRE_CACHE_DROP=1 run drop_caches)" 1
exit $fail
