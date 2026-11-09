#!/usr/bin/env python3
"""Disposable management-to-KVM lifecycle qualification; never a live cutover.

Requires new candidate image IDs explicitly. Only --execute creates resources.
All Docker names, volumes, networks, identities, keys and VM state are freshly
UUID-scoped. Failed fixtures are stopped and retained for scoped investigation;
successful fixtures prove deletion before their exact inventory is removed.
"""
import argparse
import base64
from contextlib import ExitStack
from datetime import datetime
import hashlib
import hmac
import importlib.util
import io
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import ssl
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, ProxyHandler, Request, build_opener
import uuid
try:
    import resource
except ImportError:
    resource = None

import hosted_vm_transport as transport

PHASES = frozenset({
    'parent_inventory', 'candidate_package', 'private_control_services', 'management_ready',
    'bootstrap_admin', 'totp_setup', 'totp_enable', 'create_organisation', 'organisation_policy',
    'parent_capacity', 'guest_create', 'guest_bootstrap', 'guest_apparmor', 'guest_registry_ca',
    'guest_docker_info', 'host_register', 'host_credential', 'host_capacity_start', 'instance_create',
    'guest_image_pull', 'guest_networks', 'operator_preparation', 'operator_context', 'manager_schema',
    'operator_auth_fences', 'reserved_host_capacity', 'operation_request', 'operation_claim',
    'operation_execute', 'operation_complete', 'operation_receipt', 'guest_runtime',
    'guest_login', 'guest_raw_run', 'guest_artifact', 'execution_release', 'full_lifecycle',
    'guest_deletion', 'guest_network_cleanup', 'guest_stop', 'parent_cleanup', 'private_cleanup',
    'auth_refresh', 'host_capacity_challenge', 'host_capacity_attest', 'operator_request', 'api_request',
})
API_ERROR_CODES = frozenset({
    'bad_request', 'unauthorized', 'mfa_required', 'not_found', 'conflict', 'unprocessable',
    'quota_exceeded', 'unavailable', 'database_error', 'invalid_json', 'internal_error',
})
HOST_ERROR_CODES = frozenset({
    'host_disabled', 'host_capacity_unqualified', 'host_capacity_unqualified_or_stale',
    'host_capacity_exhausted', 'host_allocation_unknown', 'host_allocation_unqualified',
    'host_allocation_unqualified_or_stale', 'host_allocation_plan_mismatch',
    'host_capacity_attestation_failed', 'host_capacity_renewal_did_not_stop',
    'executor_daemon_identity_mismatch', 'executor_trust_domain_mismatch',
    'executor_host_resource_capacity_mismatch', 'executor_daemon_identity_unproven',
    'executor_host_legacy_state_requires_review', 'executor_host_migration_requires_review',
    'invalid_executor_tls_credentials', 'unsafe_executor_tls_file', 'unsafe_executor_tls_permissions',
})


def phase(report, name):
    require(name in PHASES, 'invalid_qualification_phase')
    report['phase'] = name


def http_observation(name, expected, actual, content=b''):
    require(name in PHASES and type(expected) is int and type(actual) is int
            and 100 <= expected <= 599 and 100 <= actual <= 599, 'invalid_http_observation')
    value = {'phase': name, 'expected_status': expected, 'actual_status': actual}
    # Error messages can contain paths, credentials or user text. Only exact
    # server-owned code values are copied; the response body itself is discarded.
    try:
        body = json.loads(content) if len(content) <= 4096 else None
    except (ValueError, UnicodeError):
        body = None
    if isinstance(body, dict):
        if isinstance(body.get('error'), str) and body['error'] in API_ERROR_CODES:
            value['api_error_code'] = body['error']
        if isinstance(body.get('message'), str) and body['message'] in HOST_ERROR_CODES:
            value['host_error_code'] = body['message']
    return value


class HttpQualificationFailure(RuntimeError):
    def __init__(self, observation):
        super().__init__('http_status_unexpected')
        self.observation = observation


class PublicDiagnostics:
    def __init__(self):
        self.lock = threading.Lock()
        self.values = {}

    def http(self, observation, *, host=False):
        with self.lock:
            self.values['last_host_http_status' if host else 'last_http_status'] = observation
            if observation['expected_status'] != observation['actual_status']:
                self.values['last_host_http_failure' if host else 'last_unexpected_http_status'] = observation

    def host_rejection(self, result):
        if isinstance(result, dict) and result.get('qualified') is False:
            reason = result.get('reason')
            if isinstance(reason, str) and reason in HOST_ERROR_CODES:
                with self.lock:
                    self.values['host_capacity_rejection_code'] = reason

    def guest_command(self, observation):
        # Fixed roles/verbs and one record per command keep this inventory finite.
        allowed = {'version', 'image_role', 'action', 'timeout_seconds', 'status', 'exit_code',
            'termination_signal', 'timed_out', 'interrupted', 'owned_child_started', 'owned_child_reaped',
            'duration_millis', 'registry_before', 'registry_after', 'registry_delta', 'stdout', 'stderr',
            'observed_error_classification'}
        require(type(observation) is dict and set(observation) <= allowed
                and observation.get('image_role') in transport.REGISTRY_IMAGE_ROLES
                and observation.get('action') in {'pull', 'inspect'}
                and observation.get('status') in {'prepared', 'running', 'completed', 'supervision_pending'},
                'invalid_public_guest_command')
        with self.lock:
            rows = self.values.setdefault('guest_image_commands', [])
            key = (observation['image_role'], observation['action'])
            previous = next((i for i, row in enumerate(rows)
                             if (row['image_role'], row['action']) == key), None)
            value = json.loads(json.dumps(observation))
            if previous is None:
                require(len(rows) < 2 * len(transport.REGISTRY_IMAGE_ROLES), 'guest_command_inventory_limit')
                rows.append(value)
            else:
                rows[previous] = value

    def snapshot(self):
        with self.lock:
            return dict(self.values)


class ObservedManagerHttp:
    def __init__(self, http, diagnostics):
        self.http, self.diagnostics = http, diagnostics

    def open(self, req, **kwargs):
        path = urlsplit(req.full_url).path
        name = {'/management/runner/v1/host-capacity/challenge': 'host_capacity_challenge',
                '/management/runner/v1/host-capacity/attest': 'host_capacity_attest'}.get(path, 'operator_request')
        try:
            response = self.http.open(req, **kwargs)
        except HTTPError as error:
            try:
                content = error.read(4096)
            finally:
                error.close()
            self.diagnostics.http(http_observation(name, 200, error.code, content), host=True)
            # Preserve the existing operator HTTP exception/retry behavior.
            raise
        observation = http_observation(name, 200, response.status)
        self.diagnostics.http(observation, host=True)
        if response.status != 200:
            response.close()
            raise HttpQualificationFailure(observation)
        return response


def observe_manager(manager, diagnostics):
    manager.http = ObservedManagerHttp(manager.http, diagnostics)
    original = manager.call
    def call_observed(path, payload):
        result = original(path, payload)
        if path == '/host-capacity/attest':
            diagnostics.host_rejection(result)
        return result
    manager.call = call_observed
    return manager


def timestamp(value):
    try:
        parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
        require(parsed.tzinfo is not None, 'invalid_fixture_auth_tokens')
        return parsed.timestamp()
    except (TypeError, ValueError, AttributeError):
        raise RuntimeError('invalid_fixture_auth_tokens') from None


class AdminSession:
    """Rotate the original session before a request; never replay a failed POST."""
    def __init__(self, opener, origin, tokens, diagnostics, *, clock=time.time):
        self.opener, self.origin, self.diagnostics, self.clock = opener, origin, diagnostics, clock
        self.tokens = self.validated(tokens)

    @staticmethod
    def validated(tokens):
        require(isinstance(tokens, dict) and all(isinstance(tokens.get(key), str)
                and 16 <= len(tokens[key]) <= 16384 for key in ['access_token', 'refresh_token']),
                'invalid_fixture_auth_tokens')
        for key in ['user_id', 'session_id']:
            public_identity(tokens.get(key), 'invalid_fixture_auth_tokens')
        timestamp(tokens.get('access_expires_at')); timestamp(tokens.get('refresh_expires_at'))
        return dict(tokens)

    def access_token(self):
        if timestamp(self.tokens['access_expires_at']) <= self.clock() + 60:
            require(timestamp(self.tokens['refresh_expires_at']) > self.clock() + 30,
                    'fixture_admin_refresh_expired')
            updated = self.validated(request(self.opener, self.origin, '/api/v1/auth/refresh',
                body={'refresh_token': self.tokens['refresh_token']}, phase_name='auth_refresh',
                diagnostics=self.diagnostics))
            require(all(updated[key] == self.tokens[key] for key in ['user_id', 'session_id'])
                    and timestamp(updated['refresh_expires_at']) == timestamp(self.tokens['refresh_expires_at'])
                    and updated['refresh_token'] != self.tokens['refresh_token']
                    and timestamp(updated['access_expires_at']) > self.clock() + 60,
                    'fixture_admin_refresh_scope_changed')
            self.tokens = updated
        return self.tokens['access_token']


class QualificationSignals:
    def __enter__(self):
        self.interrupted = False
        self.previous = {number: signal.getsignal(number) for number in [signal.SIGINT, signal.SIGTERM]}
        def interrupted(number, frame):
            if not self.interrupted:
                self.interrupted = True
                raise RuntimeError('qualification_interrupted')
        for number in self.previous:
            signal.signal(number, interrupted)
        return self

    def __exit__(self, *ignored):
        for number, handler in self.previous.items():
            signal.signal(number, handler)


def public_identity(value, code):
    try:
        require(isinstance(value, str) and str(uuid.UUID(value)) == value and uuid.UUID(value).int != 0, code)
    except (TypeError, ValueError, AttributeError):
        raise RuntimeError(code) from None
    return value


def accept_host(report, report_path, registered, binding):
    require(isinstance(registered, dict) and registered.get('id') == binding['host_id']
            and registered.get('trust_domain') == binding['trust_domain']
            and type(registered.get('revision')) is int and registered['revision'] > 0,
            'host_registration_scope_mismatch')
    report.update(host_binding=dict(binding), host_revision=registered['revision'])
    durable_report(report_path, report)


def accept_credential(report, report_path, credential, binding):
    require(isinstance(credential, dict) and credential.get('host_id') == binding['host_id'],
            'host_credential_scope_mismatch')
    report['host_credential_id'] = public_identity(credential.get('id'), 'host_credential_scope_mismatch')
    durable_report(report_path, report)


def start_capacity_observation(package, config, stack, diagnostics):
    # Infrastructure's host-only constructor does not need an instance or an
    # image. The real attestor obtains a fresh DB-time challenge and observes the
    # complete guest daemon; no locally assigned boolean grants qualification.
    require(not any(key in config for key in ['instance_id', 'guest_runtime_endpoints',
            'remote_lifecycle_qualification']), 'initial_host_context_contains_instance')
    infrastructure = package.Infrastructure(config)
    manager = observe_manager(package.Manager(config), diagnostics)
    attestor = package.host_capacity_attestation.CapacityAttestor(manager, infrastructure)
    infrastructure.capacity_attestor = manager.capacity_attestor = attestor
    stack.callback(attestor.close)  # Also closes partial/failed start and SIGINT.
    attestor.start()
    with attestor.lock:
        require(attestor.configured is not None and attestor.qualified,
                'initial_empty_host_capacity_not_qualified')
    return infrastructure, manager, attestor


