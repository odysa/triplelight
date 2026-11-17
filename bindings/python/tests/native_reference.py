"""Slow reference oracle for native Arrow coercion."""

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
                    if v in ("INF", "+INF", "-INF", "NaN"): return {"INF": math.inf, "+INF": math.inf, "-INF": -math.inf, "NaN": math.nan}[v]
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


