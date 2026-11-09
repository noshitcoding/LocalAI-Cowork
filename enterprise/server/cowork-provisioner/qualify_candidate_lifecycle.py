#!/usr/bin/env python3
"""Real candidate lifecycle qualification in a newly created disposable fleet.

No live account, published port, existing state volume or mutable deployment tag
is used. A failed test retains only its new, labelled fixture for inspection.
The default label-only transition contains the same binary. Explicit source
and target image IDs enable a real binary/schema transition; evidence records
both hashes and every observed schema version.
"""
import argparse
import base64
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, ProxyHandler, build_opener
import uuid

from lifecycle_schema_delta import LifecycleSchemaDelta

SERVER = 'sha256:edb08fb843da235eab25c416b7cb5a4a7db3c258fced5426eb1102a116d75bdd'
PROVISIONER = 'sha256:09bdffb03e96bee9e8ab0593f2d8b5a407d55f5c699ce6c4e72a7d93aa496e68'
RUNNER = 'sha256:0d69c9387daf14eaefdc6d798a54e6b98e22abce1898a4ecb3eba3d2c3a8cbe7'


def uid():
    return str(uuid.uuid4())


def wait(predicate, message, seconds=90):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.25)
    raise AssertionError(message)


def docker(*arguments, timeout=240, data=None):
    completed = subprocess.run(['docker', *arguments], input=data, capture_output=True,
                               encoding='utf-8', timeout=timeout)
    if completed.returncode:
        # Docker errors can contain paths, but this helper never passes secrets.
        raise RuntimeError('docker ' + arguments[0] + ': ' + completed.stderr[-1800:])
    return completed.stdout.strip()


def image(reference):
    value = json.loads(docker('image', 'inspect', reference))[0]
    digests = value.get('RepoDigests') or []
    selected = reference if '@sha256:' in reference else next(iter(digests), None)
    assert selected in digests, 'immutable local repository digest required'
    if re.fullmatch(r'sha256:[a-f0-9]{64}', reference):
        assert value['Id'] == reference, 'local image ID changed'
    assert value['Os'] == 'linux' and value['Architecture'] == 'amd64'
    return selected, value['Id']


def image_evidence(reference):
    selected, identifier = image(reference)
    value = json.loads(docker('image', 'inspect', identifier))[0]
    return {'requested_reference': reference, 'resolved_local_reference': selected,
            'actual_image_id': identifier, 'local_repository_digests': value.get('RepoDigests') or [],
            'registry_publication_verified': False}


def running_snapshot():
    identifiers = docker('ps', '--quiet', '--no-trunc').splitlines()
    if not identifiers:
        return {}
    return {value['Id']: value['Image'] for value in json.loads(docker('inspect', *identifiers))}


def assert_preserved(snapshot):
    values = json.loads(docker('inspect', *snapshot)) if snapshot else []
    assert {value['Id'] for value in values} == set(snapshot), 'pre-existing runtime inventory changed'
    for current in values:
        expected_image = snapshot[current['Id']]
        assert current['State']['Running'] and current['Image'] == expected_image, 'pre-existing runtime changed'


def remove_fixture_secrets(root):
    for name in ['management_password', 'fleet_password', 'bootstrap', 'broker', 's3_password', 's3_user',
                 'management_url', 'fleet_url', 'auth_key', 'audit_key', 'control_tls_key', 'control_tls_certificate']:
        path = Path(root) / name
        assert not path.is_symlink()
        if path.exists(): path.chmod(0o600)
        path.unlink(missing_ok=True)


def remove_fixture_images(scope, baseline, alias_base_image_id):
    identifiers = set(docker('image', 'ls', '--quiet', '--no-trunc', '--filter', 'label=cowork.acceptance=' + scope).splitlines())
    values = json.loads(docker('image', 'inspect', *identifiers)) if identifiers else []
    for value in values:
        assert value['Config']['Labels']['cowork.acceptance'] == scope
        assert value['Id'] not in baseline.values()
        assert not docker('ps', '--all', '--quiet', '--filter', 'ancestor=' + value['Id']), 'fixture image still in use'
        tags = value.get('RepoTags') or []
        assert all(tag.startswith(scope + ':') for tag in tags), 'foreign image reference retained'
        docker('image', 'rm', *(tags or [value['Id']]))
    source_tag = scope + ':source'
    if docker('image', 'ls', '--quiet', '--filter', 'reference=' + source_tag):
        assert image(source_tag)[1] == alias_base_image_id
        docker('image', 'rm', source_tag)


def cleanup_fixture(root):
    root = Path(root).resolve()
    marker = json.loads((root / 'fixture.json').read_text())
    scope = marker['scope']
    assert re.fullmatch(r'cowork-lifecycle-test-[0-9a-f]{32}', scope)
    assert root == Path(marker['root']).resolve() and root.parent == Path(tempfile.gettempdir()).resolve()
    assert root.name.startswith(scope + '-')
    assert json.loads(docker('info', '--format', '{{json .}}'))['ID'] == marker['daemon_id']
    assert_preserved(marker['baseline_container_images'])
    containers = docker('ps', '--all', '--quiet', '--no-trunc', '--filter', 'label=cowork.acceptance=' + scope).splitlines()
    inspected = json.loads(docker('inspect', *containers)) if containers else []
    operators = []
    for value in inspected:
        assert value['Id'] not in marker['baseline_container_images']
        assert value['Config']['Labels'].get('com.docker.compose.project') == scope
        assert value['Name'].startswith('/' + scope + '-')
        if value['Config']['Labels'].get('com.docker.compose.service') == 'operator': operators.append(value)
    if operators:
        assert len(operators) == 1 and operators[0]['State']['Running']
        # Only this new fixture's state descriptor may authorize tenant cleanup.
        shutil.copyfile(__file__, root / 'qualify_candidate_lifecycle.py')
        docker('exec', '--env', 'PYTHONPATH=/opt/cowork-provisioner:/fixture', operators[0]['Id'], 'python3',
            '/fixture/qualify_candidate_lifecycle.py', '--inside', '--inside-cleanup',
            '--management-container', next(v['Id'] for v in inspected if v['Config']['Labels'].get('com.docker.compose.service') == 'management'))
    elif not (root / 'results.json').is_file() and not (root / 'cleanup.json').is_file():
        raise AssertionError('failed fixture has no scoped operator to prove tenant cleanup')
    if inspected: docker('rm', '--force', *[value['Id'] for value in inspected])
    for suffix in ['management', 'services', 'routing']:
        name = scope + '-' + suffix
        matches = docker('network', 'ls', '--quiet', '--filter', 'label=cowork.acceptance=' + scope).splitlines()
        values = json.loads(docker('network', 'inspect', *matches)) if matches else []
        selected = next((value for value in values if value['Name'] == name), None)
        if selected:
            assert selected['Labels']['cowork.acceptance'] == scope and not selected['Containers']
            docker('network', 'rm', selected['Id'])
    for suffix in ['state', 'management-data', 'fleet-data', 'storage-data']:
        name = scope + '-' + suffix
        matches = docker('volume', 'ls', '--quiet', '--filter', 'label=cowork.acceptance=' + scope).splitlines()
        if name in matches:
            selected = json.loads(docker('volume', 'inspect', name))[0]
            assert selected['Name'] == name and selected['Labels']['cowork.acceptance'] == scope
            docker('volume', 'rm', name)
    remove_fixture_images(scope, marker['baseline_container_images'], marker.get('fixture_alias_base_image_id', marker['actual_image_ids']['server']))
    assert_preserved(marker['baseline_container_images'])
    remove_fixture_secrets(root)
    (root / 'cleanup.json').write_text(json.dumps({'scope': scope, 'fixture_removed': True,
        'live_containers_preserved': len(marker['baseline_container_images'])}, indent=2))
    print('Exact failed-fixture cleanup complete: ' + scope, flush=True)


