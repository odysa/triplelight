#!/usr/bin/env bash
# Load or serve one system on one dataset, with the same CPU and memory budget for every system (docs/eval-plan/PLAN.md).
#   systems.sh <system> load|serve <dataset> [--mem GB] [--cpus N]
#   system:  triplelight | qlever | qlever-two-order | virtuoso | jena-tdb2 | oxigraph | rdf4j-native | rdf4j-native-default | millenniumdb
#   dataset: tools/<dataset>.nt must exist (TOOLS=... to use another directory); stores are tools/<system>-<dataset>.
# `serve` runs in the foreground and prints nothing but the server's own log; the SPARQL endpoint of each system:
#   triplelight http://127.0.0.1:7979/sparql   qlever http://127.0.0.1:7001/   virtuoso http://127.0.0.1:8890/sparql
#   jena-tdb2 http://127.0.0.1:3030/ds/sparql  oxigraph http://127.0.0.1:7878/query
#   millenniumdb http://127.0.0.1:1234/sparql   rdf4j-native* http://127.0.0.1:3040/rdf4j-server/repositories/bench  (RDF4J's own server; indexes spoc,posc,ospc, or spoc,posc for -default)
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
# --memory-swap equal to --memory: no swap, the same as the native servers' systemd scope (MemorySwapMax=0)
LIM=(--memory="${MEM}g" --memory-swap="${MEM}g" --cpus="$CPUS")
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
  qlever|qlever-two-order)
    QDIR=qlever-$D; QOPT=()
    # the two-order arm builds only PSO and POS (qlever-index -o); queries with a variable predicate are then unsupported
    [ "$SYS" = qlever-two-order ] && { QDIR=qlever-two-order-$D; QOPT=(-o --no-patterns); }   # the has-predicate patterns need all six permutations (qlever-index says so)
    case $MODE in
      load)  mkdir -p "$T/$QDIR"
             docker run --rm "${LIM[@]}" -v "$T":/data -w /data/$QDIR --entrypoint /qlever/qlever-index "$QLEVER_IMAGE" \
               -i "$D" -f "/data/$D.nt" -F nt -p true "${QOPT[@]}" ;;
      # Result cache off (-c/-e/-E 1MB) so repeated runs measure execution, not cache hits.
      serve) docker rm -f bench-qlever >/dev/null 2>&1 || true
             exec docker run --rm --name bench-qlever "${LIM[@]}" -p 7001:7001 -v "$T":/data -w /data/$QDIR \
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
             rm -rf "$T/jena-tdb2-$D"; JVM_ARGS="-Xmx${MEM}g" "$J/bin/tdb2.tdbloader" --loc "$T/jena-tdb2-$D" "$NT"
             # optimizer statistics (tdb2.tdbstats): without them TDB2 plans with fixed heuristics. UNTESTED: the target path
             # (Data-0001/stats.opt) and the tool name must be checked in the qualification rerun
             "$J/bin/tdb2.tdbstats" --loc "$T/jena-tdb2-$D" > "$T/jena-tdb2-$D/stats.tmp" && mv "$T/jena-tdb2-$D/stats.tmp" "$T/jena-tdb2-$D/Data-0001/stats.opt"   # write to a temp file: a redirect straight into stats.opt makes tdbstats itself log "Empty stats file"
             # the first rerun found this file empty (Jena logged "Empty stats file"): fail loudly instead of benchmarking without statistics
             [ -s "$T/jena-tdb2-$D/Data-0001/stats.opt" ] || { echo "tdb2.tdbstats wrote no statistics" >&2; exit 1; } ;;
      serve) echo "budget: JVM heap only (-Xmx${MEM}g); CPUs not enforced" >&2
             JVM_ARGS="-Xmx${MEM}g" exec "$F/fuseki-server" --loc="$T/jena-tdb2-$D" --port=3030 /ds ;;
    esac ;;
  rdf4j-native|rdf4j-native-default)
    # RDF4J's own server (docker image eclipse/rdf4j-workbench, pinned by digest in VERSIONS), NativeStore repository "bench" created
    # through the repository config with the registered index spec. Config vocabulary (sail type openrdf:NativeStore) and the triples.prop receipt were checked by hand against the pinned image on 2026-11-23; the full upload is not yet.
    IDX=spoc,posc,ospc; [ "$SYS" = rdf4j-native-default ] && IDX=spoc,posc
    DIR=$T/$SYS-$D; mkdir -p "$DIR"
    case $MODE in
      serve) docker rm -f bench-rdf4j >/dev/null 2>&1 || true
             exec docker run --rm --name bench-rdf4j "${LIM[@]}" -e JAVA_OPTS="-Xmx${MEM}g" -p 3040:8080 -v "$DIR":/var/rdf4j "$RDF4J_IMAGE" ;;
      load)  docker rm -f bench-rdf4j-load >/dev/null 2>&1 || true
             trap 'docker rm -f bench-rdf4j-load >/dev/null 2>&1 || true' EXIT   # a failed step must not leave the container on the port
             docker run -d --rm --name bench-rdf4j-load "${LIM[@]}" -e JAVA_OPTS="-Xmx${MEM}g" -p 3040:8080 -v "$DIR":/var/rdf4j "$RDF4J_IMAGE" >/dev/null
             until curl -sf http://127.0.0.1:3040/rdf4j-server/protocol >/dev/null 2>&1; do sleep 2; done
             cat > "$DIR/bench-config.ttl" <<TTL
