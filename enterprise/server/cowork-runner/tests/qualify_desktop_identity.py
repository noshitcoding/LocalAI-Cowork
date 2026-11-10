#!/usr/bin/env python3
"""Bounded real Runner qualification. Importing this module performs no actions.

Only --execute creates a fresh UUID fixture. Images and the Runner binary hash
are explicit; no pull/build/prune or Docker configuration mutation is permitted.
The original container baseline must be supplied, never rediscovered as truth.
"""
import argparse
import base64
from bisect import bisect_right
from concurrent.futures import ThreadPoolExecutor
from contextlib import AbstractContextManager, closing
import datetime
import hashlib
import hmac
import http.client
import http.server
import io
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import select
import shutil
import socket
import ssl
import struct
import subprocess
import tarfile
import threading
import time
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, ProxyHandler, Request, build_opener
import uuid

CREATE_DOMAIN = b'open-cowork/runner-desktop-create/v1\0'
PROBE_DOMAIN = b'open-cowork/runner-desktop-probe/v1\0'
DESKTOP_STOP_DOMAIN = b'open-cowork/runner-desktop-stop/v1\0'
EXECUTION_STOP_DOMAIN = b'open-cowork/runner-stop/v1\0'
BINDING_FIELDS = ('version', 'runner_id', 'engine_id', 'scope_prefix', 'reservation_id',
                  'attempt_id', 'container_id', 'container_name', 'created_at', 'started_at',
                  'run_id', 'session_id', 'lease_token', 'workspace_id', 'workspace_volume')
MAX_REPLY = 2 * 1024 * 1024
LABEL = 'cowork.runner_qualification'
EXECUTION_DEADLINE = None
LAST_ACTION = None


class FixtureCommandError(RuntimeError):
    """Public diagnostics contain a typed action/code, never argv or stderr."""
    def __init__(self, action, code, returncode=None):
        super().__init__(code)
        self.action, self.code, self.returncode = action, code, returncode


class FixtureSeedError(RuntimeError):
    def __init__(self, detail):
        super().__init__('fixture_seed_failed')
        self.detail = detail


def command_failure_code(stderr):
    # Inspect only bounded local command output; never include it in a report.
    value = stderr[:65536].lower()
    for fragments, code in [
        ((b'all predefined address pools have been fully subnetted', b'could not find an available, non-overlapping ipv4 address pool'), 'docker_address_pool_exhausted'),
        ((b'pool overlaps with other one on this address space',), 'docker_subnet_overlap'),
        ((b'no space left on device',), 'storage_exhausted'),
        ((b'permission denied', b'access is denied', b'operation not permitted', b'permissionerror:'), 'permission_denied'),
        ((b'cannot connect to the docker daemon', b'is the docker daemon running', b'the system cannot find the file specified'), 'docker_daemon_unavailable'),
    ]:
        if any(fragment in value for fragment in fragments): return code
    return 'command_failed'


def public_failure(error):
    if isinstance(error, FixtureSeedError):
        return {'code': 'fixture_seed_failed', 'classification': 'seed', **error.detail}
    if isinstance(error, FixtureCommandError):
        return {'code': error.code, 'classification': 'unknown_command_outcome' if error.code == 'command_timeout' else 'command', 'action': error.action,
                'exit_code': error.returncode}
    if isinstance(error, subprocess.TimeoutExpired):
        return {'code': 'command_timeout', 'classification': 'unknown_command_outcome', 'action': LAST_ACTION}
    code = str(error) if isinstance(error, RuntimeError) else type(error).__name__
    if not re.fullmatch('[A-Za-z0-9_:.-]{1,200}', code): code = 'qualification_failed'
    return {'code': code, 'classification': 'qualification', 'action': LAST_ACTION}


def control_subnet(scope, networks):
    """UUID-rotated RFC1918 /24, excluding the actual daemon's entire IPAM."""
    require(str(uuid.UUID(scope)) == scope, 'uuid_network_scope_required')
    require(isinstance(networks, list) and len(networks) <= 10000, 'invalid_parent_network_inventory')
    intervals = []
    try:
        for network in networks:
            require(isinstance(network, dict) and isinstance(network.get('IPAM'), dict), 'invalid_parent_network_inventory')
            entries = network['IPAM'].get('Config') or []
            require(isinstance(entries, list) and len(entries) <= 64, 'invalid_parent_network_inventory')
            for entry in entries:
                require(isinstance(entry, dict), 'invalid_parent_network_inventory')
                if entry.get('Subnet'):
                    occupied = ipaddress.ip_network(entry['Subnet'], strict=True)
                    if occupied.version == 4:
                        intervals.append((int(occupied.network_address), int(occupied.broadcast_address)))
    except (ValueError, TypeError):
        raise RuntimeError('invalid_parent_network_subnet') from None
    # Merge first so the complete, bounded /24 scan is O(n log n + 69888 log n),
    # rather than one 10000-network overlap scan for every possible candidate.
    merged = []
    for start, end in sorted(intervals):
        if merged and start <= merged[-1][1] + 1: merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else: merged.append((start, end))
    starts = [start for start, _ in merged]
    seed = int.from_bytes(hashlib.sha256(scope.encode()).digest()[:8], 'big')
    for block in ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']:
        pool = ipaddress.ip_network(block); count = pool.num_addresses // 256
        for offset in range(count):
            start = int(pool.network_address) + ((seed + offset) % count) * 256
            position = bisect_right(starts, start + 255) - 1
            if position < 0 or merged[position][1] < start:
                return str(ipaddress.ip_network((start, 24)))
    raise RuntimeError('private_control_subnets_exhausted')


def require(value, code):
    if not value:
        raise RuntimeError(code)


def identity():
    return str(uuid.uuid4())


def compact(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode()


def identifier(value):
    require(isinstance(value, str) and re.fullmatch('[a-f0-9]{64}', value), 'invalid_exact_container_id')
    return value


def image_id(value):
    if not re.fullmatch('sha256:[a-f0-9]{64}', value):
        raise argparse.ArgumentTypeError('exact immutable image ID required')
    return value


def sha256(value):
    if not re.fullmatch('[a-f0-9]{64}', value):
        raise argparse.ArgumentTypeError('exact lowercase SHA256 required')
    return value


def write_private(path, content, mode=0o600):
    path = Path(path)
    require(not path.is_symlink() and not path.exists(), 'fixture_file_collision')
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(content.encode() if isinstance(content, str) else content)
        stream.flush()
        os.fsync(stream.fileno())


def private_root(parent=None):
    require(os.name == 'posix', 'linux_or_wsl_required')
    parent = Path(parent or Path.home() / '.local/share/cowork/runner-qualification')
    require(parent.is_absolute() and all(not path.is_symlink() for path in [parent, *parent.parents]),
            'unsafe_fixture_parent')
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    require(parent.stat().st_uid == os.geteuid() and parent.stat().st_mode & 0o077 == 0,
            'owner_private_fixture_parent_required')
    root = parent / identity()
    root.mkdir(mode=0o700)
    return root


def wait(predicate, code, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.15)
    raise RuntimeError(code)


def command(argv, *, data=None, timeout=60, env=None, action='local.process'):
    global LAST_ACTION
    LAST_ACTION = action
    if EXECUTION_DEADLINE is not None:
        remaining = EXECUTION_DEADLINE - time.monotonic()
        require(remaining > 0, 'qualification_deadline_exceeded')
        timeout = min(timeout, remaining)
    try:
        completed = subprocess.run(argv, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   timeout=timeout, env=env)
    except subprocess.TimeoutExpired:
        raise FixtureCommandError(action, 'command_timeout') from None
    except OSError:
        raise FixtureCommandError(action, 'command_unavailable') from None
    if completed.returncode != 0:
        raise FixtureCommandError(action, command_failure_code(completed.stderr), completed.returncode)
    return completed.stdout


def baseline_records(path, required_count=39):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink()
            and path.stat().st_size <= 1024 * 1024, 'explicit_original_baseline_required')
    values = json.loads(path.read_text())
    require(isinstance(values, list) and len(values) == required_count and all(isinstance(value, dict)
            and re.fullmatch('[a-f0-9]{12,64}', value.get('ID', ''))
            and re.fullmatch('(?:sha256:)?[a-f0-9]{12,64}', value.get('Image', '')) for value in values),
            'invalid_original_baseline')
    require(len({value['ID'] for value in values}) == len(values), 'duplicate_original_baseline')
    return values


