#!/usr/bin/env python3
"""One-shot MCP stdio and Streamable HTTP client for the Linux sandbox."""

from __future__ import annotations

import json
import ipaddress
import os
import queue
import subprocess
import sys
import threading
import time
from typing import Any
from urllib import error as urllib_error
from urllib import parse as urllib_parse
from urllib import request as urllib_request


MAX_LINE_BYTES = 8 * 1024 * 1024
MAX_HTTP_BODY_BYTES = 8 * 1024 * 1024
LATEST_PROTOCOL_VERSION = "2025-11-25"
SUPPORTED_HTTP_PROTOCOL_VERSIONS = {"2025-03-26", "2025-06-18", "2025-11-25"}
RESERVED_HTTP_HEADERS = {
    "accept",
    "connection",
    "content-length",
    "content-type",
    "host",
    "http-proxy",
    "https-proxy",
    "mcp-protocol-version",
    "mcp-session-id",
    "origin",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}


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


class NoRedirect(urllib_request.HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        return None


def valid_header_name(value: str) -> bool:
    token = "!#$%&'*+-.^_`|~"
    return bool(value) and all(
        (character.isascii() and character.isalnum()) or character in token
        for character in value
    )


def read_http_body(response) -> bytes:
    body = response.read(MAX_HTTP_BODY_BYTES + 1)
    if len(body) > MAX_HTTP_BODY_BYTES:
        raise ValueError("MCP HTTP response exceeds 8 MiB")
    return body


def parse_sse(stream, request_id: int, deadline: float) -> tuple[Any | None, str | None, int]:
    last_event_id = None
    retry_ms = 0
    data_lines: list[str] = []
    total_bytes = 0

    def dispatch() -> dict[str, Any] | None:
        nonlocal data_lines
        if not data_lines:
            return None
        data = "\n".join(data_lines)
        data_lines = []
        try:
            message = json.loads(data)
        except json.JSONDecodeError:
            return None
        if not isinstance(message, dict):
            return None
        if message.get("id") == request_id and ("result" in message or "error" in message):
            return message
        elif "id" in message and "method" in message:
            raise RuntimeError("MCP HTTP server requests are not supported by this one-shot client")
        return None

    while True:
        if time.monotonic() >= deadline:
            raise TimeoutError("MCP HTTP SSE response timed out")
        raw_bytes = stream.readline(MAX_HTTP_BODY_BYTES - total_bytes + 1)
        if not raw_bytes:
            message = dispatch()
            return message, last_event_id, retry_ms
        total_bytes += len(raw_bytes)
        if total_bytes > MAX_HTTP_BODY_BYTES:
            raise ValueError("MCP HTTP response exceeds 8 MiB")
        try:
            raw_line = raw_bytes.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError("MCP SSE response is not UTF-8") from error
        raw_line = raw_line.rstrip("\r\n")
        if raw_line == "":
            message = dispatch()
            if message is not None:
                return message, last_event_id, retry_ms
            continue
        if raw_line.startswith(":"):
            continue
        field, separator, value = raw_line.partition(":")
        if separator and value.startswith(" "):
            value = value[1:]
        if field == "data":
            data_lines.append(value)
        elif field == "id" and "\0" not in value:
            last_event_id = value
        elif field == "retry" and value.isdigit():
            retry_ms = min(int(value), 5_000)


class StreamableHttpClient:
    def __init__(
        self,
        server: dict[str, Any],
        timeout_seconds: int,
        allow_insecure_http: bool = False,
    ) -> None:
        endpoint = server.get("url")
        headers = server.get("headers", {})
        if not isinstance(endpoint, str) or not endpoint.strip():
            raise ValueError("MCP streamable HTTP URL is required")
        try:
            parsed = urllib_parse.urlsplit(endpoint)
            host = parsed.hostname
            port = parsed.port
        except ValueError as error:
            raise ValueError("MCP streamable HTTP URL is not a safe endpoint") from error
        allowed_schemes = {"https", "http"} if allow_insecure_http else {"https"}
        if (
            parsed.scheme not in allowed_schemes
            or not host
            or parsed.username is not None
            or parsed.password is not None
            or parsed.query
            or parsed.fragment
            or (parsed.scheme == "https" and port not in (None, 443))
        ):
            raise ValueError("MCP streamable HTTP URL is not a safe endpoint")
        if not allow_insecure_http:
            normalized_host = host.rstrip(".").lower()
            try:
                ipaddress.ip_address(normalized_host)
            except ValueError:
                pass
            else:
                raise ValueError("MCP streamable HTTP URL must use a public DNS hostname")
            if normalized_host in {"localhost", "localhost.localdomain"} or any(
                normalized_host.endswith(suffix)
                for suffix in (".localhost", ".local", ".internal", ".home.arpa")
            ):
                raise ValueError("MCP streamable HTTP URL must use a public DNS hostname")
        if not isinstance(headers, dict) or not all(
            isinstance(key, str) and isinstance(value, str) for key, value in headers.items()
        ):
            raise ValueError("MCP HTTP headers must contain string values")
        normalized: set[str] = set()
        for key, value in headers.items():
            lower_key = key.lower()
            if (
                not valid_header_name(key)
                or lower_key in RESERVED_HTTP_HEADERS
                or lower_key in normalized
                or any(
                    character != "\t" and not 0x20 <= ord(character) <= 0x7E
                    for character in value
                )
            ):
                raise ValueError("MCP HTTP header is invalid or reserved")
            normalized.add(lower_key)
        self.endpoint = endpoint
        self.headers = dict(headers)
        self.timeout_seconds = timeout_seconds
        self.next_id = 1
        self.protocol_version = LATEST_PROTOCOL_VERSION
        self.session_id: str | None = None
        self.initialized = False
        self.opener = urllib_request.build_opener(urllib_request.ProxyHandler(), NoRedirect())

    def request_headers(self, accept: str) -> dict[str, str]:
        headers = dict(self.headers)
        headers["Accept"] = accept
        if self.session_id is not None:
            headers["MCP-Session-Id"] = self.session_id
        if self.initialized:
            headers["MCP-Protocol-Version"] = self.protocol_version
        return headers

    def open(self, request: urllib_request.Request, deadline: float):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("MCP HTTP request timed out")
        try:
            return self.opener.open(request, timeout=remaining)
        except urllib_error.HTTPError as error:
            raise RuntimeError(f"MCP HTTP endpoint returned status {error.code}") from error
        except urllib_error.URLError as error:
            raise RuntimeError(f"MCP HTTP endpoint request failed: {error.reason}") from error

    def parse_response(
        self, response, request_id: int, deadline: float
    ) -> tuple[Any | None, str | None, int]:
        content_type = response.headers.get_content_type().lower()
        if content_type == "application/json" or content_type.endswith("+json"):
            body = read_http_body(response)
            try:
                message = json.loads(body)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise ValueError("MCP HTTP JSON response is invalid") from error
            if not isinstance(message, dict) or message.get("id") != request_id:
                raise RuntimeError("MCP HTTP response has the wrong JSON-RPC id")
            return message, None, 0
        if content_type == "text/event-stream":
            return parse_sse(response, request_id, deadline)
        raise RuntimeError(f"MCP HTTP endpoint returned unsupported content type {content_type!r}")

    def post(self, payload: dict[str, Any], expects_response: bool) -> Any:
        encoded = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        if len(encoded) > MAX_HTTP_BODY_BYTES:
            raise ValueError("MCP HTTP request exceeds 8 MiB")
        request = urllib_request.Request(
            self.endpoint,
            data=encoded,
            headers={
                **self.request_headers("application/json, text/event-stream"),
                "Content-Type": "application/json",
            },
            method="POST",
        )
        deadline = time.monotonic() + self.timeout_seconds
        with self.open(request, deadline) as response:
            if not expects_response:
                if response.status != 202:
                    raise RuntimeError(
                        f"MCP HTTP notification returned status {response.status}, expected 202"
                    )
                read_http_body(response)
                return None
            if self.session_id is None:
                session_id = response.headers.get("MCP-Session-Id")
                if session_id is not None:
                    if not 1 <= len(session_id) <= 1024 or any(
                        ord(character) < 0x21 or ord(character) > 0x7E
                        for character in session_id
                    ):
                        raise RuntimeError("MCP HTTP endpoint returned an invalid session id")
                    self.session_id = session_id
            message, last_event_id, retry_ms = self.parse_response(
                response, int(payload["id"]), deadline
            )
        for _ in range(4):
            if message is not None:
                return self.unwrap(message)
            if last_event_id is None:
                break
            if retry_ms:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                time.sleep(min(retry_ms / 1000, remaining))
            resume = urllib_request.Request(
                self.endpoint,
                headers={
                    **self.request_headers("text/event-stream"),
                    "Last-Event-ID": last_event_id,
                },
                method="GET",
            )
            with self.open(resume, deadline) as response:
                message, next_event_id, retry_ms = self.parse_response(
                    response, int(payload["id"]), deadline
                )
                last_event_id = next_event_id or last_event_id
        raise RuntimeError("MCP HTTP SSE stream ended before its JSON-RPC response")

    @staticmethod
    def unwrap(message: dict[str, Any]) -> Any:
        if "error" in message:
            error = message.get("error")
            detail = error.get("message") if isinstance(error, dict) else str(error)
            raise RuntimeError(f"MCP server rejected request: {detail}")
        return message.get("result")

    def request(self, method: str, params: dict[str, Any]) -> Any:
        request_id = self.next_id
        self.next_id += 1
        return self.post(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
            True,
        )

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self.post({"jsonrpc": "2.0", "method": method, "params": params}, False)

    def close(self) -> None:
        if self.session_id is None:
            return
        request = urllib_request.Request(
            self.endpoint,
            headers=self.request_headers("application/json, text/event-stream"),
            method="DELETE",
        )
        try:
            with self.opener.open(request, timeout=min(self.timeout_seconds, 5)) as response:
                read_http_body(response)
        except Exception:
            pass


def execute_http(
    server: dict[str, Any],
    name: str,
    tool_name: str,
    arguments: dict[str, Any],
    timeout_seconds: int,
    allow_insecure_http: bool,
) -> dict[str, Any]:
    client = StreamableHttpClient(server, timeout_seconds, allow_insecure_http)
    try:
        initialized = client.request(
            "initialize",
            {
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "clientInfo": {"name": "Open Cowork Linux Executor", "version": "0.3.0"},
                "capabilities": {},
            },
        )
        protocol_version = (
            initialized.get("protocolVersion") if isinstance(initialized, dict) else None
        )
        if protocol_version not in SUPPORTED_HTTP_PROTOCOL_VERSIONS:
            raise RuntimeError(f"MCP HTTP server negotiated unsupported version {protocol_version!r}")
        client.protocol_version = protocol_version
        client.initialized = True
        client.notify("notifications/initialized", {})
        result = client.request("tools/call", {"name": tool_name, "arguments": arguments})
        is_error = isinstance(result, dict) and result.get("isError") is True
        return {
            "success": not is_error,
            "server_name": name,
            "tool_name": tool_name,
            "protocol_version": protocol_version,
            "result": result,
            "error": "MCP tool returned isError=true" if is_error else None,
        }
    finally:
        client.close()


def execute(request: dict[str, Any], allow_insecure_http: bool = False) -> dict[str, Any]:
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
    transport = server.get("transport", "stdio")
    if transport == "streamable_http":
        return execute_http(
            server,
            name,
            tool_name,
            arguments,
            timeout_seconds,
            allow_insecure_http,
        )
    if transport != "stdio":
        raise ValueError("unsupported MCP transport")
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
