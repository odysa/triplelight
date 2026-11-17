#!/usr/bin/env python3
"""Convert independently produced SPARQL JSON files into canonical references.

Usage: python reference.py /path/to/jena-json-dir > reference.json
File stems must match query names. This is an offline correctness step, not timed.
"""
import json
from pathlib import Path
import sys
from contract import json_term, signature


def convert(folder):
    references = {}
    for path in sorted(folder.glob("*.json")):
        raw = json.loads(path.read_text())
        variables = raw["head"]["vars"]
        rows = [tuple(json_term(binding.get(var)) for var in variables)
                for binding in raw["results"]["bindings"]]
        references[path.stem] = signature(variables, rows)
    if not references:
        raise ValueError("No SPARQL SELECT JSON files")
    return references


if __name__ == "__main__":
    print(json.dumps(convert(Path(sys.argv[1])), indent=2))
