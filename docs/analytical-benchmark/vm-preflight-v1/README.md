# Matched VM preflight, version 1

No competitor BI latency measurements were taken. The running VM reports
10 CPUs and 16,808,177,664 bytes RAM. Image digests and version/help responses
are retained. The existing QLever ARM64 image is pinned to
adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727.

A separate 83-triple fixture index was freshly built and served on a private
loopback port. Live settings confirm 1 MB cache caps and 10 GB query memory.
A small grouped COUNT query reports computed on its first execution and
cached_not_pinned on repetition; two unpinned cache results consume 856 bytes.
The unprivileged clear-cache command resets all five cache counters to zero.
This establishes the behavior of this fixture/image, not the hit status of
historical benchmark queries.

A second fresh server starts with disable-caching=true. Live settings confirm
that value, 10 GB query memory and four query threads. Two repetitions report
computed, and cache counters remain zero. Default hash-map grouping is false;
retain both the default and the explicit hash-group profile in future tests.

Standard SPARQL JSON returns COUNT as xsd:int. The strict datatype rule will
continue to report that discrepancy; count-equivalence-v1 adds a separately
labelled optional value-equivalence result. No confirmatory QLever BI result
has yet been inspected.

Both owned fixture containers exited and the benchmark lock was released.
The 10M competitor stores, matched Linux rdfdb/DuckDB runtimes and full HTTP
integration still need staging. Runtime settings are configuration evidence,
not measured RSS or a guarantee that all engines account for memory equally.
