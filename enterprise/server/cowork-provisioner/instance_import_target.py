"""Actual import target readback. Engine adapters are real by default; tests own their fixtures."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import uuid

import execution_generation as epochs
import executor_host
from instance_backups import BackupError, _workspace_inventory, pg_environment
from operation_journal import operator_pass_fds


def canonical(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def require(value, code):
    if not value:
        raise BackupError(code)


def private_bytes(path, limit=1048576):
    path = Path(path)
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_size <= limit
            and all(not parent.is_symlink() for parent in path.parents), 'import_target_unsafe_file')
    flags = os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0)
    with os.fdopen(os.open(path, flags), 'rb') as source:
        opened = os.fstat(source.fileno())
        require((opened.st_dev, opened.st_ino, opened.st_size) == (before.st_dev, before.st_ino, before.st_size),
                'import_target_file_changed')
        result = source.read(limit + 1)
        after = os.fstat(source.fileno())
    def identity(value):
        return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns
    require(len(result) == before.st_size and len(result) <= limit and identity(before) == identity(after) == identity(path.lstat()),
            'import_target_file_changed')
    return result


def original_provenance(verified):
    original = json.loads(private_bytes(verified.payload / 'instance.json'))
    value = verified.manifest.get('import_provenance')
    require(isinstance(value, dict) and set(value) == {'version', 'origin_epoch', 'run_ids'}
            and type(value['version']) is int and value['version'] == 1,
            'import_original_provenance_required')
    try:
        origin = epochs.identifier(original.get('execution_epoch'))
        require(value['origin_epoch'] == origin, 'import_original_epoch_mismatch')
        run_ids = value['run_ids']
        require(isinstance(run_ids, list) and len(run_ids) <= 100000 and run_ids == sorted(set(run_ids)),
                'import_original_run_inventory_invalid')
        for run in run_ids:
            epochs.identifier(run)
    except (epochs.ExecutionGenerationError, TypeError):
        raise BackupError('import_original_epoch_or_inventory_unsupported') from None
    return {'version': 1, 'origin_epoch': origin, 'run_ids': list(run_ids)}


def json_values(raw):
    require(isinstance(raw, bytes) and len(raw) <= 32 * 1024 * 1024, 'import_target_inventory_exceeds_bound')
    decoder, offset, values = json.JSONDecoder(), 0, []
    source = raw.decode('utf-8')
    while offset < len(source):
        if source[offset].isspace():
            offset += 1
            continue
        value, offset = decoder.raw_decode(source, offset)
        values.append(value)
        require(len(values) <= 100000, 'import_target_inventory_exceeds_bound')
    return values


def object_listing(manager, bucket):
    result = {}
    # mc's JSON output may be adjacent formatted JSON or JSON Lines.
    for entry in json_values(manager.storage('ls', '--recursive', '--json', bucket + '/')):
        require(isinstance(entry, dict) and entry.get('status') == 'success' and entry.get('type') == 'file',
                'import_target_object_metadata_unsupported')
        key, size = entry.get('key'), entry.get('size')
        require(isinstance(key, str) and key and '\\' not in key and len(key.encode()) <= 2048,
                'import_target_object_key_unsafe')
        path = PurePosixPath(key)
        require(not path.is_absolute() and str(path) == key and '..' not in path.parts and key not in result
                and type(size) is int and size >= 0 and not entry.get('isDeleteMarker'),
                'import_target_object_key_unsafe')
        result[key] = size
    return result


def read_object(manager, name, destination, bound):
    """Bounded actual controller-side mc stream; no object bytes are logged."""
    import guest_endpoints
    from remote_volume_streaming import pump
    require(os.name == 'posix', 'import_target_readback_requires_posix')
    environment = guest_endpoints.controller_storage_environment(manager.config)
    arguments = ['mc', '--config-dir', manager.config['s3_admin_config_directory'], 'cat', name]
    descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    process = None
    try:
        with os.fdopen(descriptor, 'wb') as output:
            process = subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, env=environment, pass_fds=operator_pass_fds())
            evidence = pump(process, output, 'capture', {'timeout_seconds': 300, 'max_archive_bytes': bound})
            output.flush()
            os.fsync(output.fileno())
        return evidence
    finally:
        if process is not None:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
            process.stdout.close()


def target_objects(manager, resources, verified, scratch):
    expected = {name.removeprefix('objects/'): value for name, value in verified.manifest['files'].items()
                if name.startswith('objects/')}
    bucket = manager.config['s3_admin_alias'] + '/' + resources['bucket']
    listed = object_listing(manager, bucket)
    require(listed == {key: value['bytes'] for key, value in expected.items()}, 'import_target_object_inventory_mismatch')
    actual = {}
    for key, value in sorted(expected.items()):
        destination = scratch / (uuid.uuid4().hex + '.object')
        evidence = manager.read_object(bucket + '/' + key, destination, value['bytes'])
        require(evidence == value, 'import_target_object_content_mismatch')
        actual[key] = evidence
    require(object_listing(manager, bucket) == listed, 'import_target_object_inventory_changed')
    return actual


def target_workspaces(manager, operation, generation, resources, verified, scratch):
    expected = set(verified.manifest['workspace_runs'])
    names = manager.run(['docker', 'volume', 'ls', '--filter', 'name=^' + resources['volume_prefix'],
                         '--format', '{{.Name}}']).decode().splitlines()
    require(len(names) == len(set(names)) and set(names) == {resources['volume_prefix'] + name for name in expected},
            'import_target_workspace_inventory_mismatch')
    result = {}
    for run_id in sorted(expected):
        destination = scratch / (run_id + '.tar')
        manager.capture_import_volume(operation, generation, run_id, destination)
        inventory = _workspace_inventory(destination)
        require(inventory == verified.workspace_inventory[run_id], 'import_target_workspace_content_mismatch')
        result[run_id] = inventory
    after = manager.run(['docker', 'volume', 'ls', '--filter', 'name=^' + resources['volume_prefix'],
                         '--format', '{{.Name}}']).decode().splitlines()
    require(len(after) == len(names) and set(after) == set(names), 'import_target_workspace_inventory_changed')
    return result


def target_history(manager, operation, generation, provenance):
    import instance_imports
    value = instance_imports.binding(operation)
    environment = pg_environment(private_bytes(generation / 'secrets/database_url').decode().strip(), manager.config)
    query = """SELECT json_build_object(
      'runs',(SELECT COALESCE(json_agg(json_build_object('id',id,'state',state,'lease_owner',lease_owner,'lease_token',lease_token,'lease_expires_at',lease_expires_at) ORDER BY id),'[]'::json) FROM runs),
      'origins',(SELECT COALESCE(json_agg(json_build_object('run_id',run_id,'import_id',import_id,'organisation_id',organisation_id,'source_instance_id',source_instance_id,'target_instance_id',target_instance_id,'source_backup_id',source_backup_id,'source_manifest_sha256',source_manifest_sha256) ORDER BY run_id),'[]'::json) FROM run_instance_import_origins),
      'active_schedules',(SELECT count(*) FROM schedules WHERE enabled OR next_run_at IS NOT NULL),
      'active_sessions',(SELECT count(*) FROM auth_sessions WHERE revoked_at IS NULL),
      'active_credentials',(SELECT count(*) FROM executor_credentials WHERE revoked_at IS NULL));"""
    data = json.loads(manager.run(['psql', '-X', '-tAc', query], env=environment))
    require(isinstance(data, dict) and set(data) == {'runs', 'origins', 'active_schedules', 'active_sessions', 'active_credentials'}
            and all(type(data[k]) is int and data[k] == 0 for k in ['active_schedules', 'active_sessions', 'active_credentials']),
            'import_target_fences_unverified')
    runs, origins = data['runs'], data['origins']
    require(isinstance(runs, list) and isinstance(origins, list) and len(runs) == len(origins) == len(provenance['run_ids']),
            'import_target_history_inventory_mismatch')
    require([row['id'] for row in runs] == provenance['run_ids'] and [row['run_id'] for row in origins] == provenance['run_ids'],
            'import_target_history_inventory_mismatch')
    for row in runs:
        require(set(row) == {'id', 'state', 'lease_owner', 'lease_token', 'lease_expires_at'}
                and row['state'] in ['completed', 'failed', 'canceled', 'expired', 'interrupted']
                and all(row[k] is None for k in ['lease_owner', 'lease_token', 'lease_expires_at']),
                'import_target_historical_run_active')
    expected = {'import_id': value['import_id'], 'organisation_id': value['organisation_id'],
                'source_instance_id': value['source']['id'], 'target_instance_id': value['target']['id'],
                'source_backup_id': value['backup']['id'], 'source_manifest_sha256': operation['arguments']['import_export']['manifest_sha256']}
    require(all({key: row.get(key) for key in expected} == expected and set(row) == {'run_id', *expected} for row in origins),
            'import_target_origin_provenance_unsupported')
    # Check the installed guard on every imported row. The savepoint contains a
    # real forbidden activation attempt, and always rolls back its tentative data.
    guard = b"""BEGIN;
