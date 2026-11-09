"""Private stdio MCP adapter, always inside cowork-executor's owned process tree.

No log includes credentials. Discovery has no tool effects. A call compares its
entire rediscovered catalogue with the catalogue actually approved by the owner.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import time
import uuid

MAX_MESSAGE = 4 * 1024 * 1024
MAX_CATALOG = 2 * 1024 * 1024


def strict_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate_json_key")
        result[key] = value
    return result


def encode(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def publish(path, value):
    raw = encode(value).encode("utf-8")
    assert len(raw) <= MAX_MESSAGE
    assert not path.exists()
    temporary = path.parent / f".{uuid.uuid4()}.tmp"
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    assert not path.exists()
    os.rename(temporary, path)


def serve(child, operation):
    directory = pathlib.Path(operation["directory"])
    assert directory.is_absolute() and directory.is_dir() and not directory.is_symlink()
    nonce = str(uuid.UUID(operation["nonce"]))
    failed = threading.Event()

    def read_original():
        try:
            for index in range(8192):
                raw = child.stdout.readline(MAX_MESSAGE + 1)
                assert raw and len(raw) <= MAX_MESSAGE and raw.endswith(b"\n")
                message = json.loads(raw, object_pairs_hook=strict_pairs)
                assert message.get("jsonrpc") == "2.0"
                publish(directory / f"{index:08}.out.json", message)
        except Exception:
            failed.set()

    reader = threading.Thread(target=read_original, daemon=True)
    reader.start()
    publish(directory / "ready.json", {"format": "device-mcp-pipe-ready/1", "nonce": nonce})
    try:
        for index in range(8192):
            path = directory / f"{index:08}.in.json"
            while not path.exists():
                close = directory / "close.json"
                if close.exists():
                    assert close.stat().st_size <= 4096 and not close.is_symlink()
                    request = json.loads(close.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
                    assert request == {"format": "device-mcp-pipe-close/1", "nonce": nonce}
                    return
                assert not failed.is_set() and child.poll() is None
                time.sleep(0.01)
            assert not path.is_symlink() and path.stat().st_size <= MAX_MESSAGE
            message = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
            assert message.get("jsonrpc") == "2.0"
            raw = (encode(message) + "\n").encode("utf-8")
            child.stdin.write(raw)
            child.stdin.flush()
            path.unlink()
        raise ValueError("original_pipe_message_bound")
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=3)
        reader.join(timeout=3)
        assert not reader.is_alive() and child.poll() is not None
        publish(directory / "closed.json", {"format": "device-mcp-pipe-closed/1", "nonce": nonce,
                                              "original_child_closed": True, "child_returncode": child.returncode})


def main():
    path = pathlib.Path(sys.argv[1])
    assert path.stat().st_size <= 4 * 1024 * 1024
    config = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
    command = pathlib.Path(config["command"])
    assert command.is_absolute() and command.is_file()
    operation = config["operation"]
    environment = dict(config["environment"])
    # Windows needs system locations to load the selected executable's runtime.
    # They are infrastructure paths, never inherited provider or server tokens.
    for key in ("SystemRoot", "WINDIR"):
        if key not in environment and key in os.environ:
            environment[key] = os.environ[key]
    with tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(
            [str(command), *config["arguments"]],
            cwd=os.getcwd(), env=environment,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
        )
        try:
            if operation["mode"] == "serve":
                serve(child, operation)
                return
            def send(value):
                raw = (encode(value) + "\n").encode("utf-8")
                assert len(raw) <= MAX_MESSAGE
                child.stdin.write(raw)
                child.stdin.flush()

            def request(method, params):
                request_id = str(uuid.uuid4())
                send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
                for _ in range(128):
                    raw = child.stdout.readline(MAX_MESSAGE + 1)
                    assert raw and len(raw) <= MAX_MESSAGE and raw.endswith(b"\n")
                    value = json.loads(raw, object_pairs_hook=strict_pairs)
                    assert value.get("jsonrpc") == "2.0"
                    if "id" in value:
                        assert value["id"] == request_id and "error" not in value
                        return value["result"]
                    # Only inert notifications are consumed. Server-to-client
                    # requests (sampling, elicitation, roots) are not authorized.
                raise ValueError("too_many_mcp_notifications")

            version = config["protocol_version"]
            initialized = request("initialize", {
                "protocolVersion": version, "capabilities": {},
                "clientInfo": {"name": "LocalAI Cowork V2 Device Agent", "version": "2"},
            })
            assert initialized.get("protocolVersion") == version
            assert isinstance(initialized.get("capabilities", {}).get("tools"), dict)
            send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools = []
            cursor = None
            seen = set()
            for _ in range(16):
                result = request("tools/list", {} if cursor is None else {"cursor": cursor})
                assert isinstance(result.get("tools"), list)
                tools.extend(result["tools"])
                assert len(tools) <= 128 and len(encode(tools).encode()) <= MAX_CATALOG
                cursor = result.get("nextCursor")
                if cursor is None:
                    break
                assert isinstance(cursor, str) and cursor not in seen
                seen.add(cursor)
            else:
                raise ValueError("too_many_mcp_catalog_pages")
            tools.sort(key=lambda tool: tool["name"])
            if operation["mode"] == "discover":
                output = {"tools": tools}
            else:
                expected = sorted(operation["expected_tools"], key=lambda tool: tool["name"])
                assert tools == expected, "mcp_catalog_changed_before_dispatch"
                output = {"response": request("tools/call", {
                    "name": operation["name"], "arguments": operation["arguments"],
                })}
            raw = encode(output)
            assert len(raw.encode()) <= MAX_MESSAGE
            print(raw)
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=3)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Detailed untrusted server messages and secrets must not enter logs.
        sys.stderr.write("device_mcp_stdio_bridge_failed\n")
        sys.exit(1)