class ParentDocker:
    """One explicit daemon and original baseline, reusable across API fixtures."""
    def __init__(self, client, context, baseline_file, required_baseline_count=39):
        require(context == 'desktop-linux', 'explicit_desktop_linux_context_required')
        client = Path(client)
        require(client.is_absolute() and client.is_file(), 'explicit_parent_docker_client_required')
        self.argv = [str(client), '--context', context]
        allowed = {'PATH', 'HOME', 'USER', 'LOGNAME', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH',
                   'APPDATA', 'LOCALAPPDATA', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC',
                   'TEMP', 'TMP', 'TMPDIR', 'WSL_INTEROP', 'WSLENV', 'DOCKER_CONFIG'}
        self.env = {key: value for key, value in os.environ.items() if key in allowed}
        original = baseline_records(baseline_file, required_baseline_count)
        self.engine_id = self.json('info', '--format', '{{json .}}')['ID']
        running = self.run('ps', '--quiet', '--no-trunc').splitlines()
        resolved = self.json('inspect', *[value['ID'] for value in original])
        require({value['Id'] for value in resolved} == set(running), 'exclusive_original_baseline_required')
        self.baseline, self.baseline_started = {}, {}
        for expected, actual in zip(original, resolved):
            require(actual['Id'].startswith(expected['ID']) and actual['Image'].removeprefix('sha256:')
                    .startswith(expected['Image'].removeprefix('sha256:')) and actual['State']['Running'],
                    'original_baseline_changed')
            self.baseline[actual['Id']] = actual['Image']
            self.baseline_started[actual['Id']] = (actual['State']['StartedAt'], actual.get('RestartCount', 0))
        self.containers, self.volumes, self.networks, self.network_subnets = {}, {}, {}, {}
        self.pending_creates = {}

    def run(self, *arguments, data=None, timeout=90):
        return self.binary(*arguments, data=data, timeout=timeout).decode().strip()

    def binary(self, *arguments, data=None, timeout=90):
        require(arguments and arguments[0] in {'info', 'inspect', 'ps', 'image', 'network', 'volume',
                'create', 'run', 'cp', 'exec', 'rm', 'stop', 'start', 'restart', 'rename'},
                'untyped_docker_action')
        if arguments[0] in {'image', 'network', 'volume'}:
            require(len(arguments) > 1 and arguments[1] in {'inspect', 'ls', 'create', 'rm'},
                    'untyped_docker_resource_action')
            require(arguments[0] != 'image' or arguments[1] in {'inspect', 'ls'}, 'image_mutation_refused')
        self.last_action = 'docker.' + arguments[0]
        if arguments[0] in {'image', 'network', 'volume'}: self.last_action += '.' + arguments[1]
        return command([*self.argv, *arguments], data=data, timeout=timeout, env=self.env, action=self.last_action)

    def json(self, *arguments):
        return json.loads(self.run(*arguments))

    def assert_preserved(self):
        require(self.json('info', '--format', '{{json .}}')['ID'] == self.engine_id, 'parent_engine_changed')
        values = self.json('inspect', *self.baseline) if self.baseline else []
        require({value['Id'] for value in values} == set(self.baseline), 'original_inventory_changed')
        for value in values:
            require(value['State']['Running'] and value['Image'] == self.baseline[value['Id']]
                    and (value['State']['StartedAt'], value.get('RestartCount', 0)) == self.baseline_started[value['Id']],
                    'original_container_changed')

    def inspect_image(self, image):
        image_id(image)
        value = self.json('image', 'inspect', image)[0]
        require(value['Id'] == image and value['Os'] == 'linux' and value['Architecture'] == 'amd64',
                'immutable_linux_image_required')
        return {'actual_image_id': image, 'repository_digests': value.get('RepoDigests') or [],
                'tags_before_fixture': value.get('RepoTags') or []}

    def create_network(self, name, scope):
        require(not self.run('network', 'ls', '--quiet', '--filter', 'name=^' + re.escape(name) + '$'),
                'network_collision')
        identifiers = self.run('network', 'ls', '--quiet', '--no-trunc').splitlines()
        require(len(identifiers) <= 10000 and len(set(identifiers)) == len(identifiers)
                and all(re.fullmatch('[a-f0-9]{64}', key) for key in identifiers), 'invalid_parent_network_inventory')
        networks = []
        for offset in range(0, len(identifiers), 64):
            networks.extend(self.json('network', 'inspect', *identifiers[offset:offset + 64]))
        require(len(networks) == len(identifiers) and {value.get('Id') for value in networks} == set(identifiers),
                'parent_network_inventory_changed')
        subnet = control_subnet(scope, networks)
        self.network_subnets[name] = subnet
        self.pending_creates[('network', name)] = scope
        value = self.run('network', 'create', '--internal', '--subnet', subnet, '--label', LABEL + '=' + scope, name)
        identifier(value)
        self.networks[value] = (name, scope)
        self.pending_creates.pop(('network', name))
        return name

    def create_volume(self, name, scope):
        require(not self.run('volume', 'ls', '--quiet', '--filter', 'name=^' + re.escape(name) + '$'),
                'volume_collision')
        self.pending_creates[('volume', name)] = scope
        self.run('volume', 'create', '--label', LABEL + '=' + scope, name)
        self.volumes[name] = scope
        self.pending_creates.pop(('volume', name))
        return name

    def create_container(self, name, scope, image, arguments, command_arguments=()):
        require(not self.run('ps', '--all', '--quiet', '--filter', 'name=^/' + re.escape(name) + '$'),
                'container_collision')
        self.assert_preserved()
        self.pending_creates[('container', name)] = scope
        container = identifier(self.run('create', '--pull', 'never', '--name', name, '--label', LABEL + '=' + scope,
            '--cpus', '1', '--memory', '512m', '--pids-limit', '128', '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges:true', *arguments, image, *command_arguments))
        self.containers[container] = {'scope': scope, 'name': name}
        self.pending_creates.pop(('container', name))
        require(self.json('inspect', container)[0]['Image'] == image, 'container_image_changed')
        return container

    def owned_inventory(self, scope=None):
        """Read tracked fixture ownership only; never issue a cleanup/query."""
        own_runs = getattr(self, '_scope_runs', {}).get(scope, set())
        return {
            'containers': [{'container_id': key, **owner} for key, owner in sorted(self.containers.items())
                if scope is None or owner.get('scope') == scope or owner.get('run_id') in own_runs],
            'networks': [{'network_id': key, 'name': name, 'scope': owner,
                'subnet': getattr(self, 'network_subnets', {}).get(name)} for key, (name, owner) in sorted(self.networks.items())
                if scope is None or owner == scope],
            'volumes': [{'name': name, 'scope': owner} for name, owner in sorted(self.volumes.items())
                if scope is None or owner == scope],
            'pending_creates': [{'kind': kind, 'name': name, 'scope': owner,
                **({'subnet': getattr(self, 'network_subnets', {}).get(name)} if kind == 'network' else {})}
                for (kind, name), owner in sorted(self.pending_creates.items()) if scope is None or owner == scope],
        }

    def reconcile_fixture_creates(self, scope):
        # A CLI timeout is not evidence that the daemon did not create a job.
        # Reconcile only an observed exact name/owner. Absence after an unknown
        # result cannot release the fixture state while a late create is possible.
        for (kind, name), owner in list(self.pending_creates.items()):
            if owner != scope: continue
            if kind == 'container':
                ids = self.run('ps', '--all', '--no-trunc', '--quiet', '--filter', 'name=^/' + re.escape(name) + '$').splitlines()
                require(len(ids) == 1, 'fixture_create_outcome_unknown')
                container = identifier(ids[0]); require(container not in self.baseline, 'original_container_cannot_be_fixture')
                value = self.json('inspect', container)[0]
                require(value['Id'] == container and value['Name'] == '/' + name
                        and (value['Config'].get('Labels') or {}).get(LABEL) == scope, 'fixture_create_owner_unknown')
                self.containers[container] = {'scope': scope, 'name': name}
            elif kind == 'network':
                ids = self.run('network', 'ls', '--no-trunc', '--quiet', '--filter', 'name=^' + re.escape(name) + '$').splitlines()
                require(len(ids) == 1, 'fixture_create_outcome_unknown')
                value = self.json('network', 'inspect', ids[0])[0]
                require(value['Id'] == ids[0] and value['Name'] == name
                        and (value.get('Labels') or {}).get(LABEL) == scope, 'fixture_create_owner_unknown')
                self.networks[ids[0]] = (name, scope)
            else:
                names = self.run('volume', 'ls', '--quiet', '--filter', 'name=^' + re.escape(name) + '$').splitlines()
                require(names == [name], 'fixture_create_outcome_unknown')
                value = self.json('volume', 'inspect', name)[0]
                require(value['Name'] == name and (value.get('Labels') or {}).get(LABEL) == scope,
                        'fixture_create_owner_unknown')
                self.volumes[name] = scope
            self.pending_creates.pop((kind, name))

    def register_sandbox(self, binding):
        container = identifier(binding['container_id'])
        require(container not in self.baseline, 'original_container_cannot_be_fixture')
        owner = {'run_id': binding['run_id'], 'reservation_id': binding['reservation_id']}
        require(container not in self.containers or self.containers[container] == owner, 'fixture_container_binding_changed')
        self.containers[container] = owner

    def inspect_owned(self, container):
        require(container in self.containers and container not in self.baseline, 'foreign_container_action_refused')
        value = self.json('inspect', identifier(container))[0]
        require(value['Id'] == container, 'exact_container_identity_changed')
        owner, labels = self.containers[container], value['Config'].get('Labels') or {}
        if 'scope' in owner:
            require(labels.get(LABEL) == owner['scope'] and value['Name'] == '/' + owner['name'],
                    'fixture_container_owner_changed')
        else:
            require(labels.get('dev.opencowork.run_id') == owner['run_id']
                    and labels.get('dev.opencowork.admission') == owner['reservation_id'],
                    'sandbox_owner_changed')
        return value

    def exists(self, container):
        values = self.run('ps', '--all', '--no-trunc', '--quiet', '--filter', 'id=' + identifier(container)).splitlines()
        require(all(value == container for value in values), 'ambiguous_exact_container_inventory')
        return bool(values)

    def remove_container(self, container):
        require(container in self.containers and container not in self.baseline, 'foreign_container_cleanup_refused')
        if self.exists(container):
            self.inspect_owned(container)
            self.run('rm', '--force', container)
            require(not self.exists(container), 'fixture_container_removal_unconfirmed')
        self.containers.pop(container)

    def cleanup_scope(self, scope):
        self.assert_preserved()
        self.reconcile_fixture_creates(scope)
        for container, owner in list(self.containers.items()):
            if owner.get('scope') == scope or owner.get('run_id') in getattr(self, '_scope_runs', {}).get(scope, set()):
                self.remove_container(container)
        for key, (name, owner) in list(self.networks.items()):
            if owner == scope:
                value = self.json('network', 'inspect', key)[0]
                require(value['Id'] == key and value['Name'] == name and value['Labels'].get(LABEL) == scope
                        and not value['Containers'], 'foreign_or_active_network_cleanup_refused')
                self.run('network', 'rm', key)
                self.networks.pop(key)
        for name, owner in list(self.volumes.items()):
            if owner == scope:
                value = self.json('volume', 'inspect', name)[0]
                require(value['Name'] == name and value['Labels'].get(LABEL) == scope, 'foreign_volume_cleanup_refused')
                self.run('volume', 'rm', name)
                self.volumes.pop(name)
        self.assert_preserved()


