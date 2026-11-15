#!/bin/bash
# Virtuoso 7 (Homebrew) for the benchmark: per-dataset database under tools/virtuoso-<D>.
# usage: virtuoso.sh <dataset> load|serve      (serve runs in the foreground; SPARQL at http://127.0.0.1:8890/sparql)
set -eu
D=$1; MODE=$2
cd "$(dirname "$0")"
DIR=$PWD/tools/virtuoso-$D; INI=$DIR/virtuoso.ini; SRC=/opt/homebrew/Cellar/virtuoso/7.2.17/var/lib/virtuoso/db/virtuoso.ini
mkdir -p $DIR
if [ ! -f $INI ]; then
  # 64 GB machine: buffers per Virtuoso's sizing table (~10 GB of 8 KB pages); result limits off so answers are complete.
  sed -e "s|/opt/homebrew/Cellar/virtuoso/7.2.17/var/lib/virtuoso/db/|$DIR/|g" \
      -e "s|^DirsAllowed.*|DirsAllowed = ., $PWD/tools, /opt/homebrew/Cellar/virtuoso/7.2.17/share/virtuoso/vad|" \
      -e "s|^NumberOfBuffers.*|NumberOfBuffers = 1360000|" -e "s|^MaxDirtyBuffers.*|MaxDirtyBuffers = 1000000|" \
      -e "s|^ThreadsPerQuery.*|ThreadsPerQuery = 10|" \
      -e "s|^;ResultSetMaxRows.*|ResultSetMaxRows = 1000000000|" -e "s|^;MaxQueryExecutionTime.*|MaxQueryExecutionTime = 0|" \
      $SRC > $INI
fi
case $MODE in
  serve) cd $DIR && exec virtuoso-t -f -c $INI ;;
  load)
    cd $DIR && virtuoso-t -c $INI && sleep 5
    until isql 1111 dba dba exec="status();" >/dev/null 2>&1; do sleep 1; done
    start=$(date +%s)
    isql 1111 dba dba exec="ld_dir('$PWD/../tools', '$D.nt', 'http://bench/$D');" >/dev/null
    for i in 1 2 3 4 5 6 7 8; do isql 1111 dba dba exec="rdf_loader_run();" >/dev/null & done; wait
    isql 1111 dba dba exec="checkpoint;" >/dev/null
    echo "real $(( $(date +%s) - start ))"
    isql 1111 dba dba exec="shutdown;" >/dev/null 2>&1 || true ;;
esac
