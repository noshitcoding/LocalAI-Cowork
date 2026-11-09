"""Explicit opt-in dedicated Compose daemon, pinned to one trust domain.

The daemon client certificate is an administrator credential. This transport is
not an authorization proxy or a substitute for a physically isolated host/VM.
"""
import json
import os
from pathlib import Path
import re
import secrets
import ssl
import uuid
from urllib.parse import urlsplit


class HostError(Exception):
    pass


def check(condition, code):
    if not condition:
        raise HostError(code)


def settings(config):
    value = config.get('executor_host')
    if value is None:
        # Never reinterpret daemon selection from ambient operator credentials.
        check(not config.get('docker_host') and not config.get('docker_endpoint')
              and not config.get('runner_docker_host'), 'executor_host_explicit_configuration_required')
        return None
    keys = {'id', 'trust_domain', 'operator_state_id', 'endpoint', 'daemon_id', 'ca_file', 'cert_file', 'key_file'}
    check(config.get('backend') == 'compose' and isinstance(value, dict) and set(value) == keys,
          'invalid_executor_host_configuration')
    check(config.get('fleet_host_budget') is not None, 'executor_host_resource_budget_required')
    check(all(isinstance(value[key], str) for key in keys), 'invalid_executor_host_configuration')
    try:
        for key in ['id', 'trust_domain', 'operator_state_id']:
            identity = uuid.UUID(value[key])
            check(str(identity) == value[key] and identity.int != 0, 'invalid_executor_host_identity')
        endpoint = urlsplit(value['endpoint'])
        check(endpoint.scheme == 'tcp' and endpoint.port is not None and 1024 <= endpoint.port <= 65535
              and re.fullmatch('[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?', endpoint.hostname or '')
              and not endpoint.username and not endpoint.password and not endpoint.path
              and not endpoint.query and not endpoint.fragment, 'executor_host_requires_tls_endpoint')
        check(isinstance(value['daemon_id'], str) and re.fullmatch('[a-zA-Z0-9:_.-]{12,128}', value['daemon_id']), 'invalid_executor_daemon_identity')
    except (TypeError, ValueError):
        raise HostError('invalid_executor_host_configuration') from None
    return dict(value)


def manager_binding(config):
    if config.get('backend')=='kubernetes':
        import backend_profile
        return backend_profile.manager_binding(config)
    value = settings(config)
    if value is None:
        return None
    return {'version': 1, 'host_id': value['id'], **{key: value[key]
            for key in ['trust_domain', 'daemon_id', 'operator_state_id']}}


def operation_binding(operation):
    """Validate manager-frozen identity before any receipt or journal is accepted."""
    value = operation.get('executor_host_binding')
    selected = operation['instance'].get('executor_host_id')
    if value is None:
        check(selected is None, 'executor_operation_binding_required')
        return None
    keys = {'version', 'host_id', 'trust_domain', 'daemon_id', 'operator_state_id'}
    check(isinstance(value, dict) and set(value) == keys and type(value['version']) is int
          and value['version'] == 1, 'invalid_executor_operation_binding')
    try:
        for key in ['host_id', 'trust_domain', 'operator_state_id']:
            identity = uuid.UUID(value[key])
            check(str(identity) == value[key] and identity.int != 0, 'invalid_executor_operation_identity')
    except (TypeError, ValueError, AttributeError):
        raise HostError('invalid_executor_operation_identity') from None
    check(isinstance(value['daemon_id'], str) and re.fullmatch('[a-zA-Z0-9:_.-]{12,128}', value['daemon_id']),
          'invalid_executor_operation_identity')
    check(selected == value['host_id'] and operation['organisation_id'] == value['trust_domain']
          and operation['instance']['organisation_id'] == value['trust_domain'], 'executor_operation_scope_mismatch')
    return dict(value)


def validate_receipt(operation, receipt):
    check(isinstance(receipt, dict) and receipt.get('executor_host_binding') == operation_binding(operation),
          'executor_host_receipt_binding_mismatch')


def attach_receipt(operation, receipt):
    value = operation_binding(operation)
    check('executor_host_binding' not in receipt or receipt['executor_host_binding'] == value,
          'executor_host_receipt_binding_mismatch')
    return {**receipt, **({'executor_host_binding': value} if value is not None else {})}


