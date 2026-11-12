# Generating the small WatDiv fixture

Source: [official WatDiv v0.6 site](https://dsg.uwaterloo.ca/watdiv/),
[archive](https://dsg.uwaterloo.ca/watdiv/watdiv_v06.tar).
This is scale factor 1 (~100K triples), not WatDiv-10M.

Extract the source outside the repository. In `src/model.cpp`, replace
`static_cast<unsigned> (time(0))` with `static_cast<unsigned> (20261112)`
and `srand (time(NULL))` with `srand (20261112)`. These are the two generator
seeds; the model, templates and distributions are otherwise unchanged.
The words dictionary is `/usr/share/dict/words`; its hash is in provenance.
The first/last-name dictionaries come from the source archive.

With Boost headers available (this run used a task-local extraction of Homebrew's
Boost 1.92.0_1 bottle; no system package installation):

```sh
mkdir -p bin/Release
clang++ -std=c++14 -O2 -w -Iinclude -I/path/to/boost/include src/dictionary.cpp src/model.cpp src/statistics.cpp src/volatility_gen.cpp -o bin/Release/watdiv
cd bin/Release
./watdiv -d ../../model/wsdbm-data-model.txt 1 > watdiv-sf1.nt
```

Keep `saved.txt`. For each of the 20 `.txt` files directly in `testsuite/`, run:

```sh
./watdiv -q ../../model/wsdbm-data-model.txt ../../testsuite/C1.txt 1 1 > C1.rq
```

Store one instance of each template in `queries/`, with no selection based on its
answer count. The saved queries and all source/data checksums are committed.
The dataset and extracted dependencies are kept outside the repository.

```sh
python3 docs/optimizer-watdiv-sf1/reference_counts.py DATA.nt /path/to/jena/bin/arq
cargo run --release --example optimizer_qerror -- DATA.nt docs/optimizer-watdiv-sf1/queries docs/optimizer-watdiv-sf1/reference_counts.tsv --verify-executor
```
