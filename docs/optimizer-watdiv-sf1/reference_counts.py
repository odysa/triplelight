"""Independent Jena bag-cardinality reference for the saved pure-BGP queries.
Usage: python3 reference_counts.py DATA.nt /path/to/jena/bin/arq
No running service is modified. Each sequential JVM is limited to 1 GiB.
"""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent
counts = []
raw = {}
for query in sorted((root / 'queries').glob('*.rq')):
    # Nested SELECT preserves projection multiplicities; COUNT(*) counts bindings.
    with tempfile.NamedTemporaryFile(mode='w', suffix='.rq') as temp:
        temp.write('SELECT (COUNT(*) AS ?count) WHERE { { ' + query.read_text() + ' } }')
        temp.flush()
        run = subprocess.run(
            [sys.argv[2], '--data=' + str(Path(sys.argv[1]).resolve()),
             '--query=' + temp.name, '--results=JSON'],
            capture_output=True, text=True, timeout=40, check=True,
            env={**os.environ, 'JVM_ARGS': '-Xmx1g'},
        )
    result = json.loads(run.stdout)
    count = int(result['results']['bindings'][0]['count']['value'])
    raw[query.stem] = result
    counts.append((query.stem, count))
    print(query.stem, count, flush=True)
(root / 'reference_counts.tsv').write_text(''.join(f'{q}\t{n}\n' for q, n in counts))
(root / 'jena_counts.json').write_text(json.dumps(raw, indent=2) + '\n')
