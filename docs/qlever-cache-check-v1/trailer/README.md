# QLever's JSON trailer (task #105)

QLever (the baseline image, LUBM-1000 store, read-only through hard links, cache as the harness configures it) ends every
`application/sparql-results+json` answer with a `meta` member whose `query-time-ms` changes from run to run. The last 400 bytes of
two responses of the same request (`q1.1.tail`, `q1.2.tail`):

```
...}}]},"meta":{"query-time-ms":15,"result-size-total":4,"warnings":[]}}
...}}]},"meta":{"query-time-ms":1,"result-size-total":4,"warnings":[]}}
```

So two correct answers differ in length by the digits of the time (q1: 467 and 466 bytes; q13: 235,817 and 235,815), which made the
drained-run byte comparison of plan rule B8 mark QLever's large answers `unstable`. `bench/eval/driver.py` now leaves the
`"query-time-ms":N` member out of the byte count of both the hashed check run and the drained runs. My first pattern (from a
description) required a comma before the key and would not have matched, because the key follows `{`; the capture caught that.

`results.txt`: the byte counts of the captured responses, and `bytecheck.sh` on LUBM Q6 (7.9M rows): both runs 761,668,080 bytes
(the digits happened to match), HTTP 200. Laptop Docker, one session.
