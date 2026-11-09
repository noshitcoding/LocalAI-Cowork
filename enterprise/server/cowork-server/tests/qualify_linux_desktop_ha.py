#!/usr/bin/env python3
"""Source-prepared real Docker desktop/three-native-API qualification (Linux/WSL).

Execution requires explicit exact binaries/images, a Docker client/context and
the parent's original-container baseline. Nothing starts on import or without
--execute. The private model fixture requests one genuine Worker GUI tool, then
holds the next model response; it does not implement an executor or a Runner.
API/Runner commands, private mTLS, Docker identities and RFB bytes are real.
Select an exact Runner image, or an exact native ELF plus an independently
approved Linux Docker CLI. Native mode uses the existing fixture's private
Unix socket relay to the parent's explicit Windows Docker client/context;
API-to-Runner HTTPS stays on WSL loopback and needs no Windows loopback access.
No passing report changes the product's default-off HA support flag.
--include-api-desktop-phase additionally qualifies public API Create and
reauthenticated Control on a real, continuously leased Worker run.
--diagnostic-only stops after the initial and cross-API partition/healing phase;
it can never qualify the full matrix. Transport counters describe raw websocket
bytes read/accepted by send, including a coalesced HTTP-101 body prefix.
"""
import argparse
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
from email.message import Message
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import select
import shutil
import signal
import socket
import ssl
import struct
import sys
import threading
import time
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from urllib.request import Request
import uuid

from qualify_api_cluster import (ClusterFixture, MAX_HTTP_HEADER, PROXY_BUFFER,
    WireSocket, canonical_uuid, file_sha256, websocket_accept)
from qualify_runtime import (Fixture, HANDLED_SIGNALS, PG_BIN,
    QualificationFailure, require)


IMAGE_ID = re.compile(r"sha256:[0-9a-f]{64}")
SHA256 = re.compile(r"[0-9a-f]{64}")
MAX_PROXY_BODY = 2 * 1024 * 1024
MAX_MODEL_BODY = 2 * 1024 * 1024
BINDING_FIELDS = ("version", "runner_id", "engine_id", "scope_prefix", "reservation_id",
    "attempt_id", "container_id", "container_name", "created_at", "started_at", "run_id",
    "session_id", "lease_token", "workspace_id", "workspace_volume")
ROOT_CLEANUP_KEYS = {"native", "runner", "parent", "inventory", "archive"}
X11_POINTER_QUERY = """import ctypes,json
x=ctypes.CDLL('libX11.so.6')
x.XOpenDisplay.argtypes=[ctypes.c_char_p];x.XOpenDisplay.restype=ctypes.c_void_p
x.XDefaultRootWindow.argtypes=[ctypes.c_void_p];x.XDefaultRootWindow.restype=ctypes.c_ulong
x.XQueryPointer.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.POINTER(ctypes.c_ulong),ctypes.POINTER(ctypes.c_ulong),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_uint)]
x.XCloseDisplay.argtypes=[ctypes.c_void_p];x.XCloseDisplay.restype=ctypes.c_int
d=x.XOpenDisplay(None)
if not d: raise RuntimeError('owned_x11_display_unavailable')
try:
 r,c=ctypes.c_ulong(),ctypes.c_ulong();rx,ry,wx,wy=[ctypes.c_int() for _ in range(4)];mask=ctypes.c_uint()
 if not x.XQueryPointer(d,x.XDefaultRootWindow(d),ctypes.byref(r),ctypes.byref(c),ctypes.byref(rx),ctypes.byref(ry),ctypes.byref(wx),ctypes.byref(wy),ctypes.byref(mask)): raise RuntimeError('owned_x11_pointer_unavailable')
 print(json.dumps({'x':rx.value,'y':ry.value}))
finally: x.XCloseDisplay(d)
"""


class QualificationRoot:
    """A failure never destroys the operator's private evidence directory."""
    def __init__(self, helpers, parent=None, *, kind="linux_desktop_ha"):
        require(isinstance(kind, str) and kind in {"linux_desktop_ha", "server_resume"},
                "owned_fixture_kind_not_allowed")
        self.helpers = helpers
        self.parent = Path(parent or Path.home() / ".local/share/cowork/ha-qualification")
        require(os.name != "posix" or len(os.fsencode(self.parent / ("0" * 36) / "r" / "docker.sock")) < 104,
                "owned_ha_unix_socket_path_bound")
        self.path = helpers.private_root(self.parent)
        self.scope = canonical_uuid(self.path.name)
        self.identity = {"version": 1, "kind": kind, "scope": self.scope,
            "fixture_root": str(self.path), "parent": str(self.parent),
            "owner_uid": getattr(os, "geteuid", lambda: 0)()}
        helpers.write_private(self.path / "fixture.json", json.dumps(self.identity, separators=(",", ":")))

    def finish(self, successful, cleanup):
        if not successful:
            return "retained_failed_qualification"
        require(set(cleanup) == ROOT_CLEANUP_KEYS and all(value is True for value in cleanup.values()),
                "private_root_removal_requires_all_cleanup_proofs")
        target, parent = self.path, self.parent
        require(target.is_absolute() and target.name == self.scope and target.parent == parent
                and target.is_dir() and not any(path.is_symlink() for path in [target, *target.parents])
                and target.resolve(strict=True) == parent.resolve(strict=True) / self.scope,
                "private_root_removal_scope_mismatch")
        marker = target / "fixture.json"
        require(marker.is_file() and not marker.is_symlink() and marker.stat().st_size <= 4096
                and json.loads(marker.read_text()) == self.identity, "private_root_removal_identity_mismatch")
        if os.name == "posix":
            require(target.stat().st_uid == os.geteuid() and target.stat().st_mode & 0o077 == 0,
                    "private_root_removal_owner_mismatch")
        shutil.rmtree(target)
        return "removed_after_confirmed_success"


def public_failure(error, helpers):
    if isinstance(error, QualificationFailure):
        return {"code": str(error), "classification": "qualification"}
    return helpers.public_failure(error) if helpers else {"code": type(error).__name__, "classification": "qualification"}



def failure_source_positions(error):
    """Only reviewed fixture source locations; never messages, locals or paths."""
    allowed = {"qualify_linux_desktop_ha.py", "qualify_api_cluster.py",
               "qualify_runtime.py", "qualify_desktop_identity.py", "qualify_server_resume.py"}
    positions = []
    current = error.__traceback__
    while current is not None:
        code = current.tb_frame.f_code
        name = Path(code.co_filename).name
        if name in allowed and re.fullmatch(r"[a-zA-Z_][a-zA-Z_0-9]{0,95}", code.co_name):
            positions.append({"source": name, "function": code.co_name,
                              "line": current.tb_lineno})
        current = current.tb_next
    return positions[-8:]


def archive_runner_evidence(storage, runner, helpers):
    """Private snapshots only; neither journal inventory nor files grant authority."""
    destination = storage.path / "evidence"
    destination.mkdir(mode=0o700)
    for name in ("ca.pem", "ca-key.pem", "server.pem", "server-key.pem"):
        source = runner.root / name
        if source.exists():
            require(source.is_file() and not source.is_symlink() and source.stat().st_size <= 1024 * 1024,
                    "runner_private_tls_evidence_bound")
            helpers.write_private(destination / name, source.read_bytes())
    if runner.native_binary or hasattr(runner, "state_volume"):
        snapshot = json.dumps({"operator_snapshot_only": True, "ledger": runner.ledger()}, separators=(",", ":"))
        require(len(snapshot.encode()) <= MAX_PROXY_BODY, "runner_private_ledger_snapshot_bound")
        helpers.write_private(destination / "ledger-before-cleanup.json", snapshot)


def stop_runner_preserving_state(runner, parent):
    """An archive failure permits shutdown, never journal/volume/key deletion."""
    errors = []
    for action in [runner._stop_native,
            (lambda: parent.run("stop", "--time", "5", runner.container))
                if runner.container in parent.containers else None,
            (lambda: runner.relay.__exit__(None, None, None)) if runner.relay is not None else None]:
        if action:
            try:
                action()
            except Exception:
                errors.append("owned_runner_stop_only_unconfirmed")
    require(not errors, "owned_runner_stop_only_unconfirmed")


def cleanup_owned_fixtures(fixture, runner, parent, helpers, storage):
    for number in HANDLED_SIGNALS:
        signal.signal(number, signal.SIG_IGN)
    proofs = {key: False for key in ROOT_CLEANUP_KEYS}
    failures = []
    def attempt(action, callback):
        try:
            callback()
            return True
        except Exception as error:
            failures.append({"cleanup_action": action, **public_failure(error, helpers)})
            return False
    if fixture is not None:
        proofs["native"] = attempt("native_fixture", fixture.cleanup)
        if proofs["native"]:
            proofs["native"] = all(child.poll() is not None for child in fixture.children) and not (fixture.data / "postmaster.pid").exists()
    else:
        proofs["native"] = True
    if runner is not None:
        proofs["archive"] = attempt("private_runner_evidence", lambda: archive_runner_evidence(storage, runner, helpers))
        if proofs["archive"]:
            proofs["runner"] = attempt("runner_fixture", runner.cleanup)
        else:
            # Preserve original TLS/journal/volumes if a private snapshot could
            # not be saved. Stop owned native execution without erasing state.
            attempt("runner_stop_only", lambda: stop_runner_preserving_state(runner, parent))
    else:
        proofs["archive"], proofs["runner"] = True, True
    if parent is not None:
        proofs["parent"] = attempt("original39", parent.assert_preserved)
        inventory = None
        def read_inventory():
            nonlocal inventory
            inventory = parent.owned_inventory(storage.scope)
            require(not any(inventory.values()), "owned_runner_resources_remain_after_cleanup")
        proofs["inventory"] = attempt("owned_inventory", read_inventory)
    else:
        proofs["parent"], proofs["inventory"] = True, True
        inventory = None
    return proofs, failures, inventory


def qualification_inputs(args):
    """Validate the execution matrix before any import, socket or subprocess."""
    require(all(isinstance(value, str) and SHA256.fullmatch(value)
                for value in (args.expected_sha256, args.expected_runner_sha256)), "explicit_binary_sha256_required")
    require(isinstance(args.expected_migration, int) and args.expected_migration >= 58,
            "physical_desktop_schema58_or_later_required")
    require(not (getattr(args, "diagnostic_only", False) and getattr(args, "include_api_desktop_phase", False)),
            "diagnostic_only_cannot_claim_full_api_phase")
    core = args.core_image_id
    native = getattr(args, "runner_binary", None)
    cli = getattr(args, "linux_docker_cli", None)
    cli_sha = getattr(args, "expected_linux_docker_cli_sha256", None)
    require(bool(native) != bool(args.runner_image_id), "select_exact_runner_image_or_native_binary")
    images = (args.gui_image_id, core) if native else (args.runner_image_id, args.gui_image_id, core)
    require(all(isinstance(value, str) and IMAGE_ID.fullmatch(value) for value in images), "exact_local_image_ids_required")
    if native:
        require(Path(native).is_absolute() and cli is not None and Path(cli).is_absolute(),
                "absolute_native_runner_and_linux_docker_cli_required")
        require(isinstance(cli_sha, str) and SHA256.fullmatch(cli_sha), "explicit_linux_docker_cli_sha256_required")
    else:
        require(cli is None and cli_sha is None, "linux_docker_cli_is_for_native_runner_only")
    require(args.docker_context == "desktop-linux", "explicit_parent_desktop_linux_context_required")
    require(args.baseline_count == 39, "original39_baseline_required")
    require(Path(args.docker_client).is_absolute(), "absolute_parent_docker_client_required")
    require(Path(args.baseline).is_absolute(), "absolute_original39_baseline_required")
    return {"server_bin": Path(args.server_bin), "server_sha": args.expected_sha256,
        "runner_sha": args.expected_runner_sha256, "migration": args.expected_migration,
        "runner_image": args.runner_image_id, "gui_image": args.gui_image_id, "core_image": core,
        "runner_binary": Path(native) if native else None, "linux_docker_cli": Path(cli) if cli else None,
        "linux_docker_cli_sha": cli_sha, "runner_mode": "native" if native else "container",
        "include_api_desktop_phase": bool(getattr(args, "include_api_desktop_phase", False)),
        "diagnostic_only": bool(getattr(args, "diagnostic_only", False)),
        "docker_client": Path(args.docker_client), "docker_context": args.docker_context,
        "baseline": Path(args.baseline), "baseline_count": args.baseline_count, "report": Path(args.report)}


def checked_native_elf(path, expected, check):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink()
            and os.access(path, os.X_OK) and path.stat().st_size <= 120 * 1024 * 1024, check)
    with path.open("rb") as source:
        require(source.read(4) == b"\x7fELF", check)
    require(file_sha256(path) == expected, check)
    return path


def validate_native_inputs(inputs):
    """No parent commands or sockets may precede these two independent gates."""
    if inputs["runner_mode"] == "native":
        checked_native_elf(inputs["runner_binary"], inputs["runner_sha"], "exact_approved_native_runner_binary")
        checked_native_elf(inputs["linux_docker_cli"], inputs["linux_docker_cli_sha"], "exact_approved_linux_docker_cli")