def qualification_inputs(args):
    """Validate the source/target/manager matrix before creating any fixture."""
    source_reference = args.server_image_id
    cross_binary = args.transition_server_image_id is not None
    transition_reference = args.transition_server_image_id if cross_binary else source_reference
    target_migration = args.source_migration if args.target_migration is None else args.target_migration
    manager_reference = transition_reference if args.management_server_image_id is None else args.management_server_image_id
    manager_migration = target_migration if args.management_migration is None else args.management_migration
    for reference in [source_reference, transition_reference, manager_reference]:
        assert isinstance(reference, str) and re.fullmatch(r'sha256:[a-f0-9]{64}', reference), 'exact local server image IDs required'
    for version in [args.source_migration, target_migration, manager_migration]:
        assert type(version) is int and version > 0, 'positive migration versions required'
    assert target_migration >= args.source_migration, 'target migration precedes source'
    if cross_binary:
        assert transition_reference != source_reference, 'distinct source/target image IDs required'
        assert manager_reference == transition_reference and manager_migration == target_migration, 'cross-binary matrix requires the target manager and schema'
    LifecycleSchemaDelta(args.source_migration, target_migration)
    return {'source_reference': source_reference, 'cross_binary': cross_binary,
            'transition_reference': transition_reference, 'target_migration': target_migration,
            'manager_reference': manager_reference, 'manager_migration': manager_migration}


