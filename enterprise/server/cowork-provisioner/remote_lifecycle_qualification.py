"""Original remote lifecycle profiles; source delivery is not physical qualification.

The frozen host, instance, capacity and epoch envelopes remain authoritative.
Fixture profiles remain constrained to their own disposable scope. Product VM
profiles require the real activated backend, fresh physical observations and
original host/capacity/epoch assignments before using the existing adapters.
"""
import json
import hashlib
from pathlib import Path
import re
import uuid

import execution_generation
import executor_host
import guest_endpoints
import host_capacity_attestation
import remote_volume_streaming
import resource_budget


class LifecycleError(Exception):
    pass


def require(condition, code):
    if not condition:
        raise LifecycleError(code)


def settings(config):
    import backend_profile
    product=backend_profile.settings(config)
    if product is not None and product['specification']['kind']=='vm_compose':
        spec=product['specification'];host=executor_host.manager_binding(config)
        guest=guest_endpoints.settings(config);limits=remote_volume_streaming.settings(config)
        require(host is not None and guest is not None and spec['instance_id']==guest['instance_id'] and host['host_id']==spec['host_id']
                and host['trust_domain']==spec['organisation_id'] and host['daemon_id']==spec['daemon_id']
                and host_capacity_attestation.settings(config) is not None,'product_vm_lifecycle_profile_differs')
        require(type(config.get('max_workspace_archives',1000)) is int and 1<=config.get('max_workspace_archives',1000)<=1000,'invalid_product_vm_workspace_bounds')
        return {'version':1,'mode':'product_vm','instance_id':spec['instance_id'],'executor_host_binding':host,
                'specification_sha256':hashlib.sha256(backend_profile.canonical(spec)).hexdigest(),
                'max_workspace_archives':config.get('max_workspace_archives',1000),'max_workspace_archive_bytes':limits['max_archive_bytes']}
    selected = config.get('remote_lifecycle_qualification')
    if selected is None:
        return None
    require(isinstance(selected, dict) and set(selected) == {'version', 'mode', 'fixture_id', 'instance_id',
            'executor_host_binding', 'max_workspace_archives', 'max_workspace_archive_bytes'}
            and type(selected['version']) is int and selected['version'] == 1
            and selected['mode'] == 'fixture_only', 'invalid_remote_lifecycle_qualification')
    for key in ['fixture_id', 'instance_id']:
        try:
            require(type(selected[key]) is str and str(uuid.UUID(selected[key])) == selected[key]
                    and uuid.UUID(selected[key]).int != 0, 'invalid_remote_lifecycle_qualification')
        except (ValueError, TypeError, AttributeError):
            raise LifecycleError('invalid_remote_lifecycle_qualification') from None
    require(type(selected['max_workspace_archives']) is int and 1 <= selected['max_workspace_archives'] <= 1000
            and type(selected['max_workspace_archive_bytes']) is int
            and 1048576 <= selected['max_workspace_archive_bytes'] <= 8 * 1024**3,
            'invalid_remote_lifecycle_qualification_bounds')
    try:
        host = executor_host.manager_binding(config)
        guest = guest_endpoints.settings(config)
        limits = remote_volume_streaming.settings(config)
        capacity = host_capacity_attestation.settings(config)
        require(host is not None and selected['executor_host_binding'] == host and guest is not None
                and guest['instance_id'] == selected['instance_id'] and capacity is not None
                and limits['max_archive_bytes'] <= selected['max_workspace_archive_bytes'],
                'remote_lifecycle_qualification_profile_mismatch')
    except (executor_host.HostError, guest_endpoints.GuestEndpointError,
            remote_volume_streaming.StreamError, host_capacity_attestation.CapacityError) as error:
        raise LifecycleError(str(error)) from None
    return dict(selected)


def enabled(config):
    selected = settings(config)
    require(selected is not None, 'remote_executor_lifecycle_not_qualified')
    return selected


def bind(infrastructure):
    marker = infrastructure.root / 'remote-lifecycle-qualification.json'
    selected = settings(infrastructure.config)
    require(not marker.is_symlink(), 'unsafe_remote_lifecycle_qualification_binding')
    if selected is None:
        require(not marker.exists(), 'remote_lifecycle_qualification_disable_requires_review')
        return
    if marker.exists():
        try:
            require(marker.is_file() and marker.stat().st_nlink == 1 and marker.stat().st_size <= 65536
                    and json.loads(marker.read_text()) == selected,
                    'remote_lifecycle_qualification_retarget_requires_review')
        except (ValueError, OSError):
            raise LifecycleError('invalid_remote_lifecycle_qualification_binding') from None
    else:
        require(not any(path.is_dir() and re.fullmatch('[a-f0-9]{32}', path.name)
                        for path in infrastructure.root.iterdir()),
                'remote_lifecycle_qualification_requires_new_fixture')
        infrastructure.private_write(marker, json.dumps(selected, sort_keys=True).encode())


def instance(config, value, action):
    selected = enabled(config)
    require(value.get('id') == selected['instance_id'] and value.get('organisation_id')
            == selected['executor_host_binding']['trust_domain']
            and value.get('executor_host_id') == selected['executor_host_binding']['host_id']
            and value.get('backend') == 'compose' and value['policy']['minimum_isolation'] == ('virtual_machine' if selected['mode']=='product_vm' else 'container')
            and action in ['backup', 'restore', 'upgrade', 'rollback','enroll_execution','import_quiesce','import_export','import_restore','import_qualify'],
            'remote_lifecycle_qualification_scope_mismatch')
    return selected


