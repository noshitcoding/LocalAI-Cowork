"""Exact source-bound MFA configuration; runtime authority stays with management."""
import json
import uuid
import re

CONTRACT = {'version': 1, 'current_organisation_policy': True,
            'original_session_admission': True, 'worker_and_device_fences': True,
            'scoped_recovery': True, 'stream_fences': True}

def configuration(instance: dict) -> dict:
    policy = instance.get('mfa_policy')
    if not isinstance(policy, dict) or set(policy) != {'organisation_id', 'revision', 'epoch', 'required', 'max_age_seconds', 'allow_validated_idp'}:
        raise ValueError('current_organisation_mfa_policy_required')
    if str(uuid.UUID(policy['organisation_id'])) != instance['organisation_id'] or str(uuid.UUID(instance['id'])) != instance['id']:
        raise ValueError('organisation_mfa_scope_mismatch')
    if uuid.UUID(policy['epoch']).int == 0 or type(policy['revision']) is not int or policy['revision'] < 1:
        raise ValueError('organisation_mfa_revision_required')
    if type(policy['required']) is not bool or type(policy['allow_validated_idp']) is not bool or type(policy['max_age_seconds']) is not int or not 300 <= policy['max_age_seconds'] <= 43200:
        raise ValueError('invalid_organisation_mfa_policy')
    return {'version': 1, 'instance_id': instance['id'], 'policy': policy}

def environment(config: dict, instance: dict) -> dict:
    value = configuration(instance)
    if not config.get('fleet_management_url'):
        raise ValueError('organisation_mfa_management_authority_required')
    return {'COWORK_ORGANISATION_MFA_POLICY': json.dumps(value, separators=(',', ':'), sort_keys=True)}

def probe(infrastructure, instance: dict, image: str) -> None:
    configuration(instance)
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[0-9a-f]{64}', image):
        raise ValueError('organisation_mfa_requires_pinned_server_image')
    cache = getattr(infrastructure, '_organisation_mfa_image_probes', set())
    if image in cache:
        return
    output = infrastructure.run(['docker', 'run', '--rm', '--pull', 'never', '--network', 'none',
        '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
        '--cpus', '0.1', '--memory', '128m', '--memory-swap', '128m', '--pids-limit', '32',
        '--entrypoint', '/usr/local/bin/cowork-server', image, '--organisation-mfa-contract'], timeout=30)
    if len(output) > 4096:
        raise ValueError('organisation_mfa_image_incompatible')
    value = json.loads(output)
    if not isinstance(value, dict) or set(value) != set(CONTRACT) or any(type(value[k]) is not type(v) or value[k] != v for k, v in CONTRACT.items()):
        raise ValueError('organisation_mfa_image_incompatible')
    # This is a binary compatibility declaration, never a hosted qualification.
    infrastructure._organisation_mfa_image_probes = cache | {image}