def checked_native_hash(path, expected):
    path = Path(path)
    require(path.is_absolute() and path.is_file() and not path.is_symlink()
            and path.stat().st_size <= 120 * 1024 * 1024, 'native_runner_file_not_bounded_regular')
    with path.open('rb') as stream:
        require(stream.read(4) == b'\x7fELF', 'linux_runner_binary_required')
        stream.seek(0); actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    require(actual == expected, 'native_runner_hash_mismatch')
    return actual


def canonical_binding_bytes(binding):
    require(isinstance(binding, dict) and set(binding) == set(BINDING_FIELDS), 'invalid_binding_fields')
    return compact({name: binding[name] for name in BINDING_FIELDS})


def binding_digest(binding):
    return hashlib.sha256(canonical_binding_bytes(binding)).hexdigest()


def validate_binding(binding):
    canonical_binding_bytes(binding)
    require(type(binding['version']) is int and binding['version'] == 1, 'invalid_binding_version')
    for key in ['runner_id', 'reservation_id', 'attempt_id', 'run_id', 'session_id', 'lease_token']:
        require(str(uuid.UUID(binding[key])) == binding[key] and uuid.UUID(binding[key]).int != 0, 'invalid_binding_uuid')
    if binding['workspace_id'] is not None:
        require(str(uuid.UUID(binding['workspace_id'])) == binding['workspace_id']
                and uuid.UUID(binding['workspace_id']).int != 0, 'invalid_workspace_uuid')
    identifier(binding['container_id'])
    engine, prefix = binding['engine_id'], binding['scope_prefix']
    require(isinstance(engine, str) and 0 < len(engine) <= 256 and all(33 <= ord(char) <= 126 for char in engine),
            'invalid_engine_identity')
    resource = lambda value: isinstance(value, str) and re.fullmatch('[A-Za-z0-9_.-]{1,256}', value)
    require(resource(prefix) and len(prefix) <= 96 and resource(binding['workspace_volume'])
            and binding['workspace_volume'].startswith(prefix)
            and binding['container_name'] == prefix + 'desktop-' + uuid.UUID(binding['session_id']).hex,
            'invalid_binding_scope')
    for name in ['created_at', 'started_at']:
        value = binding[name]
        require(isinstance(value, str) and len(value) <= 64
                and datetime.datetime.fromisoformat(value.replace('Z', '+00:00')).tzinfo is not None,
                'invalid_binding_lifecycle_time')


def response_mac(key, domain, fields, request, response, binding=None):
    values = [*fields, hashlib.sha256(request).hexdigest(), '200', hashlib.sha256(response).hexdigest()]
    if binding is not None:
        values.append(hashlib.sha256(binding).hexdigest())
    message = domain + b''.join(struct.pack('>Q', len(value.encode())) + value.encode() for value in values)
    return 'v1:' + hmac.new(key, message, hashlib.sha256).hexdigest()


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


