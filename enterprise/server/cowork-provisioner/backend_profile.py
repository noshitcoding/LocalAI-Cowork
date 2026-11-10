"""Frozen manager backend profile, original assignment and fresh physical probe."""
import hashlib
import json
import os
from pathlib import Path
import re
import uuid

class ProfileError(Exception):
    pass

def require(ok,message):
    if not ok:
        raise ProfileError(message)

def canonical(value):
    return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()

def settings(config):
    value=config.get('backend_profile')
    if value is None:
        return None
    require(isinstance(value,dict) and set(value)=={'version','specification','activation_receipt_file','probe_command','probe_script_sha256'}
            and value['version']==1,'invalid_backend_profile_configuration')
    specification=value['specification']
    require(specification['format']==1 and specification['kind'] in ('vm_compose','kubernetes'),'invalid_backend_profile_specification')
    for key in ('instance_id','organisation_id','host_id','operator_state_id'):
        require(str(uuid.UUID(specification[key]))==specification[key],'invalid_backend_profile_identity')
    command=value['probe_command']
    require(isinstance(command,list) and len(command)>=4 and all(isinstance(v,str) and v and '\0' not in v for v in command)
            and Path(command[0]).is_absolute() and Path(command[1]).is_absolute(),'invalid_exact_backend_probe_command')
    script=Path(command[1])
    require(script.is_file() and not script.is_symlink() and all(not p.is_symlink() for p in script.parents)
            and hashlib.sha256(script.read_bytes()).hexdigest()==value['probe_script_sha256'],'backend_probe_source_changed')
    if os.name=='posix':
        require(script.stat().st_mode&0o022==0,'backend_probe_is_writable_by_other_users')
    receipt_path=Path(value['activation_receipt_file'])
    require(receipt_path.is_absolute() and receipt_path.is_file() and not receipt_path.is_symlink()
            and all(not p.is_symlink() for p in receipt_path.parents),'backend_activation_receipt_missing')
    receipt=json.loads(receipt_path.read_bytes())
    require(receipt['format']=='cowork-backend-activation-v1' and receipt['instance_id']==specification['instance_id']
            and receipt['organisation_id']==specification['organisation_id']
            and receipt['specification_sha256']==hashlib.sha256(canonical(specification)).hexdigest(),'backend_activation_receipt_scope_differs')
    return value

def manager_binding(config):
    selected=settings(config)
    if selected is None or selected['specification']['kind']!='kubernetes':
        return None
    spec=selected['specification']
    return {'version':1,'host_id':spec['host_id'],'trust_domain':spec['organisation_id'],
            'operator_state_id':spec['operator_state_id'],'daemon_id':spec['cluster_uid']}

def operation(config,operation):
    frozen=operation.get('backend_profile')
    selected=settings(config)
    if frozen is None:
        require(selected is None,'manager_did_not_freeze_configured_backend_profile')
        return None
    require(selected is not None and frozen['specification']==selected['specification'],'manager_operator_backend_profile_differs')
    spec=selected['specification']; instance=operation['instance']
    receipt=json.loads(Path(selected['activation_receipt_file']).read_bytes())
    require(frozen['specification_sha256']==receipt['specification_sha256'] and frozen['revision']==receipt['profile_revision']
            and frozen['plan_id']==receipt['plan_id'] and instance['id']==spec['instance_id']
            and instance['organisation_id']==spec['organisation_id'] and instance['executor_host_id']==spec['host_id']
            and operation['instance_id']==instance['id'] and operation['organisation_id']==instance['organisation_id']
            and operation['executor_host_binding']==frozen['executor_host_binding'],'frozen_backend_operation_assignment_differs')
    return frozen

def validate(infra,value,*,physical=True):
    frozen=operation(infra.config,value)
    if frozen is None:
        return None
    selected=settings(infra.config)
    if physical:
        observation=json.loads(infra.run([*selected['probe_command'],'--challenge',value['id'],
                                        '--specification-sha256',frozen['specification_sha256']]))
        require(observation['challenge']==value['id'] and observation['specification_sha256']==frozen['specification_sha256']
                and observation['executor_host_binding']==frozen['executor_host_binding'],'fresh_backend_physical_probe_differs')
        spec=frozen['specification']; actual=observation['physical']
        require(observation['routing']['serving_https_verified'] is True and observation['routing']['configuration_sha256']==spec['routing']['configuration_sha256'],'fresh_backend_routing_probe_differs')
        if spec['kind']=='vm_compose':
            require(actual['vm_id']==spec['vm_id'] and actual['kvm_enabled'] is True and actual['qemu_owned'] is True
                    and actual['mutual_tls_verified'] is True and actual['daemon_id']==spec['daemon_id'],'fresh_vm_physical_probe_differs')
        else:
            require(actual['cluster_uid']==spec['cluster_uid'] and actual['namespace_uid']==spec['namespace_uid']
                    and actual['runner']['backend']=='kubernetes' and actual['runner']['instance_id']==spec['instance_id']
                    and actual['runner']['organisation_id']==spec['organisation_id'], 'fresh_kubernetes_physical_probe_differs')
    infra._backend_physically_verified_operation_id=value['id'] if physical else None
    infra._backend_operation_id=value['id'];infra._backend_profile=frozen
    return frozen

def vm_instance(config,instance):
    selected=settings(config)
    return selected is not None and selected['specification']['kind']=='vm_compose' and selected['specification']['instance_id']==instance['id']

def vm_scope(infra,value,action=None):
    instance=value['instance'] if 'instance' in value else value
    require(vm_instance(infra.config,instance) and instance['backend']=='compose'
            and instance['policy']['minimum_isolation']=='virtual_machine','vm_backend_profile_scope_required')
    require(getattr(infra,'_backend_profile',None) is not None and infra._backend_profile['instance_id']==instance['id'],'fresh_vm_operation_probe_required')
    require(getattr(infra,'_backend_physically_verified_operation_id',None)==infra._backend_operation_id,'actual_vm_operation_probe_required')
    if 'instance' in value:
        frozen=operation(infra.config,value)
        require(infra._backend_operation_id==value['id'] and frozen==infra._backend_profile,'fresh_vm_original_operation_differs')
    return settings(infra.config)

def attach(operation,receipt):
    frozen=operation.get('backend_profile')
    if frozen is None:
        require('backend_profile' not in receipt,'unexpected_backend_receipt')
        return receipt
    bound={'instance_id':frozen['instance_id'],'profile_revision':frozen['revision'],'specification_sha256':frozen['specification_sha256']}
    require('backend_profile' not in receipt or receipt['backend_profile']==bound,'original_backend_receipt_differs')
    return {**receipt,'backend_profile':bound}

def validate_receipt(config,operation_value,receipt):
    operation(config,operation_value)
    require(attach(operation_value,receipt)==receipt,'backend_receipt_binding_missing')

def serving(infra,value):
    frozen=operation(infra.config,value)
    if frozen is None or value['action'] not in ('provision','start','resume'):
        return
    selected=settings(infra.config)
    observation=json.loads(infra.run([*selected['probe_command'],'--challenge',value['id'],'--specification-sha256',frozen['specification_sha256'],'--runtime-ready']))
    require(observation['challenge']==value['id'] and observation['specification_sha256']==frozen['specification_sha256']
            and observation['routing']['runtime_ready_verified'] is True,'actual backend public runtime readiness is unproven')
