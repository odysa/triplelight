#!/usr/bin/env python3
"""Fail CI when the W3C SPARQL 1.1 query suite regresses.

Usage: w3c_floor.py <results.json> <floor.json>

`floor.json` records the minimum pass count and the exact set of test names that must pass. A run passes only if every
listed test still passes; new passes are reported so the floor can be raised in the same PR that fixes them.
"""
import json, sys

results = json.load(open(sys.argv[1]))
floor = json.load(open(sys.argv[2]))
passing = {r["suite"] + "/" + r["test"] for r in results if r["outcome"] == "pass"}
required = set(floor["must_pass"])
lost = sorted(required - passing)
gained = sorted(passing - required)
print(f"W3C SPARQL 1.1 query suite: {len(passing)} pass (floor {len(required)})")
if gained:
    print(f"newly passing ({len(gained)}): " + ", ".join(gained) + "  -> raise ci/w3c_floor.json")
if lost:
    print(f"REGRESSED ({len(lost)}): " + ", ".join(lost))
    sys.exit(1)