def outer(args):
    inputs = qualification_inputs(args)
    source_reference, cross_binary = inputs['source_reference'], inputs['cross_binary']
    transition_reference, target_migration = inputs['transition_reference'], inputs['target_migration']
    manager_reference, manager_migration = inputs['manager_reference'], inputs['manager_migration']
    assert not any(os.environ.get(name) for name in ['DOCKER_HOST', 'DOCKER_TLS_VERIFY', 'DOCKER_CERT_PATH']), 'remote Docker override forbidden'
    info = json.loads(docker('info', '--format', '{{json .}}'))
    assert info['OperatingSystem'] == 'Docker Desktop', 'qualification requires local Docker Desktop'
    daemon = info['ID']
    baseline = running_snapshot()
    scope = 'cowork-lifecycle-test-' + uuid.uuid4().hex
    root = Path(tempfile.mkdtemp(prefix=scope + '-'))
    repo = Path(__file__).resolve().parents[2]
    label = {'cowork.acceptance': scope}
    created = {'volumes': [], 'networks': []}
    source = root / 'qualify_candidate_lifecycle.py'
    shutil.copyfile(__file__, source)
    shutil.copyfile(Path(__file__).with_name('lifecycle_schema_delta.py'), root / 'lifecycle_schema_delta.py')
    helper = Path(__file__).with_name('candidate_lifecycle_workspace.py')
    if not args.basic:
        assert helper.is_file(), 'workspace helper missing'
        shutil.copyfile(helper, root / helper.name)
    images = {name: image(reference)[0] for name, reference in {
        'server': source_reference, 'runner': args.runner_image_id, 'sandbox_core': 'cowork-sandbox-core:local-review',
        'sandbox_gui': 'cowork-sandbox-gui:local-review', 'sandbox_crew': 'cowork-sandbox-crew:local-review',
        'egress_proxy': 'cowork-egress:local-review'}.items()}
    auxiliary = {name: image(reference)[0] for name, reference in {
        'provisioner': args.provisioner_image_id, 'postgres': 'postgres:16.10-bookworm', 'storage': 'cowork-storage:local-review',
        'web': 'cowork-web:local-review'}.items()}
    # The production image-retention path may add immutable candidate aliases.
    # Assert that it cannot change any pre-existing tag's meaning.
    selected = json.loads(docker('image', 'inspect', *sorted(set([*images.values(), *auxiliary.values(), transition_reference, manager_reference]))))
    protected_image_tags = {tag: value['Id'] for value in selected for tag in value.get('RepoTags') or []}
    retained_candidates = {}
    for reference in images.values():
        repository, digest = reference.split('@')
        tag = repository + ':retained-' + digest.replace(':', '-')
        present = bool(docker('image', 'ls', '--quiet', '--filter', 'reference=' + tag))
        identifier = image(reference)[1]
        if present: assert image(tag)[1] == identifier, 'pre-existing retention alias points to another image'
        retained_candidates[tag] = {'image_id': identifier, 'present_before': present}
    def write(name, value):
        (root / name).write_text(value, encoding='utf-8')
    def same_daemon():
        assert json.loads(docker('info', '--format', '{{json .}}'))['ID'] == daemon, 'Docker daemon changed'
        assert_preserved(baseline)
    def temporary_run(name, selected_image, entrypoint, command):
        same_daemon()
        return docker('run', '--rm', '--pull', 'never', '--name', scope + '-' + name, '--label', 'cowork.acceptance=' + scope,
            '--network', 'none', '--cpus', '1', '--memory', '256m', '--pids-limit', '64', '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges:true', '--mount', 'type=bind,src=' + str(root) + ',dst=/out',
            '--entrypoint', entrypoint, selected_image, *command)
    temporary_run('cert', images['server'], 'openssl', ['req', '-x509', '-newkey', 'rsa:3072', '-nodes', '-sha256', '-days', '2',
        '-subj', '/CN=operator-control', '-addext', 'subjectAltName=DNS:operator-control', '-addext', 'extendedKeyUsage=serverAuth',
        '-keyout', '/out/control_tls_key', '-out', '/out/control_tls_certificate'])
    temporary_run('cert-mode', images['server'], 'chmod', ['0444', '/out/control_tls_key', '/out/control_tls_certificate'])
    write('operator-control-nginx.conf', (repo / 'deploy/enterprise/operator-control-nginx.conf').read_text(encoding='utf-8'))
    target = root / 'target'; target.mkdir()
    # BuildKit cannot resolve this unpublished local repository digest in FROM.
    # Give only the already inspected candidate a fresh, fixture-owned local tag.
    source_tag = scope + ':source'
    assert not docker('image', 'ls', '--quiet', '--filter', 'reference=' + source_tag)
    docker('tag', transition_reference, source_tag)
    assert image(source_tag)[1] == image(transition_reference)[1]
    (target / 'Dockerfile').write_text('FROM ' + source_tag + '\nLABEL cowork.acceptance="' + scope + '"\n', encoding='utf-8')
    target_tag = scope + ':transition'
    same_daemon()
    docker('build', '--pull=false', '--tag', target_tag, str(target))
    target_image, target_id = image(target_tag)
    assert target_id != image(source_reference)[1], 'label-only transition must have a distinct immutable package identity'
    binary_hashes = {name: temporary_run('binary-' + name, reference, 'sha256sum', ['/usr/local/bin/cowork-server']).split()[0]
                     for name, reference in {'source': images['server'], 'transition': target_image}.items()}
    if cross_binary:
        assert binary_hashes['source'] != binary_hashes['transition'], 'cross-binary qualification requires genuinely different executable hashes'
    else:
        assert binary_hashes['source'] == binary_hashes['transition'], 'label-only qualification must preserve the executable'
    values = {name: secrets.token_hex(32) for name in ['management_password', 'fleet_password', 'bootstrap', 'broker', 's3_password']}
    values.update({'s3_user': 'fixture_' + uuid.uuid4().hex[:12],
        'management_url': 'postgres://fixture:' + values['management_password'] + '@management-db:5432/fixture',
        'fleet_url': 'postgres://fixture:' + values['fleet_password'] + '@fleet-db:5432/fixture',
        'auth_key': base64.b64encode(secrets.token_bytes(32)).decode()})
    for name, value in values.items(): write(name, value)
    (root / 'audit_key').write_bytes(secrets.token_bytes(32))
    write('empty.env', '')
    occupied = []
    networks_existing = docker('network', 'ls', '--quiet').splitlines()
    if networks_existing:
        occupied = [ipaddress.ip_network(entry['Subnet']) for network in json.loads(docker('network', 'inspect', *networks_existing))
                    for entry in network.get('IPAM', {}).get('Config', []) or [] if entry.get('Subnet')]
    def subnet(prefix):
        result = next((v for v in ipaddress.ip_network('10.249.0.0/16').subnets(new_prefix=prefix)
                       if not any(v.version == old.version and v.overlaps(old) for old in occupied)), None)
        assert result is not None, 'no unoccupied private fixture subnet'
        occupied.append(result)
        return str(result)
    selected_networks = {name: subnet(27) for name in ['management', 'services', 'routing']}
    tenant_pool = subnet(24)
    for suffix in ['state', 'management-data', 'fleet-data', 'storage-data']:
        name = scope + '-' + suffix
        same_daemon()
        docker('volume', 'create', '--label', 'cowork.acceptance=' + scope, name)
        created['volumes'].append(name)
    state = docker('volume', 'inspect', scope + '-state', '--format', '{{.Mountpoint}}')
    config = {'backend': 'compose', 'management_url': 'https://operator-control:8443',
        'management_ca_file': '/run/secrets/control_tls_certificate', 'acceptance_management_url': 'http://fixture-management:8080',
        'fleet_management_url': 'http://fixture-management:8080', 'provisioner_token_file': '/run/secrets/broker',
        'state_directory': state + '/instances', 'backup_directory': state + '/backups',
        'backup_identity_file': state + '/backup-identity', 'release_directory': state + '/releases',
        'deletion_enabled': True, 'database_endpoint': 'fleet-db:5432', 'database_admin_url_file': '/run/secrets/fleet_url',
        's3_endpoint': 'http://fleet-storage:9000', 's3_admin_alias': 'fixture', 's3_admin_config_directory': state + '/mc',
        'services_network': scope + '-services', 'routing_network': scope + '-routing', 'images': images,
        'compose_network_pool': tenant_pool, 'sandbox_security_profile': 'docker_desktop_seccomp',
        'acceptance_scope': scope, 'acceptance_target_image': target_image, 'acceptance_basic': args.basic,
        'acceptance_cross_binary': cross_binary, 'acceptance_source_migration': args.source_migration,
        'acceptance_target_migration': target_migration, 'acceptance_manager_migration': manager_migration}
    write('config.json', json.dumps(config))
    def bounds(cpu, memory, pids=128): return {'cpus': cpu, 'mem_limit': memory, 'pids_limit': pids, 'labels': label}
    def pg(secret, network, volume):
        return {'image': auxiliary['postgres'], **bounds(1, '512m'),
            'environment': {'POSTGRES_USER': 'fixture', 'POSTGRES_DB': 'fixture', 'POSTGRES_PASSWORD_FILE': '/run/secrets/' + secret},
            'secrets': [secret], 'networks': [network], 'volumes': [volume + ':/var/lib/postgresql/data'],
            'healthcheck': {'test': ['CMD-SHELL', 'pg_isready -U fixture -d fixture'], 'interval': '2s', 'timeout': '3s', 'retries': 40}}
    manager = {'image': image(manager_reference)[0], **bounds(1, '512m', 256), 'read_only': True, 'tmpfs': ['/tmp'], 'cap_drop': ['ALL'],
        'security_opt': ['no-new-privileges:true'], 'environment': {'COWORK_MODE': 'management', 'DATABASE_URL_FILE': '/run/secrets/management_url',
            'COWORK_BOOTSTRAP_TOKEN_FILE': '/run/secrets/bootstrap', 'COWORK_AUTH_MASTER_KEY_FILE': '/run/secrets/auth_key',
            'COWORK_MANAGEMENT_AUDIT_KEY_FILE': '/run/secrets/audit_key', 'COWORK_PROVISIONER_TOKEN_FILE': '/run/secrets/broker',
            'COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED': 'false', 'COWORK_MANAGEMENT_PROVISIONING_ENABLED': 'true',
            'COWORK_MANAGEMENT_BACKUPS_ENABLED': 'true', 'COWORK_MANAGEMENT_UPDATES_ENABLED': 'true',
            'COWORK_MANAGEMENT_DELETION_ENABLED': 'true', 'COWORK_WEB_PUSH_ENABLED': 'false',
            'COWORK_INSTANCE_URL_TEMPLATE': 'http://{instance}.localhost'},
        'secrets': ['management_url', 'bootstrap', 'auth_key', 'audit_key', 'broker'],
        'networks': {'management': {}, 'services': {'aliases': ['fixture-management']}},
        'depends_on': {'management-db': {'condition': 'service_healthy'}},
        'healthcheck': {'test': ['CMD', 'curl', '--fail', '--silent', 'http://127.0.0.1:8080/readyz'], 'interval': '2s', 'timeout': '3s', 'retries': 60}}
    manifest = {'name': scope, 'services': {'management-db': pg('management_password', 'management', 'management-data'),
        'fleet-db': pg('fleet_password', 'services', 'fleet-data'), 'management': manager,
        'fleet-storage': {'image': auxiliary['storage'], **bounds(1, '512m', 256), 'command': ['server', '/data'],
            'environment': {'MINIO_ROOT_USER_FILE': '/run/secrets/s3_user', 'MINIO_ROOT_PASSWORD_FILE': '/run/secrets/s3_password'},
            'secrets': ['s3_user', 's3_password'], 'volumes': ['storage-data:/data'], 'networks': ['services'],
            'healthcheck': {'test': ['CMD', 'curl', '--fail', '--silent', 'http://127.0.0.1:9000/minio/health/live'], 'interval': '2s', 'timeout': '3s', 'retries': 40}},
        'operator-control': {'image': auxiliary['web'], **bounds(.25, '128m'), 'read_only': True, 'tmpfs': ['/tmp'],
            'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true'], 'networks': ['management'],
            'secrets': ['control_tls_key', 'control_tls_certificate'],
            'volumes': [{'type': 'bind', 'source': str(root / 'operator-control-nginx.conf'), 'target': '/etc/nginx/conf.d/default.conf', 'read_only': True}],
            'depends_on': {'management': {'condition': 'service_healthy'}}},
        'operator': {'image': auxiliary['provisioner'], **bounds(1, '512m', 256), 'entrypoint': ['sleep', 'infinity'],
            'networks': ['management', 'services'], 'read_only': True, 'tmpfs': ['/tmp'], 'cap_drop': ['ALL'],
            'security_opt': ['no-new-privileges:true'], 'volumes': ['/var/run/docker.sock:/var/run/docker.sock', 'state:' + state,
                {'type': 'bind', 'source': str(root), 'target': '/fixture'}],
            'secrets': ['bootstrap', 'broker', 'fleet_url', 'management_url', 's3_user', 's3_password', 'control_tls_certificate'],
            'depends_on': {**{name: {'condition': 'service_healthy'} for name in ['management', 'fleet-db', 'fleet-storage']}, 'operator-control': {'condition': 'service_started'}}}},
        'networks': {name: {'name': scope + '-' + name, 'internal': True, 'labels': label, 'ipam': {'config': [{'subnet': selected_networks[name]}]}}
                     for name in ['management', 'services']},
        'volumes': {name: {'external': True, 'name': scope + '-' + suffix} for name, suffix in {
            'state': 'state', 'management-data': 'management-data', 'fleet-data': 'fleet-data', 'storage-data': 'storage-data'}.items()},
        'secrets': {name: {'file': str(root / name)} for name in [*values, 'audit_key', 'control_tls_key', 'control_tls_certificate']}}
    assert all('ports' not in service for service in manifest['services'].values())
    write('compose.json', json.dumps(manifest))
    marker = {'scope': scope, 'root': str(root), 'images': images, 'auxiliary': auxiliary, 'target_image': target_image,
        'actual_image_ids': {name: image(reference)[1] for name, reference in {**images, **auxiliary}.items()},
        'image_evidence': {name: image_evidence(reference) for name, reference in {**images, **auxiliary, 'transition': target_image}.items()},
        'requested_candidate_image_ids': {'server': args.server_image_id, 'runner': args.runner_image_id, 'provisioner': args.provisioner_image_id,
            'transition_server': transition_reference, 'management_server': manager_reference},
        'transition_image_id': target_id, 'fixture_alias_base_image_id': image(transition_reference)[1],
        'management_server_image_id': image(manager_reference)[1],
        'server_binary_sha256': binary_hashes, 'transition_dockerfile_sha256': hashlib.sha256((target / 'Dockerfile').read_bytes()).hexdigest(),
        'control_resource_limits': {name: {key: service[key] for key in ['cpus', 'mem_limit', 'pids_limit']}
                                    for name, service in manifest['services'].items()},
        'protected_image_tags': protected_image_tags, 'retained_candidate_references': retained_candidates,
        'baseline_container_images': baseline, 'daemon_id': daemon, 'created': created, 'live_modified': False,
        'release_transition': 'distinct executable hashes' if cross_binary else 'same binary in a distinct labelled package',
        'source_migration': args.source_migration, 'target_migration': target_migration, 'manager_migration': manager_migration}
    write('fixture.json', json.dumps(marker, indent=2))
    print('Fixture: ' + scope + '\nPrivate report: ' + str(root / 'results.json'), flush=True)
    compose = ['compose', '--env-file', str(root / 'empty.env'), '--project-name', scope, '--file', str(root / 'compose.json')]
    try:
        same_daemon()
        docker(*compose, 'up', '--detach', '--wait', '--wait-timeout', '180', timeout=240)
        for name in ['management', 'services']:
            created['networks'].append(scope + '-' + name)
        same_daemon()
        docker('network', 'create', '--internal', '--subnet', selected_networks['routing'], '--label', 'cowork.acceptance=' + scope, scope + '-routing')
        created['networks'].append(scope + '-routing')
        operator = docker(*compose, 'ps', '--quiet', 'operator')
        management = docker(*compose, 'ps', '--quiet', 'management')
        command = ['docker', 'exec', '--env', 'PYTHONPATH=/opt/cowork-provisioner:/fixture', operator, 'python3', '-u',
            '/fixture/qualify_candidate_lifecycle.py', '--inside', '--management-container', management]
        completed = subprocess.run(command, timeout=2400)
        assert completed.returncode == 0, 'candidate lifecycle failed; new fixture retained'
        result = json.loads((root / 'results.json').read_text())
        assert result['tenant_deleted'] is True
        same_daemon()
        containers = docker('ps', '--all', '--quiet', '--no-trunc', '--filter', 'label=cowork.acceptance=' + scope).splitlines()
        inspected = json.loads(docker('inspect', *containers)) if containers else []
        for value in inspected:
            assert value['Id'] not in baseline and value['Config']['Labels']['com.docker.compose.project'] == scope
            assert value['Name'].startswith('/' + scope + '-')
        if containers: docker('rm', '--force', *[v['Id'] for v in inspected])
        for name in created['networks']:
            current = json.loads(docker('network', 'inspect', name))[0]
            assert current['Name'] == name and current['Labels']['cowork.acceptance'] == scope and not current['Containers']
            docker('network', 'rm', current['Id'])
        for name in created['volumes']:
            current = json.loads(docker('volume', 'inspect', name))[0]
            assert current['Name'] == name and current['Labels']['cowork.acceptance'] == scope
            docker('volume', 'rm', name)
        assert image(target_tag)[1] == target_id
        remove_fixture_images(scope, baseline, marker.get('fixture_alias_base_image_id', marker['actual_image_ids']['server']))
        for tag, identifier in protected_image_tags.items():
            assert image(tag)[1] == identifier, 'pre-existing image tag changed'
        for tag, evidence in retained_candidates.items():
            assert image(tag)[1] == evidence['image_id'], 'candidate retention alias changed'
            evidence['present_after'] = True
        same_daemon()
        result.update(marker); result['fixture_removed'] = True; result['live_containers_preserved'] = len(baseline)
        write('results.json', json.dumps(result, indent=2))
        remove_fixture_secrets(root)
        print('PASS: fixture removed; existing runtimes preserved; report=' + str(root / 'results.json'), flush=True)
    except Exception:
        same_daemon()
        print('Only newly created fixture retained: ' + str(root / 'fixture.json'), flush=True)
        raise


