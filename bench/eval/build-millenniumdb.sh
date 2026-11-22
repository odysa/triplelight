#!/usr/bin/env bash
# Build the MillenniumDB Docker image at the commit pinned in VERSIONS (there is no published image). UNTESTED.
#   build-millenniumdb.sh [work-dir]
set -eu
HERE=$(cd "$(dirname "$0")" && pwd); . "$HERE/VERSIONS"
W=${1:-${TMPDIR:-/tmp}/millenniumdb-src}
[ -d "$W/.git" ] || git clone -q https://github.com/MillenniumDB/MillenniumDB "$W"
git -C "$W" checkout -q "$MILLENNIUMDB_COMMIT"
docker build -t "mdb:$MILLENNIUMDB_COMMIT" "$W"
docker image inspect "mdb:$MILLENNIUMDB_COMMIT" --format '{{.Id}}'   # record this image id in the receipt
