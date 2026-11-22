#!/usr/bin/env python3
"""#91: term occurrences vs distinct terms in an N-Triples file, per shard (same byte split as the builder: 8 shards cut at
the next line boundary), globally, and per kind. Terms are the raw tokens of the line (IRI, blank node, literal); hash64
stands in for the term, so a collision (about 1e-12 per pair) could undercount distinct terms by one.
usage: term_dup.py file.nt out.json [shards]"""
import sys, json, os, time, collections
path, out = sys.argv[1], sys.argv[2]
shards = int(sys.argv[3]) if len(sys.argv) > 3 else 8
size = os.path.getsize(path)
def boundary(f, off):
    if off == 0: return 0
    f.seek(off); f.readline(); return f.tell()
with open(path, "rb") as f:
    bounds = [0] + [boundary(f, size * i // shards) for i in range(1, shards)] + [size]
t0 = time.time()
glob = collections.Counter()
kind_occ = collections.Counter(); kind_bytes = collections.Counter()
rows = []
distinct_bytes_global = {}
with open(path, "rb") as f:
    for s in range(shards):
        f.seek(bounds[s]); end = bounds[s + 1]
        seen = set(); occ = 0; occ_bytes = 0; dist_bytes = 0; triples = 0
        while f.tell() < end:
            line = f.readline()
            if not line: break
            parts = line.split(b" ", 2)
            if len(parts) < 3: continue
            triples += 1
            o = parts[2]
            if o.endswith(b" .\n"): o = o[:-3]
            elif o.endswith(b" ."): o = o[:-2]
            for tok in (parts[0], parts[1], o):
                h = hash(tok); n = len(tok)
                occ += 1; occ_bytes += n + 10
                glob[h] += 1
                k = tok[:1]; kind_occ[k] += 1
                if h not in seen:
                    seen.add(h); dist_bytes += n + 10
                    kind_bytes[k] += 0
        rows.append({"shard": s, "triples": triples, "occurrences": occ, "distinct": len(seen), "occurrence_record_bytes": occ_bytes, "distinct_record_bytes": dist_bytes})
        print(rows[-1], flush=True)
total_occ = sum(r["occurrences"] for r in rows)
sum_distinct = sum(r["distinct"] for r in rows)
freq = collections.Counter()
for h, c in glob.items():
    b = 1 if c == 1 else 2 if c < 10 else 3 if c < 100 else 4 if c < 1000 else 5 if c < 100000 else 6
    freq[b] += c
labels = {1: "1", 2: "2-9", 3: "10-99", 4: "100-999", 5: "1000-99999", 6: ">=100000"}
res = {"file": os.path.basename(path), "shards": shards, "seconds": round(time.time() - t0, 1),
       "occurrences": total_occ, "distinct_global": len(glob), "sum_of_per_shard_distinct": sum_distinct,
       "distinct_share_global": round(len(glob) / total_occ, 5), "per_shard_distinct_share": round(sum_distinct / total_occ, 5),
       "occurrence_record_bytes": sum(r["occurrence_record_bytes"] for r in rows),
       "per_shard_distinct_record_bytes": sum(r["distinct_record_bytes"] for r in rows),
       "kind_occurrences": {k.decode(): v for k, v in kind_occ.items()},
       "occurrences_by_term_frequency": {labels[b]: freq[b] for b in sorted(freq)}, "per_shard": rows}
json.dump(res, open(out, "w"), indent=1)
print(json.dumps({k: v for k, v in res.items() if k != "per_shard"}, indent=1))
