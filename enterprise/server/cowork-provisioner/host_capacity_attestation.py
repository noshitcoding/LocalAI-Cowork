"""Opt-in, daemon-observed Host55 capacity coverage. Never releases a charge.

The manager owns the DB-clock challenge and coverage deadline. Local monotonic
time only limits collection and schedules renewal; it never grants admission.
Unknown observations are reported as uncertainty, not adopted as allocations.
"""
import copy
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import threading
import time
import uuid

import execution_generation
import executor_host
import resource_budget


class CapacityError(Exception):
    pass


def check(value, code):
    if not value:
        raise CapacityError(code)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def identity(value):
    try:
        check(type(value) is str and str(uuid.UUID(value)) == value and uuid.UUID(value).int != 0,
              'invalid_host_capacity_identity')
    except (ValueError, TypeError, AttributeError):
        raise CapacityError('invalid_host_capacity_identity') from None
    return value


def settings(config):
    value = config.get('host_capacity_attestation')
    if value is None:
        return None
    check(isinstance(value, dict) and set(value) == {'version', 'host_revision'}
          and type(value['version']) is int and value['version'] == 1
          and type(value['host_revision']) is int and value['host_revision'] > 0,
          'invalid_host_capacity_configuration')
    check(executor_host.manager_binding(config) is not None and config.get('execution_admission') is not None,
          'host_capacity_requires_scoped_execution_operator')
    return dict(value)


def private_configuration(path):
    path = Path(path)
    check(os.name == 'posix' and path.is_absolute() and path.is_file() and not path.is_symlink()
          and all(not parent.is_symlink() for parent in path.parents), 'private_host_capacity_configuration_required')
    info = path.stat()
    check(info.st_uid == os.geteuid() and info.st_nlink == 1 and not info.st_mode & 0o077
          and info.st_size <= 65536, 'private_host_capacity_configuration_required')


def plan(value):
    check(isinstance(value, dict) and set(value) == {'version', 'instance_id', 'services', 'sandbox', 'total'}
          and type(value['version']) is int and value['version'] == 1, 'invalid_host_allocation_plan')
    identity(value['instance_id'])
    check(isinstance(value['services'], dict) and set(value['services']) in [set(resource_budget.DEFAULT_SERVICES),set(resource_budget.DEFAULT_SERVICES)|set(resource_budget.DATA_SERVICES)]
          and isinstance(value['sandbox'], dict) and set(value['sandbox']) == {'cpu_millis', 'memory_mib', 'concurrent_runs'},
          'invalid_host_allocation_plan')
    try:
        expected = resource_budget.instance_budget({'instance_service_limits': value['services']},
            {'id': value['instance_id'], 'policy': {'limits': value['sandbox']}})
    except resource_budget.BudgetError:
        raise CapacityError('invalid_host_allocation_plan') from None
    check(expected == value, 'invalid_host_allocation_plan')
    return copy.deepcopy(value)


def operation_allocation(operation):
    value = operation.get('host_allocation')
    if value is None:
        return None
    host = executor_host.operation_binding(operation)
    check(host is not None and isinstance(value, dict) and set(value) == {'binding', 'plan'},
          'invalid_operation_host_allocation')
    binding = value['binding']
    check(isinstance(binding, dict) and set(binding) == {'version', 'host_id', 'organisation_id', 'instance_id',
          'allocation_revision', 'plan_sha256', 'execution_epoch'} and type(binding['version']) is int
          and binding['version'] == 1 and type(binding['allocation_revision']) is int and binding['allocation_revision'] > 0,
          'invalid_operation_host_allocation')
    for key in ['host_id', 'organisation_id', 'instance_id', 'execution_epoch']:
        identity(binding[key])
    initial = plan(value['plan'])
    check(binding['host_id'] == host['host_id'] and binding['organisation_id'] == host['trust_domain']
          and binding['instance_id'] == operation['instance_id'] == operation['instance']['id'] == initial['instance_id']
          and binding['execution_epoch'] == operation['instance'].get('execution_epoch')
          and binding['plan_sha256'] == digest(initial), 'operation_host_allocation_scope_mismatch')
    return copy.deepcopy(value)


