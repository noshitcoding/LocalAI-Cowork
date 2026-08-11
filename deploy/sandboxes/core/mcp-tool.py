#!/usr/bin/env python3
"""One-shot MCP stdio client for the isolated Linux executor sandbox."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from typing import Any


MAX_LINE_BYTES = 8 * 1024 * 1024


def read_request() -> dict[str, Any]:
    value = json.load(sys.stdin)
    if not isinstance(value, dict):
        raise ValueError("request must be an object")
    return value


def reader(stream, messages: queue.Queue[object]) -> None:
    try:
        for line in stream:
            if len(line.encode("utf-8", errors="replace")) > MAX_LINE_BYTES:
                messages.put(ValueError("MCP response line exceeds 8 MiB"))
                return
            try:
                messages.put(json.loads(line))
            except json.JSONDecodeError:
                continue
    except Exception as error:  # pragma: no cover - defensive pipe failure
        messages.put(error)
    finally:
        messages.put(EOFError("MCP process output closed"))


class McpProcess:
    def __init__(self, server: dict[str, Any], timeout_seconds: int) -> None:
        command = server.get("command")
        args = server.get("args", [])
        environment = server.get("environment", {})
        if not isinstance(command, str) or not command.strip():
            raise ValueError("MCP command is required")
        if not isinstance(args, list) or not all(isinstance(value, str) for value in args):
            raise ValueError("MCP args must be strings")
        if not isinstance(environment, dict) or not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in environment.items()
        ):
            raise ValueError("MCP environment must contain string values")
        child_environment = os.environ.copy()
        child_environment.update(environment)
        self.timeout_seconds = timeout_seconds
        self.next_id = 1
        # Bound unsolicited notifications so a noisy or hostile server cannot
        # grow the client process without limit while a request is pending.
        self.messages: queue.Queue[object] = queue.Queue(maxsize=256)
        self.process = subprocess.Popen(
            [command, *args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=child_environment,
            cwd="/workspace" if os.path.isdir("/workspace") else os.getcwd(),
            start_new_session=True,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError("MCP process did not expose stdio")
        self.stdin = self.process.stdin
        threading.Thread(
            target=reader,
            args=(self.process.stdout, self.messages),
            daemon=True,
        ).start()

    def send(self, payload: dict[str, Any]) -> None:
        self.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self.stdin.flush()

    def request(self, method: str, params: dict[str, Any]) -> Any:
        request_id = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + self.timeout_seconds
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"MCP request {method} timed out")
            try:
                message = self.messages.get(timeout=min(remaining, 0.5))
            except queue.Empty:
                if self.process.poll() is not None:
                    raise RuntimeError(f"MCP process exited with code {self.process.returncode}")
                continue
            if isinstance(message, Exception):
                raise message
            if not isinstance(message, dict) or message.get("id") != request_id:
                continue
            if "error" in message:
                error = message.get("error")
                detail = error.get("message") if isinstance(error, dict) else str(error)
                raise RuntimeError(f"MCP server rejected {method}: {detail}")
            return message.get("result")

    def close(self) -> None:
        try:
            self.stdin.close()
        except Exception:
            pass
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)


def execute(request: dict[str, Any]) -> dict[str, Any]:
    server = request.get("server")
    tool_name = request.get("tool_name")
    arguments = request.get("arguments", {})
    timeout_seconds = request.get("timeout_seconds", 120)
    if not isinstance(server, dict):
        raise ValueError("server must be an object")
    if not isinstance(tool_name, str) or not tool_name.strip():
        raise ValueError("tool_name is required")
    if not isinstance(arguments, dict):
        raise ValueError("arguments must be an object")
    if not isinstance(timeout_seconds, int) or not 1 <= timeout_seconds <= 120:
        raise ValueError("timeout_seconds must be between 1 and 120")
    name = str(server.get("name") or "MCP")
    client = McpProcess(server, timeout_seconds)
    try:
        initialized = client.request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "Open Cowork Linux Executor", "version": "0.3.0"},
                "capabilities": {},
            },
        )
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
        result = client.request("tools/call", {"name": tool_name, "arguments": arguments})
        is_error = isinstance(result, dict) and result.get("isError") is True
        return {
            "success": not is_error,
            "server_name": name,
            "tool_name": tool_name,
            "protocol_version": initialized.get("protocolVersion")
            if isinstance(initialized, dict)
            else None,
            "result": result,
            "error": "MCP tool returned isError=true" if is_error else None,
        }
    finally:
        client.close()


def main() -> int:
    try:
        response = execute(read_request())
    except Exception as error:
        response = {
            "success": False,
            "server_name": None,
            "tool_name": None,
            "result": None,
            "error": f"{type(error).__name__}: {error}",
        }
    json.dump(response, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
