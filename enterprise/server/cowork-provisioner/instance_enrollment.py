"""Monotonic, explicitly dispatched legacy enrollment; never starts workloads."""
import json
from pathlib import Path
import re
import tempfile
import uuid

import execution_generation as epochs
import execution_enrollment as preflight
from instance_backups import InstanceBackups, pg_environment
from instance_deletion import InstanceDeletion, safe_tree


def binding(operation):
    expected=epochs.binding(operation)
    epochs.check(operation['action']=='enroll_execution' and expected is not None
        and operation.get('execution_runtime_required') is True
        and operation['instance'].get('execution_epoch')==expected['source_epoch']
        and operation['arguments'].get('acknowledge_execution_enrollment') is True,
        'execution_enrollment_binding_required')
    return expected


def validate_receipt(operation,result):
    proof=result.get('execution_enrollment')
    if operation['action']!='enroll_execution':
        epochs.check(proof is None,'unexpected_execution_enrollment_receipt')
        return
    expected=binding(operation)
    epochs.check(isinstance(proof,dict) and set(proof)=={'binding','outcome','source_manifest_sha256','inventory_sha256','verified_generations'}
        and proof['binding']==expected and isinstance(proof['source_manifest_sha256'],str)
        and re.fullmatch('[a-f0-9]{64}',proof['source_manifest_sha256'])
        and type(proof['verified_generations']) is int and result.get('instance_state')=='stopped', 'invalid_execution_enrollment_receipt')
    if proof['outcome']=='enrolled':
        epochs.check(proof['verified_generations']>0 and isinstance(proof['inventory_sha256'],str)
            and re.fullmatch('[a-f0-9]{64}',proof['inventory_sha256']) and result.get('backup_id')==operation['id']
            and not result.get('error_code'), 'unproven_execution_enrollment')
    else:
        epochs.check(proof['outcome']=='unchanged' and proof['verified_generations']==0 and proof['inventory_sha256'] is None
            and result.get('error_code')=='execution_enrollment_precheck_failed', 'unproven_execution_enrollment')


def configuration(infra,instance):
    # Endpoint/policy changes after the commit point require review. Merely adding
    # another qualified image to the operator allowlist does not rewrite this plan.
    keys=['backend','fleet_management_url','services_network','routing_network','sandbox_security_profile',
          'database_endpoint','s3_endpoint','compose_network_pool']
    return preflight.canonical({'config':{key:infra.config.get(key) for key in keys},'policy':instance['policy']})


def stopped_inventory(infra,operation,directory):
    inventory=InstanceDeletion(infra)
    value=inventory.inventory(operation,directory)
    epochs.check(not value['staging'],'enrollment_backup_staging_unsettled')
    containers=inventory.docker('container',value['scope'])
    for container in containers:
        labels=container.get('Config',{}).get('Labels') or {}
        epochs.check(labels.get('com.docker.compose.project')==value['scope']
            and re.fullmatch(re.escape(value['scope'])+r'-(?:api|worker|runner|egress-proxy)-[1-9][0-9]*',container['Name'].lstrip('/'))
            and all(container.get('State',{}).get(key) is False for key in ['Running','Paused','Restarting']),
            'enrollment_requires_stopped_owned_service_containers')
    admin=pg_environment(Path(infra.config['database_admin_url_file']).read_text().strip())
    observations=[]
    for resource in value['resources']:
        database=resource['database']
        epochs.check(re.fullmatch('ci_[a-f0-9]{32}',database),'invalid_enrollment_database')
        epochs.check(preflight.sql(infra,admin,"SELECT count(*) FROM pg_stat_activity WHERE datname='"+database+"';")=='0',
                     'enrollment_database_still_active')
        work=preflight.counts(infra,{**admin,'PGDATABASE':database})
        # Historical local-worker IDs are safe only when this database has never
        # registered an external executor. Registered/offline agents need their
        # own completion evidence; expiry and revocation are not such evidence.
        # These rows cannot be claimed automatically: interrupted work needs a
        # user transition; an unstarted snapshot wait needs its validated upload.
        # Preserve their data/state, but only after every producer is proven
        # stopped and every registered/external executor and reservation excluded.
        epochs.check(work['unfinished_runs']==work['preservable_server_runs'],
                     'enrollment_work_requires_reconciliation')
        permitted={'legacy_assignments_requiring_review','enabled_schedules','unfinished_runs','preservable_server_runs'}
        epochs.check(not any(count for key,count in work.items() if key not in permitted),'enrollment_work_requires_reconciliation')
        observations.append({'generation':resource['generation'],'work':work})
    return inventory,value,containers,preflight.canonical({'inventory':value,'observations':observations})


