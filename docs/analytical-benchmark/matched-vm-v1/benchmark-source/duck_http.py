#!/usr/bin/env python3
"""Serial HTTP adapter for the registered BI SQL translations, not a SPARQL frontend.

Uses a read-only plain-triples DuckDB store. Every request executes its SQL anew;
there is no prepared result, statement handle, or answer cache. SQL generation,
planning, execution, fetch and JSON serialization occur inside the HTTP request.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import urllib.parse

from bi_sql import translate
from fixture_check import duck_result


def registry(workload):
    manifest = json.loads((workload / 'manifest.json').read_text())
    queries = {}
    for item in manifest['queries']:
        raw = (workload / 'queries' / item['file']).read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        if digest != item['sha256']:
            raise ValueError('Query hash differs: ' + item['file'])
        previous = queries.get(digest)
        if previous and (previous['template'], previous['parameters']) != (item['template'], item['parameters']):
            raise ValueError('Ambiguous query registry')
        queries[digest] = item
    if not queries:
        raise ValueError('Empty workload registry')
    return manifest, queries


def handler(connection, queries):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def respond(self, status, body, content_type='application/json'):
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path != '/sparql':
                self.respond(404, b'{"error":"unknown endpoint"}')
                return
            try:
                length = int(self.headers.get('Content-Length', '-1'))
                if not 0 <= length <= 1024 * 1024:
                    raise ValueError('Invalid request size')
                if self.headers.get_content_type() != 'application/x-www-form-urlencoded':
                    raise ValueError('Expected form-encoded query')
                fields = urllib.parse.parse_qs(self.rfile.read(length).decode(), keep_blank_values=True)
                if set(fields) != {'query'} or len(fields['query']) != 1:
                    raise ValueError('Exactly one registered query is required')
                digest = hashlib.sha256(fields['query'][0].encode()).hexdigest()
                item = queries.get(digest)
                if item is None:
                    raise ValueError('Query is not in the pinned BI workload')
            except (ValueError, UnicodeError) as exc:
                self.respond(400, json.dumps({'error': str(exc)}).encode())
                return
            try:
                rows = connection.execute(translate(item['template'], item['parameters'])).fetchall()
                body = json.dumps(duck_result(item['template'], rows), allow_nan=False).encode()
            except Exception as exc:
                self.respond(500, json.dumps({'error': type(exc).__name__ + ': ' + str(exc)}).encode())
                return
            self.respond(200, body, 'application/sparql-results+json')
    return Handler


def serve(args):
    def lock():
        if Path('/tmp/rdf-bench.lock/owner').read_text().strip() != args.lock_owner:
            raise RuntimeError('Own the benchmark lock before starting the adapter')
    lock()
    import duckdb
    manifest, queries = registry(args.workload)
    imported = json.loads(args.import_report.read_text())
    if not imported['complete'] or imported['source_sha256'] != manifest['source_data_sha256']:
        raise ValueError('Unqualified import or dataset mismatch')
    args.spill.mkdir(parents=True, exist_ok=True)
    if any(args.spill.iterdir()):
        raise ValueError('Use a fresh owned spill directory')
    connection = duckdb.connect(str(args.database), read_only=True)
    try:
        connection.execute('SET threads=?', [args.threads])
        connection.execute('SET memory_limit=?', [args.memory_limit])
        connection.execute('SET temp_directory=?', [str(args.spill.resolve())])
        connection.execute('SET max_temp_directory_size=?', [args.max_spill])
        # Bind before recording readiness; a busy port is a failed start.
        with HTTPServer((args.host, args.port), handler(connection, queries)) as server:
            report = {'system': 'duckdb', 'version': duckdb.__version__, 'mode': 'registered-BI-SQL-HTTP-adapter',
                      'endpoint': 'http://' + args.host + ':' + str(server.server_port) + '/sparql',
                      'dataset_sha256': manifest['source_data_sha256'], 'threads': args.threads,
                      'memory_limit': args.memory_limit, 'max_spill': args.max_spill,
                      'spill': str(args.spill.resolve()), 'query_result_cache': False,
                      'sql_work_in_request': ['translation', 'planning', 'execution', 'fetch', 'serialization']}
            print(json.dumps(report), flush=True)
            server.serve_forever()
    finally:
        connection.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for field in ('workload', 'database', 'import-report', 'spill'):
        parser.add_argument('--' + field, type=Path, required=True)
    parser.add_argument('--lock-owner', required=True)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--threads', type=int, default=4)
    parser.add_argument('--memory-limit', default='8GB')
    parser.add_argument('--max-spill', default='8GB')
    args = parser.parse_args()
    if args.threads < 1:
        parser.error('positive threads required')
    serve(args)
