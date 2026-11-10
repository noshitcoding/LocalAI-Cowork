"""Private bounded transports used only by the disposable hosted-VM harness.

Runtime manager paths are explicitly allowed; administrative requests use a
different loopback TLS listener. PostgreSQL accepts SSL negotiation only. The
readonly registry serves genuine Docker distribution manifests and blob hashes.
"""
from contextlib import contextmanager
import gzip
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import select
import socket
import socketserver
import ssl
import struct
import subprocess
import tarfile
import threading
from urllib.parse import unquote, urlsplit
import uuid

MANAGER_PATHS = frozenset('/management/runner/v1/' + name for name in
    ['instance-access', 'execution-reserve', 'execution-release', 'content-access', 'content-capability'])
MAX_HTTP_BYTES = 32 * 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024 * 1024
MAX_LAYER_BYTES = 12 * 1024 * 1024 * 1024
REGISTRY_IMAGE_ROLES = frozenset({'server', 'runner', 'provisioner', 'postgres', 'storage',
    'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy', 'source_server', 'source_runner'})
DIAGNOSTIC_COUNTER_MAX = 2**63 - 1


def require(value, code):
    if not value:
        raise RuntimeError(code)


def authorized(kind, method, target, *, bucket=None, generations=None):
    require(kind in ['admin', 'manager', 's3', 'storage_admin'], 'invalid_proxy_kind')
    if not isinstance(target, str) or len(target) > 16384:
        return False
    value = urlsplit(target)
    if value.scheme or value.netloc or value.fragment or not value.path.startswith('/'):
        return False
    if kind == 'manager':
        return method == 'POST' and target in MANAGER_PATHS
    if kind == 's3':
        allowed = generations.buckets() if generations is not None else {bucket}
        decoded = unquote(value.path)
        return method in ['GET', 'HEAD', 'PUT', 'POST', 'DELETE'] and not any(
            part in ['.', '..'] for part in decoded.split('/')) and '\\' not in decoded and any(
            isinstance(item, str) and (value.path == '/' + item or value.path.startswith('/' + item + '/'))
            for item in allowed)
    if kind == 'storage_admin':
        return method in ['GET', 'HEAD', 'PUT', 'POST', 'DELETE']
    return value.path == '/readyz' or value.path.startswith(('/api/v1/', '/management/'))


class GenerationScope:
    """A private fixture allowlist; only a scoped frozen claim can extend it."""
    def __init__(self, instance_id, host_binding):
        require(type(instance_id) is str and str(uuid.UUID(instance_id)) == instance_id,
                'invalid_generation_scope_instance')
        self.instance_id, self.host_binding = instance_id, dict(host_binding)
        self.lock = threading.Lock()
        self.rows = {None: uuid.UUID(instance_id).hex}

    def allow_operation(self, operation):
        require(operation.get('executor_host_binding') == self.host_binding
                and operation.get('instance_id') == operation['instance']['id'] == self.instance_id
                and operation.get('organisation_id') == operation['instance']['organisation_id']
                    == self.host_binding['trust_domain'], 'generation_proxy_operation_scope_mismatch')
        if operation['action'] not in ['restore', 'upgrade', 'rollback']:
            return
        generation = operation['id']; epoch = operation.get('execution_binding')
        allocation = operation.get('host_allocation', {}).get('binding', {})
        require(type(generation) is str and str(uuid.UUID(generation)) == generation and isinstance(epoch, dict)
                and epoch.get('operation_id') == generation and epoch.get('instance_id') == self.instance_id
                and epoch.get('organisation_id') == self.host_binding['trust_domain']
                and epoch.get('source_epoch') != epoch.get('candidate_epoch')
                and allocation.get('host_id') == self.host_binding['host_id']
                and set(allocation) == {'version', 'host_id', 'organisation_id', 'instance_id',
                    'allocation_revision', 'plan_sha256', 'execution_epoch'}
                and type(allocation.get('version')) is int and allocation['version'] == 1
                and type(allocation.get('allocation_revision')) is int and allocation['allocation_revision'] > 0
                and isinstance(allocation.get('plan_sha256'), str)
                and re.fullmatch('[a-f0-9]{64}', allocation['plan_sha256']) is not None
                and allocation.get('organisation_id') == self.host_binding['trust_domain']
                and allocation.get('instance_id') == self.instance_id
                and allocation.get('execution_epoch') == epoch.get('source_epoch'),
                'generation_proxy_frozen_binding_required')
        for key in ['source_epoch', 'candidate_epoch']:
            require(type(epoch.get(key)) is str and str(uuid.UUID(epoch[key])) == epoch[key],
                    'generation_proxy_frozen_binding_required')
        suffix = hashlib.sha256((self.instance_id + ':' + generation).encode()).hexdigest()[:32]
        with self.lock:
            require(generation in self.rows or len(self.rows) < 16, 'generation_proxy_inventory_exceeds_bound')
            self.rows[generation] = suffix

    def databases(self):
        with self.lock:
            return {'ci_' + suffix for suffix in self.rows.values()}

    def buckets(self):
        with self.lock:
            return {'cowork-' + suffix for suffix in self.rows.values()}

    def evidence(self):
        with self.lock:
            return [{'generation': generation, 'database': 'ci_' + suffix, 'bucket': 'cowork-' + suffix}
                    for generation, suffix in self.rows.items()]