def owned_native_copy(source, destination, expected, check):
    shutil.copyfile(source, destination)
    destination.chmod(0o500)
    return checked_native_elf(destination, expected, check)


def binding_bytes(binding):
    require(isinstance(binding, dict) and set(binding) == set(BINDING_FIELDS), "exact_private_binding_shape")
    return json.dumps({field: binding[field] for field in BINDING_FIELDS}, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def binding_digest(binding):
    return hashlib.sha256(binding_bytes(binding)).hexdigest()


def load_runner_helpers():
    path = Path(__file__).resolve().parents[2] / "cowork-runner" / "tests" / "qualify_desktop_identity.py"
    require(path.is_file(), "real_runner_fixture_source_present")
    spec = importlib.util.spec_from_file_location("cowork_owned_real_runner_fixture", path)
    require(spec is not None and spec.loader is not None, "real_runner_fixture_importable")
    module = importlib.util.module_from_spec(spec)
    import sys
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    require(all(hasattr(module, name) for name in ("ParentDocker", "RunnerFixture", "RunnerProtocol")),
            "real_runner_fixture_contract")
    return module, path


class ModelScenario:
    def __init__(self):
        self.id = str(uuid.uuid4())
        self.ready = threading.Event()
        self.release = threading.Event()
        self.requests = 0
        self.tool_deliveries = 0


def model_tool_message(identifier):
    identifier = canonical_uuid(identifier)
    return {"role": "assistant", "content": None, "tool_calls": [{
        "id": "owned-" + uuid.UUID(identifier).hex, "type": "function", "function": {
            "name": "DesktopOpenOffice", "arguments": json.dumps({
                "path": "artifacts/ha-fixture-" + uuid.UUID(identifier).hex + ".txt"})}}]}


class OwnedHttpServer(http.server.ThreadingHTTPServer):
    """Bounded accepted-socket/TLS handshakes and explicitly joined handlers."""
    daemon_threads = True

    def __init__(self, address, handler, tls=None):
        self.owned_lock = threading.Lock()
        self.owned_connections = set()
        self.owned_handlers = set()
        self.tls = tls
        super().__init__(address, handler)

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(5)
        try:
            if self.tls:
                connection = self.tls.wrap_socket(connection, server_side=True)
            with self.owned_lock:
                self.owned_connections.add(connection)
            return connection, address
        except Exception:
            connection.close()
            raise

    def process_request_thread(self, request, address):
        handler = threading.current_thread()
        with self.owned_lock:
            self.owned_handlers.add(handler)
        try:
            super().process_request_thread(request, address)
        finally:
            with self.owned_lock:
                self.owned_connections.discard(request)
                self.owned_handlers.discard(handler)

    def close_owned(self, serving_thread):
        with self.owned_lock:
            connections = list(self.owned_connections)
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        self.shutdown()
        self.server_close()
        deadline = time.monotonic() + 6
        serving_thread.join(timeout=max(0, deadline - time.monotonic()))
        with self.owned_lock:
            handlers = list(self.owned_handlers)
        for handler in handlers:
            handler.join(timeout=max(0, deadline - time.monotonic()))
        require(not serving_thread.is_alive() and not any(handler.is_alive() for handler in handlers),
                "owned_http_threads_cleanup")


class PrivateModel:
    """One DesktopOpenOffice delivery; the next model response is bounded/held."""
    def __init__(self, port):
        self.lock = threading.Lock()
        self.scenarios = {}
        self.errors = []
        self.stopping = threading.Event()
        fixture = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *arguments):
                pass
            def do_POST(self):
                try:
                    length = self.headers.get("Content-Length", "")
                    require(self.path == "/v1/chat/completions" and length.isdecimal()
                            and 0 < int(length) <= MAX_MODEL_BODY, "private_model_request_shape")
                    body = self.rfile.read(int(length))
                    require(len(body) == int(length), "private_model_complete_request")
                    request = json.loads(body)
                    identifiers = {match for message in request.get("messages", [])
                        if isinstance(message.get("content"), str)
                        for match in re.findall(r"ha-fixture:([0-9a-f-]{36})", message["content"])}
                    require(len(identifiers) == 1, "private_model_owned_scenario")
                    identifier = canonical_uuid(next(iter(identifiers)))
                    with fixture.lock:
                        scenario = fixture.scenarios.get(identifier)
                        require(scenario is not None, "private_model_known_scenario")
                        scenario.requests += 1
                        first = scenario.requests == 1
                    if first:
                        require(scenario.ready.wait(60) and not fixture.stopping.is_set(),
                                "private_model_workspace_registration_barrier")
                        require(any(tool.get("function", {}).get("name") == "DesktopOpenOffice"
                                    for tool in request.get("tools", [])), "worker_gui_tool_is_available")
                        with fixture.lock:
                            scenario.tool_deliveries += 1
                        message = model_tool_message(identifier)
                    else:
                        deadline = time.monotonic() + 180
                        while not scenario.release.wait(.1):
                            require(not fixture.stopping.is_set() and time.monotonic() < deadline,
                                    "private_model_owned_hold_deadline")
                        message = {"role": "assistant", "content": "Owned qualification complete."}
                    response = json.dumps({"choices": [{"message": message}],
                                           "usage": {"prompt_tokens": 1, "completion_tokens": 1}}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(response)))
                    self.end_headers()
                    self.wfile.write(response)
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    pass
                except Exception as error:
                    with fixture.lock:
                        fixture.errors.append(str(error) if isinstance(error, QualificationFailure)
                                              else "private_model_internal_failure")
                    self.close_connection = True
        self.server = OwnedHttpServer(("127.0.0.1", port), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, name="owned-ha-model")
        self.thread.start()
        self.origin = f"http://127.0.0.1:{port}/v1"

    def new(self):
        scenario = ModelScenario()
        with self.lock:
            self.scenarios[scenario.id] = scenario
        return scenario

    def close(self):
        self.stopping.set()
        with self.lock:
            for scenario in self.scenarios.values():
                scenario.ready.set()
                scenario.release.set()
        self.server.close_owned(self.thread)


TRACE_COUNTER_MAX = (1 << 53) - 1
TRACE_ERRORS = {"timeout", "tls_verification", "tls_closed", "tls_error", "connection_reset",
    "connection_refused", "broken_pipe", "io_error", "contract_refused", "invalid_metadata", "unexpected_error"}
TRACE_PHASES = {"accepted", "tls_connect", "upstream_request", "upstream_response", "upgrade_forward", "pump", "finished"}
TRACE_DIRECTIONS = {"client_to_runner", "runner_to_client"}


def transport_error_class(error):
    # Never include an exception message: TLS/HTTP errors can contain secrets.
    for kind, code in [(TimeoutError, "timeout"), (ssl.SSLCertVerificationError, "tls_verification"),
            ((ssl.SSLZeroReturnError, ssl.SSLEOFError), "tls_closed"), (ssl.SSLError, "tls_error"),
            (ConnectionResetError, "connection_reset"), (ConnectionRefusedError, "connection_refused"),
            (BrokenPipeError, "broken_pipe"), (OSError, "io_error"), (QualificationFailure, "contract_refused"),
            ((ValueError, KeyError, TypeError), "invalid_metadata")]:
        if isinstance(error, kind):
            return code
    return "unexpected_error"


class TransportTrace:
    """Bounded metadata about raw websocket bytes; never retains their contents."""
    def __init__(self, index):
        require(type(index) is int and 0 <= index <= 4096, "observer_stream_index_bound")
        self.lock = threading.Lock()
        self.value = {"stream_index": index, "upstream_status": None, "upgrade_completed": False,
            "upstream_prefix_bytes": 0, "phase": "accepted", "error_class": None,
            "finished": False, "counter_saturated": False, "error_direction": None, "error_operation": None,
            **{direction: {"read_bytes": 0, "sent_bytes": 0, "pending_bytes": 0,
                 "source_eof": False, "destination_eof": False} for direction in TRACE_DIRECTIONS}}

    def metadata(self, **fields):
        require(set(fields) <= {"upstream_status", "upgrade_completed", "upstream_prefix_bytes", "phase", "error_class", "finished"},
                "observer_fixed_metadata_required")
        for key, value in fields.items():
            valid = (key in {"upgrade_completed", "finished"} and type(value) is bool
                or key == "upstream_status" and type(value) is int and 100 <= value <= 599
                or key == "upstream_prefix_bytes" and type(value) is int and 0 <= value <= PROXY_BUFFER
                or key == "phase" and value in TRACE_PHASES
                or key == "error_class" and value in TRACE_ERRORS)
            require(valid, "observer_typed_metadata_required")
        with self.lock:
            self.value.update(fields)

    def note(self, direction, event, count=0):
        require(direction in TRACE_DIRECTIONS and event in {"read", "sent", "source_eof", "destination_eof", "read_error", "sent_error"}
                and type(count) is int and 0 <= count <= PROXY_BUFFER, "observer_fixed_counter_event_required")
        with self.lock:
            row = self.value[direction]
            if event in {"read", "sent"}:
                field = "read_bytes" if event == "read" else "sent_bytes"
                total = row[field] + count
                self.value["counter_saturated"] |= total > TRACE_COUNTER_MAX
                row[field] = min(TRACE_COUNTER_MAX, total)
                row["pending_bytes"] = min(PROXY_BUFFER, max(0, row["read_bytes"] - row["sent_bytes"]))
            elif event in {"read_error", "sent_error"}:
                self.value["error_direction"] = direction
                self.value["error_operation"] = "read" if event == "read_error" else "send"
            else:
                row[event] = True

    def snapshot(self):
        with self.lock:
            return {key: dict(value) if isinstance(value, dict) else value for key, value in self.value.items()}


def forward_rfb_upgrade(client, headers, remainder, trace):
    """Count a coalesced 101+RFB prefix even when send() completes partially."""
    require(len(headers) <= MAX_HTTP_HEADER and len(remainder) <= PROXY_BUFFER, "observer_upgrade_prefix_bound")
    trace.metadata(phase="upgrade_forward", upstream_prefix_bytes=len(remainder))
    trace.note("runner_to_client", "read", len(remainder))
    prefix = headers + b"\r\n\r\n"
    packet, offset = prefix + remainder, 0
    while offset < len(packet):
        try:
            count = client.send(packet[offset:])
        except OSError:
            trace.note("runner_to_client", "sent_error")
            raise
        require(type(count) is int and 0 <= count <= len(packet) - offset, "observer_upgrade_send_bound")
        if not count:
            trace.note("runner_to_client", "destination_eof")
            raise QualificationFailure("observer_upgrade_destination_eof")
        raw_count = max(0, offset + count - len(prefix)) - max(0, offset - len(prefix))
        trace.note("runner_to_client", "sent", raw_count)
        offset += count
    trace.metadata(upgrade_completed=True, phase="pump")


def pump_tls(client, upstream, stopping, observe=None):
    """Fixed-origin bridge with bounded backpressure and no redirect traversal."""
    client.setblocking(False)
    upstream.setblocking(False)
    buffers = {client: bytearray(), upstream: bytearray()}
    opposite = {client: upstream, upstream: client}
    directions = {client: "client_to_runner", upstream: "runner_to_client"}
    def note(direction, event, count=0):
        if observe is not None:
            observe(direction, event, count)
    while not stopping.is_set():
        readable = [side for side in opposite if len(buffers[opposite[side]]) < PROXY_BUFFER]
        writable = [side for side in opposite if buffers[side]]
        ready, write_ready, _ = select.select(readable, writable, [], .1)
        for side in readable:
            if isinstance(side, ssl.SSLSocket) and side.pending() and side not in ready:
                ready.append(side)
        for side in write_ready:
            try:
                count = side.send(buffers[side])
            except (BlockingIOError, ssl.SSLWantReadError, ssl.SSLWantWriteError):
                continue
            except OSError:
                note(directions[opposite[side]], "sent_error")
                raise
            require(type(count) is int and 0 <= count <= len(buffers[side]), "observer_pump_send_bound")
            if not count:
                note(directions[opposite[side]], "destination_eof")
                return
            note(directions[opposite[side]], "sent", count)
            del buffers[side][:count]
        for side in ready:
            try:
                body = side.recv(min(16384, PROXY_BUFFER - len(buffers[opposite[side]])))
            except (BlockingIOError, ssl.SSLWantReadError, ssl.SSLWantWriteError):
                continue
            except OSError:
                note(directions[side], "read_error")
                raise
            if not body:
                note(directions[side], "source_eof")
                return
            require(len(body) <= PROXY_BUFFER - len(buffers[opposite[side]]), "observer_pump_read_bound")
            note(directions[side], "read", len(body))
            buffers[opposite[side]].extend(body)


