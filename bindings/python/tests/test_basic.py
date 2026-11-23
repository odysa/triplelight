import tempfile, os
import pytest
import triplelight

def test_roundtrip_and_types():
    d = tempfile.mkdtemp()
    path = os.path.join(d, "db")
    with triplelight.Database(path) as db:
        n = db.update('''INSERT DATA {
            <urn:a> <urn:age> 30 . <urn:a> <urn:name> "Alice" . <urn:a> <urn:label> "x"@en .
            <urn:a> <urn:knows> <urn:b> . <urn:a> <urn:score> "1.5"^^<http://www.w3.org/2001/XMLSchema#double> }''')
        assert n == 5
        r = db.query("SELECT ?p ?o WHERE { <urn:a> ?p ?o }")
        got = {row["p"]: row["o"] for row in r}
        assert got[triplelight.IRI("urn:age")] == 30
        assert got["urn:name"] == "Alice"
        assert got["urn:label"] == triplelight.Literal("x", None, "en")
        assert got["urn:knows"] == triplelight.IRI("urn:b")
        assert got["urn:score"] == triplelight.Literal("1.5", "http://www.w3.org/2001/XMLSchema#double")
        assert db.query("ASK { <urn:a> <urn:knows> <urn:b> }")
    # persistence across reopen
    with triplelight.Database(path) as db:
        assert len(db.query("SELECT * WHERE { ?s ?p ?o }")) == 5

def test_snapshot_isolation_and_errors():
    d = tempfile.mkdtemp()
    db = triplelight.Database(os.path.join(d, "db"))
    db.update("INSERT DATA { <urn:a> <urn:p> 1 }")
    with db.read() as snap:
        db.update("INSERT DATA { <urn:a> <urn:p> 2 }")
        assert len(snap.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")) == 1
    assert len(db.query("SELECT ?o WHERE { <urn:a> <urn:p> ?o }")) == 2
    try:
        db.query("SELEC nonsense")
        assert False
    except triplelight.SyntaxError:
        pass

def test_named_graphs_and_graph_state_errors():
    d = tempfile.mkdtemp()
    db = triplelight.Database(os.path.join(d, "db"))
    db.update("INSERT DATA { GRAPH <urn:g> { <urn:a> <urn:p> 1 } } ; COPY <urn:g> TO <urn:h>")
    assert len(db.query("SELECT ?o WHERE { GRAPH <urn:h> { <urn:a> <urn:p> ?o } }")) == 1
    try:
        db.update("CREATE GRAPH <urn:g>")
        assert False
    except triplelight.GraphStateError as e:
        assert isinstance(e, triplelight.TripleLightError)

def test_lexical_identity_and_unbound():
    d = tempfile.mkdtemp()
    db = triplelight.Database(os.path.join(d, "db"))
    db.update('INSERT DATA { <urn:a> <urn:v> "1"^^<http://www.w3.org/2001/XMLSchema#int> . '
              '<urn:a> <urn:v> "01"^^<http://www.w3.org/2001/XMLSchema#int> . <urn:a> <urn:e> "" }')
    vs = {row["v"] for row in db.query("SELECT ?v WHERE { <urn:a> <urn:v> ?v }")}
    assert len(vs) == 2   # distinct RDF terms stay distinct
    r = db.query("SELECT ?e ?z WHERE { <urn:a> <urn:e> ?e OPTIONAL { <urn:a> <urn:none> ?z } }")
    assert r.rows[0]["e"] == "" and r.rows[0]["z"] is None

def test_lock_load_and_json():
    d = tempfile.mkdtemp()
    path = os.path.join(d, "db")
    db = triplelight.Database(path)
    try:
        triplelight.Database(path)
        assert False, "second open must fail"
    except triplelight.LockedError:
        pass
    nt = os.path.join(d, "x.nt")
    open(nt, "w").write('<urn:s> <urn:p> "v" .\n<urn:s> <urn:q> <urn:o> .\n')
    assert db.load(nt) == 2
    import json
    j = json.loads(db.query_json("SELECT ?p WHERE { <urn:s> ?p ?o }"))
    assert len(j["results"]["bindings"]) == 2


def test_rdf12_triple_terms_and_direction():
    """Needs a build with the `rdf-12` feature; a plain build refuses the 1.2 syntax and the test skips."""
    db = triplelight.Database.in_memory()
    try:
        db.update('INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> "v" )>> . <urn:a> <urn:label> "שלום"@he--rtl }')
    except triplelight.TripleLightError:
        if os.environ.get("TRIPLELIGHT_EXPECT_RDF12"):
            raise            # the build is supposed to have rdf-12: a refusal is a failure, not a skip
        pytest.skip("built without the rdf-12 feature")
    rows = {row["p"]: row["o"] for row in db.query("SELECT ?p ?o WHERE { <urn:a> ?p ?o }")}
    t = rows["urn:says"]
    assert isinstance(t, triplelight.Triple)
    assert t.subject == triplelight.IRI("urn:b") and t.predicate == triplelight.IRI("urn:p") and t.object == "v"
    assert rows["urn:label"] == triplelight.Literal("שלום", None, "he", "rtl")
