# Matched embedded Python LUBM-100 harness check

TripleLight and PyOxigraph 0.5.13 each reopened 13,409,395 triples and matched all 30 typed LUBM-100 aggregate answers against the frozen Jena references. The workloads ran through their Python APIs on Linux ARM64 and CPython 3.13.18. Both workers fully consumed their results. The check retained zero query durations.

| Stage | Registration SHA-256 prefix | Result |
| --- | --- | --- |
| TripleLight build and import | `34cc44ff` | The locked ABI3 build from public main `24913796` imported in CPython 3.11.18 without opening a database. |
| PyOxigraph load | `a65c71a5` | The ARM64 CPython 3.13 wheel pinned by the rental requirements loaded the same LUBM-100 input. The loader called `bulk_load` and `flush`, closed the store and sealed its 56 files. It omitted `optimize` to match the fresh Oxigraph store procedure checked in #93. |
| Separate imports and disk reopen | `5bfcffac` | Both modules imported before either store opened. Two fresh query processes reopened the same graph, then passed all 30 exact typed checks per engine. |

The query containers each had a four-CPU quota, 12 GiB of memory, zero swap and `RAYON_NUM_THREADS=4`. Each query process reported one Python-managed thread after import and at exit. The highest sampled native process thread counts were five for TripleLight and thirteen for PyOxigraph. These counts came from periodic Linux process samples. The Python APIs provided no direct measurement of the Rayon pool width.

PyOxigraph opened its store read-only. TripleLight used a private copy of the qualified store and waited for statistics readiness. Its lock and WAL metadata changed during the process. Full store hashes and the TripleLight statistics matched after each query worker. The source stores and runtime files also matched after the run.

The exact PyOxigraph ARM64 wheel SHA-256 is `7ad11cc64424fffb65e407ad6a88ce1483c8597cf196788aa2e4b89287eef860`. The x86-64 rental entry has its own wheel hash. This check qualifies the laptop harness and answers for these builds and this workload. Paper timings require the dedicated-machine run.

## Replay

Extract `packet.tar.gz` (SHA-256 `87680d9ef0d6c775256dcda8fecdc2e17623c4a2a09db59219695b3a489b14ee`) into a fresh directory. Verify its `SHA256SUMS`, then run `python3 -I -B replay.py` from the extracted directory. The archive keeps unchanged build, load and query receipts, typed observations, resource records, query texts and references. The replay runs without a database and validates typed values, projection and row multiplicity for all 60 observed answers.