def complete_capacity_context(package, config, attestor):
    # The same keepalive survives long image pulls/preparation. Freeze renewal
    # while validating/rebinding; do not start a competing challenge producer.
    with attestor.lock:
        previous = attestor.infra.config
        require(all(config.get(key) == previous.get(key) for key in ['state_directory', 'backend',
                'executor_host', 'fleet_host_budget', 'management_url', 'management_ca_file', 'provisioner_token_file'])
                and package.resource_budget.service_caps(config) == package.resource_budget.service_caps(previous)
                and package.host_capacity_attestation.settings(config) == attestor.configured,
                'completed_host_context_scope_changed')
        infrastructure = package.Infrastructure(config)
        require(infrastructure.root == attestor.infra.root, 'completed_host_context_scope_changed')
        infrastructure.capacity_attestor = attestor
        attestor.infra = infrastructure
        return infrastructure, attestor.manager

RELAY = """import os,select,socket,sys
host,port=sys.argv[1],int(sys.argv[2])
with socket.create_connection((host,port),timeout=10) as target:
 target.settimeout(120)
 while True:
  ready,_,_=select.select([sys.stdin.buffer,target],[],[],120)
  if not ready: break
  for source in ready:
   data=os.read(0,65536) if source is sys.stdin.buffer else target.recv(65536)
   if not data: sys.exit(0)
   if source is sys.stdin.buffer: target.sendall(data)
   else: sys.stdout.buffer.write(data);sys.stdout.buffer.flush()
"""


def require(value, code):
    if not value:
        raise RuntimeError(code)


def identity():
    return str(uuid.uuid4())


def image_id(value):
    if not re.fullmatch(r'sha256:[a-f0-9]{64}', value):
        raise argparse.ArgumentTypeError('exact docker image inspect .Id required')
    return value


def write(path, value, mode=0o600):
    content = value if isinstance(value, bytes) else value.encode()
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def durable_report(path, report):
    path = Path(path)
    require(path.is_absolute() and not path.is_symlink(), 'unsafe_qualification_report')
    path.parent.mkdir(parents=True, exist_ok=True)
    require(all(not parent.is_symlink() for parent in path.parents), 'unsafe_qualification_report')
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.tmp')
    write(temporary, json.dumps(report, indent=2))
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(descriptor)
    finally: os.close(descriptor)


def private_fixture_root(scope):
    parent = Path.home() / '.local/share/cowork/qualification'
    require(all(not path.is_symlink() for path in [parent, *parent.parents]), 'unsafe_qualification_parent')
    parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    require(parent.stat().st_uid == os.geteuid() and not parent.stat().st_mode & 0o077,
            'private_qualification_parent_required')
    root = parent / scope
    root.mkdir(mode=0o700)  # UUID collision refuses reuse.
    return root.resolve()


def failure_code(error):
    if str(error) in HOST_ERROR_CODES:
        return str(error)
    code = str(error) if isinstance(error, RuntimeError) else type(error).__name__
    return code if re.fullmatch('[a-zA-Z0-9_:.-]{1,200}', code) else 'qualification_failure'


def control_subnet(scope, networks):
    require(isinstance(networks, list) and len(networks) <= 10000, 'invalid_parent_network_inventory')
    occupied = []
    try:
        for network in networks:
            require(isinstance(network, dict) and isinstance(network.get('IPAM'), dict), 'invalid_parent_network_inventory')
            entries = network['IPAM'].get('Config') or []
            require(isinstance(entries, list) and len(entries) <= 64, 'invalid_parent_network_inventory')
            for entry in entries:
                require(isinstance(entry, dict), 'invalid_parent_network_inventory')
                if entry.get('Subnet'):
                    occupied.append(ipaddress.ip_network(entry['Subnet'], strict=True))
    except (ValueError, TypeError):
        raise RuntimeError('invalid_parent_network_subnet') from None
    # Only an internal UUID-owned network is created; default daemon address
    # pools and existing networks are never changed or reclaimed.
    seed = int.from_bytes(hashlib.sha256(scope.encode()).digest()[:8], 'big')
    for block in ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']:
        pool = ipaddress.ip_network(block)
        count = pool.num_addresses // 256
        for offset in range(count):
            candidate = ipaddress.ip_network((int(pool.network_address) + ((seed + offset) % count) * 256, 24))
            if not any(candidate.overlaps(value) for value in occupied if value.version == candidate.version):
                return str(candidate)
    raise RuntimeError('private_control_subnets_exhausted')


def call(argv, *, data=None, timeout=120, env=None):
    result = subprocess.run(argv, input=data, capture_output=True, timeout=timeout, env=env)
    require(result.returncode == 0, 'command_failed:' + Path(argv[0]).name)
    return result.stdout


GUEST_STDERR_LIMIT = 65536
GUEST_INSPECT_LIMIT = 1024 * 1024


def guest_error_classification(prefix, code):
    """Observed text markers only; these do not establish a failure's root cause."""
    value = prefix.lower()
    for name, markers in [
        ('tls_certificate_marker', (b'x509:', b'certificate signed by unknown authority', b'certificate has expired')),
        ('tls_marker', (b'tls handshake', b'http: server gave http response to https client')),
        ('connection_marker', (b'connection refused', b'connection reset', b'no route to host', b'i/o timeout')),
        ('manifest_marker', (b'manifest unknown', b'no matching manifest', b'manifest invalid')),
        ('blob_marker', (b'blob unknown', b'blob upload invalid')),
        ('digest_marker', (b'digest mismatch', b'digest invalid', b'checksum mismatch')),
        ('authorization_marker', (b'unauthorized', b'authentication required', b'denied:')),
    ]:
        if any(marker in value for marker in markers):
            return name
    return 'clean_exit' if code == 0 else ('running' if code is None else 'unclassified_command_failure')


def guest_diagnostic_directory(root):
    root = Path(root)
    require(os.name == 'posix' and root.is_absolute()
            and not any(parent.is_symlink() for parent in (root, *root.parents)), 'unsafe_guest_diagnostic_root')
    def private_directory(path):
        value = path.lstat()
        require(stat.S_ISDIR(value.st_mode) and value.st_uid == os.geteuid()
                and stat.S_IMODE(value.st_mode) == 0o700, 'unsafe_guest_diagnostic_directory')
    private_directory(root)
    directory = root / 'diagnostics'
    directory.mkdir(mode=0o700, exist_ok=True)
    private_directory(directory)
    return directory


class GuestCommandStream:
    """Drain continuously while retaining bounded private evidence only."""
    def __init__(self, pipe, limit, descriptor=None):
        self.pipe, self.limit, self.descriptor = pipe, limit, descriptor
        self.lock = threading.Lock()
        self.prefix = bytearray()
        self.total = 0
        self.saturated = False
        self.failed = False
        self.thread = threading.Thread(target=self.drain, daemon=True)
        try:
            self.thread.start()
        except BaseException:
            self.pipe.close()
            raise

    def drain(self):
        try:
            while block := self.pipe.read(4096):
                with self.lock:
                    self.saturated |= self.total + len(block) > transport.DIAGNOSTIC_COUNTER_MAX
                    self.total = min(transport.DIAGNOSTIC_COUNTER_MAX, self.total + len(block))
                    retained = block[:max(0, self.limit - len(self.prefix))]
                    self.prefix.extend(retained)
                if retained and self.descriptor is not None and not self.failed:
                    try:
                        view = memoryview(retained)
                        while view:
                            written = os.write(self.descriptor, view)
                            require(written > 0, 'guest_diagnostic_short_write')
                            view = view[written:]
                    except (OSError, RuntimeError):
                        self.failed = True
            if self.descriptor is not None:
                os.fsync(self.descriptor)
        except BaseException:
            self.failed = True
        finally:
            try:
                if self.descriptor is not None:
                    os.close(self.descriptor)
                self.pipe.close()
            except BaseException:
                self.failed = True

    def snapshot(self):
        with self.lock:
            return {'observed_bytes': self.total, 'retained_bytes': len(self.prefix),
                    'retained_sha256': hashlib.sha256(self.prefix).hexdigest(),
                    'truncated': self.total > self.limit, 'counter_saturated': self.saturated,
                    'io_failed': self.failed}


def guest_image_command(root, role, action, argv, registry, publish):
    """No retry; preserve pull=900s/inspect=120s and reap the exact owned child."""
    require(role in transport.REGISTRY_IMAGE_ROLES and action in {'pull', 'inspect'}, 'invalid_guest_image_command')
    expected = ['pull'] if action == 'pull' else ['image', 'inspect']
    require(isinstance(argv, list) and len(argv) > len(expected) + 1
            and argv[-len(expected)-1:-1] == expected, 'invalid_guest_image_command_arguments')
    timeout = 900 if action == 'pull' else 120
    directory = guest_diagnostic_directory(root)
    descriptor = os.open(directory / (role + '-' + action + '.stderr'),
                         os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | getattr(os, 'O_BINARY', 0), 0o600)
    before = registry.diagnostics.snapshot()
    started = time.monotonic()
    observation = {'version': 1, 'image_role': role, 'action': action, 'timeout_seconds': timeout,
        'status': 'prepared', 'exit_code': None, 'termination_signal': None, 'timed_out': False, 'interrupted': False,
        'owned_child_started': False, 'owned_child_reaped': False, 'duration_millis': 0,
        'registry_before': before}
    child, streams, spawn_attempted = None, [], False
    problem = None
    def update(status):
        observation.update(status=status, duration_millis=min(2**63-1, max(0, int((time.monotonic() - started) * 1000))))
        after = registry.diagnostics.snapshot()
        observation.update(registry_after=after, registry_delta=transport.RegistryDiagnostics.delta(before, after))
        if streams:
            observation['stdout'] = streams[0].snapshot()
        if len(streams) == 2:
            observation.update(stderr=streams[1].snapshot(),
                observed_error_classification=guest_error_classification(bytes(streams[1].prefix), child.returncode))
        publish(observation)
    try:
        update('prepared')  # Durable before starting an owned subprocess.
        spawn_attempted = True
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, start_new_session=True)
        observation['owned_child_started'] = True
        streams.append(GuestCommandStream(child.stdout, GUEST_INSPECT_LIMIT if action == 'inspect' else 0))
        streams.append(GuestCommandStream(child.stderr, GUEST_STDERR_LIMIT, descriptor))
        descriptor = None  # The stderr drainer now owns and closes it.
        update('running')
        deadline = started + timeout
        while True:
            if any(stream.failed for stream in streams):
                problem = 'guest_image_diagnostic_io_failed'; break
            if action == 'inspect' and streams[0].total > GUEST_INSPECT_LIMIT:
                problem = 'guest_image_inspect_output_limit'; break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                observation['timed_out'] = True; problem = 'guest_image_' + action + '_timed_out'; break
            try:
                child.wait(timeout=min(.1, remaining)); break
            except subprocess.TimeoutExpired:
                continue
    except BaseException as error:
        observation['interrupted'] = isinstance(error, (KeyboardInterrupt, SystemExit)) or str(error) == 'qualification_interrupted'
        problem = ('guest_image_' + action + '_interrupted' if observation['interrupted'] else
            'guest_image_command_launch_failed' if spawn_attempted and child is None else 'guest_image_command_supervision_failed')
    finally:
        if descriptor is not None:
            try: os.close(descriptor)
            except OSError: problem = 'guest_image_diagnostic_io_failed'
        if child is not None:
            try:
                child.wait(timeout=0)
            except subprocess.TimeoutExpired:
                # Only this unreaped Popen is signaled, never a PID from disk or
                # another process group. Unknown signal/wait states remain held.
                try:
                    child.terminate(); child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    try: child.kill(); child.wait(timeout=2)
                    except BaseException: problem = 'guest_image_child_supervision_pending'
                except BaseException:
                    problem = 'guest_image_child_supervision_pending'
                if problem == 'guest_image_child_supervision_pending':
                    try: update('supervision_pending')
                    except BaseException: pass
            except BaseException:
                problem = 'guest_image_child_supervision_pending'
                try: update('supervision_pending')
                except BaseException: pass
            # A final wait records explicit ownership even after an earlier wait.
            while True:
                try: observation['exit_code'] = child.wait(timeout=.25); break
                except subprocess.TimeoutExpired: continue
                except BaseException:
                    problem = 'guest_image_child_supervision_pending'
            observation['owned_child_reaped'] = True
            observation['termination_signal'] = -child.returncode if child.returncode < 0 else None
            for stream in streams:
                # Do not return while an inherited pipe remains unconfirmed.
                try: stream.thread.join(timeout=2)
                except BaseException: problem = 'guest_image_command_supervision_failed'
                if stream.thread.is_alive():
                    problem = 'guest_image_stream_supervision_pending'
                    try: update('supervision_pending')
                    except BaseException: pass
                while stream.thread.is_alive():
                    try: stream.thread.join(timeout=.25)
                    except BaseException: problem = 'guest_image_command_supervision_failed'
            for pipe in [child.stdout, child.stderr]:
                if not any(stream.pipe is pipe for stream in streams):
                    try: pipe.close()
                    except BaseException: problem = 'guest_image_diagnostic_io_failed'
    if streams and action == 'inspect' and streams[0].total > GUEST_INSPECT_LIMIT:
        problem = problem or 'guest_image_inspect_output_limit'
    if any(stream.failed for stream in streams):
        problem = problem or 'guest_image_diagnostic_io_failed'
    try: update('completed')
    except BaseException: raise RuntimeError('guest_image_diagnostic_report_failed') from None
    require(problem is None, problem or 'guest_image_command_failed')
    require(observation['exit_code'] == 0, 'guest_image_' + action + '_failed')
    return bytes(streams[0].prefix)


