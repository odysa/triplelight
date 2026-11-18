import hashlib
from http.server import HTTPServer
import json
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from unittest.mock import Mock, patch

from duck_http import handler


class DuckHttpTests(unittest.TestCase):
    def setUp(self):
        self.connection = Mock()
        self.connection.execute.return_value.fetchall.return_value = [('urn:t', 2)]
        self.query = 'SELECT registered query'
        queries = {hashlib.sha256(self.query.encode()).hexdigest(): {'template': 1, 'parameters': {}}}
        self.server = HTTPServer(('127.0.0.1', 0), handler(self.connection, queries))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = 'http://127.0.0.1:' + str(self.server.server_port) + '/sparql'

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, query):
        request = urllib.request.Request(self.endpoint, data=urllib.parse.urlencode({'query': query}).encode(),
            headers={'Content-Type': 'application/x-www-form-urlencoded'})
        return urllib.request.urlopen(request, timeout=5)

    def test_repeated_query_executes_sql_again_and_serializes_all_columns(self):
        with patch('duck_http.translate', return_value='SELECT work'):
            for _ in range(2):
                with self.request(self.query) as response:
                    result = json.load(response)
                self.assertEqual(result['head']['vars'], ['productType', 'reviewCount'])
                self.assertEqual(result['results']['bindings'][0]['reviewCount']['value'], '2')
        self.assertEqual(self.connection.execute.call_count, 2)
        self.assertEqual(self.connection.execute.return_value.fetchall.call_count, 2)

    def test_unregistered_query_is_not_executed(self):
        with self.assertRaises(urllib.error.HTTPError) as caught:
            self.request('SELECT other')
        self.assertEqual(caught.exception.code, 400)
        self.connection.execute.assert_not_called()
        caught.exception.close()

    def test_engine_error_is_http_failure_not_empty_success(self):
        self.connection.execute.side_effect = RuntimeError('spill limit')
        with patch('duck_http.translate', return_value='SELECT work'):
            with self.assertRaises(urllib.error.HTTPError) as caught:
                self.request(self.query)
        self.assertEqual(caught.exception.code, 500)
        self.assertIn('spill limit', caught.exception.read().decode())
        caught.exception.close()


if __name__ == '__main__':
    unittest.main()