class RunnerObserver:
    """Owned TLS fault/observation hop; all successful job proofs come upstream.

    It never signs a Runner response. It can corrupt/refuse/delay an already
    genuine Probe, or lose one response after the real Runner performed Stop.
    """
    def __init__(self, port, origin, ca, certificate, key, protocol):
        upstream = urlsplit(origin)
        require(upstream.scheme == "https" and upstream.hostname in {"localhost", "127.0.0.1"}
                and not upstream.username and not upstream.password and not upstream.query
                and not upstream.fragment and upstream.path in {"", "/"}, "fixed_private_runner_origin")
        self.upstream = upstream
        self.upstream_tls = ssl.create_default_context(cafile=str(ca))
        self.lock = threading.Lock()
        self.records = []
        self.verified_creations = {}
        self.verify_creations = False
        self.lost_proven_stops = set()
        self.faults = {}
        self.stopping = threading.Event()
        self.connections = set()
        observer = self
        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            def log_message(self, *arguments):
                pass
            def do_POST(self):
                self.forward()
            def do_DELETE(self):
                self.forward()
            def do_GET(self):
                self.forward()
            def forward(self):
                raw, connection, trace = None, None, None
                try:
                    length = self.headers.get("Content-Length", "0")
                    require(length.isdecimal() and 0 <= int(length) <= MAX_PROXY_BODY
                            and not self.headers.get("Transfer-Encoding"), "owned_runner_observer_body_bound")
                    body = self.rfile.read(int(length))
                    require(len(body) == int(length), "owned_runner_observer_complete_body")
                    route = urlsplit(self.path)
                    require(not route.scheme and not route.netloc and self.path.startswith("/v"),
                            "owned_runner_observer_fixed_target")
                    session = None
                    if self.command == "POST" and route.path.endswith("desktop-sessions"):
                        session = canonical_uuid(json.loads(body)["session_id"])
                        kind = "create"
                    elif route.path == "/v1/desktop-session-probe":
                        session = canonical_uuid(json.loads(body)["binding"]["session_id"])
                        kind = "probe"
                    elif re.fullmatch(r"/v1/desktop-sessions/[0-9a-f-]{36}(?:/stream)?", route.path):
                        session = canonical_uuid(route.path.split("/")[3])
                        kind = "stop" if self.command == "DELETE" else "rfb"
                    else:
                        kind = "other"
                    with observer.lock:
                        require(len(observer.records) < 4096, "owned_observer_record_bound")
                        if kind == "rfb":
                            trace = TransportTrace(sum(row["kind"] == "rfb" for row in observer.records))
                        observer.records.append({"kind": kind, "session": session, "trace": trace})
                        fault = observer.faults.get(session)
                    if trace:
                        trace.metadata(phase="tls_connect")
                    raw = socket.create_connection((upstream.hostname, upstream.port or 443), timeout=5)
                    connection = observer.upstream_tls.wrap_socket(raw, server_hostname=upstream.hostname)
                    connection.settimeout(40)
                    with observer.lock:
                        observer.connections.update((self.connection, connection))
                    fields = [(name, value) for name, value in self.headers.items()
                              if name.lower() not in {"host", "connection", "content-length"}]
                    upgrade = self.headers.get("Upgrade", "").lower() == "websocket"
                    fields.extend((name, value) for name, value in [
                        ("Host", upstream.netloc), ("Content-Length", str(len(body))),
                        ("Connection", "Upgrade" if upgrade else "close")])
                    header = f"{self.command} {self.path} HTTP/1.1\r\n" + "".join(f"{k}: {v}\r\n" for k, v in fields) + "\r\n"
                    if trace:
                        trace.metadata(phase="upstream_request")
                    connection.sendall(header.encode("ascii") + body)
                    if trace:
                        trace.metadata(phase="upstream_response")
                    received = bytearray()
                    while b"\r\n\r\n" not in received:
                        require(len(received) < MAX_HTTP_HEADER, "owned_runner_observer_header_bound")
                        part = connection.recv(4096)
                        require(bool(part), "owned_runner_observer_upstream_response")
                        received.extend(part)
                    headers, remainder = bytes(received).split(b"\r\n\r\n", 1)
                    lines = headers.decode("iso-8859-1").split("\r\n")
                    status = int(lines[0].split()[1])
                    if trace:
                        trace.metadata(upstream_status=status)
                    if upgrade and status == 101:
                        require(trace is not None, "owned_observer_exact_rfb_upgrade_required")
                        forward_rfb_upgrade(self.connection, headers, remainder, trace)
                        pump_tls(self.connection, connection, observer.stopping, trace.note)
                        self.close_connection = True
                        return
                    response_headers = [(name.strip(), value.strip()) for line in lines[1:]
                                        if ":" in line for name, value in [line.split(":", 1)]]
                    values = {name.lower(): value for name, value in response_headers}
                    require(values.get("content-length", "").isdecimal()
                            and int(values["content-length"]) <= MAX_PROXY_BODY
                            and "transfer-encoding" not in values, "owned_runner_observer_response_bound")
                    size = int(values["content-length"])
                    response = bytearray(remainder)
                    while len(response) < size:
                        part = connection.recv(min(65536, size - len(response)))
                        require(bool(part), "owned_runner_observer_complete_response")
                        response.extend(part)
                    require(len(response) == size, "owned_runner_observer_exact_response")
                    if kind == "create" and status == 200 and observer.verify_creations:
                        binding = assert_real_creation_reply(protocol, self.command, self.path, self.headers,
                            body, response_headers, bytes(response))
                        with observer.lock:
                            observer.verified_creations[session] = binding
                    if kind == "stop" and fault == "lose_stop_response" and status == 200:
                        assert_real_stop_reply(protocol, self.command, self.path, self.headers,
                                               response_headers, bytes(response))
                        with observer.lock:
                            observer.faults.pop(session, None)
                            observer.lost_proven_stops.add(session)
                        self.close_connection = True
                        return
                    if kind == "probe" and fault == "delayed_probe":
                        observer.stopping.wait(6)
                    if kind == "probe" and fault == "refused_probe":
                        status, response = 503, bytearray(b'{}')
                        response_headers = [("Content-Length", str(len(response)))]
                    self.send_response(status)
                    for name, value in response_headers:
                        if name.lower() in {"connection", "server", "date"}:
                            continue
                        if kind == "probe" and fault == "invalid_proof" and name.lower() == "x-cowork-desktop-proof":
                            value = "v1:" + "00" * 32
                        self.send_header(name, value)
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(response)
                    self.close_connection = True
                except (OSError, ssl.SSLError, QualificationFailure, ValueError, KeyError, RuntimeError) as error:
                    if trace:
                        trace.metadata(error_class=transport_error_class(error))
                    self.close_connection = True
                finally:
                    if trace:
                        trace.metadata(finished=True)
                    if connection is not None:
                        connection.close()
                    elif raw is not None:
                        raw.close()
                    with observer.lock:
                        observer.connections.discard(connection)
                        observer.connections.discard(self.connection)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(certificate, key)
        self.server = OwnedHttpServer(("127.0.0.1", port), Handler, tls=tls)
        self.thread = threading.Thread(target=self.server.serve_forever, name="owned-real-runner-observer")
        self.thread.start()
        self.origin = f"https://127.0.0.1:{port}"

    def fault(self, session, mode):
        require(mode in {"invalid_proof", "delayed_probe", "refused_probe", "lose_stop_response"},
                "owned_runner_observer_allowlisted_fault")
        with self.lock:
            self.faults[canonical_uuid(session)] = mode

    def count(self, kind, session):
        with self.lock:
            return sum(row["kind"] == kind and row["session"] == session for row in self.records)

    def transport_snapshot(self, session=None):
        # Public snapshots omit private record/session data entirely.
        if session is not None:
            session = canonical_uuid(session)
        with self.lock:
            selected = [row["trace"] for row in self.records if row["trace"] is not None
                        and (session is None or row["session"] == session)]
        return [trace.snapshot() for trace in selected[-(16 if session else 64):]]

    def close(self):
        self.stopping.set()
        with self.lock:
            connections = list(self.connections)
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()
        self.server.close_owned(self.thread)


def assert_real_stop_reply(protocol, method, path, request_headers, response_headers, response):
    """Independently verify the real upstream proof before injecting loss."""
    headers = Message()
    for name, value in response_headers:
        headers[name] = value
    protocol.verify({"status": 200, "headers": headers, "body": response, "request": b"",
        "fields": [method, path, request_headers["x-cowork-timestamp"], request_headers["x-cowork-nonce"]]},
        b"open-cowork/runner-desktop-stop/v1\0")
    result = json.loads(response)
    require(method == "DELETE" and set(result) == {"version", "binding", "execution_stopped"}
            and result["version"] == 1 and result["execution_stopped"] is True
            and parse_qs(urlsplit(path).query) == {"binding_sha256": [binding_digest(result["binding"])]},
            "lost_stop_was_a_real_exact_request_bound_runner_proof")


def assert_real_creation_reply(protocol, method, path, request_headers, request, response_headers, response):
    """Verify upstream's CreationReceipt, never mint fixture authority."""
    route = urlsplit(path)
    query = parse_qs(route.query, keep_blank_values=True, strict_parsing=True)
    require(method == "POST" and route.path in {"/v1/desktop-sessions", "/v2/desktop-sessions"}
            and not route.scheme and not route.netloc and not route.fragment
            and set(query) == {"lease_token", "attempt_id"}
            and all(len(values) == 1 for values in query.values()), "real_creation_exact_signed_query")
    lease, attempt = (canonical_uuid(query[field][0]) for field in ["lease_token", "attempt_id"])
    headers = Message()
    for name, value in response_headers:
        headers[name] = value
    _, binding = protocol.creation_reply({"status": 200, "headers": headers, "body": response, "request": request,
        "fields": [method, path, request_headers["x-cowork-timestamp"], request_headers["x-cowork-nonce"]]},
        json.loads(request), lease, attempt)
    return binding


def same_live_worker_lease(before, after, *, renewed=False):
    require(before["state"] == after["state"] == "running" and before["live"] is True and after["live"] is True
            and all(canonical_uuid(before[key]) == canonical_uuid(after[key]) for key in ["owner", "token", "workspace"])
            and (after["expiry"] > before["expiry"] if renewed else after["expiry"] >= before["expiry"]),
            "api_created_desktop_keeps_same_live_worker_lease")


class DeadlineSocket:
    """One monotonic deadline, including slow-drip send/recv and TLS setup."""
    def __init__(self, stream, deadline):
        self.stream, self.deadline, self.requested_timeout = stream, deadline, None

    def remaining(self):
        value = self.deadline - time.monotonic()
        if value <= 0:
            raise TimeoutError
        return value

    def settimeout(self, seconds):
        self.requested_timeout = seconds

    def arm(self):
        value = self.remaining()
        self.stream.settimeout(min(value, self.requested_timeout) if self.requested_timeout is not None else value)

    def recv(self, count):
        self.arm()
        return self.stream.recv(count)

    def sendall(self, body):
        offset = 0
        while offset < len(body):
            self.arm()
            count = self.stream.send(body[offset:])
            require(type(count) is int and 0 < count <= len(body) - offset, "direct_runner_send_complete")
            offset += count

    def shutdown(self, how):
        return self.stream.shutdown(how)

    def close(self):
        self.stream.close()


@contextmanager
def direct_runner_tls(protocol, deadline):
    target = urlsplit(protocol.origin)
    require(target.scheme == "https" and target.hostname in {"localhost", "127.0.0.1"}
            and target.path in {"", "/"} and not target.query and not target.fragment
            and not target.username and not target.password, "direct_owned_runner_origin_required")
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError
    tcp = socket.create_connection((target.hostname, target.port or 443), timeout=remaining)
    stream = None
    try:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError
        tcp.settimeout(remaining)
        stream = protocol.context.wrap_socket(tcp, server_hostname=target.hostname)
        yield DeadlineSocket(stream, deadline)
    finally:
        if stream is not None:
            stream.close()
        tcp.close()


def direct_response_head(stream):
    received = bytearray()
    while b"\r\n\r\n" not in received:
        require(len(received) < MAX_HTTP_HEADER, "direct_runner_header_bound")
        part = stream.recv(min(4096, MAX_HTTP_HEADER - len(received)))
        require(bool(part), "direct_runner_response_complete")
        received.extend(part)
    head, remainder = bytes(received).split(b"\r\n\r\n", 1)
    lines = head.decode("iso-8859-1").split("\r\n")
    require(re.fullmatch(r"HTTP/1\.[01] [1-5][0-9]{2}(?: .*|)", lines[0]), "direct_runner_status_required")
    headers = Message()
    for line in lines[1:]:
        require(":" in line, "direct_runner_response_header_required")
        name, value = line.split(":", 1)
        headers[name.strip()] = value.strip()
    return int(lines[0].split()[1]), headers, remainder


