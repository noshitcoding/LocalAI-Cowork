"""Prove a stopped Compose execution scope without releasing offline device work.

Only an explicit administrative operation supplies the selected central IDs.
The operator removes stopped service containers as well, so no acknowledged but
unstarted container or restart policy can resume the old producers afterwards.
Persistent files, credentials, databases, images and workspace volumes are kept.
"""
import hashlib
import json
import re
from pathlib import Path
import execution_generation as epochs
from execution_generation import check, identifier, document
from instance_backups import pg_environment
from instance_deletion import InstanceDeletion


def binding(operation):
    value=operation.get('execution_recovery_binding')
    arguments=operation.get('arguments',{})
    check(isinstance(arguments,dict),'invalid_execution_recovery_arguments')
    selected=arguments.get('reconcile_reservation_ids',[])
    if value is None:
        check(not selected,'execution_recovery_binding_required')
        return None
    check(operation['action']=='stop' and operation['arguments'].get('acknowledge_execution_stop') is True,
          'execution_recovery_stop_acknowledgement_required')
    check(isinstance(value,dict) and set(value)=={'generation','reservations'}
          and value['generation']==epochs.binding(operation),'execution_recovery_scope_mismatch')
    reservations=value['reservations']
    check(isinstance(reservations,list) and 0<len(reservations)<=100,'invalid_execution_recovery_reservations')
    ids=[]
    for reservation in reservations:
        check(isinstance(reservation,dict) and set(reservation)=={'reservation_id','execution_epoch','run_id'},'invalid_execution_recovery_reservation')
        for field in reservation: identifier(reservation[field])
        ids.append(reservation['reservation_id'])
    check(len(ids)==len(set(ids)) and len(selected)==len(ids) and set(ids)==set(selected),'execution_recovery_selection_mismatch')
    return value


def validate_receipt(operation,result):
    expected=binding(operation)
    value=result.get('execution_recovery')
    if expected is None:
        check(value is None,'unexpected_execution_recovery_receipt')
        return
    check(isinstance(value,dict) and set(value)=={'binding','stopped','inventory_sha256','verified_generations'}
          and value['binding']==expected and value['stopped'] is True
          and type(value['verified_generations']) is int and value['verified_generations']>0
          and isinstance(value['inventory_sha256'],str) and re.fullmatch('[a-f0-9]{64}',value['inventory_sha256']),
          'execution_recovery_receipt_mismatch')


def stop(infra,operation,directory):
    value=binding(operation)
    check(value is not None,'execution_recovery_binding_required')
    instance=operation['instance']
    import backend_profile
    profiled=backend_profile.vm_instance(infra.config,instance)
    if profiled:backend_profile.vm_scope(infra,operation)
    check(instance['backend']=='compose' and (instance['policy']['minimum_isolation']=='container' or profiled),
          'execution_recovery_backend_not_qualified')
    marker=directory/('execution-stop-'+identifier(operation['id']).replace('-','')+'.json')
    if marker.exists() or marker.is_symlink():
        receipt=document(marker,32768)
        validate_receipt(operation,{'execution_recovery':receipt})
        return receipt
    inventory=InstanceDeletion(infra)
    # Reuse the existing ownership validation; inventory() has no deletions.
    # This also rejects foreign container/network labels and unknown generations.
    before=inventory.inventory(operation,directory)
    infra.stop_compose(instance,directory)
    scope=before['scope']
    containers=inventory.docker('container',scope)
    for container in containers:
        labels=container.get('Config',{}).get('Labels') or {}
        check(labels.get('com.docker.compose.project')==scope
              and re.fullmatch(re.escape(scope)+r'-(?:api|worker|runner|egress-proxy)-[1-9][0-9]*',container['Name'].lstrip('/')),
              'execution_recovery_unproven_container')
        state=container.get('State',{})
        check(state.get('Running') is False and state.get('Paused') is False and state.get('Restarting') is False,
              'execution_recovery_container_still_active')
    if containers: infra.run(['docker','rm',*[c['Id'] for c in containers]])
    check(not inventory.docker('container',scope),'execution_recovery_containers_remain')
    after=inventory.inventory(operation,directory)
    check(before['resources']==after['resources'],'execution_recovery_generation_changed')
    env=pg_environment(Path(infra.config['database_admin_url_file']).read_text().strip())
    for resource in after['resources']:
        connections=infra.run(['psql','-X','-tAc',f"SELECT count(*) FROM pg_stat_activity WHERE datname='{resource['database']}';"],env=env)
        check(connections.strip()==b'0','execution_recovery_database_still_active')
    digest=hashlib.sha256(json.dumps({'binding':value,'inventory':before},sort_keys=True,separators=(',',':')).encode()).hexdigest()
    receipt={'binding':value,'stopped':True,'inventory_sha256':digest,'verified_generations':len(after['resources'])}
    validate_receipt(operation,{'execution_recovery':receipt})
    infra.private_write(marker,json.dumps(receipt,sort_keys=True).encode())
    return receipt
