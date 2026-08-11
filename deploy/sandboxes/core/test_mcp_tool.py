from __future__ import annotations

import json
import os
import subprocess
import sys
import importlib.util
import http.server
import threading
import time
import unittest
from pathlib import Path


TOOL = Path(os.environ.get("COWORK_MCP_TOOL_PATH") or Path(__file__).resolve().with_name("mcp-tool.py"))
SPEC = importlib.util.spec_from_file_location("cowork_mcp_tool", TOOL)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("failed to load MCP tool module")
MCP_TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MCP_TOOL)
FAKE_SERVER = r"""
import json, os, sys
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    if message["method"] == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}}
    elif message["method"] == "tools/call":
        result = {
            "content": [{"type": "text", "text": "authorized=" + str(os.environ.get("MCP_TEST_SECRET") == "one-time-secret")}],
            "structuredContent": message["params"]["arguments"],
            "isError": message["params"]["name"] == "fail",
        }
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"""


def invoke(tool_name: str) -> dict:
    request = {
        "server": {
            "name": "Fake server",
            "command": sys.executable,
            "args": ["-u", "-c", FAKE_SERVER],
            "environment": {"MCP_TEST_SECRET": "one-time-secret"},
        },
        "tool_name": tool_name,
        "arguments": {"query": "hello"},
        "timeout_seconds": 5,
    }
    completed = subprocess.run(
        [sys.executable, str(TOOL)],
        input=json.dumps(request),
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=15,
        check=True,
    )
    return json.loads(completed.stdout)


class StreamableHttpFixture(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    records: list[dict] = []

    def read_message(self) -> dict:
        length = int(self.headers.get("Content-Length", "0"))
        return json.loads(self.rfile.read(length))

    def send_bytes(self, status: int, content_type: str, body: bytes, session: bool = False) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        if session:
            self.send_header("MCP-Session-Id", "fixture-session")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:
        message = self.read_message()
        self.records.append(
            {
                "method": message.get("method"),
                "session": self.headers.get("MCP-Session-Id"),
                "protocol": self.headers.get("MCP-Protocol-Version"),
                "token": self.headers.get("X-Test-Token"),
            }
        )
        if message.get("method") == "initialize":
            response = {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                },
            }
            self.send_bytes(
                200,
                "application/json",
                json.dumps(response).encode(),
                session=True,
            )
            return
        if message.get("method") == "notifications/initialized":
            self.send_bytes(202, "application/json", b"")
            return
        if message.get("method") == "tools/call":
            result = {
                "content": [{"type": "text", "text": "HTTP fixture"}],
                "structuredContent": message["params"]["arguments"],
                "isError": False,
            }
            response = {"jsonrpc": "2.0", "id": message["id"], "result": result}
            if message["params"]["name"] == "stream":
                body = ("id: fixture-event\nevent: message\ndata: " + json.dumps(response) + "\n\n").encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()
                # A Streamable HTTP server may leave the SSE response open after
                # the matching JSON-RPC response. The client must not wait for EOF.
                time.sleep(2)
                self.close_connection = True
            else:
                self.send_bytes(200, "application/json", json.dumps(response).encode())
            return
        self.send_bytes(400, "application/json", b"{}")

    def do_DELETE(self) -> None:
        self.records.append(
            {
                "method": "DELETE",
                "session": self.headers.get("MCP-Session-Id"),
                "protocol": self.headers.get("MCP-Protocol-Version"),
                "token": self.headers.get("X-Test-Token"),
            }
        )
        self.send_bytes(200, "application/json", b"{}")

    def log_message(self, format: str, *args) -> None:
        pass


def invoke_http(tool_name: str) -> tuple[dict, list[dict], float]:
    StreamableHttpFixture.records = []
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StreamableHttpFixture)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        started = time.monotonic()
        response = MCP_TOOL.execute(
            {
                "server": {
                    "name": "HTTP fixture",
                    "transport": "streamable_http",
                    "url": f"http://127.0.0.1:{server.server_port}/mcp",
                    "headers": {"X-Test-Token": "http-secret"},
                },
                "tool_name": tool_name,
                "arguments": {"query": "hello"},
                "timeout_seconds": 5,
            },
            allow_insecure_http=True,
        )
        return response, list(StreamableHttpFixture.records), time.monotonic() - started
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


class McpToolTests(unittest.TestCase):
    def test_initializes_calls_and_returns_structured_content(self) -> None:
        response = invoke("lookup")

        self.assertTrue(response["success"], response)
        self.assertEqual(response["protocol_version"], "2024-11-05")
        self.assertEqual(response["result"]["structuredContent"], {"query": "hello"})
        self.assertEqual(response["result"]["content"][0]["text"], "authorized=True")

    def test_preserves_mcp_is_error_as_a_failed_tool_result(self) -> None:
        response = invoke("fail")

        self.assertFalse(response["success"])
        self.assertEqual(response["error"], "MCP tool returned isError=true")

    def test_streamable_http_negotiates_session_headers_and_json_response(self) -> None:
        response, records, _ = invoke_http("lookup")

        self.assertTrue(response["success"], response)
        self.assertEqual(response["protocol_version"], "2025-11-25")
        self.assertEqual(response["result"]["structuredContent"], {"query": "hello"})
        self.assertEqual([record["method"] for record in records], [
            "initialize", "notifications/initialized", "tools/call", "DELETE"
        ])
        self.assertIsNone(records[0]["session"])
        self.assertIsNone(records[0]["protocol"])
        for record in records[1:]:
            self.assertEqual(record["session"], "fixture-session")
            self.assertEqual(record["protocol"], "2025-11-25")
            self.assertEqual(record["token"], "http-secret")

    def test_streamable_http_accepts_sse_tool_response(self) -> None:
        response, _, elapsed = invoke_http("stream")

        self.assertTrue(response["success"], response)
        self.assertEqual(response["result"]["content"][0]["text"], "HTTP fixture")
        self.assertLess(elapsed, 1.5, "client waited for the persistent SSE response to close")

    def test_streamable_http_defensively_rejects_local_and_non_proxy_https_endpoints(self) -> None:
        for endpoint in (
            "https://127.0.0.1/mcp",
            "https://service.internal/mcp",
            "https://mcp.example.com:8443/mcp",
        ):
            with self.subTest(endpoint=endpoint):
                with self.assertRaises(ValueError):
                    MCP_TOOL.StreamableHttpClient(
                        {"url": endpoint, "headers": {}},
                        timeout_seconds=1,
                    )


if __name__ == "__main__":
    unittest.main()
