import importlib.util
import json
import os
import pathlib
import socket
import subprocess
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SCRIPT = pathlib.Path(__file__).parents[1] / "src" / "connector.py"


class ConnectorTests(unittest.TestCase):
    def setUp(self):
        self.received = []
        records = self.received

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                raw = self.rfile.read(int(self.headers["Content-Length"]))
                records.append((self.path, self.headers.get("Authorization"), json.loads(raw)))
                if self.path.startswith("/lost"):
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                self.send_response(302 if self.path.startswith("/redirect") else 200)
                self.send_header("Location", "/should-never-be-called")
                self.end_headers()
                self.wfile.write(b"secret echoed response that must stay hidden")

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.destination = "http://127.0.0.1:" + str(self.server.server_port)

    def tearDown(self):
        self.server.shutdown()
        self.thread.join()
        self.server.server_close()

    def execute(self, path, requests):
        environment = {k: os.environ[k] for k in ("SystemRoot", "WINDIR", "PATH") if k in os.environ}
        environment["CONNECTOR_CONFIG"] = json.dumps({"url": self.destination + path, "bearer": "synthetic-private-bearer"})
        completed = subprocess.run([sys.executable, "-I", str(SCRIPT), self.destination, "Fixture"],
                                   input="".join(json.dumps(r) + "\n" for r in requests),
                                   capture_output=True, text=True, encoding="utf-8", env=environment, timeout=10)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")
        return [json.loads(line) for line in completed.stdout.splitlines()]

    def call(self):
        return {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "send_json", "arguments": {"destination": self.destination, "payload": {"text": "reviewed message"}}}}

    def test_discovery_is_offline_and_real_post_never_returns_secret_paths_or_headers(self):
        replies = self.execute("/private-webhook?token=synthetic-private-query", [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}])
        self.assertEqual(len(replies), 2)
        self.assertEqual(self.received, [])
        self.assertIn("Fixture — ", replies[1]["result"]["tools"][0]["title"])
        self.assertEqual(replies[1]["result"]["tools"][0]["inputSchema"]["properties"]["destination"]["const"], self.destination)
        replies += self.execute("/private-webhook?token=synthetic-private-query", [self.call()])
        self.assertEqual(self.received, [("/private-webhook?token=synthetic-private-query", "Bearer synthetic-private-bearer", {"text": "reviewed message"})])
        raw = json.dumps(replies)
        for secret in ("private-webhook", "synthetic-private", "secret echoed"):
            self.assertNotIn(secret, raw)

    def test_redirects_do_not_forward_credentials_or_send_a_second_message(self):
        replies = self.execute("/redirect", [self.call()])
        self.assertEqual(len(self.received), 1)
        self.assertIn('"status": 302', replies[0]["result"]["content"][0]["text"])
        self.assertTrue(replies[0]["result"]["isError"])

    def test_lost_delivery_reply_closes_transport_without_ack_or_replay(self):
        replies = self.execute("/lost", [self.call(), self.call()])
        self.assertEqual(replies, [])
        self.assertEqual(len(self.received), 1)

    def test_destination_cannot_be_overridden_by_model_arguments(self):
        request = self.call()
        request["params"]["arguments"]["destination"] = "https://elsewhere.invalid"
        replies = self.execute("/private", [request])
        self.assertIn("error", replies[0])
        self.assertEqual(self.received, [])


if __name__ == "__main__":
    unittest.main()
