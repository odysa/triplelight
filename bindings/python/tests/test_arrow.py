"""Arrow interchange preserves RDF identity and remains usable after the DB is closed."""
import pytest
pa = pytest.importorskip("pyarrow")
import rdfdb


def test_arrow_rdf_identity_and_lifetime(tmp_path):
    db = rdfdb.Database(tmp_path / "db")
    db.update('''INSERT DATA {
      <urn:s1> <urn:p> "1"^^<http://www.w3.org/2001/XMLSchema#int> .
      <urn:s2> <urn:p> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
      <urn:s3> <urn:p> "" .
      <urn:s4> <urn:p> "bonjour"@fr .
      _:x <urn:p> <urn:o> .
    }''')
    reader = db.query_arrow('SELECT ?s ?o ?missing WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:missing> ?missing } }')
    db.close()
    table = reader.read_all()
    assert table.num_rows == 5
    assert table.column_names == ["s", "o", "missing"]
    rows = table.to_pylist()
    assert all(r["missing"] is None for r in rows)
    terms = [r["o"] for r in rows]
    assert {x["value"] for x in terms} == {"1", "01", "", "bonjour", "urn:o"}
    one = next(x for x in terms if x["value"] == "01")
    assert one == {"kind": "literal", "value": "01", "datatype": "http://www.w3.org/2001/XMLSchema#int", "language": None}
    fr = next(x for x in terms if x["value"] == "bonjour")
    assert fr["language"] == "fr"
    assert next(x for x in terms if x["value"] == "urn:o")["kind"] == "iri"
    assert any(r["s"]["kind"] == "bnode" for r in rows)


def test_empty_and_non_select(tmp_path):
    db = rdfdb.Database(tmp_path / "db")
    table = db.query_arrow("SELECT ?s WHERE { ?s <urn:absent> ?o }").read_all()
    assert table.num_rows == 0 and table.column_names == ["s"]
    with pytest.raises(rdfdb.UnsupportedError, match="SELECT"):
        db.query_arrow("ASK { <urn:a> <urn:b> <urn:c> }")
    db.close()


def test_parquet_csv_export_and_failure_preserves_destination(tmp_path):
    import csv
    import pyarrow.parquet as pq
    db = rdfdb.Database(tmp_path / "db")
    db.update('INSERT DATA { <urn:s> <urn:p> "" . <urn:t> <urn:p> "01"^^<http://www.w3.org/2001/XMLSchema#int> . }')
    query = 'SELECT ?s ?o ?missing WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:no> ?missing } }'
    parquet = tmp_path / "results.parquet"
    csv_path = tmp_path / "results.csv"
    db.export_parquet(query, parquet)
    db.export_csv(query, csv_path)
    assert pq.read_table(parquet).to_pylist() == db.query_arrow(query).read_all().to_pylist()
    with csv_path.open(newline="", encoding="utf-8") as f:
        rows = list(csv.DictReader(f))
    assert len(rows) == 2
    assert all(row["missing.kind"] == "" for row in rows)
    assert any(row["o.kind"] == "literal" and row["o.value"] == "" for row in rows)
    assert any(row["o.value"] == "01" and row["o.datatype"].endswith("#int") for row in rows)
    with pytest.raises(rdfdb.RdfdbError):
        db.export_csv("ASK { <urn:s> <urn:p> ?x }", csv_path)
    with csv_path.open(newline="", encoding="utf-8") as f:
        assert len(list(csv.DictReader(f))) == 2
    assert not list(tmp_path.glob(".rdfdb-export-*"))
    db.close()


def test_arrow_multi_batch(tmp_path):
    source = tmp_path / "data.nt"
    source.write_text("".join(f'<urn:s{i}> <urn:p> "v{i}" .\n' for i in range(8201)), encoding="utf-8")
    db = rdfdb.Database(tmp_path / "db", initial=source)
    reader = db.query_arrow('SELECT ?s ?o WHERE { ?s <urn:p> ?o }')
    batches = list(reader)
    assert [batch.num_rows for batch in batches] == [8192, 9]
    assert sum(batch.num_rows for batch in batches) == 8201
    db.close()


def test_pandas_and_polars_preserve_struct_values(tmp_path):
    pd = pytest.importorskip("pandas")
    pl = pytest.importorskip("polars")
    db = rdfdb.Database(tmp_path / "db")
    db.update('INSERT DATA { <urn:s> <urn:p> "" . <urn:t> <urn:p> "bonjour"@fr . }')
    table = db.query_arrow('SELECT ?s ?o ?missing WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:no> ?missing } }').read_all()
    pandas_frame = table.to_pandas(types_mapper=pd.ArrowDtype)
    polars_frame = pl.from_arrow(table, rechunk=False)
    assert pandas_frame.to_dict("records") == table.to_pylist()
    assert polars_frame.to_arrow().to_pylist() == table.to_pylist()
    db.close()


