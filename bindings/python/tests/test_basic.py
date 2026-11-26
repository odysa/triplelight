import os, signal, tempfile, threading, time
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


def test_rdfs_entailment_is_opt_in():
    db = triplelight.Database.in_memory()
    db.update("INSERT DATA { <urn:ann> a <urn:Student> . <urn:Student> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Person> }")
    q = "SELECT ?x WHERE { ?x a <urn:Person> }"
    assert len(db.query(q)) == 0
    if not next(entry.compiled_in for entry in db.list_extensions() if entry.name == "rdfs"):
        with pytest.raises(triplelight.UnsupportedError):
            db.query(q, entailment="rdfs")
        return
    assert [row["x"] for row in db.query(q, entailment="rdfs")] == [triplelight.IRI("urn:ann")]
    assert "urn:ann" in db.query_json(q, entailment="rdfs")
    with db.read() as snap:
        assert len(snap.query(q, entailment="rdfs")) == 1
    with pytest.raises(ValueError):
        db.query(q, entailment="owl")


SHAPES = """@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ; sh:property [ sh:path <urn:name> ; sh:minCount 1 ] ."""

def test_shacl_validate_reports_and_errors():
    with triplelight.Database.in_memory() as db:
        shacl = next(entry for entry in db.list_extensions() if entry.name == "shacl")
    if not shacl.compiled_in:
        with pytest.raises(triplelight.UnsupportedError):
            triplelight.validate("", SHAPES)
        return
    ok = triplelight.validate("<urn:ann> a <urn:Person> ; <urn:name> \"Ann\" .", SHAPES)
    assert ok.conforms and bool(ok)
    bad = triplelight.validate("<urn:bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .", SHAPES, data_format="ntriples")
    assert not bad.conforms
    focus = [o for s, p, o in bad.triples if p == triplelight.IRI("http://www.w3.org/ns/shacl#focusNode")]
    assert focus == [triplelight.IRI("urn:bob")]
    with pytest.raises(triplelight.SyntaxError):
        triplelight.validate("<urn:bob> a", SHAPES)
    with pytest.raises(triplelight.ShapesError):
        triplelight.validate("", "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:S> a sh:NodeShape ; sh:targetClass <urn:P> ; sh:property [ sh:minCount 1 ] .")


def test_extension_registry_load_and_autoload():
    with triplelight.Database.in_memory() as db:
        before = {entry.name: entry for entry in db.list_extensions()}
        for name in ("rdfs", "shacl"):
            assert not before[name].loaded
            if before[name].compiled_in:
                if name == "rdfs":
                    assert db.query("ASK {}", entailment="rdfs")
                db.load_extension(name)
                db.load_extension(name)
            else:
                with pytest.raises(triplelight.UnsupportedError, match=name):
                    db.load_extension(name)
        after = {entry.name: entry for entry in db.list_extensions()}
        for name in ("rdfs", "shacl"):
            assert after[name].loaded == before[name].compiled_in
        with pytest.raises(triplelight.UnsupportedError, match="unknown extension"):
            db.load_extension("missing")


def _big(n=2000):
    db = triplelight.Database.in_memory()
    db.update("INSERT DATA { " + " ".join(f'<urn:s{i}> <urn:p> "{i}" .' for i in range(n)) + " }")
    return db

ENDLESS = "SELECT (COUNT(*) AS ?n) { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f FILTER(?b != ?d && ?d != ?f) }"
GROUPED = "SELECT ?a (COUNT(*) AS ?n) { ?a ?p ?b } GROUP BY ?a"
# Four million groups: far more than one megabyte of working memory.
WIDE = "SELECT ?a ?c (COUNT(*) AS ?n) { ?a ?p ?b . ?c ?q ?d } GROUP BY ?a ?c"


def test_query_timeout_and_memory_budget():
    db = _big()
    start = time.monotonic()
    with pytest.raises(triplelight.QueryTimeout):
        db.query(ENDLESS, timeout=0.05)
    assert time.monotonic() - start < 10
    with pytest.raises(triplelight.ResourceLimitError):
        db.query_json(WIDE, memory_mb=1)
    with db.read() as snap:
        with pytest.raises(triplelight.QueryTimeout):
            snap.query_json(ENDLESS, timeout=0.05)
    assert len(db.query(GROUPED, timeout=60, memory_mb=1024)) == 2000
    assert issubclass(triplelight.QueryTimeout, triplelight.TripleLightError)
    with pytest.raises(ValueError):
        db.query(GROUPED, timeout=-1)
    with pytest.raises(ValueError):
        db.query(GROUPED, memory_mb=0)


def test_spill_lets_a_group_by_over_the_budget_finish(tmp_path, monkeypatch):
    # 10,000 groups of 4 rows: grouping in memory does not fit 3 MB; spilling by key partition does (#111)
    data = tmp_path / "d.nt"
    data.write_text("".join(f'<http://ex/s{i // 4}> <http://ex/p> "v{i}" .\n' for i in range(40_000)))
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    monkeypatch.setenv("TRIPLELIGHT_SCRATCH_DIR", str(scratch))
    db = triplelight.Database.in_memory()
    db.load(data)
    q = "SELECT ?s (GROUP_CONCAT(?o) AS ?all) WHERE { ?s <http://ex/p> ?o } GROUP BY ?s"
    with pytest.raises(triplelight.ResourceLimitError):
        db.query(q, memory_mb=3)
    assert len(db.query(q, memory_mb=3, spill=True)) == 10_000
    assert len(db.query(q)) == 10_000
    assert list(scratch.iterdir()) == [], "no spill files are left behind"
    with pytest.raises(ValueError):
        db.query(q, spill=True)


def test_keyboard_interrupt_cancels_a_running_query():
    db = _big()
    # A real SIGINT, as Ctrl-C sends: the chained action cancels the query, and Python still raises.
    threading.Timer(0.1, os.kill, (os.getpid(), signal.SIGINT)).start()
    start = time.monotonic()
    with pytest.raises(KeyboardInterrupt):
        db.query(ENDLESS)
    assert time.monotonic() - start < 10
    # The worker stopped: the database answers at once.
    assert db.query("ASK { <urn:s1> <urn:p> \"1\" }").boolean


def test_ctrl_c_cancellation_can_be_turned_off():
    db = _big()
    triplelight.set_cancel_on_ctrl_c(False)
    try:
        # Not watched: the query runs to its own timeout, and Python raises the KeyboardInterrupt after.
        threading.Timer(0.05, os.kill, (os.getpid(), signal.SIGINT)).start()
        with pytest.raises(KeyboardInterrupt):
            try:
                db.query(ENDLESS, timeout=0.5)
            except triplelight.QueryTimeout:
                time.sleep(0.01)  # the pending signal is raised here
    finally:
        triplelight.set_cancel_on_ctrl_c(True)
