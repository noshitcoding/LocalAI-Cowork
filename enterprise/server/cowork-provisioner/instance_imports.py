"""Separate same-org import: original export identity, fresh target authority, durable old-run fences."""
import hashlib
import json
from pathlib import Path
import tempfile
import uuid
from instance_backups import InstanceBackups, BackupError, compact, pg_environment, verify_backup
from execution_generation import check, identifier, document

ACTIONS = {'import_quiesce':'quiesce','import_export':'export','import_restore':'restore','import_qualify':'qualify'}

def canonical(value):
    return hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest()

def binding(operation):
    value = operation.get('arguments',{}).get('import_binding')
    fields={'version','import_id','organisation_id','source','target','source_host_binding','target_host_binding',
            'source_allocation','target_allocation','source_epoch','target_epoch','candidate_epoch','backup','reservation_ids'}
    check(operation.get('action') in ACTIONS and isinstance(value,dict) and set(value)==fields and type(value['version']) is int and value['version']==1,'invalid_instance_import_binding')
    for name in ['import_id','organisation_id','source_epoch','target_epoch','candidate_epoch']:identifier(value[name])
    source,target=value['source'],value['target']
    check(isinstance(source,dict) and isinstance(target,dict),'invalid_instance_import_identities')
    for instance in [source,target]:
        identifier(instance['id']);check(instance['organisation_id']==value['organisation_id'],'import_cross_org_denied')
    check(source['id']!=target['id'] and value['candidate_epoch'] not in [value['source_epoch'],value['target_epoch']]
          and source.get('execution_epoch')==value['source_epoch'] and target.get('execution_epoch')==value['target_epoch']
          and target.get('executor_host_id') is not None and target.get('execution_required') is True,'import_fresh_target_required')
    acting=target if operation['action'] in ['import_restore','import_qualify'] else source
    check(operation['instance_id']==acting['id'] and operation['organisation_id']==value['organisation_id']
          and operation['instance']['id']==acting['id'],'import_operator_instance_mismatch')
    host=value['target_host_binding'] if acting is target else value['source_host_binding']
    check(operation.get('executor_host_binding')==host,'import_operator_host_mismatch')
    check(operation['arguments'].get('import_binding_sha256')==canonical(value),'import_binding_digest_mismatch')
    catalog=value['backup'];check(isinstance(catalog,dict) and catalog['instance_id']==source['id']
        and catalog['organisation_id']==value['organisation_id'],'import_original_catalog_required')
    identifier(catalog['id'])
    import re
    check(isinstance(catalog.get('sha256'),str) and re.fullmatch('[a-f0-9]{64}',catalog['sha256'])
        and type(catalog.get('size_bytes')) is int and catalog['size_bytes']>0,'invalid_import_catalog_integrity')
    for instance,host in [(source,value['source_host_binding']),(target,value['target_host_binding'])]:
        if instance.get('executor_host_id') is None:check(host is None,'import_unbound_source_host_mismatch')
        else:check(isinstance(host,dict) and host.get('version')==1 and host.get('host_id')==instance['executor_host_id']
            and host.get('trust_domain')==value['organisation_id'],'import_host_identity_mismatch')
    return value

def verify_export(operation, manifest):
    value=binding(operation);proof=operation['arguments']['import_export']
    check(isinstance(proof,dict) and proof['backup_id']==value['backup']['id']
          and proof['backup_sha256']==value['backup']['sha256']
          and proof['manifest_sha256']==canonical(manifest) and proof['workspace_count']==len(manifest['workspace_runs'])
          and proof['images']==manifest['images'],'import_original_export_mismatch')

def origin_fence(operation):
    value=binding(operation);proof=operation['arguments']['import_export']
    # All values are canonical UUIDs or a strictly checked hex digest, never SQL supplied by a tenant.
    import re
    manifest=proof['manifest_sha256'];check(isinstance(manifest,str) and re.fullmatch('[a-f0-9]{64}',manifest),'invalid_import_manifest_digest')
    fields=[identifier(value['import_id']),identifier(value['organisation_id']),identifier(value['source']['id']),
            identifier(value['target']['id']),identifier(value['backup']['id'])]
    sql="""BEGIN;
CREATE TABLE IF NOT EXISTS run_instance_import_origins (
 run_id UUID PRIMARY KEY REFERENCES runs(id), import_id UUID NOT NULL, organisation_id UUID NOT NULL,
 source_instance_id UUID NOT NULL, target_instance_id UUID NOT NULL, source_backup_id UUID NOT NULL,
 source_manifest_sha256 TEXT NOT NULL CHECK(source_manifest_sha256 ~ '^[a-f0-9]{64}$'),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), CHECK(source_instance_id<>target_instance_id));
INSERT INTO run_instance_import_origins(run_id,import_id,organisation_id,source_instance_id,target_instance_id,source_backup_id,source_manifest_sha256)
 SELECT id,'%s','%s','%s','%s','%s','%s' FROM runs ON CONFLICT(run_id) DO NOTHING;
UPDATE runs SET lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL;
CREATE OR REPLACE FUNCTION guard_imported_run_activation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF EXISTS(SELECT 1 FROM run_instance_import_origins o WHERE o.run_id=NEW.id)
  AND (NEW.state NOT IN ('completed','failed','canceled','expired','interrupted')
    OR NEW.lease_owner IS NOT NULL OR NEW.lease_token IS NOT NULL OR NEW.lease_expires_at IS NOT NULL) THEN
  RAISE EXCEPTION 'imported historical run cannot acquire execution authority'; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS imported_run_activation_fence ON runs;
CREATE TRIGGER imported_run_activation_fence BEFORE UPDATE ON runs FOR EACH ROW EXECUTE FUNCTION guard_imported_run_activation();
COMMIT;
""" % (*fields,manifest)
    return sql.encode()