def inside(args):
    import provisioner as p
    from instance_backups import pg_environment
    from operation_journal import OperationJournal
    from release_catalog import ReleaseCatalog, image_set_id
    config = json.loads(Path('/fixture/config.json').read_text())
    scope = config['acceptance_scope']
    assert re.fullmatch(r'cowork-lifecycle-test-[0-9a-f]{32}', scope)
    assert config['services_network'] == scope + '-services' and config['routing_network'] == scope + '-routing'
    state = Path(config['state_directory']).parent
    assert state == Path('/var/lib/docker/volumes') / (scope + '-state') / '_data'
    manager_labels = json.loads(p.run(['docker', 'inspect', args.management_container, '--format', '{{json .Config.Labels}}']))
    assert manager_labels['com.docker.compose.project'] == scope and manager_labels['cowork.acceptance'] == scope
    original_manifest = p.compose_manifest
    chat_enabled = False
    def private_manifest(*values, **keywords):
        result = original_manifest(*values, **keywords)
        for name in ['api', 'worker']:
            result['services'][name]['environment']['COWORK_CHAT_WORKSPACES_ENABLED'] = 'true' if chat_enabled else 'false'
        result['networks']['internet']['internal'] = True
        return result
    p.compose_manifest = private_manifest
    manager, infra = p.Manager(config), p.Infrastructure(config)
    if args.inside_cleanup:
        descriptor = state / 'tenant-descriptor.json'
        with p.operator_lock(infra.root):
            if descriptor.exists():
                from instance_deletion import InstanceDeletion
                instance = json.loads(descriptor.read_text())
                assert instance['name'] == scope + ' tenant' and instance['backend'] == 'compose'
                identifier = str(uuid.UUID(instance['id']))
                assert instance['id'] == identifier
                directory = infra.directory(identifier)
                if not (directory / 'deleted.json').exists():
                    InstanceDeletion(infra).execute({'id': uid(), 'action': 'delete', 'instance': instance,
                        'arguments': {'confirmation_name': instance['name'], 'purge_backups': True}}, directory)
                assert not infra.run(['docker', 'ps', '--all', '--quiet', '--filter', 'name=^/cowork-' + uuid.UUID(identifier).hex + '-']).strip()
            else:
                assert not any(path.is_dir() and re.fullmatch('[0-9a-f]{32}', path.name) for path in infra.root.iterdir()), 'unknown tenant state retained'
        return
    http = build_opener(ProxyHandler({}))
    checks, operations, runtime_snapshots, health_snapshots = [], [], [], []
    observed_process_ids = set()
    def check(name): checks.append(name); print(name + '=ok', flush=True)
    def request(origin, path, token=None, body=None, expected=200, method=None, raw=False):
        headers = {'content-type': 'application/json'}
        if token: headers['authorization'] = 'Bearer ' + token
        data = body if isinstance(body, bytes) else None if body is None else json.dumps(body).encode()
        req = Request(origin + path, data=data, headers=headers, method=method)
        try:
            with http.open(req, timeout=25) as response: status, content = response.status, response.read()
        except HTTPError as error: status, content = error.code, error.read()
        if status != expected:
            detail = '' if '/auth/' in path or '/credentials' in path or '/launch' in path else content.decode(errors='replace')[:500]
            raise AssertionError(f'{req.get_method()} {path}: {status}, expected {expected}; {detail}')
        return content if raw else json.loads(content) if content and status < 400 else None
    def sql(url, statement):
        return p.run(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'], env=pg_environment(url), data=statement.encode()).decode().strip()
    management = config['acceptance_management_url']
    management_db = Path('/run/secrets/management_url').read_text().strip()
    assert sql(management_db, 'SELECT max(version) FROM _sqlx_migrations;') == str(config['acceptance_manager_migration'])
    check('candidate_management_expected_migration')
    (state / 'mc').mkdir(mode=0o700, exist_ok=True)
    p.run(['mc', '--config-dir', str(state / 'mc'), 'alias', 'set', 'fixture', config['s3_endpoint'],
        Path('/run/secrets/s3_user').read_text().strip(), Path('/run/secrets/s3_password').read_text().strip()])
    p.run(['age-keygen', '--output', config['backup_identity_file']])
    bootstrap = Path('/run/secrets/bootstrap').read_text().strip()
    admin = request(management, '/api/v1/auth/bootstrap', bootstrap,
        {'email': 'admin@lifecycle.test', 'display_name': 'Private fixture admin', 'password': secrets.token_urlsafe(32), 'device_id': uid()}, expected=201)
    setup = request(management, '/api/v1/auth/totp/setup', admin['access_token'], {})
    digest = hmac.new(base64.b32decode(setup['secret'] + '=' * ((8 - len(setup['secret']) % 8) % 8)),
                      (int(time.time()) // 30).to_bytes(8, 'big'), hashlib.sha1).digest()
    offset = digest[-1] & 15
    code = f'{(int.from_bytes(digest[offset:offset + 4], "big") & 0x7fffffff) % 1000000:06d}'
    request(management, '/api/v1/auth/totp/enable', admin['access_token'], {'code': code})
    org = request(management, '/management/v1/organisations', admin['access_token'], {'name': scope})['id']
    base = '/management/v1/organisations/' + org
    created = request(management, base + '/instances', admin['access_token'], {'name': scope + ' tenant', 'backend': 'compose'})
    assert str(uuid.UUID(created['id'])) == created['id']
    check('fresh_bootstrap_admin_totp_organisation_and_instance')
    class Context:
        project = None
        token = None
        session_tokens = None
        instance = created
        directory = infra.directory(created['id'])
        origin = 'http://cowork-' + uuid.UUID(created['id']).hex + '-api-1:8080'
        def current(self):
            self.instance = next(v for v in request(management, base + '/instances', admin['access_token']) if v['id'] == created['id'])
            assert self.instance['name'] == scope + ' tenant'
            return self.instance
        def active(self): return infra.active_directory(self.directory)
        def _sql(self, statement): return sql((self.active() / 'secrets/database_url').read_text().strip(), statement)
        def sql(self, statement):
            if 'jsonb_agg(to_jsonb(r) ORDER BY id)' in statement:
                current = int(self._sql('SELECT max(version) FROM _sqlx_migrations;'))
                return self.schema_delta.historical_runs(self._sql, statement, current)
            return self._sql(statement)
        def request(self, *values, **keywords): return request(*values, **keywords)
        def check(self, name): return check(name)
        def observe_health(self, phase, *, worker=True):
            samples = []
            deployed = infra.deployed_images(self.directory)["server"]
            if deployed == config["acceptance_target_image"]:
                expected_migration = config["acceptance_target_migration"]
            else:
                assert deployed == config["images"]["server"], "unexpected server image in lifecycle probe"
                expected_migration = config["acceptance_source_migration"]
            def healthy():
                rows = json.loads(self.sql("SELECT coalesce(jsonb_agg(jsonb_build_object('process_id',process_id,'mode',process_mode,'migration',migration_version,'last_seen_at',last_seen_at,'observations',observations,'now_ms',(extract(epoch FROM now())*1000)::bigint)),'[]'::jsonb) FROM server_process_health WHERE lifecycle='running' AND last_seen_at>now()-interval '20 seconds';"))
                expected = {'api', 'worker'} if worker else {'api'}
                if len(rows) != len(expected) or {row['mode'] for row in rows} != expected:
                    return False
                for row in rows:
                    if row['migration'] != expected_migration or row['process_id'] in observed_process_ids:
                        return False
                    if row['mode'] == 'worker':
                        for name in ['scheduler_tick_ms', 'maintenance_tick_ms', 'dispatch_tick_ms']:
                            tick = row['observations'].get(name, 0)
                            if tick <= 0 or not 0 <= row['now_ms'] - tick <= 20000:
                                return False
                samples[:] = rows
                return True
            wait(healthy, 'current fixture processes lack the expected schema health or actual worker loop ticks', seconds=40)
            observed_process_ids.update(row['process_id'] for row in samples)
            health_snapshots.append({'phase': phase, 'generation': infra.resource_names(self.current(), self.active())['generation'], 'processes': samples})
        def api(self, path, body=None, expected=200, method=None, raw=False):
            return request(self.origin, '/api/v1' + path, self.token, body, expected, method, raw)
        def login(self):
            ticket = request(management, base + '/instances/' + created['id'] + '/launch', admin['access_token'], {})
            self.session_tokens = request(self.origin, '/api/v1/auth/fleet', body={'ticket': ticket['ticket'], 'device_id': uid()})
            self.token = self.session_tokens['access_token']
            return self.token
        def lifecycle(self, action, expected='succeeded', **extra):
            row = self.current()
            ident = request(management, base + '/instances/' + created['id'] + '/operations', admin['access_token'],
                {'action': action, 'expected_revision': row['revision'], 'idempotency_key': uid(), **extra})['id']
            operation = manager.call('/claim', p.operator_contract(config))
            assert operation['id'] == ident and operation['instance_id'] == created['id'] and operation['organisation_id'] == org
            assert operation['instance']['name'] == scope + ' tenant'
            p.private_write(state / 'tenant-descriptor.json', json.dumps(operation['instance']).encode())
            Path('/fixture/tenant-proof.json').write_text(json.dumps({name: operation['instance'][name]
                for name in ['id', 'organisation_id', 'name', 'backend']}))
            journal = OperationJournal(infra, operation); journal.begin()
            try: result, succeeded = infra.execute(operation), True
            except p.ProvisioningError as error:
                result, succeeded = {**error.result, 'error_code': str(error)}, False
                if error.instance_state: result['instance_state'] = error.instance_state
                if expected != 'failed': raise
            completion = journal.finish(succeeded, result)
            manager.call('/operations/' + ident + '/complete', completion)
            manager.call('/operations/' + ident + '/complete', journal.recover())
            if action == 'delete':
                result_row = json.loads(sql(management_db, "SELECT json_build_object('id',id,'state',state,'result',result) FROM management_operations WHERE id='" + ident + "' AND instance_id='" + created['id'] + "' AND organisation_id='" + org + "';"))
                request(management, base + '/instances/' + created['id'] + '/operations', admin['access_token'], expected=404)
            else:
                result_row = next(v for v in request(management, base + '/instances/' + created['id'] + '/operations', admin['access_token']) if v['id'] == ident)
            assert result_row['state'] == expected, action + ' incorrect terminal state'
            operations.append({'id': ident, 'action': action, 'state': result_row['state'], 'result': result_row['result']})
            if action != 'delete':
                manifest = json.loads((self.directory / 'compose.json').read_text())
                for name in ['api', 'worker']:
                    assert manifest['services'][name]['environment']['COWORK_CHAT_WORKSPACES_ENABLED'] == ('true' if chat_enabled else 'false')
                if action in ['provision', 'start', 'resume']:
                    identifiers = infra.compose(self.current(), self.directory, 'ps', '--quiet').decode().splitlines()
                    values = json.loads(infra.run(['docker', 'inspect', *identifiers]))
                    actual = {}
                    for value in values:
                        labels = value['Config']['Labels']
                        assert labels['com.docker.compose.project'] == 'cowork-' + uuid.UUID(created['id']).hex
                        service = labels['com.docker.compose.service']
                        assert service in manifest['services'] and value['State']['Running']
                        expected_image = json.loads(infra.run(['docker', 'image', 'inspect', manifest['services'][service]['image']]))[0]['Id']
                        assert value['Image'] == expected_image
                        actual[service] = {'container_id': value['Id'], 'image_id': value['Image']}
                        actual[service]['resource_limits'] = {key: manifest['services'][service][key]
                            for key in ['cpus', 'mem_limit', 'pids_limit']}
                        assert not manifest['services'][service].get('ports'), 'fixture must not publish tenant ports'
                    assert set(actual) == {'api', 'worker', 'runner', 'egress-proxy'}
                    runtime_snapshots.append({'operation_id': ident, 'action': action, 'generation': infra.resource_names(self.current(), self.active())['generation'], 'services': actual})
                    self.observe_health(action + ':' + ident)
            return result_row
        def run_shell(self, thread_id, script):
            limits = {'cpu_nanos': 100000000, 'memory_bytes': 128 * 1024 * 1024, 'pids': 64, 'timeout_seconds': 20,
                      'tmpfs_bytes': 4 * 1024 * 1024, 'output_bytes': 4096}
            run_request = {'project_id': self.project['id'], 'thread_id': thread_id, 'project_revision': self.project['revision'],
                'project_privacy': 'team_managed', 'executor_target': {'kind': 'server_linux', 'pool_id': None},
                'required_capabilities': ['shell'], 'idempotency_key': uid(), 'input': {'prompt': 'Private lifecycle proof',
                    'sandbox': {'schema_version': 2, 'run_id': uid(), 'image': 'core', 'argv': ['python3', '-c', script], 'network': 'none', 'limits': limits}}}
            run = self.api('/threads/' + thread_id + '/messages',
                {'content': {'text': 'Private lifecycle conversation proof'}, 'run': run_request}, expected=201)['run']
            ident = run['spec']['id']
            records = []
            def done():
                value = self.api('/runs/' + ident); records.append(value)
                return value['state'] in ['completed', 'failed', 'canceled']
            wait(done, 'own fixture sandbox task did not complete')
            assert records[-1]['state'] == 'completed', json.dumps(records[-1].get('error'))
            return records[-1]
        def set_chat_enabled(self, enabled):
            nonlocal chat_enabled
            if self.current()['state'] == 'ready': self.lifecycle('stop')
            chat_enabled = enabled
            self.lifecycle('start'); self.login()
        def read_workspace(self, volume_name, relative_path):
            prefixes = [infra.resource_names(self.current(), path)['volume_prefix'] for path in [self.directory, *list((self.directory / 'generations').glob('*'))]
                        if path.is_dir() and (path / 'secrets').is_dir()]
            assert any(re.fullmatch(re.escape(prefix) + '[0-9a-f]{32}', volume_name) for prefix in prefixes), 'foreign workspace read forbidden'
            assert re.fullmatch(r'[a-zA-Z0-9._/-]+', relative_path) and '..' not in relative_path.split('/')
            infra.run(['docker', 'volume', 'inspect', volume_name])
            return infra.run(['docker', 'run', '--rm', '--pull', 'never', '--name', scope + '-read-' + uuid.uuid4().hex,
                '--label', 'cowork.acceptance=' + scope, '--network', 'none', '--read-only', '--cpus', '.25', '--memory', '128m',
                '--pids-limit', '32', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
                '--mount', 'type=volume,src=' + volume_name + ',dst=/workspace,readonly', images['sandbox_core'], 'cat', '/workspace/' + relative_path])
    images = config['images']
    ctx = Context(); ctx.config, ctx.infra = config, infra
    ctx.schema_delta = LifecycleSchemaDelta(config['acceptance_source_migration'], config['acceptance_target_migration'])
    with p.operator_lock(infra.root):
        ctx.lifecycle('provision'); ctx.login()
        assert ctx.sql('SELECT max(version) FROM _sqlx_migrations;') == str(config['acceptance_source_migration'])
        team = ctx.api('/teams', {'name': scope}, expected=201)
        ctx.project = ctx.api('/projects', {'name': 'Lifecycle before backup', 'privacy': 'team_managed', 'team_id': team['id']}, expected=201)
        if config['acceptance_basic']:
            thread = ctx.api('/threads', {'project_id': ctx.project['id'], 'title': 'Basic lifecycle'}, expected=201)
            run = ctx.run_shell(thread['id'], "from pathlib import Path; Path('/workspace/marker.txt').write_text('private-basic'); print('private-basic')")
            assert run['result']['sandbox']['stdout'].strip() == 'private-basic'
            check('basic_candidate_shell_with_legacy_workspace')
            workspace_state = None
        else:
            from candidate_lifecycle_workspace import (prepare_workspace_checks, assert_restored_workspace_checks,
                assert_current_workspace_checks, arm_schedule_for_upgrade_checks, assert_candidate_schedule_paused,
                capture_activation_checks, assert_activation_unchanged)
            workspace_state = prepare_workspace_checks(ctx)
        ctx.lifecycle('stop'); backup = ctx.lifecycle('backup')
        entry = next(v for v in request(management, base + '/instances/' + created['id'] + '/backups', admin['access_token']) if v['id'] == backup['result']['backup_id'])
        assert entry['size_bytes'] > 0 and re.fullmatch('[0-9a-f]{64}', entry['sha256'])
        check('real_stopped_encrypted_backup_and_catalog')
        before = {name: (ctx.directory / name).read_bytes() if (ctx.directory / name).exists() else None for name in ['active.json', 'compose.json']}
        activation_before = capture_activation_checks(ctx, workspace_state) if workspace_state else None
        archive = Path(config['backup_directory']) / (uuid.UUID(entry['id']).hex + '.age')
        original = archive.read_bytes()
        try:
            p.private_write(archive, original + b'tampered-fixture')
            rejected = ctx.lifecycle('restore', expected='failed', backup_id=entry['id'])
            assert rejected['result']['error_code'] == 'backup_catalog_integrity_failed'
            assert ctx.current()['state'] == 'stopped'
            assert all(((ctx.directory / name).read_bytes() if (ctx.directory / name).exists() else None) == value for name, value in before.items())
            if workspace_state: assert_activation_unchanged(ctx, workspace_state, activation_before)
        finally: p.private_write(archive, original)
        check('tampered_archive_rejected_before_activation_changes')
        mutations = None
        if workspace_state:
            ctx.lifecycle('start'); ctx.login()
            post_backup_marker = 'after-backup-' + uuid.uuid4().hex
            changed = ctx.run_shell(workspace_state['chat_thread'],
                "from pathlib import Path; Path('lifecycle-marker.txt').write_text(" + repr(post_backup_marker) + "); print('post-backup-change')")
            project = ctx.api('/projects/' + ctx.project['id'])
            ctx.project = ctx.api('/projects/' + project['id'],
                {'expected_revision': project['revision'], 'name': 'Changed only after backup'}, method='PUT')
            extra_project = ctx.api('/projects', {'name': 'Created only after backup', 'privacy': 'private_local'}, expected=201)
            mutations = {'run_id': changed['spec']['id'], 'project_id': extra_project['id'], 'marker': post_backup_marker}
            ctx.lifecycle('stop')
        restored = ctx.lifecycle('restore', backup_id=entry['id'])
        ctx.lifecycle('start'); ctx.login()
        if workspace_state:
            ctx.project = ctx.api('/projects/' + workspace_state['project_ids'][0])
            assert ctx.sql("SELECT count(*) FROM runs WHERE id='" + mutations['run_id'] + "';") == '0'
            ctx.api('/projects/' + mutations['project_id'], expected=404)
            source_volume = workspace_state['resources']['volume_prefix'] + uuid.UUID(workspace_state['workspace_id']).hex
            assert ctx.read_workspace(source_volume, 'lifecycle-marker.txt') == mutations['marker'].encode()
            assert_restored_workspace_checks(ctx, workspace_state, restored)
            check('restore_recovers_backup_content_and_excludes_later_database_and_workspace_mutations')
        else:
            prefix = infra.resource_names(ctx.current(), ctx.active())['volume_prefix']
            assert ctx.read_workspace(prefix + uuid.UUID(run['spec']['id']).hex, 'marker.txt').strip() == b'private-basic'
        check('real_restore_uses_fresh_generation_and_preserves_workspace')
        if workspace_state: arm_schedule_for_upgrade_checks(ctx, workspace_state)
        ctx.lifecycle('stop')
        source_images = infra.deployed_images(ctx.directory)
        target_images = {**source_images, 'server': config['acceptance_target_image']}
        catalog = ReleaseCatalog(config, p.private_write)
        catalog.publish(scope + ' source', source_images, [image_set_id(target_images)])
        target = catalog.publish(scope + (' cross-binary' if config['acceptance_cross_binary'] else ' same-binary-package'), target_images, [image_set_id(source_images)])
        p.sync_release_catalog(manager, infra)
        if workspace_state:
            from instance_updates import InstanceUpdates
            original_probe = InstanceUpdates.probe
            original_probe_services = InstanceUpdates._probe_services
            def candidate_probe_services(engine, *values, **keywords):
                original_probe_services(engine, *values, **keywords)
                ctx.observe_health('private_update_probe', worker=keywords['worker'])
            InstanceUpdates._probe_services = candidate_probe_services
            def candidate_probe(engine, *values, **keywords):
                original_probe(engine, *values, **keywords)
                if keywords.get('worker') is True:
                    assert_candidate_schedule_paused(ctx, workspace_state)
            InstanceUpdates.probe = candidate_probe
        original_active = ctx.active()
        canary = ctx.lifecycle('upgrade', release=target['id'], qualification_only=True)
        assert canary['result']['rollback_verified'] is True and ctx.active() == original_active
        assert infra.deployed_images(ctx.directory) == source_images
        if workspace_state:
            ctx.lifecycle('start'); ctx.login()
            assert_current_workspace_checks(ctx, workspace_state)
            ctx.lifecycle('stop')
        promoted = ctx.lifecycle('upgrade', release=target['id'], qualification_only=False)
        assert infra.deployed_images(ctx.directory) == target_images and ctx.active() != original_active
        ctx.lifecycle('start'); ctx.login()
        assert ctx.sql('SELECT max(version) FROM _sqlx_migrations;') == str(config['acceptance_target_migration'])
        if workspace_state: assert_current_workspace_checks(ctx, workspace_state)
        check('real_canary_and_cross_binary_promotion' if config['acceptance_cross_binary'] else 'real_canary_and_same_binary_package_promotion')
        ctx.lifecycle('stop')
        if workspace_state: InstanceUpdates.probe = original_probe
        ctx.lifecycle('rollback', rollback_operation_id=promoted['id'])
        assert infra.deployed_images(ctx.directory) == source_images
        ctx.lifecycle('start'); ctx.login()
        assert ctx.sql('SELECT max(version) FROM _sqlx_migrations;') == str(config['acceptance_source_migration'])
        if workspace_state:
            workspace_state['schedule_paused'] = True
            assert_current_workspace_checks(ctx, workspace_state)
        check('real_rollback_preserves_workspace_bindings_and_files')
        if workspace_state: InstanceUpdates._probe_services = original_probe_services
        ctx.lifecycle('delete', confirmation_name=scope + ' tenant', purge_backups=True)
        assert all(value['id'] != created['id'] for value in request(management, base + '/instances', admin['access_token']))
        assert not infra.run(['docker', 'ps', '--all', '--quiet', '--filter', 'name=^/cowork-' + uuid.UUID(created['id']).hex + '-']).strip()
        check('real_deletion_removes_only_fresh_instance_and_retained_generations')
    report = {'checks': checks, 'operations': operations, 'instance_id': created['id'], 'organisation_id': org,
        'runtime_snapshots': runtime_snapshots,
        'health_snapshots': health_snapshots,
        'tenant_deleted': True, 'migration': config['acceptance_target_migration'], 'basic': config['acceptance_basic'], 'live_modified': False,
        'source_migration': config['acceptance_source_migration'], 'manager_migration': config['acceptance_manager_migration'],
        'release_transition': 'different executable hashes and isolated database generations' if config['acceptance_cross_binary'] else 'same binary, distinct labelled package',
        'same_binary_repackage': not config['acceptance_cross_binary'], 'cross_version_qualified': config['acceptance_cross_binary'],
        'qualification_scope': {'fresh_local_lifecycle': True, 'remote_lifecycle': False,
                                'central_execution_admission': False, 'live_fleet_activation': False},
        'private_manifest_overrides': ['API/worker chat flag', 'internal-only tenant internet network'],
        'harness_sha256': hashlib.sha256(Path('/fixture/qualify_candidate_lifecycle.py').read_bytes()).hexdigest(),
        'workspace_helper_sha256': hashlib.sha256(Path('/fixture/candidate_lifecycle_workspace.py').read_bytes()).hexdigest() if not config['acceptance_basic'] else None,
        'schema_delta_evidence': ctx.schema_delta.evidence(),
        'schema_delta_helper_sha256': hashlib.sha256(Path('/fixture/lifecycle_schema_delta.py').read_bytes()).hexdigest(),
        'completed_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
    Path('/fixture/results.json').write_text(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--basic', action='store_true', help='First qualify the narrow shell/backup/restore/update/rollback/deletion cycle')
    def local_image_id(value):
        if not re.fullmatch(r'sha256:[a-f0-9]{64}', value):
            raise argparse.ArgumentTypeError('an exact local docker image inspect .Id is required')
        return value
    parser.add_argument('--server-image-id', type=local_image_id, default=SERVER)
    parser.add_argument('--transition-server-image-id', type=local_image_id, help='Different executable used for the actual canary/promotion target')
    parser.add_argument('--management-server-image-id', type=local_image_id, help='Manager executable, defaults to the transition target')
    parser.add_argument('--source-migration', type=int, default=52)
    parser.add_argument('--target-migration', type=int)
    parser.add_argument('--management-migration', type=int)
    parser.add_argument('--provisioner-image-id', type=local_image_id, default=PROVISIONER)
    parser.add_argument('--runner-image-id', type=local_image_id, default=RUNNER)
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--inside-cleanup', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--management-container', help=argparse.SUPPRESS)
    parser.add_argument('--cleanup-fixture', type=Path, help='Remove only a proven UUID fixture created by this harness')
    args = parser.parse_args()
    if args.cleanup_fixture: cleanup_fixture(args.cleanup_fixture)
    elif args.inside: inside(args)
    else: outer(args)


if __name__ == '__main__':
    main()
