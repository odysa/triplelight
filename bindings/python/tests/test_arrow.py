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
    with pytest.raises(rdfdb.RdfdbError, match="SELECT"):
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