def export(infra,operation,directory,manager):
    value=binding(operation);source=value['source'];active=infra.active_directory(directory)
    manager.config={**manager.config,'images':infra.deployed_images(directory)}
    manager.quiescent(source,active)
    archive=manager.root/(compact(value['backup']['id'])+'.age')
    with tempfile.TemporaryDirectory(prefix='import-export-',dir=manager.root) as temporary:
        verified=verify_backup(archive,value['backup'],source,value['backup']['id'],manager.config['images'],
                               manager.identity,manager.run,Path(temporary))
        manifest=verified.manifest
        inventory=canonical({'manifest':manifest,'files':manifest['files'],'workspace_runs':manifest['workspace_runs'],
                             'verified_workspace_inventory':verified.workspace_inventory})
        proof={'backup_id':value['backup']['id'],'backup_sha256':verified.cipher_sha256,'backup_bytes':verified.cipher_bytes,
               'manifest_sha256':verified.manifest_sha256,'images':manifest['images'],'workspace_count':len(manifest['workspace_runs']),
               'inventory_sha256':inventory}
    return proof

def _target_evidence(infra,operation,directory,*,job=None,manager=None):
    value=binding(operation);active=infra.active_directory(directory);instance=operation['instance']
    env=pg_environment((active/'secrets'/'database_url').read_text().strip(),infra.config)
    query="SELECT json_build_object('runs',(SELECT count(*) FROM runs),'origins',(SELECT count(*) FROM run_instance_import_origins),'active_runs',(SELECT count(*) FROM runs WHERE state NOT IN ('completed','failed','canceled','expired','interrupted') OR lease_owner IS NOT NULL OR lease_token IS NOT NULL OR lease_expires_at IS NOT NULL),'schedules',(SELECT count(*) FROM schedules WHERE enabled),'sessions',(SELECT count(*) FROM auth_sessions WHERE revoked_at IS NULL),'credentials',(SELECT count(*) FROM executor_credentials WHERE revoked_at IS NULL));"
    counts=json.loads(infra.run(['psql','-X','-tAc',query],env=env))
    check(counts['runs']==counts['origins'] and all(counts[k]==0 for k in ['active_runs','schedules','sessions','credentials']),'import_target_fences_unverified')
    manager = manager or InstanceBackups(infra)
    from instance_import_target import verify_target
    archive=manager.root/(compact(value['backup']['id'])+'.age')
    with tempfile.TemporaryDirectory(prefix='import-target-',dir=manager.root) as temporary:
        scratch=Path(temporary)
        verified=verify_backup(archive,value['backup'],value['source'],value['backup']['id'],
            operation['arguments']['import_export']['images'],manager.identity,manager.run,scratch,
            manifest_sha256=operation['arguments']['import_export']['manifest_sha256'])
        readback=scratch/'readback';readback.mkdir(mode=0o700)
        return verify_target(manager,operation,directory,active,verified,readback,job=job)

