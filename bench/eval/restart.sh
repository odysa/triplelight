#!/usr/bin/env bash
# Cold-start hook for driver.py: stop the system, drop the page cache if possible, start it again and wait until it is ready.
# Environment: SYSTEM, DATASET, OUT, TOOLS and (optional) BUDGET="--mem 12 --cpus 8", set by session.sh.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib.sh"
stop_system "$SYSTEM"
drop_caches
# shellcheck disable=SC2086
start_system "$SYSTEM" "$DATASET" ${BUDGET:-}
