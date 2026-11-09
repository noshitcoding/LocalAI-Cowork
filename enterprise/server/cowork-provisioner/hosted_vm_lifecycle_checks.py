"""Private FullLifecycle context; callbacks are fixed by the UUID VM harness.

No Docker, VM, account or endpoint discovery occurs here. Authentication state
and workspace snapshots stay private in memory and are never placed in reports.
"""
import hashlib
import json
from pathlib import Path
import re
import time
from urllib.parse import unquote, urlsplit
import uuid

from lifecycle_schema_delta import LifecycleSchemaDelta


def require(value, code):
    if not value:
        raise RuntimeError(code)


class Context:
    def __init__(self, **bindings):
        self.__dict__.update(bindings)
        self.infra = self.infrastructure
        self.config = self.infra.config
        self.origin = self.api_origin
        self.directory = self.infra.directory(self.current()['id'])
        self.process_ids = set()
        self.report['health_snapshots'] = []
        self.schema_delta = LifecycleSchemaDelta(self.source_schema, self.target_schema)

    def active(self):
        return self.infra.active_directory(self.directory)

    def request(self, origin, path, token=None, body=None, expected=200, method=None, raw=False):
        require(origin in [self.origin, self.management_origin], 'foreign_lifecycle_endpoint_refused')
        opener = self.api_opener if origin == self.origin else self.management_opener
        headers = {'content-type': 'application/octet-stream'} if isinstance(body, bytes) else None
        return self.__dict__['request_callback'](opener, origin, path, token, body,
            method=method, expected=expected, headers=headers, raw=raw)

    def api(self, path, body=None, expected=200, method=None, raw=False):
        return self.request(self.origin, '/api/v1' + path, self.token, body, expected, method, raw)

    def login(self):
        ticket = self.request(self.management_origin,
            self.management_base + '/instances/' + self.current()['id'] + '/launch', self.management_token, {})
        self.session_tokens = self.request(self.origin, '/api/v1/auth/fleet', body={
            'ticket': ticket['ticket'], 'device_id': str(uuid.uuid4())})
        self.token = self.session_tokens['access_token']
        return self.token

    def sql(self, statement):
        if 'jsonb_agg(to_jsonb(r) ORDER BY id)' in statement:
            return self.schema_delta.historical_runs(self._sql, statement, self.schema())
        return self._sql(statement)

    def _sql(self, statement):
        value = urlsplit((self.active() / 'secrets/database_url').read_text().strip())
        require(re.fullmatch('ci_[a-f0-9]{32}', value.path.lstrip('/'))
                and unquote(value.username) == value.path.lstrip('/'), 'lifecycle_database_scope_mismatch')
        return self.sql_callback(self.pg_fleet_port, statement, database=value.path.lstrip('/'),
            user=unquote(value.username), password=unquote(value.password))

    def schema(self):
        return int(self._sql('SELECT max(version) FROM _sqlx_migrations;'))

    def assert_epoch_fence(self, receipt, user_id):
        execution = receipt['result']['execution']
        source, epoch = execution['binding']['source_epoch'], execution['active_epoch']
        require(source != epoch and self.current()['execution_epoch'] == epoch,
                'guest_transition_epoch_not_committed')
        marker = json.loads((self.active() / 'execution.json').read_text())
        manifest = json.loads((self.directory / 'compose.json').read_text())
        require(marker['epoch'] == epoch and all(manifest['services'][name]['environment']['COWORK_EXECUTION_EPOCH'] == epoch
                for name in ['api', 'worker']), 'guest_transition_runtime_epoch_mismatch')
        key = (self.active() / 'secrets/fleet_key').read_text().strip()
        require(re.fullmatch('[a-f0-9]{64}', key) is not None, 'guest_transition_instance_key_invalid')
        self.request_callback(self.management_opener, self.runtime_management_origin,
            '/management/runner/v1/execution-reserve', body={'organisation_id': self.current()['organisation_id'],
                'instance_id': self.current()['id'], 'execution_epoch': source, 'run_id': str(uuid.uuid4()),
                'user_id': user_id, 'target': {'kind': 'server_linux', 'pool_id': None}}, expected=403,
            headers={'x-cowork-instance-key': key})
        self.report.setdefault('epoch_fences', []).append({'operation_id': receipt['id'], 'source_epoch': source,
            'active_epoch': epoch, 'stale_reservation_refused': True, 'runtime_epoch_matches': True})

    def set_chat_enabled(self, enabled):
        require(type(enabled) is bool, 'invalid_private_chat_flag')
        if self.current()['state'] == 'ready': self.lifecycle('stop')
        self.chat_enabled['value'] = enabled
        self.lifecycle('start'); self.login()
        manifest = json.loads((self.directory / 'compose.json').read_text())
        require(all(manifest['services'][name]['environment']['COWORK_CHAT_WORKSPACES_ENABLED'] == str(enabled).lower()
                    for name in ['api', 'worker']), 'chat_flag_generation_binding_mismatch')

    def run_shell(self, thread_id, script):
        self.infra.capacity_attestor.refresh()
        require(self.infra.capacity_attestor.qualified, 'host55_capacity_not_qualified_before_actual_run')
        request = {'project_id': self.project['id'], 'thread_id': thread_id,
            'project_revision': self.project['revision'], 'project_privacy': 'team_managed',
            'executor_target': {'kind': 'server_linux', 'pool_id': None}, 'required_capabilities': ['shell'],
            'idempotency_key': str(uuid.uuid4()), 'input': {'prompt': 'Private VM lifecycle proof', 'sandbox': {
                'schema_version': 2, 'run_id': str(uuid.uuid4()), 'image': 'core', 'network': 'none',
                'argv': ['python3', '-c', script], 'limits': {'cpu_nanos': 100000000, 'memory_bytes': 128 * 1024**2,
                    'pids': 64, 'timeout_seconds': 20, 'tmpfs_bytes': 4 * 1024**2, 'output_bytes': 4096}}}}
        run = self.api('/threads/' + thread_id + '/messages', {'content': {'text': 'Private lifecycle conversation'},
            'run': request}, expected=201)['run']
        records = []
        def finished():
            records[:] = [self.api('/runs/' + run['spec']['id'])]
            return records[0]['state'] in ['completed', 'failed', 'canceled', 'expired']
        self.wait(finished, 'guest_lifecycle_run_unfinished', 120)
        require(records[0]['state'] == 'completed', 'guest_lifecycle_run_failed')
        return records[0]

    def read_workspace(self, volume_name, relative_path):
        paths = [self.directory, *list((self.directory / 'generations').glob('*'))]
        prefixes = [self.infra.resource_names(self.current(), path)['volume_prefix'] for path in paths
                    if path.is_dir() and (path / 'secrets').is_dir()]
        require(any(re.fullmatch(re.escape(prefix) + '[a-f0-9]{32}', volume_name) for prefix in prefixes)
                and re.fullmatch('[a-zA-Z0-9._/-]+', relative_path)
                and '..' not in relative_path.split('/'), 'foreign_workspace_read_refused')
        self.infra.run(['docker', 'volume', 'inspect', volume_name])
        name = 'cowork-' + uuid.UUID(self.current()['id']).hex + '-snapshot-' + uuid.uuid4().hex
        return self.infra.run(['docker', 'run', '--rm', '--pull', 'never', '--name', name, '--network', 'none',
            '--read-only', '--user', '10001:10001', '--cpus', '.1', '--memory', '64m', '--memory-swap', '64m',
            '--pids-limit', '16', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
            '--mount', 'type=volume,src=' + volume_name + ',dst=/workspace,readonly',
            self.config['images']['sandbox_core'], 'cat', '/workspace/' + relative_path])

    def observe_health(self, phase, *, worker=True):
        expected_schema = self.target_schema if self.infra.deployed_images(self.directory)['server'] == self.target_images['server'] else self.source_schema
        samples = []
        def healthy():
            rows = json.loads(self._sql("SELECT coalesce(jsonb_agg(jsonb_build_object('process_id',process_id,'mode',process_mode,'migration',migration_version,'observations',observations,'now_ms',(extract(epoch FROM now())*1000)::bigint)),'[]'::jsonb) FROM server_process_health WHERE lifecycle='running' AND last_seen_at>now()-interval '20 seconds';"))
            if len(rows) != (2 if worker else 1) or {row['mode'] for row in rows} != ({'api', 'worker'} if worker else {'api'}): return False
            for row in rows:
                if row['migration'] != expected_schema or row['process_id'] in self.process_ids: return False
                if row['mode'] == 'worker' and any(not 0 <= row['now_ms'] - row['observations'].get(name, 0) <= 20000
                    for name in ['scheduler_tick_ms', 'maintenance_tick_ms', 'dispatch_tick_ms']): return False
            samples[:] = rows; return True
        self.wait(healthy, 'fresh_guest_process_schema_and_loop_ticks_missing', 40)
        self.process_ids.update(row['process_id'] for row in samples)
        self.report['health_snapshots'].append({'phase': phase, 'generation': self.infra.resource_names(self.current(), self.active())['generation'], 'processes': samples})


