# Jena 100M reference-store count

A fresh query-only Fuseki process over the existing Jena TDB2 BSBM-100M store returns **98,861,444 triples** for its default graph. The raw typed COUNT response, query, runtime/help output, server lifecycle, and before/after store metadata are retained. This is a reference diagnostic, not a BI query, load time or performance comparison.

Kaiming authorized this in-place query-only use under the shared benchmark lock. The store was built with `tdb2.tdbloader` from the shared `bsbm100.nt`, with no subsequent updates reported. The source is pinned separately at 25,965,260,343 bytes and SHA-256 `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`. Its length/mtime match the earlier pin and remain unchanged during this check. Agreement with the NT-derived DuckDB graph is **still pending**; this one COUNT does not establish full input correspondence or the numerical input freeze.

The process launches without the `--update` flag, after recording the local CLI help. It runs only an ASK readiness probe and COUNT, with an 8 GiB heap and four active processors. It exits after the count and releases the owned lock. Only the two `tdb.lock` files change metadata; no shared lock file is removed manually. There is no graph update, compaction or BI candidate query.

`file-sha256.json` records the retained evidence hashes. The full 100M input anchors, independent per-group count verification, DuckDB import and matched comparison remain pending.
