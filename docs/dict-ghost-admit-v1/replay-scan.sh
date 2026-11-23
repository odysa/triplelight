#!/bin/bash
# Replay the #105 baseline's LUBM-1000 order (q1..q14, each 1 warm-up + 3 timed, one server lifetime) against one
# binary, with phase and block-cache tracing. Usage: replay.sh <binary> <tag>. Caller holds the lock.
set -u
D=<evidence dir>; cd $D
Q=<repo>/bench/lubm
BIN=$1; TAG=$2; PORT=7981
TRIPLELIGHT_TRACE_PHASES=1 TRIPLELIGHT_TRACE_BLOCK_CACHE=1 TRIPLELIGHT_HOST=127.0.0.1 "$BIN" serve <LUBM-1000 four-order store> $PORT --disk --cache-mb ${CACHE_MB:-6144} > server-scan-$TAG.out 2> server-scan-$TAG.err &
PID=$!
for _ in $(seq 1 120); do curl -s -o /dev/null "http://127.0.0.1:$PORT/sparql?query=ASK%7B%7D" && break; sleep 0.5; done
echo -e "query\tkind\trun\tseconds\tbytes" > times-scan-$TAG.tsv
for n in 13 6 13 14 13; do
  for k in warmup:0 timed:1 timed:2 timed:3; do
    out=$(curl -s -o /dev/null -w '%{time_total}\t%{size_download}' --data-urlencode "query@$Q/q$n.rq" -H 'Accept: application/sparql-results+json' "http://127.0.0.1:$PORT/sparql")
    echo -e "q$n\t${k%%:*}\t${k#*:}\t$out" >> times-scan-$TAG.tsv
  done
done
kill $PID; wait $PID 2>/dev/null
echo done-$TAG
