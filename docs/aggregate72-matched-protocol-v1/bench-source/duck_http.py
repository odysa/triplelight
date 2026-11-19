"""Workload-bound HTTP adapter; translation, SQL, fetch and encoding are inside each request."""
import argparse,hashlib,json,threading,urllib.parse
from http.server import HTTPServer,BaseHTTPRequestHandler
from pathlib import Path
from aggregate_sql import translate
import duckdb

parser=argparse.ArgumentParser()
for name in ['database','workload','rules','dataset','spill']:parser.add_argument('--'+name,required=True)
parser.add_argument('--port',type=int,required=True)
args=parser.parse_args()
rules=json.loads(Path(args.rules).read_text())
allowed={}
for name,record in rules['queries'].items():
    if record['dataset']!=args.dataset:continue
    query=Path(args.workload)/name
    if hashlib.sha256(query.read_bytes()).hexdigest()!=record['query_sha256']:raise ValueError('Registered query changed')
    allowed[record['query_sha256']]=record
connection=duckdb.connect(args.database,read_only=True)
connection.execute('SET threads=4')
connection.execute("SET memory_limit='8GB'")
connection.execute('SET temp_directory=?',[args.spill])
connection.execute("SET max_temp_directory_size='8GB'")

class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def send_json(self,status,obj):
        raw=json.dumps(obj,separators=(',',':')).encode()
        self.send_response(status)
        self.send_header('Content-Type','application/sparql-results+json')
        self.send_header('Content-Length',str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def do_GET(self):
        if self.path!='/health':return self.send_json(404,{'error':'unknown endpoint'})
        self.send_json(200,{'system':'duckdb','version':duckdb.__version__,'rows':connection.execute('SELECT count(*) FROM triples').fetchone()[0],'threads':4,'memory_limit':'8GB','max_spill':'8GB'})
    def do_POST(self):
        try:
            raw=self.rfile.read(int(self.headers['Content-Length']))
            query=urllib.parse.parse_qs(raw.decode())['query'][0] if 'application/x-www-form-urlencoded' in self.headers.get('Content-Type','') else raw.decode()
            digest=hashlib.sha256(query.encode()).hexdigest()
            if digest not in allowed:raise ValueError('Query is outside registered dataset workload')
            translated=translate(query)
            if [list(x) for x in translated.columns]!=allowed[digest]['columns']:raise ValueError('Projection differs from registered workload')
            timer=threading.Timer(600,connection.interrupt)
            timer.start()
            try:result=translated.result_json(connection.execute(translated.sql).fetchall())
            finally:timer.cancel()
            self.send_json(200,result)
        except Exception as exc:self.send_json(500,{'error':type(exc).__name__+': '+str(exc)})

print(json.dumps({'system':'duckdb','version':duckdb.__version__,'ready':True}),flush=True)
try:HTTPServer(('0.0.0.0',args.port),Handler).serve_forever()
finally:connection.close()
