"""Explicit permanent deletion of one Compose instance, including generations.

The private frozen inventory survives a crash. It is resumed only after the
management service dispatches an explicit recovery request under the operator
lock. No command, resource name or filesystem path comes from a client.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import uuid

from instance_backups import compact, pg_environment


class DeletionError(Exception):
    pass


def check(condition, code):
    if not condition:
        raise DeletionError(code)


def read_json(path):
    check(not path.is_symlink() and path.is_file() and path.stat().st_size <= 16 * 1024 * 1024, 'invalid_deletion_evidence')
    try:
        return json.loads(path.read_text())
    except (ValueError, OSError) as error:
        raise DeletionError('invalid_deletion_evidence') from error


def safe_tree(path, parent):
    check(not path.is_symlink() and path.resolve().is_relative_to(parent.resolve()) and path.resolve() != parent.resolve(), 'deletion_path_escape')
    if path.is_dir():
        for child in path.rglob('*'):
            check(not child.is_symlink() and (child.is_dir() or child.is_file()), 'unsafe_deletion_tree')
            if child.is_file():
                check(child.stat().st_nlink == 1, 'unsafe_deletion_hardlink')
    elif path.exists():
        check(path.is_file() and path.stat().st_nlink == 1, 'unsafe_deletion_file')


class InstanceDeletion:
    def __init__(self, infrastructure):
        self.infra = infrastructure
        self.config = infrastructure.config
        self.run = infrastructure.run
        self.write = infrastructure.private_write
        self.root = Path(self.config.get('backup_directory', ''))
        check(self.root.is_absolute() and not self.root.is_symlink() and self.root.resolve() != Path(self.root.anchor), 'deletion_backup_directory_not_configured')
        state = infrastructure.root.resolve()
        check(not self.root.resolve().is_relative_to(state) and not state.is_relative_to(self.root.resolve()), 'overlapping_deletion_roots')
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.alias = self.config['s3_admin_alias']
        check(re.fullmatch('[a-zA-Z][a-zA-Z0-9_-]*', self.alias), 'invalid_deletion_storage_alias')
        self.db_env = pg_environment(Path(self.config['database_admin_url_file']).read_text().strip(), self.config)

    def storage(self, *args):
        import guest_endpoints
        try:
            environment = guest_endpoints.controller_storage_environment(self.config)
        except guest_endpoints.GuestEndpointError as error:
            raise DeletionError(str(error)) from None
        return self.run(['mc', '--config-dir', self.config['s3_admin_config_directory'], '--json', *args], timeout=21600,
                        **({'env': environment} if environment is not None else {}))

    def rows(self, *args):
        rows = [json.loads(line) for line in self.storage(*args).splitlines() if line.strip()]
        check(all(row.get('status') == 'success' for row in rows), 'deletion_storage_inventory_failed')
        return rows

    def sql(self, query):
        return self.run(['psql', '-X', '-v', 'ON_ERROR_STOP=1', '-tAc', query], env=self.db_env)

    def docker(self, kind, scope):
        if kind == 'container':
            names = self.run(['docker', 'ps', '--all', '--filter', 'name=^/' + scope, '--format', '{{.Names}}']).decode().splitlines()
            names = [name for name in names if name.startswith(scope + '-')]
        else:
            names = self.run(['docker', kind, 'ls', '--format', '{{.Name}}']).decode().splitlines()
            names = [name for name in names if name.startswith(scope + '-') or name.startswith(scope + '_')]
        return json.loads(self.run(['docker', kind, 'inspect', *names])) if names else []

    def generations(self, instance, directory):
        result = []
        paths = [directory]
        parent = directory / 'generations'
        if parent.exists():
            check(not parent.is_symlink() and parent.is_dir(), 'unsafe_deletion_generations')
            for path in sorted(parent.iterdir()):
                check(re.fullmatch('[a-f0-9]{32}', path.name) and path.is_dir(), 'unknown_deletion_generation')
                marker = read_json(path / 'generation.json')
                check(compact(marker['generation']) == path.name, 'deletion_generation_scope_mismatch')
                paths.append(path)
        for path in paths:
            resources = self.infra.resource_names(instance, path)
            secret = path / 'secrets' / 's3_access_key'
            user = secret.read_text().strip() if secret.exists() else None
            check(user is None or re.fullmatch('[0-9a-f]{64}', user), 'invalid_deletion_storage_identity')
            result.append({**resources, 's3_user': user})
        return result

    def backups(self, operation, directory):
        instance = operation['instance']
        ids = set(operation['arguments'].get('delete_backup_ids', []))
        for value in ids:
            compact(value)
        # Includes a crash between archive publication and metadata publication.
        for path in directory.glob('operation-*.json'):
            state = read_json(path)
            binding = state['binding']
            check(binding['instance_id'] == instance['id'] and binding['organisation_id'] == instance['organisation_id'], 'deletion_journal_scope_mismatch')
            check(path.name == 'operation-' + compact(binding['id']) + '.json', 'deletion_journal_id_mismatch')
            if binding['action'] in ['backup', 'upgrade', 'rollback', 'enroll_execution']:
                ids.add(binding['id'])
        for path in self.root.glob('*.json'):
            metadata = read_json(path)
            if metadata.get('instance_id') != instance['id']:
                continue
            check(metadata.get('organisation_id') == instance['organisation_id'], 'deletion_backup_scope_mismatch')
            backup = metadata['backup_id']
            check(path.name == compact(backup) + '.json', 'deletion_backup_id_mismatch')
            ids.add(backup)
        for value in ids:
            metadata = self.root / (compact(value) + '.json')
            if metadata.exists():
                data = read_json(metadata)
                check(data.get('instance_id') == instance['id'] and data.get('organisation_id') == instance['organisation_id'], 'deletion_backup_scope_mismatch')
            for suffix in ['.json', '.age']:
                safe_tree(self.root / (compact(value) + suffix), self.root)
        staging = []
        for path in self.root.iterdir():
            if not path.is_dir() or not path.name.startswith(('capture-', 'restore-')):
                continue
            safe_tree(path, self.root)
            marker = path / 'owner.json'
            legacy = path / 'payload' / 'instance.json'
            check(marker.exists() or legacy.exists(), 'unattributed_backup_staging_requires_review')
            owner = read_json(marker if marker.exists() else legacy)
            owner_instance = owner.get('instance_id', owner.get('id'))
            if owner_instance == instance['id']:
                check(owner.get('organisation_id') == instance['organisation_id'], 'deletion_staging_scope_mismatch')
                staging.append(path.name)
        return sorted(ids), sorted(staging)

    def inventory(self, operation, directory):
        instance = operation['instance']
        scope = 'cowork-' + compact(instance['id'])
        safe_tree(directory, self.infra.root)
        resources = self.generations(instance, directory)
        volumes = self.docker('volume', scope)
        allowed_volumes = {scope + '-runner-admission'}
        for volume in volumes:
            name = volume['Name']
            check(name in allowed_volumes or any(re.fullmatch(re.escape(r['volume_prefix']) + '[a-f0-9]{32}', name) for r in resources), 'unknown_deletion_volume')
            check((volume.get('Labels') or {}).get('com.docker.compose.project', scope) == scope, 'foreign_deletion_volume')
        containers = self.docker('container', scope)
        for container in containers:
            name = container['Name'].lstrip('/')
            labels = container.get('Config', {}).get('Labels') or {}
            check(labels.get('com.docker.compose.project', scope) == scope, 'foreign_deletion_container')
            services='api|worker|runner|egress-proxy|postgres|minio' if self.config.get('instance_disk_quotas') is not None else 'api|worker|runner|egress-proxy'
            check(re.fullmatch(re.escape(scope) + r'-(?:(?:'+services+r')-[1-9][0-9]*|(?:job|desktop|terminal|transfer)-[a-f0-9]{32}|(?:g-[a-f0-9]{32}-)?snapshot-[a-f0-9]{32})', name), 'unknown_deletion_container')
        networks = self.docker('network', scope)
        for network in networks:
            check(network['Name'] in [scope + '_control', scope + '_internet', scope + '-sandbox']+([scope+'-data'] if self.config.get('instance_disk_quotas') is not None else []), 'unknown_deletion_network')
            check((network.get('Labels') or {}).get('com.docker.compose.project') == scope, 'foreign_deletion_network')
        manifest = directory / 'compose.json'
        if manifest.exists():
            value = read_json(manifest)
            env = value['services']['api']['environment']
            check(value['name'] == scope and env['COWORK_INSTANCE_ID'] == instance['id'] and env['COWORK_ORGANISATION_ID'] == instance['organisation_id'], 'deletion_manifest_scope_mismatch')
        else:
            check(not containers, 'deletion_manifest_missing')
        users = {row['accessKey']: row for row in self.rows('admin', 'user', 'list', self.alias)}
        for resource in resources:
            user = resource['s3_user']
            if user in users:
                check(users[user].get('policyName', '') in ['', resource['bucket']], 'foreign_deletion_storage_policy')
            db = resource['database']
            check(re.fullmatch('ci_[a-f0-9]{32}', db), 'invalid_deletion_database')
            owner = self.sql(f"SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='{db}';").decode().strip()
            check(owner in ['', db], 'foreign_deletion_database_owner')
        backup_ids, staging = self.backups(operation, directory)
        return {'version': 1, 'operation_id': operation['id'], 'instance_id': instance['id'], 'organisation_id': instance['organisation_id'],
                'resources': resources, 'volumes': sorted(v['Name'] for v in volumes), 'backup_ids': backup_ids, 'staging': staging,
                'database_endpoint': self.config['database_endpoint'], 's3_endpoint': self.config['s3_endpoint'],
                'backup_root': str(self.root.resolve()), 'scope': scope}

    def validate_plan(self, plan, operation):
        instance = operation['instance']
        check(plan['version'] == 1 and all(plan[key] == value for key, value in [('operation_id', operation['id']), ('instance_id', instance['id']), ('organisation_id', instance['organisation_id']), ('scope', 'cowork-' + compact(instance['id'])), ('database_endpoint', self.config['database_endpoint']), ('s3_endpoint', self.config['s3_endpoint']), ('backup_root', str(self.root.resolve()))]), 'deletion_plan_scope_changed')
        check(plan['resources'] and len({r['generation'] for r in plan['resources']}) == len(plan['resources']), 'invalid_deletion_generations')
        for r in plan['resources']:
            generation = r['generation']
            storage = hashlib.sha256((instance['id'] + ':' + generation).encode()).hexdigest()[:32] if generation else compact(instance['id'])
            prefix = plan['scope'] + ('-g-' + compact(generation) if generation else '') + '-run-'
            check(r['database'] == 'ci_' + storage and r['bucket'] == 'cowork-' + storage and r['volume_prefix'] == prefix, 'deletion_plan_resource_changed')
            check(r['s3_user'] is None or re.fullmatch('[a-f0-9]{64}', r['s3_user']), 'invalid_deletion_plan_user')
        for name in plan['volumes']:
            check(name == plan['scope'] + '-runner-admission' or any(re.fullmatch(re.escape(r['volume_prefix']) + '[a-f0-9]{32}', name) for r in plan['resources']), 'invalid_deletion_plan_volume')
        for name in plan['staging']:
            check(re.fullmatch(r'(?:capture|restore)-[a-zA-Z0-9_-]+', name), 'invalid_deletion_plan_staging')
        for value in plan['backup_ids']:
            compact(value)
        check(set(operation['arguments'].get('delete_backup_ids', [])) <= set(plan['backup_ids']), 'deletion_plan_backup_mismatch')

    def execute(self, operation, directory, *, resume=False):
        instance = operation['instance']
        check(self.config.get('deletion_enabled') is True, 'instance_deletion_disabled')
        import backend_profile
        check(instance['backend'] == 'compose' and (instance['policy']['minimum_isolation'] == 'container' or backend_profile.vm_instance(self.infra.config,instance)), 'deletion_backend_not_qualified')
        check(operation['arguments'].get('confirmation_name') == instance['name'] and operation['arguments'].get('purge_backups') is True, 'permanent_deletion_acknowledgement_required')
        path = directory / ('deletion-' + compact(operation['id']) + '.json')
        if path.exists():
            check(resume, 'deletion_requires_explicit_recovery')
            plan = read_json(path)
        else:
            # Validate ownership before stopping anything. Then stop producers and
            # rescan to include the last workload they could have acknowledged.
            self.inventory(operation, directory)
            if (directory / 'compose.json').exists():
                self.infra.stop_compose(instance, directory)
            plan = self.inventory(operation, directory)
            self.write(path, json.dumps(plan, sort_keys=True).encode())
        self.validate_plan(plan, operation)
        safe_tree(directory, self.infra.root)
        # Revalidate mutable backup evidence before destructive work, including
        # after a crash. A frozen filename alone is not proof of current ownership.
        for backup in plan['backup_ids']:
            metadata = self.root / (compact(backup) + '.json')
            if metadata.exists():
                owner = read_json(metadata)
                check(owner.get('instance_id') == instance['id'] and owner.get('organisation_id') == instance['organisation_id'], 'deletion_backup_scope_mismatch')
        for name in plan['staging']:
            target = self.root / name
            safe_tree(target, self.root)
            if target.exists():
                marker = target / 'owner.json'
                owner = read_json(marker if marker.exists() else target / 'payload' / 'instance.json')
                check(owner.get('instance_id', owner.get('id')) == instance['id'] and owner.get('organisation_id') == instance['organisation_id'], 'deletion_staging_scope_mismatch')
        plan_digest = hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest()
        scope = plan['scope']
        # Revalidate container ownership on every explicitly authorised recovery.
        containers = self.docker('container', scope)
        dedicated=self.config.get('instance_disk_quotas') is not None
        data_ids=set()
        data_proof=None
        data_proof_path=directory/('deletion-'+uuid.uuid5(uuid.UUID(operation['id']),'data-physical-stop').hex+'.json')
        if dedicated:
            import instance_disk_quotas
            data_ids=instance_disk_quotas.data_service_ids(self.infra)
            if data_proof_path.exists():
                data_proof=read_json(data_proof_path)
                check(data_proof['format']=='cowork-original-data-service-retirement-v1' and data_proof['operation_id']==operation['id']
                    and data_proof['instance_id']==instance['id'] and data_proof['plan_sha256']==plan_digest
                    and set(data_proof['container_ids'])==data_ids and data_proof['original_objects_removed'] is True,
                    'original data-service retirement proof changed')
                stops=data_proof['physical_stops']
                check(len(stops)==2 and {v['Id'] for v in stops}==data_ids and all(v['State']['Running'] is False
                    and v['State']['Paused'] is False and v['State']['Restarting'] is False and v['State']['Pid']==0
                    and v['Config']['Labels'].get('cowork.disk.instance')==instance['id'] for v in stops),
                    'positive original data-service stop evidence required')
            else:
                check({v['Id'] for v in instance_disk_quotas.verified_data_services(self.infra)}==data_ids,'original data services changed')
        for container in containers:
            labels = container.get('Config', {}).get('Labels') or {}
            check(labels.get('com.docker.compose.project') == scope, 'unproven_deletion_container')
            check(re.fullmatch(re.escape(scope) + (r'-(?:api|worker|runner|egress-proxy|postgres|minio)-[1-9][0-9]*' if dedicated else r'-(?:api|worker|runner|egress-proxy)-[1-9][0-9]*'), container['Name'].lstrip('/')), 'unexpected_deletion_container_after_stop')
        if containers:
            business=[c['Id'] for c in containers if c['Id'] not in data_ids]
            if business:self.run(['docker', 'rm', '--force', *business])
        if data_proof is None:
            users = {row['accessKey']: row for row in self.rows('admin', 'user', 'list', self.alias)}
            for r in plan['resources']:
                db, user = r['database'], r['s3_user']
                owner = self.sql(f"SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='{db}';").decode().strip()
                check(owner in ['', db], 'foreign_deletion_database_owner')
                if self.sql(f"SELECT 1 FROM pg_roles WHERE rolname='{db}';").strip() == b'1':
                    self.sql(f'ALTER ROLE {db} NOLOGIN;')
                if owner:
                    self.sql(f'ALTER DATABASE {db} ALLOW_CONNECTIONS false;')
                    self.sql(f'DROP DATABASE {db} WITH (FORCE);')
                self.sql(f'DROP ROLE IF EXISTS {db};')
                if user in users:
                    check(users[user].get('policyName', '') in ['', r['bucket']], 'foreign_deletion_storage_policy')
                    self.storage('admin', 'user', 'disable', self.alias, user)
                    self.storage('admin', 'user', 'remove', self.alias, user)
            buckets = {row['key'].rstrip('/') for row in self.rows('ls', self.alias)}
            policies = {row['policy'] for row in self.rows('admin', 'policy', 'list', self.alias)}
            for r in plan['resources']:
                if r['bucket'] in buckets:
                    self.storage('rb', '--force', self.alias + '/' + r['bucket'])
                if r['bucket'] in policies:
                    self.storage('admin', 'policy', 'remove', self.alias, r['bucket'])
            # The dedicated data services remain alive until their real DB/S3
            # readbacks prove the original objects gone. No absence inference.
            if dedicated:
                remaining_users={row['accessKey'] for row in self.rows('admin','user','list',self.alias)}
                remaining_buckets={row['key'].rstrip('/') for row in self.rows('ls',self.alias)}
                remaining_policies={row['policy'] for row in self.rows('admin','policy','list',self.alias)}
                for r in plan['resources']:
                    check(r['s3_user'] not in remaining_users and r['bucket'] not in remaining_buckets and r['bucket'] not in remaining_policies,'deletion_storage_still_present')
                    check(not self.sql(f"SELECT 1 FROM pg_database WHERE datname='{r['database']}' UNION ALL SELECT 1 FROM pg_roles WHERE rolname='{r['database']}';").strip(),'deletion_database_still_present')
                self.run(['docker','stop','--time','30',*sorted(data_ids)])
                actual=json.loads(self.run(['docker','inspect',*sorted(data_ids)]))
                check({v['Id'] for v in actual}==data_ids and all(not v['State']['Running'] and not v['State']['Paused'] and not v['State']['Restarting'] and v['State']['Pid']==0 for v in actual),'original data services remain active')
                data_proof={'format':'cowork-original-data-service-retirement-v1','operation_id':operation['id'],
                    'instance_id':instance['id'],'plan_sha256':plan_digest,'container_ids':sorted(data_ids),
                    'original_objects_removed':True,'physical_stops':actual}
                self.write(data_proof_path,json.dumps(data_proof,sort_keys=True).encode())
                self.run(['docker','rm',*sorted(data_ids)])
        else:
            # Readback-only recovery after the positive original stop was
            # persisted before rm. Missing containers alone are never proof.
            remaining=[v for v in self.docker('container',scope) if v['Id'] in data_ids]
            if remaining:
                check(all(v['State']['Running'] is False and v['State']['Paused'] is False and v['State']['Restarting'] is False
                    and v['State']['Pid']==0 for v in remaining),'original stopped data service resumed; retirement held')
                self.run(['docker','rm',*[v['Id'] for v in remaining]])
        current_volumes = self.docker('volume', scope)
        check(all(v['Name'] in plan['volumes'] and (v.get('Labels') or {}).get('com.docker.compose.project', scope) == scope for v in current_volumes), 'deletion_volume_inventory_changed')
        if current_volumes:
            self.run(['docker', 'volume', 'rm', *[v['Name'] for v in current_volumes]])
        for network in self.docker('network', scope):
            check(network['Name'] in [scope + '_control', scope + '_internet', scope + '-sandbox']+([scope+'-data'] if dedicated else []) and (network.get('Labels') or {}).get('com.docker.compose.project') == scope, 'foreign_deletion_network')
            self.run(['docker', 'network', 'rm', network['Id']])
        for backup in plan['backup_ids']:
            for suffix in ['.age', '.json']:
                target = self.root / (compact(backup) + suffix)
                safe_tree(target, self.root)
                target.unlink(missing_ok=True)
        for name in plan['staging']:
            target = self.root / name
            safe_tree(target, self.root)
            if target.exists():
                shutil.rmtree(target)
        # Retain only metadata receipts and journals. All generations, database
        # credentials, storage keys, runtime mounts and network plans disappear.
        for target in directory.iterdir():
            if re.fullmatch(r'(?:operation|receipt|deletion)-[a-f0-9]{32}\.json', target.name) or target.name in ['deleted.json', 'executor-host.json']:
                continue
            safe_tree(target, directory)
            if target.is_dir():
                shutil.rmtree(target)
            else:
                target.unlink()
        for kind in ['container', 'volume', 'network']:
            check(not self.docker(kind, scope), 'deletion_resources_still_present')
        if not dedicated:
            remaining_users = {row['accessKey'] for row in self.rows('admin', 'user', 'list', self.alias)}
            remaining_buckets = {row['key'].rstrip('/') for row in self.rows('ls', self.alias)}
            remaining_policies = {row['policy'] for row in self.rows('admin', 'policy', 'list', self.alias)}
            for r in plan['resources']:
                check(r['s3_user'] not in remaining_users and r['bucket'] not in remaining_buckets and r['bucket'] not in remaining_policies, 'deletion_storage_still_present')
                check(not self.sql(f"SELECT 1 FROM pg_database WHERE datname='{r['database']}' UNION ALL SELECT 1 FROM pg_roles WHERE rolname='{r['database']}';").strip(), 'deletion_database_still_present')
        receipt = {'receipt_id': operation['id'], 'instance_id': instance['id'], 'deletion_verified': True,
                   'deletion_inventory_sha256': plan_digest, 'deleted_backup_count': len(plan['backup_ids'])}
        self.write(directory / 'deleted.json', json.dumps(receipt, sort_keys=True).encode())
        return receipt
