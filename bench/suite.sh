#!/bin/bash
# One system at a time (fair memory, no interference): start server, wait until it answers, run queries, stop it.
# usage: suite.sh <dataset> [systems...]     dataset = tools/<dataset>.nt; queries = <dataset without digits>/ (lubm, watdiv)
#   e.g. suite.sh lubm100 rdfdb rdfdb-stats jena-tdb2 oxigraph rdf4j-native rdf4j-mem
# Disk stores (tdb2-<dataset>, oxi-<dataset>, rdf4j-native-<dataset>) are built on first use; load time is logged.
set -u
D=$1; shift
cd "$(dirname "$0")"
T=tools; R=rdf4j-server; OUT=results/$D; QDIR=${D//[0-9]/}; mkdir -p $OUT
CP="$R/target/rdf4j-server-1.jar:$R/target/lib/*"
NT=$T/$D.nt
for sys in "$@"; do
  case $sys in
    rdfdb)        cmd=(../target/release/rdfdb serve $NT 7979); # statistics on (default)
    rdfdb-nostats) cmd=(../target/release/rdfdb serve $NT 7979 --no-stats); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-parallel) cmd=(../target/release/rdfdb serve $NT 7979 --parallel-load=4); url=http://127.0.0.1:7979/sparql ;;
    rdfdb-stats)  cmd=(../target/release/rdfdb serve $NT 7979 --stats); url=http://127.0.0.1:7979/sparql ;;
    jena-tdb2)    [ -d $T/tdb2-$D ] || { /usr/bin/time -p $T/apache-jena-6.2.0/bin/tdb2.tdbloader --loc $T/tdb2-$D $NT 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --loc=$T/tdb2-$D --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    jena-mem)     cmd=(env JVM_ARGS=-Xmx24g $T/apache-jena-fuseki-6.2.0/fuseki-server --file=$NT --port=3030 /ds); url=http://127.0.0.1:3030/ds/sparql ;;
    oxigraph)     [ -d $T/oxi-$D ] || { /usr/bin/time -p $T/oxigraph load --location $T/oxi-$D --file $NT 2>&1 | grep real > $OUT/$sys.load.log; }
                  cmd=($T/oxigraph serve-read-only --location $T/oxi-$D --bind 127.0.0.1:7878); url=http://127.0.0.1:7878/query ;;
    rdf4j-native) if [ -d $T/rdf4j-native-$D ]; then src=-; else src=$NT; fi
                  cmd=(java -Xmx24g -cp "$CP" Rdf4jServer native $T/rdf4j-native-$D $src 3040); url=http://127.0.0.1:3040/sparql ;;
    rdf4j-mem)    cmd=(java -Xmx24g -cp "$CP" Rdf4jServer memory x $NT 3041); url=http://127.0.0.1:3041/sparql ;;
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
  ps -o rss= -p $pid | awk '{printf "rss_mb %d\n", $1/1024}' >> $OUT/$sys.server.log
  kill $pid; wait $pid 2>/dev/null
done
