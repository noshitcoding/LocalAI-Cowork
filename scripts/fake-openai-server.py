#!/usr/bin/env python3
"""Deterministic OpenAI-compatible endpoint for local integration tests."""

import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        if self.path == "/healthz":
            self._json(200, {"status": "ok"})
        else:
            self._json(404, {"error": "not_found"})

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self._json(404, {"error": "not_found"})
            return
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length) or b"{}")
        tool_completed = any(
            message.get("role") == "tool" for message in request.get("messages", [])
        )
        if tool_completed:
            message = {"content": "Browser artifact captured.", "tool_calls": []}
            finish_reason = "stop"
        else:
            message = {
                "content": None,
                "tool_calls": [
                    {
                        "id": "browser-e2e-1",
                        "type": "function",
                        "function": {
                            "name": "BrowserNavigate",
                            "arguments": json.dumps(
                                {
                                    "url": "http://example.com",
                                    "visible": False,
                                    "timeout_ms": 30000,
                                }
                            ),
                        },
                    }
                ],
            }
            finish_reason = "tool_calls"
        self._json(
            200,
            {
                "choices": [
                    {"message": message, "finish_reason": finish_reason}
                ],
                "usage": {"prompt_tokens": 7, "completion_tokens": 3},
            },
        )

    def _json(self, status, value):
        payload = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 18091), Handler).serve_forever()