def wait(predicate, code, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.5)
    raise RuntimeError(code)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def request(opener, origin, path, token=None, body=None, *, method=None, expected=200, headers=None, raw=False,
            phase_name='api_request', diagnostics=None):
    require(phase_name in PHASES, 'invalid_qualification_phase')
    if isinstance(token, AdminSession):
        token = token.access_token()
    selected = {'content-type': 'application/json', **{key.lower(): value for key, value in (headers or {}).items()}}
    if token:
        selected['authorization'] = 'Bearer ' + token
    payload = body if isinstance(body, bytes) else None if body is None else json.dumps(body).encode()
    req = Request(origin + path, payload, selected, method=method)
    try:
        with opener.open(req, timeout=30) as response:
            status, content = response.status, response.read(4 * 1024 * 1024 + 1)
    except HTTPError as error:
        try:
            status, content = error.code, error.read(4096)
        finally:
            error.close()
    observation = http_observation(phase_name, expected, status, content if status >= 400 else b'')
    if diagnostics is not None:
        diagnostics.http(observation)
    if status != expected:
        raise HttpQualificationFailure(observation)
    require(len(content) <= 4 * 1024 * 1024, 'http_response_exceeds_limit')
    return content if raw else json.loads(content) if content and status < 400 else None


def certificate(root):
    call(['openssl', 'req', '-x509', '-newkey', 'rsa:3072', '-nodes', '-sha256', '-days', '2',
          '-subj', '/CN=Private hosted VM fixture CA', '-addext', 'basicConstraints=critical,CA:TRUE',
          '-keyout', str(root / 'ca-key.pem'), '-out', str(root / 'ca.pem')])
    call(['openssl', 'req', '-newkey', 'rsa:3072', '-nodes', '-sha256',
          '-subj', '/CN=Private hosted VM runtime', '-keyout', str(root / 'server-key.pem'),
          '-out', str(root / 'server.csr')])
    write(root / 'server.ext', 'basicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n'
          'subjectAltName=DNS:localhost,IP:127.0.0.1,IP:10.0.2.2\n')
    call(['openssl', 'x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.pem'),
          '-CAkey', str(root / 'ca-key.pem'), '-CAcreateserial', '-days', '2', '-sha256',
          '-extfile', str(root / 'server.ext'), '-out', str(root / 'server.pem')])
    for path in root.glob('*.pem'):
        path.chmod(0o600)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(root / 'server.pem', root / 'server-key.pem')
    return context


def baseline_records(path):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink() and path.stat().st_size <= 1048576,
            'explicit_container_baseline_required')
    values = json.loads(path.read_text())
    require(isinstance(values, list) and values and all(isinstance(value, dict)
            and re.fullmatch('[0-9a-f]{12,64}', value.get('ID', ''))
            and re.fullmatch('(?:sha256:)?[0-9a-f]{12,64}', value.get('Image', '')) for value in values),
            'invalid_original_container_baseline')
    require(len({value['ID'] for value in values}) == len(values), 'duplicate_original_container_baseline')
    return values


