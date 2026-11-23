# Provenance: redaction

The repository is public, so the raw files were redacted before commit:
agent workspace paths became `<workspace>` (home paths `<home>`), names of other workloads' containers on the test laptop became `<external container N>` (numbered consistently across files), and process lines in `*.load.txt` that belong to other software became `<external process>` with their CPU figure kept. Counts, CPU figures and every measured number are unchanged. SHA-256 of each file before redaction:

| File | Original SHA-256 |
| --- | --- |
| `r1-dedup0.load.txt` | `60325932e74d03ced8666ce5a350b445a3e40b78e3fa3e7af0a634fec8f8225f` |
| `r2-dedup1.load.txt` | `56ce17ce044ee7d4015427e59ab2f904dc842c97a100f741ede85b29c9ca3eac` |
| `r3-dedup1.load.txt` | `a4dd22cf9aa607e13c6f82bbe6e4899ccf47246864712cd2fd3c2ed1884bbe50` |
| `r4-dedup0.load.txt` | `b2945e38f058609e889a9474f3cfa5f35dc107be2474a02bd1dcb4e66bf4a086` |
| `run.sh` | `08da457bb6419aa886599b2aa43ab8e316436e6a6ac0d79a855070cd71a19f36` |
| `gates/run.sh` | `0bca44c38e80890a52a4c5981e4e55c19f2ae955b6f385d564ff43bf08aefdf1` |
