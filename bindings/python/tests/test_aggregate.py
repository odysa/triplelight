from concurrent.futures import ThreadPoolExecutor

import pytest
import triplelight


class Count:
    def __init__(self):
        self.total = 0

    def step(self, value):
        self.total += 1

    def finish(self):
        return self.total


def test_aggregate_groups_empty_input_and_snapshot():
    db = triplelight.Database.in_memory()
    old = db.read()
    db.register_aggregate_function("urn:py:count", Count)
    with pytest.raises(ValueError):
        db.register_aggregate_function("urn:py:count", Count)
    query = """SELECT ?g (<urn:py:count>(DISTINCT ?v) AS ?n) WHERE {
        VALUES (?g ?v) { (1 "x") (1 "x") (1 UNDEF) (2 "y") (2 "z") }
    } GROUP BY ?g ORDER BY ?g"""
    with pytest.raises(triplelight.UnsupportedError):
        old.query(query)
    current = db.read()
    db.close()
    with ThreadPoolExecutor(max_workers=2) as executor:
        futures = [executor.submit(current.query, query) for _ in range(2)]
        assert [list(future.result()) for future in futures] == [[(1, 1), (2, 2)]] * 2
    assert list(current.query("SELECT (<urn:py:count>(?v) AS ?n) WHERE { FILTER(false) }")) == [(0,)]


def test_aggregate_callback_failures_leave_unbound_results():
    db = triplelight.Database.in_memory()

    def fail_factory():
        raise ValueError("create failed")

    class FailStep(Count):
        def step(self, value):
            raise ValueError("step failed")

    class FailFinish(Count):
        def finish(self):
            return object()

    db.register_aggregate_function("urn:py:create", fail_factory)
    db.register_aggregate_function("urn:py:step", FailStep)
    db.register_aggregate_function("urn:py:finish", FailFinish)
    result = db.query("""SELECT (<urn:py:create>(?v) AS ?a) (<urn:py:step>(?v) AS ?b)
        (<urn:py:finish>(?v) AS ?c) WHERE { VALUES ?v { "x" } }""")
    assert list(result) == [(None, None, None)]
