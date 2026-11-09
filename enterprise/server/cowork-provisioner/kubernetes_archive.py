"""Real PostgreSQL/S3/PVC archives for the bound, quiescent namespace.

The existing immutable managed catalog and age integrity verifier remain the
provenance boundary. Kubernetes snapshots are not claimed from names or TTLs.
"""
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import uuid

from instance_backups import (BackupError, RESTORE_FENCE_SQL, compact, digest,
                             pg_environment, verify_backup, _workspace_inventory)
import execution_generation as epochs


class Archives:
    def __init__(self, lifecycle):
        self.kube = lifecycle
        self.infra = lifecycle.infra
        self.config = self.infra.config
        self.root = Path(self.config['backup_directory'])
        if not self.root.is_absolute() or self.root.is_symlink() or self.root.resolve() == Path(self.root.anchor):
            raise BackupError('invalid_kubernetes_backup_directory')
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.identity = self.config['backup_identity_file']
        self.recipient = self.infra.run(['age-keygen', '-y', self.identity]).decode().strip()
        if not self.recipient.startswith('age1'):
            raise BackupError('invalid_kubernetes_backup_identity')
        path = self.root/'recipient.txt'
        if path.exists():
            if path.is_symlink() or path.read_text().strip() != self.recipient:
                raise BackupError('backup_recovery_identity_changed')
        elif any(self.root.glob('*.age')):
            raise BackupError('backup_recipient_missing_requires_reconciliation')
        else:
            self.infra.private_write(path, (self.recipient+'\n').encode())

    def storage(self, *args):
        import guest_endpoints
        environment = guest_endpoints.controller_storage_environment(self.config)
        return self.infra.run(['mc', '--config-dir', self.config['s3_admin_config_directory'], *args],
                              timeout=21600, **({'env': environment} if environment is not None else {}))

    def quiescent(self, active):
        self.kube.quiescent()
        resources = self.infra.resource_names(self.kube.instance, active)
        environment = pg_environment(Path(self.config['database_admin_url_file']).read_text().strip(), self.config)
        count = self.infra.run(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-c',
                              "SELECT count(*) FROM pg_stat_activity WHERE datname='"+resources['database']+"';"], env=environment)
        if count.strip() != b'0':
            raise BackupError('kubernetes_database_not_quiescent')

    def backup(self, operation, directory, active):
        self.quiescent(active)
        identifier = operation['id']
        archive = self.root/(compact(identifier)+'.age')
        metadata = archive.with_suffix('.json')
        if archive.exists() or metadata.exists():
            if not archive.is_file() or not metadata.is_file():
                raise BackupError('original_backup_publication_unknown')
            prior = epochs.document(metadata)
            if prior['backup_id'] != identifier or prior['instance_id'] != self.kube.instance['id']:
                raise BackupError('original_backup_scope_changed')
            with tempfile.TemporaryDirectory(dir=self.root, prefix='verify-kube-') as scratch:
                verified = verify_backup(archive, {'id':identifier,'instance_id':self.kube.instance['id'],
                    'organisation_id':self.kube.instance['organisation_id'],'sha256':prior['backup_sha256'],
                    'size_bytes':prior['backup_bytes']}, self.kube.instance, identifier,
                    self.kube.images(active), self.identity, self.infra.run, Path(scratch))
            return {key:prior[key] for key in ('backup_id','backup_sha256','backup_bytes','manifest_version')}
        resources = self.infra.resource_names(self.kube.instance, active)
        images = self.kube.images(active)
        with tempfile.TemporaryDirectory(dir=self.root, prefix='capture-kube-') as scratch:
            temp = Path(scratch); payload = temp/'payload';payload.mkdir(mode=0o700)
            for name in ('objects','workspaces','secrets'):(payload/name).mkdir(mode=0o700)
            for secret in ('bootstrap_token','runner_signing_key','storage_master_key'):
                self.infra.private_write(payload/'secrets'/secret, (active/'secrets'/secret).read_bytes())
            self.infra.private_write(payload/'instance.json', json.dumps(self.kube.instance,sort_keys=True).encode())
            env = pg_environment((active/'secrets/database_url').read_text().strip(),self.config)
            self.infra.run(['pg_dump','--format=custom','--no-owner','--no-privileges','--file',str(payload/'database.dump')],env=env,timeout=21600)
            self.storage('mirror',self.config['s3_admin_alias']+'/'+resources['bucket'],str(payload/'objects'))
            inventory = self.kube.workspace_inventory(active)
            workspaces=[];total=0
            for original in inventory:
                workspace=compact(original['workspace_id']);target=payload/'workspaces'/(workspace+'.tar')
                self.kube.workspace_archive(original,target,restore=False)
                total+=target.stat().st_size
                if total>self.kube.settings['max_archive_bytes']:
                    raise BackupError('kubernetes_workspace_archive_total_exceeds_bound')
                _workspace_inventory(target) # actual archive contents, no links/special files
                workspaces.append(workspace)
            self.quiescent(active)
            if self.kube.workspace_inventory(active)!=inventory:
                raise BackupError('original_workspace_inventory_changed_during_capture')
            files={}
            for path in sorted(payload.rglob('*')):
                if path.is_symlink():raise BackupError('unsafe_kubernetes_backup_file')
                if path.is_file():files[path.relative_to(payload).as_posix()]={'bytes':path.stat().st_size,'sha256':digest(path)}
            manifest={'schema_version':1,'backup_id':identifier,'organisation_id':self.kube.instance['organisation_id'],
                'instance_id':self.kube.instance['id'],'created_at':datetime.now(timezone.utc).isoformat(),
                'images':images,'workspace_runs':workspaces,'files':files,
                'kubernetes_original_inventory':inventory,'backend_profile':operation['backend_profile']}
            if self.kube.instance.get('execution_epoch'):
                run_ids=json.loads(self.infra.run(['psql','-X','-qAt','-c',"SELECT COALESCE(json_agg(id ORDER BY id),'[]'::json) FROM runs;"],env=env))
                if len(run_ids)>100000 or run_ids!=sorted(set(run_ids)):raise BackupError('backup_original_run_inventory_invalid')
                for value in run_ids:epochs.identifier(value)
                manifest['import_provenance']={'version':1,'origin_epoch':self.kube.instance['execution_epoch'],'run_ids':run_ids}
            self.infra.private_write(payload/'manifest.json',json.dumps(manifest,sort_keys=True).encode())
            plain=temp/'backup.tar'
            with tarfile.open(plain,'w') as out:
                out.add(payload/'manifest.json',arcname='manifest.json',recursive=False)
                for name in files:out.add(payload/name,arcname=name,recursive=False)
            cipher=temp/'backup.age'
            self.infra.run(['age','--encrypt','-r',self.recipient,'-o',str(cipher),str(plain)],timeout=21600)
            os.chmod(cipher,0o600)
            catalog={'id':identifier,'instance_id':self.kube.instance['id'],'organisation_id':self.kube.instance['organisation_id'],
                'sha256':digest(cipher),'size_bytes':cipher.stat().st_size}
            verification=temp/'verification';verification.mkdir(mode=0o700)
            verified=verify_backup(cipher,catalog,self.kube.instance,identifier,images,self.identity,self.infra.run,verification)
            if digest(verification/'verified-backup.tar')!=digest(plain):raise BackupError('kubernetes_backup_encryption_readback_changed')
            receipt={'backup_id':identifier,'backup_sha256':verified.cipher_sha256,'backup_bytes':verified.cipher_bytes,'manifest_version':1}
            with cipher.open('rb') as stream:os.fsync(stream.fileno())
            os.replace(cipher,archive)
            self.infra.private_write(metadata,json.dumps({**receipt,'organisation_id':self.kube.instance['organisation_id'],
                'instance_id':self.kube.instance['id']},sort_keys=True).encode())
            return receipt

    def restore(self, operation, directory, active, *, images=None, activate=True, upgrade=False, verification_images=None):
        self.quiescent(active)
        backup_id=operation['arguments']['backup_id'];compact(backup_id)
        original=operation['backup'];source_images=verification_images or self.kube.images(active)
        generation=operation['id'];stage=directory/'generations'/compact(generation)
        if stage.exists():raise BackupError('original_kubernetes_restore_started_requires_reconciliation')
        with tempfile.TemporaryDirectory(dir=self.root,prefix='restore-kube-') as scratch:
            verified=verify_backup(self.root/(compact(backup_id)+'.age'),original,self.kube.instance,
                backup_id,source_images,self.identity,self.infra.run,Path(scratch))
            manifest,payload=verified.manifest,verified.payload
            inventory=manifest.get('kubernetes_original_inventory')
            if not isinstance(inventory,list) or {compact(v['workspace_id']) for v in inventory}!=set(manifest['workspace_runs']):
                raise BackupError('backup_has_no_original_kubernetes_workspace_inventory')
            stage.mkdir(mode=0o700,parents=True);(stage/'secrets').mkdir(mode=0o700)
            epochs.stage(self.infra,operation,stage)
            self.infra.private_write(stage/'generation.json',json.dumps({'generation':generation}).encode())
            self.infra.private_write(stage/'secrets/storage_master_key',(payload/'secrets/storage_master_key').read_bytes())
            self.infra.private_write(stage/'images.json',json.dumps(images or source_images,sort_keys=True).encode())
            values=self.infra.ensure_secrets(self.kube.instance,stage)
            self.infra.ensure_database(self.kube.instance,values,stage)
            env=pg_environment(values['database_url'],self.config)
            self.infra.run(['pg_restore','--exit-on-error','--no-owner','--no-privileges','--dbname',env['PGDATABASE'],str(payload/'database.dump')],env=env,timeout=21600)
            fence=RESTORE_FENCE_SQL
            if upgrade:fence=fence.replace(b'UPDATE schedules SET',b'CREATE TABLE cowork_upgrade_schedules AS SELECT id,next_run_at FROM schedules WHERE enabled;\nUPDATE schedules SET',1)
            self.infra.run(['psql','-X','-v','ON_ERROR_STOP=1'],env=env,data=fence)
            self.infra.ensure_bucket(self.kube.instance,values,stage)
            resources=self.infra.resource_names(self.kube.instance,stage)
            self.storage('mirror',str(payload/'objects'),self.config['s3_admin_alias']+'/'+resources['bucket'])
            for original_workspace in inventory:
                workspace=original_workspace['workspace_id'];original=self.kube.create_workspace(generation,workspace,operation['id'])
                archive=payload/'workspaces'/(compact(workspace)+'.tar')
                self.kube.workspace_archive(original,archive,restore=True)
                # Re-read restored files through the actual PVC mount and compare
                # semantic content, not tar's variable timestamps/order.
                readback=Path(scratch)/('readback-'+compact(workspace)+'.tar')
                self.kube.workspace_archive(original,readback,restore=False)
                if _workspace_inventory(archive)!=_workspace_inventory(readback):
                    raise BackupError('restored_kubernetes_workspace_readback_changed')
            self.infra.private_write(stage/'restore.json',json.dumps({'backup_id':backup_id,'previous_generation':
                self.infra.resource_names(self.kube.instance,active)['generation'],'original_manifest_sha256':verified.manifest_sha256}).encode())
            if activate:self.kube.promote(directory,generation)
            return {'backup_id':backup_id,'generation':generation,'instance_state':'stopped'}
