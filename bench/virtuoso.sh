#!/bin/bash
# Virtuoso 7 open source (official Docker image openlink/virtuoso-opensource-7), per-dataset database under
# tools/virtuoso-<D>. (The Homebrew build's /sparql endpoint answers 406, so the Docker image is used.)
# usage: virtuoso.sh <dataset> load|serve      (serve runs in the foreground; SPARQL at http://127.0.0.1:8890/sparql)
set -eu
D=$1; MODE=$2
cd "$(dirname "$0")"
TOOLS=$PWD/tools; DIR=$TOOLS/virtuoso-$D; mkdir -p $DIR
# Sizing for a ~16 GB VM per Virtuoso's table (~1.36M 8 KB buffers ≈ 10.6 GB); no result-row cap so answers are complete.
ENV=(-e DBA_PASSWORD=dba -e VIRT_Parameters_NumberOfBuffers=1360000 -e VIRT_Parameters_MaxDirtyBuffers=1000000
     -e VIRT_Parameters_ThreadsPerQuery=10 -e VIRT_Parameters_DirsAllowed=.,/import,/usr/share/proj
     -e VIRT_SPARQL_ResultSetMaxRows=1000000000 -e VIRT_SPARQL_MaxQueryExecutionTime=0)
IMG=openlink/virtuoso-opensource-7:latest
case $MODE in
  serve) exec docker run --rm --name bench-virtuoso -p 8890:8890 "${ENV[@]}" -v $DIR:/database -v $TOOLS:/import $IMG ;;
  load)
    docker rm -f bench-virtuoso-load >/dev/null 2>&1 || true
    docker run -d --rm --name bench-virtuoso-load "${ENV[@]}" -v $DIR:/database -v $TOOLS:/import $IMG >/dev/null
    until docker exec bench-virtuoso-load isql 1111 dba dba exec="status();" >/dev/null 2>&1; do sleep 2; done
    start=$(date +%s)
    docker exec bench-virtuoso-load isql 1111 dba dba exec="ld_dir('/import', '$D.nt', 'http://bench/$D');" >/dev/null
    for i in 1 2 3 4 5 6 7 8; do docker exec bench-virtuoso-load isql 1111 dba dba exec="rdf_loader_run();" >/dev/null & done; wait
    docker exec bench-virtuoso-load isql 1111 dba dba exec="checkpoint;" >/dev/null
    echo "real $(( $(date +%s) - start ))"
    docker stop bench-virtuoso-load >/dev/null ;;
esac
