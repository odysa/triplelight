from pathlib import Path
p=Path('graph.nt')
with p.open('w') as f:
 for i in range(50000):
  f.write(f'<urn:a{i}> <urn:p> <urn:b{i}> .\n<urn:b{i}> <urn:q> <urn:c{i}> .\n<urn:c{i}> <urn:r> <urn:a{i}> .\n')
Path('cycle.rq').write_text('SELECT ?a ?b ?c WHERE { ?a <urn:p> ?b . ?b <urn:q> ?c . ?c <urn:r> ?a }')
