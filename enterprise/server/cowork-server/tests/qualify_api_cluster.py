#!/usr/bin/env python3
"""Isolated native three-API routing qualification; Python stdlib, Linux/WSL.

Every process, PG16 database, TLS certificate, proxy and protocol client belongs
to this invocation. A fake executor never starts a tool or desktop. The native
API-to-API mTLS protocol is exercised unchanged; public WSS uses a fixture TLS
front proxy over owned loopback HTTP listeners. This is not production HA,
Docker/Runner acceptance, real desktop acceptance or a live-fleet test.
"""
import argparse
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import select
import secrets
import shutil
import signal
import socket
import ssl
import struct
import tempfile
import threading
import time
import uuid

from qualify_runtime import (API_LOCK_KEY, Fixture, HANDLED_SIGNALS, PG_BIN,
                             QualificationFailure, require)


MAX_WS_FRAME = 8 * 1024 * 1024
MAX_HTTP_HEADER = 16 * 1024
PROXY_BUFFER = 64 * 1024


def canonical_uuid(value):
    require(isinstance(value, str) and bool(re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", value))
            and uuid.UUID(value).int != 0, "owned_canonical_uuid")
    return value


def qualification_inputs(args):
    """Pure gates, before any subprocess, port allocation or database action."""
    require(bool(re.fullmatch(r"[a-f0-9]{64}", args.expected_sha256)), "explicit_candidate_sha256_required")
    require(args.expected_migration >= 56, "cluster_schema56_or_later_required")
    return {"binary": Path(args.server_bin), "sha256": args.expected_sha256,
            "migration": args.expected_migration, "report": Path(args.report)}


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for part in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(part)
    return digest.hexdigest()


def client_frame(opcode, body, mask):
    require(opcode in {1, 2, 8, 9, 10} and len(mask) == 4, "fixture_ws_frame_shape")
    require(len(body) <= MAX_WS_FRAME, "fixture_ws_frame_bound")
    require(opcode < 8 or len(body) <= 125, "fixture_ws_control_frame_shape")
    length = len(body)
    size = bytes([length | 0x80]) if length < 126 else (
        bytes([126 | 0x80]) + struct.pack("!H", length) if length < 65536 else
        bytes([127 | 0x80]) + struct.pack("!Q", length))
    return bytes([0x80 | opcode]) + size + mask + bytes(value ^ mask[index % 4]
                                                       for index, value in enumerate(body))


def take_server_frame(buffer):
    """Incremental parser: incomplete headers/bodies remain entirely buffered."""
    if len(buffer) < 2:
        return None
    first, second = buffer[:2]
    require(not second & 0x80 and not first & 0x70, "fixture_ws_server_flags")
    length, offset = second & 0x7f, 2
    if length == 126:
        if len(buffer) < 4:
            return None
        length, offset = struct.unpack("!H", buffer[2:4])[0], 4
    elif length == 127:
        if len(buffer) < 10:
            return None
        length, offset = struct.unpack("!Q", buffer[2:10])[0], 10
    require(length <= MAX_WS_FRAME, "fixture_ws_server_frame_bound")
    opcode, final = first & 15, bool(first & 0x80)
    require(opcode in {0, 1, 2, 8, 9, 10}, "fixture_ws_server_opcode")
    require(opcode < 8 or (final and length <= 125), "fixture_ws_control_frame_shape")
    if len(buffer) < offset + length:
        return None
    body = bytes(buffer[offset:offset + length])
    del buffer[:offset + length]
    return opcode, final, body


def websocket_accept(key):
    return base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")).digest()).decode("ascii")


class WireSocket:
    def __init__(self, connection, remainder=b""):
        self.connection = connection
        self.buffer = bytearray(remainder)
        self.write_lock = threading.Lock()
        self.closed = False
        self.fragment_opcode = None
        self.fragments = bytearray()

    def send(self, opcode, body):
        with self.write_lock:
            require(not self.closed, "fixture_ws_closed")
            self.connection.sendall(client_frame(opcode, body, secrets.token_bytes(4)))

    def json(self, body):
        self.send(1, json.dumps(body, separators=(",", ":")).encode())

    def binary(self, body):
        self.send(2, body)

    def recv(self, timeout=3):
        deadline = time.monotonic() + timeout
        while True:
            frame = take_server_frame(self.buffer)
            if frame is not None:
                opcode, final, body = frame
                if opcode == 9:
                    self.send(10, body)
                    continue
                if opcode == 10:
                    continue
                if opcode == 8:
                    return None
                if opcode == 0:
                    require(self.fragment_opcode is not None, "fixture_ws_continuation_order")
                    self.fragments.extend(body)
                    require(len(self.fragments) <= MAX_WS_FRAME, "fixture_ws_message_bound")
                    if final:
                        opcode, body = self.fragment_opcode, bytes(self.fragments)
                        self.fragment_opcode, self.fragments = None, bytearray()
                    else:
                        continue
                elif not final:
                    require(self.fragment_opcode is None, "fixture_ws_fragment_order")
                    self.fragment_opcode, self.fragments = opcode, bytearray(body)
                    continue
                else:
                    require(self.fragment_opcode is None, "fixture_ws_fragment_order")
                return opcode, body
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError
            self.connection.settimeout(remaining)
            part = self.connection.recv(65536)
            if not part:
                return None
            self.buffer.extend(part)

    def assert_closed(self, check, seconds=6):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                if self.recv(timeout=min(.5, max(.01, deadline - time.monotonic()))) is None:
                    return
            except TimeoutError:
                continue
            except (ConnectionError, OSError, ssl.SSLError):
                return
        raise QualificationFailure(check)

    def close(self):
        self.closed = True
        try:
            self.connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.connection.close()


class OwnedProxy:
    """Transparent private TCP, or public fixture TLS -> owned loopback HTTP.

    One bounded-buffer pump owns both socket directions. A partition closes
    existing tunnels and rejects new ones. No peer TLS is terminated here.
    """
    def __init__(self, port, target, tls=None):
        require(isinstance(port, int) and 0 < port < 65536
                and isinstance(target, int) and 0 < target < 65536 and port != target,
                "owned_proxy_port_scope")
        self.target = target
        self.tls = tls
        self.stop = threading.Event()
        self.blocked = threading.Event()
        self.lock = threading.Lock()
        self.connections = set()
        self.handlers = []
        self.accepted = 0
        self.listener = socket.socket()
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", port))
        self.listener.listen(32)
        self.listener.settimeout(.2)
        self.thread = threading.Thread(target=self._accept, name="owned-cluster-proxy")
        self.thread.start()

    @staticmethod
    def _close(connection):
        try:
            connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        connection.close()

    def _accept(self):
        while not self.stop.is_set():
            try:
                incoming, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with self.lock:
                self.connections.add(incoming)
                self.accepted += 1
                active = len(self.connections)
            if self.blocked.is_set() or active > 64:
                self._close(incoming)
                with self.lock:
                    self.connections.discard(incoming)
                continue
            handler = threading.Thread(target=self._pump, args=(incoming,), name="owned-proxy-tunnel")
            with self.lock:
                self.handlers.append(handler)
            handler.start()

    def _pump(self, original):
        client, upstream = original, None
        try:
            client.settimeout(2)
            if self.tls is not None:
                client = self.tls.wrap_socket(client, server_side=True)
                with self.lock:
                    self.connections.discard(original)
                    self.connections.add(client)
            upstream = socket.create_connection(("127.0.0.1", self.target), timeout=2)
            with self.lock:
                self.connections.add(upstream)
            client.setblocking(False)
            upstream.setblocking(False)
            outgoing = {client: bytearray(), upstream: bytearray()}
            opposite = {client: upstream, upstream: client}
            while not self.stop.is_set() and not self.blocked.is_set():
                readable = [side for side in opposite if len(outgoing[opposite[side]]) < PROXY_BUFFER]
                writable = [side for side in opposite if outgoing[side]]
                ready, write_ready, _ = select.select(readable, writable, [], .1)
                if isinstance(client, ssl.SSLSocket) and client in readable and client.pending():
                    ready = list(set(ready + [client]))
                for side in write_ready:
                    try:
                        count = side.send(outgoing[side])
                    except (BlockingIOError, ssl.SSLWantReadError, ssl.SSLWantWriteError):
                        continue
                    if count == 0:
                        return
                    del outgoing[side][:count]
                for side in ready:
                    try:
                        data = side.recv(min(16384, PROXY_BUFFER - len(outgoing[opposite[side]])))
                    except (BlockingIOError, ssl.SSLWantReadError, ssl.SSLWantWriteError):
                        continue
                    if not data:
                        return
                    outgoing[opposite[side]].extend(data)
        except (OSError, ssl.SSLError, ValueError):
            pass
        finally:
            for side in {original, client, upstream} - {None}:
                self._close(side)
            with self.lock:
                for side in (original, client, upstream):
                    self.connections.discard(side)

    def partition(self, blocked):
        if blocked:
            self.blocked.set()
            with self.lock:
                connections = list(self.connections)
            for connection in connections:
                self._close(connection)
        else:
            self.blocked.clear()

    def close(self):
        self.stop.set()
        self.partition(True)
        self._close(self.listener)
        self.thread.join(timeout=3)
        deadline = time.monotonic() + 5
        for handler in self.handlers:
            handler.join(timeout=max(0, deadline - time.monotonic()))
        require(not self.thread.is_alive() and not any(h.is_alive() for h in self.handlers),
                "owned_proxy_cleanup")


class FakeExecutor:
    """Authenticated native protocol client; never executes leases or commands."""
    def __init__(self, fixture, node, executor_id, credential):
        self.fixture, self.node = fixture, node
        self.executor_id, self.credential = executor_id, credential
        self.socket = fixture.websocket(node, f"/api/v1/agent/executors/{executor_id}/connect",
                                        credential["token"])
        first = self.socket.recv()
        require(first is not None and first[0] == 1 and json.loads(first[1])["type"] == "hello",
                "fake_executor_native_hello")
        self.stop = threading.Event()
        self.expected_close = False
        self.closed = threading.Event()
        self.lock = threading.Lock()
        self.leases = {}
        self.commands = []
        self.errors = []
        self.outgoing = queue.SimpleQueue()
        self.thread = threading.Thread(target=self._run, name="owned-fake-executor")
        self.thread.start()
        fixture.fake_executors.append(self)

    def _run(self):
        heartbeat_at = time.monotonic()
        try:
            while not self.stop.is_set():
                # One thread owns SSL_read and SSL_write on this connection.
                # Main-thread events are queued, never concurrent SSL calls.
                while True:
                    try:
                        self.socket.json(self.outgoing.get_nowait())
                    except queue.Empty:
                        break
                if time.monotonic() >= heartbeat_at:
                    with self.lock:
                        leases = list(self.leases.items())
                    for run, token in leases:
                        self.socket.json({"type": "lease_heartbeat", "run_id": run, "lease_token": token})
                    heartbeat_at = time.monotonic() + 3
                try:
                    message = self.socket.recv(timeout=.2)
                except TimeoutError:
                    continue
                if message is None:
                    break
                require(message[0] == 1, "fake_executor_control_text_only")
                dto = json.loads(message[1])
                with self.lock:
                    if dto["type"] == "lease":
                        lease = dto["lease"]
                        run = canonical_uuid(lease["run"]["spec"]["id"])
                        self.leases[run] = canonical_uuid(lease["lease_token"])
                    elif dto["type"] == "desktop_stream_requested":
                        for key in ("run_id", "session_id", "stream_id"):
                            canonical_uuid(dto[key])
                        self.commands.append(dto)
                    elif dto["type"] == "error":
                        self.errors.append("native_executor_protocol_error")
                    else:
                        require(dto["type"] == "ack", "fake_executor_known_message")
        except Exception:
            if not self.stop.is_set() and not self.expected_close:
                with self.lock:
                    self.errors.append("fake_executor_transport_error")
        finally:
            self.closed.set()

    def lease(self, run):
        with self.lock:
            return self.leases.get(run)

    def count(self):
        with self.lock:
            return len(self.commands)

    def next_command(self, prior):
        def next_one():
            with self.lock:
                require(not self.errors, self.errors[0] if self.errors else "fake_executor_health")
                return dict(self.commands[prior]) if len(self.commands) > prior else None
        return self.fixture.until(next_one, "native_desktop_command_timeout", seconds=10)

    def event(self, run, marker):
        token = self.fixture.until(lambda: self.lease(run), "fake_executor_current_lease")
        self.outgoing.put({"type": "event", "run_id": run, "request": {"lease_token": token,
            "source_event_id": str(uuid.uuid4()), "kind": "warning", "payload": {"fixture_marker": marker}}})

    def complete(self, run):
        token = self.fixture.until(lambda: self.lease(run), "fake_executor_current_lease")
        with self.lock:
            self.leases.pop(run, None)
        self.outgoing.put({"type": "complete", "run_id": run, "request": {
            "lease_token": token, "result": {"private_protocol_fixture": True}}})

    def close(self):
        self.stop.set()
        self.socket.close()
        self.thread.join(timeout=4)
        require(not self.thread.is_alive(), "fake_executor_cleanup")


class ClusterFixture(Fixture):
    def __init__(self, root, binary, expected_sha):
        super().__init__(root, binary)
        self.expected_sha = expected_sha
        self.group = str(uuid.uuid4())
        self.node_ids = [str(uuid.uuid4()) for _ in range(3)]
        self.api_ports = [self.api_port, self.reserve_port(), self.reserve_port()]
        self.peer_ports = [self.reserve_port() for _ in range(3)]
        self.public_tls_ports = [self.reserve_port() for _ in range(3)]
        self.peer_proxy_ports = {(source, target): self.reserve_port()
                                 for source in range(3) for target in range(3) if source != target}
        self.proxies, self.public_proxies, self.peer_proxies = [], [], {}
        self.nodes = {}
        self.fake_executors = []
        self.wire_sockets = []
        self.owned_runs = []
        self.phase_counts = {}
        self.identity = "cluster_fixture_" + uuid.uuid4().hex
        self.last_phase = None
        self.last_api_status = None
        self.failed_api_status = None
        self.last_ws_status = None

    def prepare_certificates(self):
        ca, ca_key = self.root / "ca.pem", self.root / "ca.key"
        self.command(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256",
                      "-days", "2", "-subj", "/CN=Owned cluster fixture CA", "-keyout", str(ca_key),
                      "-out", str(ca)], "owned_ca")
        self.certificates, self.keys, self.fingerprints = [], [], []
        for node in range(4):  # Fourth CA-valid leaf is deliberately not allowlisted.
            name = f"node{node}.internal"
            cert, key, csr, ext, der = (self.root / f"node{node}.{suffix}" for suffix in
                                       ["pem", "key", "csr", "ext", "der"])
            ext.write_text(f"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
                           f"extendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:{name}\n")
            self.command(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-sha256",
                          "-subj", f"/CN={name}", "-keyout", str(key), "-out", str(csr)], f"owned_node{node}_csr")
            self.command(["openssl", "x509", "-req", "-in", str(csr), "-CA", str(ca), "-CAkey", str(ca_key),
                          "-CAcreateserial", "-days", "2", "-sha256", "-extfile", str(ext), "-out", str(cert)],
                         f"owned_node{node}_certificate")
            self.command(["openssl", "x509", "-in", str(cert), "-outform", "DER", "-out", str(der)],
                         f"owned_node{node}_der")
            key.chmod(0o600)
            self.certificates.append(cert)
            self.keys.append(key)
            self.fingerprints.append(file_sha256(der))
        ca_key.chmod(0o600)
        self.ca = ca
        self.client_tls = ssl.create_default_context(cafile=str(ca))

    def start_proxies(self):
        for (source, target), port in self.peer_proxy_ports.items():
            self.release_port(port)
            proxy = OwnedProxy(port, self.peer_ports[target])
            self.proxies.append(proxy)
            self.peer_proxies[source, target] = proxy
        for node, port in enumerate(self.public_tls_ports):
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.load_cert_chain(self.certificates[node], self.keys[node])
            self.release_port(port)
            proxy = OwnedProxy(port, self.api_ports[node], tls=tls)
            self.public_proxies.append(proxy)
            self.proxies.append(proxy)

    def node_config(self, node):
        peers = []
        for target in range(3):
            port = self.peer_ports[target] if node == target else self.peer_proxy_ports[node, target]
            peers.append({"node_id": self.node_ids[target], "address": f"127.0.0.1:{port}",
                          "server_name": f"node{target}.internal", "certificate_sha256": self.fingerprints[target]})
        return {"group_id": self.group, "node_id": self.node_ids[node],
                "listen_addr": f"127.0.0.1:{self.peer_ports[node]}",
                "certificate_file": str(self.certificates[node]), "private_key_file": str(self.keys[node]),
                "ca_certificate_file": str(self.ca), "peers": peers}

    def node_environment(self, node):
        config = self.root / f"cluster-node{node}.json"
        config.write_text(json.dumps(self.node_config(node)))
        config.chmod(0o600)
        environment = super().server_environment("api", self.api_ports[node])
        environment.update({"COWORK_API_CLUSTER_CONFIG_FILE": str(config),
            "DATABASE_URL": self.database_url + "?application_name=" + self.identity + f"_node{node}"})
        return environment

    def start_node(self, node, generation=0):
        require(file_sha256(self.binary) == self.expected_sha, "copied_candidate_still_matches_approved_sha")
        self.release_port(self.api_ports[node])
        self.release_port(self.peer_ports[node])
        child = self.spawn([str(self.binary)], f"api{node}_generation{generation}", self.node_environment(node))
        self.nodes[node] = child
        self.wait_ready(child, self.api_ports[node])
        return child

    def until(self, condition, name, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            answer = condition()
            if answer:
                return answer
            time.sleep(.05)
        raise QualificationFailure(name)

    def api(self, node, path, *, expected=200, method="GET", data=None, token=None, record_failure=True):
        status, _, body = self.http(self.api_ports[node], "/api/v1" + path, method=method,
                                    data=data, token=token or self.access_token)
        self.last_api_status = {"node": node, "expected": expected, "actual": status}
        if record_failure and status != expected and self.failed_api_status is None:
            # A phase's cleanup uses the API too. Preserve the original failure
            # without logging request paths, bodies, credentials or error text.
            code = "unrecognized_error"
            try:
                candidate = json.loads(body).get("error")
                if isinstance(candidate, str) and candidate in {"bad_request", "unauthorized", "mfa_required", "not_found",
                                 "conflict", "unprocessable", "quota_exceeded", "unavailable",
                                 "database_error", "invalid_json", "internal_error"}:
                    code = candidate
            except (ValueError, AttributeError):
                pass
            self.failed_api_status = dict(self.last_api_status, phase=self.last_phase, error_code=code)
        require(status == expected, "owned_cluster_authenticated_api_response")
        return json.loads(body) if body else None

    def websocket(self, node, path, token=None, *, expected=101, receive_buffer=None):
        connection = socket.create_connection(("127.0.0.1", self.public_tls_ports[node]), timeout=3)
        if receive_buffer:
            connection.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, receive_buffer)
        try:
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
            fields = dict(line.split(":", 1) for line in lines[1:] if ":" in line)
            fields = {key.lower(): value.strip() for key, value in fields.items()}
            require(fields.get("sec-websocket-accept") == websocket_accept(key)
                    and fields.get("upgrade", "").lower() == "websocket"
                    and "upgrade" in fields.get("connection", "").lower(), "fixture_ws_upgrade_authentic")
            wire = WireSocket(connection, remainder)
            self.wire_sockets.append(wire)
            return wire
        except Exception:
            connection.close()
            raise

    def owner(self, executor):
        executor = canonical_uuid(executor)
        raw = self.sql(f"SELECT row_to_json(o) FROM api_cluster_executor_owners o WHERE executor_id='{executor}'::uuid;")
        return json.loads(raw) if raw else None

    def epoch(self, node):
        return self.sql(f"SELECT process_epoch FROM api_cluster_nodes WHERE group_id='{self.group}'::uuid AND node_id='{self.node_ids[node]}'::uuid;")

    def new_executor(self, node=1):
        executor = str(uuid.uuid4())
        self.api(0, "/executors", method="POST", expected=201, data={
            "schema_version": 2, "executor_id": executor, "kind": "personal_device", "pool_id": None,
            "owner_user_id": None, "display_name": "Owned protocol fixture", "protocol_version": 2,
            "capabilities": [{"schema_version": 2, "name": name, "version": "1", "attributes": {}}
                             for name in ["shell", "desktop.linux"]], "labels": {"os": "linux"},
            "personal_device_remote_control": "unattended", "max_concurrent_runs": 1})
        credential = self.api(0, f"/executors/{executor}/credentials", expected=201, method="POST",
                              data={"label": "owned cluster fixture", "expires_at": None})
        fake = FakeExecutor(self, node, executor, credential)
        self.until(lambda: self.owner(executor), "durable_executor_registration")
        return fake

    def new_run(self, fake):
        thread = self.api(0, "/threads", expected=201, method="POST", data={
            "project_id": self.project["id"], "title": "Owned cluster protocol fixture"})
        run = self.api(0, "/runs", expected=201, method="POST", data={
            "project_id": self.project["id"], "project_revision": self.project["revision"],
            "thread_id": thread["id"], "project_privacy": "team_managed",
            "executor_target": {"kind": "personal_device", "device_id": fake.executor_id},
            "required_capabilities": ["shell"], "idempotency_key": str(uuid.uuid4()),
            "input": {"private_protocol_fixture": True}})
        run_id = canonical_uuid(run["spec"]["id"])
        self.owned_runs.append(run_id)
        self.until(lambda: fake.lease(run_id), "native_executor_lease_dispatch", seconds=15)
        return run_id

    def new_desktop(self, run, node=0):
        row = self.api(node, f"/runs/{run}/desktop-sessions", expected=201, method="POST",
                       data={"width": 1024, "height": 768})
        return canonical_uuid(row["id"])

    def finish_protocol_run(self, fake, run, session):
        # There can be only one active desktop per run (migration0009). An
        # executor reconnect fences the old desktop binding; it does not prove
        # a physical GUI was stopped or authorise its adoption. This fake has
        # started no GUI, so complete its protocol-only run through the normal
        # authenticated executor path before creating a fresh run and desktop.
        fake.complete(run)
        self.until(lambda: self.api(0, f"/runs/{run}")["state"] == "completed",
                   "replacement_protocol_run_completed")
        self.until(lambda: self.sql(f"SELECT state FROM desktop_sessions WHERE id='{canonical_uuid(session)}'::uuid AND run_id='{canonical_uuid(run)}'::uuid;") == "ended",
                   "completed_protocol_run_ends_old_desktop")

    def stream(self, fake, run, session, *, user_node=0, callback_node=2, token=None,
               control=False, callback=True, slow=False):
        prior = fake.count()
        request = {"control": control}
        if control:
            grant = self.api(user_node, "/auth/reauthenticate", method="POST", token=token,
                             data={"password": self.admin_password, "purpose": "desktop_control"})
            request["reauthentication_token"] = grant["token"]
        ticket = self.api(user_node, f"/runs/{run}/desktop-sessions/{session}/tickets", method="POST",
                          data=request, token=token)
        user = self.websocket(user_node, f"/api/v1/desktop-sessions/{session}/stream?ticket={ticket['token']}")
        command = fake.next_command(prior)
        require(command["run_id"] == run and command["session_id"] == session and command["control"] == control,
                "desktop_command_exact_run_session_control")
        reverse = None
        if callback:
            reverse = self.websocket(callback_node, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{command['stream_id']}",
                                     fake.credential["token"], receive_buffer=4096 if slow else None)
        return {"user": user, "callback": reverse, "command": command, "ticket": ticket}

    def pending(self, stream):
        stream = canonical_uuid(stream)
        raw = self.sql(f"SELECT jsonb_build_object('user_used',user_claimed_at IS NOT NULL,'command_used',command_claimed_at IS NOT NULL,'sent_used',command_sent_at IS NOT NULL,'callback_used',callback_claimed_at IS NOT NULL,'revoked',revoked_at IS NOT NULL,'owner_epoch',owner_process_epoch,'connection_epoch',connection_epoch) FROM api_cluster_desktop_pending WHERE stream_id='{stream}'::uuid;")
        return json.loads(raw) if raw else None

    def assert_idle_commands(self, fake, count, seconds=3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            require(fake.count() == count, "unknown_desktop_command_was_automatically_retried")
            time.sleep(.05)

    def exchange(self, stream):
        # Byte-stream equivalence, including bytes spanning private frame chunks.
        sent = b"owned-rfb-" + bytes(range(256)) * 1200
        stream["user"].binary(sent)
        self.read_bytes(stream["callback"], sent)
        answer = b"owned-screen-" + bytes(range(255, -1, -1)) * 100
        stream["callback"].binary(answer)
        self.read_bytes(stream["user"], answer)

    @staticmethod
    def read_bytes(wire, expected):
        received = bytearray()
        deadline = time.monotonic() + 6
        while len(received) < len(expected):
            message = wire.recv(timeout=max(.01, deadline - time.monotonic()))
            require(message is not None and message[0] == 2, "forwarded_rfb_binary_stream")
            received.extend(message[1])
        require(bytes(received) == expected, "forwarded_rfb_exact_bytes")

    def ready_all(self, nodes=(0, 1, 2)):
        for node in nodes:
            require(self.nodes[node].poll() is None and self.http(self.api_ports[node], "/readyz")[0] == 200,
                    "remaining_cluster_api_ready")

    @contextmanager
    def phase(self, name):
        self.last_phase = name
        before_wire, before_fake, before_runs = len(self.wire_sockets), len(self.fake_executors), len(self.owned_runs)
        before_sse = len(self.streams)
        try:
            yield
        finally:
            errors = []
            for wire in self.wire_sockets[before_wire:]:
                try:
                    wire.close()
                except Exception:
                    errors.append("phase_wire_cleanup")
            for response in self.streams[before_sse:]:
                try:
                    response.close()
                except Exception:
                    errors.append("phase_sse_cleanup")
            for fake in self.fake_executors[before_fake:]:
                self.phase_counts[name] = self.phase_counts.get(name, 0) + fake.count()
                try:
                    fake.close()
                except Exception:
                    errors.append("phase_fake_executor_cleanup")
            # Cancel only this phase's own runs through a still-active API. The
            # fake has never started a tool/GUI; cancellation closes fixture DB state.
            active = next((node for node, child in self.nodes.items() if child.poll() is None), None)
            if active is not None and self.access_token:
                for run in self.owned_runs[before_runs:]:
                    try:
                        self.api(active, f"/runs/{run}/cancel", method="POST", record_failure=False)
                    except Exception:
                        pass
            require(not errors, errors[0] if errors else "phase_cleanup")

    def private_refused(self, cert_node, target, request):
        tls = ssl.create_default_context(cafile=str(self.ca))
        if cert_node is not None:
            tls.load_cert_chain(self.certificates[cert_node], self.keys[cert_node])
        raw = socket.create_connection(("127.0.0.1", self.peer_ports[target]), timeout=3)
        connection = None
        try:
            connection = tls.wrap_socket(raw, server_hostname=f"node{target}.internal")
            body = json.dumps(request).encode()
            connection.sendall(struct.pack("!I", len(body)) + body)
            connection.settimeout(3)
            try:
                answer = connection.recv(4)
                require(not answer, "untrusted_private_peer_must_not_acknowledge")
            except (ConnectionError, ssl.SSLError):
                pass
        except ssl.SSLError:
            pass  # Missing/untrusted client certificate may fail during TLS.
        finally:
            (connection or raw).close()

    def cleanup(self):
        saved = {number: signal.getsignal(number) for number in HANDLED_SIGNALS}
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        errors = []
        try:
            for wire in self.wire_sockets:
                try:
                    wire.close()
                except Exception:
                    errors.append("owned_wire_cleanup")
            for fake in self.fake_executors:
                try:
                    fake.close()
                except Exception:
                    errors.append("fake_executor_cleanup")
            for proxy in self.proxies:
                try:
                    proxy.close()
                except Exception:
                    errors.append("owned_proxy_cleanup")
            try:
                super().cleanup()
            except Exception:
                errors.append("owned_native_fixture_cleanup")
        finally:
            for number, handler in saved.items():
                signal.signal(number, handler)
        require(not errors, errors[0] if errors else "owned_cluster_cleanup")


def sse_event(response, marker, check, seconds=4, forbidden_marker=None):
    deadline, event_id, data = time.monotonic() + seconds, None, []
    while time.monotonic() < deadline:
        line = response.readline().decode("utf-8").rstrip("\r\n")
        if not line:
            if data:
                event = json.loads("\n".join(data))
                if forbidden_marker is not None:
                    require(event.get("payload", {}).get("fixture_marker") != forbidden_marker,
                            "last_event_id_excludes_already_delivered_event")
                if event.get("payload", {}).get("fixture_marker") == marker:
                    require(event_id is not None, check)
                    return int(event_id)
            event_id, data = None, []
        elif line.startswith("id:"):
            event_id = line[3:].strip()
        elif line.startswith("data:"):
            data.append(line[5:].lstrip())
    raise QualificationFailure(check)


def open_run_sse(fixture, node, run, token, last=0):
    from urllib.request import Request
    response = fixture.opener.open(Request(f"http://127.0.0.1:{fixture.api_ports[node]}/api/v1/runs/{run}/events",
        headers={"Authorization": "Bearer " + token, "Accept": "text/event-stream", "Last-Event-ID": str(last)}), timeout=4)
    fixture.streams.append(response)
    require(response.status == 200, "authenticated_run_sse_response")
    return response


def qualify(fixture, report, expected_migration):
    def passed(name):
        report["checks"].append({"name": name, "passed": True})

    fixture.start_cluster()
    fixture.prepare_certificates()
    fixture.start_proxies()
    for node in range(3):
        fixture.start_node(node)
    require(fixture.sql("SELECT max(version) FROM _sqlx_migrations WHERE success;") == str(expected_migration),
            "expected_cluster_database_migration")
    status, _, body = fixture.http(fixture.api_ports[0], "/api/v1/auth/bootstrap", method="POST",
        token=fixture.bootstrap_secret, data={"email": "qualification@localhost.invalid", "display_name": "Owned cluster fixture",
                                            "password": fixture.admin_password, "device_id": str(uuid.uuid4())})
    require(status == 201, "cluster_bootstrap_admin")
    fixture.access_token = json.loads(body)["access_token"]
    team = fixture.api(0, "/teams", expected=201, method="POST", data={"name": "Owned cluster fixture"})
    fixture.project = fixture.api(0, "/projects", expected=201, method="POST", data={
        "name": "Owned cluster fixture", "privacy": "team_managed", "team_id": team["id"]})
    for node in range(3):
        metrics = fixture.metrics(fixture.api_ports[node])
        require(not metrics["application"]["runner_configured"] and not metrics["application"]["object_store_configured"],
                "cluster_no_runner_s3_or_fleet_credentials")
        require(metrics["runtime"]["api_routing"]["mode"] == "experimental_api_cluster"
                and metrics["runtime"]["api_routing"]["multiple_api_replicas_supported"] is False,
                "cluster_status_preserves_qualification_boundary")
    fixture.ready_all()
    passed("three_native_api_processes_fresh_pg16_mtls_group_and_authenticated_readiness")
    report["database_migration_version"] = expected_migration

    # Mixed default singleton and configured cluster processes must never coexist.
    refused_port = fixture.reserve_port()
    fixture.release_port(refused_port)
    refused = fixture.spawn([str(fixture.binary)], "default_singleton_refused",
                            Fixture.server_environment(fixture, "api", refused_port))
    require(refused.wait(timeout=12) != 0, "singleton_refused_while_cluster_shared_locks_active")
    passed("default_singleton_refused_while_cluster_nodes_own_database")

    with fixture.phase("forwarding_replay"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run, node=2)
        stream = fixture.stream(fake, run, session, user_node=0, callback_node=2, control=True)
        fixture.exchange(stream)
        row = fixture.pending(stream["command"]["stream_id"])
        require(all(row[key] for key in ["user_used", "command_used", "sent_used", "callback_used"])
                and not row["revoked"] and row["owner_epoch"] == fixture.epoch(1)
                and row["connection_epoch"] == fixture.owner(fake.executor_id)["connection_epoch"],
                "durable_forwarding_exact_process_connection_and_one_use_slots")
        passed("cross_api_desktop_start_control_user_wss_executor_callback_and_bidirectional_rfb")
        fixture.websocket(2, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{stream['command']['stream_id']}",
                          fake.credential["token"], expected=409)
        fixture.websocket(0, f"/api/v1/desktop-sessions/{session}/stream?ticket={stream['ticket']['token']}", expected=401)
        require(fake.count() == 1, "one_use_replay_has_exactly_one_desktop_command")
        passed("public_user_ticket_and_executor_callback_replays_refused_without_command_retry")
        marker1, marker2 = uuid.uuid4().hex, uuid.uuid4().hex
        fake.event(run, marker1)
        first = open_run_sse(fixture, 2, run, fixture.access_token)
        cursor = sse_event(first, marker1, "cross_api_durable_run_event_replay")
        first.close()
        replay = open_run_sse(fixture, 0, run, fixture.access_token, cursor)
        started = time.monotonic()
        fake.event(run, marker2)
        next_cursor = sse_event(replay, marker2, "cross_api_notify_wakeup", forbidden_marker=marker1)
        require(next_cursor > cursor and time.monotonic() - started < 4, "notify_wakeup_precedes_replay_fallback")
        fake.complete(run)
        fixture.assert_sse_closed(replay, "terminal_run_sse_closes")
        passed("durable_run_sse_last_event_replay_cross_api_notify_and_terminal_close")

    with fixture.phase("private_identity"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        # Clone only owned, immutable routing references into an unused slot;
        # it is never granted through an untrusted public API.
        original = fixture.stream(fake, run, session, callback=False)
        source_stream = original["command"]["stream_id"]
        original["user"].close()
        probe = str(uuid.uuid4())
        fixture.sql(f"INSERT INTO api_cluster_desktop_pending(stream_id,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,executor_id,connection_epoch,credential_id,desktop_session_id,run_id,user_id,auth_session_id,control,expires_at) SELECT '{probe}'::uuid,group_id,origin_node_id,origin_process_epoch,owner_node_id,owner_process_epoch,executor_id,connection_epoch,credential_id,desktop_session_id,run_id,user_id,auth_session_id,false,clock_timestamp()+interval '1 minute' FROM api_cluster_desktop_pending WHERE stream_id='{canonical_uuid(source_stream)}'::uuid;")
        request = {"protocol": 1, "group_id": fixture.group, "source_node_id": fixture.node_ids[0],
            "source_process_epoch": fixture.epoch(0), "target_node_id": fixture.node_ids[1],
            "target_process_epoch": fixture.epoch(1), "stream_id": probe, "role": "user",
            "executor_id": None, "credential_id": None}
        fixture.private_refused(3, 1, request)
        fixture.private_refused(None, 1, request)
        for field, changed in [("source_node_id", fixture.node_ids[2]), ("group_id", str(uuid.uuid4())),
                               ("source_process_epoch", str(uuid.uuid4())), ("target_process_epoch", str(uuid.uuid4()))]:
            fixture.private_refused(0, 1, dict(request, **{field: changed}))
        fixture.private_refused(0, 1, dict(request, callback_url="https://payload.invalid"))
        row = fixture.pending(probe)
        require(not any(row[key] for key in ["user_used", "command_used", "sent_used", "callback_used"]),
                "untrusted_peers_did_not_consume_private_slot")
        fixture.sql(f"UPDATE api_cluster_desktop_pending SET expires_at=clock_timestamp()-interval '1 second' WHERE stream_id='{probe}'::uuid;")
        fixture.private_refused(0, 1, request)
        row = fixture.pending(probe)
        require(not row["user_used"] and not row["command_used"], "expired_private_ticket_refused_before_command")
        fixture.assert_idle_commands(fake, 1)
        passed("missing_unallowlisted_or_spoofed_peer_identity_injected_url_and_expired_ticket_refused")

    with fixture.phase("user_revocation"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        independent = fixture.new_admin_session(fixture.api_ports[2])
        stream = fixture.stream(fake, run, session, token=independent["access_token"])
        fixture.exchange(stream)
        sync = fixture.open_sync_sse(fixture.api_ports[1], independent["access_token"])
        fixture.api(2, "/auth/logout", method="POST", expected=204, token=independent["access_token"])
        stream["user"].assert_closed("revoked_user_forwarded_stream_closed")
        stream["callback"].assert_closed("revoked_user_executor_callback_closed")
        fixture.assert_sse_closed(sync, "revoked_user_sync_sse_closed")
        require(fixture.http(fixture.api_ports[0], "/api/v1/operations/metrics", token=independent["access_token"])[0] == 401,
                "revoked_user_token_is_rejected_across_nodes")
        fixture.assert_idle_commands(fake, 1)
        require(not fake.closed.is_set(), "user_logout_does_not_revoke_executor")
        passed("cross_api_user_session_revocation_closes_user_callback_and_sync_sse")

    with fixture.phase("credential_revocation"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session)
        fixture.exchange(stream)
        fake.expected_close = True
        fixture.api(0, f"/executors/{fake.executor_id}/credentials/{fake.credential['credential_id']}/revoke",
                    method="POST", expected=204)
        fixture.until(fake.closed.is_set, "revoked_executor_control_socket_closed", seconds=6)
        stream["user"].assert_closed("revoked_credential_user_stream_closed")
        stream["callback"].assert_closed("revoked_credential_callback_closed")
        fixture.websocket(2, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{stream['command']['stream_id']}",
                          fake.credential["token"], expected=401)
        fixture.assert_idle_commands(fake, 1)
        passed("credential_revocation_fences_control_callback_and_forwarded_stream_without_retry")

    with fixture.phase("owner_change"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session)
        lease_token = fake.lease(run)
        before = fixture.owner(fake.executor_id)
        fake.expected_close = True
        replacement = FakeExecutor(fixture, 2, fake.executor_id, fake.credential)
        after = fixture.until(lambda: (row if (row := fixture.owner(fake.executor_id))
                                      and row["connection_epoch"] != before["connection_epoch"] else None),
                              "durable_owner_replacement")
        require(after["node_id"] == fixture.node_ids[2], "reconnect_routes_to_new_owner_node")
        stream["user"].assert_closed("owner_change_closes_old_user_stream")
        stream["callback"].assert_closed("owner_change_closes_old_callback")
        fixture.websocket(0, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{stream['command']['stream_id']}",
                          fake.credential["token"], expected=409)
        fixture.assert_idle_commands(fake, 1)
        require(replacement.count() == 0, "owner_change_never_replays_old_desktop_command")
        fixture.until(lambda: replacement.lease(run), "replacement_current_run_lease")
        require(replacement.lease(run) == lease_token, "replacement_recovers_same_run_lease_token")
        fixture.finish_protocol_run(replacement, run, session)
        run = fixture.new_run(replacement)
        new_session = fixture.new_desktop(run, node=1)
        new_stream = fixture.stream(replacement, run, new_session, callback_node=1)
        fixture.exchange(new_stream)
        passed("executor_reconnect_changes_epoch_closes_stale_streams_and_routes_fresh_session")

    with fixture.phase("unknown_outcome"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session, callback=False)
        stream["user"].close()  # The dispatched request has no successful callback.
        row = fixture.until(lambda: (p if (p := fixture.pending(stream["command"]["stream_id"])) and p["revoked"] else None),
                            "dispatched_unknown_stream_durably_revoked")
        require(row["command_used"] and row["sent_used"] and not row["callback_used"],
                "unknown_outcome_retains_consumed_command_slots")
        fixture.websocket(2, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{stream['command']['stream_id']}",
                          fake.credential["token"], expected=409)
        fixture.assert_idle_commands(fake, 1, seconds=4)
        passed("unknown_dispatched_desktop_outcome_never_authorises_retry_or_late_callback")

    with fixture.phase("partition"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session)
        fixture.exchange(stream)
        for source in [0, 2]:
            fixture.peer_proxies[source, 1].partition(True)
        try:
            stream["user"].assert_closed("private_partition_closes_user_stream")
            stream["callback"].assert_closed("private_partition_closes_callback")
            fixture.assert_idle_commands(fake, 1)
            ticket = fixture.api(0, f"/runs/{run}/desktop-sessions/{session}/tickets", method="POST", data={"control": False})
            fixture.websocket(0, f"/api/v1/desktop-sessions/{session}/stream?ticket={ticket['token']}", expected=409)
            require(fake.count() == 1, "partition_refusal_emits_no_command")
            fixture.ready_all()
        finally:
            for source in [0, 2]:
                fixture.peer_proxies[source, 1].partition(False)
        healed = fixture.stream(fake, run, session)
        fixture.exchange(healed)
        require(fake.count() == 2, "partition_heal_requires_one_explicit_fresh_user_request")
        passed("private_peer_partition_fails_closed_no_automatic_retry_and_fresh_request_heals")

    with fixture.phase("backpressure"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session, slow=True)
        writer_stopped = threading.Event()
        def flood():
            try:
                for _ in range(48):
                    stream["user"].binary(b"x" * (1024 * 1024))
            except Exception:
                pass
            finally:
                writer_stopped.set()
        writer = threading.Thread(target=flood, name="owned-backpressure-writer")
        writer.start()
        try:
            fixture.until(writer_stopped.is_set, "owned_backpressure_writer_bounded", seconds=20)
            stream["user"].assert_closed("slow_callback_has_bounded_backpressure_close", seconds=20)
        finally:
            stream["user"].close()
            writer.join(timeout=5)
        require(writer_stopped.is_set() and not writer.is_alive(), "owned_backpressure_writer_cleanup")
        fixture.ready_all()
        fixture.assert_idle_commands(fake, 1)
        passed("slow_executor_callback_backpressure_is_bounded_and_never_retries_command")

    old_epoch = fixture.epoch(2)
    held_sync = fixture.open_sync_sse(fixture.api_ports[2])
    high, low = API_LOCK_KEY >> 32, API_LOCK_KEY & 0xffffffff
    application_name = fixture.identity + "_node2"
    lock_pid = fixture.sql(f"SELECT a.pid FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid WHERE a.datname='{fixture.database}' AND a.application_name='{application_name}' AND l.locktype='advisory' AND l.classid::bigint={high} AND l.objid::bigint={low} AND l.objsubid=1 AND l.mode='ShareLock' AND l.granted;")
    require(lock_pid.isdecimal(), "exact_owned_node_shared_lock_backend")
    require(fixture.sql(f"SELECT pg_terminate_backend({int(lock_pid)});") == "t", "owned_shared_lock_backend_terminated")
    fixture.until(lambda: fixture.nodes[2].poll() is not None, "node_session_loss_fail_closed_exit", seconds=15)
    require(fixture.nodes[2].returncode != 0, "node_session_loss_exit_is_failure")
    fixture.assert_sse_closed(held_sync, "node_session_loss_closes_sync_sse", allow_abrupt=True)
    fixture.ready_all((0, 1))
    fixture.until(lambda: fixture.sql(f"SELECT lease_until<=clock_timestamp() FROM api_cluster_nodes WHERE group_id='{fixture.group}' AND node_id='{fixture.node_ids[2]}';") == "t",
                  "lost_node_db_lease_expired", seconds=12)
    fixture.start_node(2, generation=1)
    require(fixture.epoch(2) != old_epoch, "node_restart_has_new_process_epoch")
    passed("exact_shared_lock_session_loss_stops_only_owned_api_and_restart_mints_new_epoch")

    with fixture.phase("kill_reconnect"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session)
        lease_token = fake.lease(run)
        epoch = fixture.epoch(1)
        fake.expected_close = True
        fixture.kill_group(fixture.nodes[1])
        stream["user"].assert_closed("killed_owner_closes_user_stream")
        stream["callback"].assert_closed("killed_owner_closes_callback")
        fixture.ready_all((0, 2))
        fixture.assert_idle_commands(fake, 1)
        replacement = FakeExecutor(fixture, 0, fake.executor_id, fake.credential)
        fixture.until(lambda: replacement.lease(run), "killed_owner_reconnect_lease")
        require(replacement.lease(run) == lease_token, "killed_owner_recovers_same_run_lease_token")
        require(replacement.count() == 0, "killed_owner_reconnect_does_not_replay_command")
        fixture.until(lambda: fixture.sql(f"SELECT lease_until<=clock_timestamp() FROM api_cluster_nodes WHERE group_id='{fixture.group}' AND node_id='{fixture.node_ids[1]}';") == "t",
                      "killed_owner_db_lease_expired", seconds=12)
        fixture.start_node(1, generation=1)
        require(fixture.epoch(1) != epoch, "killed_owner_restart_epoch_changed")
        fixture.finish_protocol_run(replacement, run, session)
        run = fixture.new_run(replacement)
        fresh_session = fixture.new_desktop(run, node=2)
        fresh_stream = fixture.stream(replacement, run, fresh_session, user_node=2, callback_node=1)
        fixture.exchange(fresh_stream)
        fixture.websocket(1, f"/api/v1/agent/executors/{fake.executor_id}/desktop-streams/{stream['command']['stream_id']}",
                          fake.credential["token"], expected=409)
        passed("sigkill_owner_reconnect_on_other_api_fences_old_session_and_routes_fresh_session")

    with fixture.phase("drain"):
        fake = fixture.new_executor()
        run = fixture.new_run(fake)
        session = fixture.new_desktop(run)
        stream = fixture.stream(fake, run, session)
        lease_token = fake.lease(run)
        held = fixture.open_sync_sse(fixture.api_ports[1])
        fake.expected_close = True
        fixture.stop_server(fixture.nodes[1], "cluster_owner_drain", timeout=15)
        fixture.assert_sse_closed(held, "graceful_owner_drain_closes_sync_sse")
        stream["user"].assert_closed("graceful_owner_drain_closes_user_stream")
        stream["callback"].assert_closed("graceful_owner_drain_closes_callback")
        fixture.ready_all((0, 2))
        fixture.assert_idle_commands(fake, 1)
        replacement = FakeExecutor(fixture, 2, fake.executor_id, fake.credential)
        fixture.until(lambda: replacement.lease(run), "drained_owner_reconnect_lease")
        require(replacement.lease(run) == lease_token, "drained_owner_recovers_same_run_lease_token")
        fixture.finish_protocol_run(replacement, run, session)
        run = fixture.new_run(replacement)
        fresh_session = fixture.new_desktop(run)
        fresh_stream = fixture.stream(replacement, run, fresh_session, callback_node=0)
        fixture.exchange(fresh_stream)
        passed("sigterm_owner_drain_exits_zero_closes_held_streams_and_remaining_apis_continue")

    for node in [2, 0]:
        held = fixture.open_sync_sse(fixture.api_ports[node])
        fixture.stop_server(fixture.nodes[node], f"cluster_final_node{node}", timeout=15)
        fixture.assert_sse_closed(held, "final_cluster_sigterm_held_sse_drain")
    passed("all_owned_api_processes_drain_cleanly_with_held_sse")
    report["desktop_command_counts_by_phase"] = fixture.phase_counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-bin", required=True, type=Path)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-migration", required=True, type=int)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    def interrupted(signum, frame):
        for number in HANDLED_SIGNALS:
            signal.signal(number, signal.SIG_IGN)
        raise QualificationFailure("api_cluster_qualification_interrupted")
    for number in HANDLED_SIGNALS:
        signal.signal(number, interrupted)
    started = time.monotonic()
    report = {"schema_version": 1, "passed": False, "checks": [],
        "started_at": datetime.now(timezone.utc).isoformat(),
        "scope": {"native_api_processes": 3, "fresh_postgresql16": True,
                  "native_private_mtls": True, "public_wss_fixture_tls_proxy": True,
                  "fake_executor_protocol_only": True, "real_runner_or_desktop_execution": False,
                  "production_ha_release": False, "docker": False, "live_fleet": False},
        "limitations": ["The fake executor consumes leases/events and forwards bytes; it never executes a tool or GUI.",
            "Public WSS uses owned TLS front proxies; native API public listeners are loopback HTTP.",
            "This executor-protocol fixture does not exercise physical worker-created Linux desktop ownership or cross-owner Linux stop.",
            "A passing isolated matrix is evidence for these paths, not a blanket production HA approval.",
            "SIGKILL or host termination cannot execute fixture cleanup handlers."]}
    try:
        inputs = qualification_inputs(args)
        require(os.name == "posix" and os.geteuid() != 0, "linux_nonroot_execution")
        require((PG_BIN / "initdb").is_file() and shutil.which("openssl"), "pg16_and_openssl_available")
        source = inputs["binary"].resolve(strict=True)
        require(source.is_file() and os.access(source, os.X_OK), "candidate_binary_executable")
        require(file_sha256(source) == inputs["sha256"], "candidate_matches_approved_sha256")
        report["candidate_sha256"] = inputs["sha256"]
        report["harness_sha256"] = file_sha256(Path(__file__))
        report["fixture_helper_sha256"] = file_sha256(Path(__file__).with_name("qualify_runtime.py"))
        with tempfile.TemporaryDirectory(prefix="cowork-api-cluster-qualification-", dir="/tmp") as directory:
            root = Path(directory)
            binary = root / "approved-candidate"
            shutil.copyfile(source, binary)
            binary.chmod(0o500)
            require(file_sha256(binary) == inputs["sha256"], "isolated_copy_matches_approved_candidate")
            fixture = ClusterFixture(root, binary, inputs["sha256"])
            try:
                qualify(fixture, report, inputs["migration"])
            finally:
                report["diagnostics"] = {"node_process_states": {
                    str(node): "running" if child.poll() is None else "exited" for node, child in fixture.nodes.items()},
                    "last_phase": fixture.last_phase, "last_api_status": fixture.last_api_status,
                    "failed_api_status": fixture.failed_api_status,
                    "last_ws_status": fixture.last_ws_status,
                    "fake_executor_command_counts": [fake.count() for fake in fixture.fake_executors],
                    "fake_executor_error_codes": sorted({code for fake in fixture.fake_executors for code in fake.errors}),
                    "desktop_command_counts_by_phase": fixture.phase_counts,
                    "private_proxy_accept_count": sum(proxy.accepted for proxy in fixture.peer_proxies.values())}
                fixture.cleanup()
        report["passed"] = True
    except QualificationFailure as error:
        report["failure_check"] = str(error)
    except Exception as error:
        report["failure_kind"] = type(error).__name__
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, separators=(",", ":")))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