DO $$ DECLARE historical UUID; blocked BOOLEAN; BEGIN
 FOR historical IN SELECT run_id FROM run_instance_import_origins LOOP
  blocked:=false;
  BEGIN
   UPDATE runs SET state='queued',lease_owner=gen_random_uuid(),lease_token=gen_random_uuid(),lease_expires_at=clock_timestamp()+interval '1 minute' WHERE id=historical;
  EXCEPTION WHEN raise_exception THEN blocked:=SQLERRM='imported historical run cannot acquire execution authority'; END;
  IF NOT blocked THEN RAISE EXCEPTION 'import activation fence was not proved'; END IF;
 END LOOP;
END $$;
ROLLBACK;"""
    manager.run(['psql', '-X', '-v', 'ON_ERROR_STOP=1'], env=environment, data=guard)
    return data


def candidate_compose(manager, operation, directory, generation):
    import provisioner
    import resource_budget
    caps = (resource_budget.read(directory / 'resource-budget.json')['services']
            if (directory / 'resource-budget.json').exists() else resource_budget.service_caps(manager.config))
    return provisioner.compose_manifest({**manager.config, 'images': json.loads(private_bytes(generation / 'images.json')),
        'compose_subnets': manager.infra.network_plan(operation['instance'], directory),
        'instance_service_limits': caps}, operation['instance'], generation)


def runtime_scope(manager, operation, directory, generation, *, before_activation, prepare_candidate=True):
    import instance_imports
    import execution_runtime
    value = instance_imports.binding(operation)
    target = operation['instance']
    require(directory == manager.infra.root / uuid.UUID(target['id']).hex
            and generation.parent == directory / 'generations' and not generation.is_symlink(), 'import_target_generation_scope_mismatch')
    epochs.identifier(str(uuid.UUID(generation.name)))
    require(uuid.UUID(generation.name).hex == generation.name, 'import_target_generation_scope_mismatch')
    require(epochs.read(target, generation) == value['candidate_epoch'], 'import_target_epoch_mismatch')
    marker = epochs.document(generation / 'generation.json')
    require(marker.get('generation') == str(uuid.UUID(generation.name)), 'import_target_generation_scope_mismatch')
    if before_activation:
        require(operation['action'] == 'import_restore' and marker['generation'] == operation['id'],
                'import_target_generation_scope_mismatch')
        if prepare_candidate:
            manifest = candidate_compose(manager, operation, directory, generation)
            manager.write(generation / 'import-compose.json', json.dumps(manifest).encode())
        else:
            manifest = epochs.document(generation / 'import-compose.json', 1048576)
    else:
        require(manager.infra.active_directory(directory) == generation, 'import_target_generation_changed')
        manifest = epochs.document(directory / 'compose.json', 1048576)
        require(manifest == epochs.document(generation / 'import-compose.json', 1048576), 'import_target_compose_changed')
    execution_runtime.validate_manifest(manager.config, manifest)
    resources = manager.infra.resource_names(target, generation)
    secret_names = ['bootstrap_token', 'database_url', 'storage_master_key', 's3_access_key', 's3_secret_key', 'runner_signing_key', 'fleet_key']
    for service in ['api', 'worker']:
        environment = manifest['services'][service]['environment']
        require(all(environment.get(key) == expected for key, expected in {
            'COWORK_INSTANCE_ID': target['id'], 'COWORK_ORGANISATION_ID': value['organisation_id'],
            'COWORK_EXECUTION_EPOCH': value['candidate_epoch'], 'COWORK_S3_BUCKET': resources['bucket']}.items()),
            'import_target_compose_scope_mismatch')
        require(set(secret_names).issubset(manifest['services'][service]['secrets']), 'import_target_secret_mount_mismatch')
    runner = manifest['services']['runner']['environment']
    require(runner.get('COWORK_RUNNER_INSTANCE_ID') == target['id'] and runner.get('COWORK_RUNNER_STORAGE_GENERATION') == marker['generation'],
            'import_target_runner_scope_mismatch')
    for secret in secret_names:
        require(manifest['secrets'].get(secret) == {'file': str(generation / 'runtime-secrets' / secret)}
                and private_bytes(generation / 'runtime-secrets' / secret) == private_bytes(generation / 'secrets' / secret).strip(),
                'import_target_secret_mount_mismatch')
    require(executor_host.operation_binding(operation) == executor_host.manager_binding(manager.config) == value['target_host_binding'],
            'import_target_host_binding_mismatch')
    manager.infra.host.verify()
    return resources, canonical(manifest)


def credential_checks(manager, directory, generation, restore, resources, verified):
    previous_id = restore.get('previous_generation')
    previous = directory if previous_id is None else directory / 'generations' / uuid.UUID(epochs.identifier(previous_id)).hex
    require(previous != generation and previous.is_dir() and not previous.is_symlink(), 'import_target_previous_generation_unverified')
    old, new = previous / 'secrets', generation / 'secrets'
    for name in ['bootstrap_token', 'runner_signing_key', 'database_password', 's3_access_key', 's3_secret_key', 'fleet_key']:
        require(len(private_bytes(new / name).strip()) >= 32, 'import_target_credential_metadata_unsupported')
    for name in ['bootstrap_token', 'runner_signing_key', *(['fleet_key'] if 'secrets/fleet_key' in verified.manifest['files'] else [])]:
        require(private_bytes(new / name).strip() != private_bytes(verified.payload / 'secrets' / name).strip(),
                'import_target_source_credential_reused')
    require(private_bytes(new / 'storage_master_key') == private_bytes(verified.payload / 'secrets/storage_master_key'),
            'import_target_data_key_changed')
    for name in ['bootstrap_token', 'runner_signing_key', 'database_password', 's3_access_key', 's3_secret_key', 'fleet_key']:
        # Older generations mounted the fleet key directly from runtime-secrets
        # without retaining a second copy in secrets.
        old_path = (previous / 'runtime-secrets' / name
                    if name == 'fleet_key' and not (old / name).exists() else old / name)
        require(private_bytes(new / name).strip() != private_bytes(old_path).strip(), 'import_target_previous_credential_reused')
    database = pg_environment(private_bytes(new / 'database_url').decode().strip(), manager.config)
    require(database['PGDATABASE'] == resources['database'] and database['PGUSER'] == resources['database']
            and database['PGPASSWORD'].encode() == private_bytes(new / 'database_password').strip(), 'import_target_database_scope_mismatch')


def verify_target(manager, operation, directory, generation, verified, scratch, *, before_activation=False, job=None):
    import instance_imports
    value = instance_imports.binding(operation)
    provenance = original_provenance(verified)
    instance_imports.verify_export(operation, verified.manifest)
    restore = epochs.document(generation / 'restore.json')
    require(restore.get('import_binding_sha256') == canonical(value)
            and restore.get('original_manifest_sha256') == verified.manifest_sha256
            and restore.get('origin_epoch') == provenance['origin_epoch'] and restore.get('backup_id') == value['backup']['id'],
            'import_target_restore_binding_mismatch')
    resources, compose_hash = runtime_scope(manager, operation, directory, generation, before_activation=before_activation)
    credential_checks(manager, directory, generation, restore, resources, verified)
    manager.quiescent(operation['instance'], generation)
    history = target_history(manager, operation, generation, provenance)
    objects = target_objects(manager, resources, verified, scratch)
    workspaces = target_workspaces(manager, operation, generation, resources, verified, scratch)
    final_resources, final_compose_hash = runtime_scope(manager, operation, directory, generation,
        before_activation=before_activation, prepare_candidate=False)
    require(final_resources == resources and final_compose_hash == compose_hash
            and epochs.document(generation / 'restore.json') == restore, 'import_target_metadata_changed')
    credential_checks(manager, directory, generation, restore, resources, verified)
    require(epochs.read(operation['instance'], generation) == value['candidate_epoch']
            and (before_activation or manager.infra.active_directory(directory) == generation), 'import_target_generation_changed')
    # Object and volume readback can take minutes. Recheck execution fences and
    # the exact historical rows after those reads, including the real forbidden
    # activation probe, before accepting the target inventory.
    manager.quiescent(operation['instance'], generation)
    require(target_history(manager, operation, generation, provenance) == history, 'import_target_history_changed')
    inventory = {'version': 1, 'import_binding_sha256': canonical(value), 'original_manifest_sha256': verified.manifest_sha256,
        'origin_epoch': provenance['origin_epoch'], 'target_epoch': value['candidate_epoch'], 'resources': resources,
        'compose_sha256': compose_hash, 'history': history, 'objects': objects, 'workspaces': workspaces,
        'credential_checks': {'source_bootstrap_runner_changed': True, 'data_key_preserved': True,
                              'source_fleet_changed': 'secrets/fleet_key' in verified.manifest['files'],
                              'previous_target_bootstrap_runner_database_s3_fleet_changed': True}}
    return {'instance_id': operation['instance']['id'], 'execution_epoch': value['candidate_epoch'],
        'manifest_sha256': verified.manifest_sha256, 'inventory_sha256': canonical(inventory), 'credentials_rotated': True,
        'schedules_paused': True, 'unfinished_runs_interrupted': True, 'historical_resume_blocked': True,
        'workspace_inventory_complete': True, 'bounded_job_sha256': job}
