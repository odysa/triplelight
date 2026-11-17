import tempfile, os
import rdfdb

def test_roundtrip_and_types():
    d = tempfile.mkdtemp()
    path = os.path.join(d, "db")
    with rdfdb.Database(path) as db:
        n = db.update('''INSERT DATA {
            <urn:a> <urn:age> 30 . <urn:a> <urn:name> "Alice" . <urn:a> <urn:label> "x"@en .
            <urn:a> <urn:knows> <urn:b> . <urn:a> <urn:score> "1.5"^^<http://www.w3.org/2001/XMLSchema#double> }''')
        assert n == 5
        r = db.query("SELECT ?p ?o WHERE { <urn:a> ?p ?o }")
        got = {row["p"]: row["o"] for row in r}
        assert got[rdfdb.IRI("urn:age")] == 30
        assert got["urn:name"] == "Alice"
        assert got["urn:label"] == rdfdb.Literal("x", None, "en")
        assert got["urn:knows"] == rdfdb.IRI("urn:b")
        assert got["urn:score"] == rdfdb.Literal("1.5", "http://www.w3.org/2001/XMLSchema#double")
        assert db.query("ASK { <urn:a> <urn:knows> <urn:b> }")
    # persistence across reopen
    with rdfdb.Database(path) as db:
        assert len(db.query("SELECT * WHERE { ?s ?p ?o }")) == 5

def test_snapshot_isolation_and_errors():
    d = tempfile.mkdtemp()
    db = rdfdb.Database(os.path.join(d, "db"))
    db.update("INSERT DATA { <urn:a> <urn:p> 1 }")
    with db.read() as snap:
        db.update("INSERT DATA { <urn:a> <urn:p> 2 }")
        assert len(snap.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")) == 1
    assert len(db.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")) == 2
    try:
        db.query("SELEC nonsense")
        assert False
    except rdfdb.SyntaxError:
        pass

def test_lexical_identity_and_unbound():
    d = tempfile.mkdtemp()
    db = rdfdb.Database(os.path.join(d, "db"))
    db.update('INSERT DATA { <urn:a> <urn:v> "1"^^<http://www.w3.org/2001/XMLSchema#int> . '
              '<urn:a> <urn:v> "01"^^<http://www.w3.org/2001/XMLSchema#int> . <urn:a> <urn:e> "" }')
    vs = {row["v"] for row in db.query("SELECT ?v WHERE { <urn:a> <urn:v> ?v }")}
    assert len(vs) == 2   # distinct RDF terms stay distinct
    r = db.query("SELECT ?e ?z WHERE { <urn:a> <urn:e> ?e OPTIONAL { <urn:a> <urn:none> ?z } }")
    assert r.rows[0]["e"] == "" and r.rows[0]["z"] is None
