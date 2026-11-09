"""Operator-owned host allocation; sandbox policy is separate from service caps.

Allocation reserves the maximum for every retained instance, including stopped
instances. Only verified permanent deletion removes a reservation. The caller
must hold the existing exclusive operator lock before reserving capacity.
"""
import copy
import json
from pathlib import Path
import re
import uuid


class BudgetError(Exception):
    pass


DEFAULT_SERVICES = {
    'api': {'cpu_millis': 1000, 'memory_mib': 512, 'pids': 256},
    'worker': {'cpu_millis': 500, 'memory_mib': 512, 'pids': 256},
    'runner': {'cpu_millis': 500, 'memory_mib': 256, 'pids': 128},
    'egress-proxy': {'cpu_millis': 250, 'memory_mib': 128, 'pids': 64},
}
DATA_SERVICES = {'postgres': {'cpu_millis': 500, 'memory_mib': 512, 'pids': 128},
                 'minio': {'cpu_millis': 500, 'memory_mib': 512, 'pids': 128}}
def service_defaults(config):
    overrides=config.get('instance_service_limits', {})
    qualified=config.get('instance_disk_quotas') is not None or set(overrides)==set(DEFAULT_SERVICES)|set(DATA_SERVICES)
    return {**DEFAULT_SERVICES, **(DATA_SERVICES if qualified else {})}


def check(condition, code):
    if not condition:
        raise BudgetError(code)


def service_caps(config):
    overrides = config.get('instance_service_limits', {})
    defaults=service_defaults(config)
    check(isinstance(overrides, dict) and set(overrides) <= set(defaults), 'invalid_instance_service_limits')
    services = copy.deepcopy(defaults)
    for name, values in overrides.items():
        check(isinstance(values, dict) and set(values) == {'cpu_millis', 'memory_mib', 'pids'}, 'invalid_instance_service_limits')
        for key, minimum, maximum in [('cpu_millis', 100, 32000), ('memory_mib', 64, 32768), ('pids', 16, 4096)]:
            check(type(values[key]) is int and minimum <= values[key] <= maximum, 'invalid_instance_service_limits')
        services[name] = copy.deepcopy(values)
    return services


def service_compose(caps):
    return {'cpus': caps['cpu_millis'] / 1000, 'mem_limit': str(caps['memory_mib']) + 'm', 'pids_limit': caps['pids']}


def instance_budget(config, instance):
    services = service_caps(config)
    canonical_id(instance['id'])
    policy = instance['policy']['limits']
    check(isinstance(policy, dict), 'invalid_resource_budget_evidence')
    for key, minimum, maximum in [('cpu_millis', 100, 1024000), ('memory_mib', 128, 4194304), ('concurrent_runs', 1, 1000)]:
        check(type(policy.get(key)) is int and minimum <= policy[key] <= maximum, 'invalid_resource_budget_evidence')
    sandbox = {key: policy[key] for key in ['cpu_millis', 'memory_mib', 'concurrent_runs']}
    return {'version': 1, 'instance_id': instance['id'], 'services': services, 'sandbox': sandbox,
            'total': {key: sandbox[key] + sum(value[key] for value in services.values()) for key in ['cpu_millis', 'memory_mib']}}


def host_budget(config):
    value = config.get('fleet_host_budget')
    if value is None:
        return None
    if config.get('backend','compose')!='compose':
        import backend_profile
        selected=backend_profile.settings(config)
        check(config.get('backend')=='kubernetes' and selected is not None and selected['specification']['kind']=='kubernetes','fleet_host_budget_backend_not_qualified')
    check(isinstance(value, dict) and set(value) == {'cpu_millis', 'memory_mib', 'reserved_cpu_millis', 'reserved_memory_mib'}, 'invalid_fleet_host_budget')
    for key in ['cpu_millis', 'memory_mib', 'reserved_cpu_millis', 'reserved_memory_mib']:
        check(type(value[key]) is int and 0 <= value[key] <= 4194304, 'invalid_fleet_host_budget')
    for key in ['cpu_millis', 'memory_mib']:
        check(value[key] > value['reserved_' + key], 'invalid_fleet_host_budget')
    return value


