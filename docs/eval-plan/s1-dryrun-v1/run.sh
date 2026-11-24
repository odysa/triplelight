#!/bin/bash
# #92 S1 dry run on LUBM-100 (layout check only, no timing claims). Caller holds the lock.
set -u
D=<evidence dir>
REPO=<repo>
SHARED=<datasets dir>
T=/tmp/alex-s1-tools; OUT=/tmp/alex-s1-out
rm -rf $T $OUT; mkdir -p $T $OUT
ln -s $SHARED/lubm100.nt $T/lubm100.nt
( cd $REPO && cargo build --release -q --locked --features server --bin triplelight ) || exit 1
export TRIPLELIGHT_BIN=$REPO/target/release/triplelight TOOLS=$T RUNS=2 TIMEOUT=120
# reference answers for the layout check: from TripleLight itself (a real run takes them from Jena)
$TRIPLELIGHT_BIN build-disk $T/lubm100.nt $T/triplelight-lubm100 > $D/load.log 2>&1
$TRIPLELIGHT_BIN serve $T/triplelight-lubm100 7979 --disk > $D/ref-server.log 2>&1 & P=$!
for _ in $(seq 1 120); do curl -s -o /dev/null "http://127.0.0.1:7979/sparql?query=ASK%7B%7D" && break; sleep 0.5; done
python3 $REPO/bench/eval/driver.py reference --queries $REPO/bench/lubm --url http://127.0.0.1:7979/sparql --out $OUT/reference.json --timeout 120 > $D/ref.log 2>&1
kill $P; wait $P 2>/dev/null
$REPO/bench/eval/s1-run.sh lubm100 $REPO/bench/lubm $OUT 1 2 > $D/s1-run.out 2>&1; echo "s1-run exit $?" >> $D/s1-run.out
for g in 1 2; do cp $OUT/${g}g/triplelight.s1.json $D/s1-${g}g.json 2>/dev/null; cp $OUT/${g}g/triplelight.s1-cache.json $D/s1-cache-${g}g.json 2>/dev/null; cp $OUT/${g}g/triplelight.s1.tsv $D/s1-${g}g.tsv 2>/dev/null; done
rm -rf $T $OUT
echo ALL-DONE >> $D/s1-run.out