def direct_runner_readonly(protocol, binding, *, seconds=12, progress=None):
    """Fresh signed Probe and read-only RFB; no Create, Stop or control command."""
    require(type(seconds) in {int, float} and 0 < seconds <= 15, "direct_runner_deadline_bound")
    binding_bytes(binding)
    deadline = time.monotonic() + seconds
    result = progress if progress is not None else {}
    result.update(phase="probe", fresh_probe_confirmed=False, upgrade_status=None, framebuffer_bytes=0)
    target = urlsplit(protocol.origin)
    path, method = "/v1/desktop-session-probe", "POST"
    challenge = str(uuid.uuid4())
    body = json.dumps({"version": 1, "challenge": challenge, "binding": binding}, separators=(",", ":")).encode()
    fields = protocol.headers(method, path, body)
    probe_started = time.monotonic()
    with direct_runner_tls(protocol, min(deadline, probe_started + 5)) as stream:
        request = (f"POST {path} HTTP/1.1\r\nHost: {target.netloc}\r\nConnection: close\r\n"
            f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n"
            + "".join(f"{name}: {value}\r\n" for name, value in fields.items()) + "\r\n").encode() + body
        stream.sendall(request)
        status, headers, initial = direct_response_head(stream)
        lengths = headers.get_all("Content-Length") or []
        require(len(lengths) == 1 and lengths[0].isdecimal() and int(lengths[0]) <= MAX_PROXY_BODY
                and not headers.get_all("Transfer-Encoding"), "direct_runner_body_bound")
        response = bytearray(initial)
        size = int(lengths[0])
        while len(response) < size:
            part = stream.recv(min(65536, size - len(response)))
            require(bool(part), "direct_runner_body_complete")
            response.extend(part)
        require(len(response) == size, "direct_runner_body_exact")
        protocol.verify({"status": status, "headers": headers, "body": bytes(response), "request": body,
            "fields": [method, path, fields["x-cowork-timestamp"], fields["x-cowork-nonce"]]},
            b"open-cowork/runner-desktop-probe/v1\0")
        reply = json.loads(response)
        require(time.monotonic() < min(deadline, probe_started + 5) and reply["version"] == 1
                and reply["challenge"] == challenge and reply["job"]["binding"] == binding
                and reply["job"]["running"] is True and reply["job"]["paused"] is False
                and reply["job"]["restarting"] is False, "direct_runner_fresh_exact_probe_required")
    result["fresh_probe_confirmed"] = True
    result["phase"] = "rfb_upgrade"
    path = "/v1/desktop-sessions/" + binding["session_id"] + "/stream?control=false&binding_sha256=" + binding_digest(binding)
    fields = protocol.headers("GET", path, b"")
    key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
    with direct_runner_tls(protocol, deadline) as stream:
        request = (f"GET {path} HTTP/1.1\r\nHost: {target.netloc}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n"
            f"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
            + "".join(f"{name}: {value}\r\n" for name, value in fields.items()) + "\r\n").encode()
        stream.sendall(request)
        status, headers, initial = direct_response_head(stream)
        result["upgrade_status"] = status
        require(status == 101 and headers.get_all("Sec-WebSocket-Accept") == [websocket_accept(key)]
                and headers.get("Upgrade", "").lower() == "websocket"
                and "upgrade" in headers.get("Connection", "").lower(), "direct_runner_genuine_upgrade_required")
        wire = WireSocket(stream, initial)
        result["phase"] = "rfb_framebuffer"
        try:
            result["framebuffer_bytes"] = real_framebuffer(wire)
            stream.remaining()
        finally:
            wire.close()
    result["phase"] = "completed"
    # Deliberately omit name/pixel contents, hashes, nonce, headers and Binding.
    return result


class RfbReader:
    def __init__(self, wire, diagnostics=None):
        self.wire = wire
        self.buffer = bytearray()
        self.diagnostics = diagnostics
    def take(self, size):
        require(0 <= size <= 65536, "real_rfb_read_bound")
        deadline = time.monotonic() + 8
        while len(self.buffer) < size:
            message = self.wire.recv(timeout=max(.01, deadline - time.monotonic()))
            require(message is not None and message[0] == 2, "real_rfb_binary_transport")
            self.buffer.extend(message[1])
            require(len(self.buffer) <= 1024 * 1024, "real_rfb_stream_buffer_bound")
        answer = bytes(self.buffer[:size])
        del self.buffer[:size]
        if self.diagnostics is not None:
            self.diagnostics["bytes_consumed"] += size
        return answer


def rfb_report(wires):
    """Only numeric protocol metadata; never names, clipboard or pixel data."""
    return [{"stream_index": index, **wire.rfb_protocol}
            for index, wire in enumerate(wires)
            if isinstance(getattr(wire, "rfb_protocol", None), dict)]



def real_framebuffer(wire):
    try:
        return _real_framebuffer(wire)
    except TimeoutError:
        observer = getattr(wire, "rfb_failure_observer", None)
        if observer is not None:
            try:
                wire.rfb_protocol["failure_observation"] = observer()
            except Exception as error:
                wire.rfb_protocol["failure_observation"] = {"observation_error_class": transport_error_class(error)}
        raise


