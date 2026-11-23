# LUBM-100 DuckDB layout qualification

DuckDB 2.0.0 on Linux ARM64 passed all 90 scheduled answer checks on LUBM-100. Each of the three query arms matched the pinned Jena reference on all 30 registered labels, including duplicate query texts with separate labels.

| Configuration | File bytes | Exact typed checks |
| --- | ---: | ---: |
| Single triples table | 347,353,088 | 30/30 |
| Six sorted copies, always query SPO | 1,102,852,096 | 30/30 |
| Six sorted copies, route by longest constant prefix | Same file as the fixed-SPO run | 30/30 |

The importer read 2,377,133,547 N-Triples bytes and produced 13,409,395 distinct triples. Each of the six orders reopened with the same typed schema and row bag as the control. Both `EXCEPT ALL` differences were empty for every table, and each table had zero adjacent inversions in its declared order. The fixed and routed query arms opened the same checkpointed file read-only.

The first attempt stopped during offline installation with ENOSPC before loading any data. The retry used a dedicated writable pip temporary directory under its guarded scratch root. The archive includes both attempts' setup evidence, the successful import, build and reopen receipts, and the 90 saved answers. This qualification collected no query timings. WatDiv and BSBM BI remain pending.

Replay the evidence with Python 3.12 or later. The replay checks the packet and input hashes, receipts and journals, then compares the saved typed results with the pinned Jena answers. It does not start DuckDB.

```sh
python3 -IB replay.py packet.tar.gz
```

[Evidence archive](packet.tar.gz) · [Archive SHA-256](PACKET.sha256) · [Replay output](AUDIT.json)