def attach_receipt(operation, receipt):
    allocation = operation_allocation(operation)
    binding = None if allocation is None else allocation['binding']
    check('host_allocation' not in receipt or receipt['host_allocation'] == binding,
          'host_allocation_receipt_binding_mismatch')
    return {**receipt, **({'host_allocation': binding} if binding is not None else {})}


def validate_receipt(operation, receipt):
    allocation = operation_allocation(operation)
    check(receipt.get('host_allocation') == (None if allocation is None else allocation['binding']),
          'host_allocation_receipt_binding_mismatch')


def document(path, maximum=1048576):
    check(path.is_file() and not path.is_symlink() and path.stat().st_nlink == 1
          and path.stat().st_size <= maximum and all(not parent.is_symlink() for parent in path.parents),
          'unsafe_host_capacity_evidence')
    try:
        return json.loads(path.read_text(encoding='utf-8'))
    except (ValueError, OSError):
        raise CapacityError('invalid_host_capacity_evidence') from None


def challenge(value, config):
    configured = settings(config)
    check(configured is not None and isinstance(value, dict) and set(value) == {'version', 'challenge_id', 'expires_at',
          'executor_host_binding', 'host_revision', 'allocation_revision', 'capacity', 'service_plan', 'allocations'}
          and type(value['version']) is int and value['version'] == 1, 'invalid_host_capacity_challenge')
    identity(value['challenge_id'])
    check(value['executor_host_binding'] == executor_host.manager_binding(config)
          and type(value['host_revision']) is int and value['host_revision'] == configured['host_revision']
          and type(value['allocation_revision']) is int and value['allocation_revision'] >= 0,
          'host_capacity_challenge_scope_changed')
    check(value['capacity'] == resource_budget.host_budget(config)
          and value['service_plan'] == resource_budget.service_caps(config), 'host_capacity_operator_plan_mismatch')
    try:
        check(isinstance(value['expires_at'], str) and len(value['expires_at']) <= 64
              and datetime.fromisoformat(value['expires_at'].replace('Z', '+00:00')).utcoffset() is not None,
              'invalid_host_capacity_challenge')
    except ValueError:
        raise CapacityError('invalid_host_capacity_challenge') from None
    rows = value['allocations']
    check(isinstance(rows, list) and len(rows) <= 10000, 'invalid_host_capacity_challenge')
    ids = set()
    for row in rows:
        check(isinstance(row, dict) and set(row) == {'instance_id', 'allocation_revision', 'execution_epoch',
              'instance_state', 'plan', 'plan_sha256', 'expected_resource_budget', 'expected_manifest_sha256'},
              'invalid_host_capacity_challenge')
        identity(row['instance_id']); identity(row['execution_epoch'])
        check(row['instance_id'] not in ids and type(row['allocation_revision']) is int and row['allocation_revision'] > 0
              and isinstance(row['instance_state'], str), 'invalid_host_capacity_challenge')
        ids.add(row['instance_id'])
        initial, expected = plan(row['plan']), plan(row['expected_resource_budget'])
        check(initial['instance_id'] == expected['instance_id'] == row['instance_id']
              and initial['services'] == expected['services'] == value['service_plan']
              and row['plan_sha256'] == digest(initial)
              and all(expected['sandbox'][key] <= initial['sandbox'][key] for key in initial['sandbox'])
              and (row['expected_manifest_sha256'] is None or isinstance(row['expected_manifest_sha256'], str)
                   and re.fullmatch('[a-f0-9]{64}', row['expected_manifest_sha256'])), 'host_capacity_allocation_plan_mismatch')
    return copy.deepcopy(value)


