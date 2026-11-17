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
from ._rdfdb import LockedError, RdfdbError, StorageError, SyntaxError, UnsupportedError

__all__ = ["Database", "Snapshot", "IRI", "BNode", "Literal", "Result",
           "RdfdbError", "SyntaxError", "UnsupportedError", "StorageError", "LockedError"]



class IRI(str):
    """An IRI (named node)."""
    def __repr__(self) -> str: return f"IRI({str.__repr__(self)})"


class BNode(str):
    """A blank node label (only stable within one result)."""
    def __repr__(self) -> str: return f"BNode({str.__repr__(self)})"


@dataclass(frozen=True)
class Literal:
    """Any literal without a lossless native Python value: language-tagged, non-canonical or derived integer types
    (xsd:int, "01"), decimal/double (whose lexical form a float would lose), dates, custom types. `value` is the
    exact lexical form. Unbound variables are `None`, never an empty string."""
    value: str
    datatype: Optional[str] = None
    lang: Optional[str] = None


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
            self.rows = [cls(r) for r in raw[2]]
        elif self.kind == "ask":
            self.boolean = bool(raw[1])
        else:
            self.triples = list(raw[1])

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


def _native_arrow(reader):
    """Opt-in value view; columns that cannot be safely coerced remain RDF structs."""
    import datetime
    import decimal
    import math
    import re
    import struct
    import pyarrow as pa
    import pyarrow.compute as pc

    table = reader.read_all()  # Current engine already materializes the result.
    arrays = []
    for column in table.columns:
        kinds = pc.struct_field(column, "kind").to_pylist()
        datatypes = pc.struct_field(column, "datatype").to_pylist()
        languages = pc.struct_field(column, "language").to_pylist()
        values = pc.struct_field(column, "value").to_pylist()
        bound = [(k, d, l, v) for k, d, l, v in zip(kinds, datatypes, languages, values) if k is not None]
        if not bound or any(k != "literal" or l is not None for k, _, l, _ in bound):
            arrays.append(column); continue
        types = {d for _, d, _, _ in bound}
        if len(types) != 1:
            arrays.append(column); continue
        datatype = next(iter(types))
        prefix = "http://www.w3.org/2001/XMLSchema#"
        if not datatype or not datatype.startswith(prefix):
            arrays.append(column); continue
        datatype = datatype[len(prefix):]
        try:
            if datatype == "string":
                out = pa.array(values, type=pa.large_string())
            elif datatype == "integer":
                if any(v is not None and not re.fullmatch(r"[+-]?[0-9]+", v) for v in values):
                    raise ValueError("invalid integer")
                integers = [None if v is None else int(v) for v in values]
                if any(v is not None and not -(1 << 63) <= v < (1 << 63) for v in integers):
                    raise ValueError("integer exceeds int64")
                out = pa.array(integers, type=pa.int64())
            elif datatype == "decimal":
                if any(v is not None and not re.fullmatch(r"[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)", v) for v in values):
                    raise ValueError("invalid decimal")
                decimals = [None if v is None else decimal.Decimal(v) for v in values]
                scale = max((max(-v.as_tuple().exponent, 0) for v in decimals if v is not None), default=0)
                integer_digits = max((max(v.adjusted() + 1, 0) for v in decimals if v is not None), default=0)
                precision = max(integer_digits + scale, 1)
                if precision > 38: raise ValueError("decimal exceeds Decimal128")
                out = pa.array(decimals, type=pa.decimal128(precision, scale))
            elif datatype in ("float", "double"):
                pattern = r"[+-]?(?:(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)"
                def parse_float(v):
                    if v is None: return None
                    if v in ("INF", "-INF", "NaN"): return {"INF": math.inf, "-INF": -math.inf, "NaN": math.nan}[v]
                    if not re.fullmatch(pattern, v): raise ValueError("invalid float")
                    f = float(v)
                    if datatype == "float":
                        try: f = struct.unpack("!f", struct.pack("!f", f))[0]
                        except OverflowError: f = math.copysign(math.inf, f)
                    return f
                out = pa.array([parse_float(v) for v in values], type=pa.float32() if datatype == "float" else pa.float64())
            elif datatype == "date":
                if any(v is not None and not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", v) for v in values): raise ValueError("unsupported date")
                out = pa.array([None if v is None else datetime.date.fromisoformat(v) for v in values], type=pa.date32())
            elif datatype == "dateTime":
                # Arrow microseconds cannot preserve greater-than-microsecond precision; fall back.
                pattern = r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,6})?(?:Z|[+-][0-9]{2}:[0-9]{2})?"
                if any(v is not None and not re.fullmatch(pattern, v) for v in values): raise ValueError("unsupported datetime")
                for v in values:
                    if v is not None and (zone := re.search(r"([+-])([0-9]{2}):([0-9]{2})$", v)):
                        hours, minutes = int(zone.group(2)), int(zone.group(3))
                        if hours > 14 or minutes > 59 or (hours == 14 and minutes): raise ValueError("invalid XSD timezone")
                parsed = [None if v is None else datetime.datetime.fromisoformat(v.replace("Z", "+00:00")) for v in values]
                zones = {v.tzinfo is not None for v in parsed if v is not None}
                if len(zones) != 1: raise ValueError("mixed timezone presence")
                out = pa.array(parsed, type=pa.timestamp("us", tz="UTC" if True in zones else None))
            else:
                arrays.append(column); continue
            arrays.append(out)
        except (ValueError, OverflowError, decimal.InvalidOperation, pa.ArrowException):
            arrays.append(column)
    return pa.Table.from_arrays(arrays, names=table.column_names).to_reader(max_chunksize=8192)