def read(path):
    check(not path.is_symlink() and path.is_file() and path.stat().st_size <= 1048576, 'invalid_resource_budget_evidence')
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        raise BudgetError('invalid_resource_budget_evidence') from None


def canonical_id(value):
    try:
        check(str(uuid.UUID(value)) == value, 'invalid_resource_budget_evidence')
    except (ValueError, TypeError, AttributeError):
        raise BudgetError('invalid_resource_budget_evidence') from None
    return value


def bind_configuration(infrastructure):
    marker = infrastructure.root / 'host-budget.json'
    value = host_budget(infrastructure.config)
    check(not marker.is_symlink(), 'invalid_host_budget_binding')
    if marker.exists():
        saved = read(marker)
        check(isinstance(saved, dict) and saved.get('version') == 1
              and host_budget({'fleet_host_budget': saved.get('budget')}) is not None, 'invalid_host_budget_binding')
        check(value is not None, 'host_budget_disable_requires_review')
    if value is not None:
        infrastructure.private_write(marker, json.dumps({'version': 1, 'budget': value}, sort_keys=True).encode())


def generations(directory):
    tree = directory / 'generations'
    pointer = directory / 'active.json'
    if not tree.exists():
        check(not pointer.exists(), 'resource_budget_generation_requires_review')
        return
    check(not tree.is_symlink() and tree.is_dir(), 'resource_budget_generation_requires_review')
    active = None
    if pointer.exists():
        selection = read(pointer)
        check(isinstance(selection, dict), 'resource_budget_generation_requires_review')
        active = selection.get('generation')
        try:
            check(str(uuid.UUID(active)) == active, 'resource_budget_generation_requires_review')
        except (ValueError, TypeError, AttributeError):
            raise BudgetError('resource_budget_generation_requires_review') from None
    else:
        # A failed first upgrade can restore the original root generation while
        # retaining its candidate for review. It still has exactly one active
        # root manifest, rather than an invented active generation pointer.
        check((directory / 'compose.json').exists(), 'resource_budget_generation_requires_review')
    found = active is None
    for child in tree.iterdir():
        check(not child.is_symlink() and child.is_dir() and re.fullmatch('[a-f0-9]{32}', child.name), 'resource_budget_generation_requires_review')
        value = read(child / 'generation.json')
        check(isinstance(value, dict) and isinstance(value.get('generation'), str)
              and value['generation'].replace('-', '') == child.name, 'resource_budget_generation_requires_review')
        canonical_id(value['generation'])
        found = found or value['generation'] == active
    check(found, 'resource_budget_generation_requires_review')


