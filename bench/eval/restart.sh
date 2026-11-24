#!/usr/bin/env bash
# Cold-start hook for driver.py: stop the system, drop the page cache if possible, start it again and wait until it is ready.
#   restart.sh         restart after a timed-out query (warm mode): a page cache that cannot be dropped is reported, not fatal
#   restart.sh --cold  the cold-start hook: a page cache that cannot be dropped fails the hook (the driver then times nothing);
#                      ALLOW_WARM_COLD=1 turns that off for exploration on a machine without the permission
# Environment: SYSTEM, DATASET, OUT, TOOLS and (optional) BUDGET="--mem 12 --cpus 8", set by session.sh.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib.sh"
# container id (Docker systems) or listener pid (native ones) before and after, so a restart can be proven (a Docker listener pid is the port forwarder's)
ident() { case $SYSTEM in qlever|qlever-two-order) docker inspect -f '{{.Id}}' bench-qlever 2>/dev/null;; virtuoso) docker inspect -f '{{.Id}}' bench-virtuoso 2>/dev/null;;
                          rdf4j-native|rdf4j-native-default) docker inspect -f '{{.Id}}' bench-rdf4j 2>/dev/null;; millenniumdb) docker inspect -f '{{.Id}}' bench-mdb 2>/dev/null;;
                          *) lsof -nP -iTCP:"$(port_of "$SYSTEM")" -sTCP:LISTEN 2>/dev/null | awk 'NR==2{print $2}';; esac | cut -c1-12; }
[ "${1:-}" = --cold ] && [ "${ALLOW_WARM_COLD:-0}" != 1 ] && export REQUIRE_CACHE_DROP=1
before=$(ident)
stop_system "$SYSTEM"
drop_caches || { echo "restart: the page cache could not be dropped; a cold measurement would time a warm query" >&2; exit 1; }
# shellcheck disable=SC2086
start_system "$SYSTEM" "$DATASET" ${BUDGET:-}
echo "restart: $SYSTEM id before=${before:-none} after=$(ident)"
