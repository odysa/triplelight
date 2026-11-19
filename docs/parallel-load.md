# Parallel N-Triples loading

`triplelight serve data.nt 7878` uses the number of CPUs reported by `available_parallelism` to parse N-Triples chunks and sort the six indexes. The term dictionary is still constructed serially. Parsed chunks and all indexes are held in memory, so this is not an external-memory loader. The final available parallelism and memory use depend on the host.

Use `triplelight serve data.nt 7878 --parallel-load=1` for the original serial path, or `--parallel-load=4` to set a specific positive worker count. `triplelight query` accepts the same option. Turtle continues on its serial parser; requesting more than one worker explicitly with a `.ttl` input exits with an error.

The benchmark harness accepts `bench/suite.sh lubm100 triplelight-parallel` to select four workers. Its post-query RSS sample is not a peak-memory measurement.