def operator_secrets(config, operation):
    """A response-only envelope; no global manager secret is transmitted."""
    value = operation.get('operator_secrets')
    binding = operation_binding(operation)
    check(binding == manager_binding(config), 'executor_manager_operator_binding_mismatch')
    if binding is None:
        check(value is None, 'unexpected_operator_secrets')
        return None
    import execution_generation as epochs
    try:
        generation = epochs.binding(operation)
    except epochs.ExecutionGenerationError:
        raise HostError('executor_operator_epoch_scope_mismatch') from None
    check(generation is not None or operation['instance'].get('execution_required') is not True,
          'executor_operator_epoch_required')
    expected = {'version': 2 if generation is not None else 1, 'operation_id': operation['id'], 'lease_token': operation['lease_token'],
                'organisation_id': operation['organisation_id'], 'instance_id': operation['instance_id'],
                'executor_host_binding': binding}
    if generation is not None:
        expected['execution_epoch'] = generation['candidate_epoch']
    check(isinstance(value, dict) and set(value) == {*expected, 'fleet_key'} and type(value['version']) is int
          and all(value.get(key) == selected for key, selected in expected.items()), 'executor_operator_secret_scope_mismatch')
    check(isinstance(value['fleet_key'], str) and re.fullmatch('[0-9a-f]{64}', value['fleet_key']),
          'invalid_executor_instance_key')
    return dict(value)


