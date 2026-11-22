# Documentation

**Start here**
- [README](../README.md): what TripleLight is, quickstart, language bindings.
- [Embedded Rust API](embedded/README.md), including the async API.
- [Short technical report](report/technical-report.md) and the [demo paper](paper/main.pdf).

**How it works**
- [Architecture](../DESIGN.md) and [design rationale](design/first-principles.md).
- [Persistence and disk formats](persistence.md), [lazy dictionary design](lazy-dictionary-design.md),
  [parallel loading](parallel-load.md).
- [Optimizer](optimizer.md).

**Measurements**
- [Benchmark overview](benchmark-overview.md) and [full benchmark method and results](benchmarks.md).
- [Evaluation summary](report/evaluation.md).
- [Embedded-store inventory](embedded-store-inventory.md): which other engines were evaluated, and how.
- Each `*-qualification-v1/`, `*-v1/` and `*-v3/` directory is one registered measurement or diagnostic, with its
  method, raw data and receipts.