class RunnerProtocol:
    def __init__(self, origin, ca_file, signing_key, expected_engine=None):
        value = urlsplit(origin)
        require(value.scheme == 'https' and value.hostname and not value.username and not value.password
                and value.path in ['', '/'] and not value.query and not value.fragment, 'exact_https_origin_required')
        self.origin, self.signing_key, self.expected_engine = origin.rstrip('/'), signing_key, expected_engine
        self.context = ssl.create_default_context(cafile=str(ca_file))
        self.context.minimum_version = ssl.TLSVersion.TLSv1_2
        self.opener = build_opener(ProxyHandler({}), HTTPSHandler(context=self.context), NoRedirect())

    def headers(self, method, path, body, nonce=None):
        timestamp, nonce = str(int(time.time())), nonce or identity()
        message = '\n'.join([timestamp, nonce, method, path]).encode() + b'\n' + body
        return {'x-cowork-timestamp': timestamp, 'x-cowork-nonce': nonce,
                'x-cowork-signature': hmac.new(self.signing_key, message, hashlib.sha256).hexdigest()}

    def signed_request(self, method, path, body=b'', timeout=40, headers=None):
        global LAST_ACTION
        require(path.startswith('/') and not path.startswith('//') and len(body) <= 32 * 1024 * 1024,
                'invalid_signed_request')
        route = urlsplit(path).path
        LAST_ACTION = ('runner.execution_stop' if route == '/v1/execution-stop' else
            'runner.desktop_probe' if route == '/v1/desktop-session-probe' else
            'runner.desktop_create' if route.endswith('/desktop-sessions') else
            'runner.desktop_stop' if method == 'DELETE' and '/desktop-sessions/' in route else
            'runner.job' if route.endswith('/jobs') else 'runner.signed_request')
        headers = dict(headers or self.headers(method, path, body))
        started = time.monotonic()
        if EXECUTION_DEADLINE is not None:
            require(EXECUTION_DEADLINE > started, 'qualification_deadline_exceeded')
            timeout = min(timeout, EXECUTION_DEADLINE - started)
        request = Request(self.origin + path, data=body if method in ['POST', 'PUT'] else None,
                          method=method, headers={**headers, 'content-type': 'application/json'})
        try:
            response = self.opener.open(request, timeout=timeout)
        except HTTPError as error:
            response = error
        with response:
            data = response.read(MAX_REPLY + 1)
            require(len(data) <= MAX_REPLY, 'runner_response_exceeds_bounds')
            return {'status': response.status, 'headers': response.headers, 'body': data, 'request': body,
                    'fields': [method, path, headers['x-cowork-timestamp'], headers['x-cowork-nonce']], 'started': started}

    @staticmethod
    def one_header(reply, name):
        values = reply['headers'].get_all(name) or []
        require(len(values) == 1, 'missing_or_duplicate_proof_header')
        return values[0]

    def verify(self, reply, domain, binding=None):
        require(reply['status'] == 200, 'runner_operation_not_confirmed')
        supplied = self.one_header(reply, 'x-cowork-desktop-proof' if domain != EXECUTION_STOP_DOMAIN else 'x-cowork-stop-proof')
        require(re.fullmatch('v1:[a-f0-9]{64}', supplied)
                and hmac.compare_digest(supplied, response_mac(self.signing_key, domain, reply['fields'],
                    reply['request'], reply['body'], binding)), 'request_bound_runner_proof_invalid')

    def create(self, spec, lease, attempt):
        path = ('/v2' if spec.get('workspace_id') else '/v1') + '/desktop-sessions?lease_token=' + lease + '&attempt_id=' + attempt
        reply = self.signed_request('POST', path, compact(spec))
        return self.creation_reply(reply, spec, lease, attempt)

    def creation_reply(self, reply, spec, lease, attempt):
        encoded = self.one_header(reply, 'x-cowork-desktop-binding')
        require(len(encoded) <= 16384 and re.fullmatch('[A-Za-z0-9_-]+', encoded), 'invalid_binding_encoding')
        raw = base64.b64decode(encoded + '=' * (-len(encoded) % 4), altchars=b'-_', validate=True)
        require(len(raw) <= 8192, 'binding_header_exceeds_bounds')
        self.verify(reply, CREATE_DOMAIN, raw)
        job, result = json.loads(raw), json.loads(reply['body'])
        require(set(job) == {'binding', 'running', 'paused', 'restarting'} and job['running'] is True
                and job['paused'] is False and job['restarting'] is False, 'desktop_job_not_live')
        binding = job['binding']; validate_binding(binding)
        require(binding['run_id'] == spec['run_id'] and binding['session_id'] == spec['session_id']
                and binding['workspace_id'] == spec.get('workspace_id') and binding['lease_token'] == lease
                and binding['attempt_id'] == attempt and result['run_id'] == spec['run_id']
                and result['session_id'] == spec['session_id'] and result['schema_version'] == spec['schema_version']
                and result['dimensions'] == spec['dimensions'] and result['container_name'] == binding['container_name']
                and result['workspace_volume'] == binding['workspace_volume']
                and (self.expected_engine is None or binding['engine_id'] == self.expected_engine),
                'creation_binding_differs_from_request_or_engine')
        return result, binding

    def probe(self, binding):
        validate_binding(binding)
        request = {'version': 1, 'challenge': identity(), 'binding': binding}
        reply = self.signed_request('POST', '/v1/desktop-session-probe', compact(request), timeout=5)
        self.verify(reply, PROBE_DOMAIN)
        result = json.loads(reply['body'])
        require(time.monotonic() < reply['started'] + 5 and result['version'] == 1
                and result['challenge'] == request['challenge'] and result['job']['binding'] == binding
                and result['job']['running'] is True and result['job']['paused'] is False
                and result['job']['restarting'] is False, 'probe_not_fresh_or_same_physical_job')
        return result

    def stop(self, binding):
        validate_binding(binding)
        path = '/v1/desktop-sessions/' + binding['session_id'] + '?binding_sha256=' + binding_digest(binding)
        reply = self.signed_request('DELETE', path, timeout=20)
        return self.stop_reply(reply, binding)

    def stop_reply(self, reply, binding):
        self.verify(reply, DESKTOP_STOP_DOMAIN)
        result = json.loads(reply['body'])
        require(result == {'version': 1, 'binding': binding, 'execution_stopped': True}, 'desktop_stop_not_exact')
        return result

    def execution_stop(self, run, lease, attempts, cancel_lease=None):
        request = {'version': 1, 'run_id': run, 'lease_token': lease, 'cancel_lease': cancel_lease, 'attempts': attempts}
        reply = self.signed_request('POST', '/v1/execution-stop', compact(request), timeout=30)
        self.verify(reply, EXECUTION_STOP_DOMAIN)
        result = json.loads(reply['body'])
        require(all(result.get(key) == value for key, value in request.items())
                and result.get('execution_stopped') is True and isinstance(result.get('containers'), list),
                'execution_stop_not_exact')
        return result

    def health(self):
        global LAST_ACTION
        LAST_ACTION = 'runner.health'
        with self.opener.open(self.origin + '/healthz', timeout=2) as response:
            body = response.read(65537)
            require(response.status == 200 and len(body) <= 65536, 'runner_health_invalid')
            return json.loads(body)

    def rfb(self, binding, control=False):
        global LAST_ACTION
        self.probe(binding)
        LAST_ACTION = 'runner.desktop_stream'
        path = '/v1/desktop-sessions/' + binding['session_id'] + '/stream?control=' + str(control).lower() + '&binding_sha256=' + binding_digest(binding)
        target = urlsplit(self.origin)
        with socket.create_connection((target.hostname, target.port or 443), timeout=5) as tcp:
            with self.context.wrap_socket(tcp, server_hostname=target.hostname) as stream:
                stream.settimeout(5)
                key = base64.b64encode(secrets.token_bytes(16)).decode()
                headers = self.headers('GET', path, b'')
                request = ('GET ' + path + ' HTTP/1.1\r\nHost: ' + target.netloc + '\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ' + key + '\r\n'
                    + ''.join(name + ': ' + value + '\r\n' for name, value in headers.items()) + '\r\n').encode()
                stream.sendall(request)
                raw = bytearray()
                while b'\r\n\r\n' not in raw:
                    received = stream.recv(4096); require(received, 'rfb_upgrade_closed')
                    raw.extend(received); require(len(raw) <= 65536, 'rfb_upgrade_exceeds_bounds')
                head, initial = bytes(raw).split(b'\r\n\r\n', 1)
                lines = head.decode('ascii').split('\r\n')
                require(lines[0].startswith('HTTP/1.1 101 '), 'rfb_upgrade_rejected')
                actual = {name.lower(): value.strip() for name, value in (line.split(':', 1) for line in lines[1:])}
                expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
                require(actual.get('sec-websocket-accept') == expected, 'rfb_websocket_accept_invalid')
                wire = WebSocketBytes(stream, initial)
                version = wire.read(12); require(version == b'RFB 003.008\n', 'actual_vnc_version_required')
                wire.send(version)
                security_count = wire.read(1)[0]; require(0 < security_count <= 16, 'bounded_vnc_security_required')
                require(1 in wire.read(security_count), 'fixture_vnc_noauth_required')
                wire.send(b'\x01'); require(wire.read(4) == b'\0' * 4, 'vnc_security_rejected')
                wire.send(b'\x01')
                initial = wire.read(24)
                width, height, name_length = struct.unpack('>HH16xI', initial)
                require(0 < width <= 3840 and 0 < height <= 2160 and name_length <= 1024, 'vnc_server_init_invalid')
                wire.read(name_length)
                wire.send(struct.pack('>BBHHHH', 3, 0, 0, 0, 1, 1))
                header = wire.read(4); require(header[0] == 0, 'framebuffer_update_required')
                rectangles = struct.unpack('>H', header[2:])[0]; require(0 < rectangles <= 16, 'bounded_framebuffer_required')
                rectangle = wire.read(12); _, _, w, h, encoding = struct.unpack('>HHHHi', rectangle)
                require(w == 1 and h == 1 and encoding == 0 and initial[4] in [16, 32], 'small_raw_framebuffer_required')
                pixel = wire.read(initial[4] // 8)
                wire.close()
                return {'vnc_version': version.decode().strip(), 'width': width, 'height': height,
                        'pixel_bytes': len(pixel), 'pixel_sha256': hashlib.sha256(pixel).hexdigest()}


class WebSocketBytes:
    """Bounded binary RFC6455 adapter; no external websocket package required."""
    def __init__(self, stream, initial=b''):
        self.stream, self.raw, self.payload = stream, bytearray(initial), bytearray()

    def exact(self, count):
        while len(self.raw) < count:
            value = self.stream.recv(min(65536, count - len(self.raw)))
            require(value, 'websocket_closed_before_vnc_data'); self.raw.extend(value)
        result = bytes(self.raw[:count]); del self.raw[:count]; return result

    def send(self, value, opcode=2):
        require(len(value) <= 65536, 'websocket_write_exceeds_bounds')
        mask = secrets.token_bytes(4)
        size = bytes([len(value) | 0x80]) if len(value) < 126 else b'\xfe' + struct.pack('>H', len(value))
        self.stream.sendall(bytes([0x80 | opcode]) + size + mask + bytes(byte ^ mask[index % 4] for index, byte in enumerate(value)))

    def read(self, count):
        require(0 <= count <= 65536, 'vnc_read_exceeds_bounds')
        while len(self.payload) < count:
            first, second = self.exact(2)
            require(first & 0x70 == 0 and not second & 0x80, 'invalid_server_websocket_frame')
            length, opcode = second & 0x7f, first & 0xf
            if length == 126: length = struct.unpack('>H', self.exact(2))[0]
            if length == 127: length = struct.unpack('>Q', self.exact(8))[0]
            require(length <= 65536, 'websocket_frame_exceeds_bounds')
            value = self.exact(length)
            if opcode == 9: self.send(value, 10)
            elif opcode in [0, 2]: self.payload.extend(value)
            else: require(opcode == 10, 'unexpected_websocket_frame')
            require(len(self.payload) <= 131072, 'vnc_buffer_exceeds_bounds')
        result = bytes(self.payload[:count]); del self.payload[:count]; return result

    def close(self):
        self.send(struct.pack('>H', 1000), 8)


def certificates(root, dns_name):
    require(all(not (root / name).exists() and not (root / name).is_symlink() for name in
        ['ca-key.pem', 'ca.pem', 'ca.srl', 'server-key.pem', 'server.csr', 'server.pem', 'server.ext']), 'fixture_tls_file_collision')
    openssl = shutil.which('openssl'); require(openssl, 'existing_openssl_required')
    command([openssl, 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-days', '2', '-subj', '/CN=Owned Runner Fixture CA',
             '-keyout', str(root / 'ca-key.pem'), '-out', str(root / 'ca.pem')], action='openssl.ca_certificate')
    command([openssl, 'req', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-subj', '/CN=localhost',
             '-keyout', str(root / 'server-key.pem'), '-out', str(root / 'server.csr')], action='openssl.server_request')
    write_private(root / 'server.ext', 'subjectAltName=DNS:localhost,DNS:' + dns_name + ',IP:127.0.0.1\nextendedKeyUsage=serverAuth\n')
    command([openssl, 'x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.pem'),
             '-CAkey', str(root / 'ca-key.pem'), '-CAcreateserial', '-days', '2', '-sha256',
             '-extfile', str(root / 'server.ext'), '-out', str(root / 'server.pem')], action='openssl.server_certificate')
    for path in root.glob('*'):
        if path.is_file(): path.chmod(0o600)


def binary_from_tar(payload):
    require(len(payload) <= 128 * 1024 * 1024, 'runner_binary_archive_exceeds_bounds')
    with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
        entries = archive.getmembers()
        require(len(entries) == 1 and entries[0].isfile() and entries[0].size <= 120 * 1024 * 1024,
                'runner_binary_archive_not_exact')
        content = archive.extractfile(entries[0]).read()
        require(content.startswith(b'\x7fELF'), 'linux_runner_binary_required')
        return hashlib.sha256(content).hexdigest()


RELAY_PROGRAM = """import os,select,socket,sys
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as target:
 target.connect('/var/run/docker.sock')
 sources=[sys.stdin.buffer,target]
 while True:
  ready,_,_=select.select(sources,[],[],120)
  if not ready:break
  for source in ready:
   data=os.read(0,65536) if source is sys.stdin.buffer else target.recv(65536)
   if not data:
    if source is sys.stdin.buffer:
     sources.remove(source);target.shutdown(socket.SHUT_WR);continue
    sys.exit(0)
   if source is sys.stdin.buffer:target.sendall(data)
   else:sys.stdout.buffer.write(data);sys.stdout.buffer.flush()
"""


