# RDFLib LevelDB storage qualification

The RDFLib LevelDB adapter stored and reopened all nine fixture triples on Linux ARM64. With RDFLib literal normalization disabled, the parsed, loaded and reopened triples all matched the exact typed reference. With default normalization, all three observations retained nine triples and the same two changed lexical forms.

| Profile | In-memory parse | LevelDB after load | LevelDB after fresh-process reopen |
| --- | --- | --- | --- |
| RDFLib defaults | 9 terms, two lexical changes | Graph length 9, exact match to parsed terms | Graph length 9, exact match to parsed terms |
| Normalization disabled | 9 exact reference terms | Graph length 9, exact reference bag | Graph length 9, exact reference bag |

The first observed changes appear during RDFLib's in-memory parse. It returned `"01"^^xsd:int` as `"1"^^xsd:int` and `"INF"^^xsd:double` as `"inf"^^xsd:double`. Neither LevelDB boundary introduced further differences in the typed term multisets.

After both loaders closed and exited, the directories held `CURRENT`, a manifest, a non-empty `000003.log` write-ahead log, `LOCK` and `LOG`. There were no table files at that point. Each fresh reader opened its store with creation disabled. Its log records `Recovering log #3`, and the post-reopen inventory contains `000005.ldb`. The successful fresh-process opens establish that the previous processes released the database locks. The `LOCK` files remained on disk. This was a normal close and reopen.

The separate shared RDFLib evaluator diagnostic returned nine rows for `FILTER(false)` in all four storage observations, against zero expected. This fixture run reports term fidelity and successful reopen. It collected no workload timing and makes no crash-durability or broader SPARQL claim.

The runtime used the unmodified RDFLib LevelDB source at `a0f3386c71e6b1cfbd09c257e29400f5fde43ed3` (package 0.2), Plyvel 1.5.1, Debian LevelDB 1.23-4 and Snappy 1.1.9-3, and RDFLib 7.6.0. The pinned Linux ARM64 Bookworm image ran CPython 3.12.16 and GCC/G++ 12.2.0. Both Python packages built offline. The run verified 756 installed files and, in each worker, the exact mapped private LevelDB and Snappy files. [Adapter source](https://github.com/RDFLib/rdflib-leveldb/tree/a0f3386c71e6b1cfbd09c257e29400f5fde43ed3) · [Plyvel 1.5.1](https://pypi.org/project/plyvel/1.5.1/)

The first harness revision built both wheels and then stopped when its native identity check used the wrong Snappy library name. It opened no RDF data. The second revision checked the actual ELF SONAME `libsnappy.so.1` and the exact versioned files, with unchanged package and fixture inputs. The packet retains both revisions.

## Replay

Extract `packet.tar.gz` into a fresh directory. From that directory run:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` lists all archived digests. The packet includes both frozen harnesses, the storage and runtime observations, the post-reopen databases, and the LevelDB info logs from both boundaries. The replay reads the frozen N-Triples reference independently and checks the typed multisets, file inventories and WAL recovery messages without starting an RDF engine.