@prefix config: <tag:rdf4j.org,2023:config/> .
[] a config:Repository ; config:rep.id "bench" ;
   config:rep.impl [ config:rep.type "openrdf:SailRepository" ;
     config:sail.impl [ config:sail.type "openrdf:NativeStore" ; config:native.tripleIndexes "$IDX" ] ] .
TTL
             curl -sf -X PUT -H 'Content-Type: text/turtle' --data-binary @"$DIR/bench-config.ttl" http://127.0.0.1:3040/rdf4j-server/repositories/bench
             curl -sf -X POST -H 'Content-Type: application/n-triples' -T "$NT" http://127.0.0.1:3040/rdf4j-server/repositories/bench/statements
             # receipt: the indexes the store actually created
             docker exec bench-rdf4j-load sh -c 'cat /var/rdf4j/server/repositories/bench/triples.prop 2>/dev/null || find /var/rdf4j -name "triples.prop" -exec cat {} +' >&2 || true
             docker stop bench-rdf4j-load >/dev/null ;;
    esac ;;
  millenniumdb)
    # MillenniumDB RDF model (docs: github.com/MillenniumDB/MillenniumDB/wiki). UNTESTED. Facts from its documentation that matter here:
    # the importer reads a real .ttl file (a hard link works, a symlink does not); SPARQL is POSTed to :1234/sparql as
    # application/sparql-query; the documented Accept is text/csv (JSON is unverified, and CSV loses term types); named graphs
    # and FROM/GRAPH are unsupported; numeric lexical forms are not preserved. Build the image first: build-millenniumdb.sh.
    DIR=$T/millenniumdb-$D; IMG=mdb:$MILLENNIUMDB_COMMIT
    case $MODE in
      load)  echo "budget: container cgroup limits" >&2
             mkdir -p "$DIR/in"; [ -e "$DIR/in/$D.ttl" ] || ln "$NT" "$DIR/in/$D.ttl"
             rm -rf "$DIR/db"
             docker run --rm "${LIM[@]}" -v "$DIR":/data "$IMG" mdb import "/data/in/$D.ttl" /data/db ;;
      serve) docker rm -f bench-mdb >/dev/null 2>&1 || true
             exec docker run --rm --name bench-mdb "${LIM[@]}" -p 1234:1234 -v "$DIR":/data "$IMG" mdb server /data/db ;;
    esac ;;
  oxigraph)
    O=$T/oxigraph
    [ "$("$O" --version | awk '{print $2}')" = "$OXIGRAPH_VERSION" ] || { echo "oxigraph version is not $OXIGRAPH_VERSION" >&2; exit 1; }
    case $MODE in
      load)  echo "budget not enforced for native oxigraph" >&2
             rm -rf "$T/oxigraph-$D"; "$O" load --location "$T/oxigraph-$D" --file "$NT"
             # vendor advice for read-heavy use (printed by the loader itself): compact the store before serving it
             "$O" optimize --location "$T/oxigraph-$D" ;;
      serve) echo "budget not enforced for native oxigraph" >&2
             exec "$O" serve-read-only --location "$T/oxigraph-$D" --bind 127.0.0.1:7878 ;;
    esac ;;
  *) echo "unknown system $SYS" >&2; exit 2 ;;
esac