class NativeDockerRelay(AbstractContextManager):
    """Private Unix socket to exact parent Docker, via one labelled helper."""
    def __init__(self, parent, container, root):
        self.parent, self.container, self.path = parent, container, root / 'docker.sock'
        require(len(os.fsencode(self.path)) < 104, 'native_relay_root_path_too_long')
        self.socket, self.closed = socket.socket(socket.AF_UNIX), threading.Event()
        self.socket.bind(str(self.path)); self.path.chmod(0o600); self.socket.listen(16); self.socket.settimeout(.2)
        self.slots, self.threads, self.processes = threading.BoundedSemaphore(16), [], set()
        self.lock = threading.Lock()

    def __enter__(self):
        self.acceptor = threading.Thread(target=self.accept, daemon=True); self.acceptor.start(); return self

    def accept(self):
        while not self.closed.is_set():
            try: stream, _ = self.socket.accept()
            except socket.timeout: continue
            except OSError: break
            if not self.slots.acquire(False): stream.close(); continue
            thread = threading.Thread(target=self.relay, args=(stream,), daemon=True)
            self.threads.append(thread); thread.start()

    def relay(self, stream):
        process = None
        try:
            self.parent.inspect_owned(self.container)
            process = subprocess.Popen([*self.parent.argv, 'exec', '--interactive', self.container, 'python3', '-u', '-c', RELAY_PROGRAM],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=self.parent.env)
            with self.lock: self.processes.add(process)
            stream.settimeout(10); deadline = time.monotonic() + 180
            sources = [stream, process.stdout]
            while not self.closed.is_set() and time.monotonic() < deadline:
                ready, _, _ = select.select(sources, [], [], .2)
                for source in ready:
                    data = stream.recv(65536) if source is stream else os.read(process.stdout.fileno(), 65536)
                    if not data:
                        if source is stream:
                            # Docker attach closes its write half when no input
                            # remains. Keep pumping the daemon's output to EOF.
                            process.stdin.close()
                            sources.remove(stream)
                            continue
                        return
                    if source is stream: process.stdin.write(data); process.stdin.flush()
                    else: stream.sendall(data)
        finally:
            stream.close()
            if process is not None:
                process.terminate()
                try: process.wait(timeout=2)
                except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=2)
                with self.lock: self.processes.discard(process)
            self.slots.release()

    def __exit__(self, *_args):
        self.closed.set(); self.socket.close(); self.acceptor.join(timeout=2)
        for thread in self.threads: thread.join(timeout=2)
        require(not any(thread.is_alive() for thread in self.threads), 'native_relay_shutdown_unconfirmed')
        self.path.unlink(missing_ok=True)