def qualify_lifecycle(ctx):
    from candidate_lifecycle_workspace import (prepare_workspace_checks, assert_restored_workspace_checks,
        assert_current_workspace_checks, arm_schedule_for_upgrade_checks, assert_candidate_schedule_paused,
        capture_activation_checks, assert_activation_unchanged)
    import provisioner as p
    from release_catalog import ReleaseCatalog, image_set_id
    from instance_updates import InstanceUpdates
    require(ctx.schema() == ctx.source_schema, 'guest_source_schema_mismatch')
    ctx.observe_health('source')
    state = prepare_workspace_checks(ctx)
    ctx.lifecycle('stop'); backup = ctx.lifecycle('backup')
    entry = next(row for row in ctx.request(ctx.management_origin,
        ctx.management_base + '/instances/' + ctx.current()['id'] + '/backups', ctx.management_token)
        if row['id'] == backup['result']['backup_id'])
    require(entry['size_bytes'] > 0 and re.fullmatch('[a-f0-9]{64}', entry['sha256']), 'encrypted_backup_catalog_invalid')
    ctx.check('real_guest_stopped_encrypted_backup_catalog')
    before = capture_activation_checks(ctx, state)
    archive = Path(ctx.config['backup_directory']) / (uuid.UUID(entry['id']).hex + '.age')
    original = archive.read_bytes()
    try:
        p.private_write(archive, original + b'own-tamper-' + uuid.uuid4().hex.encode())
        rejected = ctx.lifecycle('restore', expected='failed', backup_id=entry['id'])
        require(rejected['result']['error_code'] == 'backup_catalog_integrity_failed'
                and ctx.current()['state'] == 'stopped', 'tamper_restore_did_not_fail_closed')
        assert_activation_unchanged(ctx, state, before)
    finally:
        p.private_write(archive, original)
    ctx.check('guest_tamper_rejected_before_source_or_activation_mutation')
    ctx.lifecycle('start'); ctx.login()
    post_backup_marker = 'after-backup-' + uuid.uuid4().hex
    changed = ctx.run_shell(state['chat_thread'],
        "from pathlib import Path;Path('lifecycle-marker.txt').write_text(" + repr(post_backup_marker) + ");print('post-backup-change')")
    project = ctx.api('/projects/' + ctx.project['id'])
    ctx.project = ctx.api('/projects/' + project['id'], {'expected_revision': project['revision'],
        'name': 'Changed only after private VM backup'}, method='PUT')
    extra = ctx.api('/projects', {'name': 'Created after private VM backup', 'privacy': 'private_local'}, expected=201)
    ctx.lifecycle('stop')
    restored = ctx.lifecycle('restore', backup_id=entry['id'])
    ctx.lifecycle('start'); ctx.login()
    ctx.project = ctx.api('/projects/' + state['project_ids'][0])
    require(ctx.sql("SELECT count(*) FROM runs WHERE id='" + changed['spec']['id'] + "';") == '0',
            'post_backup_run_survived_guest_restore')
    ctx.api('/projects/' + extra['id'], expected=404)
    source_volume = state['resources']['volume_prefix'] + uuid.UUID(state['workspace_id']).hex
    require(ctx.read_workspace(source_volume, 'lifecycle-marker.txt') == post_backup_marker.encode(),
            'restore_mutated_retained_source_workspace')
    assert_restored_workspace_checks(ctx, state, restored)
    user_id = ctx.api('/runs/' + state['completed_run_ids'][0])['spec']['creator_user_id']
    ctx.assert_epoch_fence(restored, user_id)
    ctx.check('guest_restore_recovers_snapshot_and_keeps_later_source_content_immutable')
    ctx.observe_health('restored_source')
    require(ctx.schema() == ctx.source_schema, 'restore_changed_source_schema')
    arm_schedule_for_upgrade_checks(ctx, state); ctx.lifecycle('stop')
    source_images = ctx.infra.deployed_images(ctx.directory)
    catalog = ReleaseCatalog(ctx.config, p.private_write)
    catalog.publish('Private VM source', source_images, [image_set_id(ctx.target_images)])
    target = catalog.publish('Private VM cross-binary target', ctx.target_images, [image_set_id(source_images)])
    p.sync_release_catalog(ctx.manager_client, ctx.infra)
    original_probe, original_services = InstanceUpdates.probe, InstanceUpdates._probe_services
    def services(engine, *values, **keywords):
        original_services(engine, *values, **keywords)
        ctx.observe_health('private_candidate_or_source_probe', worker=keywords['worker'])
    def probe(engine, *values, **keywords):
        original_probe(engine, *values, **keywords)
        if keywords.get('worker') is True: assert_candidate_schedule_paused(ctx, state)
    InstanceUpdates._probe_services, InstanceUpdates.probe = services, probe
    source_active = ctx.active()
    try:
        canary = ctx.lifecycle('upgrade', release=target['id'], qualification_only=True)
        require(canary['result']['rollback_verified'] is True and ctx.active() == source_active
                and ctx.infra.deployed_images(ctx.directory) == source_images, 'canary_source_pointer_or_images_changed')
        ctx.lifecycle('start'); ctx.login(); assert_current_workspace_checks(ctx, state); ctx.lifecycle('stop')
        promoted = ctx.lifecycle('upgrade', release=target['id'], qualification_only=False)
        require(ctx.active() != source_active and ctx.infra.deployed_images(ctx.directory) == ctx.target_images,
                'guest_promotion_did_not_activate_exact_target')
        ctx.lifecycle('start'); ctx.login(); assert_current_workspace_checks(ctx, state)
        ctx.assert_epoch_fence(promoted, user_id)
        require(ctx.schema() == ctx.target_schema, 'guest_target_schema_mismatch')
        ctx.observe_health('promoted_target')
        target_run = ctx.run_shell(state['chat_thread'], state['follow_script'])
        require(target_run['result']['sandbox']['workspace_volume'] ==
                ctx.infra.resource_names(ctx.current(), ctx.active())['volume_prefix'] + uuid.UUID(state['workspace_id']).hex,
                'promoted_target_run_used_wrong_workspace')
        ctx.check('real_guest_cross_binary_promotion_and_target_admitted_chat_run')
        ctx.lifecycle('stop'); InstanceUpdates.probe = original_probe
        rolled_back = ctx.lifecycle('rollback', rollback_operation_id=promoted['id'])
        ctx.lifecycle('start'); ctx.login(); state['schedule_paused'] = True
        assert_current_workspace_checks(ctx, state)
        require(ctx.schema() == ctx.source_schema and ctx.infra.deployed_images(ctx.directory) == source_images,
                'guest_rollback_not_exact_source_schema_and_images')
        require(ctx.sql("SELECT count(*) FROM runs WHERE id='" + target_run['spec']['id'] + "';") == '0',
                'post_promotion_run_survived_source_backup_rollback')
        ctx.observe_health('rollback_source')
        ctx.assert_epoch_fence(rolled_back, user_id)
        ctx.check('real_guest_rollback_source_fences_bindings_and_files')
    finally:
        InstanceUpdates.probe, InstanceUpdates._probe_services = original_probe, original_services
    ctx.report.update({'cross_version_qualified': True, 'source_schema_version': ctx.source_schema,
        'target_schema_version': ctx.target_schema, 'same_binary_repackage': False,
        'workspace_helper_sha256': hashlib.sha256(Path(__file__).with_name('candidate_lifecycle_workspace.py').read_bytes()).hexdigest(),
        'lifecycle_helper_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'schema_delta_evidence': ctx.schema_delta.evidence(),
        'schema_delta_helper_sha256': hashlib.sha256(Path(__file__).with_name('lifecycle_schema_delta.py').read_bytes()).hexdigest(),
        'not_qualified': ['remote execution enrollment', 'live fleet migration', 'disk quotas', 'automatic certificate renewal']})
