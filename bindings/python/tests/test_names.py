"""Both import names resolve to the same engine and exception types."""
import rdfdb
import triplelight


def test_renamed_import_and_compatibility_alias(tmp_path):
    assert rdfdb.Database is triplelight.Database
    assert rdfdb.LockedError is triplelight.LockedError
    with triplelight.Database(tmp_path / "store") as db:
        db.update('INSERT DATA { <urn:s> <urn:p> "v" }')
        assert db.query('ASK { <urn:s> <urn:p> "v" }')
