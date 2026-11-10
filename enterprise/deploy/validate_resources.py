#!/usr/bin/env python3
"""Validate rendered Compose JSON from stdin; never contacts the Docker daemon.

Usage: docker compose ... config --format json | python deploy/validate_resources.py
The sums describe service ceilings. Runtime sandbox budgets are reported separately.
"""
import json
import re
import sys


class ConfigurationError(Exception):
    pass


def memory_mib(value):
    match = re.fullmatch(r'([0-9]+)([kmgt]?)b?', str(value).lower())
    if not match:
        raise ConfigurationError('invalid_memory_bound')
    size = int(match[1]) * {'': 1, 'k': 1024, 'm': 1024 ** 2, 'g': 1024 ** 3, 't': 1024 ** 4}[match[2]]
    if not 16 * 1024 ** 2 <= size <= 32768 * 1024 ** 2:
        raise ConfigurationError('invalid_memory_bound')
    return (size + 1024 ** 2 - 1) // 1024 ** 2


def validate(manifest):
    services = manifest.get('services')
    if not isinstance(services, dict) or not services:
        raise ConfigurationError('services_required')
    total = {'cpu_millis': 0, 'memory_mib': 0, 'pids': 0}
    sandbox = []
    for name, service in services.items():
        try:
            cpu = service['cpus']
            pids = service['pids_limit']
            if type(cpu) not in [int, float] or not 0 < cpu <= 32 or type(pids) is not int or not 1 <= pids <= 4096:
                raise ConfigurationError('invalid_cpu_or_pids_bound')
            memory = memory_mib(service['mem_limit'])
            replicas = service.get('scale', service.get('deploy', {}).get('replicas', 1))
            if type(replicas) is not int or not 1 <= replicas <= 100:
                raise ConfigurationError('invalid_service_replicas')
            if service.get('privileged') is True:
                raise ConfigurationError('privileged_service_forbidden')
            volumes = service.get('volumes', [])
            if name in ['api', 'worker', 'management', 'web', 'management-web', 'workspace-web'] and any(
                    '/var/run/docker.sock' in str(volume) for volume in volumes):
                raise ConfigurationError('application_docker_socket_forbidden')
            total['cpu_millis'] += round(cpu * 1000) * replicas
            total['memory_mib'] += memory * replicas
            total['pids'] += pids * replicas
            env = service.get('environment', {})
            if env.get('COWORK_RUNNER_CPU_MILLIS') is not None:
                sandbox.append({key: env.get(variable) for key, variable in [('cpu_millis', 'COWORK_RUNNER_CPU_MILLIS'),
                    ('memory_mib', 'COWORK_RUNNER_MEMORY_MIB'), ('concurrent_runs', 'COWORK_RUNNER_CONCURRENT_RUNS')]})
        except (KeyError, TypeError, ValueError):
            raise ConfigurationError('service_resource_bounds_required:' + name) from None
    return {'services': len(services), 'service_maximum': total, 'sandbox_budgets': sandbox}


def main():
    try:
        value = validate(json.load(sys.stdin))
    except (ConfigurationError, ValueError, TypeError) as error:
        print('configuration_invalid:' + str(error), file=sys.stderr)
        return 1
    print(json.dumps(value, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
