# Provenance: redaction

The repository is public, so the raw files were redacted before commit:
agent workspace paths became `<workspace>` (home paths `<home>`), names of other workloads' containers on the test laptop became `<external container N>` (numbered consistently across files), and process lines in `*.load.txt` that belong to other software became `<external process>` with their CPU figure kept. Counts, CPU figures and every measured number are unchanged. SHA-256 of each file before redaction:

| File | Original SHA-256 |
| --- | --- |
| `run.sh` | `d26a5d39719f2cdc764da00f7936fc4830cd038d9fe170157f78e83e7ee609ad` |
| `ab/r1-a03774f2.load.txt` | `f8765c3a6574a8272c1847a954b7a660014ec02441f9ccbd72f1538022f86494` |
| `ab/r2-5cfdd0ad.load.txt` | `dbd002a86274c4cc1a1c7943b571373ad4f4d5a0e6609f8ecfb2556e2df0ea62` |
| `ab/r3-5cfdd0ad.load.txt` | `619e617e0823e893b40d7f9aa12a3a7b1e775247fbb2ea2a9e96465444bf3e0b` |
| `ab/r4-a03774f2.load.txt` | `4de75b1ef53b18abee14022f283829e4cc327eb7468b953ed622c02a818cedd9` |
| `ab/run.sh` | `a0107073614a2fbbd1262d72cf32b92392d1ba01662cc1adc4d8aff61ce4e9fa` |