def inspect_caps(container):
    caps = container.get('HostConfig')
    check(isinstance(caps, dict) and all(type(caps.get(key)) is int and caps[key] > 0
          for key in ['NanoCpus', 'Memory', 'PidsLimit']), 'host_capacity_container_unbounded')
    check(not caps.get('Privileged') and caps.get('ReadonlyRootfs') is True
          and {str(value).upper() for value in caps.get('CapDrop') or []} == {'ALL'}
          and not caps.get('CapAdd') and 'no-new-privileges:true' in (caps.get('SecurityOpt') or []),
          'host_capacity_container_security_mismatch')
    return caps


def inventory(infrastructure, selected, info, containers):
    """Pure bounded inventory classification except for owned local evidence reads."""
    host = executor_host.manager_binding(infrastructure.config)
    check(info.get('ID') == host['daemon_id'] and type(info.get('NCPU')) is int and info['NCPU'] > 0
          and type(info.get('MemTotal')) is int and info['MemTotal'] > 0
          and [label for label in info.get('Labels') or [] if label.startswith('cowork.role=')] == ['cowork.role=executor']
          and [label for label in info.get('Labels') or [] if label.startswith('cowork.trust-domain=')]
          == ['cowork.trust-domain=' + host['trust_domain']], 'host_capacity_daemon_changed')
    check(isinstance(containers, list) and len(containers) <= 20000, 'host_capacity_inventory_exceeds_bound')
    expected = {row['instance_id']: row for row in selected['allocations']}
    by_instance = {key: [] for key in expected}
    unknown = False
    seen = set()
    for container in containers:
        name = container.get('Name', '') if isinstance(container, dict) else ''
        if isinstance(container,dict) and container.get('Config',{}).get('Labels',{}).get('openshell.ai/sandbox-id'):
            import openshell_environment
            owners=[]
            for instance_id in expected:
                try:openshell_environment.capacity(infrastructure,container,instance_id);owners.append(instance_id)
                except (RuntimeError,KeyError,ValueError,OSError,StopIteration):pass
            if len(owners)!=1 or container.get('Id') in seen:unknown=True;continue
            seen.add(container['Id']);by_instance[owners[0]].append(('openshell-original-'+container['Id'],container));continue
        match = re.fullmatch(r'/cowork-([a-f0-9]{32})-(.+)', name)
        ident = container.get('Id') if isinstance(container, dict) else None
        if not match or not isinstance(ident, str) or not re.fullmatch('[a-f0-9]{64}', ident) or ident in seen:
            unknown = True; continue
        seen.add(ident)
        instance = str(uuid.UUID(match[1]))
        if instance not in by_instance:
            unknown = True; continue
        by_instance[instance].append((match[2], container))
    for path in infrastructure.root.iterdir():
        if re.fullmatch('[a-f0-9]{32}', path.name):
            if path.is_symlink() or not path.is_dir():
                unknown = True; continue
            if str(uuid.UUID(path.name)) not in expected:
                # A locally verified tombstone has no retained resource budget;
                # actual containers are independently rejected above.
                try:
                    marker = document(path / 'deleted.json')
                    check(marker.get('deletion_verified') is True and marker.get('instance_id') == str(uuid.UUID(path.name))
                          and not (path / 'compose.json').exists() and not (path / 'resource-budget.json').exists()
                          and not (path / 'generations').exists(), 'host_capacity_unregistered_instance')
                except (CapacityError, OSError):
                    unknown = True
    reports = []
    for instance, row in expected.items():
        report = {'instance_id': instance, 'allocation_revision': row['allocation_revision'],
                  'state': 'unknown', 'execution_epoch': row['execution_epoch'], 'manifest_sha256': None, 'resource_budget': None}
        directory = infrastructure.root / uuid.UUID(instance).hex
        owned = by_instance[instance]
        try:
            if row['instance_state'] == 'pending' and row['expected_manifest_sha256'] is None:
                import instance_disk_quotas
                check(instance_disk_quotas.pending_data_services(infrastructure,instance,owned,row['plan']['services']) and not (directory / 'compose.json').exists() and not (directory / 'resource-budget.json').exists()
                      and not (directory / 'active.json').exists() and not (directory / 'generations').exists(),
                      'host_capacity_pending_resources_uncommitted')
                report['state'] = 'reserved'
            else:
                check(directory.is_dir() and not directory.is_symlink(), 'host_capacity_instance_evidence_missing')
                resource_budget.generations(directory)
                manifest = document(directory / 'compose.json')
                budget = plan(document(directory / 'resource-budget.json'))
                check(resource_budget.manifest_budget(manifest, instance) == budget == row['expected_resource_budget']
                      and digest(manifest) == row['expected_manifest_sha256']
                      and manifest.get('name') == 'cowork-' + uuid.UUID(instance).hex,
                      'host_capacity_manifest_changed')
                active = infrastructure.active_directory(directory)
                check(execution_generation.read({'id': instance, 'organisation_id': host['trust_domain']}, active)
                      == row['execution_epoch'], 'host_capacity_epoch_changed')
                for service in ['api', 'worker']:
                    environment = manifest['services'][service]['environment']
                    check(environment.get('COWORK_EXECUTION_EPOCH') == row['execution_epoch']
                          and environment.get('COWORK_INSTANCE_ID') == instance
                          and environment.get('COWORK_ORGANISATION_ID') == host['trust_domain'], 'host_capacity_epoch_changed')
                services, runtime_cpu, runtime_memory, runtime_count = set(), 0, 0, 0
                for suffix, container in owned:
                    caps = container['HostConfig'] if suffix.startswith('openshell-original-') else inspect_caps(container)
                    labels = container.get('Config', {}).get('Labels') or {}
                    service = next((key for key in budget['services'] if suffix == key + '-1'), None)
                    if service is not None:
                        check(service not in services and labels.get('com.docker.compose.project') == manifest['name']
                              and labels.get('com.docker.compose.service') == service
                              and caps['NanoCpus'] == budget['services'][service]['cpu_millis'] * 1000000
                              and caps['Memory'] == budget['services'][service]['memory_mib'] * 1024 * 1024
                              and caps['PidsLimit'] == budget['services'][service]['pids']
                              and container['Config'].get('Image') == manifest['services'][service]['image'],
                              'host_capacity_service_changed')
                        environment = container['Config'].get('Env') or []
                        for key, value in manifest['services'][service].get('environment', {}).items():
                            check([entry for entry in environment if entry.startswith(key + '=')] == [key + '=' + str(value)],
                                  'host_capacity_service_environment_changed')
                        services.add(service)
                    else:
                        if re.fullmatch(r'openshell-original-[a-f0-9]{64}',suffix):
                            import openshell_environment
                            try:main=openshell_environment.capacity(infrastructure,container,instance)
                            except (RuntimeError,KeyError,ValueError,OSError):raise CapacityError('host_capacity_unknown_openshell') from None
                            runtime_cpu+=caps['NanoCpus'];runtime_memory+=caps['Memory'];runtime_count+=int(main)
                            continue
                        if re.fullmatch(r'environment-[a-f0-9]{32}',suffix):
                            import environment_lifecycle
                            try:environment_lifecycle.docker_capacity(infrastructure,container,instance)
                            except (RuntimeError,KeyError,ValueError,OSError):raise CapacityError('host_capacity_unknown_environment') from None
                            runtime_cpu += caps['NanoCpus'];runtime_memory += caps['Memory'];runtime_count += 1
                            continue
                        check(re.fullmatch(r'(?:job|desktop|terminal|transfer)-[a-f0-9]{32}', suffix)
                              and isinstance(labels.get('dev.opencowork.admission'), str)
                              and caps['PidsLimit'] <= 4096 and caps.get('MemorySwap') == caps['Memory'],
                              'host_capacity_unknown_runtime')
                        identity(labels.get('dev.opencowork.admission')); identity(labels.get('dev.opencowork.run_id'))
                        runtime_cpu += caps['NanoCpus']; runtime_memory += caps['Memory']; runtime_count += 1
                check(runtime_cpu <= budget['sandbox']['cpu_millis'] * 1000000
                      and runtime_memory <= budget['sandbox']['memory_mib'] * 1024 * 1024
                      and runtime_count <= budget['sandbox']['concurrent_runs'], 'host_capacity_runtime_exceeds_pool')
                if row['instance_state'] in ['ready', 'running']:
                    check(services == set(budget['services']), 'host_capacity_services_incomplete')
                report.update(state='applied', manifest_sha256=digest(manifest), resource_budget=budget)
        except (CapacityError, resource_budget.BudgetError, execution_generation.ExecutionGenerationError,
                OSError, KeyError, TypeError, ValueError):
            unknown = True
        reports.append(report)
    return {'observed_capacity': {'cpu_millis': info['NCPU'] * 1000, 'memory_mib': info['MemTotal'] // (1024 * 1024)},
            'inventory_complete': True, 'unknown_workloads': unknown, 'allocations': reports}


class CapacityAttestor:
    refresh_seconds = 20
    collection_seconds = 25

    def __init__(self, manager, infrastructure, *, clock=time.monotonic):
        self.manager, self.infra, self.clock = manager, infrastructure, clock
        self.configured = settings(infrastructure.config)
        self.lock = threading.RLock()
        self.stop_event = threading.Event()
        self.thread = None
        self.latest = None
        self.qualified = False
        self.path = infrastructure.root / 'host-capacity-status.json'

    def bind(self):
        marker = self.infra.root / 'host-capacity-client.json'
        if marker.exists():
            check(self.configured is not None, 'host_capacity_disable_requires_review')
            check(document(marker) == self.binding(), 'host_capacity_client_configuration_changed')
        if self.configured is not None:
            self.infra.host.bind()
            self.infra.private_write(marker, json.dumps(self.binding(), sort_keys=True).encode())

    def binding(self):
        return {'version': 1, 'executor_host_binding': executor_host.manager_binding(self.infra.config),
                'configuration': self.configured}

    def status(self, reason):
        value = {'version': 1, 'enabled': True, 'qualified': self.qualified, 'reason': reason,
                 'executor_host_binding': executor_host.manager_binding(self.infra.config)}
        if self.latest is not None:
            value.update(host_revision=self.latest['host_revision'], allocation_revision=self.latest['allocation_revision'],
                         challenge_id=self.latest['challenge_id'], server_challenge_expires_at=self.latest['expires_at'])
        self.infra.private_write(self.path, json.dumps(value, sort_keys=True).encode())

    def refresh(self):
        with self.lock:
            self.qualified = False
            started = self.clock()
            try:
                selected = challenge(self.manager.call('/host-capacity/challenge',
                    {'executor_host_binding': executor_host.manager_binding(self.infra.config)}), self.infra.config)
                self.latest = selected
                self.infra.host.bind(); self.infra.host.verify()
                if self.infra.config.get('backend')=='kubernetes':
                    import kubernetes_capacity
                    observed=kubernetes_capacity.collect(self.infra,selected)
                else:
                    info = json.loads(self.infra.run(['docker', 'info', '--format', '{{json .}}'], timeout=10))
                    identifiers = self.infra.run(['docker', 'ps', '--all', '--quiet', '--no-trunc'], timeout=10).decode().splitlines()
                    check(len(identifiers) <= 20000 and all(re.fullmatch('[a-f0-9]{64}', key) for key in identifiers)
                          and len(set(identifiers)) == len(identifiers), 'invalid_host_capacity_daemon_inventory')
                    containers = []
                    for offset in range(0, len(identifiers), 64):
                        containers.extend(json.loads(self.infra.run(['docker', 'inspect', *identifiers[offset:offset+64]], timeout=10)))
                    actual = {entry.get('Id') for entry in containers}
                    check(actual == set(identifiers) and len(containers) == len(identifiers), 'host_capacity_inventory_changed')
                    observed = inventory(self.infra, selected, info, containers)
                    # A create/remove during collection is uncertain. Never claim an
                    # exhaustive view from one filtered tenant subset.
                    after = self.infra.run(['docker', 'ps', '--all', '--quiet', '--no-trunc'], timeout=10).decode().splitlines()
                    if set(after) != set(identifiers):
                        observed['inventory_complete'] = False; observed['unknown_workloads'] = True
                check(self.clock() - started < self.collection_seconds, 'host_capacity_challenge_collection_expired')
                result = self.manager.call('/host-capacity/attest', {'version': 1, 'challenge_id': selected['challenge_id'],
                    'executor_host_binding': selected['executor_host_binding'], **observed})
                check(isinstance(result, dict) and set(result) == {'qualified', 'reason'} and type(result['qualified']) is bool
                      and ((result['reason'] is None) == result['qualified']), 'invalid_host_capacity_attestation_response')
                self.qualified = result['qualified']
                self.status(None if self.qualified else 'manager_rejected_inventory')
                return self.qualified
            except Exception:
                self.status('capacity_observation_failed')
                # No 404, token, TLS, clock or malformed-response fallback.
                raise CapacityError('host_capacity_attestation_failed') from None

    def start(self):
        self.bind()
        if self.configured is None:
            return
        self.refresh()
        def renew():
            while not self.stop_event.wait(self.refresh_seconds):
                try:
                    self.refresh()
                except CapacityError:
                    pass  # Durable local status and manager TTL both fail closed.
        self.thread = threading.Thread(target=renew, name='cowork-host-capacity', daemon=True)
        self.thread.start()

    def close(self):
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=55)
            check(not self.thread.is_alive(), 'host_capacity_renewal_did_not_stop')

    def validate_operation(self, operation, *, activating=False):
        value = operation_allocation(operation)
        if self.configured is None:
            check(value is None, 'host_capacity_attestation_opt_in_required')
            return
        # Cleanup of a pre-allocation assigned instance remains possible; it
        # cannot create a resource or invent a releasable charge.
        if value is None:
            check(operation['action'] in ['stop', 'suspend', 'delete'], 'operation_host_allocation_required')
            return
        check(value['plan']['services'] == resource_budget.service_caps(self.infra.config),
              'host_capacity_operator_plan_mismatch')
        qualification = self.infra.config.get('remote_lifecycle_qualification') is not None or self.infra.config.get('backend_profile') is not None
        if activating and (operation['action'] in ['provision', 'start', 'resume','import_quiesce','import_export','import_restore','import_qualify']
                           or qualification and operation['action'] in ['backup', 'restore', 'upgrade', 'rollback']):
            with self.lock:
                self.refresh()
                check(self.qualified and self.latest is not None, 'host_capacity_unqualified')
                row = next((entry for entry in self.latest['allocations'] if entry['instance_id'] == operation['instance_id']), None)
                check(row is not None and row['allocation_revision'] == value['binding']['allocation_revision']
                      and row['execution_epoch'] == value['binding']['execution_epoch']
                      and row['plan_sha256'] == value['binding']['plan_sha256'], 'operation_host_allocation_changed')


def validate_operation(infrastructure, operation, *, activating=False):
    attestor = getattr(infrastructure, 'capacity_attestor', None)
    if attestor is not None:
        attestor.validate_operation(operation, activating=activating)
    else:
        check(settings(infrastructure.config) is None and operation_allocation(operation) is None,
              'host_capacity_attestation_operator_required')
