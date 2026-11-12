# Benchmarks

`bench/tools/` (gitignored) holds downloaded systems and data: Jena 6.2.0 + Fuseki 6.2.0, Oxigraph 0.5.13 (release binary),
LUBM UBA 1.7 generator. Reproduce LUBM-10:

    java -cp tools/uba/classes edu.lehigh.swat.bench.uba.Generator -univ 10 -index 0 -seed 0 -onto http://swat.cse.lehigh.edu/onto/univ-bench.owl
    tools/apache-jena-6.2.0/bin/riot --output=ntriples lubm10/*.owl > tools/lubm10.nt      # 1,316,700 triples
    tools/apache-jena-6.2.0/bin/tdb2.tdbloader --loc tools/tdb2-lubm10 tools/lubm10.nt
    tools/oxigraph load --location tools/oxi-lubm10 --file tools/lubm10.nt
    python3 run.py lubm jena-tdb2=http://127.0.0.1:3030/ds/sparql oxigraph=http://127.0.0.1:7878/query rdfdb=http://127.0.0.1:7979/sparql

`run.py`: per query 1 warm-up + 5 runs over HTTP (SPARQL protocol, JSON results), median reported; answers
(sorted rows, blank labels erased) are hashed and compared with the first endpoint. Raw numbers: `results_*.json`.
