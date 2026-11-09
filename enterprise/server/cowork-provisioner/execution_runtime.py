"""Operator opt-in and image compatibility checks for central execution admission.

The allowlist is operator-owned. A binary's declaration is a compatibility check,
not a substitute for qualification of the image or fleet-wide enrollment.
"""
import json
import re
from urllib.parse import urlsplit
from execution_generation import ExecutionGenerationError, check, read

CONTRACT = {"version": 1, "reservation_before_claim": True, "durable_completion": True,
            "generation_fencing": True, "external_claims": True, "worker": True}


def settings(config, image):
    value = config.get('execution_admission')
    check(isinstance(value, dict) and set(value) == {'server_images'}, 'execution_admission_operator_opt_in_required')
    images = value['server_images']
    check(isinstance(images, list) and 0 < len(images) <= 100 and all(
        isinstance(item, str) and re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[0-9a-f]{64}', item)
        for item in images), 'execution_admission_invalid_image_allowlist')
    check(image in images, 'execution_admission_image_not_allowed')
    endpoint = config.get('fleet_management_url', '')
    try:
        url = urlsplit(endpoint)
        check(url.scheme in ['http', 'https'] and url.hostname and not url.username and not url.password
              and not url.query and not url.fragment and url.path in ['', '/'], 'execution_admission_invalid_endpoint')
        url.port
    except (ValueError, TypeError):
        raise ExecutionGenerationError('execution_admission_invalid_endpoint') from None
    check(config.get('backend') == 'compose', 'execution_admission_backend_not_qualified')
    return endpoint


def probe(infrastructure, instance, active, images):
    if read(instance, active) is None:
        return
    probe_image(infrastructure, images['server'])


def probe_image(infrastructure, image):
    """Also usable before enrollment without creating an execution marker."""
    settings(infrastructure.config, image)
    cache = getattr(infrastructure, '_execution_image_probes', set())
    if image in cache:
        return
    try:
        output = infrastructure.run(['docker', 'run', '--rm', '--pull', 'never', '--network', 'none',
            '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
            '--cpus', '0.1', '--memory', '128m', '--memory-swap', '128m', '--pids-limit', '32', '--entrypoint', '/usr/local/bin/cowork-server',
            image, '--execution-admission-contract'], timeout=30)
        check(len(output) < 4096, 'execution_admission_image_incompatible')
        value=json.loads(output)
        check(isinstance(value,dict) and set(value)==set(CONTRACT)
              and all(type(value[k]) is type(v) and value[k]==v for k,v in CONTRACT.items()), 'execution_admission_image_incompatible')
    except Exception:
        raise ExecutionGenerationError('execution_admission_image_incompatible') from None
    infrastructure._execution_image_probes = cache | {image}


def environment(config, image):
    return {'COWORK_FLEET_EXECUTION_ADMISSION': 'true', 'COWORK_FLEET_KEY_FILE': '/run/secrets/fleet_key',
            'COWORK_FLEET_MANAGEMENT_URL': settings(config, image)}


def validate_manifest(config, contents):
    services = contents.get('services', {})
    check(isinstance(services,dict) and all(isinstance(services.get(name),dict)
          for name in ['api','worker','runner','egress-proxy']), 'execution_admission_manifest_mismatch')
    expected_image = services['api'].get('image')
    expected = environment(config, expected_image)
    for name in ['api', 'worker']:
        service = services.get(name, {})
        env = service.get('environment', {})
        check(isinstance(env,dict) and isinstance(service.get('secrets'),list)
              and isinstance(service.get('networks'),(list,dict)), 'execution_admission_manifest_mismatch')
        check(service.get('image') == expected_image and all(env.get(k) == v for k, v in expected.items())
              and 'fleet_key' in service.get('secrets', []) and 'services' in service.get('networks', []),
              'execution_admission_manifest_mismatch')
    for name in ['runner', 'egress-proxy']:
        secrets=services[name].get('secrets', [])
        check(isinstance(secrets,list) and 'fleet_key' not in secrets, 'execution_admission_secret_scope_mismatch')