def _real_framebuffer(wire):
    # Retain the parsed header before each assertion, including failed reads.
    # Numeric byte offsets distinguish malformed headers from a parser offset
    # bug without recording any desktop, clipboard or server-name contents.
    diagnostics = {"handshake_step": 0, "bytes_consumed": 0,
                   "client_messages": [], "side_message_types": [],
                   "rectangles": [], "pixel_bytes_received": 0, "requested_pixels_received": 0}
    wire.rfb_protocol = diagnostics
    reader = RfbReader(wire, diagnostics)
    version = reader.take(12)
    require(version == b"RFB 003.008\n", "genuine_rfb_server_version")
    diagnostics["server_version"] = [3, 8]
    diagnostics["handshake_step"] = 1
    wire.binary(version)
    count = reader.take(1)[0]
    diagnostics["security_type_count"] = count
    require(0 < count <= 32, "genuine_rfb_none_auth_negotiation")
    types = reader.take(count)
    diagnostics["security_types"] = list(types)
    require(1 in types, "genuine_rfb_none_auth_negotiation")
    diagnostics["handshake_step"] = 2
    wire.binary(b"\x01")
    security = reader.take(4)
    diagnostics["security_result"] = struct.unpack("!I", security)[0]
    require(security == b"\x00" * 4, "genuine_rfb_security_result")
    diagnostics["handshake_step"] = 3
    wire.binary(b"\x01")
    initial = reader.take(24)
    width, height = struct.unpack("!HH", initial[:4])
    diagnostics["framebuffer"] = {"width": width, "height": height}
    format_fields = ("bits_per_pixel", "depth", "big_endian", "true_color",
                     "red_max", "green_max", "blue_max", "red_shift", "green_shift", "blue_shift")
    diagnostics["native_pixel_format"] = dict(zip(format_fields,
        struct.unpack("!BBBBHHHBBBxxx", initial[4:20])))
    require(4 <= width <= 4096 and 4 <= height <= 4096, "genuine_rfb_dimensions")
    wire.real_rfb_dimensions = (width, height)
    name_length = struct.unpack("!I", initial[20:24])[0]
    diagnostics["server_name_bytes"] = name_length
    require(name_length <= 16384, "genuine_rfb_server_name_bound")
    reader.take(name_length)
    diagnostics["handshake_step"] = 4
    pixel_format = struct.pack("!BBBBHHHBBBxxx", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
    wire.binary(b"\x00\x00\x00\x00" + pixel_format)
    diagnostics["client_messages"].append({"message_type": 0, "bytes": 20})
    diagnostics["requested_pixel_format"] = dict(zip(format_fields,
        struct.unpack("!BBBBHHHBBBxxx", pixel_format)))
    wire.binary(b"\x02\x00\x00\x01" + struct.pack("!i", 0))
    diagnostics["client_messages"].append({"message_type": 2, "bytes": 8})
    diagnostics["requested_encodings"] = [0]
    wire.binary(struct.pack("!BBHHHH", 3, 0, 0, 0, 4, 4))
    diagnostics["client_messages"].append({"message_type": 3, "bytes": 10})
    diagnostics["requested_rectangle"] = {"incremental": 0, "x": 0, "y": 0, "width": 4, "height": 4}
    diagnostics["handshake_step"] = 5
    for _ in range(8):
        message_offset = diagnostics["bytes_consumed"]
        kind = reader.take(1)[0]
        if kind == 0:
            break
        diagnostics["side_message_types"].append(kind)
        if kind == 2:  # Bell carries no body.
            continue
        require(kind == 3, "genuine_rfb_framebuffer_update")
        cut = reader.take(7)
        size = struct.unpack("!I", cut[3:])[0]
        require(size <= 65536, "genuine_rfb_clipboard_bound")
        reader.take(size)
    else:
        raise QualificationFailure("genuine_rfb_side_message_bound")
    update = reader.take(3)
    rectangles = struct.unpack("!H", update[1:])[0]
    diagnostics["update_header"] = {"message_type": kind, "padding": update[0],
        "rectangle_count": rectangles, "byte_offset": message_offset}
    require(0 < rectangles <= 16, "genuine_rfb_update_rectangle_bound")
    diagnostics["handshake_step"] = 6
    total, covered = 0, 0
    framebuffer_width, framebuffer_height = wire.real_rfb_dimensions
    # RFC 6143 sections 7.5.3/7.6.1 request the complete area and describe
    # multi-rectangle updates. The measured server also sends other in-bounds
    # rectangles. Consume their bounded raw payloads while independently
    # proving that every one of the requested 4x4 pixels actually arrived.
    for _ in range(rectangles):
        rectangle_offset = diagnostics["bytes_consumed"]
        x, y, width, height, encoding = struct.unpack("!HHHHi", reader.take(12))
        diagnostics["rectangles"].append({"x": x, "y": y, "width": width, "height": height,
            "encoding": encoding, "byte_offset": rectangle_offset})
        require(encoding == 0 and width > 0 and height > 0
                and x + width <= framebuffer_width and y + height <= framebuffer_height,
                "genuine_rfb_raw_requested_rectangle")
        remaining = width * height * 4
        require(total + remaining <= 8 * 1024 * 1024, "genuine_rfb_pixel_payload_bound")
        diagnostics["handshake_step"] = 7
        while remaining:
            count = min(remaining, 65536)
            reader.take(count)
            remaining -= count
            total += count
            diagnostics["pixel_bytes_received"] = total
        for row in range(y, min(4, y + height)):
            for column in range(x, min(4, x + width)):
                covered |= 1 << (row * 4 + column)
        diagnostics["requested_pixels_received"] = covered.bit_count()
    require(covered == 0xffff, "genuine_rfb_complete_requested_pixels")
    require(total >= 64, "genuine_rfb_actual_framebuffer_bytes")
    diagnostics["handshake_step"] = 8
    return total


def native_rss(child):
    require(child.poll() is None, "owned_native_api_alive_for_memory_observation")
    status = Path(f"/proc/{child.pid}/status").read_text(encoding="ascii")
    value = re.search(r"^VmRSS:\s+(\d+) kB$", status, re.MULTILINE)
    require(value is not None, "owned_native_api_memory_observable")
    return int(value[1]) * 1024


class LinuxFixture(ClusterFixture):
    def __init__(self, root, binary, expected_sha, parent, runner, protocol):
        super().__init__(root, binary, expected_sha)
        self.parent, self.runner, self.protocol = parent, runner, protocol
        self.model, self.observer, self.worker = None, None, None
        self.model_port, self.observer_port = self.reserve_port(), self.reserve_port()
        self.trust_overrides = {}
        self.scenarios = []
        self.cleanup_failures = []

    def prepare_private_services(self):
        cert, key, csr, ext = (self.root / ("runner-observer." + suffix) for suffix in ["pem", "key", "csr", "ext"])
        ext.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
                       "extendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n")
        self.command(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-sha256", "-subj", "/CN=Owned Runner observer",
                      "-keyout", str(key), "-out", str(csr)], "owned_observer_csr")
        self.command(["openssl", "x509", "-req", "-in", str(csr), "-CA", str(self.ca), "-CAkey", str(self.root / "ca.key"),
                      "-CAcreateserial", "-days", "2", "-sha256", "-extfile", str(ext), "-out", str(cert)], "owned_observer_cert")
        key.chmod(0o600)
        self.release_port(self.model_port)
        self.model = PrivateModel(self.model_port)
        self.release_port(self.observer_port)
        self.observer = RunnerObserver(self.observer_port, self.runner.origin, self.runner.ca_file, cert, key, self.protocol)
        self.runner_key_file = self.root / "runner-signing-key"
        key_bytes = self.runner.signing_key if isinstance(self.runner.signing_key, bytes) else self.runner.signing_key.encode()
        descriptor = os.open(self.runner_key_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(key_bytes)

    def runtime_environment(self):
        return {"COWORK_RUNNER_URL": self.observer.origin, "COWORK_RUNNER_SIGNING_KEY_FILE": str(self.runner_key_file),
            "COWORK_RUNNER_DESKTOP_JOB_PROOFS": "true", "COWORK_RUNNER_CA_CERTIFICATE_FILE": str(self.ca),
            "COWORK_MODEL_BASE_URL": self.model.origin, "COWORK_MODEL_NAME": "owned-private-model-fixture",
            "COWORK_SERVER_CAPABILITIES": "shell,files,desktop.linux", "COWORK_WORKER_MAX_CONCURRENCY": "1",
            "COWORK_LEASE_SECONDS": "12", "COWORK_WORKER_SHUTDOWN_GRACE_SECONDS": "5",
            "COWORK_CHAT_WORKSPACES_ENABLED": "true"}

    def node_environment(self, node):
        environment = super().node_environment(node)
        environment.update(self.runtime_environment())
        if node in self.trust_overrides:
            environment["COWORK_RUNNER_CA_CERTIFICATE_FILE"] = str(self.trust_overrides[node])
        return environment

    def http(self, port, path, method="GET", data=None, token=None, accept="application/json", request_id=None):
        headers = {"Accept": accept}
        if token:
            headers["Authorization"] = "Bearer " + token
        if request_id:
            headers["X-Request-ID"] = request_id
        if data is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(data).encode()
        request = Request(f"http://127.0.0.1:{port}{path}", data=data, headers=headers, method=method)
        try:
            response = self.opener.open(request, timeout=25)
        except HTTPError as error:
            response = error
        with response:
            self.last_response_headers = {key.lower(): value for key, value in response.headers.items()}
            return response.status, response.headers.get("Content-Type", ""), response.read()

    def websocket(self, node, path, token=None, *, expected=101, receive_buffer=None):
        # A deliberately stale Probe takes five seconds to reject. The client
        # must observe that HTTP rejection rather than abandon the handshake.
        connection = socket.create_connection(("127.0.0.1", self.public_tls_ports[node]), timeout=12)
        try:
            if receive_buffer:
                connection.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, receive_buffer)
            connection = self.client_tls.wrap_socket(connection, server_hostname=f"node{node}.internal")
            key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
            headers = [f"GET {path} HTTP/1.1", "Host: localhost", "Upgrade: websocket", "Connection: Upgrade",
                "Sec-WebSocket-Version: 13", f"Sec-WebSocket-Key: {key}"]
            if token:
                headers.append("Authorization: Bearer " + token)
            connection.sendall(("\r\n".join(headers) + "\r\n\r\n").encode("ascii"))
            received = bytearray()
            while b"\r\n\r\n" not in received:
                require(len(received) < MAX_HTTP_HEADER, "fixture_ws_response_headers_bound")
                part = connection.recv(4096)
                require(bool(part), "fixture_ws_upgrade_complete")
                received.extend(part)
            header, remainder = bytes(received).split(b"\r\n\r\n", 1)
            lines = header.decode("iso-8859-1").split("\r\n")
            status = int(lines[0].split()[1])
            self.last_ws_status = {"node": node, "expected": expected, "actual": status}
            require(status == expected, "fixture_ws_expected_upgrade_status")
            if status != 101:
                connection.close()
                return None
            fields = {name.lower(): value.strip() for line in lines[1:] if ":" in line
                for name, value in [line.split(":", 1)]}
            require(fields.get("sec-websocket-accept") == websocket_accept(key)
                    and fields.get("upgrade", "").lower() == "websocket"
                    and "upgrade" in fields.get("connection", "").lower(), "fixture_ws_upgrade_authentic")
            wire = WireSocket(connection, remainder)
            self.wire_sockets.append(wire)
            return wire
        except Exception:
            connection.close()
            raise

    def create_worker_desktop(self):
        scenario = self.model.new()
        self.scenarios.append(scenario)
        thread = self.api(0, "/threads", method="POST", expected=201, data={
            "project_id": self.project["id"], "title": "Owned physical HA fixture"})
        row = self.api(0, "/runs", method="POST", expected=201, data={
            "project_id": self.project["id"], "project_revision": self.project["revision"], "thread_id": thread["id"],
            "project_privacy": "team_managed", "executor_target": {"kind": "server_linux", "pool_id": None},
            "required_capabilities": ["desktop.linux"], "idempotency_key": str(uuid.uuid4()),
            "input": {"prompt": "ha-fixture:" + scenario.id}})
        run = canonical_uuid(row["spec"]["id"])
        self.owned_runs.append(run)
        def claimed():
            lease = self.run_lease(run)
            return lease if lease["state"] == "running" and lease["live"] and lease["workspace"] else None
        lease = self.until(claimed, "worker_claimed_owned_workspace_before_gui_tool", seconds=30)
        self.runner.register_run(run, canonical_uuid(lease["workspace"]))
        scenario.ready.set()
        def available():
            value = self.sql(f"SELECT jsonb_build_object('session',d.id,'binding',d.runner_metadata->'runner_job_binding') FROM desktop_sessions d WHERE d.run_id='{run}'::uuid AND d.state='agent_controlled' AND d.runner_metadata->>'start_pending'='false';")
            if not value or scenario.requests < 2:
                return None
            record = json.loads(value)
            record["run"], record["scenario"] = run, scenario
            record["session"] = canonical_uuid(record["session"])
            binding_bytes(record["binding"])
            return record
        result = self.until(available, "worker_created_attested_desktop_timeout", seconds=70)
        require(self.desktop_owner(result["session"]) is None, "worker_desktop_initially_has_no_api_owner")
        require(self.observer.count("create", result["session"]) == 1 and scenario.tool_deliveries == 1,
                "worker_tool_and_physical_create_once")
        require(self.sql(f"SELECT count(*) FROM run_events WHERE run_id='{run}'::uuid AND kind='tool_completed' AND payload->>'tool'='DesktopOpenOffice';") == "1",
                "actual_worker_gui_tool_completed_successfully")
        self.require_identity(result)
        return result

    def require_identity(self, case):
        row = self.run_lease(case["run"])
        binding = case["binding"]
        require(binding["run_id"] == case["run"] and binding["session_id"] == case["session"]
                and binding["lease_token"] == row["token"] and binding["workspace_id"] == row["workspace"]
                and row["state"] == "running" and row["live"], "actual_creation_receipt_matches_live_worker_lease")
        physical = self.runner.inspect(binding)
        require(binding["engine_id"] == self.runner.actual_engine_id
                and physical["Id"] == binding["container_id"] and physical["State"]["Running"]
                and physical["Created"] == binding["created_at"]
                and physical["State"]["StartedAt"] == binding["started_at"], "receipt_matches_exact_live_docker_identity")
        self.protocol.probe(binding)

    def create_api_desktop(self, worker_case, node=0):
        run = canonical_uuid(worker_case["run"])
        before = self.run_lease(run)
        dimensions = {"width": 640, "height": 480, "scale_factor": 1.0}
        response = self.api(node, f"/runs/{run}/desktop-sessions", method="POST", expected=201,
            data={"width": dimensions["width"], "height": dimensions["height"]})
        session = canonical_uuid(response["id"])
        require(response["run_id"] == run and response["state"] == "agent_controlled"
                and response["stream_protocol"] == "rfb.binary.v1" and response["dimensions"] == dimensions,
                "public_api_desktop_create_response_exact")
        record = json.loads(self.sql(f"SELECT jsonb_build_object('state',state,'pending',runner_metadata->'start_pending','binding',runner_metadata->'runner_job_binding','lease',runner_metadata->>'start_lease_token','attempt',runner_metadata->>'start_attempt_id') FROM desktop_sessions WHERE id='{session}'::uuid AND run_id='{run}'::uuid;"))
        binding_bytes(record["binding"])
        with self.observer.lock:
            observed = self.observer.verified_creations.get(session)
        require(record["state"] == "agent_controlled" and record["pending"] is False
                and observed is not None and record["binding"] == observed
                and record["lease"] == before["token"] == observed["lease_token"]
                and record["attempt"] == observed["attempt_id"], "api_creation_has_genuine_request_bound_receipt")
        old = worker_case["binding"]
        require(session != worker_case["session"] and observed["container_id"] != old["container_id"]
                and observed["attempt_id"] != old["attempt_id"] and observed["reservation_id"] != old["reservation_id"],
                "api_create_is_new_session_attempt_and_physical_job_after_confirmed_stop")
        owner = self.desktop_owner(session)
        require(owner is not None and owner["node_id"] == self.node_ids[node]
                and owner["process_epoch"] == self.epoch(node) and owner["group_id"] == self.group
                and owner["executor_id"] is None and owner["connection_epoch"] is None and owner["credential_id"] is None,
                "api_creator_owns_desktop_before_first_user_stream")
        same_live_worker_lease(before, self.run_lease(run))
        case = {"run": run, "session": session, "binding": observed,
                "scenario": worker_case["scenario"], "dimensions": dimensions}
        self.require_identity(case)
        self.no_replay(case)
        return case

    def pointer_position(self, case):
        binding = case["binding"]
        physical = self.runner.inspect(binding)
        require(physical["Id"] == binding["container_id"] and physical["State"]["Running"]
                and physical["Created"] == binding["created_at"]
                and physical["State"]["StartedAt"] == binding["started_at"], "pointer_probe_exact_live_owned_docker_id")
        body = self.parent.binary("exec", "--user", "10001:10001", binding["container_id"],
            "python3", "-c", X11_POINTER_QUERY, timeout=10)
        require(len(body) <= 128, "owned_x11_pointer_response_bound")
        value = json.loads(body)
        require(isinstance(value, dict) and set(value) == {"x", "y"}
                and all(type(value[field]) is int and 0 <= value[field] < case["dimensions"][dimension]
                    for field, dimension in [("x", "width"), ("y", "height")]), "owned_x11_pointer_coordinates")
        return value

    def run_lease(self, run):
        run = canonical_uuid(run)
        return json.loads(self.sql(f"SELECT jsonb_build_object('state',state,'owner',lease_owner,'token',lease_token,'workspace',sandbox_workspace_id,'expiry',extract(epoch FROM lease_expires_at),'live',lease_expires_at>clock_timestamp()) FROM runs WHERE id='{run}'::uuid;"))

    def desktop_owner(self, session):
        raw = self.sql(f"SELECT row_to_json(o) FROM api_cluster_desktop_owners o WHERE desktop_session_id='{canonical_uuid(session)}'::uuid;")
        return json.loads(raw) if raw else None

    def live_stream(self, case, node=0, token=None, expected=101, receive_buffer=None):
        ticket = self.api(node, f"/runs/{case['run']}/desktop-sessions/{case['session']}/tickets", method="POST",
                          token=token, data={"control": False})
        wire = self.websocket(node, f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}",
                              expected=expected, receive_buffer=receive_buffer)
        if wire is not None:
            wire.rfb_failure_observer = lambda: self.observe_failed_rfb(case, node)
        return wire, ticket

    def observe_failed_rfb(self, case, node):
        """Read-only diagnostics on this exact live Docker identity; no inputs."""
        binding = case["binding"]
        physical = self.runner.inspect(binding)
        identity_matches = (physical["Id"] == binding["container_id"]
            and physical["State"]["Running"] is True
            and physical["Created"] == binding["created_at"]
            and physical["State"]["StartedAt"] == binding["started_at"])
        result = {"exact_live_docker_identity": identity_matches,
                  "worker_lease_live": self.run_lease(case["run"])["live"],
                  "rfb_requests_observed": self.observer.count("rfb", case["session"]),
                  "observer_transport": self.observer.transport_snapshot(case["session"])}
        relay = self.runner.relay
        if relay is not None:
            with relay.lock:
                result["relay_active_processes"] = len(relay.processes)
            result["relay_active_threads"] = sum(thread.is_alive() for thread in relay.threads)
        require(identity_matches, "rfb_observation_exact_owned_identity_required")
        script = r"""import json,socket,time
result={};deadline=time.monotonic()+8
def remaining():
 value=deadline-time.monotonic()
 if value<=0:raise TimeoutError
 return min(2,value)
for port in (5900,5901):
 item={'connected':False,'banner_matches':False}
 try:
  with socket.create_connection(('127.0.0.1',port),timeout=remaining()) as connection:
   item['connected']=True;data=b''
   while len(data)<12:
    connection.settimeout(remaining())
    part=connection.recv(12-len(data))
    if not part:break
    data+=part
   item['banner_matches']=data==b'RFB 003.008\n'
 except OSError as error:item['error_class']=type(error).__name__
 result[str(port)]=item
print(json.dumps(result,separators=(',',':')))
"""
        response = self.parent.run("exec", binding["container_id"], "python3", "-I", "-c", script, timeout=10)
        require(type(response) is str and len(response.encode()) <= 4096, "rfb_observation_response_bound")
        ports = json.loads(response)
        require(set(ports) == {"5900", "5901"}, "rfb_observation_exact_ports")
        for value in ports.values():
            require(isinstance(value, dict) and set(value) <= {"connected", "banner_matches", "error_class"}
                and type(value.get("connected")) is bool and type(value.get("banner_matches")) is bool
                and value.get("error_class", "") in {"", "TimeoutError", "ConnectionRefusedError", "OSError", "ConnectionResetError"},
                "rfb_observation_typed_port_metadata")
        result["ports"] = ports
        if getattr(self, "diagnostic_only", False):
            progress = {"phase": "authority", "authority_confirmed": False}
            try:
                result["direct_runner_readonly"] = self.direct_runner_observation(case, progress=progress)
            except Exception as error:
                result["direct_runner_readonly"] = {**progress, "error_class": transport_error_class(error)}
        return result

    def direct_runner_observation(self, case, *, progress=None):
        binding_bytes(case["binding"])
        binding = case["binding"]
        run, session = canonical_uuid(case["run"]), canonical_uuid(case["session"])
        before = self.run_lease(run)
        require(before["state"] == "running" and before["live"] is True and binding["run_id"] == run
                and binding["session_id"] == session and binding["lease_token"] == before["token"]
                and binding["workspace_id"] == before["workspace"], "direct_runner_current_worker_lease_required")
        raw = self.sql(f"SELECT jsonb_build_object('state',state,'binding',runner_metadata->'runner_job_binding','start_pending',runner_metadata->>'start_pending','cleanup_state',runner_metadata->>'cleanup_state') FROM desktop_sessions WHERE id='{session}'::uuid AND run_id='{run}'::uuid;")
        row = json.loads(raw) if raw else None
        require(row is not None and row["state"] in {"agent_controlled", "user_controlled", "paused"}
                and row["binding"] == binding and row["start_pending"] == "false"
                and row["cleanup_state"] != "stop_pending", "direct_runner_current_desktop_binding_required")
        physical = self.runner.inspect(binding)
        require(binding["engine_id"] == self.runner.actual_engine_id
                and physical["Id"] == binding["container_id"] and physical["State"]["Running"] is True
                and physical["Created"] == binding["created_at"]
                and physical["State"]["StartedAt"] == binding["started_at"], "direct_runner_exact_live_identity_required")
        if progress is not None:
            progress["authority_confirmed"] = True
        result = direct_runner_readonly(self.protocol, binding, progress=progress)
        same_live_worker_lease(before, self.run_lease(run))
        after_raw = self.sql(f"SELECT jsonb_build_object('state',state,'binding',runner_metadata->'runner_job_binding','start_pending',runner_metadata->>'start_pending','cleanup_state',runner_metadata->>'cleanup_state') FROM desktop_sessions WHERE id='{session}'::uuid AND run_id='{run}'::uuid;")
        after = json.loads(after_raw) if after_raw else None
        require(after is not None and after["state"] in {"agent_controlled", "user_controlled", "paused"}
                and after["binding"] == binding and after["start_pending"] == "false"
                and after["cleanup_state"] != "stop_pending", "direct_runner_desktop_authority_changed")
        self.no_replay(case)
        return result

    def control_stream(self, case, node=2):
        grant = self.api(node, "/auth/reauthenticate", method="POST",
            data={"password": self.admin_password, "purpose": "desktop_control"})
        ticket = self.api(node, f"/runs/{case['run']}/desktop-sessions/{case['session']}/tickets", method="POST",
            data={"control": True, "reauthentication_token": grant["token"]})
        require(ticket["control"] is True, "real_control_ticket_requires_reauthentication")
        wire = self.websocket(node, f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}")
        return wire, ticket, grant

    def invalidate_owned_lease_expectation(self, case):
        # Fault only this fixture's exact run/token, with the same workspace
        # contract acknowledgment as the product. This is not a successor claim.
        run = canonical_uuid(case["run"])
        token = canonical_uuid(case["binding"]["lease_token"])
        replacement = str(uuid.uuid4())
        result = self.sql("BEGIN; SET LOCAL cowork.chat_workspace_version='1'; "
            f"WITH changed AS (UPDATE runs SET lease_token='{replacement}'::uuid "
            f"WHERE id='{run}'::uuid AND lease_token='{token}'::uuid RETURNING 1) "
            "SELECT count(*) FROM changed; COMMIT;")
        require(result.splitlines() == ["BEGIN", "SET", "1", "COMMIT"],
                "owned_lease_expectation_exact_one_row")

    def refuse_old_lease_stream(self, case, node):
        # The real Worker's cleanup may fence the session between our lease
        # mutation and ticket minting. Either authenticated refusal is valid.
        status, _, body = self.http(self.api_ports[node],
            f"/api/v1/runs/{case['run']}/desktop-sessions/{case['session']}/tickets",
            method="POST", token=self.access_token, data={"control": False})
        self.last_api_status = {"node": node, "expected": "200_or_409", "actual": status}
        require(status in {200, 409}, "old_lease_authenticated_ticket_or_stream_refusal")
        if status == 200:
            ticket = json.loads(body)
            self.websocket(node, f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}", expected=409)

    def no_replay(self, case):
        require(self.observer.count("create", case["session"]) == 1
                and case["scenario"].tool_deliveries == 1
                and self.sql(f"SELECT count(*) FROM run_events WHERE run_id='{case['run']}'::uuid AND kind='tool_started' AND payload->>'tool'='DesktopOpenOffice';") == "1",
                "same_lease_owner_change_never_replays_tool_or_container_create")

    @contextmanager
    def real_phase(self, name):
        self.last_phase = name
        before_wire, before_sse, before_cases, before_runs = len(self.wire_sockets), len(self.streams), len(self.scenarios), len(self.owned_runs)
        self.parent.assert_preserved()
        try:
            yield
        finally:
            original_error = sys.exc_info()[0] is not None
            errors = []
            for wire in self.wire_sockets[before_wire:]:
                try:
                    wire.close()
                except Exception:
                    errors.append("owned_phase_wire_cleanup")
            for response in self.streams[before_sse:]:
                try:
                    response.close()
                except Exception:
                    errors.append("owned_phase_sse_cleanup")
            for scenario in self.scenarios[before_cases:]:
                scenario.ready.set()
                scenario.release.set()
            active = next((node for node, child in self.nodes.items() if child.poll() is None), None)
            if active is not None:
                for run in self.owned_runs[before_runs:]:
                    try:
                        self.api(active, f"/runs/{run}/cancel", method="POST", record_failure=False)
                    except Exception:
                        pass
                    try:
                        self.until(lambda: self.api(active, f"/runs/{run}")["state"] in {
                            "completed", "failed", "canceled", "interrupted", "expired"}, "owned_phase_run_terminal", seconds=30)
                    except Exception:
                        errors.append("owned_phase_run_cleanup")
            try:
                self.parent.assert_preserved()
            except Exception:
                errors.append("owned_phase_original39_cleanup")
            self.cleanup_failures.extend(errors)
            if not original_error:
                require(not errors, errors[0] if errors else "owned_phase_cleanup")

    def cleanup(self):
        saved = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        errors = []
        try:
            for scenario in self.scenarios:
                scenario.ready.set()
                scenario.release.set()
            for close in (super().cleanup, self.observer.close if self.observer else None,
                          self.model.close if self.model else None, self.parent.assert_preserved):
                if close:
                    try:
                        close()
                    except Exception:
                        errors.append("owned_ha_fixture_cleanup")
        finally:
            for number, handler in saved.items():
                signal.signal(number, handler)
        require(not errors, errors[0] if errors else "owned_ha_fixture_cleanup")