class ExecutorHost:
    def __init__(self, infrastructure, command):
        self.infra, self.command = infrastructure, command
        self.value = settings(infrastructure.config)
        self.kube_binding=manager_binding(infrastructure.config) if infrastructure.config.get('backend')=='kubernetes' else None
        self.binding = None if self.value is None else {key: self.value[key] for key in ['id', 'trust_domain', 'operator_state_id', 'endpoint', 'daemon_id']}
        if self.kube_binding is not None:
            self.binding={'id':self.kube_binding['host_id'],'trust_domain':self.kube_binding['trust_domain'],
                          'operator_state_id':self.kube_binding['operator_state_id'],'daemon_id':self.kube_binding['daemon_id'],'backend':'kubernetes'}
        if self.value is not None:
            self.validate_secrets()

    def validate_secrets(self):
        for key in ['ca_file', 'cert_file', 'key_file']:
            path = Path(self.value[key])
            check(path.is_absolute() and path.is_file() and not path.is_symlink()
                  and all(not parent.is_symlink() for parent in path.parents), 'unsafe_executor_tls_file')
            check(path.stat().st_size <= 1048576, 'unsafe_executor_tls_file')
            if os.name == 'posix':
                check(path.stat().st_uid == os.geteuid() and path.stat().st_mode & 0o022 == 0
                      and (key != 'key_file' or path.stat().st_mode & 0o077 == 0), 'unsafe_executor_tls_permissions')
        try:
            context = ssl.create_default_context(cafile=self.value['ca_file'])
            context.load_cert_chain(self.value['cert_file'], self.value['key_file'])
        except (OSError, ssl.SSLError):
            raise HostError('invalid_executor_tls_credentials') from None

    def arguments(self, args):
        check(self.kube_binding is None,'kubernetes_backend_cannot_use_docker_fallback')
        check(args[0] == 'docker' and not any(arg in ['--host', '-H', '--context', '--tls', '--tlsverify',
              '--tlscacert', '--tlscert', '--tlskey'] or arg.startswith(('--host=', '--context=', '--tls'))
              for arg in args[1:]), 'executor_daemon_override_forbidden')
        if self.value is None:
            return args
        return ['docker', '--host', self.value['endpoint'], '--tlsverify', '--tlscacert', self.value['ca_file'],
                '--tlscert', self.value['cert_file'], '--tlskey', self.value['key_file'], *args[1:]]

    def environment(self, env):
        value = dict(os.environ if env is None else env)
        for key in ['DOCKER_HOST', 'DOCKER_CONTEXT', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH', 'DOCKER_TLS',
                    'DOCKER_API_VERSION']:
            value.pop(key, None)
        # Explicit local daemon even if ~/.docker/config.json selects a remote context.
        if self.value is None:
            value['DOCKER_HOST'] = 'unix:///var/run/docker.sock'
        return value

    def verify(self):
        if self.value is None:
            return
        self.validate_secrets()
        try:
            info = json.loads(self.command(self.arguments(['docker', 'info', '--format', '{{json .}}']),
                env=self.environment(None), timeout=30))
            check(info.get('ID') == self.value['daemon_id'], 'executor_daemon_identity_mismatch')
            labels = info.get('Labels') or []
            check([label for label in labels if label.startswith('cowork.role=')] == ['cowork.role=executor']
                  and [label for label in labels if label.startswith('cowork.trust-domain=')]
                  == ['cowork.trust-domain=' + self.value['trust_domain']],
                  'executor_trust_domain_mismatch')
            capacity = self.infra.config['fleet_host_budget']
            check(type(info.get('NCPU')) is int and type(info.get('MemTotal')) is int
                  and capacity['cpu_millis'] <= info['NCPU'] * 1000
                  and capacity['memory_mib'] <= info['MemTotal'] // (1024 * 1024), 'executor_host_resource_capacity_mismatch')
        except (ValueError, TypeError):
            raise HostError('executor_daemon_identity_unproven') from None

    def bind(self, directory=None):
        target = (self.infra.root if directory is None else directory) / 'executor-host.json'
        expected = {'version': 1, 'host': self.binding}
        check(not target.is_symlink(), 'unsafe_executor_host_binding')
        if target.exists():
            try:
                actual = json.loads(target.read_text())
            except (ValueError, OSError):
                raise HostError('invalid_executor_host_binding') from None
            check(actual == expected, 'executor_host_migration_requires_review')
        elif self.binding is not None:
            # Never attach existing local operations or workloads to a remote host.
            legacy = any(path.name != 'operator.lock' and path.name != 'executor-host.json'
                         for path in (self.infra.root if directory is None else directory).iterdir())
            check(not legacy, 'executor_host_legacy_state_requires_review')
            self.infra.private_write(target, json.dumps(expected, sort_keys=True).encode())

    def prepare(self, instance, directory, action):
        if self.kube_binding is not None:
            check(instance['backend']=='kubernetes' and instance['id']==self.infra.config['backend_profile']['specification']['instance_id']
                  and instance['organisation_id']==self.kube_binding['trust_domain'],'kubernetes_operator_assignment_differs')
            self.bind(directory)
            return
        self.bind()
        if self.value is None:
            return
        check(instance.get('backend') == 'compose' and instance.get('organisation_id') == self.value['trust_domain'],
              'executor_instance_trust_domain_mismatch')
        import backend_profile
        check(instance['policy']['minimum_isolation'] == 'container' or backend_profile.vm_instance(self.infra.config,instance), 'executor_vm_isolation_not_qualified')
        # Staging files and migrations must never accidentally use local mounts.
        if action not in ['provision', 'start', 'resume', 'stop', 'suspend', 'delete']:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.instance(self.infra.config, instance, action)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise HostError(str(error)) from None
        self.bind(directory)
        self.verify()

    def prepare_operation(self, operation, directory):
        check(operation_binding(operation) == manager_binding(self.infra.config),
              'executor_manager_operator_binding_mismatch')
        self.prepare(operation['instance'], directory, operation['action'])
        import backend_profile
        if self.value is not None and operation['action'] in ['backup', 'restore', 'upgrade', 'rollback'] and not backend_profile.vm_instance(self.infra.config,operation['instance']):
            import execution_generation
            import host_capacity_attestation
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.operation(self.infra, operation)
            except (remote_lifecycle_qualification.LifecycleError, execution_generation.ExecutionGenerationError,
                    host_capacity_attestation.CapacityError) as error:
                raise HostError(str(error)) from None

    def verify_shared_state(self, image):
        if self.value is None:
            return
        self.bind()
        # Docker bind sources exist on the daemon host, not on the CLI host.
        # Prove the exact root is the same operator-controlled filesystem before
        # any remote Compose secrets/configs can be mounted.
        nonce = secrets.token_hex(32)
        marker = self.infra.root / ('qualification-' + secrets.token_hex(16))
        self.infra.private_write(marker, nonce.encode())
        try:
            # Core images deliberately default to an unprivileged sandbox UID.
            # The nonce is operator-private (0600), so this trusted helper must
            # read as its owner; do not weaken the file or add DAC capabilities.
            operator_uid = str(os.geteuid()) if os.name == 'posix' else '0'
            args = ['docker', 'run', '--rm', '--pull', 'never', '--user', operator_uid, '--network', 'none', '--read-only', '--cap-drop', 'ALL',
                    '--security-opt', 'no-new-privileges:true', '--memory', '64m', '--memory-swap', '64m', '--cpus', '0.1', '--pids-limit', '16',
                    '--mount', 'type=bind,src=' + str(self.infra.root) + ',dst=/qualification,readonly',
                    '--entrypoint', '/bin/cat', image, '/qualification/' + marker.name]
            result = self.infra.run(args, timeout=30)
            check(result == nonce.encode(), 'executor_shared_state_unproven')
        finally:
            marker.unlink(missing_ok=True)
