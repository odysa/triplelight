#!/usr/bin/env python3
"""Generate TripleLight's Unicode 17.0.0 analyzer data from a pinned UCD."""
import hashlib
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "crates/triplelight-exec/src/expr/text_data"
INPUTS = {
    "DerivedNormalizationProps.txt": "71fd6a206a2c0cdd41feb6b7f656aa31091db45e9cedc926985d718397f9e488",
    "DerivedGeneralCategory.txt": "d62e5bab70ca74f099343f71224fa051cb1fdd61a1ab45c0488c44cfc0b6102e",
}


def entries(path):
    raw = path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != INPUTS[path.name]:
        raise ValueError(f"incorrect Unicode 17.0.0 input: {path}")
    for line in raw.decode().splitlines():
        fields = [value.strip() for value in line.partition("#")[0].split(";")]
        if not fields[0]:
            continue
        start, _, end = fields[0].partition("..")
        yield int(start, 16), int(end or start, 16), fields[1:]


def casefold(path):
    mappings = sorted(
        (start, end, "".join(chr(int(cp, 16)) for cp in fields[1].split()).encode())
        for start, end, fields in entries(path)
        if fields[0] == "NFKC_CF"
    )
    records = bytearray()
    text = bytearray()
    for start, end, mapping in mappings:
        records.extend(struct.pack("<IIIB", start, end, len(text), len(mapping)))
        text.extend(mapping)
    (OUTPUT / "fold_ranges.bin").write_bytes(records)
    (OUTPUT / "fold_utf8.bin").write_bytes(text)


def letter_or_number(path):
    runs = []
    for start, end in sorted(
        (start, end) for start, end, fields in entries(path) if fields[0][0] in "LN"
    ):
        if runs and runs[-1][1] + 1 == start:
            runs[-1] = (runs[-1][0], end)
        else:
            runs.append((start, end))
    (OUTPUT / "word_ranges.bin").write_bytes(
        b"".join(struct.pack("<II", start, end) for start, end in runs)
    )


if __name__ == "__main__":
    ucd = Path(sys.argv[1])
    casefold(ucd / "DerivedNormalizationProps.txt")
    letter_or_number(ucd / "DerivedGeneralCategory.txt")