class BoundedServer(socketserver.ThreadingMixIn):
    daemon_threads = True
    block_on_close = True

    def initialize_limit(self, maximum):
        self.slots = threading.BoundedSemaphore(maximum)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()


class TLSHTTPServer(BoundedServer, HTTPServer):
    def __init__(self, handler, context, maximum=8, diagnostics=None):
        self.context = context
        self.diagnostics = diagnostics
        self.initialize_limit(maximum)
        super().__init__(('127.0.0.1', 0), handler)

    def finish_request(self, request, client_address):
        request.settimeout(30)
        try:
            with self.context.wrap_socket(request, server_side=True) as secure:
                super().finish_request(secure, client_address)
        except (OSError, ssl.SSLError) as error:
            if self.diagnostics is not None:
                self.diagnostics.error('tls_error' if isinstance(error, ssl.SSLError) else 'socket_error')

    def handle_error(self, request, client_address):
        pass  # Socket errors must not print resolved credentials or paths.


@contextmanager
def serving(server):
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=10)


def proxy(context, upstream_port, kind, *, bucket=None, connector=None, generations=None):
    require(type(upstream_port) is int and 1024 <= upstream_port <= 65535, 'invalid_proxy_upstream')
    if kind == 's3' and generations is None:
        require(isinstance(bucket, str) and re.fullmatch('cowork-[0-9a-f]{32}', bucket), 'invalid_proxy_bucket')

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *args):
            pass

        def dispatch(self):
            self.close_connection = True
            try:
                length = int(self.headers.get('Content-Length', '0'))
                require(0 <= length <= MAX_HTTP_BYTES and 'Transfer-Encoding' not in self.headers,
                        'invalid_proxy_body')
                body = self.rfile.read(length)
                require(len(body) == length, 'incomplete_proxy_body')
                if not authorized(kind, self.command, self.path, bucket=bucket, generations=generations):
                    # Drain only the bounded request before closing TLS, so a
                    # rejected POST cannot reset away its explicit 404 response.
                    self.send_error(404)
                    return
                headers = {key: value for key, value in self.headers.items()
                           if key.lower() not in ['connection', 'proxy-authorization', 'proxy-connection']}
                headers['Connection'] = 'close'
                connection = http.client.HTTPConnection('127.0.0.1', upstream_port, timeout=30)
                if connector is not None:
                    connection.connect = lambda: setattr(connection, 'sock', connector())
                try:
                    connection.request(self.command, self.path, body=body, headers=headers)
                    response = connection.getresponse()
                    require(int(response.getheader('Content-Length', '0')) <= MAX_HTTP_BYTES,
                            'proxy_response_too_large')
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() not in ['connection', 'transfer-encoding', 'server', 'date']:
                            self.send_header(key, value)
                    self.send_header('Connection', 'close')
                    self.end_headers()
                    total = 0
                    while self.command != 'HEAD':
                        block = response.read(65536)
                        if not block:
                            break
                        total += len(block)
                        require(total <= MAX_HTTP_BYTES, 'proxy_response_too_large')
                        self.wfile.write(block)
                finally:
                    connection.close()
            except (OSError, ValueError, RuntimeError, http.client.HTTPException):
                self.close_connection = True

        do_GET = do_HEAD = do_POST = do_PUT = do_DELETE = dispatch

    return TLSHTTPServer(Handler, context)