def private_control(fixture, row, target, *, certificate_node=None, role="desktop_stop", refused=False):
    origin = fixture.node_ids.index(row["origin_node_id"])
    request = {"protocol": 1, "group_id": fixture.group, "source_node_id": row["origin_node_id"],
        "source_process_epoch": row["origin_process_epoch"], "target_node_id": row["owner_node_id"],
        "target_process_epoch": row["owner_process_epoch"], "stream_id": row["ticket_id"],
        "role": role, "executor_id": None, "credential_id": None}
    if refused:
        fixture.private_refused(origin if certificate_node is None else certificate_node, target, request)
        return
    tls = ssl.create_default_context(cafile=str(fixture.ca))
    tls.load_cert_chain(fixture.certificates[origin], fixture.keys[origin])
    with socket.create_connection(("127.0.0.1", fixture.peer_ports[target]), timeout=3) as raw:
        with tls.wrap_socket(raw, server_hostname=f"node{target}.internal") as connection:
            body = json.dumps(request).encode()
            connection.sendall(struct.pack("!I", len(body)) + body)
            def frame():
                def take(size):
                    body = bytearray()
                    while len(body) < size:
                        part = connection.recv(size - len(body))
                        require(bool(part), "private_control_status_complete")
                        body.extend(part)
                    return bytes(body)
                size = struct.unpack("!I", take(4))[0]
                require(0 < size <= 16384, "private_control_status_bound")
                return json.loads(take(size))
            require(frame() == {"protocol": 1, "accepted": True}
                    and frame() == {"protocol": 1, "stopped": True}, "completed_control_is_status_only")


def qualify_api_desktop_phase(fixture, passed):
    """Public Create/Control authority on one genuine, still-leased Worker run."""
    with fixture.real_phase("public_api_created_desktop_control_and_owner_failure"):
        fixture.observer.verify_creations = True
        try:
            worker_case = fixture.create_worker_desktop()
            before = fixture.run_lease(worker_case["run"])
            fixture.api(2, f"/runs/{worker_case['run']}/desktop-sessions/{worker_case['session']}", method="DELETE", expected=204)
            require(fixture.sql(f"SELECT state='ended' AND runner_metadata->>'cleanup_state'='confirmed' FROM desktop_sessions WHERE id='{worker_case['session']}'::uuid;") == "t"
                    and not fixture.parent.exists(worker_case["binding"]["container_id"])
                    and fixture.observer.count("stop", worker_case["session"]) == 1,
                    "first_worker_desktop_stop_confirmed_before_public_api_create")
            same_live_worker_lease(before, fixture.run_lease(worker_case["run"]))
            case = fixture.create_api_desktop(worker_case, 0)
            view, _ = fixture.live_stream(case, 0)
            require(real_framebuffer(view) > 0, "public_api_created_desktop_real_rfb")
            view.close()
            passed("public_api_create_genuine_receipt_exact_new_docker_id_creator_owner_same_worker_lease")

            fixture.api(2, f"/runs/{case['run']}/desktop-sessions/{case['session']}/tickets", method="POST",
                data={"control": True}, expected=401)
            controlled, ticket, grant = fixture.control_stream(case, 2)
            require(real_framebuffer(controlled) > 0, "public_api_control_real_rfb")
            user = canonical_uuid(fixture.sql("SELECT id FROM users WHERE email='qualification@localhost.invalid' AND deleted_at IS NULL;"))
            require(fixture.sql(f"SELECT state='user_controlled' AND controller_user_id='{user}'::uuid FROM desktop_sessions WHERE id='{case['session']}'::uuid;") == "t"
                    and fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[0],
                    "reauthenticated_control_is_current_user_forwarded_to_creator_owner")
            pointer_before = fixture.pointer_position(case)
            target = {"x": 37, "y": 41} if pointer_before != {"x": 37, "y": 41} else {"x": 73, "y": 79}
            controlled.binary(struct.pack("!BBHH", 5, 0, target["x"], target["y"]))
            fixture.until(lambda: fixture.pointer_position(case) == target,
                "reauthenticated_rfb_control_changes_actual_owned_x11_pointer", seconds=5)
            require(pointer_before != target, "physical_control_pointer_actually_changed")
            controlled.close()
            fixture.until(lambda: fixture.sql(f"SELECT state='agent_controlled' AND controller_user_id IS NULL FROM desktop_sessions WHERE id='{case['session']}'::uuid;") == "t",
                "control_close_restores_agent_control")
            fixture.api(2, f"/runs/{case['run']}/desktop-sessions/{case['session']}/tickets", method="POST",
                data={"control": True, "reauthentication_token": grant["token"]}, expected=401)
            fixture.websocket(1, f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}", expected=401)
            require(fixture.sql(f"SELECT count(*) FROM audit_events WHERE target_type='desktop_session' AND target_id='{case['session']}'::uuid AND actor_user_id='{user}'::uuid AND action IN ('desktop_session.takeover_start','desktop_session.takeover_end');") == "2",
                    "real_control_start_and_end_audited_once")
            same_live_worker_lease(before, fixture.run_lease(case["run"]))
            fixture.no_replay(case)
            passed("public_api_reauth_one_use_control_forwarded_to_exact_gui_pointer_and_restored_on_close")

            old_epoch = fixture.epoch(0)
            live, _ = fixture.live_stream(case, 2)
            real_framebuffer(live)
            probes = fixture.observer.count("probe", case["session"])
            binding_before = dict(case["binding"])
            fixture.kill_group(fixture.nodes[0])
            live.assert_closed("public_api_creator_sigkill_closes_forwarded_rfb")
            fixture.until(lambda: fixture.sql(f"SELECT lease_until<=clock_timestamp() FROM api_cluster_nodes WHERE group_id='{fixture.group}'::uuid AND node_id='{fixture.node_ids[0]}'::uuid;") == "t",
                "public_api_creator_db_lease_expires")
            replacement, _ = fixture.live_stream(case, 1)
            require(real_framebuffer(replacement) > 0 and fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[1]
                    and fixture.observer.count("probe", case["session"]) > probes,
                    "api_created_desktop_stale_owner_cas_requires_fresh_genuine_probe")
            same_live_worker_lease(before, fixture.run_lease(case["run"]), renewed=True)
            persisted = json.loads(fixture.sql(f"SELECT runner_metadata->'runner_job_binding' FROM desktop_sessions WHERE id='{case['session']}'::uuid;"))
            require(persisted == binding_before, "api_creator_failure_preserves_exact_id_created_started_and_lease_binding")
            fixture.require_identity(case)
            fixture.no_replay(case)
            fixture.no_replay(worker_case)
            fixture.start_node(0, generation=4)
            require(fixture.epoch(0) != old_epoch and fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[1],
                    "restarted_public_api_creator_cannot_steal_live_successor_owner")
            replacement.close()
            passed("public_api_created_owner_sigkill_same_physical_job_worker_lease_fresh_probe_cas_no_replay")

            fixture.api(2, f"/runs/{case['run']}/desktop-sessions/{case['session']}", method="DELETE", expected=204)
            control = json.loads(fixture.sql(f"SELECT row_to_json(c) FROM api_cluster_desktop_controls c WHERE desktop_session_id='{case['session']}'::uuid ORDER BY expires_at DESC LIMIT 1;"))
            require(control["claimed_at"] and control["completed_at"] and control["purpose"] == "stop"
                    and control["owner_node_id"] == fixture.node_ids[1] and control["origin_node_id"] == fixture.node_ids[2]
                    and control["binding_sha256"] == binding_digest(case["binding"])
                    and fixture.observer.count("stop", case["session"]) == 1
                    and not fixture.parent.exists(case["binding"]["container_id"]),
                    "public_api_created_cross_owner_one_use_stop_has_exact_physical_absence")
            private_control(fixture, control, 1)
            require(fixture.observer.count("stop", case["session"]) == 1,
                    "public_api_created_completed_stop_ticket_is_status_only_without_command_retry")
            same_live_worker_lease(before, fixture.run_lease(case["run"]))
            fixture.no_replay(case)
            fixture.no_replay(worker_case)
            passed("public_api_created_live_owner_private_stop_exact_absence_completed_ticket_status_only")
        finally:
            fixture.observer.verify_creations = False


