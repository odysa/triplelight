# Large answers: the Python hash, not the engine, was being timed (task #105)

**Finding.** In the #105 baseline the LUBM-1000 Q6 and Q14 requests (7,924,765 rows each) take about 22 s for TripleLight (21.9 s) and
QLever (21.6 s alike). The server's own phase trace for the same request shows 0.86 s in total (execution 0.46 s, output 0.40 s),
and a plain `curl` to `/dev/null` takes 1.2 s. The 22 s is the time `driver.py` needs to hash 7.9M rows while streaming them
(about 0.35M rows per second), so the large-answer rows of the baseline measured the driver, and every engine ties at that speed.

**Change (`bench/eval/driver.py`).** For a request whose reference answer has at least `--drain-rows` rows (default 200,000), the
warm-up run is the one hashed answer check; the timed runs only read the body to the last byte (time to last byte) and must return
exactly the check run's byte count and a complete closing brace, or they are recorded as `unstable`. A wrong or incomplete check
answer is never timed. Cold runs are unchanged (one run, hashed). `driver.py selftest` covers the drain path.

## Data (`trace.txt`, `curl.txt`)
TripleLight release binary, LUBM-1000 four-order store (a copy-on-write clone of the baseline store), `--cache-mb 6144`,
`TRIPLELIGHT_TRACE_PHASES=1` and `TRIPLELIGHT_TRACE_BLOCK_CACHE=1`, one server, requests q13, q6 and q14 five times each (laptop,
macOS arm64; one session, one run per row). `curl.txt` is the client-side time and body size of each request; `trace.txt` the
server's phase and block-cache lines.

## Side result: LUBM q13 (2,604 rows)
The 2,604 rows touch 2,476 distinct dictionary blocks. First request (all misses): output 522 ms, 2,476 block reads, 11.8 MB.
Repeat requests (all hits): output 2.4 to 4 ms, total 3.6 to 5.4 ms. The baseline's 41 to 55 ms output per request equals about
16 µs per block read, the cost of blocks served from the OS page cache, so the baseline's dictionary blocks were not staying in
TripleLight's block cache between runs. Why is not established here.
