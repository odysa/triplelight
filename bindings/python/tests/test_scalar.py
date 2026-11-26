import gc
import json
import os
import time
import weakref
from concurrent.futures import ThreadPoolExecutor

import pytest
import triplelight


def test_scalar_registration_lifetime_and_worker_threads():
    db = triplelight.Database.in_memory()
    old = db.read()
    query = "SELECT (<urn:example:inc>(41) AS ?value) WHERE {}"

    def increment(value):
        return value + 1

    callback = weakref.ref(increment)
    db.register_function("urn:example:inc", increment, arity=1, pure=True)
    with pytest.raises(ValueError):
        db.register_function("urn:example:inc", increment)
    with pytest.raises(ValueError):
        db.register_function("relative", increment)
    with pytest.raises(TypeError):
        db.register_function("urn:example:invalid", 42)
    with pytest.raises(triplelight.UnsupportedError):
        old.query(query)

    current = db.read()
    del increment
    db.close()
    assert callback() is not None
    with ThreadPoolExecutor(max_workers=2) as executor:
        queries = [executor.submit(current.query, query), executor.submit(current.query_json, query)]
        assert queries[0].result().rows[0]["value"] == 42
        assert json.loads(queries[1].result())["results"]["bindings"][0]["value"]["value"] == "42"
    del current
    gc.collect()
    assert callback() is None


def test_scalar_round_trip_for_python_rdf_values():
    db = triplelight.Database.in_memory()
    db.update('''INSERT DATA {
      <urn:s> <urn:p> <urn:value>, _:id, "", "01"^^<http://www.w3.org/2001/XMLSchema#int>,
        "bonjour"@fr, true, 7, 9223372036854775808 .
    }''')
    db.register_function("urn:example:id", lambda value: value, arity=1, pure=True)
    rows = db.query("SELECT ?x (<urn:example:id>(?x) AS ?y) WHERE { <urn:s> <urn:p> ?x }")
    assert len(rows) == 8
    for original, result in rows:
        assert type(original) is type(result)
        assert original == result


def test_scalar_errors_are_sparql_expression_errors():
    db = triplelight.Database.in_memory()

    def fail(kind):
        if kind == 0:
            raise RuntimeError("callback failed")
        if kind == 1:
            return None
        if kind == 2:
            return object()
        return triplelight.IRI("relative")

    db.register_function("urn:example:fail", fail, arity=1, pure=True)
    result = db.query("""SELECT ?x ?y WHERE {
      VALUES ?x { 0 1 1 2 3 }
      BIND(<urn:example:fail>(?x) AS ?y)
    } ORDER BY ?x""")
    assert list(result) == [(0, None), (1, None), (1, None), (2, None), (3, None)]
    assert db.query("ASK { FILTER(COALESCE(<urn:example:fail>(), true)) }")
    assert db.query("ASK { FILTER(true || <urn:example:fail>(0)) }")
    assert not db.query("ASK { FILTER(false && <urn:example:fail>(0)) }")
    db.register_function("urn:example:id", lambda value: value, arity=1)
    assert db.query("ASK { FILTER EXISTS { FILTER(<urn:example:id>(1) = 1) } }")


def test_scalar_rdf12_triple_and_direction():
    db = triplelight.Database.in_memory()
    try:
        db.update('INSERT DATA { <urn:s> <urn:p> <<( <urn:b> <urn:q> "שלום"@he--rtl )>> }')
    except triplelight.TripleLightError:
        if os.environ.get("TRIPLELIGHT_EXPECT_RDF12"):
            raise
        pytest.skip("built without the rdf-12 feature")

    db.register_function("urn:example:id", lambda value: value, arity=1)
    row, = db.query("SELECT (<urn:example:id>(?x) AS ?y) WHERE { <urn:s> <urn:p> ?x }")
    assert row["y"] == triplelight.Triple(
        triplelight.IRI("urn:b"), triplelight.IRI("urn:q"), triplelight.Literal("שלום", None, "he", "rtl")
    )
    cyclic = triplelight.Triple(triplelight.IRI("urn:b"), triplelight.IRI("urn:q"), "value")
    object.__setattr__(cyclic, "object", cyclic)
    db.register_function("urn:example:cyclic", lambda: cyclic, arity=0)
    row, = db.query("SELECT (<urn:example:cyclic>() AS ?y) WHERE {}")
    assert row["y"] is None


def test_scalar_deadline_during_callback():
    db = triplelight.Database.in_memory()
    completed = []

    def slow():
        time.sleep(0.3)
        completed.append(True)
        return True

    db.register_function("urn:example:slow", slow, arity=0)
    with pytest.raises(triplelight.QueryTimeout):
        db.query("ASK { FILTER(<urn:example:slow>()) }", timeout=0.2)
    assert completed == [True]
    assert db.query("ASK {}")