class ParentDocker:
    def __init__(self, client, context, scope, expected_baseline):
        require(context == 'desktop-linux', 'explicit_desktop_linux_context_required')
        require(Path(client).is_absolute() and Path(client).is_file(), 'explicit_parent_docker_exe_required')
        self.argv = [client, '--context', context]
        # Freeze the Windows client's read-only configuration before installing
        # a separate Linux operator CLI/configuration in the process environment.
        allowed = {'PATH', 'HOME', 'USER', 'LOGNAME', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH',
                   'APPDATA', 'LOCALAPPDATA', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC',
                   'TEMP', 'TMP', 'TMPDIR', 'WSL_INTEROP', 'WSLENV', 'DOCKER_CONFIG'}
        self.env = {key: value for key, value in os.environ.items() if key in allowed}
        self.last_action = None
        self.scope = scope
        self.daemon = self.json('info', '--format', '{{json .}}')['ID']
        self.baseline = self.snapshot()
        resolved = self.json('inspect', *[value['ID'] for value in expected_baseline])
        require({value['Id'] for value in resolved} == set(self.baseline), 'exclusive_original_baseline_required')
        for original, value in zip(expected_baseline, resolved):
            require(value['Id'].startswith(original['ID']) and value['Image'].removeprefix('sha256:')
                    .startswith(original['Image'].removeprefix('sha256:')), 'original_container_baseline_changed')
        self.containers, self.networks, self.volumes = set(), set(), set()

    def run(self, *args, data=None, timeout=180):
        return self.binary(*args, data=data, timeout=timeout).decode().strip()

    def binary(self, *args, data=None, timeout=180):
        verb = args[0] if args else 'unknown'
        allowed = {'info', 'inspect', 'ps', 'image', 'volume', 'network', 'create', 'run', 'cp', 'rm', 'stop', 'exec'}
        require(verb in allowed, 'untyped_parent_docker_action')
        detail = args[1] if verb in ['image', 'volume', 'network'] and len(args) > 1 else None
        require(detail is None or detail in ['ls', 'inspect', 'save', 'create', 'rm'], 'untyped_parent_docker_action')
        self.last_action = verb + ('_' + detail if detail else '')
        try:
            return call([*self.argv, *args], data=data, timeout=timeout, env=self.env)
        except (RuntimeError, OSError, subprocess.SubprocessError):
            raise RuntimeError('parent_docker_failed:' + self.last_action) from None

    def json(self, *args):
        return json.loads(self.run(*args))

    def snapshot(self):
        identifiers = self.run('ps', '--quiet', '--no-trunc').splitlines()
        return {value['Id']: value['Image'] for value in self.json('inspect', *identifiers)} if identifiers else {}

    def preserved(self):
        require(self.json('info', '--format', '{{json .}}')['ID'] == self.daemon, 'parent_daemon_changed')
        for value in self.json('inspect', *self.baseline) if self.baseline else []:
            require(value['State']['Running'] and value['Image'] == self.baseline[value['Id']],
                    'original_container_changed')

    def inspect_image(self, identifier):
        value = self.json('image', 'inspect', identifier)[0]
        require(value['Id'] == identifier and value['Os'] == 'linux' and value['Architecture'] == 'amd64',
                'candidate_image_identity_mismatch')
        return {'actual_image_id': value['Id'], 'repository_digests_before_fixture': value.get('RepoDigests') or [],
                'tags_before_fixture': value.get('RepoTags') or []}

    def create_volume(self, suffix):
        name = self.scope + '-' + suffix
        require(not self.run('volume', 'ls', '--quiet', '--filter', 'name=^' + name + '$'), 'volume_collision')
        self.run('volume', 'create', '--label', 'cowork.acceptance=' + self.scope, name)
        self.volumes.add(name)
        return name

    def create_network(self):
        name = self.scope + '-control'
        require(not self.run('network', 'ls', '--quiet', '--filter', 'name=^' + name + '$'), 'network_collision')
        identifiers = self.run('network', 'ls', '--quiet', '--no-trunc').splitlines()
        require(len(identifiers) <= 10000 and len(set(identifiers)) == len(identifiers)
                and all(re.fullmatch('[a-f0-9]{64}', key) for key in identifiers), 'invalid_parent_network_inventory')
        networks = []
        for offset in range(0, len(identifiers), 64):
            networks.extend(self.json('network', 'inspect', *identifiers[offset:offset+64]))
        require(len(networks) == len(identifiers) and {value.get('Id') for value in networks} == set(identifiers),
                'parent_network_inventory_changed')
        self.control_subnet = control_subnet(self.scope, networks)
        self.run('network', 'create', '--internal', '--subnet', self.control_subnet,
                 '--label', 'cowork.acceptance=' + self.scope, name)
        self.networks.add(name)
        return name

    def run_container(self, suffix, image, arguments, *, transient=False, data=None):
        self.preserved()
        name = self.scope + '-' + suffix
        require(not self.run('ps', '--all', '--quiet', '--filter', 'name=^/' + name + '$'), 'container_collision')
        common = ['--pull', 'never', '--name', name, '--label', 'cowork.acceptance=' + self.scope,
                  '--cpus', '1', '--memory', '512m', '--pids-limit', '128', '--cap-drop', 'ALL',
                  '--security-opt', 'no-new-privileges:true']
        if transient:
            self.run('run', '--rm', '--interactive', *common, *arguments, image, data=data)
            return None
        identifier = self.run('run', '--detach', *common, *arguments, image)
        self.containers.add(identifier)
        return identifier

    def seed(self, volume, core, values):
        require(volume in self.volumes and isinstance(values, dict) and 0 < len(values) <= 32
                and all(isinstance(key, str) and re.fullmatch('[a-z][a-z0-9_]{0,39}', key)
                        and isinstance(value, bytes) and len(value) <= 16384 for key, value in values.items()),
                'invalid_seed_scope')
        program = "import json,os,sys;os.chmod('/out',0o755);v=json.load(sys.stdin);" \
            "[(open('/out/'+k,'xb').write(bytes.fromhex(s)),os.chmod('/out/'+k,0o444)) for k,s in v.items()]"
        suffix = 'seed-' + uuid.uuid4().hex
        self.preserved()
        name = self.scope + '-' + suffix
        self.run('run', '--rm', '--interactive', '--pull', 'never', '--name', name,
            '--label', 'cowork.acceptance=' + self.scope, '--network', 'none', '--read-only',
            '--user', '0', '--cpus', '.25', '--memory', '128m', '--pids-limit', '32', '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges:true', '--volume', volume + ':/out',
            '--entrypoint', 'python3', core, '-c', program,
            data=json.dumps({key: value.hex() for key, value in values.items()}).encode())

    def extraction(self, provisioner_id, root):
        self.preserved()
        name = self.scope + '-extract'
        identifier = self.run('create', '--name', name, '--label', 'cowork.acceptance=' + self.scope,
            '--network', 'none', '--read-only', '--cpus', '.25', '--memory', '128m', '--pids-limit', '32',
            '--entrypoint', 'sleep', provisioner_id, '1')
        self.containers.add(identifier)
        require(self.json('inspect', identifier)[0]['Image'] == provisioner_id, 'extraction_image_changed')
        package, tools = root / 'operator-package', root / 'tools'
        package.mkdir(mode=0o700); tools.mkdir(mode=0o700)
        hashes = {}
        for source, target, package_mode in [('/opt/cowork-provisioner/.', package, True),
                ('/usr/local/bin/docker', tools / 'docker', False),
                ('/usr/local/bin/mc', tools / 'mc', False),
                ('/usr/local/libexec/docker/cli-plugins/docker-compose', tools / 'docker-compose', False)]:
            payload = self.binary('cp', identifier + ':' + source, '-')
            require(len(payload) <= 128 * 1024 * 1024, 'candidate_extraction_too_large')
            with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                files = [member for member in archive.getmembers() if not member.isdir()]
                require(len(files) <= 100 and all(member.isfile() for member in files), 'unsafe_candidate_package')
                if not package_mode:
                    require(len(files) == 1, 'candidate_binary_inventory_mismatch')
                for member in files:
                    name = Path(member.name).name
                    require(member.size <= 100 * 1024 * 1024 and
                            (not package_mode or re.fullmatch(r'[a-z_]+\.py', name)), 'unsafe_candidate_member')
                    destination = target / name if package_mode else target
                    write(destination, archive.extractfile(member).read(), 0o600 if package_mode else 0o700)
                    hashes[destination.name] = transport.hash_file(destination)
        require((package / 'guest_endpoints.py').is_file() and (package / 'remote_entrypoint.py').is_file(),
                'candidate_guest_profile_missing')
        self.remove_container(identifier)
        return package, tools, hashes

    def server_binary_hash(self, image, role):
        require(role in ['source', 'target'], 'invalid_binary_extraction_role')
        self.preserved()
        identifier = self.run('create', '--name', self.scope + '-binary-' + role,
            '--label', 'cowork.acceptance=' + self.scope, '--network', 'none', '--read-only',
            '--cpus', '.1', '--memory', '128m', '--pids-limit', '32', '--entrypoint', 'sleep', image, '1')
        self.containers.add(identifier)
        require(self.json('inspect', identifier)[0]['Image'] == image, 'binary_extraction_image_changed')
        try:
            payload = self.binary('cp', identifier + ':/usr/local/bin/cowork-server', '-')
            require(len(payload) <= 128 * 1024**2, 'binary_extraction_exceeds_bound')
            with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                entries = archive.getmembers()
                require(len(entries) == 1 and entries[0].isfile() and entries[0].size <= 120 * 1024**2,
                        'binary_extraction_inventory_mismatch')
                stream = archive.extractfile(entries[0])
                require(stream.read(4) == b'\x7fELF', 'candidate_server_not_elf'); stream.seek(0)
                return hashlib.file_digest(stream, 'sha256').hexdigest()
        finally:
            self.remove_container(identifier)

    def remove_container(self, identifier):
        require(identifier in self.containers and identifier not in self.baseline, 'foreign_container_cleanup_refused')
        value = self.json('inspect', identifier)[0]
        require(value['Config']['Labels'].get('cowork.acceptance') == self.scope
                and value['Name'].startswith('/' + self.scope + '-'), 'container_cleanup_scope_mismatch')
        self.run('rm', '--force', identifier)
        self.containers.remove(identifier)

    def cleanup(self):
        self.preserved()
        for identifier in list(self.containers):
            self.remove_container(identifier)
        for name in list(self.networks):
            value = self.json('network', 'inspect', name)[0]
            require(value['Name'] == name and value['Labels'].get('cowork.acceptance') == self.scope
                    and not value['Containers'], 'network_cleanup_scope_mismatch')
            self.run('network', 'rm', value['Id']); self.networks.remove(name)
        for name in list(self.volumes):
            value = self.json('volume', 'inspect', name)[0]
            require(value['Name'] == name and value['Labels'].get('cowork.acceptance') == self.scope,
                    'volume_cleanup_scope_mismatch')
            self.run('volume', 'rm', name); self.volumes.remove(name)
        require(not self.run('ps', '--all', '--quiet', '--filter', 'label=cowork.acceptance=' + self.scope),
                'fixture_container_retained')
        self.preserved()


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def ssh_arguments(vm, root):
    value = vm.load(root)
    return ['ssh', '-i', str(root / 'ssh-key'), '-p', str(value['ssh_port']),
            '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'UserKnownHostsFile=' + str(root / 'known_hosts'), '-o', 'ConnectTimeout=5']


def guest_command(vm, root, program, *, data=None, timeout=90):
    require(isinstance(program, str) and len(program) < 32768, 'invalid_guest_program')
    # Fixed Python interpreter + base64 encoding; data never enters remote shell text.
    command = "python3 -c 'import base64;exec(base64.b64decode(\"" + base64.b64encode(program.encode()).decode() + "\"))'"
    return call([*ssh_arguments(vm, root), 'cowork@127.0.0.1', command], data=data, timeout=timeout)


def bound_vm_processes(vm):
    """Own the fresh QEMU child and retain bounded private error evidence."""
    from hosted_qemu_process import QemuChildren
    children = QemuChildren()
    original, original_process, original_stop = vm.call, vm.process, vm.stop
    def bounded(argv, **kwargs):
        if Path(argv[0]).name == 'qemu-system-x86_64':
            return children.launch(argv, **kwargs)
        return original(argv, **kwargs)
    vm.call = bounded
    def owned_process(root):
        evidence = children.evidence(root)
        if evidence is not None and evidence['exit_code'] is not None:
            children.reap_after_stop(root)
            return None
        result = original_process(root)
        require(evidence is None or result == evidence['owned_child_pid'], 'owned_qemu_process_identity_mismatch')
        return result
    vm.process = owned_process
    def stopped(root):
        try:
            original_stop(root)
        finally:
            # wait() reaps only this exact Popen child, never an inherited PID.
            if children.evidence(root) is not None and vm.process(root) is None:
                children.reap_after_stop(root)
    vm.stop = stopped
    vm.qualification_vm_evidence = children.evidence
    vm.qualification_require_running = children.require_running
    def private_qmp(owned_root, command):
        metadata = owned_root.lstat()
        require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid()
                and stat.S_IMODE(metadata.st_mode) == 0o700,
                'invalid_owned_qmp_directory')
        require(vm.process(owned_root) is not None, 'vm_not_running')
        require(command in {'query-kvm', 'system_powerdown', 'quit'}, 'unexpected_fixture_qmp_command')
        directory_fd = os.open(owned_root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.settimeout(10)
                sock.connect('/proc/self/fd/' + str(directory_fd) + '/qmp.sock')
                stream = sock.makefile('rwb')
                greeting = stream.readline(65537)
                require(len(greeting) <= 65536 and greeting.endswith(b'\n'), 'invalid_qmp_greeting')
                json.loads(greeting)
                for item in ('qmp_capabilities', command):
                    stream.write((json.dumps({'execute': item}) + '\n').encode()); stream.flush()
                    for _ in range(64):
                        line = stream.readline(65537)
                        require(len(line) <= 65536 and line.endswith(b'\n'), 'invalid_qmp_reply')
                        result = json.loads(line)
                        require('error' not in result, 'qmp_command_failed')
                        if 'return' in result:
                            break
                    else:
                        raise RuntimeError('qmp_reply_limit_exceeded')
                return result['return']
        finally:
            os.close(directory_fd)
    vm.qmp = private_qmp


def prepare_parent_pool(vm, args, report):
    """Explicit read-only pilot metadata adoption; no pilot keys or process reads."""
    capacity = vm.capacity_pool
    plan = capacity.read_private_json(args.parent_capacity_plan_file)
    capacity.validate_plan(plan)
    pilot = capacity.canonical_path(args.retained_pilot_root)
    require(str(uuid.UUID(args.retained_pilot_vm_id)) == args.retained_pilot_vm_id
            and pilot.name == 'cowork-executor-' + uuid.UUID(args.retained_pilot_vm_id).hex
            and plan['existing_vm_roots'] == [str(pilot)], 'explicit_parent_pilot_scope_mismatch')
    vm.private_directory(pilot, 'unsafe_retained_pilot_root')
    metadata = pilot / 'vm.json'; vm.private_file(metadata, 'unsafe_retained_pilot_metadata')
    require(metadata.stat().st_size <= 262144, 'retained_pilot_metadata_exceeds_bound')
    before = transport.hash_file(metadata)
    value = vm.load(pilot, allow_preparing=False)
    require(value['id'] == args.retained_pilot_vm_id, 'retained_pilot_identity_mismatch')
    with capacity.locked_pool(allow_unconfigured=True) as pool:
        revision = pool.data['revision'] if pool.data else 0
        require(revision == args.expected_parent_capacity_revision, 'parent_capacity_revision_conflict')
        if pool.data is None or pool.data['plan'] != plan:
            pool.configure(plan, revision)
        with vm.inventory_lock(pilot.parent):
            pool.reserve(pilot, value, adopted=True)
        require(before == transport.hash_file(metadata), 'retained_pilot_metadata_changed')
        report['parent_capacity'] = {'pool_id': plan['pool_id'], 'revision': pool.data['revision'],
            'plan_sha256': capacity.ledger_digest(plan), 'held': pool.totals(), 'pilot_vm_id': value['id'],
            'pilot_metadata_sha256': before, 'pilot_metadata_unchanged': True,
            'pilot_started': False, 'physical_scope': 'explicit Linux account allocation plan'}


def validate_profile_arguments(args):
    require(args.schema_version >= 54, 'host_assignment_and_waiting_migrations_required')
    if args.qualification_profile == 'host55-lifecycle':
        require(args.schema_version >= 56 and args.source_schema_version is not None
                and 54 <= args.source_schema_version < args.schema_version
                and args.source_server_image_id is not None and args.source_runner_image_id is not None
                and args.source_server_image_id != args.server_image_id,
                'private_ca_capable_source_and_different_target_required')
    require(args.parent_capacity_plan_file is not None and args.parent_capacity_plan_file.is_absolute()
            and args.retained_pilot_root is not None and args.retained_pilot_root.is_absolute()
            and args.retained_pilot_vm_id is not None and args.retained_pilot_known_stopped
            and type(args.expected_parent_capacity_revision) is int and args.expected_parent_capacity_revision >= 0,
            'explicit_parent_capacity_and_stopped_pilot_required')


def execute(args):
    require(os.name == 'posix' and resource is not None and os.geteuid() != 0, 'unprivileged_linux_required')
    tools_required = ['openssl', 'ssh', 'qemu-system-x86_64', 'qemu-img', 'cloud-localds', 'psql', 'prlimit']
    if args.qualification_profile == 'host55-lifecycle':
        tools_required.extend(['age-keygen', 'age', 'pg_dump', 'pg_restore'])
    for name in tools_required:
        require(shutil.which(name), 'required_tool_missing:' + name)
    require(Path(args.report).is_absolute() and not Path(args.report).exists(), 'new_absolute_report_required')
    require(shutil.disk_usage(Path.home()).free >= 60 * 1024**3, 'fixture_requires_60gib_free')
    # Native operator/transport processes inherit bounded address space, files and
    # descriptors. Docker services and the VM have separate explicit caps.
    resource.setrlimit(resource.RLIMIT_AS, (1536 * 1024**2, 8192 * 1024**2))
    resource.setrlimit(resource.RLIMIT_NOFILE, (512, 512))
    resource.setrlimit(resource.RLIMIT_FSIZE, (transport.MAX_ARCHIVE_BYTES, transport.MAX_ARCHIVE_BYTES))
    resource.setrlimit(resource.RLIMIT_CPU, (1800, 5400))
    scope = 'cowork-hosted-vm-' + uuid.uuid4().hex
    root = private_fixture_root(scope)
    images = {name: getattr(args, name + '_image_id') for name in
              ['server', 'runner', 'provisioner', 'postgres', 'storage', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']}
    full = args.qualification_profile == 'host55-lifecycle'
    if full:
        images.update(source_server=args.source_server_image_id, source_runner=args.source_runner_image_id)
    parent = None
    try:
        parent = ParentDocker(args.parent_docker_client, args.parent_docker_context, scope, baseline_records(args.baseline_file))
        evidence = {name: parent.inspect_image(identifier) for name, identifier in images.items()}
    except BaseException as error:
        initial = {'version': 1, 'scope': scope, 'passed': False, 'checks': [], 'phase': 'parent_inventory',
                   'failure_code': failure_code(error),
                   'harness_sha256': transport.hash_file(Path(__file__).resolve()),
                   'transport_sha256': transport.hash_file(Path(transport.__file__).resolve()),
                   'requested_image_ids': images, 'private_fixture_retained': True, 'vm_started': False,
                   'retained_parent_inventory': {'containers': [], 'networks': [], 'volumes': []}}
        if parent is not None:
            initial.update(last_parent_action=parent.last_action, live_containers_preserved=len(parent.baseline))
        durable_report(args.report, initial)
        write(root / 'failure.json', json.dumps(initial, indent=2))
        raise RuntimeError('hosted_vm_qualification_failed') from None
    report = {'version': 1, 'scope': scope, 'passed': False, 'checks': [], 'phase': 'candidate_package',
              'harness_sha256': transport.hash_file(Path(__file__).resolve()),
              'transport_sha256': transport.hash_file(Path(transport.__file__).resolve()),
              'actual_images': evidence, 'schema_version': args.schema_version,
              'qualification_profile': args.qualification_profile,
              'baseline_container_images': parent.baseline, 'parent_daemon_id': parent.daemon,
              'live_fleet_activation': False, 'same_binary_repackage': False,
              'qualification_scope': ['management host assignment', 'scoped provision/start/login/rawrun/stop/delete',
                                      'one dedicated local KVM guest', 'private CA transports', 'guest AppArmor enforcement'],
              'not_qualified': ['remote backup/restore/update/rollback/enrollment', 'cross-version migration',
                                'production fleet migration', 'disk quotas', 'automatic certificate renewal'],
              'operator_execution_mode': 'native Linux package and tools extracted from exact candidate image',
              'transport_caps': {'native_address_space_mib': 1536, 'http_connections_per_listener': 8,
                                 'registry_connections': 4, 'postgres_connections_per_listener': 64,
                                 'http_body_bytes': transport.MAX_HTTP_BYTES, 'native_cpu_seconds': 1800,
                                 'native_file_descriptors': 512, 'native_address_space_hard_mib': 8192,
                                 'qemu_address_space_mib': 8192, 'qemu_cpu_seconds': 5400},
              'guest_image_diagnostic_caps': {'stderr_retained_bytes_per_command': GUEST_STDERR_LIMIT,
                  'inspect_stdout_retained_bytes': GUEST_INSPECT_LIMIT, 'command_records': 2 * len(transport.REGISTRY_IMAGE_ROLES),
                  'registry_counter_keys': (len(transport.REGISTRY_IMAGE_ROLES) + 2) * 2 * 4 * 2,
                  'counter_max': transport.DIAGNOSTIC_COUNTER_MAX},
              'private_manifest_overrides': [], 'fixture_resources_removed': False}
    write(root / 'fixture.json', json.dumps({'scope': scope, 'root': str(root), 'daemon_id': parent.daemon,
          'baseline_container_images': parent.baseline, 'actual_images': images}, indent=2))
    print('fixture_scope=' + scope, flush=True)
    vm_root, vm, forward = None, None, None
    diagnostics = PublicDiagnostics()
    def fixture_request(*arguments, **keywords):
        keywords.setdefault('phase_name', report['phase'])
        keywords.setdefault('diagnostics', diagnostics)
        return request(*arguments, **keywords)
    def check(name):
        report['checks'].append(name); print(name + '=ok', flush=True)
    try:
        package, tools, package_hashes = parent.extraction(images['provisioner'], root)
        report['operator_package_sha256'] = package_hashes
        private_docker = root / 'docker-config'; (private_docker / 'cli-plugins').mkdir(parents=True, mode=0o700)
        shutil.copyfile(tools / 'docker-compose', private_docker / 'cli-plugins/docker-compose')
        (private_docker / 'cli-plugins/docker-compose').chmod(0o700)
        os.environ.update({'PATH': str(tools) + ':' + os.environ['PATH'], 'DOCKER_CONFIG': str(private_docker)})
        for key in ['DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH', 'DOCKER_API_VERSION']:
            os.environ.pop(key, None)
        sys.path.insert(0, str(package))
        import provisioner as p
        import remote_entrypoint
        from operation_journal import OperationJournal
        require(Path(p.__file__).resolve().parent == package, 'operator_candidate_import_mismatch')
        check('candidate_package_extracted_without_running_operator_image')
        tls = certificate(root)
        client_context = ssl.create_default_context(cafile=root / 'ca.pem')
        opener = build_opener(ProxyHandler({}), NoRedirect(), HTTPSHandler(context=client_context))
        registry = transport.Registry(root / 'registry', scope)
        refs = {}
        for name in ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']:
            archive = root / (name + '.tar')
            with archive.open('xb') as output:
                parent.last_action = 'image_save'
                result = subprocess.run([*parent.argv, 'image', 'save', images[name]], stdout=output,
                                        stderr=subprocess.DEVNULL, timeout=900, env=parent.env)
            require(result.returncode == 0, 'parent_docker_failed:image_save')
            refs[name] = registry.import_archive(archive, images[name], name)
            archive.unlink()
        if full:
            report['server_binary_sha256'] = {'source': parent.server_binary_hash(images['source_server'], 'source'),
                'target': parent.server_binary_hash(images['server'], 'target')}
            require(report['server_binary_sha256']['source'] != report['server_binary_sha256']['target'],
                    'cross_binary_transition_required')
            for name in ['source_server', 'source_runner']:
                archive = root / (name + '.tar')
                with archive.open('xb') as output:
                    result = subprocess.run([*parent.argv, 'image', 'save', images[name]], stdout=output,
                        stderr=subprocess.DEVNULL, timeout=900, env=parent.env)
                require(result.returncode == 0, 'parent_docker_failed:image_save')
                refs[name] = registry.import_archive(archive, images[name], name); archive.unlink()
        network = parent.create_network()
        report['private_control_subnet'] = parent.control_subnet
        values = {name: secrets.token_hex(32).encode() for name in
                  ['management_password', 'fleet_password', 'bootstrap', 'broker', 's3_password']}
        values['s3_user'] = ('fixture_' + uuid.uuid4().hex[:12]).encode()
        values['auth_key'] = base64.b64encode(secrets.token_bytes(32)); values['audit_key'] = secrets.token_bytes(32)
        values['management_url'] = b'postgres://fixture:' + values['management_password'] + b'@' + (scope + '-management-db:5432/fixture').encode()
        values['ceiling'] = json.dumps({'revision': 1, 'platform_concurrent_runs': 1,
                                       'organisation_concurrent_runs': 1, 'user_concurrent_runs': 1}).encode()
        def control(suffix, selected_image, *, env=None, command=None, secret_values=None, data_path=None, publish=None):
            arguments = ['--network', network, '--read-only', '--tmpfs', '/tmp']
            if env:
                for key, value in env.items():
                    arguments += ['--env', key + '=' + value]
            if secret_values:
                volume = parent.create_volume(suffix + '-secrets'); parent.seed(volume, images['sandbox_core'], secret_values)
                arguments += ['--volume', volume + ':/run/secrets:ro']
            if data_path:
                volume = parent.create_volume(suffix + '-data'); arguments += ['--volume', volume + ':' + data_path]
                if suffix in ['management-db', 'fleet-db']:
                    arguments += ['--tmpfs', '/var/run/postgresql:uid=999,gid=999', '--cap-add', 'CHOWN',
                                  '--cap-add', 'FOWNER', '--cap-add', 'DAC_OVERRIDE', '--cap-add', 'SETUID', '--cap-add', 'SETGID']
            if publish:
                arguments += ['--publish', '127.0.0.1::' + str(publish)]
            if command:
                arguments += ['--entrypoint', command[0]]
            identifier = parent.run_container(suffix, selected_image, arguments)
            # Passing a command after the image is intentionally separate from
            # environment/volume flags; fixed roles below use image defaults.
            return identifier
        management_pg = control('management-db', images['postgres'], env={'POSTGRES_USER': 'fixture',
            'POSTGRES_DB': 'fixture', 'POSTGRES_PASSWORD_FILE': '/run/secrets/password'},
            secret_values={'password': values['management_password']}, data_path='/var/lib/postgresql/data')
        fleet_pg = control('fleet-db', images['postgres'], env={'POSTGRES_USER': 'fixture',
            'POSTGRES_DB': 'fixture', 'POSTGRES_PASSWORD_FILE': '/run/secrets/password'},
            secret_values={'password': values['fleet_password']}, data_path='/var/lib/postgresql/data')
        # MinIO's existing candidate entrypoint is a shell wrapper around minio;
        # give the explicit fixed server argv through its dedicated container.
        storage_secrets = parent.create_volume('storage-secrets')
        parent.seed(storage_secrets, images['sandbox_core'], {'s3_user': values['s3_user'], 's3_password': values['s3_password']})
        storage_data = parent.create_volume('storage-data')
        storage = parent.run('run', '--detach', '--pull', 'never', '--name', scope + '-storage',
            '--label', 'cowork.acceptance=' + scope, '--network', network, '--cpus', '1', '--memory', '512m',
            '--pids-limit', '128', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
            '--volume', storage_secrets + ':/run/secrets:ro', '--volume', storage_data + ':/data',
            '--env', 'MINIO_ROOT_USER_FILE=/run/secrets/s3_user', '--env', 'MINIO_ROOT_PASSWORD_FILE=/run/secrets/s3_password',
            images['storage'], 'server', '/data')
        parent.containers.add(storage)
        def pg_ready(identifier):
            parent.last_action = 'exec_pg_isready'
            try:
                return subprocess.run([*parent.argv, 'exec', identifier, 'pg_isready', '-U', 'fixture', '-d', 'fixture'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=parent.env, timeout=10).returncode == 0
            except (OSError, subprocess.SubprocessError):
                return False
        for identifier in [management_pg, fleet_pg]:
            wait(lambda ident=identifier: pg_ready(ident), 'fixture_postgres_not_ready')
        phase(report, 'private_control_services')
        manager_env = {'COWORK_MODE': 'management', 'COWORK_LISTEN_ADDR': '0.0.0.0:8080',
            'DATABASE_URL_FILE': '/run/secrets/management_url', 'COWORK_BOOTSTRAP_TOKEN_FILE': '/run/secrets/bootstrap',
            'COWORK_AUTH_MASTER_KEY_FILE': '/run/secrets/auth_key', 'COWORK_MANAGEMENT_AUDIT_KEY_FILE': '/run/secrets/audit_key',
            'COWORK_PROVISIONER_TOKEN_FILE': '/run/secrets/broker', 'COWORK_MANAGEMENT_EXECUTION_CEILING_FILE': '/run/secrets/ceiling',
            'COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED': 'true', 'COWORK_MANAGEMENT_PROVISIONING_ENABLED': 'true',
            'COWORK_MANAGEMENT_DELETION_ENABLED': 'true', 'COWORK_MANAGEMENT_BACKUPS_ENABLED': str(full).lower(),
            'COWORK_MANAGEMENT_UPDATES_ENABLED': str(full).lower(), 'COWORK_EXECUTION_ENROLLMENT_ENABLED': 'false',
            'COWORK_WEB_PUSH_ENABLED': 'false', 'COWORK_INSTANCE_URL_TEMPLATE': 'http://{instance}.localhost'}
        manager = control('management', images['server'], env=manager_env, secret_values={key: values[key] for key in
            ['management_url', 'bootstrap', 'auth_key', 'audit_key', 'broker', 'ceiling']})
        # A bounded, idle trusted relay. No socket, admin file or host directory
        # is mounted in it; only this fresh private Docker network is attached.
        relay = parent.run('run', '--detach', '--pull', 'never', '--name', scope + '-relay',
            '--label', 'cowork.acceptance=' + scope, '--network', network, '--read-only', '--tmpfs', '/tmp',
            '--user', '10001:10001', '--cpus', '.5', '--memory', '256m', '--pids-limit', '192', '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges:true', '--entrypoint', 'python3', images['sandbox_core'], '-c',
            'import time;time.sleep(3600)')
        parent.containers.add(relay)
        def connector(suffix, port):
            require(suffix in ['management', 'management-db', 'fleet-db', 'storage'] and port in [8080, 5432, 9000],
                    'invalid_typed_relay_destination')
            require(relay in parent.containers, 'unowned_relay')
            return lambda: transport.PipeSocket([*parent.argv, 'exec', '--interactive', relay, 'python3', '-u', '-c',
                                                 RELAY, scope + '-' + suffix, str(port)], env=parent.env)
        with ExitStack() as stack:
            registry_port = stack.enter_context(transport.serving(registry.server(tls)))
            admin_port = stack.enter_context(transport.serving(transport.proxy(tls, 8080, 'admin', connector=connector('management', 8080))))
            manager_port = stack.enter_context(transport.serving(transport.proxy(tls, 8080, 'manager', connector=connector('management', 8080))))
            pg_admin = stack.enter_context(transport.serving(transport.PostgreSQLProxy(tls, 5432, connector('management-db', 5432))))
            pg_fleet = stack.enter_context(transport.serving(transport.PostgreSQLProxy(tls, 5432, connector('fleet-db', 5432))))
            s3_admin = stack.enter_context(transport.serving(transport.proxy(tls, 9000, 'storage_admin', connector=connector('storage', 9000))))
            management_origin = 'https://localhost:' + str(admin_port)
            def ready():
                try:
                    fixture_request(opener, management_origin, '/readyz'); return True
                except (OSError, RuntimeError):
                    return False
            phase(report, 'management_ready')
            wait(ready, 'fixture_management_not_ready', 180)
            phase(report, 'bootstrap_admin')
            admin = fixture_request(opener, management_origin, '/api/v1/auth/bootstrap', values['bootstrap'].decode(),
                {'email': scope + '@fixture.invalid', 'display_name': 'Private hosted VM admin',
                 'password': secrets.token_urlsafe(32), 'device_id': identity()}, expected=201)
            admin_session = AdminSession(opener, management_origin, admin, diagnostics)
            phase(report, 'totp_setup')
            setup = fixture_request(opener, management_origin, '/api/v1/auth/totp/setup', admin_session, {})
            digest = hmac.new(base64.b32decode(setup['secret'] + '=' * ((8 - len(setup['secret']) % 8) % 8)),
                              (int(time.time()) // 30).to_bytes(8, 'big'), hashlib.sha1).digest()
            offset = digest[-1] & 15
            code = f'{(int.from_bytes(digest[offset:offset+4], "big") & 0x7fffffff) % 1000000:06d}'
            phase(report, 'totp_enable')
            fixture_request(opener, management_origin, '/api/v1/auth/totp/enable', admin_session, {'code': code})
            phase(report, 'create_organisation')
            org = fixture_request(opener, management_origin, '/management/v1/organisations', admin_session, {'name': scope})['id']
            base = '/management/v1/organisations/' + org
            phase(report, 'organisation_policy')
            policy = next(row['policy'] for row in fixture_request(opener, management_origin,
                '/management/v1/organisations', admin_session)['items'] if row['id'] == org)
            policy['revision'] += 1
            policy['limits'].update({'cpu_millis': 500, 'memory_mib': 512, 'storage_gib': 1, 'concurrent_runs': 1})
            policy['allowed_network_origins'] = []
            fixture_request(opener, management_origin, base + '/policy', admin_session, policy, method='PUT')
            check('fresh_bootstrap_totp_org_and_narrow_policy')
            repo = Path(__file__).resolve().parents[2]
            vm = module(repo / 'deploy/executor-vm/local_executor_vm.py', 'hosted_vm_controller')
            bound_vm_processes(vm)
            phase(report, 'parent_capacity')
            prepare_parent_pool(vm, args, report)
            check('canonical_parent_capacity_and_readonly_pilot_metadata_adoption')
            vm_parent = root / 'vms'; vm_parent.mkdir(mode=0o700)
            phase(report, 'guest_create')
            vm_root = vm.create(vm_parent, org)
            vm_value = vm.load(vm_root)
            require(vm_value['version'] == 2 and vm_value['organisation_id'] == org, 'new_vm_v2_required')
            report.update({'vm_id': vm_value['id'], 'organisation_id': org, 'operator_state_id': vm_value['operator_state_id'],
                           'ubuntu_image_sha256': vm_value['ubuntu_image_sha256'], 'vm_caps': {'cpus': 4, 'memory_mib': 4096}})
            vm.start(vm_root)
            report['vm_started'] = True
            report['qemu_process'] = vm.qualification_vm_evidence(vm_root)
            def guest_ready():
                vm.qualification_require_running(vm_root)
                try:
                    return vm.ssh(vm_root, 'sudo cloud-init status --wait >/dev/null && sudo docker info --format "{{.ID}}"', timeout=10).strip()
                except Exception:
                    return False
            phase(report, 'guest_bootstrap')
            wait(guest_ready, 'new_guest_bootstrap_not_ready', 900)
            phase(report, 'guest_apparmor')
            profile = (repo / 'deploy/security/open-cowork-sandbox.apparmor').read_bytes()
            install = "import pathlib,subprocess,sys;v=sys.stdin.buffer.read(262145);assert len(v)<=262144;" \
                "p=pathlib.Path('/tmp/cowork-profile');p.write_bytes(v);" \
                "subprocess.run(['sudo','install','-m','0644',str(p),'/etc/apparmor.d/open-cowork-sandbox'],check=True);" \
                "subprocess.run(['sudo','apparmor_parser','-r','/etc/apparmor.d/open-cowork-sandbox'],check=True);p.unlink()"
            guest_command(vm, vm_root, install, data=profile)
            enforcement = vm.ssh(vm_root, 'sudo cat /sys/kernel/security/apparmor/profiles').decode()
            require('open-cowork-sandbox (enforce)' in enforcement, 'guest_apparmor_not_enforced')
            check('new_guest_own_kernel_and_enforced_apparmor')
            registry_address = '10.0.2.2:' + str(registry_port)
            ca_install = "import pathlib,subprocess,sys;v=sys.stdin.buffer.read(262145);assert b'PRIVATE KEY' not in v;" \
                "p=pathlib.Path('/tmp/cowork-registry-ca');p.write_bytes(v);" \
                "subprocess.run(['sudo','mkdir','-p','/etc/docker/certs.d/" + registry_address + "'],check=True);" \
                "subprocess.run(['sudo','install','-m','0644',str(p),'/etc/docker/certs.d/" + registry_address + "/ca.crt'],check=True);p.unlink()"
            phase(report, 'guest_registry_ca')
            guest_command(vm, vm_root, ca_install, data=(root / 'ca.pem').read_bytes())
            image_refs = {name: registry_address + '/' + repository + '@' + digest for name, (repository, digest) in refs.items()}
            guest_cli = [str(tools / 'docker'), *vm.docker_tls_arguments(vm_root, vm_value)[1:]]
            phase(report, 'guest_docker_info')
            daemon_info = json.loads(call([*guest_cli, 'info', '--format', '{{json .}}']))
            host_id = identity()
            binding = {'version': 1, 'host_id': host_id, 'trust_domain': org, 'daemon_id': daemon_info['ID'],
                       'operator_state_id': vm_value['operator_state_id']}
            # QEMU's assigned RAM includes kernel overhead; the Docker allocation
            # ceiling is deliberately below the guest's physical 4096 MiB.
            capacity = {'cpu_millis': 4000, 'memory_mib': 3584, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024}
            service_plan = p.resource_budget.service_caps({})
            phase(report, 'host_register')
            registered = fixture_request(opener, management_origin, base + '/executor-hosts', admin_session,
                {'id': host_id, 'name': scope, 'daemon_id': daemon_info['ID'], 'operator_state_id': vm_value['operator_state_id'],
                 'capacity': capacity, **({'service_plan': service_plan} if full else {})})
            accept_host(report, args.report, registered, binding)
            phase(report, 'host_credential')
            credential = fixture_request(opener, management_origin, base + '/executor-hosts/' + host_id + '/credentials', admin_session, {})
            accept_credential(report, args.report, credential, binding)
            # Before an allocation exists the operator uses only real host state,
            # host-scoped credentials and TLS identity. No instance UUID is minted
            # locally to manufacture challenge/coverage evidence.
            state = vm_root / 'shared/instances'; state.mkdir(mode=0o700)
            write(root / 'host_token', credential['token'].encode())
            host_context = {'backend': 'compose', 'state_directory': str(state),
                'management_url': management_origin, 'management_ca_file': str(root / 'ca.pem'),
                'provisioner_token_file': str(root / 'host_token'), 'fleet_host_budget': capacity,
                'executor_host': {'id': host_id, 'trust_domain': org, 'operator_state_id': vm_value['operator_state_id'],
                    'endpoint': vm_value['docker_endpoint'], 'daemon_id': daemon_info['ID'],
                    'ca_file': str(vm_root / 'ca.pem'), 'cert_file': str(vm_root / 'client.pem'),
                    'key_file': str(vm_root / 'client-key.pem')}}
            attestor = None
            if full:
                host_context.update(instance_service_limits=service_plan, execution_admission={'version': 1},
                    host_capacity_attestation={'version': 1, 'host_revision': registered['revision']})
                phase(report, 'host_capacity_start')
                _, _, attestor = start_capacity_observation(p, host_context, stack, diagnostics)
                report['host_observation_started_before_instance'] = True
                report.update(diagnostics.snapshot())
                durable_report(args.report, report)
            phase(report, 'instance_create')
            created = fixture_request(opener, management_origin, base + '/executor-hosts/' + host_id + '/instances', admin_session,
                              {'name': scope + ' tenant', 'backend': 'compose'})
            report['instance_id'] = public_identity(created.get('id'), 'instance_registration_scope_mismatch')
            report.update(diagnostics.snapshot())
            durable_report(args.report, report)
            phase(report, 'guest_image_pull')
            def publish_guest_command(observation):
                diagnostics.guest_command(observation)
                report.update(diagnostics.snapshot())
                durable_report(args.report, report)
            for name, reference in image_refs.items():
                guest_image_command(root, name, 'pull', [*guest_cli, 'pull', reference], registry, publish_guest_command)
                observed = json.loads(guest_image_command(root, name, 'inspect',
                    [*guest_cli, 'image', 'inspect', reference], registry, publish_guest_command))[0]
                package_identity = registry.packages[name]
                require(observed['Id'] in [images[name], package_identity['config_digest']]
                        and observed['RootFS']['Layers'] == package_identity['rootfs_diff_ids']
                        and reference in observed['RepoDigests'], 'guest_registry_digest_identity_mismatch')
                evidence[name]['fixture_registry_reference'] = reference
                evidence[name]['registry_distribution_manifest_digest'] = reference.split('@')[1]
                evidence[name]['guest_pull_actual_image_id'] = observed['Id']
                evidence[name]['package_identity'] = package_identity
                evidence[name]['registry_publication_verified'] = True
            check('actual_registry_digests_pull_with_exact_source_config_and_rootfs')
            phase(report, 'guest_networks')
            services, routing = scope + '-services', scope + '-routing'
            for name in [services, routing]:
                call([*guest_cli, 'network', 'create', '--label', 'cowork.acceptance=' + scope, name])
            phase(report, 'operator_preparation')
            write(root / 'fleet_admin_url', ('postgres://fixture:' + values['fleet_password'].decode()
                + '@localhost:' + str(pg_fleet) + '/fixture').encode())
            backups = root / 'backups'; backups.mkdir(mode=0o700)
            releases = root / 'releases'; releases.mkdir(mode=0o700)
            mc = root / 'mc'; mc.mkdir(mode=0o700)
            os.environ.update({'SSL_CERT_FILE': str(root / 'ca.pem'), 'PGSSLMODE': 'verify-full',
                               'PGSSLROOTCERT': str(root / 'ca.pem'), 'PGGSSENCMODE': 'disable'})
            call([str(tools / 'mc'), '--config-dir', str(mc), 'alias', 'set', 'fixture', 'https://localhost:' + str(s3_admin),
                  values['s3_user'].decode(), values['s3_password'].decode()])
            bucket = 'cowork-' + uuid.UUID(created['id']).hex
            database = 'ci_' + uuid.UUID(created['id']).hex
            generations = transport.GenerationScope(created['id'], binding) if full else None
            pg_runtime = stack.enter_context(transport.serving(transport.PostgreSQLProxy(tls, 5432,
                connector('fleet-db', 5432), database=database, generations=generations)))
            s3_runtime = stack.enter_context(transport.serving(transport.proxy(tls, 9000, 's3', bucket=bucket,
                connector=connector('storage', 9000), generations=generations)))
            api_guest_port = 38080  # Only inside this new VM; private SSH -L selects a fresh host port.
            runtime_images = {key: image_refs[key] for key in ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']}
            target_images = dict(runtime_images)
            if full:
                runtime_images.update(server=image_refs['source_server'], runner=image_refs['source_runner'])
            config = {'backend': 'compose', 'state_directory': str(state), 'backup_directory': str(backups),
                'release_directory': str(releases), 'deletion_enabled': True, 'images': runtime_images,
                'management_url': management_origin, 'management_ca_file': str(root / 'ca.pem'),
                'provisioner_token_file': str(root / 'host_token'), 'fleet_management_url': 'https://10.0.2.2:' + str(manager_port),
                'database_endpoint': '10.0.2.2:' + str(pg_runtime), 'database_admin_url_file': str(root / 'fleet_admin_url'),
                's3_endpoint': 'https://10.0.2.2:' + str(s3_runtime), 's3_admin_alias': 'fixture',
                's3_admin_config_directory': str(mc), 'services_network': services, 'routing_network': routing,
                'sandbox_security_profile': 'linux_apparmor', 'fleet_host_budget': capacity,
                'execution_admission': {'server_images': list(dict.fromkeys([runtime_images['server'], target_images['server']]))},
                'executor_host': {'id': host_id, 'trust_domain': org, 'operator_state_id': vm_value['operator_state_id'],
                    'endpoint': vm_value['docker_endpoint'], 'daemon_id': daemon_info['ID'],
                    'ca_file': str(vm_root / 'ca.pem'), 'cert_file': str(vm_root / 'client.pem'), 'key_file': str(vm_root / 'client-key.pem')}}
            config['guest_runtime_endpoints'] = {'version': 1, 'instance_id': created['id'], 'executor_host_binding': binding,
                'public_ca_file': str(root / 'ca.pem'), 'api_loopback_port': api_guest_port,
                **{name: config[name] for name in ['database_endpoint', 's3_endpoint', 'fleet_management_url']}}
            if full:
                identity_file = root / 'backup-identity'
                call(['age-keygen', '-o', str(identity_file)]); identity_file.chmod(0o600)
                config.update({'backup_identity_file': str(identity_file), 'instance_service_limits': service_plan,
                    'host_capacity_attestation': {'version': 1, 'host_revision': registered['revision']},
                    'remote_volume_streaming_preparation': {'version': 1, 'max_archive_bytes': 16 * 1024**2,
                        'max_file_bytes': 4 * 1024**2, 'max_files': 1000, 'timeout_seconds': 60},
                    'remote_lifecycle_qualification': {'version': 1, 'mode': 'fixture_only',
                        'fixture_id': scope.removeprefix('cowork-hosted-vm-'), 'instance_id': created['id'],
                        'executor_host_binding': binding, 'max_workspace_archives': 32, 'max_workspace_archive_bytes': 64 * 1024**2}})
                config['remote_lifecycle_qualification']['fixture_id'] = str(uuid.UUID(config['remote_lifecycle_qualification']['fixture_id']))
            write(root / 'operator.json', json.dumps(config))
            config = remote_entrypoint.configuration(root / 'operator.json')
            phase(report, 'operator_context')
            if full:
                infrastructure, manager_client = complete_capacity_context(p, config, attestor)
            else:
                infrastructure, manager_client = p.Infrastructure(config), observe_manager(p.Manager(config), diagnostics)
            def sql(port, statement, database='fixture', user='fixture', password=None):
                env = {**os.environ, 'PGHOST': 'localhost', 'PGPORT': str(port), 'PGUSER': user, 'PGDATABASE': database,
                       'PGPASSWORD': password or values['management_password'].decode()}
                return call(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'], env=env, data=statement.encode()).decode().strip()
            phase(report, 'manager_schema')
            require(sql(pg_admin, 'SELECT max(version) FROM _sqlx_migrations;') == str(args.schema_version), 'manager_schema_mismatch')
            check('scoped_host_credential_and_immutable_instance_assignment')
            phase(report, 'operator_auth_fences')
            fixture_request(opener, management_origin, '/management/runner/v1/claim', body={'executor_host_binding': binding}, expected=403,
                    headers={'x-cowork-provisioner-token': values['broker'].decode()})
            fixture_request(opener, management_origin, '/management/runner/v1/claim',
                    body={'executor_host_binding': {**binding, 'host_id': identity()}}, expected=403,
                    headers={'x-cowork-provisioner-token': credential['token']})
            fixture_request(opener, 'https://localhost:' + str(manager_port), '/management/runner/v1/claim', body={}, expected=404)
            check('legacy_token_remote_claim_and_runtime_admin_paths_refused')
            operations = []
            chat_enabled = {'value': False}
            if full:
                original_manifest = p.compose_manifest
                def private_manifest(*values, **keywords):
                    manifest = original_manifest(*values, **keywords)
                    for name in ['api', 'worker']:
                        manifest['services'][name]['environment']['COWORK_CHAT_WORKSPACES_ENABLED'] = str(chat_enabled['value']).lower()
                    return manifest
                p.compose_manifest = private_manifest
                report['private_manifest_overrides'].append('API and worker chat flag across generations')
            def current():
                return next(row for row in fixture_request(opener, management_origin, base + '/instances', admin_session) if row['id'] == created['id'])
            def lifecycle(action, expected='succeeded', **arguments):
                phase(report, 'operation_request')
                row = current()
                operation_id = fixture_request(opener, management_origin, base + '/instances/' + created['id'] + '/operations', admin_session,
                    {'action': action, 'expected_revision': row['revision'], 'idempotency_key': identity(), **arguments})['id']
                phase(report, 'operation_claim')
                operation = manager_client.call('/claim', p.operator_contract(config))
                require(operation['id'] == operation_id and operation['instance_id'] == created['id']
                        and operation['executor_host_binding'] == binding, 'operation_scope_mismatch')
                if full:
                    generations.allow_operation(operation)
                journal = OperationJournal(infrastructure, operation); journal.begin()
                phase(report, 'operation_execute')
                try:
                    result, succeeded = infrastructure.execute(operation), True
                except p.ProvisioningError as error:
                    if expected != 'failed': raise
                    result, succeeded = {**error.result, 'error_code': str(error)}, False
                    if error.instance_state: result['instance_state'] = error.instance_state
                completion = journal.finish(succeeded, result)
                require('operator_secrets' not in json.dumps(completion), 'private_claim_secret_in_receipt')
                phase(report, 'operation_complete')
                manager_client.call('/operations/' + operation_id + '/complete', completion)
                manager_client.call('/operations/' + operation_id + '/complete', journal.recover())
                if full:
                    infrastructure.capacity_attestor.refresh()
                phase(report, 'operation_receipt')
                receipt = json.loads(sql(pg_admin, "SELECT jsonb_build_object('id',id,'state',state,'result',result) FROM management_operations WHERE id='" + operation_id + "' AND instance_id='" + created['id'] + "';"))
                require(receipt['state'] == expected and receipt['result']['executor_host_binding'] == binding
                        and (not full or receipt['result'].get('host_allocation') == operation['host_allocation']['binding']),
                        'operation_receipt_scope_mismatch')
                operations.append({'id': operation_id, 'action': action, 'state': receipt['state']})
                check('real_guest_' + action)
                return receipt if full else result
            with p.operator_lock(infrastructure.root):
                if full:
                    phase(report, 'reserved_host_capacity')
                    attestor.refresh()
                    require(attestor.qualified, 'initial_reserved_host_capacity_not_qualified')
                    check('fresh_db_clock_host55_reserved_attestation')
                lifecycle('provision')
                lifecycle('stop')
                lifecycle('start')
                phase(report, 'guest_runtime')
                directory = infrastructure.directory(created['id'])
                manifest = json.loads((directory / 'compose.json').read_text())
                require(all(manifest['services'][name]['environment']['COWORK_FLEET_CA_CERTIFICATE_FILE'] == '/run/secrets/control_public_ca'
                            for name in ['api', 'worker']), 'runtime_ca_not_bound')
                protected = [values[key] for key in ['broker', 'management_password', 'fleet_password', 's3_user',
                    's3_password', 'auth_key', 'audit_key']] + [credential['token'].encode()]
                for path in (vm_root / 'shared').rglob('*'):
                    require(not path.is_symlink(), 'guest_share_symlink_refused')
                    if path.is_file():
                        require(path.stat().st_size <= 16 * 1024**2, 'unexpected_guest_share_file')
                        content = path.read_bytes()
                        require(b'PRIVATE KEY' not in content and not any(secret in content for secret in protected),
                                'control_secret_in_guest_share')
                check('public_ca_only_and_no_control_credentials_in_readonly_guest_share')
                with socket.socket() as reservation:
                    reservation.bind(('127.0.0.1', 0)); local_api = reservation.getsockname()[1]
                forward = subprocess.Popen([*ssh_arguments(vm, vm_root), '-N', '-o', 'ExitOnForwardFailure=yes',
                    '-L', '127.0.0.1:' + str(local_api) + ':127.0.0.1:' + str(api_guest_port), 'cowork@127.0.0.1'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                local_opener = build_opener(ProxyHandler({}), NoRedirect())
                api = 'http://127.0.0.1:' + str(local_api)
                def api_ready():
                    try:
                        fixture_request(local_opener, api, '/readyz'); return forward.poll() is None
                    except (OSError, RuntimeError):
                        return False
                wait(api_ready, 'private_guest_api_forward_not_ready')
                phase(report, 'guest_login')
                ticket = fixture_request(opener, management_origin, base + '/instances/' + created['id'] + '/launch', admin_session, {})
                session = fixture_request(local_opener, api, '/api/v1/auth/fleet', body={'ticket': ticket['ticket'], 'device_id': identity()})
                token = session['access_token']
                fixture_request(local_opener, api, '/api/v1/auth/assurance', token)
                check('real_guest_fleet_login_and_management_access_over_verified_tls')
                team = fixture_request(local_opener, api, '/api/v1/teams', token, {'name': scope}, expected=201)
                project = fixture_request(local_opener, api, '/api/v1/projects', token,
                    {'name': scope, 'privacy': 'team_managed', 'team_id': team['id']}, expected=201)
                thread = fixture_request(local_opener, api, '/api/v1/threads', token,
                    {'project_id': project['id'], 'title': 'Dedicated guest raw run'}, expected=201)
                marker = uuid.uuid4().hex
                program = "from pathlib import Path;import os;Path('/workspace/hosted-vm.txt').write_text('" + marker + "');print('" + marker + "');print(os.uname().release);print(Path('/proc/self/attr/current').read_text())"
                phase(report, 'guest_raw_run')
                run = fixture_request(local_opener, api, '/api/v1/runs', token, {'project_id': project['id'], 'thread_id': thread['id'],
                    'project_revision': project['revision'], 'project_privacy': 'team_managed',
                    'executor_target': {'kind': 'server_linux', 'pool_id': None}, 'required_capabilities': ['shell'],
                    'idempotency_key': identity(), 'input': {'prompt': 'Dedicated VM qualification', 'sandbox': {
                        'schema_version': 2, 'run_id': identity(), 'image': 'core', 'argv': ['python3', '-c', program], 'network': 'none',
                        'limits': {'cpu_nanos': 100000000, 'memory_bytes': 128 * 1024**2, 'pids': 64, 'timeout_seconds': 20,
                                   'tmpfs_bytes': 4 * 1024**2, 'output_bytes': 4096}}}}, expected=201)
                records = []
                def completed():
                    records.append(fixture_request(local_opener, api, '/api/v1/runs/' + run['spec']['id'], token))
                    return records[-1]['state'] in ['completed', 'failed', 'canceled', 'expired']
                wait(completed, 'guest_raw_run_unfinished', 120)
                require(records[-1]['state'] == 'completed' and marker in records[-1]['result']['sandbox']['stdout'], 'guest_raw_run_failed')
                guest_kernel = vm.ssh(vm_root, 'uname -r').decode().strip()
                require(guest_kernel != os.uname().release and guest_kernel in records[-1]['result']['sandbox']['stdout'], 'sandbox_not_on_dedicated_guest_kernel')
                require('open-cowork-sandbox (enforce)' in records[-1]['result']['sandbox']['stdout'], 'raw_run_apparmor_not_enforced')
                require(vm.ssh(vm_root, 'sudo cat /sys/class/dmi/id/product_uuid').decode().strip().lower() == vm_value['id'], 'guest_vm_identity_mismatch')
                report['guest_kernel'] = guest_kernel; report['parent_kernel'] = os.uname().release
                report['run_id'] = run['spec']['id']
                check('raw_run_completed_on_distinct_guest_kernel_with_central_admission')
                phase(report, 'guest_artifact')
                artifact_content = ('private-vm-artifact-' + uuid.uuid4().hex).encode()
                artifact = fixture_request(local_opener, api, '/api/v1/runs/' + run['spec']['id'] +
                    '/attachments?name=vm-proof-' + uuid.uuid4().hex + '.txt', token, artifact_content,
                    expected=201, headers={'content-type': 'application/octet-stream'})
                require(artifact['storage'] == 'object_store_encrypted' and artifact['size_bytes'] == len(artifact_content),
                        'guest_object_store_artifact_not_encrypted')
                downloaded = fixture_request(local_opener, api, '/api/v1/runs/' + run['spec']['id'] + '/artifacts/' + artifact['id'],
                                     token, raw=True)
                require(downloaded == artifact_content, 'guest_object_store_tls_roundtrip_mismatch')
                report['artifact_id'] = artifact['id']
                check('guest_encrypted_s3_upload_and_verified_download_over_private_ca_tls')
                values_sql = []
                def reservations_released():
                    values_sql[:] = json.loads(sql(pg_admin, "SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'state',state,'instance_id',instance_id,'epoch',execution_epoch)),'[]'::jsonb) FROM management_execution_reservations WHERE instance_id='" + created['id'] + "';"))
                    return values_sql and all(row['state'] == 'released' for row in values_sql)
                phase(report, 'execution_release')
                wait(reservations_released, 'guest_execution_reservation_not_released', 40)
                report['reservations'] = values_sql
                if full:
                    phase(report, 'full_lifecycle')
                    from hosted_vm_lifecycle_checks import Context, qualify_lifecycle
                    context = Context(infrastructure=infrastructure, current=current, lifecycle=lifecycle, chat_enabled=chat_enabled,
                        api_origin=api, api_opener=local_opener, request_callback=fixture_request, management_origin=management_origin,
                        runtime_management_origin='https://localhost:' + str(manager_port),
                        management_opener=opener, management_base=base, management_token=admin_session,
                        sql_callback=sql, pg_fleet_port=pg_fleet, project=project, check=check, wait=wait,
                        report=report, source_schema=args.source_schema_version, target_schema=args.schema_version,
                        target_images=target_images, manager_client=manager_client)
                    context.token, context.session_tokens = token, session
                    qualify_lifecycle(context)
                lifecycle('stop')
                lifecycle('delete', confirmation_name=created['name'], purge_backups=True)
                phase(report, 'guest_deletion')
                require(not call([*guest_cli, 'ps', '--all', '--quiet']).strip(), 'guest_container_survived_deletion')
                require(not call([*guest_cli, 'volume', 'ls', '--quiet']).strip(), 'guest_volume_survived_deletion')
                require(sql(pg_fleet, "SELECT count(*) FROM pg_database WHERE datname='ci_" + uuid.UUID(created['id']).hex + "';",
                            password=values['fleet_password'].decode()) == '0', 'tenant_database_survived_deletion')
                check('guest_runtime_volume_database_deletion_verified')
                if full:
                    report['generation_transport_inventory'] = generations.evidence()
                    require(sql(pg_fleet, "SELECT count(*) FROM pg_database WHERE datname IN (" + ','.join(
                        "'" + row['database'] + "'" for row in generations.evidence()) + ');',
                        password=values['fleet_password'].decode()) == '0', 'retained_generation_database_survived_deletion')
                    held = sql(pg_admin, "SELECT count(*) FROM management_host_allocations WHERE instance_id='" + created['id'] + "' AND state!='released';")
                    require(held == '0', 'static_host_allocation_survived_verified_deletion')
                    check('all_generation_database_and_frozen_host_charge_deletion_verified')
                report['operations'] = operations
            phase(report, 'guest_network_cleanup')
            for name in [services, routing]:
                value = json.loads(call([*guest_cli, 'network', 'inspect', name]))[0]
                require(value['Labels'].get('cowork.acceptance') == scope and not value['Containers'], 'guest_network_cleanup_scope_mismatch')
                call([*guest_cli, 'network', 'rm', value['Id']])
            if forward is not None:
                forward.terminate(); forward.wait(timeout=10); forward = None
            # Stop renewal before stopping its physical daemon. The registered
            # callback remains responsible for every earlier failure path.
            if attestor is not None:
                attestor.close()
            phase(report, 'guest_stop')
            vm.stop(vm_root)
            report['qemu_process'] = vm.qualification_vm_evidence(vm_root)
            check('new_vm_stopped_after_scoped_deletion')
        phase(report, 'parent_cleanup')
        parent.cleanup()
        report.update(diagnostics.snapshot())
        report.update({'passed': True, 'fixture_resources_removed': False, 'active_fixture_resources_removed': True,
                       'live_containers_preserved': len(parent.baseline),
                       'finished_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())})
        require(vm.process(vm_root) is None, 'owned_vm_running_after_cleanup')
        # The descriptor and owner/identity proof precede recursive deletion.
        require(vm_root.parent == root / 'vms' and vm.load(vm_root)['id'] == report['vm_id'], 'vm_cleanup_scope_mismatch')
        durable_report(args.report, report)
        phase(report, 'private_cleanup')
        require(root.parent == (Path.home() / '.local/share/cowork/qualification').resolve() and root.name == scope,
                'private_cleanup_scope_mismatch')
        with vm.capacity_pool.locked_pool() as pool:
            pool.require_registered(vm_root, vm.load(vm_root))
            report['parent_capacity'].update(final_revision=pool.data['revision'], final_held=pool.totals(),
                new_vm_charge_retained=True, new_vm_stopped=True)
        if full:
            # Keep the exact charged VM's metadata/disk/private credentials. There
            # is no parent-pool release API; deleting its root would lose proof.
            for path in list(root.iterdir()):
                if path.name == 'vms': continue
                require(not path.is_symlink(), 'unsafe_private_fixture_cleanup')
                if path.is_dir(): shutil.rmtree(path)
                else: path.unlink()
            report['private_fixture_retained'] = True
            report['retained_vm_state'] = 'stopped and charged; no release API'
            durable_report(args.report, report)
        else:
            # Current controller also retains charges for a minimum fixture.
            report['private_fixture_retained'] = True
            report['retained_vm_state'] = 'stopped and charged; no release API'
            durable_report(args.report, report)
        print('active_fixture_resources_removed=true_retained_vm_stopped_and_charged=true', flush=True)
    except BaseException as error:
        report['passed'] = False
        report.update(diagnostics.snapshot())
        if isinstance(error, HttpQualificationFailure):
            report['failure_http_status'] = error.observation
        report['failure_code'] = failure_code(error)
        report['last_parent_action'] = parent.last_action
        report['private_control_subnet'] = getattr(parent, 'control_subnet', None)
        # No exception detail from subprocess/HTTP is retained; those may include secrets.
        report['private_fixture_retained'] = True
        if vm is not None and vm_root is not None:
            report['qemu_process'] = vm.qualification_vm_evidence(vm_root)
        # Persist the secret-free failure outside temporary/private state before
        # cleanup attempts; interruption of cleanup cannot erase diagnostics.
        durable_report(args.report, report)
        if forward is not None:
            forward.terminate()
            try: forward.wait(timeout=10)
            except subprocess.TimeoutExpired: forward.kill(); forward.wait(timeout=10)
        if vm is not None and vm_root is not None:
            report['qemu_process'] = vm.qualification_vm_evidence(vm_root)
            try: vm.stop(vm_root); report['owned_vm_stopped'] = vm.process(vm_root) is None
            except Exception: report['owned_vm_stopped'] = False
            report['qemu_process'] = vm.qualification_vm_evidence(vm_root)
        try:
            parent.preserved(); report['live_containers_preserved'] = len(parent.baseline)
            for identifier in list(parent.containers):
                value = parent.json('inspect', identifier)[0]
                require(value['Config']['Labels'].get('cowork.acceptance') == scope and identifier not in parent.baseline,
                        'failed_fixture_stop_scope_mismatch')
                parent.run('stop', '--time', '10', identifier)
            report['retained_parent_inventory'] = {'containers': sorted(parent.containers), 'networks': sorted(parent.networks), 'volumes': sorted(parent.volumes)}
        except Exception:
            report['cleanup_requires_review'] = True
        write(root / 'failure.json', json.dumps(report, indent=2))
        durable_report(args.report, report)
        print('qualification_failed_private_fixture_retained=' + str(root), flush=True)
        raise RuntimeError('hosted_vm_qualification_failed') from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--execute', action='store_true', help='Create and qualify only a fresh UUID fixture; otherwise validate arguments only')
    parser.add_argument('--qualification-profile', choices=['source54-minimum', 'host55-lifecycle'], default='source54-minimum')
    for name in ['server', 'runner', 'provisioner', 'postgres', 'storage', 'sandbox-core', 'sandbox-gui', 'sandbox-crew', 'egress-proxy']:
        parser.add_argument('--' + name + '-image-id', required=True, type=image_id)
    parser.add_argument('--schema-version', required=True, type=int)
    parser.add_argument('--source-schema-version', type=int)
    parser.add_argument('--source-server-image-id', type=image_id)
    parser.add_argument('--source-runner-image-id', type=image_id)
    parser.add_argument('--parent-capacity-plan-file', required=True, type=Path)
    parser.add_argument('--expected-parent-capacity-revision', required=True, type=int)
    parser.add_argument('--retained-pilot-root', required=True, type=Path)
    parser.add_argument('--retained-pilot-vm-id', required=True)
    parser.add_argument('--retained-pilot-known-stopped', action='store_true')
    parser.add_argument('--parent-docker-client', required=True)
    parser.add_argument('--parent-docker-context', required=True, choices=['desktop-linux'])
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--baseline-file', required=True, type=Path,
                        help='Authoritative pre-existing running container IDs and image IDs; additional fixtures refuse execution')
    args = parser.parse_args()
    validate_profile_arguments(args)
    require(args.report.is_absolute(), 'absolute_report_required')
    if args.execute:
        with QualificationSignals():
            execute(args)
    else:
        print('arguments_validated_no_resources_created=true')


if __name__ == '__main__':
    main()