def execute(infra,operation,directory,*,resume=False):
    expected=binding(operation);instance=operation['instance']
    import backend_profile
    epochs.check(infra.config.get('execution_enrollment_enabled') is True
        and instance.get('execution_required') is False and instance['state']=='stopped'
        and instance['backend']=='compose' and (instance['policy']['minimum_isolation']=='container' or backend_profile.vm_instance(infra.config,instance)), 'execution_enrollment_not_available')
    safe_tree(directory,infra.root)
    active=infra.active_directory(directory)
    manifest_path=directory/'compose.json'
    manifest=epochs.document(manifest_path,1048576)
    for name in ['api','worker']:
        env=manifest['services'][name]['environment']
        epochs.check(env.get('COWORK_INSTANCE_ID')==instance['id'] and env.get('COWORK_ORGANISATION_ID')==instance['organisation_id'],
                     'enrollment_manifest_scope_mismatch')
    marker=directory/('enrollment-'+uuid.UUID(operation['id']).hex+'.json')
    if marker.exists() or marker.is_symlink():
        epochs.check(resume,'enrollment_requires_explicit_recovery')
        plan=epochs.document(marker,32768)
        epochs.check(plan.get('version')==1 and plan.get('binding')==expected
            and plan.get('configuration_sha256')==configuration(infra,instance)
            and plan.get('active_generation')==infra.resource_names(instance,active)['generation'], 'enrollment_plan_changed')
    else:
        epochs.check(epochs.read(instance,active) is None,'instance_already_has_execution_epoch')
        images=infra.deployed_images(directory)
        plan={'version':1,'binding':expected,'phase':'checking','source_manifest_sha256':preflight.canonical(manifest),
              'configuration_sha256':configuration(infra,instance),'active_generation':infra.resource_names(instance,active)['generation'],
              'images':images,'backup':None}
        infra.private_write(marker,json.dumps(plan,sort_keys=True).encode())
    epochs.check(plan.get('phase') in ['checking','prepared'],'invalid_enrollment_phase')
    backup={}
    try:
        if epochs.read(instance,active) is None:
            epochs.check(preflight.canonical(manifest)==plan['source_manifest_sha256'],'enrollment_source_manifest_changed')
            inventory,before,containers,_=stopped_inventory(infra,operation,directory)
            manager=InstanceBackups(infra)
            manager.config={**manager.config,'images':plan['images']}
            archive=manager.root/(uuid.UUID(operation['id']).hex+'.age')
            # A previously captured archive belongs to this exact frozen operation.
            # Reuse only after full integrity verification, never overwrite it.
            if not archive.exists() and not archive.with_suffix('.json').exists():
                manager.quiescent(instance,active)
                backup=manager.backup(operation,directory,active)
            with tempfile.TemporaryDirectory(prefix='cowork-enrollment-check-') as temporary:
                verified=preflight.verify_backup(infra,instance,operation['id'],plan['images'],Path(temporary))
            backup={'backup_id':operation['id'],'backup_sha256':verified['sha256'],'backup_bytes':verified['bytes'],'manifest_version':1}
            observed=preflight.inspect(infra,instance,operation['id'],current_operation=operation)
            epochs.check(not set(observed['blockers'])-{'legacy_assignments_requiring_review','enabled_schedules','unfinished_runs','preservable_server_runs'},
                         'enrollment_work_requires_reconciliation')
            inventory,after,containers,inventory_hash=stopped_inventory(infra,operation,directory)
            epochs.check(before['resources']==after['resources'],'enrollment_resources_changed')
            if containers:infra.run(['docker','rm',*[c['Id'] for c in containers]])
            epochs.check(not inventory.docker('container',after['scope']),'enrollment_containers_remain')
            plan.update(phase='prepared',backup=backup,inventory_sha256=inventory_hash,verified_generations=len(after['resources']))
            infra.private_write(marker,json.dumps(plan,sort_keys=True).encode())
            # Commit point: after this immutable marker appears, no failure can
            # certify a legacy runtime or roll this instance back to unbound work.
            epochs.write(infra,instance,active,expected['source_epoch'])
        else:
            epochs.check(plan['phase']=='prepared' and epochs.read(instance,active)==expected['source_epoch'],
                         'enrollment_commit_unproven')
            backup=plan['backup']
            with tempfile.TemporaryDirectory(prefix='cowork-enrollment-check-') as temporary:
                verified=preflight.verify_backup(infra,instance,operation['id'],plan['images'],Path(temporary))
            epochs.check(verified['sha256']==backup['backup_sha256'],'enrollment_backup_changed')
        epochs.check(infra.deployed_images(directory)==plan['images'],'enrollment_installed_images_changed')
        infra.ensure_secrets(instance,active)
        infra.write_compose(instance,directory)
        _,observed,containers,inventory_hash=stopped_inventory(infra,operation,directory)
        epochs.check(not containers,'enrollment_containers_remain')
        result={**backup,'instance_state':'stopped','execution_enrollment':{'binding':expected,'outcome':'enrolled',
            'source_manifest_sha256':plan['source_manifest_sha256'],'inventory_sha256':inventory_hash,
            'verified_generations':len(observed['resources'])}}
        validate_receipt(operation,result)
        return result
    except Exception as error:
        # An unchanged, verified original configuration can be returned as a
        # failed operation. Ambiguous/committed changes stay in reconciliation.
        if epochs.read(instance,active) is None and preflight.canonical(epochs.document(manifest_path,1048576))==plan['source_manifest_sha256']:
            from provisioner import ProvisioningError
            result={**backup,'receipt_id':operation['id'],'instance_state':'stopped',
                'execution':{'binding':expected,'active_epoch':expected['source_epoch']},
                'execution_enrollment':{'binding':expected,'outcome':'unchanged','source_manifest_sha256':plan['source_manifest_sha256'],
                    'inventory_sha256':None,'verified_generations':0}}
            raise ProvisioningError('execution_enrollment_precheck_failed','stopped',result) from None
        raise epochs.ExecutionGenerationError('execution_enrollment_requires_reconciliation') from error
