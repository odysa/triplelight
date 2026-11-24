#!/usr/bin/env bash
# RDFS answers of TripleLight against Jena (#108). The input graph is the ontology plus the data. Jena closes it with
# `infer --rdfs` (rules rdfs2, 3, 5, 7, 9 and 11, with no axiomatic triples and no range types on literals: the rule
# set of TripleLight's `Entailment::Rdfs`). `sparql` then queries the closure under simple entailment, and TripleLight
# answers the same query over the unclosed input with `--entailment=rdfs`. Each query's solutions must be equal as
# multisets. Blank nodes are compared as one placeholder.
#
#   bench/rdfs/check.sh <triplelight-binary> <jena-home> <ontology> <data files...>
# Queries: bench/rdfs/lubm/*.rq (the 14 original LUBM queries, which rely on inference). OUT keeps the work directory.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=$1 JENA=$2 ONTO=$3; shift 3
OUT=${OUT:-$(mktemp -d)}; mkdir -p "$OUT"
"$JENA/bin/riot" --quiet --output=nt "$ONTO" "$@" > "$OUT/input.nt"
"$JENA/bin/riot" --quiet --output=nt "$ONTO" > "$OUT/ontology.nt"
"$JENA/bin/infer" --rdfs="$OUT/ontology.nt" "$OUT/input.nt" > "$OUT/closure.nt"
echo "input $(wc -l < "$OUT/input.nt") triples, Jena closure $(sort -u "$OUT/closure.nt" | wc -l) distinct triples" >&2
fail=0
for q in "$HERE"/lubm/q*.rq; do
  n=$(basename "$q" .rq)
  "$JENA/bin/sparql" --data "$OUT/closure.nt" --query "$q" --results=JSON > "$OUT/$n.jena.json"
  "$BIN" query "$OUT/input.nt" "$(cat "$q")" --entailment=rdfs > "$OUT/$n.tl.json" 2>/dev/null
  python3 -I "$HERE/same_solutions.py" "$n" "$OUT/$n.jena.json" "$OUT/$n.tl.json" || fail=1
done
exit $fail
