#!/bin/bash
# One system at a time (fair memory, no interference): start server, wait until it answers, run queries, stop it.
# usage: suite.sh <dataset> [systems...]     dataset = tools/<dataset>.nt; queries = <dataset without digits>/ (lubm, watdiv)
#   e.g. suite.sh lubm100 rdfdb rdfdb-stats jena-tdb2 oxigraph rdf4j-native rdf4j-mem
# Disk stores (tdb2-<dataset>, oxi-<dataset>, rdf4j-native-<dataset>) are built on first use; load time is logged.
set -u
D=$1; shift
cd "$(dirname "$0")"
# Machine lock shared by the team: one timed run at a time.
if mkdir /tmp/rdf-bench.lock 2>/dev/null; then echo "suite.sh $D $* ($(whoami), pid $$)" > /tmp/rdf-bench.lock/owner; trap 'rm -rf /tmp/rdf-bench.lock' EXIT
elif ! grep -q "pid $PPID\|suite.sh" /tmp/rdf-bench.lock/owner 2>/dev/null; then echo "machine locked by: $(cat /tmp/rdf-bench.lock/owner)" >&2; exit 1; fi
T=tools; R=rdf4j-server; OUT=${OUT_PREFIX:-results/}$D; QDIR=${D//[0-9]/}; mkdir -p $OUT
CP="$R/target/rdf4j-server-1.jar:$R/target/lib/*"
NT=$T/$D.nt
for sys in "$@"; do
  case $sys in
    rdfdb)        cmd=(../target/release/rdfdb serve $NT 7979); url=http://127.0.0.1:7979/sparql ;; # statistics on (default)
    rdfdb-disk)   [ -d $T/rdfdb-disk-$D ] || { /usr/bin/time -p ../target/release/rdfdb build-disk $NT $T/rdfdb-disk-$D 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=(../target/release/rdfdb serve $T/rdfdb-disk-$D 7979 --disk); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-nostats) cmd=(../target/release/rdfdb serve $NT 7979 --no-stats); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-parallel) cmd=(../target/release/rdfdb serve $NT 7979 --parallel-load=4); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-disk)   cmd=(../target/release/rdfdb serve $T/$D.rdfdb-snapshot 7979 --disk); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-stats)  cmd=(../target/release/rdfdb serve $NT 7979 --stats); url=http://127.0.0.1:7979/sparql ;;
    jena-tdb2)    [ -d $T/tdb2-$D ] || { /usr/bin/time -p $T/apache-jena-6.2.0/bin/tdb2.tdbloader --loc $T/tdb2-$D $NT 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --loc=$T/tdb2-$D --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    jena-mem)     cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --file=$NT --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    oxigraph)     [ -d $T/oxi-$D ] || { /usr/bin/time -p $T/oxigraph load --location $T/oxi-$D --file $NT 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=($T/oxigraph serve-read-only --location $T/oxi-$D --bind 127.0.0.1:7878); url=http://127.0.0.1:7878/query ;;
    rdf4j-native) if [ -d $T/rdf4j-native-$D ]; then src=-; else src=$NT; fi
                  cmd=(java -Xmx24g -cp "$CP" Rdf4jServer native $T/rdf4j-native-$D $src 3040); url=http://127.0.0.1:3040/sparql ;;
    rdf4j-mem)    cmd=(java -Xmx24g -cp "$CP" Rdf4jServer memory x $NT 3041); url=http://127.0.0.1:3041/sparql ;;
    # Docker (Linux VM) systems: QLever, and rdfdb built for Linux so it runs in the same VM (fair comparison).
    qlever)       [ -d $T/qlever-$D ] || { mkdir -p $T/qlever-$D; /usr/bin/time -p docker run --rm -v $PWD/$T:/data -w /data/qlever-$D --entrypoint /qlever/qlever-index adfreiburg/qlever:latest -i $D -f /data/$D.nt -F nt -p true 2>&1 | grep real > $OUT/$sys.load.log; }
                  # Result cache off (-c/-e/-E 1MB) so repeated runs measure execution, not cache hits.
                  cmd=(docker run --rm --name bench-$sys -p 7001:7001 -v $PWD/$T:/data -w /data/qlever-$D --entrypoint /qlever/qlever-server adfreiburg/qlever:latest -i $D -p 7001 -m 12G -c 1MB -e 1MB -E 1MB); url=http://127.0.0.1:7001/ ;;
    rdfdb-vm)     cmd=(docker run --rm --name bench-$sys -p 7979:7979 -e RDFDB_HOST=0.0.0.0 -v $PWD/$T:/data -v $PWD/../target-linux:/bin-rdfdb --platform linux/arm64 debian:trixie-slim /bin-rdfdb/release/rdfdb serve /data/$D.nt 7979); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-disk-vm) [ -d $T/rdfdb-diskvm-$D ] || { /usr/bin/time -p docker run --rm -v $PWD/$T:/data -v $PWD/../target-linux:/bin-rdfdb --platform linux/arm64 debian:trixie-slim /bin-rdfdb/release/rdfdb build-disk /data/$D.nt /data/rdfdb-diskvm-$D 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=(docker run --rm --name bench-$sys -p 7979:7979 -e RDFDB_HOST=0.0.0.0 -v $PWD/$T:/data -v $PWD/../target-linux:/bin-rdfdb --platform linux/arm64 debian:trixie-slim /bin-rdfdb/release/rdfdb serve /data/rdfdb-diskvm-$D 7979 --disk); url=http://127.0.0.1:7979/sparql ;;
    *) echo "unknown system $sys" >&2; exit 1 ;;
  esac
  port=${url#http://127.0.0.1:}; port=${port%%/*}
  if lsof -nP -iTCP:$port -sTCP:LISTEN >/dev/null; then echo "port $port busy; refusing to measure a stale server" >&2; exit 1; fi
  "${cmd[@]}" > $OUT/$sys.server.log 2>&1 &
  pid=$!
  start=$(date +%s)
  until curl -s -o /dev/null --data-urlencode 'query=ASK{?s ?p ?o}' $url; do sleep 2; kill -0 $pid 2>/dev/null || break; done
  echo "$sys ready after $(( $(date +%s) - start ))s" | tee -a $OUT/$sys.server.log
  python3 -I run.py $QDIR $sys=$url > $OUT/$sys.txt 2>&1
  f=$(ls -t results_${QDIR}_*.json | head -1); mv "$f" $OUT/$sys.json
  if docker ps -q -f name=bench-$sys | grep -q .; then
    docker stats --no-stream --format '{{.MemUsage}}' bench-$sys | awk '{print "container_mem " $1}' >> $OUT/$sys.server.log
    docker rm -f bench-$sys >/dev/null
  else
    ps -o rss= -p $pid | awk '{printf "rss_mb %d\n", $1/1024}' >> $OUT/$sys.server.log
  fi
  kill $pid 2>/dev/null; wait $pid 2>/dev/null
done
