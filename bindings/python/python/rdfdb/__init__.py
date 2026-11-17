"""Embedded persistent RDF database with SPARQL 1.1 queries and updates.

    import rdfdb
    db = rdfdb.Database("my.db")                     # opens or creates; one process per directory
    db.update('INSERT DATA { <urn:a> <urn:p> "x" }')
    for row in db.query("SELECT ?s ?o WHERE { ?s <urn:p> ?o }"):
        print(row["s"], row["o"])
    with db.read() as snap:                          # consistent view across several queries
        snap.query(...)
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Iterator, Optional

from . import _rdfdb
from ._rdfdb import RdfdbError, StorageError, SyntaxError, UnsupportedError

__all__ = ["Database", "Snapshot", "IRI", "BNode", "Literal", "Result",
           "RdfdbError", "SyntaxError", "UnsupportedError", "StorageError"]

XSD = "http://www.w3.org/2001/XMLSchema#"
_INT = {XSD + t for t in ("integer", "int", "long", "short", "byte", "nonNegativeInteger", "positiveInteger",
                          "negativeInteger", "nonPositiveInteger", "unsignedInt", "unsignedLong", "unsignedShort",
                          "unsignedByte")}
_FLOAT = {XSD + "double", XSD + "float"}


class IRI(str):
    """An IRI (named node)."""
    def __repr__(self) -> str: return f"IRI({str.__repr__(self)})"


class BNode(str):
    """A blank node label (only stable within one result)."""
    def __repr__(self) -> str: return f"BNode({str.__repr__(self)})"


@dataclass(frozen=True)
class Literal:
    """A literal that has no lossless native Python value (language-tagged, decimal, date, custom types)."""
    value: str
    datatype: Optional[str] = None
    lang: Optional[str] = None


def _term(t):
    if t is None:
        return None
    kind, value, datatype, lang = t
    if kind == 0:
        return IRI(value)
    if kind == 1:
        return BNode(value)
    if lang is not None:
        return Literal(value, None, lang)
    if datatype in (None, XSD + "string"):
        return value
    try:
        if datatype in _INT:
            return int(value)
        if datatype in _FLOAT:
            return float(value)
        if datatype == XSD + "boolean":
            return value in ("true", "1")
    except ValueError:
        pass
    return Literal(value, datatype, None)


class Row(tuple):
    """A solution: a tuple in `Result.vars` order that also supports `row["var"]`."""
    _index: dict

    def __getitem__(self, key):
        if isinstance(key, str):
            return tuple.__getitem__(self, self._index[key])
        return tuple.__getitem__(self, key)


class Result:
    """Query result. SELECT: iterate rows (or `.rows`), `.vars`. ASK: `bool(result)`. CONSTRUCT: `.triples`."""

    def __init__(self, raw):
        self.kind = raw[0]
        self.vars: list[str] = []
        self.rows: list[Row] = []
        self.triples: list[tuple] = []
        self.boolean: Optional[bool] = None
        if self.kind == "select":
            self.vars = list(raw[1])
            index = {v: i for i, v in enumerate(self.vars)}
            cls = type("Row", (Row,), {"_index": index})
            self.rows = [cls(_term(t) for t in r) for r in raw[2]]
        elif self.kind == "ask":
            self.boolean = bool(raw[1])
        else:
            self.triples = [tuple(_term(t) for t in tr) for tr in raw[1]]

    def __iter__(self) -> Iterator:
        return iter(self.rows if self.kind == "select" else self.triples)

    def __len__(self) -> int:
        return len(self.rows if self.kind == "select" else self.triples)

    def __bool__(self) -> bool:
        return self.boolean if self.kind == "ask" else len(self) > 0

    def __repr__(self) -> str:
        if self.kind == "ask":
            return f"Result(ask={self.boolean})"
        return f"Result({self.kind}, {len(self)} rows, vars={self.vars})"


class Snapshot:
    """An immutable view of the database; unaffected by later updates."""

    def __init__(self, raw): self._raw = raw
    def query(self, sparql: str) -> Result: return Result(self._raw.query(sparql))
    def __enter__(self): return self
    def __exit__(self, *exc): return False


class Database:
    """A persistent RDF database in a local directory (created if missing).

    `initial`: an N-Triples file loaded only when the directory holds no database yet.
    """

    def __init__(self, path, initial=None):
        self._db = _rdfdb.Database(str(path), None if initial is None else str(initial))

    def query(self, sparql: str) -> Result:
        """Run a SPARQL SELECT / ASK / CONSTRUCT query on the current state."""
        return Result(self._db.query(sparql))

    def update(self, sparql: str) -> int:
        """Apply a SPARQL Update atomically and durably (fsync before return). Returns changed triples."""
        return self._db.update(sparql)

    def read(self) -> Snapshot:
        """A consistent snapshot for several queries."""
        return Snapshot(self._db.read())

    def compact(self) -> None:
        """Merge updates into a new immutable base; readers keep their snapshots."""
        self._db.compact()

    def close(self) -> None:
        """Release the database (and its directory lock) once no snapshots remain."""
        self._db = None

    def __enter__(self): return self
    def __exit__(self, *exc): self.close(); return False
