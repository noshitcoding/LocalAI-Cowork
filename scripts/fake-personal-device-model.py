#!/usr/bin/env python3
"""Deterministic OpenAI-compatible model for the personal-daemon bridge E2E."""

import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        if self.path == "/healthz":
            self.respond(200, {"status": "ok"})
        else:
            self.respond(404, {"error": "not_found"})

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self.respond(404, {"error": "not_found"})
            return
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length) or b"{}")
        delay_ms = int(os.environ.get("COWORK_FAKE_MODEL_DELAY_MS", "0"))
        if delay_ms > 0:
            time.sleep(delay_ms / 1000)
        completed = any(message.get("role") == "tool" for message in request.get("messages", []))
        if completed:
            message = {"content": "Personal daemon bridge completed.", "tool_calls": []}
            finish_reason = "stop"
        else:
            message = {
                "content": None,
                "tool_calls": [{
                    "id": "personal-write-1",
                    "type": "function",
                    "function": {
                        "name": "Write",
                        "arguments": json.dumps({
                            "path": "bridge-result.txt",
                            "content": "written by the shared local daemon runtime\n",
                        }),
                    },
                }],
            }
            finish_reason = "tool_calls"
        self.respond(200, {
            "choices": [{"message": message, "finish_reason": finish_reason}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 5},
        })

    def respond(self, status, value):
        payload = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    port = int(os.environ.get("COWORK_FAKE_MODEL_PORT", "18095"))
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