def operation(infrastructure, value):
    if enabled(infrastructure.config)['mode']=='product_vm':
        import backend_profile
        backend_profile.vm_scope(infrastructure,value)
    if value.get('action','').startswith('import_'):
        import instance_imports
        instance_imports.binding(value)
    selected = instance(infrastructure.config, value['instance'], value['action'])
    require(executor_host.operation_binding(value) == selected['executor_host_binding'],
            'remote_lifecycle_qualification_scope_mismatch')
    try:
        allocation = host_capacity_attestation.operation_allocation(value)
        epoch = execution_generation.binding(value)
        host_capacity_attestation.validate_operation(infrastructure, value)
    except (host_capacity_attestation.CapacityError, execution_generation.ExecutionGenerationError) as error:
        raise LifecycleError(str(error)) from None
    require(allocation is not None and epoch is not None
            and allocation['binding']['execution_epoch'] == epoch['source_epoch'],
            'remote_lifecycle_qualification_frozen_allocation_required')
    bind(infrastructure)
    return selected


def quiescent(infrastructure, value):
    """This single-instance fixture refuses every unaccounted daemon container."""
    selected = enabled(infrastructure.config)
    if selected['mode']=='product_vm':
        import backend_profile
        backend_profile.vm_scope(infrastructure,value)
    require(value['id'] == selected['instance_id'], 'remote_lifecycle_qualification_scope_mismatch')
    infrastructure.host.verify()
    names = infrastructure.run(['docker', 'ps', '--all', '--format', '{{.Names}}']).decode().splitlines()
    prefix = 'cowork-' + uuid.UUID(value['id']).hex + '-'
    dedicated=infrastructure.config.get('instance_disk_quotas') is not None
    allowed = {prefix + service + '-1' for service in resource_budget.service_caps(infrastructure.config)}
    require(len(names) == len(set(names)) and set(names) <= allowed,
            'remote_lifecycle_unknown_daemon_inventory')
    running=infrastructure.run(['docker','ps','--format','{{.Names}}']).decode().splitlines()
    require(not running or dedicated and set(running)<={prefix+'postgres-1',prefix+'minio-1'},'remote_lifecycle_requires_quiescent_daemon')
    if dedicated:
        import instance_disk_quotas
        instance_disk_quotas.verified_data_services(infrastructure)
    if names:
        directory = infrastructure.root / uuid.UUID(value['id']).hex
        manifest = host_capacity_attestation.document(directory / 'compose.json')
        inspected = json.loads(infrastructure.run(['docker', 'inspect', *names]))
        require(isinstance(inspected, list) and len(inspected) == len(names), 'remote_lifecycle_unknown_daemon_inventory')
        found = set()
        for container in inspected:
            name = container.get('Name', '').removeprefix('/')
            service = name.removeprefix(prefix).removesuffix('-1')
            labels = container.get('Config', {}).get('Labels') or {}
            require(name in names and name not in found and service in manifest['services']
                    and container.get('State', {}).get('Running') is False
                    and labels.get('com.docker.compose.project') == prefix[:-1]
                    and labels.get('com.docker.compose.service') == service
                    and container.get('Config', {}).get('Image') == manifest['services'][service]['image'],
                    'remote_lifecycle_unknown_daemon_inventory')
            found.add(name)


def workspace_inventory(config, manifest):
    selected = enabled(config)
    names = manifest['workspace_runs']
    require(len(names) <= selected['max_workspace_archives'], 'remote_workspace_inventory_exceeds_bound')
    require({key for key in manifest['files'] if key.startswith('workspaces/')}
            == {'workspaces/' + suffix + '.tar' for suffix in names}, 'remote_workspace_archive_inventory_mismatch')
    total = sum(manifest['files']['workspaces/' + suffix + '.tar']['bytes'] for suffix in names)
    require(total <= selected['max_workspace_archive_bytes'], 'remote_workspace_archive_total_exceeds_bound')


def validate_workspaces(infrastructure, value, payload, manifest):
    """Validate every inner tar before generation creation, DB or volume writes."""
    selected = operation(infrastructure, value)
    if value.get('action') == 'import_restore':
        import instance_imports
        frozen=instance_imports.binding(value)
        instance_imports.verify_export(value,manifest)
        require(manifest.get('executor_host_binding') == frozen['source_host_binding']
                and isinstance(manifest.get('remote_fixture_id'),str)
                and str(uuid.UUID(manifest['remote_fixture_id']))==manifest['remote_fixture_id'], 'remote_backup_source_host_scope_mismatch')
    else:
        require(manifest.get('executor_host_binding') == selected['executor_host_binding']
                and manifest.get('remote_fixture_id') == selected['fixture_id'], 'remote_backup_host_scope_mismatch')
    workspace_inventory(infrastructure.config, manifest)
    limits = remote_volume_streaming.settings(infrastructure.config)
    for suffix in manifest['workspace_runs']:
        relative = 'workspaces/' + suffix + '.tar'
        remote_volume_streaming.verified_archive(Path(payload) / relative, manifest['files'][relative], limits)
