#!/usr/bin/env python3
"""Every binding package must carry LICENSE and THIRD_PARTY_NOTICES.md, identical to the root files.

Python (in bindings/python/notices/, listed in license-files), Node, Go, Java and C keep a checked-in copy next to their manifest; the WASM package copies the root file
into its generated pkg/ directories (scripts/bindgen.js). This script fails if a copy differs or a manifest stops
packaging it. Run from anywhere: python3 ci/check_notices.py
"""
import pathlib, sys

root = pathlib.Path(__file__).resolve().parent.parent
FILES = ("LICENSE", "THIRD_PARTY_NOTICES.md")
texts = {n: (root / n).read_text() for n in FILES}
bad = []
copies = {"python": "bindings/python/notices", "node": "bindings/node", "go": "bindings/go", "java": "bindings/java", "c": "bindings/c"}
# The published crates carry the licence; the crates with derived code also carry the notices.
crate_copies = {f"crates/triplelight-{c}": ("LICENSE",) for c in ("plan", "sparql", "storage")}
crate_copies.update({f"crates/triplelight-{c}": FILES for c in ("core", "exec", "iri", "rdf", "rdfs", "shacl")})
# The patched tiny-http copy (#123) carries its own MIT/Apache licences and the notices naming it.
crate_copies["vendor/tiny_http"] = ("THIRD_PARTY_NOTICES.md",)
for d, names in crate_copies.items():
    for n in names:
        f = root / d / n
        if not f.exists() or f.read_text() != texts[n]:
            bad.append(f"{d}/{n} is missing or differs from the root file (cp {n} {d}/)")
for name, d in copies.items():
    for n in FILES:
        f = root / d / n
        if not f.exists() or f.read_text() != texts[n]:
            bad.append(f"{d}/{n} is missing or differs from the root file (cp {n} {d}/)")
needs = {
    "bindings/python/pyproject.toml": ["license-files", "notices/LICENSE", "THIRD_PARTY_NOTICES.md"],
    "bindings/node/package.json": ['"files"', '"LICENSE"', "THIRD_PARTY_NOTICES.md"],
    "bindings/wasm/scripts/bindgen.js": ['"LICENSE"', "THIRD_PARTY_NOTICES.md"],
    "bindings/java/pom.xml": ["<include>LICENSE</include>", "THIRD_PARTY_NOTICES.md"],
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
        names = zipfile.ZipFile(w).namelist()
        found["wheel"] = all(any(n.endswith("/" + f) and ".dist-info/" in n for n in names) for f in FILES)
    for t in art.glob("*.tar.gz"):
        names = tarfile.open(t).getnames()
        if any(n.endswith("/PKG-INFO") for n in names):
            found["sdist"] = all(any(n.endswith("/" + f) for n in names) for f in FILES)
        elif "package/package.json" in names:
            found["npm"] = all("package/" + f in names for f in FILES)
    for t in art.glob("*.tgz"):
        found["npm"] = all("package/" + f in tarfile.open(t).getnames() for f in FILES)
    missing = [k for k, v in found.items() if not v]
    if not found:
        print("no wheel, sdist or npm tarball found in " + str(art)); sys.exit(1)
    if missing:
        print("built package lacks LICENSE or the notices: " + ", ".join(missing)); sys.exit(1)
print("LICENSE and third-party notices present in every binding package")
