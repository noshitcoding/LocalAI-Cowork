from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest
from pathlib import Path


TOOL = Path(os.environ.get("COWORK_MCP_TOOL_PATH") or Path(__file__).resolve().with_name("mcp-tool.py"))
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


if __name__ == "__main__":
    unittest.main()