def qualify(fixture, report, migration, *, include_api_desktop_phase=False, diagnostic_only=False):
    fixture.diagnostic_only = diagnostic_only
    def passed(name):
        report["checks"].append({"name": name, "passed": True})
    fixture.start_cluster()
    fixture.prepare_certificates()
    fixture.prepare_private_services()
    fixture.start_proxies()
    for node in range(3):
        fixture.start_node(node)
    require(fixture.sql("SELECT max(version) FROM _sqlx_migrations WHERE success;") == str(migration), "expected_physical_ha_schema")
    status, _, body = fixture.http(fixture.api_ports[0], "/api/v1/auth/bootstrap", method="POST", token=fixture.bootstrap_secret,
        data={"email": "qualification@localhost.invalid", "display_name": "Owned physical HA fixture", "password": fixture.admin_password, "device_id": str(uuid.uuid4())})
    require(status == 201, "physical_ha_bootstrap")
    fixture.access_token = json.loads(body)["access_token"]
    team = fixture.api(0, "/teams", expected=201, method="POST", data={"name": "Owned physical HA fixture"})
    fixture.project = fixture.api(0, "/projects", expected=201, method="POST", data={"name": "Owned physical HA fixture", "privacy": "team_managed", "team_id": team["id"]})
    port = fixture.reserve_port()
    fixture.release_port(port)
    environment = Fixture.server_environment(fixture, "worker", port)
    environment.update(fixture.runtime_environment())
    fixture.worker = fixture.spawn([str(fixture.binary)], "owned_ha_worker", environment)
    fixture.wait_fresh_groups(fixture.api_ports[0], {"api", "worker"})
    for node in range(3):
        metrics = fixture.metrics(fixture.api_ports[node])
        require(metrics["application"]["runner_configured"] and not metrics["application"]["object_store_configured"]
                and metrics["runtime"]["api_routing"]["multiple_api_replicas_supported"] is False,
                "real_runner_with_default_off_ha_boundary")
    fixture.parent.assert_preserved()
    passed("three_native_apis_worker_fresh_pg16_real_https_docker_runner_and_original39_preserved")
    report["database_migration_version"] = migration
    report["runner_engine_id"] = fixture.runner.actual_engine_id
    report["runner_instance_id"] = fixture.runner.instance_id

    with fixture.real_phase("physical_owner_kill_and_stop"):
        case = fixture.create_worker_desktop()
        before = fixture.run_lease(case["run"])
        wire, ticket = fixture.live_stream(case, 0)
        require(real_framebuffer(wire) > 0, "worker_desktop_actual_screen_through_api")
        owner = fixture.desktop_owner(case["session"])
        require(owner["node_id"] == fixture.node_ids[0] and owner["executor_id"] is None, "unbound_job_attested_owner_cas")
        passed("worker_created_creation_receipt_absent_owner_cas_and_real_rfb_framebuffer")
        fixture.websocket(1, f"/api/v1/desktop-sessions/{case['session']}/stream?ticket={ticket['token']}", expected=401)
        fixture.no_replay(case)
        passed("public_desktop_ticket_replay_refused_without_physical_create_retry")
        forwarded, _ = fixture.live_stream(case, 2)
        real_framebuffer(forwarded)
        require(fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[0],
                "forwarded_real_rfb_does_not_steal_live_api_owner")
        fixture.peer_proxies[2, 0].partition(True)
        try:
            forwarded.assert_closed("physical_private_partition_closes_rfb")
            fixture.live_stream(case, 2, expected=409)
            require(fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[0],
                    "partition_cannot_steal_a_live_physical_job_owner")
            fixture.no_replay(case)
        finally:
            fixture.peer_proxies[2, 0].partition(False)
        healed, _ = fixture.live_stream(case, 2)
        real_framebuffer(healed)
        healed.close()
        same_live_worker_lease(before, fixture.run_lease(case["run"]))
        require(fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[0],
                "healed_partition_keeps_live_api_owner")
        fixture.no_replay(case)
        passed("real_rfb_cross_api_partition_fails_closed_and_explicit_fresh_request_heals_without_owner_steal")
        if diagnostic_only:
            transport = fixture.observer.transport_snapshot(case["session"])
            require(len(transport) == 3 and all(row["upstream_status"] == 101 and row["upgrade_completed"]
                and not row["counter_saturated"]
                and row["runner_to_client"]["read_bytes"] > 0 and row["runner_to_client"]["sent_bytes"] > 0
                and row["client_to_runner"]["read_bytes"] > 0 and row["client_to_runner"]["sent_bytes"] > 0
                for row in transport), "diagnostic_genuine_upstream_and_bidirectional_bytes_required")
            report["owner_partition_diagnostic"] = {"observer_transport": transport,
                "same_worker_lease": True, "same_live_api_owner": True, "no_create_or_tool_replay": True}
            return  # real_phase finally still stops only this own Run.
        old_epoch = fixture.epoch(0)
        probes = fixture.observer.count("probe", case["session"])
        fixture.kill_group(fixture.nodes[0])
        wire.assert_closed("killed_physical_api_owner_closes_rfb")
        fixture.until(lambda: fixture.sql(f"SELECT lease_until<=clock_timestamp() FROM api_cluster_nodes WHERE group_id='{fixture.group}'::uuid AND node_id='{fixture.node_ids[0]}'::uuid;") == "t", "killed_owner_db_lease_expires")
        replacement, _ = fixture.live_stream(case, 1)
        require(real_framebuffer(replacement) > 0, "adopted_same_physical_desktop_real_screen")
        after = fixture.run_lease(case["run"])
        owner = fixture.desktop_owner(case["session"])
        require(owner["node_id"] == fixture.node_ids[1] and before["token"] == after["token"] and before["owner"] == after["owner"]
                and after["expiry"] > before["expiry"] and fixture.observer.count("probe", case["session"]) > probes,
                "dead_api_owner_replaced_only_after_fresh_probe_with_same_renewed_worker_lease")
        fixture.require_identity(case)
        fixture.no_replay(case)
        fixture.start_node(0, generation=1)
        require(fixture.epoch(0) != old_epoch and fixture.desktop_owner(case["session"])["node_id"] == fixture.node_ids[1],
                "restarted_api_never_steals_live_successor_owner")
        passed("owner_sigkill_db_expiry_same_lease_probe_cas_reconnect_without_tool_replay")
        replacement.close()
        fixture.api(2, f"/runs/{case['run']}/desktop-sessions/{case['session']}", method="DELETE", expected=204)
        control = json.loads(fixture.sql(f"SELECT row_to_json(c) FROM api_cluster_desktop_controls c WHERE desktop_session_id='{case['session']}'::uuid ORDER BY expires_at DESC LIMIT 1;"))
        require(control["claimed_at"] and control["completed_at"] and control["purpose"] == "stop"
                and control["owner_node_id"] == fixture.node_ids[1] and control["origin_node_id"] == fixture.node_ids[2]
                and control["binding_sha256"] == binding_digest(case["binding"]) and fixture.observer.count("stop", case["session"]) == 1,
                "live_owner_private_control_ticket_completed_once")
        require(not fixture.parent.exists(case["binding"]["container_id"]),
                "physical_stop_exact_docker_id_absent")
        passed("cross_api_live_owner_stop_private_mtls_one_use_ticket_and_exact_physical_absence")
        private_control(fixture, control, 1)
        private_control(fixture, control, 1, role="user", refused=True)
        private_control(fixture, control, 1, certificate_node=3, refused=True)
        require(fixture.observer.count("stop", case["session"]) == 1, "completed_purpose_ticket_or_spoofed_peer_never_redispatches_stop")
        passed("completed_control_status_replay_purpose_swap_and_unallowlisted_tls_peer_fenced")

    with fixture.real_phase("physical_auth_revocation"):
        case = fixture.create_worker_desktop()
        login = fixture.new_admin_session(fixture.api_ports[0])
        token = login["access_token"]
        wire, _ = fixture.live_stream(case, 1, token=token)
        real_framebuffer(wire)
        from qualify_api_cluster import open_run_sse
        events = open_run_sse(fixture, 2, case["run"], token)
        fixture.api(0, "/auth/logout", method="POST", token=token, expected=204)
        wire.assert_closed("revoked_real_desktop_auth_session_closes_rfb")
        fixture.assert_sse_closed(events, "revoked_real_desktop_auth_session_closes_sse")
        fixture.require_identity(case)
        fixture.no_replay(case)
        passed("current_auth_revocation_closes_real_rfb_and_run_sse_without_new_physical_commands")

    with fixture.real_phase("physical_graceful_drain"):
        case = fixture.create_worker_desktop()
        wire, _ = fixture.live_stream(case, 2)
        real_framebuffer(wire)
        held = fixture.open_sync_sse(fixture.api_ports[2])
        fixture.stop_server(fixture.nodes[2], "real_rfb_owner_graceful_drain")
        wire.assert_closed("drained_real_owner_closes_rfb")
        fixture.assert_sse_closed(held, "drained_real_owner_closes_sync_sse")
        fixture.start_node(2, generation=1)
        replacement, _ = fixture.live_stream(case, 0)
        real_framebuffer(replacement)
        fixture.require_identity(case)
        fixture.no_replay(case)
        passed("normal_sigterm_closes_held_real_rfb_and_sse_exit_zero_then_same_job_probe_reconnect")

    with fixture.real_phase("physical_slow_reader"):
        case = fixture.create_worker_desktop()
        initial, _ = fixture.live_stream(case, 0)
        real_framebuffer(initial)
        initial.close()
        slow, _ = fixture.live_stream(case, 1, receive_buffer=4096)
        real_framebuffer(slow)
        baseline = {node: native_rss(child) for node, child in fixture.nodes.items()}
        width, height = slow.real_rfb_dimensions
        require(width * height * 4 * 8 >= 8 * 1024 * 1024, "real_slow_reader_requested_screen_volume")
        for _ in range(8):
            slow.binary(struct.pack("!BBHHHH", 3, 0, 0, 0, width, height))
        time.sleep(3)
        require(all(native_rss(child) - baseline[node] <= 128 * 1024 * 1024
                    for node, child in fixture.nodes.items()), "real_rfb_slow_reader_native_memory_bound")
        start = time.monotonic()
        fixture.ready_all()
        require(time.monotonic() - start < 5, "real_rfb_slow_reader_preserves_other_http_responsiveness")
        fixture.no_replay(case)
        slow.close()
        passed("real_rfb_eight_screen_requests_slow_reader_bounded_memory_and_responsive_apis")

    for mode in ("stopped_job", "engine_binding", "old_lease", "invalid_proof", "refused_probe", "delayed_probe", "foreign_runner_ca"):
        with fixture.real_phase(mode):
            case = fixture.create_worker_desktop()
            session, run = case["session"], case["run"]
            if mode == "stopped_job":
                fixture.protocol.stop(case["binding"])
            elif mode == "engine_binding":
                fixture.sql(f"UPDATE desktop_sessions SET runner_metadata=jsonb_set(runner_metadata,'{{runner_job_binding,engine_id}}','\"owned-different-engine\"') WHERE id='{session}'::uuid;")
            elif mode == "old_lease":
                fixture.invalidate_owned_lease_expectation(case)
            elif mode == "foreign_runner_ca":
                fixture.stop_server(fixture.nodes[1], "own_node1_before_foreign_ca")
                fixture.trust_overrides[1] = fixture.runner.ca_file
                fixture.start_node(1, generation=2)
            else:
                fixture.observer.fault(session, mode)
            before_rfb = fixture.observer.count("rfb", session)
            if mode == "old_lease":
                fixture.refuse_old_lease_stream(case, 1)
            else:
                fixture.live_stream(case, 1, expected=409)
            require(fixture.desktop_owner(session) is None and fixture.observer.count("rfb", session) == before_rfb,
                    "unproven_stale_or_stopped_job_never_mints_owner_or_runner_rfb_command")
            fixture.no_replay(case)
            if mode == "foreign_runner_ca":
                fixture.stop_server(fixture.nodes[1], "own_node1_foreign_ca")
                fixture.trust_overrides.pop(1)
                fixture.start_node(1, generation=3)
            passed("physical_desktop_failclosed_" + mode)

    with fixture.real_phase("lost_physical_stop_response"):
        case = fixture.create_worker_desktop()
        wire, _ = fixture.live_stream(case, 0)
        real_framebuffer(wire)
        wire.close()
        fixture.observer.fault(case["session"], "lose_stop_response")
        fixture.api(2, f"/runs/{case['run']}/desktop-sessions/{case['session']}", method="DELETE", expected=409)
        row = json.loads(fixture.sql(f"SELECT jsonb_build_object('claimed',EXISTS(SELECT 1 FROM api_cluster_desktop_controls c WHERE c.desktop_session_id=d.id AND c.claimed_at IS NOT NULL AND c.completed_at IS NULL),'pending',d.runner_metadata->>'cleanup_state'='stop_pending','state',d.state) FROM desktop_sessions d WHERE d.id='{case['session']}'::uuid;"))
        require(row["claimed"] and row["pending"] and row["state"] != "ended"
                and case["session"] in fixture.observer.lost_proven_stops
                and not fixture.parent.exists(case["binding"]["container_id"]),
                "unknown_stop_retains_ticket_and_session_quarantine_after_verified_real_upstream_absence")
        stops = fixture.observer.count("stop", case["session"])
        fixture.api(1, f"/runs/{case['run']}/desktop-sessions/{case['session']}", method="DELETE", expected=409)
        fixture.api(1, f"/runs/{case['run']}/desktop-sessions/{case['session']}/tickets", method="POST", data={"control": False}, expected=409)
        time.sleep(2)
        require(stops == 1 and fixture.observer.count("stop", case["session"]) == stops, "unknown_physical_stop_has_no_automatic_or_new_ticket_retry")
        fixture.no_replay(case)
        passed("real_runner_stop_response_loss_holds_durable_unknown_outcome_and_refuses_replay")

    if include_api_desktop_phase:
        qualify_api_desktop_phase(fixture, passed)

    require(not fixture.model.errors, "private_model_healthy")
    require(not fixture.cleanup_failures, "all_phase_cleanups_succeeded")
    fixture.stop_server(fixture.worker, "real_ha_worker")
    for node, child in fixture.nodes.items():
        fixture.stop_server(child, f"real_ha_api{node}")
    fixture.parent.assert_preserved()
    passed("all_owned_native_processes_sigterm_exit_zero_and_original39_preserved")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--diagnostic-only", action="store_true",
        help="Only initial/forwarded RFB and private owner partition/healing; never qualifies the full HA matrix")
    parser.add_argument("--include-api-desktop-phase", action="store_true",
        help="Add four public API Create/reauthenticated Control/owner failure/Stop checks to the existing 19")
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-runner-sha256", required=True)
    parser.add_argument("--expected-migration", type=int, required=True)
    runner_source = parser.add_mutually_exclusive_group(required=True)
    runner_source.add_argument("--runner-image-id", help="Exact already-built local Docker .Id; no pull/build")
    runner_source.add_argument("--runner-binary", type=Path, help="Exact native Linux ELF; uses owned Unix-Docker relay")
    parser.add_argument("--linux-docker-cli", type=Path, help="Native ELF Docker CLI for the owned relay (native Runner only)")
    parser.add_argument("--expected-linux-docker-cli-sha256", help="Independent approved SHA256 for --linux-docker-cli")
    parser.add_argument("--gui-image-id", required=True)
    parser.add_argument("--core-image-id", required=True)
    parser.add_argument("--docker-client", type=Path, required=True)
    parser.add_argument("--docker-context", required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--baseline-count", type=int, default=39)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    report = {"schema_version": 1, "passed": False, "checks": [],
        "qualification_kind": "owner_partition_diagnostic" if args.diagnostic_only else "full_linux_desktop_ha",
        "diagnostic_only": args.diagnostic_only, "full_matrix_qualified": False, "started_at": datetime.now(timezone.utc).isoformat(),
        "scope": {"native_api_processes": 3, "separate_native_worker": True, "fresh_postgresql16": True,
            "native_private_mtls": True, "real_docker_runner_https": True, "real_gui_rfb": True,
            "public_api_desktop_create_and_control": args.include_api_desktop_phase,
            "owner_partition_only": args.diagnostic_only,
            "fake_model_fixture_only": True, "fake_executor": False, "original_containers": 39,
            "production_ha_release": False, "registry_publication_verified": False, "live_fleet": False},
        "limitations": ["Public API WSS uses owned TLS front proxies over native loopback HTTP.",
            "A private transparent Runner TLS hop records/faults genuine upstream responses; it never signs a successful proof.",
            "The engine mismatch and old lease cases alter only isolated own database expectations; no second physical Docker engine is claimed.",
            "The model fixture delivers one real Worker GUI tool then holds a response; this is not live model/provider acceptance.",
            "A passing isolated matrix does not activate multiple_api_replicas_supported.",
            "The real slow-reader check observes eight screen requests for three seconds with a 128 MiB per-API RSS growth bound.",
            "Failures retain the private UUID fixture root; SIGKILL/host termination cannot execute cleanup handlers."]}
    if args.diagnostic_only:
        report["limitations"] = [
            "Only the initial/forwarded RFB and private owner partition/healing phase are selected; this is never a full HA qualification.",
            "Owner failure/adoption, auth revocation, stop controls, lease/engine faults and slow-reader cases are excluded.",
            "A direct diagnostic uses only a fresh signed Probe and read-only exact-bound RFB after current own lease/identity checks, with one 12-second deadline.",
            "Raw transport counters count bytes read or accepted by send; they never claim peer application acknowledgement.",
            "Failures retain the private UUID fixture root; cleanup proof does not turn a failed diagnostic into a passing matrix."]
    started = time.monotonic()
    helpers = parent = storage = runner_fixture = fixture = None
    workload_passed = False
    def write_report():
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    def interrupted(signum, frame):
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        raise QualificationFailure("owned_linux_desktop_ha_interrupted")
    saved = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
    for number in HANDLED_SIGNALS:
        signal.signal(number, interrupted)
    try:
        inputs = qualification_inputs(args)
        require(args.execute, "explicit_real_fixture_execution_required")
        require(os.name == "posix" and os.geteuid() != 0, "linux_nonroot_execution")
        require((PG_BIN / "initdb").is_file() and shutil.which("openssl"), "pg16_and_openssl_available")
        source = inputs["server_bin"].resolve(strict=True)
        require(source.is_file() and os.access(source, os.X_OK) and file_sha256(source) == inputs["server_sha"], "exact_approved_native_server_binary")
        validate_native_inputs(inputs)
        helpers, helper_path = load_runner_helpers()
        report.update({"candidate_sha256": inputs["server_sha"], "runner_binary_sha256": inputs["runner_sha"],
            "runner_image_id": inputs["runner_image"], "gui_image_id": inputs["gui_image"], "core_image_id": inputs["core_image"],
            "runner_process_kind": inputs["runner_mode"], "linux_docker_cli_sha256": inputs["linux_docker_cli_sha"],
            "harness_sha256": file_sha256(Path(__file__)), "runner_fixture_sha256": file_sha256(helper_path),
            "cluster_fixture_sha256": file_sha256(Path(__file__).with_name("qualify_api_cluster.py")),
            "native_fixture_sha256": file_sha256(Path(__file__).with_name("qualify_runtime.py")), "baseline_sha256": file_sha256(inputs["baseline"])})
        # This root is intentionally independent of TemporaryDirectory: any
        # unconfirmed result, including a catchable signal, retains evidence.
        storage = QualificationRoot(helpers)
        root = storage.path
        report.update({"fixture_root": str(root), "fixture_identity": storage.identity,
                       "private_root_disposition": "retained_qualification_in_progress"})
        write_report()  # The parent can locate a partial start before mutations.
        parent = helpers.ParentDocker(client=inputs["docker_client"], context=inputs["docker_context"],
            baseline_file=inputs["baseline"], required_baseline_count=inputs["baseline_count"])
        binary = root / "approved-server"
        shutil.copyfile(source, binary)
        binary.chmod(0o500)
        require(file_sha256(binary) == inputs["server_sha"], "owned_copy_matches_approved_server")
        runner_root = root / "r"
        runner_root.mkdir(mode=0o700)
        runner_binary, docker_cli = None, None
        if inputs["runner_mode"] == "native":
            runner_binary = owned_native_copy(inputs["runner_binary"], root / "approved-runner",
                inputs["runner_sha"], "owned_runner_copy_matches_approved_sha")
            docker_cli = owned_native_copy(inputs["linux_docker_cli"], root / "approved-docker-cli",
                inputs["linux_docker_cli_sha"], "owned_docker_cli_copy_matches_approved_sha")
        runner_fixture = helpers.RunnerFixture(parent, runner_root, runner_image_id=inputs["runner_image"], gui_image_id=inputs["gui_image"],
            core_image_id=inputs["core_image"], expected_binary_sha256=inputs["runner_sha"],
            native_binary=runner_binary, linux_docker_cli=docker_cli, scope=storage.scope, memory_mib=8192)
        report["runner_fixture_identity"] = runner_fixture.identity_record
        write_report()
        runner = runner_fixture.start()
        fixture = LinuxFixture(root, binary, inputs["server_sha"], parent, runner, runner.protocol)
        qualify(fixture, report, inputs["migration"], include_api_desktop_phase=inputs["include_api_desktop_phase"],
                diagnostic_only=inputs["diagnostic_only"])
        workload_passed = True
    except QualificationFailure as error:
        report["failure_check"] = str(error)
        report["failure_source_positions"] = failure_source_positions(error)
        report["command_failure_detail"] = public_failure(error, helpers)
    except Exception as error:
        report["failure_kind"] = type(error).__name__
        report["failure_source_positions"] = failure_source_positions(error)
        report["command_failure_detail"] = public_failure(error, helpers)
    finally:
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        report["last_action"] = getattr(helpers, "LAST_ACTION", None)
        if fixture is not None:
            report["diagnostics"] = {"last_phase": fixture.last_phase, "last_api_status": fixture.last_api_status,
                "failed_api_status": fixture.failed_api_status, "last_ws_status": fixture.last_ws_status,
                "model_tool_delivery_counts": [scenario.tool_deliveries for scenario in fixture.scenarios],
                "model_request_counts": [scenario.requests for scenario in fixture.scenarios],
                "phase_cleanup_failures": fixture.cleanup_failures,
                "rfb_protocol": rfb_report(fixture.wire_sockets),
                "runner_observer_transport": fixture.observer.transport_snapshot() if fixture.observer else [],
                "node_process_states": {str(node): "running" if child.poll() is None else "exited" for node, child in fixture.nodes.items()}}
        if storage is not None:
            try:
                proofs, failures, inventory = cleanup_owned_fixtures(fixture, runner_fixture, parent, helpers, storage)
            except Exception as error:
                proofs, inventory = {key: False for key in ROOT_CLEANUP_KEYS}, None
                failures = [{"cleanup_action": "owned_cleanup", **public_failure(error, helpers)}]
            report.update({"cleanup_proofs": proofs, "cleanup_failures": failures,
                           "retained_owned_inventory": inventory})
            complete = workload_passed and not failures and all(proofs.values())
            try:
                report["private_root_disposition"] = storage.finish(complete, proofs)
                report["passed"] = complete
                report["full_matrix_qualified"] = complete and not args.diagnostic_only
            except Exception as error:
                report["private_root_disposition"] = "retained_root_removal_unconfirmed"
                report["cleanup_failures"].append({"cleanup_action": "private_root", **public_failure(error, helpers)})
            if not report["passed"] and storage.path.is_dir():
                # Public diagnostics omit secrets; this copy is still owner-only.
                try:
                    helpers.write_private(storage.path / "report.json", json.dumps(report, indent=2) + "\n")
                except Exception as error:
                    report["cleanup_failures"].append({"cleanup_action": "retained_private_report", **public_failure(error, helpers)})
        for number, handler in saved.items():
            signal.signal(number, handler)
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    write_report()
    print(json.dumps(report, separators=(",", ":")))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
