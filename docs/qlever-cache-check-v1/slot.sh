#!/bin/bash
# #105 QLever cache check: serve the baseline's existing stores read-only; control vs cache cleared before every request. Lock first; exit 75 if held.
W=<work-dir>; O=$W/slotq; mkdir /tmp/rdf-bench.lock 2>/dev/null || { echo "LOCK HELD"; exit 75; }
echo "WindowsXP #105 QLever result-cache check (Docker, read-only stores, ~30 min)" > /tmp/rdf-bench.lock/owner
trap 'docker rm -f bench-qlever-qc >/dev/null 2>&1; rm -rf /tmp/rdf-bench.lock; echo RELEASED >> $O/results.txt' EXIT
rm -rf $O; mkdir -p $O
A=<baseline-agent-dir>; T=$A/rdfdb/bench/tools; Q=$A/notes/perf105/raw/queries; S=<work-dir>/qstore   # hard links to the baseline stores; the server writes its query log next to them, not into the baseline directory
IMG=$(grep QLEVER_IMAGE <repo>/bench/eval/VERSIONS | cut -d= -f2- | tr -d '"')
serve() { # dir name
  docker rm -f bench-qlever-qc >/dev/null 2>&1
  docker run -d --rm --name bench-qlever-qc --memory=12g --memory-swap=12g --cpus=8 -p 127.0.0.1:7011:7001 -v "$S":/data -w /data/$1 --entrypoint /qlever/qlever-server "$IMG" -i $2 -p 7001 -m 12G -c 1MB -e 1MB -E 1MB -a qc >/dev/null
  for i in $(seq 1 120); do curl -sf -o /dev/null --data-urlencode 'query=ASK{?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o}' http://127.0.0.1:7011/ && return 0; sleep 2; done; return 1; }
{ ls $T | grep -i qlever; echo "image $IMG"; } >> $O/results.txt
serve qlever-bsbm100 bsbm100 && python3 $W/qcache.py http://127.0.0.1:7011/ $O/bi.json $(ls $Q/bi100/*_q[1-4].rq) > $O/bi.log 2>&1; echo "bi exit $?" >> $O/results.txt
serve qlever-lubm1000 lubm1000 && python3 $W/qcache.py http://127.0.0.1:7011/ $O/lubm.json $Q/agg-lubm/all_q9.rq $Q/agg-lubm/distinct_q9.rq > $O/lubm.log 2>&1; echo "lubm exit $?" >> $O/results.txt
echo DONE >> $O/results.txt
