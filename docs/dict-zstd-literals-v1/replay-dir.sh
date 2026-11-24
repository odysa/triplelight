#!/bin/bash
# Replay every .rq in a directory, in name order, one server lifetime: each query 1 warm-up + 3 timed, curl to
# /dev/null timed to the last byte (B8). Usage: replay-dir.sh <binary> <store> <query dir> <tag>. Caller holds the lock.
set -u
D=$(cd "$(dirname "$0")" && pwd); cd $D
BIN=$1; STORE=$2; Q=$3; TAG=$4; PORT=7983
TRIPLELIGHT_HOST=127.0.0.1 "$BIN" serve "$STORE" $PORT --disk --cache-mb 6144 > server-$TAG.out 2> server-$TAG.err &
PID=$!
for _ in $(seq 1 240); do curl -s -o /dev/null "http://127.0.0.1:$PORT/sparql?query=ASK%7B%7D" && break; sleep 0.5; done
echo -e "query\tkind\trun\tseconds\tbytes" > times-$TAG.tsv
for f in $(/bin/ls $Q/*.rq | /usr/bin/sort); do
  q=$(basename $f .rq)
  for k in warmup:0 timed:1 timed:2 timed:3; do
    out=$(curl -s -o /dev/null -w '%{time_total}\t%{size_download}' --data-urlencode "query@$f" -H 'Accept: application/sparql-results+json' "http://127.0.0.1:$PORT/sparql")
    echo -e "$q\t${k%%:*}\t${k#*:}\t$out" >> times-$TAG.tsv
  done
done
kill $PID; wait $PID 2>/dev/null