def test_native_columns_and_identity_fallback(tmp_path):
    import decimal
    db = rdfdb.Database(tmp_path / "db")
    integer = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "01"^^<http://www.w3.org/2001/XMLSchema#integer> "2"^^<http://www.w3.org/2001/XMLSchema#integer> } }', types="native").read_all()
    assert pa.types.is_int64(integer.schema.field("x").type)
    assert sorted(integer.column("x").to_pylist()) == [1, 2]
    exact = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "01"^^<http://www.w3.org/2001/XMLSchema#integer> } }').read_all()
    assert exact.column("x").to_pylist()[0]["value"] == "01"
    decimals = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "999.1"^^<http://www.w3.org/2001/XMLSchema#decimal> "0.001"^^<http://www.w3.org/2001/XMLSchema#decimal> } }', types="native").read_all()
    assert pa.types.is_decimal128(decimals.schema.field("x").type)
    assert sorted(decimals.column("x").to_pylist()) == [decimal.Decimal("0.001"), decimal.Decimal("999.100")]
    floats = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "1.5"^^<http://www.w3.org/2001/XMLSchema#float> "-0"^^<http://www.w3.org/2001/XMLSchema#float> } }', types="native").read_all()
    assert pa.types.is_float32(floats.schema.field("x").type)
    mixed = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "1"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:o> } }', types="native").read_all()
    assert pa.types.is_struct(mixed.schema.field("x").type)
    out_of_range = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "999999999999999999999999999999999999999"^^<http://www.w3.org/2001/XMLSchema#integer> } }', types="native").read_all()
    assert pa.types.is_struct(out_of_range.schema.field("x").type)
    with pytest.raises(ValueError): db.query_arrow('SELECT ?s WHERE { ?s ?p ?o }', types="bad")
    db.close()


def test_native_date_and_datetime_fallback(tmp_path):
    db = rdfdb.Database(tmp_path / "db")
    date = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "2024-02-29"^^<http://www.w3.org/2001/XMLSchema#date> } }', types="native").read_all()
    assert pa.types.is_date32(date.schema.field("x").type)
    timestamp = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "2024-02-29T12:01:02Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> } }', types="native").read_all()
    assert pa.types.is_timestamp(timestamp.schema.field("x").type)
    mixed = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "2024-02-29T12:01:02Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> "2024-02-29T12:01:02"^^<http://www.w3.org/2001/XMLSchema#dateTime> } }', types="native").read_all()
    assert pa.types.is_struct(mixed.schema.field("x").type)
    db.close()


def test_native_parquet_export(tmp_path):
    import pyarrow.parquet as pq
    db = rdfdb.Database(tmp_path / "db")
    path = tmp_path / "numeric.parquet"
    db.export_parquet('SELECT ?x WHERE { VALUES ?x { "42"^^<http://www.w3.org/2001/XMLSchema#integer> } }', path, types="native")
    result = pq.read_table(path)
    assert pa.types.is_int64(result.schema.field("x").type)
    assert result.column("x").to_pylist() == [42]
    db.close()


def test_native_invalid_boundaries_and_unbound(tmp_path):
    import math
    db = rdfdb.Database(tmp_path / "db")
    xsd = "http://www.w3.org/2001/XMLSchema#"
    def column(values, datatype):
        terms = " ".join(f'"{v}"^^<{xsd}{datatype}>' for v in values)
        return db.query_arrow(f'SELECT ?x WHERE {{ VALUES ?x {{ {terms} }} }}', types="native").read_all().column("x")
    assert pa.types.is_struct(column(["oops"], "integer").type)
    assert pa.types.is_struct(column(["1e2"], "decimal").type)
    assert pa.types.is_struct(column(["1" * 39 + ".1"], "decimal").type)
    assert pa.types.is_struct(column(["1.2e999999999999999999999999"], "decimal").type)
    vals = column(["INF", "-INF", "NaN", "-0", "+INF"], "float").to_pylist()
    assert math.isinf(vals[0]) and math.isinf(vals[1]) and math.isnan(vals[2])
    assert math.copysign(1.0, vals[3]) < 0 and vals[4] == math.inf
    assert pa.types.is_struct(column(["2024-01-01T00:00:00.1234567Z"], "dateTime").type)
    assert pa.types.is_struct(column(["2024-01-01T00:00:00+14:30"], "dateTime").type)
    assert pa.types.is_struct(column(["2023-02-29"], "date").type)
    table = db.query_arrow('SELECT ?x WHERE { VALUES ?x { "2"^^<http://www.w3.org/2001/XMLSchema#integer> UNDEF } }', types="native").read_all()
    assert pa.types.is_int64(table.column("x").type)
    assert table.column("x").to_pylist() == [2, None]
    db.close()


def test_native_matches_python_reference(tmp_path):
    from native_reference import _native_arrow
    db = rdfdb.Database(tmp_path / "db")
    xsd = "http://www.w3.org/2001/XMLSchema#"
    cases = [
        ("integer", ["+1", "-0", "9223372036854775807"]),
        ("integer", ["9223372036854775808"]),
        ("decimal", ["999.1", "0.001", "-0.000"]),
        ("decimal", ["9" * 39]),
        ("float", ["+INF", "-INF", "-0", "1.5"]),
        ("date", ["2024-02-29"]),
        ("dateTime", ["2024-02-29T12:01:02Z", "2024-02-29T12:01:02+01:00"]),
        ("dateTime", ["2024-02-29T12:01:02Z", "2024-02-29T12:01:02"]),
        ("int", ["1", "01"]),
    ]
    for datatype, values in cases:
        terms = " ".join(f'"{v}"^^<{xsd}{datatype}>' for v in values)
        query = f'SELECT ?x WHERE {{ VALUES ?x {{ {terms} }} }}'
        expected = _native_arrow(db.query_arrow(query)).read_all()
        actual = db.query_arrow(query, types="native").read_all()
        assert actual.equals(expected), (datatype, values, actual, expected)
    db.close()
