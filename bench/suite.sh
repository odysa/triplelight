#!/bin/bash
# One system at a time (fair memory, no interference): start server, wait until it answers, run queries, stop it.
# usage: suite.sh <scale> [systems...]    e.g. suite.sh 100 rdfdb jena-tdb2 oxigraph rdf4j-native rdf4j-mem
set -u
S=$1; shift
cd "$(dirname "$0")"
T=tools; R=rdf4j-server; OUT=results/lubm$S; mkdir -p $OUT
CP="$R/target/rdf4j-server-1.jar:$R/target/lib/*"
for sys in "$@"; do
  case $sys in
    rdfdb)        cmd=(../target/release/rdfdb serve $T/lubm$S.nt 7979); url=http://127.0.0.1:7979/sparql ;;
    jena-tdb2)    cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --loc=$T/tdb2-lubm$S --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    jena-mem)     cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --file=$T/lubm$S.nt --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    oxigraph)     cmd=($T/oxigraph serve-read-only --location $T/oxi-lubm$S --bind 127.0.0.1:7878); url=http://127.0.0.1:7878/query ;;
    rdf4j-native) cmd=(java -Xmx24g -cp "$CP" Rdf4jServer native $T/rdf4j-native$S - 3040); url=http://127.0.0.1:3040/sparql ;;
    rdf4j-mem)    cmd=(java -Xmx24g -cp "$CP" Rdf4jServer memory x $T/lubm$S.nt 3041); url=http://127.0.0.1:3041/sparql ;;
  esac
  port=${url#http://127.0.0.1:}; port=${port%%/*}
  if lsof -nP -iTCP:$port -sTCP:LISTEN >/dev/null; then echo "port $port busy; refusing to measure a stale server" >&2; exit 1; fi
  "${cmd[@]}" > $OUT/$sys.server.log 2>&1 &
  pid=$!
  start=$(date +%s)
  until curl -s -o /dev/null --data-urlencode 'query=ASK{?s ?p ?o}' $url; do sleep 2; kill -0 $pid 2>/dev/null || break; done
  echo "$sys ready after $(( $(date +%s) - start ))s" | tee -a $OUT/$sys.server.log
  python3 -I run.py lubm $sys=$url > $OUT/$sys.txt 2>&1
  f=$(ls -t results_lubm_*.json | head -1); mv "$f" $OUT/$sys.json
  ps -o rss= -p $pid | awk '{printf "rss_mb %d\n", $1/1024}' >> $OUT/$sys.server.log
  kill $pid; wait $pid 2>/dev/null
done
