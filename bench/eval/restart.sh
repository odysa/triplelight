#!/usr/bin/env bash
# Cold-start hook for driver.py: stop the system, drop the page cache if possible, start it again and wait until it is ready.
# Environment: SYSTEM, DATASET, OUT, TOOLS and (optional) BUDGET="--mem 12 --cpus 8", set by session.sh.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib.sh"
# container id (Docker systems) or listener pid (native ones) before and after, so a restart can be proven (a Docker listener pid is the port forwarder's)
ident() { case $SYSTEM in qlever|qlever-two-order) docker inspect -f '{{.Id}}' bench-qlever 2>/dev/null;; virtuoso) docker inspect -f '{{.Id}}' bench-virtuoso 2>/dev/null;;
                          rdf4j-native|rdf4j-native-default) docker inspect -f '{{.Id}}' bench-rdf4j 2>/dev/null;; millenniumdb) docker inspect -f '{{.Id}}' bench-mdb 2>/dev/null;;
                          *) lsof -nP -iTCP:"$(port_of "$SYSTEM")" -sTCP:LISTEN 2>/dev/null | awk 'NR==2{print $2}';; esac | cut -c1-12; }
before=$(ident)
stop_system "$SYSTEM"
drop_caches
# shellcheck disable=SC2086
start_system "$SYSTEM" "$DATASET" ${BUDGET:-}
echo "restart: $SYSTEM id before=${before:-none} after=$(ident)"