class RunnerFixture(AbstractContextManager):
    """An isolated fixture; the same object can serve a three-API HA harness."""
    def __init__(self, parent, root, runner_image_id, gui_image_id, core_image_id, expected_binary_sha256,
                 scope=None, native_binary=None, linux_docker_cli=None, host='127.0.0.1', memory_mib=4096):
        self.parent, self.root = parent, Path(root)
        require(self.root.is_absolute() and self.root.is_dir() and not self.root.is_symlink()
                and all(not path.is_symlink() for path in self.root.parents)
                and self.root.stat().st_uid == os.geteuid() and self.root.stat().st_mode & 0o077 == 0,
                'owner_private_uuid_root_required')
        self.scope = scope or identity(); require(str(uuid.UUID(self.scope)) == self.scope, 'uuid_fixture_scope_required')
        self.instance_id = identity(); self.resource_prefix = 'cowork-' + uuid.UUID(self.instance_id).hex + '-'
        self.name = 'cowork-runner-proof-' + uuid.UUID(self.scope).hex
        self.network, self.container, self.process, self.relay = None, None, None, None
        self.core_image_id, self.gui_image_id, self.runner_image_id = core_image_id, gui_image_id, runner_image_id
        image_id(core_image_id); image_id(gui_image_id)
        if runner_image_id is not None: image_id(runner_image_id)
        self.expected_binary_sha256 = sha256(expected_binary_sha256)
        self.native_binary, self.linux_docker_cli, self.host = native_binary, linux_docker_cli, host
        require(bool(native_binary) != bool(runner_image_id), 'select_exact_runner_binary_or_image')
        require(type(memory_mib) is int and 1024 <= memory_mib <= 16384, 'bounded_fixture_runner_memory_required')
        self.memory_mib = memory_mib
        if native_binary:
            checked_native_hash(native_binary, self.expected_binary_sha256)
            cli = Path(linux_docker_cli or '')
            require(cli.is_absolute() and cli.is_file() and not cli.is_symlink(), 'exact_existing_linux_docker_cli_required')
        self.signing_key = secrets.token_hex(32).encode()
        self.ca_file = self.root / 'ca.pem'; self.runs = set(); self.workspace_volumes = {}; self.sandbox_ids = set(); self.evidence = {}
        if not hasattr(parent, '_scope_runs'): parent._scope_runs = {}
        parent._scope_runs[self.scope] = self.runs
        self.identity_record = {'version': 1, 'scope': self.scope, 'instance_id': self.instance_id,
            'resource_prefix': self.resource_prefix, 'runner_name': self.name,
            'parent_engine_id': parent.engine_id, 'core_image_id': core_image_id,
            'gui_image_id': gui_image_id, 'runner_image_id': runner_image_id,
            'expected_runner_sha256': self.expected_binary_sha256}

    def seed(self, volume, values, owner=0):
        require(self.parent.volumes.get(volume) == self.scope and all(re.fullmatch('[a-z0-9_.-]{1,64}', name)
                and isinstance(value, bytes) and len(value) <= 1024 * 1024 for name, value in values.items()), 'invalid_fixture_seed')
        program = """import json,os,sys
directory='/out'
owner=%d
os.chmod(directory,0o700)
paths=[]
for name,encoded in json.load(sys.stdin).items():
 path=os.path.join(directory,name)
 with open(path,'xb') as output:
  output.write(bytes.fromhex(encoded));output.flush();os.fsync(output.fileno())
 os.chmod(path,0o600)
 paths.append(path)
for path in paths:os.chown(path,owner,owner)
os.chown(directory,owner,owner)
""" % owner
        name = self.name + '-seed-' + uuid.uuid4().hex
        container = self.parent.create_container(name, self.scope, self.core_image_id,
            ['--interactive', '--network', 'none', '--read-only', '--user', '0', '--volume', volume + ':/out',
             '--cap-add', 'CHOWN', '--entrypoint', 'python3'], ['-c', program])
        stage = 'workspace_initialization' if owner == 10001 else 'runner_private_volume_initialization'
        try:
            self.parent.run('start', '--attach', '--interactive', container,
                data=compact({key: value.hex() for key, value in values.items()}), timeout=30)
            state = self.parent.inspect_owned(container)['State']
            require(state['ExitCode'] == 0 and state['Status'] == 'exited' and state['Running'] is False,
                    'fixture_seed_exit_unconfirmed')
        except Exception as error:
            detail = {'stage': stage, 'action': 'docker.start', 'container_id': container,
                      'cause': public_failure(error), 'container_exit_code': None, 'container_status': 'unconfirmed'}
            try:
                state = self.parent.inspect_owned(container)['State']
                if type(state.get('ExitCode')) is int and 0 <= state['ExitCode'] <= 65535:
                    detail['container_exit_code'] = state['ExitCode']
                if state.get('Status') in ['created', 'running', 'paused', 'restarting', 'removing', 'exited', 'dead']:
                    detail['container_status'] = state['Status']
            except Exception as inspection_error:
                detail['inspection_failure'] = public_failure(inspection_error)
            self.evidence.setdefault('seed_failures', []).append(detail)
            raise FixtureSeedError(detail) from None
        finally: self.parent.remove_container(container)

    def __enter__(self):
        try: return self.start()
        except Exception:
            try: self.cleanup()
            except Exception: pass  # Own unresolved ledger stays retained.
            raise

    def start(self):
        require(self.network is None and not hasattr(self, 'origin'), 'runner_fixture_already_started')
        # Recovery names are durable before any Docker create/launch. No keys,
        # argv, user data or unrestricted process environment are written.
        write_private(self.root / 'fixture.json', compact(self.identity_record))
        self.evidence['core'] = self.parent.inspect_image(self.core_image_id)
        self.evidence['gui'] = self.parent.inspect_image(self.gui_image_id)
        certificates(self.root, self.name)
        self.network = self.parent.create_network(self.name + '-network', self.scope)
        self.state_volume = self.parent.create_volume(self.name + '-state', self.scope)
        self.cert_volume = self.parent.create_volume(self.name + '-tls', self.scope)
        self.seed(self.state_volume, {})
        self.seed(self.cert_volume, {'server.pem': (self.root / 'server.pem').read_bytes(), 'server-key.pem': (self.root / 'server-key.pem').read_bytes()})
        config = {'COWORK_RUNNER_SIGNING_KEY': self.signing_key.decode(), 'COWORK_RUNNER_INSTANCE_ID': self.instance_id,
            'COWORK_RUNNER_CORE_IMAGE': self.core_image_id, 'COWORK_RUNNER_GUI_IMAGE': self.gui_image_id,
            'COWORK_RUNNER_CREW_IMAGE': self.core_image_id, 'COWORK_RUNNER_CPU_MILLIS': '4000',
            'COWORK_RUNNER_MEMORY_MIB': str(self.memory_mib), 'COWORK_RUNNER_CONCURRENT_RUNS': '4', 'COWORK_RUNNER_MAX_CONTAINERS': '8',
            'COWORK_RUNNER_DESKTOP_JOB_PROOFS': 'true', 'COWORK_SANDBOX_SECCOMP_PROFILE': 'builtin',
            'COWORK_SANDBOX_REQUIRE_APPARMOR': 'false', 'COWORK_SANDBOX_EGRESS_NETWORK': self.network,
            'COWORK_SANDBOX_HTTP_PROXY': 'http://127.0.0.1:9', 'RUST_LOG': 'warn'}
        if self.native_binary:
            path = Path(self.native_binary); cli = Path(self.linux_docker_cli or '')
            checked_native_hash(path, self.expected_binary_sha256)
            require(cli.is_absolute() and cli.is_file() and not cli.is_symlink(), 'exact_existing_linux_docker_cli_required')
            self.evidence['native_runner'] = {'binary_sha256': self.expected_binary_sha256}
            helper = self.parent.create_container(self.name + '-relay', self.scope, self.core_image_id,
                ['--network', 'none', '--read-only', '--user', '0', '--volume', '/var/run/docker.sock:/var/run/docker.sock', '--entrypoint', 'python3'],
                ['-u', '-c', 'import time;time.sleep(600)'])
            self.parent.run('start', helper)
            self.relay = NativeDockerRelay(self.parent, helper, self.root).__enter__()
            directory = self.root / 'native-state'; directory.mkdir(mode=0o700)
            shim = self.root / 'bin'; shim.mkdir(mode=0o700)
            write_private(shim / 'docker', '#!/bin/sh\nexec ' + __import__('shlex').quote(str(cli)) + ' "$@"\n', 0o700)
            with socket.socket() as listener: listener.bind(('127.0.0.1', 0)); self.port = listener.getsockname()[1]
            self.native_env = {key: value for key, value in os.environ.items() if key in ['PATH', 'HOME', 'USER', 'LANG', 'LC_ALL']}
            self.native_env.update(config, PATH=str(shim) + ':' + self.native_env.get('PATH', ''), DOCKER_HOST='unix://' + str(self.relay.path),
                DOCKER_CONFIG=str(shim), COWORK_RUNNER_LISTEN_ADDR='127.0.0.1:' + str(self.port),
                COWORK_RUNNER_ADMISSION_DIRECTORY=str(directory), COWORK_RUNNER_TLS_CERTIFICATE_FILE=str(self.root / 'server.pem'),
                COWORK_RUNNER_TLS_PRIVATE_KEY_FILE=str(self.root / 'server-key.pem'))
            self._start_native()
        else:
            self.evidence['runner'] = self.parent.inspect_image(self.runner_image_id)
            extract = self.parent.create_container(self.name + '-extract', self.scope, self.runner_image_id,
                ['--network', 'none', '--entrypoint', '/bin/true'])
            try:
                actual_hash = binary_from_tar(self.parent.binary('cp', extract + ':/usr/local/bin/cowork-runner', '-'))
                require(actual_hash == self.expected_binary_sha256, 'runner_image_binary_hash_mismatch')
                self.evidence['runner']['binary_sha256'] = actual_hash
            finally: self.parent.remove_container(extract)
            config.update(COWORK_RUNNER_ADMISSION_DIRECTORY='/runner-state', COWORK_RUNNER_TLS_CERTIFICATE_FILE='/runner-tls/server.pem',
                          COWORK_RUNNER_TLS_PRIVATE_KEY_FILE='/runner-tls/server-key.pem')
            args = ['--network', self.network, '--read-only', '--tmpfs', '/tmp:rw,nosuid,nodev,size=64m',
                    '--volume', self.state_volume + ':/runner-state', '--volume', self.cert_volume + ':/runner-tls:ro',
                    '--volume', '/var/run/docker.sock:/var/run/docker.sock', '--publish', '127.0.0.1::8090']
            for key, value in config.items(): args.extend(['--env', key + '=' + value])
            self.container = self.parent.create_container(self.name, self.scope, self.runner_image_id, args)
            self.parent.run('start', self.container)
            ports = self.parent.inspect_owned(self.container)['NetworkSettings']['Ports']['8090/tcp']
            require(len(ports) == 1 and ports[0]['HostIp'] == '127.0.0.1', 'runner_not_exclusively_loopback_published')
            self.port = int(ports[0]['HostPort'])
        self.origin = 'https://' + self.host + ':' + str(self.port)
        self.internal_origin = 'https://' + self.name + ':8090' if self.container else self.origin
        self.actual_engine_id = self.parent.engine_id
        self.protocol = RunnerProtocol(self.origin, self.ca_file, self.signing_key, self.actual_engine_id)
        def ready():
            try: return self.protocol.health().get('desktop_job_proofs_version') == 1
            except (OSError, RuntimeError): return False
        wait(ready, 'runner_https_not_ready', 60)
        self.schema_version = self.protocol.health()['schema_version']
        self.evidence.update(engine_id=self.actual_engine_id, instance_id=self.instance_id, scope_prefix=self.resource_prefix,
                             origin=self.origin, schema_version=self.schema_version)
        self.parent.assert_preserved()
        return self

    def _start_native(self):
        log = self.root / ('native-' + uuid.uuid4().hex + '.log')
        self.log = log.open('xb'); log.chmod(0o600)
        self.process = subprocess.Popen([str(self.native_binary)], env=self.native_env, stdout=self.log,
                                        stderr=subprocess.STDOUT, start_new_session=True)

    def restart(self):
        if self.container:
            self.parent.inspect_owned(self.container); self.parent.run('restart', '--time', '5', self.container)
        else:
            self._stop_native(); self._start_native()
        def ready():
            try: return self.protocol.health().get('desktop_job_proofs_version') == 1
            except (OSError, RuntimeError): return False
        wait(ready, 'restarted_runner_not_ready', 30)

    def _stop_native(self):
        if self.process is not None:
            import signal
            os.killpg(self.process.pid, signal.SIGTERM)
            try: self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL); self.process.wait(timeout=5)
            self.log.close(); self.process = None

    def register_run(self, run, workspace=None):
        require(str(uuid.UUID(run)) == run and (workspace is None or str(uuid.UUID(workspace)) == workspace), 'invalid_fixture_run')
        # Runner uses the run resource kind for V1 and V2 alike; in V2 the
        # resource UUID is the workspace UUID, not the current run UUID.
        name = self.resource_prefix + 'run-' + uuid.UUID(workspace or run).hex
        if name not in self.parent.volumes:
            self.parent.create_volume(name, self.scope); self.seed(name, {}, owner=10001)
        require(self.parent.volumes[name] == self.scope, 'workspace_volume_scope_mismatch')
        self.runs.add(run)
        self.workspace_volumes[name] = (run, workspace)
        return name

    def desktop_spec(self):
        run, workspace = identity(), identity(); self.register_run(run, workspace)
        return {'schema_version': self.schema_version, 'run_id': run, 'session_id': identity(), 'workspace_id': workspace,
                'dimensions': {'width': 640, 'height': 480, 'scale_factor': 1.0}, 'network': 'filtered_egress',
                'limits': limits(memory=512 * 1024 * 1024, cpu=500_000_000)}

    def inspect(self, binding):
        require(binding['scope_prefix'] == self.resource_prefix and binding['run_id'] in self.runs, 'foreign_fixture_binding')
        expected_volume = self.resource_prefix + 'run-' + uuid.UUID(binding['workspace_id'] or binding['run_id']).hex
        require(binding['workspace_volume'] == expected_volume and expected_volume in self.workspace_volumes
                and self.parent.volumes.get(expected_volume) == self.scope, 'unregistered_fixture_workspace_binding')
        self.parent.register_sandbox(binding); self.sandbox_ids.add(binding['container_id'])
        return self.parent.inspect_owned(binding['container_id'])

    def rename(self, binding):
        self.inspect(binding)
        self.parent.run('rename', binding['container_id'], self.name + '-renamed-' + uuid.uuid4().hex)

    def restart_desktop(self, binding):
        self.inspect(binding)
        self.parent.run('restart', '--time', '5', binding['container_id'])

    def ledger(self):
        program = "import json,os;d='/in';print(json.dumps([json.load(open(d+'/'+n)) for n in os.listdir(d) if n.endswith('.json')]))"
        if self.native_binary:
            return [json.loads(path.read_text()) for path in (self.root / 'native-state').glob('*.json')]
        return json.loads(self.read_volume(self.state_volume, program))

    def intent(self, session):
        require(str(uuid.UUID(session)) == session, 'exact_fixture_session_uuid_required')
        if self.native_binary:
            return json.loads((self.root / 'native-state/desktop-identities' / (session + '.json')).read_text())
        program = "import json;print(json.dumps(json.load(open('/in/desktop-identities/%s.json'))))" % session
        return json.loads(self.read_volume(self.state_volume, program))

    def read_volume(self, volume, program):
        require(self.parent.volumes.get(volume) == self.scope, 'foreign_volume_read_refused')
        if volume in [getattr(self, 'state_volume', None), getattr(self, 'cert_volume', None)]:
            user = '0'
        else:
            registered = self.workspace_volumes.get(volume)
            require(registered is not None, 'unregistered_workspace_volume_read_refused')
            run, workspace = registered
            expected = self.resource_prefix + 'run-' + uuid.UUID(workspace or run).hex
            require(run in self.runs and volume == expected, 'workspace_volume_binding_mismatch')
            user = '10001'
        container = self.parent.create_container(self.name + '-reader-' + uuid.uuid4().hex, self.scope, self.core_image_id,
            ['--network', 'none', '--read-only', '--user', user, '--volume', volume + ':/in:ro', '--entrypoint', 'python3'], ['-c', program])
        try:
            output = self.parent.binary('start', '--attach', container, timeout=20)
            require(len(output) <= MAX_REPLY and self.parent.inspect_owned(container)['State']['ExitCode'] == 0,
                    'fixture_reader_failed_or_exceeds_bounds')
            return output
        finally: self.parent.remove_container(container)

    def discover(self):
        ids = self.parent.run('ps', '--all', '--quiet', '--no-trunc', '--filter', 'name=^/' + re.escape(self.resource_prefix)).splitlines()
        for value in self.parent.json('inspect', *ids) if ids else []:
            labels = value['Config'].get('Labels') or {}
            require(value['Name'].startswith('/' + self.resource_prefix) and labels.get('dev.opencowork.run_id') in self.runs,
                    'unregistered_sandbox_in_fixture_scope')
            reservation = labels.get('dev.opencowork.admission'); require(reservation and uuid.UUID(reservation).int, 'unbound_fixture_sandbox')
            self.parent.register_sandbox({'container_id': value['Id'], 'run_id': labels['dev.opencowork.run_id'], 'reservation_id': reservation})

    def cleanup(self):
        self._stop_native()
        if self.container in self.parent.containers:
            self.parent.remove_container(self.container)
        if self.relay is not None: self.relay.__exit__(None, None, None); self.relay = None
        self.discover()
        if hasattr(self, 'state_volume'):
            require(all(value.get('container_id') for value in self.ledger()),
                    'unresolved_create_retains_private_state_and_workspaces')
        self.parent.cleanup_scope(self.scope)
        self.signing_key = b''
        for name in ['server-key.pem', 'ca-key.pem']:
            (self.root / name).unlink(missing_ok=True)

    def __exit__(self, *_args):
        self.cleanup()


