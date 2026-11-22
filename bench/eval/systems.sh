#!/usr/bin/env bash
# Load or serve one system on one dataset, with the same CPU and memory budget for every system (docs/eval-plan/PLAN.md).
#   systems.sh <system> load|serve <dataset> [--mem GB] [--cpus N]
#   system:  triplelight | qlever | virtuoso | jena-tdb2 | oxigraph
#   dataset: tools/<dataset>.nt must exist (TOOLS=... to use another directory); stores are tools/<system>-<dataset>.
# `serve` runs in the foreground and prints nothing but the server's own log; the SPARQL endpoint of each system:
#   triplelight http://127.0.0.1:7979/sparql   qlever http://127.0.0.1:7001/   virtuoso http://127.0.0.1:8890/sparql
#   jena-tdb2 http://127.0.0.1:3030/ds/sparql  oxigraph http://127.0.0.1:7878/query
# Budget: Docker systems get --memory and --cpus (cgroup limits); the native ones get the JVM heap (Jena) or nothing, and the
# script prints "budget not enforced" for them; on the Linux machine the whole process is started under a cgroup instead.
set -eu
SYS=$1; MODE=$2; D=$3; shift 3
MEM=12; CPUS=8
while [ $# -gt 0 ]; do case $1 in --mem) MEM=$2; shift 2;; --cpus) CPUS=$2; shift 2;; *) echo "unknown option $1" >&2; exit 2;; esac; done
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
. "$HERE/VERSIONS"
T=${TOOLS:-$ROOT/bench/tools}; NT=$T/$D.nt
[ -f "$NT" ] || { echo "missing $NT" >&2; exit 1; }
LIM=(--memory="${MEM}g" --cpus="$CPUS")
case $SYS in
  triplelight)
    BIN=${TRIPLELIGHT_BIN:-$ROOT/target/release/triplelight}
    case $MODE in
      load)  echo "budget not enforced for native triplelight" >&2
             rm -rf "$T/triplelight-$D"; "$BIN" build-disk "$NT" "$T/triplelight-$D" --build-external ;;
      serve) echo "budget not enforced for native triplelight" >&2
             # fail fast if this binary was built without the server feature (a later plain `cargo test` or `cargo build` rebuilds it that way)
             if "$BIN" serve /nonexistent 0 2>&1 | grep -q "does not include"; then echo "$BIN has no server feature: cargo build --release --features server" >&2; exit 3; fi
             # cache budget: half of the memory limit, as in the frozen plan (S1), recorded in the receipt
             exec "$BIN" serve "$T/triplelight-$D" 7979 --disk --cache-mb "$((MEM * 1024 / 2))" ;;
    esac ;;
  qlever)
    case $MODE in
      load)  mkdir -p "$T/qlever-$D"
             docker run --rm "${LIM[@]}" -v "$T":/data -w /data/qlever-$D --entrypoint /qlever/qlever-index "$QLEVER_IMAGE" \
               -i "$D" -f "/data/$D.nt" -F nt -p true ;;
      # Result cache off (-c/-e/-E 1MB) so repeated runs measure execution, not cache hits.
      serve) docker rm -f bench-qlever >/dev/null 2>&1 || true
             exec docker run --rm --name bench-qlever "${LIM[@]}" -p 7001:7001 -v "$T":/data -w /data/qlever-$D \
               --entrypoint /qlever/qlever-server "$QLEVER_IMAGE" -i "$D" -p 7001 -m "${MEM}G" -c 1MB -e 1MB -E 1MB ;;
    esac ;;
  virtuoso)
    DIR=$T/virtuoso-$D; mkdir -p "$DIR"
    NB=$((MEM * 1024 * 1024 / 8 * 85 / 100))   # 8 KB buffers, 85% of the budget (Virtuoso's own sizing advice)
    ENV=(-e DBA_PASSWORD=dba -e VIRT_Parameters_NumberOfBuffers=$NB -e VIRT_Parameters_MaxDirtyBuffers=$((NB * 3 / 4))
         -e VIRT_Parameters_ThreadsPerQuery="$CPUS" -e VIRT_Parameters_DirsAllowed=.,/import,/usr/share/proj
         -e VIRT_SPARQL_ResultSetMaxRows=1000000000 -e VIRT_SPARQL_MaxQueryExecutionTime=0)
    case $MODE in
      serve) docker rm -f bench-virtuoso >/dev/null 2>&1 || true
             exec docker run --rm --name bench-virtuoso "${LIM[@]}" -p 8890:8890 "${ENV[@]}" -v "$DIR":/database -v "$T":/import "$VIRTUOSO_IMAGE" ;;
      load)  docker rm -f bench-virtuoso-load >/dev/null 2>&1 || true
             docker run -d --rm --name bench-virtuoso-load "${LIM[@]}" "${ENV[@]}" -v "$DIR":/database -v "$T":/import "$VIRTUOSO_IMAGE" >/dev/null
             until docker exec bench-virtuoso-load isql 1111 dba dba exec="status();" >/dev/null 2>&1; do sleep 2; done
             docker exec bench-virtuoso-load isql 1111 dba dba exec="ld_dir('/import', '$D.nt', 'http://bench/$D');" >/dev/null
             for _ in $(seq 1 "$CPUS"); do docker exec bench-virtuoso-load isql 1111 dba dba exec="rdf_loader_run();" >/dev/null & done; wait
             docker exec bench-virtuoso-load isql 1111 dba dba exec="checkpoint;" >/dev/null
             docker stop bench-virtuoso-load >/dev/null ;;
    esac ;;
  jena-tdb2)
    J=$T/apache-jena-$JENA_VERSION; F=$T/apache-jena-fuseki-$FUSEKI_VERSION
    case $MODE in
      load)  echo "budget: JVM heap only (-Xmx${MEM}g); CPUs not enforced" >&2
             rm -rf "$T/jena-tdb2-$D"; JVM_ARGS="-Xmx${MEM}g" "$J/bin/tdb2.tdbloader" --loc "$T/jena-tdb2-$D" "$NT" ;;
      serve) echo "budget: JVM heap only (-Xmx${MEM}g); CPUs not enforced" >&2
             JVM_ARGS="-Xmx${MEM}g" exec "$F/fuseki-server" --loc="$T/jena-tdb2-$D" --port=3030 /ds ;;
    esac ;;
  oxigraph)
    O=$T/oxigraph
    [ "$("$O" --version | awk '{print $2}')" = "$OXIGRAPH_VERSION" ] || { echo "oxigraph version is not $OXIGRAPH_VERSION" >&2; exit 1; }
    case $MODE in
      load)  echo "budget not enforced for native oxigraph" >&2
             rm -rf "$T/oxigraph-$D"; "$O" load --location "$T/oxigraph-$D" --file "$NT" ;;
      serve) echo "budget not enforced for native oxigraph" >&2
             exec "$O" serve-read-only --location "$T/oxigraph-$D" --bind 127.0.0.1:7878 ;;
    esac ;;
  *) echo "unknown system $SYS" >&2; exit 2 ;;
esac