def execute(infra,operation,directory):
    value=binding(operation);step=ACTIONS[operation['action']];instance=operation['instance']
    manager=InstanceBackups(infra);active=infra.active_directory(directory)
    receipt={'version':1,'import_id':value['import_id'],'operation_id':operation['id'],'step':step,
        'binding_sha256':canonical(value),'source_inventory_sha256':None,'source_stopped':True,
        'target_stopped':step in ['restore','qualify'],'export':None,'target':None}
    if step=='quiesce':
        from instance_enrollment import stopped_inventory
        infra.stop_compose(instance,directory)
        _,_,_,inventory=stopped_inventory(infra,{**operation,'id':value['import_id']},directory)
        manager.quiescent(instance,active)
        receipt['source_inventory_sha256']=inventory
    elif step=='export':
        from instance_enrollment import stopped_inventory
        _,_,_,inventory=stopped_inventory(infra,{**operation,'id':value['import_id']},directory)
        check(inventory==operation['arguments']['import_source_stop']['source_inventory_sha256'],'import_source_stop_changed')
        receipt['export']=export(infra,operation,directory,manager)
        receipt['source_inventory_sha256']=inventory
    elif step=='restore':
        manager.quiescent(instance,active)
        env=pg_environment((active/'secrets'/'database_url').read_text().strip(),infra.config)
        empty=infra.run(['psql','-X','-tAc','SELECT (SELECT count(*) FROM threads)+(SELECT count(*) FROM runs)+(SELECT count(*) FROM schedules)+(SELECT count(*) FROM snapshot_manifests);'],env=env)
        check(empty.strip()==b'0','import_target_already_contains_user_data')
        restore={**operation,'backup':value['backup'],'arguments':{**operation['arguments'],'backup_id':value['backup']['id']}}
        manager.config={**manager.config,'images':infra.deployed_images(directory)}
        restored=manager.restore(restore,directory,active,source_import=value)
        manager.quiescent(instance,infra.active_directory(directory))
        receipt['source_inventory_sha256']=operation['arguments']['import_source_stop']['source_inventory_sha256']
        receipt['target']=restored['import_target']
    else:
        from instance_updates import InstanceUpdates
        # Private API/Runner probing applies migrations and executes one fresh
        # authenticated Runner job. Copied workers and schedules stay stopped.
        policy=instance['policy']['limits']
        check(policy['cpu_millis']>=100 and policy['memory_mib']>=128,'import_smoke_job_exceeds_profile')
        job = InstanceUpdates(infra).probe(instance,directory,active,worker=False)
        check(isinstance(job,dict) and job.get('exit_code')==0 and job.get('timed_out') is False,'import_actual_job_unverified')
        infra.stop_compose(instance,directory);manager.quiescent(instance,active)
        receipt['source_inventory_sha256']=operation['arguments']['import_source_stop']['source_inventory_sha256']
        receipt['target']=_target_evidence(infra,operation,directory,job=canonical(job),manager=manager)
    result={'receipt_id':operation['id'],'instance_state':'stopped','instance_import':receipt}
    validate_receipt(operation,result)
    return result

def validate_receipt(operation,result):
    value=binding(operation);proof=result.get('instance_import') if isinstance(result,dict) else None
    fields={'version','import_id','operation_id','step','binding_sha256','source_inventory_sha256','source_stopped','target_stopped','export','target'}
    check(isinstance(proof,dict) and set(proof)==fields and type(proof['version']) is int and proof['version']==1 and proof['import_id']==value['import_id']
          and proof['operation_id']==operation['id'] and proof['step']==ACTIONS[operation['action']]
          and proof['binding_sha256']==canonical(value) and proof['source_stopped'] is True,'import_receipt_binding_mismatch')
    import re
    check(isinstance(proof['source_inventory_sha256'],str) and re.fullmatch('[a-f0-9]{64}',proof['source_inventory_sha256']),'import_quiescence_inventory_missing')
    if operation['action']!='import_quiesce':
        check(proof['source_inventory_sha256']==operation['arguments']['import_source_stop']['source_inventory_sha256'],'import_source_stop_changed')
    if operation['action'] in ['import_restore','import_qualify']:
        target=proof['target'];expected=operation['arguments']['import_export']
        fields={'instance_id','execution_epoch','manifest_sha256','inventory_sha256','credentials_rotated','schedules_paused',
            'unfinished_runs_interrupted','historical_resume_blocked','workspace_inventory_complete','bounded_job_sha256'}
        check(proof['target_stopped'] is True and proof['export'] is None and isinstance(target,dict) and set(target)==fields
              and target['instance_id']==value['target']['id'] and target['execution_epoch']==value['candidate_epoch']
              and target['manifest_sha256']==expected['manifest_sha256'] and isinstance(target['inventory_sha256'],str)
              and re.fullmatch('[a-f0-9]{64}',target['inventory_sha256'])
              and all(target[k] is True for k in ['credentials_rotated','schedules_paused','unfinished_runs_interrupted','historical_resume_blocked','workspace_inventory_complete']),
              'import_paused_target_unverified')
        if operation['action']=='import_qualify':check(isinstance(target['bounded_job_sha256'],str) and re.fullmatch('[a-f0-9]{64}',target['bounded_job_sha256']),'import_actual_job_unverified')
    else:
        check(proof['target_stopped'] is False and proof['target'] is None,'source_operator_cannot_attest_target_stop')
        if operation['action']=='import_export':
            export=proof['export'];catalog=value['backup']
            check(isinstance(export,dict) and set(export)=={'backup_id','backup_sha256','backup_bytes','manifest_sha256','images','workspace_count','inventory_sha256'}
                and export['backup_id']==catalog['id'] and export['backup_sha256']==catalog['sha256']
                and type(export['backup_bytes']) is int and export['backup_bytes']==catalog['size_bytes']
                and type(export['workspace_count']) is int and export['workspace_count']>=0 and isinstance(export['images'],dict)
                and all(isinstance(export[k],str) and re.fullmatch('[a-f0-9]{64}',export[k]) for k in ['manifest_sha256','inventory_sha256']),
                'import_original_catalog_mismatch')
        else:check(proof['export'] is None,'import_quiescence_unexpected_export')
