# Portable A1 evidence replay

Run with Python 3.12+ (standard library only):

```sh
python3 replay.py --packet /path/to/packet-v1.tar.gz --out audit.json
```

This requires the exact original packet SHA256 15af092f0cc98da4ce15f5c4f4914a03c26ff132bb670898d02b5b10d824e55e. It safely extracts it into a temporary directory, verifies all 294 file hashes, then loads only the hash-pinned, previously reviewed supervisor’s gate definitions (not its main dispatch). It recomputes counter deltas, forty summaries, timing medians, cleanup/resource checks and overlap flags from raw records. It starts no engine or container, does not inspect live stores and needs no author workspace or network.

The empty log’s archived mtime is a reconstructed host start proxy adjacent to the attach call. The script retains the original summary interval limitation; it does not claim an exact container start. Its flags agree on phases 13–15. Unknown indirect effects on both counters and timing remain. Earlier v1-unrun and v2-failed records are checked and never pooled.
