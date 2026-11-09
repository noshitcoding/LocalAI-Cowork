"""Operator-only, observational preflight for legacy Compose enrollment.

This is not an enrollment receipt or an execution capability. It deliberately
does not update management, write an epoch, start/stop containers, or settle work.
An enrollment operation must repeat these checks under its own durable binding.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import tempfile
import uuid

import execution_generation as epochs
from execution_runtime import probe_image
from instance_backups import pg_environment, verify_backup as verify_backup_artifact
from instance_deletion import InstanceDeletion, safe_tree


class EnrollmentPreflightError(Exception):
    pass


def require(condition, code):
    if not condition:
        raise EnrollmentPreflightError(code)


def canonical(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


# Counts only. No prompts, account details, paths, tokens or run payloads leave
# an instance database. Unknown/unsupported schemas fail rather than look empty.
WORK_SQL = """SELECT json_build_object(
 'unfinished_runs',(SELECT count(*) FROM runs WHERE state NOT IN ('completed','failed','canceled','expired')),
 'preservable_server_runs',(SELECT count(*) FROM runs
    WHERE target_kind='server_linux' AND target_pool_id IS NULL AND target_device_id IS NULL
      AND lease_owner IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL
      AND (state='interrupted' OR (state='waiting_for_snapshot' AND assigned_executor_id IS NULL AND started_at IS NULL))),
 'external_assignment_history',(SELECT count(*) FROM runs WHERE assigned_executor_id IS NOT NULL
    AND (target_kind<>'server_linux' OR target_pool_id IS NOT NULL OR target_device_id IS NOT NULL)),
 'legacy_assignments_requiring_review',(SELECT count(*) FROM runs WHERE assigned_executor_id IS NOT NULL),
 'executor_active_runs',(SELECT coalesce(sum(active_runs),0) FROM executors),
 'registered_executors',(SELECT count(*) FROM executors),
 'terminal_sessions',(SELECT count(*) FROM terminal_sessions WHERE state NOT IN ('ended','failed')),
 'desktop_sessions',(SELECT count(*) FROM desktop_sessions WHERE state NOT IN ('ended','failed')),
 'enabled_schedules',(SELECT count(*) FROM schedules WHERE enabled)
);"""
COUNT_KEYS = {'unfinished_runs', 'preservable_server_runs', 'external_assignment_history', 'legacy_assignments_requiring_review', 'executor_active_runs', 'registered_executors',
              'terminal_sessions', 'desktop_sessions', 'enabled_schedules'}


def sql(infra, env, query):
    # The database enforces read-only transactions, even when the operator's
    # administrative identity is needed to inspect retained generations.
    readonly = {**env, 'PGOPTIONS': '-c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000'}
    return infra.run(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'], env=readonly,
                     data=('BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;\n' + query + '\nCOMMIT;').encode(), timeout=15).decode().strip()


def counts(infra, env):
    value = json.loads(sql(infra, env, WORK_SQL))
    require(isinstance(value, dict) and set(value) == COUNT_KEYS
            and all(type(v) is int and v >= 0 for v in value.values()), 'invalid_enrollment_work_inventory')
    require(value['preservable_server_runs']<=value['unfinished_runs'], 'invalid_enrollment_work_inventory')
    tables = json.loads(sql(infra, env, "SELECT json_build_object('reservations',to_regclass('public.instance_execution_reservations') IS NOT NULL);"))
    require(isinstance(tables, dict) and set(tables) == {'reservations'}
            and type(tables['reservations']) is bool, 'invalid_enrollment_schema_inventory')
    value['unsettled_reservations'] = 0
    if tables['reservations']:
        # An unacknowledged completion is still reserved by central management.
        value['unsettled_reservations'] = int(sql(infra, env,
            "SELECT count(*) FROM instance_execution_reservations WHERE acknowledged_at IS NULL;"))
        require(value['unsettled_reservations'] >= 0, 'invalid_enrollment_work_inventory')
    return value


def verify_backup(infra, instance, backup_id, images, scratch):
    root = Path(infra.config['backup_directory'])
    stem = uuid.UUID(backup_id).hex
    metadata = epochs.document(root / (stem + '.json'))
    require(metadata.get('backup_id') == backup_id and metadata.get('instance_id') == instance['id']
            and metadata.get('organisation_id') == instance['organisation_id']
            and type(metadata.get('manifest_version')) is int and metadata['manifest_version'] == 1,
            'enrollment_backup_scope_mismatch')
    archive = root / (stem + '.age')
    safe_tree(archive, root)
    catalog = {'id': backup_id, 'instance_id': instance['id'], 'organisation_id': instance['organisation_id'],
               'size_bytes': metadata.get('backup_bytes'), 'sha256': metadata.get('backup_sha256')}
    verified = verify_backup_artifact(archive, catalog, instance, backup_id, images,
                                     infra.config['backup_identity_file'], infra.run, scratch)
    return {'backup_id': backup_id, 'sha256': verified.cipher_sha256, 'bytes': verified.cipher_bytes,
            'manifest_sha256': verified.manifest_sha256}


def inspect(infra, instance, backup_id, *, current_operation=None):
    require(infra.config.get('executor_host') is None, 'remote_executor_lifecycle_not_qualified')
    """Caller holds the existing operator lock; report has no durable authority."""
    epochs.identifier(instance['id']); epochs.identifier(instance['organisation_id']); epochs.identifier(backup_id)
    import backend_profile
    require(instance.get('backend') == 'compose' and (instance.get('policy', {}).get('minimum_isolation') == 'container' or backend_profile.vm_instance(infra.config,instance)),
            'enrollment_backend_not_qualified')
    directory = infra.root / uuid.UUID(instance['id']).hex
    require(directory.is_dir() and not directory.is_symlink(), 'enrollment_instance_not_found')
    backup_root = Path(infra.config.get('backup_directory', ''))
    require(backup_root.is_absolute() and backup_root.is_dir() and not backup_root.is_symlink(),
            'enrollment_backup_directory_not_found')
    safe_tree(directory, infra.root)
    active = infra.active_directory(directory)
    require(epochs.read(instance, active) is None, 'instance_already_has_execution_epoch')
    manifest = epochs.document(directory / 'compose.json', 1048576)
    for name in ['api', 'worker']:
        env = manifest['services'][name]['environment']
        require(env.get('COWORK_INSTANCE_ID') == instance['id'] and env.get('COWORK_ORGANISATION_ID') == instance['organisation_id'],
                'enrollment_manifest_scope_mismatch')
        require(env.get('COWORK_FLEET_EXECUTION_ADMISSION') not in ['true', True]
                and not env.get('COWORK_EXECUTION_EPOCH'), 'enrollment_manifest_epoch_mismatch')
    images = infra.deployed_images(directory)
    require(manifest['services']['worker'].get('image') == images['server'], 'enrollment_worker_image_mismatch')
    if (active / 'images.json').exists():
        require(epochs.document(active / 'images.json') == images, 'enrollment_pinned_images_mismatch')
    # This explicit isolated declaration probe never starts the application.
    probe_image(infra, images['server'])
    for journal_path in directory.glob('operation-*.json'):
        journal = epochs.document(journal_path, 1048576)
        binding = journal.get('binding', {})
        require(binding.get('instance_id') == instance['id'] and binding.get('organisation_id') == instance['organisation_id'],
                'enrollment_journal_scope_mismatch')
        if current_operation is not None and binding.get('id')==current_operation['id']:
            from operation_journal import binding as journal_binding
            require(binding==journal_binding(current_operation), 'enrollment_journal_scope_mismatch')
        else:
            require(isinstance(journal.get('completion'), dict), 'enrollment_operation_unsettled')
    # The deletion inventory is shared ownership-validation code; inventory()
    # never removes resources. Do not call its execute() method here.
    inventory = InstanceDeletion(infra)
    observation = {'id': str(uuid.uuid4()), 'action': 'inspect_execution_enrollment', 'instance': instance, 'arguments': {}}
    before = inventory.inventory(observation, directory)
    require(not before['staging'], 'enrollment_backup_staging_unsettled')
    admin_env = pg_environment(Path(infra.config['database_admin_url_file']).read_text().strip())
    blockers = []
    totals = {key: 0 for key in [*sorted(COUNT_KEYS), 'unsettled_reservations']}
    observations = []
    for resource in before['resources']:
        database = resource['database']
        require(re.fullmatch(r'ci_[a-f0-9]{32}', database), 'invalid_enrollment_database')
        connections = int(sql(infra, admin_env, "SELECT count(*) FROM pg_stat_activity WHERE datname='" + database + "';"))
        if connections:
            blockers.append('database_connections_present')
        work = counts(infra, {**admin_env, 'PGDATABASE': database})
        for key, value in work.items():
            totals[key] += value
        observations.append({'generation': resource['generation'], 'connections': connections, 'work': work})
    containers = inventory.docker('container', before['scope'])
    def stopped(entries):
        return all(all(c.get('State', {}).get(k) is False for k in ['Running', 'Paused', 'Restarting']) for c in entries)
    if not stopped(containers):
        blockers.append('containers_active')
    for key, value in totals.items():
        if value:
            blockers.append(key)
    # Encrypted bytes and every archive member are checked. Scratch is private
    # and destroyed on failure/success; no plaintext is written beside backups.
    with tempfile.TemporaryDirectory(prefix='cowork-enrollment-check-') as temporary:
        backup = verify_backup(infra, instance, backup_id, images, Path(temporary))
    after = inventory.inventory(observation, directory)
    require(before == after and epochs.document(directory / 'compose.json', 1048576) == manifest
            and infra.active_directory(directory) == active and epochs.read(instance, active) is None,
            'enrollment_inventory_changed')
    final_containers = inventory.docker('container', before['scope'])
    if sorted(c['Id'] for c in containers) != sorted(c['Id'] for c in final_containers) or not stopped(final_containers):
        blockers.append('containers_changed_during_check')
    # A report is an observation, never a reusable pass permitting a later start.
    return {'version': 1, 'purpose': 'execution_enrollment_preflight', 'observed_at': datetime.now(timezone.utc).isoformat(),
            'instance_id': instance['id'], 'organisation_id': instance['organisation_id'],
            'eligible_for_enrollment': False, 'checks_passed': not blockers, 'blockers': sorted(set(blockers)),
            'server_image': images['server'], 'verified_generations': len(observations),
            'retained_containers': len(final_containers), 'work': totals, 'backup': backup,
            'observation_sha256': canonical({'inventory': before, 'manifest': manifest, 'observations': observations})}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True)
    parser.add_argument('--instance-id', required=True)
    parser.add_argument('--organisation-id', required=True)
    parser.add_argument('--backup-id', required=True)
    args = parser.parse_args()
    # Canonical identifiers only; paths and commands are operator-configured.
    import provisioner
    from operation_journal import operator_lock
    try:
        for value in [args.instance_id, args.organisation_id, args.backup_id]: epochs.identifier(value)
        config = json.loads(Path(args.config).read_text(encoding='utf-8'))
        require(Path(config['state_directory']).is_dir(), 'enrollment_state_directory_not_found')
        infra = provisioner.Infrastructure(config)
        instance = {'id': args.instance_id, 'organisation_id': args.organisation_id, 'backend': 'compose',
                    'policy': {'minimum_isolation': 'container'}}
        with operator_lock(infra.root):
            report = inspect(infra, instance, args.backup_id)
        print(json.dumps(report, sort_keys=True))
        return 0 if report['checks_passed'] else 2
    except Exception:
        # CLI output must not accidentally echo database URLs, decrypted backup
        # content, secrets, command stderr or host paths on a failed probe.
        print(json.dumps({'version': 1, 'purpose': 'execution_enrollment_preflight',
                          'checks_passed': False, 'eligible_for_enrollment': False,
                          'error_code': 'enrollment_preflight_unproven'}))
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
