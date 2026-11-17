# Arrow SELECT results (experimental)

Install the optional `arrow` extra (PyArrow), then call `db.query_arrow(sparql)` or `snapshot.query_arrow(sparql)`. Each returns a `pyarrow.RecordBatchReader`; SELECT variables are nullable Arrow struct columns:

- `kind`: `iri`, `bnode`, or `literal`.
- `value`: exact IRI, blank-node label, or literal lexical text.
- `datatype`: datatype IRI for literals; null for other terms.
- `language`: language tag if present; null otherwise.

A null struct is an unbound variable, distinct from a literal with an empty lexical value. These fields preserve RDF identity instead of coercing every numeric or date value into a Python value. `reader.read_all()` creates a PyArrow table; Pandas/Polars can import Arrow, but their handling of nested structs and nulls varies, and conversions are not promised to be zero-copy.

The implementation currently executes and materializes the full owned result, then constructs Arrow-owned column buffers in batches of up to 8192 rows. The Arrow C stream interface transfers those buffers to PyArrow without copying the buffers; this is **not** a streaming query or zero-copy from the RDF dictionary, and memory grows with the result. ASK and graph queries are rejected. `db.export_parquet(query, path)` writes the same nested Arrow schema to Parquet. `db.export_csv(query, path)` writes four columns per variable (`name.kind`, `.value`, `.datatype`, `.language`); an empty kind means unbound, so an empty literal value remains distinguishable. For nonliteral terms datatype/language are empty; datatype/language IRIs and tags cannot be empty for valid literals. CSV field names follow the SELECT variables and can be duplicated or contain dots, so consumers should retain column positions. Each export writes a temporary sibling and renames it over the destination after success; this is atomic publication on a supporting local filesystem, not an fsync durability guarantee. CSV currently materializes each Arrow batch as Python objects and is not a zero-copy path.