def _arrow_types(reader, types):
    if types == "rdf": return reader
    if types == "native": return _native_arrow(reader)
    raise ValueError("types must be 'rdf' or 'native'")


def _atomic_export(reader, destination, write):
    """Write to a temporary sibling and publish only after successful completion."""
    import os
    import tempfile
    path = os.fspath(destination)
    directory = os.path.dirname(os.path.abspath(path))
    fd, temporary = tempfile.mkstemp(prefix=".rdfdb-export-", dir=directory)
    os.close(fd)
    try:
        write(reader, temporary)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def _write_parquet(reader, path):
    import pyarrow.parquet as pq
    with pq.ParquetWriter(path, reader.schema) as writer:
        for batch in reader:
            writer.write_batch(batch)


def _write_csv(reader, path):
    import csv
    names = reader.schema.names
    with open(path, "w", encoding="utf-8", newline="") as file:
        writer = csv.writer(file)
        writer.writerow([f"{name}.{field}" for name in names for field in ("kind", "value", "datatype", "language")])
        for batch in reader:
            # kind is empty exactly for unbound, so empty lexical literals stay distinct.
            columns = [batch.column(i).to_pylist() for i in range(batch.num_columns)]
            for row_number in range(batch.num_rows):
                fields = []
                for column in columns:
                    value = column[row_number]
                    fields.extend(("", "", "", "") if value is None else
                                  (value["kind"], value["value"], value["datatype"] or "", value["language"] or ""))
                writer.writerow(fields)


class Snapshot:
    """An immutable view of the database; unaffected by later updates."""

    def __init__(self, raw): self._raw = raw
    def query(self, sparql: str) -> Result: return Result(self._raw.query(sparql))
    def query_json(self, sparql: str) -> str: return self._raw.query_json(sparql)
    def query_arrow(self, sparql: str, *, types="rdf"):
        """Return Arrow batches, with optional value-oriented native columns."""
        return _arrow_types(self._raw.query_arrow(sparql), types)
    def export_parquet(self, sparql, path):
        """Export SELECT results as identity-preserving Arrow structs in Parquet."""
        _atomic_export(self.query_arrow(sparql), path, _write_parquet)
    def export_csv(self, sparql, path):
        """Export SELECT results using four columns per variable; empty kind means unbound."""
        _atomic_export(self.query_arrow(sparql), path, _write_csv)
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

    def query_json(self, sparql: str) -> str:
        """SPARQL 1.1 Query Results JSON as a string; the fastest path for large results."""
        return self._db.query_json(sparql)

    def query_arrow(self, sparql: str, *, types="rdf"):
        """Return Arrow batches; native columns are optional and lose RDF lexical identity."""
        return _arrow_types(self._db.query_arrow(sparql), types)

    def export_parquet(self, sparql, path):
        """Export SELECT results as identity-preserving Arrow structs in Parquet."""
        _atomic_export(self.query_arrow(sparql), path, _write_parquet)

    def export_csv(self, sparql, path):
        """Export SELECT results using four columns per variable; empty kind means unbound."""
        _atomic_export(self.query_arrow(sparql), path, _write_csv)

    def load(self, path) -> int:
        """Atomically import an N-Triples (.nt) or Turtle (.ttl) file. Parses the whole file in memory."""
        return self._db.load(str(path))

    def stats_ready(self) -> bool:
        """True once background optimizer statistics are built (queries work before that, with greedy plans)."""
        return self._db.stats_ready()

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