class PostgreSQLProxy(BoundedServer, socketserver.TCPServer):
    allow_reuse_address = False

    def __init__(self, context, upstream_port, connector=None, *, database=None, generations=None):
        require(type(upstream_port) is int and 1024 <= upstream_port <= 65535, 'invalid_pg_upstream')
        self.initialize_limit(64)

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                try:
                    self.request.settimeout(10)
                    request = b''
                    while len(request) < 8:
                        block = self.request.recv(8 - len(request))
                        if not block:
                            return
                        request += block
                    if request != struct.pack('!II', 8, 80877103):
                        return
                    self.request.sendall(b'S')
                    with context.wrap_socket(self.request, server_side=True) as secure:
                        startup = b''
                        if database is not None or generations is not None:
                            def receive(count):
                                result = b''
                                while len(result) < count:
                                    block = secure.recv(count - len(result))
                                    if not block:
                                        raise OSError('pg_startup_incomplete')
                                    result += block
                                return result
                            prefix = receive(4)
                            length = struct.unpack('!I', prefix)[0]
                            require(8 <= length <= 8192, 'invalid_pg_startup')
                            startup = prefix + receive(length - 4)
                            allowed = generations.databases() if generations is not None else database
                            require(startup_database(startup, allowed), 'pg_runtime_database_scope_mismatch')
                        with (connector() if connector is not None else
                              socket.create_connection(('127.0.0.1', upstream_port), timeout=10)) as upstream:
                            secure.settimeout(120)
                            upstream.settimeout(120)
                            if startup:
                                upstream.sendall(startup)
                            while True:
                                ready = [secure] if secure.pending() else select.select([secure, upstream], [], [], 120)[0]
                                if not ready:
                                    return
                                for source in ready:
                                    block = source.recv(65536)
                                    if not block:
                                        return
                                    (upstream if source is secure else secure).sendall(block)
                except (OSError, ssl.SSLError, RuntimeError, ValueError):
                    pass

        super().__init__(('127.0.0.1', 0), Handler)

    def handle_error(self, request, client_address):
        pass


def startup_database(payload, database):
    allowed = {database} if isinstance(database, str) else database
    if (not isinstance(allowed, (set, frozenset)) or not allowed
            or any(not isinstance(name, str) or not re.fullmatch('ci_[0-9a-f]{32}', name) for name in allowed)
            or len(payload) < 9 or payload[:4] != struct.pack('!I', len(payload))
            or payload[4:8] != struct.pack('!I', 196608) or payload[-1:] != b'\0'):
        return False
    try:
        items = payload[8:].decode('utf-8').split('\0')
        if items[-2:] != ['', ''] or len(items) % 2:
            return False
        items = items[:-2]
        pairs = dict(zip(items[::2], items[1::2]))
        fixed = {'client_encoding': 'UTF8', 'DateStyle': 'ISO, MDY', 'TimeZone': 'UTC', 'extra_float_digits': '3'}
        return len(pairs) * 2 == len(items) and set(pairs) <= {'user', 'database', 'application_name', *fixed} \
            and pairs.get('database') in allowed and pairs.get('user') == pairs.get('database') \
            and all(key not in pairs or pairs[key] == value for key, value in fixed.items()) \
            and len(pairs.get('application_name', '')) <= 128
    except UnicodeError:
        return False