def limits(memory=256 * 1024 * 1024, cpu=250_000_000):
    return {'cpu_nanos': cpu, 'memory_bytes': memory, 'pids': 64, 'timeout_seconds': 60,
            'tmpfs_bytes': 64 * 1024 * 1024, 'output_bytes': 65536}


class ResponseLossProxy(AbstractContextManager):
    """Real upstream request; one successful matching response is discarded."""
    def __init__(self, protocol, certificate_file, private_key_file, method, path):
        self.protocol, self.method, self.path = protocol, method, path
        self.reached, self.dropped = threading.Event(), threading.Event()
        proxy = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args): pass
            def do_POST(self): proxy.forward(self)
            def do_DELETE(self): proxy.forward(self)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(str(certificate_file), str(private_key_file))
        class Server(http.server.ThreadingHTTPServer):
            daemon_threads = False
            slots = threading.BoundedSemaphore(4)
            def get_request(self):
                raw, address = self.socket.accept(); raw.settimeout(5)
                try: return context.wrap_socket(raw, server_side=True), address
                except Exception: raw.close(); raise
            def process_request(self, request, address):
                if not self.slots.acquire(False): request.close(); return
                try: super().process_request(request, address)
                except Exception: self.slots.release(); raise
            def process_request_thread(self, request, address):
                try: super().process_request_thread(request, address)
                finally: self.slots.release()
        self.server = Server(('127.0.0.1', 0), Handler)
        self.origin = 'https://localhost:' + str(self.server.server_port)

    def verify_loss_reply(self, handler, body, result, data):
        fields = [handler.command, handler.path]
        for name in ['x-cowork-timestamp', 'x-cowork-nonce']:
            values = handler.headers.get_all(name) or []
            require(len(values) == 1, 'faultproxy_missing_or_duplicate_request_header')
            fields.append(values[0])
        headers = http.client.HTTPMessage()
        for name, value in result.getheaders(): headers[name] = value
        reply = {'status': result.status, 'headers': headers, 'body': data, 'request': body, 'fields': fields}
        target = urlsplit(handler.path); query = parse_qs(target.query, strict_parsing=True)
        if handler.command == 'DELETE':
            stopped = json.loads(data); binding = stopped['binding']; validate_binding(binding)
            require(target.path == '/v1/desktop-sessions/' + binding['session_id']
                    and query == {'binding_sha256': [binding_digest(binding)]}
                    and (self.protocol.expected_engine is None or binding['engine_id'] == self.protocol.expected_engine),
                    'faultproxy_stop_binding_mismatch')
            self.protocol.stop_reply(reply, binding)
        else:
            spec = json.loads(body)
            require(set(query) == {'lease_token', 'attempt_id'} and all(len(value) == 1 for value in query.values())
                    and target.path == ('/v2' if spec.get('workspace_id') else '/v1') + '/desktop-sessions',
                    'faultproxy_creation_binding_mismatch')
            self.protocol.creation_reply(reply, spec, query['lease_token'][0], query['attempt_id'][0])

    def forward(self, handler):
        try:
            require(handler.command == self.method and handler.path == self.path and not self.dropped.is_set(), 'faultproxy_scope_mismatch')
            require(handler.headers.get('Transfer-Encoding') is None, 'faultproxy_chunked_request_refused')
            size = int(handler.headers.get('Content-Length', '0')); require(0 <= size <= 32 * 1024 * 1024, 'faultproxy_request_exceeds_bounds')
            body = handler.rfile.read(size); require(len(body) == size, 'faultproxy_body_incomplete')
            target = urlsplit(self.protocol.origin)
            # HTTPSConnection has close(), but no context-manager protocol.
            with closing(http.client.HTTPSConnection(target.hostname, target.port or 443, timeout=40, context=self.protocol.context)) as upstream:
                upstream.request(handler.command, handler.path, body, {name: value for name, value in handler.headers.items() if name.lower() not in ['host', 'connection']})
                self.reached.set()
                result = upstream.getresponse(); data = result.read(MAX_REPLY + 1)
                require(len(data) <= MAX_REPLY, 'faultproxy_response_exceeds_bounds')
                if result.status == 200:
                    # Observe and verify the real response; never fabricate or
                    # sign a successful physical stop/creation for the caller.
                    self.verify_loss_reply(handler, body, result, data)
                    self.dropped.set(); handler.close_connection = True
                    handler.connection.shutdown(socket.SHUT_RDWR)
                    return
                handler.send_response(result.status)
                for name, value in result.getheaders():
                    if name.lower() not in ['transfer-encoding', 'connection', 'content-length']: handler.send_header(name, value)
                handler.send_header('Content-Length', str(len(data))); handler.end_headers(); handler.wfile.write(data)
        except (OSError, http.client.HTTPException, RuntimeError, ValueError, KeyError, TypeError):
            handler.close_connection = True

    def __enter__(self):
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start(); return self

    def __exit__(self, *_args):
        self.server.shutdown(); self.server.server_close(); self.thread.join(timeout=6)
        require(not self.thread.is_alive(), 'faultproxy_shutdown_unconfirmed')


def require_denied(call, code):
    try: call()
    except (RuntimeError, OSError, http.client.HTTPException): return
    raise RuntimeError(code)


