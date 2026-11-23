# Matched embedded Java LUBM-100 harness check

TripleLight and Blazegraph 2.1.5 each matched all 30 typed answers against the frozen reference in fresh Temurin 25.0.5.1+2 JVMs on Linux ARM64. The successful retry used private copies of the previously qualified stores. It retained zero query durations and supplies no latency comparison.

| Stage | Registration SHA-256 prefix | Result |
| --- | --- | --- |
| Build 1 | `99078079` | Dependency fetch stopped before compilation when the rustup proxy tried to create a temporary file on the read-only toolchain mount. |
| Build 2 | `42f1cdac` | Used the already installed toolchain directly. The receipts bind 33 outputs, including both workers and the C library exporting the Rayon witness. |
| Untimed attempt 1 | `06b851a0` | Blazegraph matched 30/30. The metadata check stopped the supervisor before TripleLight ran. A separate SHA-256 check of the private journal matched the registered content. |
| Untimed attempt 2 | `42f00256` | Both workers matched 30/30. The supervisor checked the full store contents and TripleLight statistics after each worker and checked the artifact hashes again before completing. |

Both JVMs reported four available processors. The container CPU quota was four CPUs and the JVMs used `-XX:ActiveProcessorCount=4`. TripleLight reported four Rayon threads. Blazegraph's cached query executor reported a core size of zero, a maximum of 2,147,483,647 and a largest observed pool of 41 on the retry. Executor pool size, Rayon pool width and JVM-wide Java thread count are different measurements.

The private Blazegraph journal and TripleLight WAL changed file metadata on reopen. Full store hashes matched before and after the successful retry. The metadata observations apply to these exact runtime and store copies.

For the rental run, each process receipt must report the JVM-wide peak live Java thread count, for example through `ThreadMXBean.getPeakThreadCount()` after the worker's final pass, as well as the available processor count, CPU limit, and engine-specific pool measurements. Keep the metadata observations and pre/post store-content checks in the receipts. Register the rental run against the exact public main commit built for it. The default-off Rayon witness is available from [PR #193](https://github.com/odysa/triplelight/pull/193).

## Replay the answer checks

Extract `packet.tar.gz` (SHA-256 `a705d4cb65595feb8293d3804a0f4aade3255ac69b14b33432b77a0b3ab4563b`) in a fresh directory, verify `SHA256SUMS` and run `python3 -I -B replay.py` there. The replay needs only Python 3.10 or later. It checks the saved typed result records and the empty duration columns for the three completed passes. It does not launch either engine.

The four JSON receipts in the archive are unchanged. The portable replay bundle replaces two host dataset paths in its inputs with relative placeholders and updates the corresponding hashes. Query texts, references and observed answer records retain their original bytes. The archive omits databases, binaries and environmental logs.