def manifest_budget(manifest, instance_id):
    """Verify the exact manifest caps, rejecting replicas and hidden services."""
    check(isinstance(manifest, dict) and isinstance(manifest.get('services'), dict)
          and set(manifest['services']) in [set(DEFAULT_SERVICES), set(DEFAULT_SERVICES)|set(DATA_SERVICES)], 'resource_budget_manifest_mismatch')
    caps = {}
    for name, service in manifest['services'].items():
        check(isinstance(service, dict) and isinstance(service.get('deploy', {}), dict)
              and type(service.get('scale', 1)) is int and service.get('scale', 1) == 1
              and type(service.get('deploy', {}).get('replicas', 1)) is int
              and service.get('deploy', {}).get('replicas', 1) == 1, 'resource_budget_replicas_not_qualified')
        try:
            cpu = float(service['cpus']) * 1000
            memory = service['mem_limit']
            check(type(service['cpus']) in [int, float] and cpu.is_integer()
                  and isinstance(memory, str) and re.fullmatch('[0-9]+m', memory), 'resource_budget_manifest_mismatch')
            caps[name] = {'cpu_millis': int(cpu), 'memory_mib': int(memory[:-1]), 'pids': service['pids_limit']}
        except (TypeError, ValueError, KeyError):
            raise BudgetError('resource_budget_manifest_mismatch') from None
    checked = service_caps({'instance_service_limits': caps})
    runner = manifest['services']['runner']['environment']
    try:
        sandbox = {key: int(runner[env]) for key, env in [('cpu_millis', 'COWORK_RUNNER_CPU_MILLIS'),
            ('memory_mib', 'COWORK_RUNNER_MEMORY_MIB'), ('concurrent_runs', 'COWORK_RUNNER_CONCURRENT_RUNS')]}
        check(100 <= sandbox['cpu_millis'] <= 1024000 and 128 <= sandbox['memory_mib'] <= 4194304
              and 1 <= sandbox['concurrent_runs'] <= 1000, 'resource_budget_manifest_mismatch')
    except (TypeError, ValueError, KeyError):
        raise BudgetError('resource_budget_manifest_mismatch') from None
    api = manifest['services']['api']['environment']
    check(api.get('COWORK_INSTANCE_ID') == instance_id, 'resource_budget_instance_mismatch')
    return {'version': 1, 'instance_id': instance_id, 'services': checked, 'sandbox': sandbox,
            'total': {key: sandbox[key] + sum(value[key] for value in checked.values()) for key in ['cpu_millis', 'memory_mib']}}


def reserve(infrastructure, instance, directory, manifest):
    candidate = manifest_budget(manifest, instance['id'])
    budget = host_budget(infrastructure.config)
    if budget is not None:
        generations(directory)
        previous = directory / 'compose.json'
        evidence = directory / 'resource-budget.json'
        if previous.exists():
            check(evidence.exists(), 'legacy_resource_budget_requires_review')
            check(manifest_budget(read(previous), instance['id']) == read(evidence), 'resource_budget_manifest_changed')
        totals = {key: budget['reserved_' + key] + candidate['total'][key] for key in ['cpu_millis', 'memory_mib']}
        for other in infrastructure.root.iterdir():
            if not re.fullmatch('[a-f0-9]{32}', other.name) or other == directory:
                continue
            check(not other.is_symlink() and other.is_dir(), 'unsafe_resource_budget_inventory')
            if (other / 'deleted.json').exists():
                # A tombstone alone cannot hide retained workloads.
                check(not (other / 'compose.json').exists() and not (other / 'resource-budget.json').exists(), 'deleted_resource_budget_not_reconciled')
                continue
            generations(other)
            evidence = other / 'resource-budget.json'
            if not evidence.exists():
                check(not (other / 'compose.json').exists() and not (other / 'active.json').exists()
                      and not (other / 'generations').exists(), 'legacy_resource_budget_requires_review')
                continue
            saved = read(evidence)
            check(isinstance(saved, dict) and saved.get('version') == 1
                  and re.fullmatch('[a-f0-9]{32}', other.name)
                  and str(saved.get('instance_id', '')).replace('-', '') == other.name, 'invalid_resource_budget_evidence')
            deployed = other / 'compose.json'
            if deployed.exists():
                check(manifest_budget(read(deployed), saved['instance_id']) == saved, 'resource_budget_manifest_changed')
            else:
                # Persist-before-start reservations survive a crash even if the
                # new compose.json was never written. Revalidate their numbers.
                check(instance_budget({'instance_service_limits': saved.get('services')},
                    {'id': saved['instance_id'], 'policy': {'limits': saved.get('sandbox', {})}}) == saved, 'invalid_resource_budget_evidence')
            for key in totals:
                totals[key] += saved['total'][key]
        check(all(totals[key] <= budget[key] for key in totals), 'fleet_host_capacity_exhausted')
    infrastructure.private_write(directory / 'resource-budget.json', json.dumps(candidate, sort_keys=True).encode())
    return candidate
