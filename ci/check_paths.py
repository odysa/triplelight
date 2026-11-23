#!/usr/bin/env python3
"""Fail if a file under docs/ or bench/results/ holds an absolute path into a developer machine (`/Users/...`, `/private/tmp/...`).
The repository is public: use `<workspace>`, `<home>`, `<scratch>`, a relative path, or an environment variable with a usage message.
A line may quote the pattern itself if it carries the marker `<redaction-note>`. Run from the repository root."""
import os, re, sys

PATTERN = re.compile(r"/Users/|/private/tmp/")
MARKER = "<redaction-note>"
bad = []
for root in ("docs", "bench/results"):
    for dirpath, _, names in os.walk(root):
        for n in names:
            p = os.path.join(dirpath, n)
            try:
                text = open(p, encoding="utf-8").read()
            except (UnicodeDecodeError, OSError):
                continue                                   # binary files (PDFs, images, archives) are not scanned
            for i, line in enumerate(text.splitlines(), 1):
                if PATTERN.search(line) and MARKER not in line:
                    bad.append(f"{p}:{i}")  # file and line only: printing the line would copy the path into the public CI log
if bad:
    print("absolute local paths found (replace them by <workspace>, <home>, <scratch> or a variable):")
    print("\n".join(bad[:50]))
    if len(bad) > 50: print(f"... and {len(bad) - 50} more")
    sys.exit(1)
print("no absolute local paths under docs/ or bench/results/")
