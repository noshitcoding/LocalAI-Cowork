#!/usr/bin/env python3
"""Back up the local control plane and rehearse migration on its isolated copy.

Explicit --capture-local authorizes read-only capture from the Docker Desktop
profile. No source service is stopped, upgraded or authenticated against. Only
the generated, labelled rehearsal containers are mutated. This is preparation,
not a live cutover or a complete tenant-data disaster-recovery backup.
"""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import tarfile
import tempfile
import time
import uuid
from datetime import datetime, timezone
from urllib.parse import urlsplit, unquote

diagnostics = None


def command(*args, data=None):
    result = subprocess.run(args, input=data, capture_output=True)
    if result.returncode:
        # Subprocess arguments/outputs may contain private runtime configuration.
        if diagnostics is not None:
            (diagnostics / ('command-failure-' + uuid.uuid4().hex + '.log')).write_bytes(result.stderr)
        raise RuntimeError(f'{args[0]} {args[1]} failed (exit {result.returncode})')
    return result.stdout


def docker(*args, data=None):
    return command('docker', *args, data=data)


def checksum(data):
    return hashlib.sha256(data).hexdigest()


def main():
    global diagnostics
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--capture-local', action='store_true', required=True)
    parser.add_argument('--stopped-control-plane', action='store_true',
                        help='Require API, operator and management web to be stopped; this command never stops/starts them')
    parser.add_argument('--source-server-image', required=True)
    parser.add_argument('--target-server-image', required=True)
    parser.add_argument('--operator-image', required=True)
    parser.add_argument('--source-migration', type=int, default=43)
    parser.add_argument('--target-migration', type=int, default=48)
    args = parser.parse_args()
    if (args.source_migration, args.target_migration) not in [(43,48),(48,49)]:
        parser.error('supported control transitions are 43 to 48 and 48 to 49')
    repo = Path(__file__).resolve().parent.parent
    private = repo / '.internal/enterprise-local'
    database_url = urlsplit((private / 'management_database_url').read_text(encoding='utf-8').strip())
    assert database_url.hostname == 'management-db' and database_url.port == 5432
    assert database_url.username == 'cowork_management'
    source_database = unquote(database_url.path.removeprefix('/'))
    assert re.fullmatch(r'cowork_management(?:_(?:recovery|release)_[a-f0-9]{12})?', source_database)
    scope = 'cowork-control-rehearsal-' + uuid.uuid4().hex[:12]
    output = private / scope
    output.mkdir(mode=0o700)
    diagnostics = output
    stage = Path(tempfile.mkdtemp(prefix=scope + '-'))
    if os.name == 'nt':
        identity = command('whoami').decode().strip()
        command('icacls', str(stage), '/inheritance:r', '/grant:r', identity + ':(OI)(CI)F', 'SYSTEM:(OI)(CI)F')
    report = {'scope': scope, 'live_modified': False, 'deployed': False, 'checks': [],
              'started_at': datetime.now(timezone.utc).isoformat(),
              'source_control_plane_stopped': args.stopped_control_plane,
              'script_sha256': checksum(Path(__file__).read_bytes())}
    created = []
    network_created = False

    def passed(name):
        report['checks'].append(name)
        print(name + '=ok', flush=True)

    def inspect(name):
        return json.loads(docker('inspect', name))[0]

    def image(reference):
        assert re.fullmatch(r'[a-z0-9./_-]+@sha256:[a-f0-9]{64}', reference), 'immutable repository digest required'
        result = json.loads(docker('image', 'inspect', reference))[0]
        assert reference in result['RepoDigests']
        return result

    def source_sql(statement):
        return docker('exec', '--env', 'PGOPTIONS=-c default_transaction_read_only=on -c statement_timeout=10000',
                      'cowork-enterprise-local-management-db-1', 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1',
                      '-U', 'cowork_management', '-d', source_database, '-c', statement).decode().strip()

    db_name = scope + '-db'
    api_name = scope + '-api'

    def copied_sql(statement):
        return docker('exec', db_name, 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1',
                      '-U', 'control_backup', '-d', 'control_backup', '-c', statement).decode().strip()

    def wait_ready(name, check):
        deadline = time.monotonic() + 100
        while time.monotonic() < deadline:
            status = inspect(name)['State']
            if not status['Running']:
                (output / (name + '.log')).write_bytes(docker('logs', name))
                raise RuntimeError(name + ' exited before readiness')
            result = subprocess.run(['docker', 'exec', name, *check], capture_output=True)
            if result.returncode == 0:
                return
            time.sleep(.4)
        raise RuntimeError(name + ' did not become ready')

    def start_api(reference):
        if api_name not in created:
            created.append(api_name)
        docker('run', '--detach', '--name', api_name, '--label', 'cowork.control-rehearsal=' + scope,
               '--network', scope, '--read-only', '--tmpfs', '/tmp', '--cap-drop', 'ALL',
               '--security-opt', 'no-new-privileges:true',
               *[item for key, file in [('DATABASE_URL_FILE', 'copy_database_url'),
                   ('COWORK_AUTH_MASTER_KEY_FILE', 'auth_master_key'), ('COWORK_MANAGEMENT_AUDIT_KEY_FILE', 'audit_key'),
                   ('COWORK_PROVISIONER_TOKEN_FILE', 'copy_broker'), ('COWORK_BOOTSTRAP_TOKEN_FILE', 'copy_bootstrap')]
                 for item in ['--mount', f'type=bind,src={stage / file},dst=/run/secrets/{file},readonly',
                              '--env', f'{key}=/run/secrets/{file}']],
               '--env', 'COWORK_MODE=management', '--env', 'COWORK_WEB_PUSH_ENABLED=false',
               reference)
        wait_ready(api_name, ['curl', '--fail', '--silent', 'http://127.0.0.1:8080/readyz'])

    def fingerprints(columns, source_version):
        values = {}
        for table, names in columns.items():
            assert re.fullmatch(r'[a-z_][a-z0-9_]*', table)
            assert all(re.fullmatch(r'[a-z_][a-z0-9_]*', name) for name in names)
            projection = ','.join('"' + name + '"' for name in names)
            condition = f' WHERE version <= {source_version}' if table == '_sqlx_migrations' else ''
            values[table] = json.loads(copied_sql(
                f"SELECT json_build_object('rows',count(*),'sha256',encode(sha256(convert_to("
                f"coalesce(string_agg(to_jsonb(t)::text,E'\\n' ORDER BY to_jsonb(t)::text),''),'UTF8')),'hex')) "
                f'FROM (SELECT {projection} FROM "{table}"{condition}) t;'))
        return values

    try:
        source_image = image(args.source_server_image)
        image(args.target_server_image)
        image(args.operator_image)
        assert args.source_server_image != args.target_server_image
        names = ['management', 'management-db', 'provisioner', 'management-web']
        if args.source_migration >= 48: names.append('operator-control')
        original = {name: inspect('cowork-enterprise-local-' + name + '-1') for name in names}
        for name, value in original.items():
            assert value['Config']['Labels']['com.docker.compose.project'] == 'cowork-enterprise-local'
            assert value['Config']['Labels']['com.docker.compose.service'] == name
            expected_running = not args.stopped_control_plane or name == 'management-db'
            assert value['State']['Running'] is expected_running
            assert not value['State']['Paused'] and not value['State']['Restarting']
        assert original['management']['Image'] == source_image['Id']
        source_version = int(source_sql('SELECT max(version) FROM _sqlx_migrations;'))
        assert source_version == args.source_migration, 'source schema must match the explicit supported transition'
        if source_version >= 48:
            assert source_sql('SELECT count(*) FROM management_execution_activation;') == '0', 'active admission needs its own maintenance procedure'
        assert source_sql("SELECT count(*) FROM management_operations WHERE state IN ('queued','running','reconciling');") == '0'
        if args.stopped_control_plane:
            assert source_sql("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() "
                              "AND backend_type='client backend' AND pid<>pg_backend_pid();") == '0', 'source database has another client'
        report.update(source_migration=source_version, source_server_image=args.source_server_image,
                      target_server_image=args.target_server_image, operator_image=args.operator_image,
                      source_database=source_database)
        (stage / 'runtime-inspection.json').write_text(json.dumps(original), encoding='utf-8')
        required = ['management_database_url', 'management_db_password', 'management_bootstrap',
                    'auth_master_key', 'audit_key', 'provisioner_token', 'fleet_database_url',
                    'fleet_db_password', 'fleet_s3_user', 'fleet_s3_password']
        if source_version >= 48:
            required += ['control-plane-images.json', 'installed-control-plane.json', 'control_tls_certificate', 'control_tls_key']
            gateway_mounts = {m['Destination']: Path(m['Source']) for m in original['operator-control']['Mounts']}
            for name in ['control_tls_certificate', 'control_tls_key']:
                assert gateway_mounts['/run/secrets/' + name].resolve() == (private / name).resolve()
        if (private / 'control-plane-features.env').exists(): required.append('control-plane-features.env')
        # Require the actual live API mounts to use the private files being saved.
        mounts = {m['Destination']: Path(m['Source']) for m in original['management']['Mounts']}
        for name in ['management_database_url', 'management_bootstrap', 'auth_master_key', 'audit_key', 'provisioner_token']:
            assert mounts['/run/secrets/' + name].resolve() == (private / name).resolve()
        for name in required:
            source = private / name
            assert source.is_file() and not source.is_symlink()
            shutil.copyfile(source, stage / name)
        (stage / 'management.dump').write_bytes(docker('exec', '--env', 'PGOPTIONS=-c default_transaction_read_only=on',
            'cowork-enterprise-local-management-db-1', 'pg_dump', '-U', 'cowork_management',
            '-d', source_database, '--format=custom', '--no-owner', '--no-acl'))
        operator_mount = next(m for m in original['provisioner']['Mounts'] if m['Destination'] ==
                              '/var/lib/docker/volumes/cowork-enterprise-local-fleet-state/_data')
        assert operator_mount['Type'] == 'volume' and operator_mount['Name'] == 'cowork-enterprise-local-fleet-state'
        volume = json.loads(docker('volume', 'inspect', operator_mount['Name']))[0]
        assert volume['Mountpoint'] == operator_mount['Source']
        # The helper has no network/socket and mounts only the existing, verified
        # volume read-only. It also works while the source daemon is stopped.
        (stage / 'operator-state.tar').write_bytes(docker('run', '--rm', '--network', 'none',
            '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
            '--mount', f'type=volume,src={operator_mount["Name"]},dst=/state,readonly',
            '--entrypoint', 'tar', args.operator_image, '-C', '/state', '-cf', '-', '.'))
        (stage / 'source-compose.yml').write_bytes((repo / 'deploy/enterprise/docker-compose.local.yml').read_bytes())
        archive_files = [p for p in stage.iterdir() if p.is_file()]
        manifest = {'version': 1, 'scope': scope, 'source_migration': source_version,
                    'source_images': {name: value['Image'] for name, value in original.items()},
                    'files': {p.name: checksum(p.read_bytes()) for p in archive_files},
                    'operator_state_quiesced': args.stopped_control_plane,
                    'scope_limit': 'Control metadata/configuration; tenant databases, object store and VM disks are not included.'}
        (stage / 'manifest.json').write_text(json.dumps(manifest), encoding='utf-8')
        with tarfile.open(stage / 'control-plane.tar', 'w') as archive:
            for path in [*archive_files, stage / 'manifest.json']:
                archive.add(path, arcname=path.name, recursive=False)
        identity = private / 'fleet_backup_identity'
        assert identity.is_file() and not identity.is_symlink()
        helper = ['run', '--rm', '--network', 'none', '--read-only', '--tmpfs', '/tmp', '--cap-drop', 'ALL',
                  '--security-opt', 'no-new-privileges:true', '--mount', f'type=bind,src={stage},dst=/fixture',
                  '--mount', f'type=bind,src={identity},dst=/run/secrets/backup_identity,readonly']
        recipient = docker(*helper, '--entrypoint', 'age-keygen', args.operator_image,
                           '-y', '/run/secrets/backup_identity').decode().strip()
        docker(*helper, '--entrypoint', 'age', args.operator_image, '-r', recipient,
               '-o', '/fixture/control-plane.tar.age', '/fixture/control-plane.tar')
        docker(*helper, '--entrypoint', 'age', args.operator_image, '-d', '-i', '/run/secrets/backup_identity',
               '-o', '/fixture/verified.tar', '/fixture/control-plane.tar.age')
        assert checksum((stage / 'control-plane.tar').read_bytes()) == checksum((stage / 'verified.tar').read_bytes())
        shutil.copyfile(stage / 'control-plane.tar.age', output / 'control-plane.tar.age')
        report['backup_sha256'] = checksum((output / 'control-plane.tar.age').read_bytes())
        report['backup_bytes'] = (output / 'control-plane.tar.age').stat().st_size
        report['manifest'] = manifest
        passed('actual_control_backup_encrypted_and_decrypted_identically')
        # Test restoration from the decrypted backup member, not the input dump.
        with tarfile.open(stage / 'verified.tar') as archive:
            restored = archive.extractfile('management.dump').read()
            assert checksum(restored) == manifest['files']['management.dump']
        # Desktop's default bridge pool may already be occupied by retained
        # tenant generations. Select a verified free subnet for this private test.
        network_ids = docker('network', 'ls', '--quiet').decode().splitlines()
        existing = json.loads(docker('network', 'inspect', *network_ids)) if network_ids else []
        occupied = [ipaddress.ip_network(entry['Subnet']) for network in existing
                    for entry in network.get('IPAM', {}).get('Config', []) or [] if entry.get('Subnet')]
        subnet = next((candidate for candidate in ipaddress.ip_network('10.249.0.0/16').subnets(new_prefix=27)
                       if not any(candidate.version == other.version and candidate.overlaps(other) for other in occupied)), None)
        assert subnet is not None, 'no dedicated rehearsal subnet available'
        docker('network', 'create', '--internal', '--subnet', str(subnet),
               '--label', 'cowork.control-rehearsal=' + scope, scope)
        network_created = True
        password = secrets.token_hex(32)
        (stage / 'copy_database_password').write_text(password, encoding='utf-8')
        (stage / 'copy_database_url').write_text(f'postgres://control_backup:{password}@control-db:5432/control_backup', encoding='utf-8')
        for name in ['copy_broker', 'copy_bootstrap']:
            (stage / name).write_text(secrets.token_hex(32), encoding='utf-8')
        created.append(db_name)
        docker('run', '--detach', '--name', db_name, '--label', 'cowork.control-rehearsal=' + scope,
               '--network', scope, '--network-alias', 'control-db', '--tmpfs', '/var/lib/postgresql/data',
               '--mount', f'type=bind,src={stage / "copy_database_password"},dst=/run/secrets/password,readonly',
               '--env', 'POSTGRES_USER=control_backup', '--env', 'POSTGRES_DB=control_backup',
               '--env', 'POSTGRES_PASSWORD_FILE=/run/secrets/password', original['management-db']['Image'])
        wait_ready(db_name, ['pg_isready', '-U', 'control_backup', '-d', 'control_backup'])
        docker('exec', '--interactive', db_name, 'pg_restore', '--exit-on-error', '--no-owner', '--no-acl',
               '-U', 'control_backup', '-d', 'control_backup', data=restored)
        assert int(copied_sql('SELECT max(version) FROM _sqlx_migrations;')) == source_version
        columns = json.loads(copied_sql("SELECT json_object_agg(table_name,names) FROM (SELECT table_name,"
            "json_agg(column_name ORDER BY ordinal_position) names FROM information_schema.columns "
            "WHERE table_schema='public' GROUP BY table_name) c;"))
        before = fingerprints(columns, source_version)
        start_api(args.source_server_image)
        assert fingerprints(columns, source_version) == before
        passed('installed_server_readiness_on_restored_actual_database_without_content_changes')
        docker('stop', api_name)
        docker('rm', api_name)
        start_api(args.target_server_image)
        target_version = int(copied_sql('SELECT max(version) FROM _sqlx_migrations;'))
        assert target_version == args.target_migration
        assert fingerprints(columns, source_version) == before
        assert copied_sql('SELECT count(*) FROM management_execution_activation;') == '0'
        report.update(target_migration=target_version, preserved_source_tables=before)
        passed('candidate_migrates_actual_copy_preserving_every_source_column_and_disabled_activation')
        for name, value in original.items():
            current = inspect('cowork-enterprise-local-' + name + '-1')
            assert current['Id'] == value['Id'] and current['Image'] == value['Image']
            assert current['State']['Running'] == value['State']['Running']
        assert int(source_sql('SELECT max(version) FROM _sqlx_migrations;')) == source_version
        passed('live_control_containers_and_source_schema_unchanged')
        report['completed_at'] = datetime.now(timezone.utc).isoformat()
    finally:
        # Only exact generated names carrying this invocation's label may be removed.
        for name in reversed(created):
            current = subprocess.run(['docker', 'inspect', name], capture_output=True)
            if current.returncode == 0:
                assert json.loads(current.stdout)[0]['Config']['Labels']['cowork.control-rehearsal'] == scope
                docker('rm', '--force', name)
        if network_created:
            net = json.loads(docker('network', 'inspect', scope))[0]
            assert net['Labels']['cowork.control-rehearsal'] == scope
            docker('network', 'rm', scope)
        assert stage.resolve().parent == Path(tempfile.gettempdir()).resolve()
        assert stage.name.startswith(scope + '-')
        for path in stage.iterdir():
            assert path.is_file() and not path.is_symlink() and path.parent == stage
            path.unlink()
        stage.rmdir()
        report['rehearsal_resources_removed'] = True
        report['limitations'] = [
            ('Control API/operator/web were already stopped; this command leaves them stopped.' if args.stopped_control_plane else
             'Point-in-time control-plane backup while source services remain running; not a quiesced cutover.'),
            'Source Compose text may differ from the running release; actual container inspections are archived separately.',
            'Operator state is included; tenant databases, objects, workspaces and VM disks are outside this backup.',
            'No login/MFA recovery, full fresh-host recovery, live migration or tenant enrollment was performed.',
        ]
        (output / 'results.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print('Control-plane rehearsal passed: ' + str(output / 'results.json'), flush=True)


if __name__ == '__main__':
    main()
