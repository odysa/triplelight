# Mulgara 2.1.13 build and network-free startup

Mulgara 2.1.13 built successfully on macOS ARM64 with Zulu OpenJDK 8u514 and the source tree's Ant 1.9.7. The local database constructor then failed under network denial. Its bundled JOTM transaction manager tried to export RMI objects during system-graph bootstrap, and macOS denied the socket bind. The constructor failed before it returned a session factory, so the fixture was never loaded. This result covers the pinned local database configuration and records no query answers, reopen observations, or workload timing.

## Build

The source commit is `a84257dae77b6c14a20b9013062bb469675f10b5` and the Git tree is `a1b7102c6accd68d06c03dfef372d263b6641aa5`. The guarded `embedded-dist` build produced a 33,239,922-byte JAR with SHA-256 `fad67f830a3cbd2a69d865a0c377b448d9217d45d04b2718a8097c7298d5e57b`. The build verified the expected local session and SPARQL classes. The restricted PATH had no `svnversion`. [Pinned Mulgara source](https://github.com/quoll/mulgara/tree/a84257dae77b6c14a20b9013062bb469675f10b5)

The first attempt stopped in a harness check that incorrectly classified the optional local `svnversion` probe as a launch. The second reached the JDK download and hit a ceiling based on Azul's metadata length of 103,757,000 bytes. The successful attempt received 103,757,046 bytes. That file matched Azul's published SHA-256 `792935f732653f5c4fd4cdc68e3ade65cb1fa1fda6b6769fc5dd8098b330b298` before extraction. The build logs and dependency inventory are in the evidence packet. [Azul package metadata](https://api.azul.com/metadata/v1/zulu/packages/c0f6e361-0067-49ca-b737-6b682a477f4a)

## Fixture startup

The fixture adapter verified the identity of the built JAR and compiler, then compiled successfully. It requested `org.mulgara.resolver.Database` through `SessionFactoryFactory` and supplied a local URI and an owned store directory. The loopback bind control received the expected permission error. JOTM's RMI export subsequently failed at an ephemeral port request, and the database constructor failed while bootstrapping the system model. Some internal store files were created. The adapter emitted only its load-stage marker. No fixture model, triples, or query results were observed. This profile provides a build pass and a network-free startup failure.

## Evidence replay

The [packet](packet.tar.gz) contains both executed source freezes, the earlier build-stop receipts, the successful build receipt, and the failed startup receipt and logs. The bundled JAR and JDK are omitted. The logs replace the local workspace prefix and redact one build-log host address and the startup log’s host aliases, and the JVM account property.

Extract the archive in a fresh directory and run the replay.

```sh
python3 -I -B replay.py replayed.json
cmp AUDIT.json replayed.json
```

Replay uses only the recorded evidence and Python's standard library. It verifies the source freezes, the build and fixture receipt chain, and the terminal trace. [Packet member digests](PACKET_FILES.json)