class PipeSocket:
    """Socket interface for one fixed docker-exec TCP channel through NAT.

    The relay receives neither credentials nor a user command. Its container ID,
    destination DNS name and port must be validated before constructing argv.
    """
    def __init__(self, argv, *, env=None):
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, bufsize=0, env=env)

    def sendall(self, content):
        view = memoryview(content)
        while view:
            written = self.process.stdin.write(view)
            if not written:
                raise OSError('relay_closed')
            view = view[written:]

    def recv(self, count):
        return self.process.stdout.read(count)

    def makefile(self, mode, *args, **kwargs):
        require(mode == 'rb', 'unsupported_relay_stream')
        import io
        return io.BufferedReader(self.process.stdout, buffer_size=65536)

    def fileno(self):
        return self.process.stdout.fileno()

    def settimeout(self, timeout):
        pass  # Relay enforces a 120-second inactivity timeout itself.

    def close(self):
        for stream in [self.process.stdin, self.process.stdout]:
            try:
                stream.close()
            except OSError:
                pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def hash_file(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class RegistryDiagnostics:
    """Finite metadata counters; never retain request targets, headers or bodies."""
    def __init__(self):
        self.lock = threading.Lock()
        self.rows = {}
        self.errors = {'tls_error': 0, 'socket_error': 0, 'response_write_error': 0}
        self.overflow = False

    def _add(self, previous, amount):
        self.overflow |= previous + amount > DIAGNOSTIC_COUNTER_MAX
        return min(DIAGNOSTIC_COUNTER_MAX, previous + amount)

    def response(self, role, method, category, status, *, written=0, count=True):
        require(role in REGISTRY_IMAGE_ROLES | {'registry', 'unmatched'} and method in {'GET', 'HEAD'}
                and category in {'v2', 'manifest', 'blob', 'unmatched'} and status in {200, 404}
                and type(written) is int and written >= 0 and type(count) is bool,
                'invalid_registry_diagnostic')
        key = (role, method, category, status)
        with self.lock:
            row = self.rows.setdefault(key, {'responses': 0, 'completed_write_bytes': 0})
            row['responses'] = self._add(row['responses'], int(count))
            row['completed_write_bytes'] = self._add(row['completed_write_bytes'], written)

    def error(self, kind):
        require(kind in self.errors, 'invalid_registry_error_diagnostic')
        with self.lock:
            self.errors[kind] = self._add(self.errors[kind], 1)

    def snapshot(self):
        with self.lock:
            return {'version': 1, 'counter_saturated': self.overflow,
                'errors': dict(self.errors), 'responses': [dict(image_role=role, method=method,
                    category=category, status=status, **values)
                    for (role, method, category, status), values in sorted(self.rows.items())]}

    @staticmethod
    def delta(before, after):
        def indexed(value):
            return {(row['image_role'], row['method'], row['category'], row['status']): row
                    for row in value['responses']}
        old = indexed(before)
        rows = []
        for key, row in indexed(after).items():
            previous = old.get(key, {})
            counts = {field: row[field] - previous.get(field, 0)
                      for field in ['responses', 'completed_write_bytes']}
            if any(counts.values()):
                rows.append(dict(image_role=key[0], method=key[1], category=key[2], status=key[3], **counts))
        return {'version': 1, 'counter_saturated': after['counter_saturated'],
                'errors': {key: value - before['errors'][key] for key, value in after['errors'].items()},
                'responses': rows}


class Registry:
    """Readonly distribution-v2 registry populated from exact local image IDs."""
    def __init__(self, root, scope):
        require(re.fullmatch('cowork-hosted-vm-[0-9a-f]{32}', scope), 'invalid_registry_scope')
        self.root, self.scope = Path(root), scope
        self.root.mkdir(mode=0o700)
        self.blobs = {}
        self.manifests = {}
        self.manifest_media = {}
        self.packages = {}
        self.layer_cache = {}
        self.total = 0
        self.diagnostics = RegistryDiagnostics()

    def import_archive(self, archive_path, image_id, name):
        require(re.fullmatch('sha256:[0-9a-f]{64}', image_id)
                and re.fullmatch('[a-z_]{1,32}', name), 'invalid_registry_image')
        require(Path(archive_path).stat().st_size <= MAX_ARCHIVE_BYTES, 'image_archive_too_large')
        with tarfile.open(archive_path, 'r:') as archive:
            members = archive.getmembers()
            require(len(members) <= 2048, 'too_many_image_members')
            require(all(member.isdir() or member.isfile() for member in members), 'unsafe_image_member')
            require(len({member.name for member in members}) == len(members), 'duplicate_image_member')
            require(archive.getmember('manifest.json').size <= 2 * 1024 * 1024, 'image_manifest_too_large')
            if any(member.name == 'index.json' for member in members):
                return self.import_oci(archive, image_id, name)
            manifests = json.load(archive.extractfile('manifest.json'))
            require(isinstance(manifests, list) and len(manifests) == 1, 'single_image_archive_required')
            entry = manifests[0]
            config_member = archive.getmember(entry['Config'])
            require(config_member.isfile() and config_member.size <= 2 * 1024 * 1024, 'invalid_image_config')
            config = archive.extractfile(config_member).read()
            require('sha256:' + hashlib.sha256(config).hexdigest() == image_id, 'image_config_identity_mismatch')
            document = json.loads(config)
            require(document['os'] == 'linux' and document['architecture'] == 'amd64', 'unsupported_image_platform')
            differences = document['rootfs']['diff_ids']
            require(len(differences) == len(entry['Layers']) <= 128, 'invalid_image_layers')
            require(all(isinstance(value, str) and re.fullmatch('sha256:[0-9a-f]{64}', value) for value in differences), 'invalid_image_layer_identity')
            config_digest = self.store_bytes(config)
            layers = []
            for source, difference in zip(entry['Layers'], differences):
                member = archive.getmember(source)
                require(member.isfile() and member.size <= MAX_LAYER_BYTES, 'image_layer_too_large')
                if difference not in self.layer_cache:
                    temporary = self.root / (hashlib.sha256(difference.encode()).hexdigest() + '.tmp')
                    require(not temporary.exists(), 'registry_temporary_collision')
                    checksum = hashlib.sha256()
                    with temporary.open('xb') as output:
                        with gzip.GzipFile(filename='', mode='wb', fileobj=output, compresslevel=1, mtime=0) as compressed:
                            stream = archive.extractfile(member)
                            while True:
                                block = stream.read(1024 * 1024)
                                if not block:
                                    break
                                checksum.update(block)
                                compressed.write(block)
                    require('sha256:' + checksum.hexdigest() == difference, 'image_layer_identity_mismatch')
                    compressed_digest = 'sha256:' + hash_file(temporary)
                    destination = self.root / compressed_digest.split(':')[1]
                    if destination.exists():
                        temporary.unlink()
                    else:
                        temporary.replace(destination)
                    self.add_blob(compressed_digest, destination)
                    self.layer_cache[difference] = compressed_digest
                digest = self.layer_cache[difference]
                layers.append({'mediaType': 'application/vnd.docker.image.rootfs.diff.tar.gzip',
                               'size': self.blobs[digest].stat().st_size, 'digest': digest})
            manifest = json.dumps({'schemaVersion': 2,
                'mediaType': 'application/vnd.docker.distribution.manifest.v2+json',
                'config': {'mediaType': 'application/vnd.docker.container.image.v1+json',
                           'size': len(config), 'digest': config_digest}, 'layers': layers},
                separators=(',', ':')).encode()
            digest = 'sha256:' + hashlib.sha256(manifest).hexdigest()
            repository = self.scope + '/' + name
            self.manifests[(repository, digest)] = manifest
            self.manifest_media[(repository, digest)] = 'application/vnd.docker.distribution.manifest.v2+json'
            self.packages[name] = {'source_image_id': image_id, 'config_digest': config_digest,
                                   'rootfs_diff_ids': differences, 'manifest_bytes_preserved': False}
            return repository, digest

    def import_oci(self, archive, image_id, name):
        """Preserve containerd's original index, manifests and compressed blobs."""
        require(archive.getmember('index.json').size <= 2 * 1024 * 1024, 'image_index_too_large')
        index = json.load(archive.extractfile('index.json'))
        entries = index.get('manifests', [])
        roots = [value for value in entries if value.get('digest') == image_id]
        require(len(roots) == 1, 'oci_source_image_identity_mismatch')
        repository = self.scope + '/' + name
        visited, packages = set(), []
        index_types = {'application/vnd.oci.image.index.v1+json', 'application/vnd.docker.distribution.manifest.list.v2+json'}
        image_types = {'application/vnd.oci.image.manifest.v1+json', 'application/vnd.docker.distribution.manifest.v2+json'}

        def blob(descriptor, maximum=MAX_LAYER_BYTES):
            digest, size = descriptor.get('digest'), descriptor.get('size')
            require(isinstance(digest, str) and re.fullmatch('sha256:[0-9a-f]{64}', digest)
                    and type(size) is int and 0 <= size <= maximum, 'invalid_oci_blob_descriptor')
            member = archive.getmember('blobs/sha256/' + digest.split(':')[1])
            require(member.isfile() and member.size == size, 'oci_blob_size_mismatch')
            source = archive.extractfile(member)
            temporary = self.root / (digest.split(':')[1] + '.oci-tmp')
            require(not temporary.exists(), 'registry_temporary_collision')
            checksum = hashlib.sha256()
            with temporary.open('xb') as output:
                while block := source.read(1024 * 1024):
                    checksum.update(block); output.write(block)
            require('sha256:' + checksum.hexdigest() == digest, 'oci_blob_identity_mismatch')
            destination = self.root / digest.split(':')[1]
            if destination.exists():
                temporary.unlink()
            else:
                temporary.replace(destination)
            self.add_blob(digest, destination)
            return destination

        def manifest(descriptor, depth=0):
            require(depth <= 4 and len(visited) < 128, 'oci_manifest_graph_too_large')
            digest, media = descriptor['digest'], descriptor['mediaType']
            require(media in index_types | image_types, 'unsupported_oci_manifest_media')
            path = blob(descriptor, 2 * 1024 * 1024)
            if digest in visited:
                return
            visited.add(digest)
            content = path.read_bytes()
            document = json.loads(content)
            require(document.get('schemaVersion') == 2 and document.get('mediaType') == media,
                    'oci_manifest_media_mismatch')
            self.manifests[(repository, digest)] = content
            self.manifest_media[(repository, digest)] = media
            if media in index_types:
                children = document.get('manifests', [])
                require(isinstance(children, list) and 0 < len(children) <= 16, 'invalid_oci_index')
                for child in children:
                    manifest(child, depth + 1)
            else:
                config_path = blob(document['config'], 2 * 1024 * 1024)
                config = json.loads(config_path.read_bytes())
                layers = document.get('layers', [])
                require(isinstance(layers, list) and len(layers) <= 128, 'invalid_oci_layers')
                for layer in layers:
                    blob(layer)
                if config.get('os') == 'linux' and config.get('architecture') == 'amd64':
                    differences = config.get('rootfs', {}).get('diff_ids', [])
                    require(isinstance(differences, list) and len(differences) == len(layers)
                            and all(re.fullmatch('sha256:[0-9a-f]{64}', value) for value in differences),
                            'invalid_oci_rootfs_identity')
                    packages.append({'source_image_id': image_id, 'config_digest': document['config']['digest'],
                                     'rootfs_diff_ids': differences, 'platform_manifest_digest': digest,
                                     'manifest_bytes_preserved': True})
        manifest(roots[0])
        require(len(packages) == 1, 'single_linux_amd64_package_required')
        self.packages[name] = packages[0]
        return repository, image_id

    def add_blob(self, digest, path):
        if digest not in self.blobs:
            self.total += path.stat().st_size
        require(self.total <= MAX_ARCHIVE_BYTES, 'registry_size_limit_exceeded')
        self.blobs[digest] = path

    def store_bytes(self, content):
        digest = 'sha256:' + hashlib.sha256(content).hexdigest()
        path = self.root / digest.split(':')[1]
        if not path.exists():
            with path.open('xb') as stream:
                stream.write(content)
        self.add_blob(digest, path)
        return digest

    def server(self, context):
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def dispatch(self):
                role, category = 'registry', 'v2'
                if self.path == '/v2/':
                    content, media, digest = b'{}', 'application/json', None
                else:
                    match = re.fullmatch(r'/v2/(' + re.escape(owner.scope) +
                        r'/[a-z_]{1,32})/(manifests|blobs)/(sha256:[0-9a-f]{64})', self.path)
                    if not match:
                        owner.diagnostics.response('unmatched', self.command, 'unmatched', 404)
                        self.send_error(404)
                        return
                    repository, category, digest = match.groups()
                    role = repository.rsplit('/', 1)[1]
                    role = role if role in REGISTRY_IMAGE_ROLES else 'unmatched'
                    category = 'manifest' if category == 'manifests' else 'blob'
                    if category == 'manifest':
                        content = owner.manifests.get((repository, digest))
                        media = owner.manifest_media.get((repository, digest))
                    else:
                        # A blob is accessible only through a registered image repository.
                        content = owner.blobs.get(digest) if any(repo == repository for repo, _ in owner.manifests) else None
                        media = 'application/octet-stream'
                    if content is None:
                        owner.diagnostics.response(role, self.command, category, 404)
                        self.send_error(404)
                        return
                size = len(content) if isinstance(content, bytes) else content.stat().st_size
                owner.diagnostics.response(role, self.command, category, 200)
                self.send_response(200)
                self.send_header('Docker-Distribution-Api-Version', 'registry/2.0')
                self.send_header('Content-Type', media)
                self.send_header('Content-Length', str(size))
                if digest:
                    self.send_header('Docker-Content-Digest', digest)
                self.end_headers()
                if self.command == 'GET':
                    try:
                        if isinstance(content, bytes):
                            self.wfile.write(content)
                            owner.diagnostics.response(role, self.command, category, 200, written=len(content), count=False)
                        else:
                            with content.open('rb') as stream:
                                while block := stream.read(1024 * 1024):
                                    self.wfile.write(block)
                                    owner.diagnostics.response(role, self.command, category, 200, written=len(block), count=False)
                    except OSError:
                        owner.diagnostics.error('response_write_error')
                        raise

            do_GET = do_HEAD = dispatch

        return TLSHTTPServer(Handler, context, maximum=4, diagnostics=self.diagnostics)
