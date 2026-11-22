#!/usr/bin/env python3
"""Every binding package must carry THIRD_PARTY_NOTICES.md, identical to the root file.

Python (in bindings/python/notices/, listed in license-files), Node, Go, Java and C keep a checked-in copy next to their manifest; the WASM package copies the root file
into its generated pkg/ directories (scripts/bindgen.js). This script fails if a copy differs or a manifest stops
packaging it. Run from anywhere: python3 ci/check_notices.py
"""
import pathlib, sys

root = pathlib.Path(__file__).resolve().parent.parent
notice = (root / "THIRD_PARTY_NOTICES.md").read_text()
bad = []
copies = {"python": "bindings/python/notices", "node": "bindings/node", "go": "bindings/go", "java": "bindings/java", "c": "bindings/c"}
for name, d in copies.items():
    f = root / d / "THIRD_PARTY_NOTICES.md"
    if not f.exists() or f.read_text() != notice:
        bad.append(f"{d}/THIRD_PARTY_NOTICES.md is missing or differs from the root file (cp THIRD_PARTY_NOTICES.md {d}/)")
needs = {
    "bindings/python/pyproject.toml": ["license-files", "THIRD_PARTY_NOTICES.md"],
    "bindings/node/package.json": ['"files"', "THIRD_PARTY_NOTICES.md"],
    "bindings/wasm/scripts/bindgen.js": ["THIRD_PARTY_NOTICES.md"],
    "bindings/java/pom.xml": ["THIRD_PARTY_NOTICES.md"],
}
for path, words in needs.items():
    text = (root / path).read_text()
    for w in words:
        if w not in text:
            bad.append(f"{path} no longer mentions {w}")
if bad:
    print("\n".join(bad)); sys.exit(1)
# Optional: python3 ci/check_notices.py --artifacts DIR checks built packages (wheel, sdist, npm tarball) for the file.
if len(sys.argv) == 3 and sys.argv[1] == "--artifacts":
    import tarfile, zipfile
    art = pathlib.Path(sys.argv[2])
    found = {}
    for w in art.glob("*.whl"):
        found["wheel"] = any(n.endswith("THIRD_PARTY_NOTICES.md") and ".dist-info/" in n for n in zipfile.ZipFile(w).namelist())
    for t in art.glob("*.tar.gz"):
        names = tarfile.open(t).getnames()
        if any(n.endswith("/PKG-INFO") for n in names):
            found["sdist"] = any(n.endswith("THIRD_PARTY_NOTICES.md") for n in names)
        elif "package/package.json" in names:
            found["npm"] = "package/THIRD_PARTY_NOTICES.md" in names
    for t in art.glob("*.tgz"):
        found["npm"] = "package/THIRD_PARTY_NOTICES.md" in tarfile.open(t).getnames()
    missing = [k for k, v in found.items() if not v]
    if not found:
        print("no wheel, sdist or npm tarball found in " + str(art)); sys.exit(1)
    if missing:
        print("built package lacks the notices: " + ", ".join(missing)); sys.exit(1)
print("third-party notices present in every binding package")
