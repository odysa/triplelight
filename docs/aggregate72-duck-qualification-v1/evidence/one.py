import sys,json,hashlib
from pathlib import Path
root=Path('/private/tmp/rdf-jianlin-analytical25');p=Path(__file__).resolve().parent;prep=root/'aggregate72-preparation-v1'
sys.path.insert(0,str(prep/'source'))
from aggregate_sql import translate
from aggregate_count_policy import compare
import duckdb
name=sys.argv[1];dataset=name.split('/')[0];f=json.loads((prep/'rules-freeze.json').read_text());record=f['queries'][name]
query=prep/'workload'/name;sqlfile=prep/'translations'/Path(name).with_suffix('.sql')
assert hashlib.sha256(query.read_bytes()).hexdigest()==record['query_sha256']
assert hashlib.sha256(sqlfile.read_bytes()).hexdigest()==record['sql_sha256']
t=translate(query.read_text());assert t.sql==sqlfile.read_text().strip()
assert [list(c) for c in t.columns]==record['columns']
spill=p/dataset/'spill';spill.mkdir(parents=True,exist_ok=True)
c=duckdb.connect(str(root/'aggregate72-duck-import-v1'/dataset/'duck.db'),read_only=True)
try:
 c.execute('SET threads=4');c.execute("SET memory_limit='8GB'");c.execute('SET temp_directory=?',[str(spill)]);c.execute("SET max_temp_directory_size='8GB'")
 actual=t.result_json(c.execute(t.sql).fetchall())
finally:c.close()
out=p/dataset/(Path(name).stem+'.json');out.write_text(json.dumps(actual,indent=2)+'\n')
ref=root/'aggregate72-reference-v2'/dataset/(Path(name).stem+'.json')
binding=json.loads((root/'aggregate72-reference-v2/reference-binding.json').read_text())
assert hashlib.sha256(ref.read_bytes()).hexdigest()==binding['references'][name]['sha256']
result=compare(actual,json.loads(ref.read_text()),record['columns']);result['output_sha256']=hashlib.sha256(out.read_bytes()).hexdigest();result['reference_sha256']=hashlib.sha256(ref.read_bytes()).hexdigest()
(p/dataset/(Path(name).stem+'-check.json')).write_text(json.dumps(result,indent=2)+'\n')