def qualify(fixture, report=None):
    report = report if report is not None else {}
    protocol, parent, checks = fixture.protocol, fixture.parent, report.setdefault('checks', [])
    def phase(name): report['phase'] = name
    def passed(name): parent.assert_preserved(); checks.append(name)
    phase('create_and_fresh_probe')
    spec = fixture.desktop_spec(); lease, attempt = identity(), identity()
    result, binding = protocol.create(spec, lease, attempt)
    physical = fixture.inspect(binding)
    require(physical['Id'] == binding['container_id'] and physical['Created'] == binding['created_at']
            and physical['State']['StartedAt'] == binding['started_at'] and physical['State']['Running'], 'receipt_not_actual_docker_job')
    protocol.probe(binding); passed('actual_engine_container_created_started_binding')
    report['job_binding'] = binding
    phase('attempt_replay_denied')
    original_count = len(fixture.ledger())
    path = '/v2/desktop-sessions?lease_token=' + lease + '&attempt_id=' + attempt
    require(protocol.signed_request('POST', path, compact(spec))['status'] != 200, 'desktop_attempt_replayed')
    require(len(fixture.ledger()) == original_count, 'replay_created_reservation'); passed('same_attempt_no_create_replay')
    phase('probe_tamper_and_engine_fences')
    altered = dict(binding, engine_id='unknown-engine')
    require_denied(lambda: protocol.probe(altered), 'foreign_engine_probe_accepted')
    body = compact({'version': 1, 'challenge': identity(), 'binding': binding})
    headers = protocol.headers('POST', '/v1/desktop-session-probe', body)
    require(protocol.signed_request('POST', '/v1/desktop-session-probe', body + b' ', headers=headers)['status'] == 401,
            'tampered_signed_probe_body_accepted')
    protocol.probe(binding); passed('fresh_probe_tamper_and_foreign_engine_rejected')
    phase('actual_rfb')
    rfb = protocol.rfb(binding)
    require(rfb['width'] == result['dimensions']['width'] and rfb['height'] == result['dimensions']['height'], 'vnc_dimensions_differ')
    passed('actual_signed_rfb_upgrade_and_small_framebuffer')
    report['rfb'] = rfb
    phase('renamed_container_stop_fence')
    fixture.rename(binding)
    require_denied(lambda: protocol.probe(binding), 'renamed_container_probe_accepted')
    require_denied(lambda: protocol.stop(binding), 'renamed_container_stop_proof_accepted')
    require(parent.exists(binding['container_id']) and parent.inspect_owned(binding['container_id'])['State']['Running']
            and len(fixture.ledger()) == original_count, 'unconfirmed_stop_released_or_stopped_job')
    require_denied(lambda: protocol.stop(binding), 'unknown_stop_replayed'); passed('renamed_exact_id_and_unknown_stop_keep_charge')
    parent.remove_container(binding['container_id'])
    protocol.stop(binding); require(not fixture.ledger(), 'confirmed_absence_did_not_release_own_charge')
    passed('readonly_reconciliation_after_explicit_exact_owned_stop')
    phase('restarted_container_stop_fence')
    spec2 = fixture.desktop_spec(); _, binding2 = protocol.create(spec2, identity(), identity()); fixture.inspect(binding2)
    fixture.restart_desktop(binding2)
    require_denied(lambda: protocol.probe(binding2), 'restarted_desktop_probe_accepted')
    require_denied(lambda: protocol.stop(binding2), 'restarted_desktop_stop_proof_accepted')
    require(fixture.ledger(), 'restarted_unconfirmed_job_released')
    parent.remove_container(binding2['container_id']); protocol.stop(binding2); passed('restarted_job_started_at_fence')
    phase('lost_stop_response_and_runner_restart')
    spec3 = fixture.desktop_spec(); lease3, attempt3 = identity(), identity()
    _, binding3 = protocol.create(spec3, lease3, attempt3); fixture.inspect(binding3)
    stop_path = '/v1/desktop-sessions/' + binding3['session_id'] + '?binding_sha256=' + binding_digest(binding3)
    with ResponseLossProxy(protocol, fixture.root / 'server.pem', fixture.root / 'server-key.pem', 'DELETE', stop_path) as proxy:
        fault = RunnerProtocol(proxy.origin, fixture.ca_file, fixture.signing_key, fixture.actual_engine_id)
        require_denied(lambda: fault.stop(binding3), 'lost_stop_response_unexpectedly_confirmed')
        require(proxy.reached.is_set() and proxy.dropped.is_set(), 'stop_fault_did_not_reach_actual_runner')
    require(not parent.exists(binding3['container_id']), 'lost_response_job_not_physically_stopped')
    protocol.stop(binding3); fixture.restart(); protocol.stop(binding3)
    require(protocol.signed_request('POST', '/v2/desktop-sessions?lease_token=' + lease3 + '&attempt_id=' + attempt3,
            compact(spec3))['status'] != 200, 'restart_replayed_old_desktop_create')
    require(not fixture.ledger(), 'stopped_jobs_still_charged_after_confirmed_absence')
    passed('real_lost_stop_response_readonly_repeat_and_runner_restart')
    phase('lost_creation_response')
    lost_spec = fixture.desktop_spec(); lost_lease, lost_attempt = identity(), identity()
    create_path = '/v2/desktop-sessions?lease_token=' + lost_lease + '&attempt_id=' + lost_attempt
    with ResponseLossProxy(protocol, fixture.root / 'server.pem', fixture.root / 'server-key.pem', 'POST', create_path) as proxy:
        fault = RunnerProtocol(proxy.origin, fixture.ca_file, fixture.signing_key, fixture.actual_engine_id)
        require_denied(lambda: fault.create(lost_spec, lost_lease, lost_attempt), 'lost_creation_response_unexpectedly_confirmed')
        require(proxy.reached.is_set() and proxy.dropped.is_set(), 'creation_fault_did_not_reach_actual_runner')
    # Direct fixture inventory is operator evidence; it does not reconstruct a
    # public CreationReceipt or authorise an API to adopt its pending session.
    lost_binding = fixture.intent(lost_spec['session_id'])['binding']; fixture.inspect(lost_binding)
    held = fixture.ledger(); require(len(held) == 1, 'lost_creation_response_released_live_capacity')
    require(protocol.signed_request('POST', create_path, compact(lost_spec))['status'] != 200,
            'unknown_creation_replayed_docker_create')
    require(fixture.ledger() == held, 'unknown_creation_replay_changed_reservation')
    lost_expected = [{'attempt_id': lost_attempt, 'lease_token': lost_lease,
        'request_sha256': hashlib.sha256(compact(lost_spec)).hexdigest(), 'workspace_id': lost_spec['workspace_id']}]
    stopped = protocol.execution_stop(lost_spec['run_id'], lost_lease, lost_expected)
    require(stopped['containers'] and not parent.exists(lost_binding['container_id']) and not fixture.ledger(),
            'exact_original_lease_stop_did_not_cover_pending_desktop')
    passed('real_lost_creation_response_retains_charge_and_never_replays_create')
    phase('stop_before_delayed_create')
    run, workspace, lease, attempt = identity(), identity(), identity(), identity()
    volume = fixture.register_run(run, workspace)
    raw = {'schema_version': fixture.schema_version, 'run_id': run, 'workspace_id': workspace, 'image': 'core',
           'argv': ['/bin/bash', '-lc', 'printf forbidden > /workspace/late-create'], 'network': 'none', 'limits': limits(),
           '_execution': {'version': 1, 'lease_token': lease, 'attempt_id': attempt}}
    raw_body = compact(raw)
    expected = [{'attempt_id': attempt, 'lease_token': lease, 'request_sha256': hashlib.sha256(raw_body).hexdigest(), 'workspace_id': workspace}]
    protocol.execution_stop(run, lease, expected)
    require(protocol.signed_request('POST', '/v2/jobs', raw_body)['status'] != 200, 'delayed_create_after_stop_accepted')
    require(fixture.read_volume(volume, "import os;print(os.path.exists('/in/late-create'))").strip() == b'False', 'delayed_create_wrote_workspace')
    require(not fixture.ledger(), 'fenced_delayed_create_reserved_capacity'); passed('stop57_before_delayed_post_prevents_create_and_workspace_write')
    phase('actual_longrunning_job_cancel')
    run, workspace, lease, attempt = identity(), identity(), identity(), identity()
    volume = fixture.register_run(run, workspace)
    program = "import os,time;f=open('/workspace/counter','ab',buffering=0)\nwhile True:\n f.write(b'x');os.fsync(f.fileno());time.sleep(.05)"
    raw = {'schema_version': fixture.schema_version, 'run_id': run, 'workspace_id': workspace, 'image': 'core',
           'argv': ['python3', '-u', '-c', program], 'network': 'none', 'limits': limits(),
           '_execution': {'version': 1, 'lease_token': lease, 'attempt_id': attempt}}
    raw_body = compact(raw)
    expected = [{'attempt_id': attempt, 'lease_token': lease, 'request_sha256': hashlib.sha256(raw_body).hexdigest(), 'workspace_id': workspace}]
    reader = "import os;print(os.path.getsize('/in/counter') if os.path.exists('/in/counter') else 0)"
    with ThreadPoolExecutor(max_workers=1) as executor:
        running = executor.submit(protocol.signed_request, 'POST', '/v2/jobs', raw_body, 80)
        wait(lambda: int(fixture.read_volume(volume, reader)) >= 3, 'raw_job_not_actually_writing', 30)
        fixture.discover(); receipt = protocol.execution_stop(run, None, expected, cancel_lease=lease)
        require(receipt['containers'] and all(not parent.exists(value['container_id']) for value in receipt['containers']), 'cancel_has_live_physical_job')
        try: running.result(timeout=15)
        except (RuntimeError, OSError, http.client.HTTPException): pass
    before = fixture.read_volume(volume, reader); time.sleep(1); after = fixture.read_volume(volume, reader)
    require(before == after and int(before) >= 3 and not fixture.ledger(), 'workspace_writes_continued_after_verified_cancel')
    passed('actual_longrunning_raw_job_cancel_stops_future_workspace_writes')
    return {'checks': checks, 'rfb': rfb, 'job_binding': binding, 'images_and_binary': fixture.evidence,
            'original_containers_preserved': len(parent.baseline), 'physical_coverage': 'fixture Docker Engine only; no manager or HA qualification'}


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument('--execute', action='store_true')
    result.add_argument('--docker-client', required=True)
    result.add_argument('--docker-context', default='desktop-linux', choices=['desktop-linux'])
    result.add_argument('--baseline-file', required=True)
    runner = result.add_mutually_exclusive_group(required=True)
    runner.add_argument('--runner-image-id', type=image_id)
    runner.add_argument('--runner-binary')
    result.add_argument('--linux-docker-cli')
    result.add_argument('--expected-runner-sha256', required=True, type=sha256)
    result.add_argument('--gui-image-id', required=True, type=image_id)
    result.add_argument('--core-image-id', required=True, type=image_id)
    result.add_argument('--fixture-parent')
    result.add_argument('--runner-loopback-host', default='127.0.0.1', choices=['127.0.0.1', 'localhost'])
    result.add_argument('--max-seconds', type=int, default=480)
    result.add_argument('--runner-memory-mib', type=int, default=4096)
    return result


def main():
    global EXECUTION_DEADLINE, LAST_ACTION
    args = parser().parse_args()
    if not args.execute:
        print(json.dumps({'mode': 'source_only_plan', 'actions': False, 'runner_image': args.runner_image_id,
                          'expected_runner_sha256': args.expected_runner_sha256, 'core_image': args.core_image_id,
                          'gui_image': args.gui_image_id, 'required_original_container_count': 39})); return 0
    root = private_root(args.fixture_parent)
    require(30 <= args.max_seconds <= 900, 'qualification_deadline_bounds')
    LAST_ACTION = None
    EXECUTION_DEADLINE = time.monotonic() + args.max_seconds
    report = {'qualified': False, 'fixture_root': str(root), 'checks': [], 'phase': 'source_validation',
              'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat()}
    parent, fixture = None, None
    try:
        parent = ParentDocker(args.docker_client, args.docker_context, args.baseline_file)
        fixture = RunnerFixture(parent, root, args.runner_image_id, args.gui_image_id, args.core_image_id,
            args.expected_runner_sha256, native_binary=args.runner_binary, linux_docker_cli=args.linux_docker_cli,
            host=args.runner_loopback_host, memory_mib=args.runner_memory_mib)
        report['fixture_identity'] = fixture.identity_record
        report['images_and_binary'] = fixture.evidence
        report['phase'] = 'runner_fixture_start'
        fixture.start(); report.update(qualify(fixture, report)); report['qualified'] = True
    except Exception as error:
        report['failure_detail'] = public_failure(error)
        report['failure'] = report['failure_detail']['code']
    finally:
        report['last_action'] = LAST_ACTION
        if parent is not None: report['owned_inventory_before_cleanup'] = parent.owned_inventory(fixture.scope if fixture else None)
        EXECUTION_DEADLINE = time.monotonic() + 120
        try:
            if fixture is not None: fixture.cleanup()
            if parent is not None: parent.assert_preserved()
            report['cleanup'] = 'exact_owned_resources_removed'
        except Exception as error:
            report['qualified'] = False; report['cleanup'] = 'unconfirmed_retained_for_operator_review'
            report['cleanup_failure'] = type(error).__name__
            report['cleanup_detail'] = public_failure(error)
        if parent is not None:
            report['retained_owned_inventory'] = parent.owned_inventory(fixture.scope if fixture else None)
            report['private_network_subnets'] = dict(parent.network_subnets)
        report['finished_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        EXECUTION_DEADLINE = None
        write_private(root / 'report.json', compact(report))
        print(json.dumps({'qualified': report['qualified'], 'report': str(root / 'report.json'), 'cleanup': report['cleanup']}))
    return 0 if report['qualified'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
