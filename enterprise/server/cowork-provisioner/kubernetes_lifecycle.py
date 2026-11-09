"""Scoped Helm, Kubernetes UID, generation, and original operation lifecycle.

No namespace-wide stop/delete, Docker emulation, or caller manifest adoption.
Infrastructure commands are generated from the reviewed operator configuration.
"""
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import ssl
import tempfile
import time
import threading
import uuid
from urllib.parse import urlsplit
from urllib.request import Request,build_opener,ProxyHandler,HTTPSHandler,HTTPRedirectHandler

import backend_profile
import execution_generation as epochs
from execution_runtime import CONTRACT
from instance_backups import compact, pg_environment
from instance_deletion import safe_tree
from kubernetes_archive import Archives
from release_catalog import ReleaseCatalog, image_set_id, validate_release


def require(condition, code):
    if not condition:
        from provisioner import ProvisioningError
        raise ProvisioningError(code)


def sha(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False,
                                     separators=(',', ':')).encode()).hexdigest()


def operation_digest(operation):
    from operation_journal import binding
    instance=operation['instance']
    return sha({'binding':binding(operation),'instance_scope':{
        k:instance.get(k) for k in ('id','organisation_id','executor_host_id','backend','policy','provisioning_release')
    }})


def deletion_marker(directory,operation):
    return directory/('kube-deletion-'+compact(operation['id'])+'.json')


def validate_deletion_plan(infra,operation,directory):
    profile=backend_profile.operation(infra.config,operation)
    require(profile is not None and profile['specification']['kind']=='kubernetes'
            and operation['action']=='delete' and operation['arguments'].get('purge_backups') is True
            and operation['arguments'].get('confirmation_name')==operation['instance']['name'],
            'original_kubernetes_deletion_authority_required')
    plan=epochs.document(deletion_marker(directory,operation),16*1024*1024)
    spec=profile['specification']
    require(plan['version']==1 and plan['operation_id']==operation['id']
            and plan['original_operation_sha256']==operation_digest(operation)
            and plan['instance_id']==operation['instance']['id'] and plan['organisation_id']==operation['organisation_id']
            and plan['namespace']==spec['namespace'] and plan['namespace_uid']==spec['namespace_uid']
            and plan['cluster_uid']==spec['cluster_uid'] and plan['backend_profile']==profile
            and plan['database_endpoint']==infra.config['database_endpoint'] and plan['s3_endpoint']==infra.config['s3_endpoint']
            and plan['backup_root']==str(Path(infra.config['backup_directory']).resolve()),
            'original_kubernetes_deletion_plan_changed')
    for row in plan['kubernetes']:
        epochs.identifier(row['uid']);require(re.fullmatch('[a-z0-9][a-z0-9.-]{0,252}',row['name']) is not None,
                                             'invalid_original_deletion_resource_name')
    for row in plan['pods']:epochs.identifier(row['uid'])
    for ident in plan['backup_ids']:epochs.identifier(ident)
    require(len({(r['kind'],r['name']) for r in plan['kubernetes']})==len(plan['kubernetes']),
            'duplicate_original_deletion_resource')
    require(plan['resources'] and len({r['generation'] for r in plan['resources']})==len(plan['resources']),
            'invalid_original_deletion_generations')
    for row in plan['resources']:
        if row['generation'] is not None:epochs.identifier(row['generation'])
        storage=hashlib.sha256((plan['instance_id']+':'+row['generation']).encode()).hexdigest()[:32] if row['generation'] else compact(plan['instance_id'])
        require(row['database']=='ci_'+storage and row['bucket']=='cowork-'+storage
                and (row['s3_user'] is None or re.fullmatch('[a-f0-9]{64}',row['s3_user'])),
                'invalid_original_deletion_data_identity')
    return plan


def partial_deletion_pending(infra,operation,directory):
    if operation['action']!='delete' or not deletion_marker(directory,operation).exists():return False
    validate_deletion_plan(infra,operation,directory)
    return True


def deletion_scope(kube):
    cluster=kube.object('namespace','kube-system');namespace=kube.object('namespace',kube.namespace)
    require(cluster and cluster['metadata']['uid']==kube.spec['cluster_uid'],'original_deletion_cluster_changed')
    if namespace is not None:
        require(namespace['metadata']['uid']==kube.spec['namespace_uid'],'original_deletion_namespace_was_replaced')
        labels=namespace['metadata'].get('labels',{})
        require(labels.get('cowork.instance-id')==kube.instance['id']
                and labels.get('cowork.organisation-id')==kube.instance['organisation_id']
                and labels.get('cowork.executor-host-id')==kube.spec['host_id'],'original_deletion_namespace_assignment_changed')
    return namespace


def preflight_release(kube,values,stage='primary'):
    """Freeze the actual chart and refuse any unbound existing Helm target."""
    import yaml
    storage=kube.object('storageclass',kube.settings['storage_class'])
    require(storage and storage.get('reclaimPolicy','Delete')=='Delete' and storage.get('provisioner')
            and kube.object('csidriver',storage['provisioner']) is not None,'reviewed_CSI_Delete_storage_class_required')
    source=Path(kube.infra.config['instance_chart'])
    require(source.is_absolute() and source.is_dir() and not source.is_symlink()
            and all(not p.is_symlink() for p in source.parents),'original_operator_chart_required')
    files={}
    for path in source.rglob('*'):
        require(not path.is_symlink(),'chart_symlink_is_not_an_original_source')
        if path.is_file():
            require(path.stat().st_size<=1048576 and len(files)<256,'original_chart_exceeds_bound')
            if os.name=='posix':require(path.stat().st_mode&0o022==0,'chart_writable_by_another_user')
            files[path.relative_to(source).as_posix()]=path.read_bytes()
    source_sha=sha({n:hashlib.sha256(b).hexdigest() for n,b in sorted(files.items())})
    frozen=kube.directory/('helm-original-'+compact(kube.operation['id'])+'-'+stage)
    if frozen.exists():
        require(frozen.is_dir() and not frozen.is_symlink(),'original_frozen_chart_path_changed')
        actual={p.relative_to(frozen).as_posix():p.read_bytes() for p in frozen.rglob('*') if p.is_file() and not p.is_symlink()}
        require(actual==files,'original_chart_changed_after_dispatch')
    else:
        frozen.mkdir(mode=0o700)
        for name,data in files.items():
            target=frozen/name;target.parent.mkdir(mode=0o700,parents=True,exist_ok=True);kube.infra.private_write(target,data)
    require(all(p.read_bytes()==files[p.relative_to(source).as_posix()] for p in source.rglob('*') if p.is_file()),
            'original_chart_changed_during_capture')
    path=kube.directory/'helm-values.json';kube.infra.private_write(path,json.dumps(values).encode())
    rendered=kube.infra.run(['helm','template',kube.release,str(frozen),'--namespace',kube.namespace,'--values',str(path)])
    require(len(rendered)<=16*1024*1024,'original_rendered_chart_exceeds_bound')
    kinds={'Deployment':'deployment','Service':'service','Secret':'secret','ConfigMap':'configmap','ServiceAccount':'serviceaccount',
        'Role':'role','RoleBinding':'rolebinding','ClusterRole':'clusterrole','ClusterRoleBinding':'clusterrolebinding',
        'NetworkPolicy':'networkpolicy','ResourceQuota':'resourcequota','PodDisruptionBudget':'poddisruptionbudget','PersistentVolumeClaim':'pvc'}
    targets={}
    for value in yaml.safe_load_all(rendered):
        if value is None:continue
        require(isinstance(value,dict) and value.get('kind') in kinds,'unreviewed_helm_resource_kind')
        metadata=value['metadata'];name=metadata['name'];kind=kinds[value['kind']]
        require(re.fullmatch('[a-z0-9][a-z0-9.-]{0,252}',name) is not None
                and metadata.get('namespace',kube.namespace)==kube.namespace
                and all(metadata.get('labels',{}).get(k)==v for k,v in kube.labels().items()),'original_helm_resource_scope_differs')
        require((kind,name) not in targets,'duplicate_original_helm_resource')
        targets[kind,name]=value
    saved=(kube.directory/'kubernetes.json').exists()
    manifest=kube.manifest() if saved else None
    expected={(r['kind'],r['name']):r['uid'] for r in (manifest['resources'] if saved else kube.settings.get('bootstrap_resources',[]))}
    if saved:
        generations=[kube.directory]
        parent=kube.directory/'generations'
        if parent.exists():
            require(not parent.is_symlink() and parent.is_dir(),'original_retained_generation_directory_changed')
            generations.extend(p for p in parent.iterdir() if p.is_dir() and not p.is_symlink())
        for directory in generations:
            for original in kube.workspace_inventory(directory):
                expected['pvc',original['name']]=original['uid']
                expected['configmap','receipt-'+original['name']]=original['receipt_uid']
    release=( {'secret_name':'sh.helm.release.v1.'+kube.release+'.v'+str(manifest['release_revision']),
               'uid':manifest['release_uid'],'revision':manifest['release_revision']} if saved else kube.settings.get('bootstrap_release'))
    bookkeeping=[]
    for kind in sorted(set(kinds.values())):
        if kind in ('clusterrole','clusterrolebinding'):actuals=[kube.object(kind,n) for k,n in set(targets)|set(expected) if k==kind]
        else:actuals=kube.list(kind)
        for actual in actuals:
            if actual is None:continue
            name=actual['metadata']['name'];key=(kind,name);labels=actual['metadata'].get('labels',{})
            if kind=='secret' and labels.get('owner')=='helm':
                require(release is not None and labels.get('name')==kube.release
                        and re.fullmatch(re.escape('sh.helm.release.v1.'+kube.release+'.v')+'[1-9][0-9]{0,9}',name),
                        'unbound_helm_release_in_original_namespace')
                bookkeeping.append(actual);continue
            if kind=='secret' and name==kube.settings['runner_tls']['secret_name']:
                require(actual['metadata']['uid']==kube.settings['runner_tls']['certificate_uid'],'original_runner_tls_secret_changed');continue
            uid=expected.get(key)
            require(uid is not None,'existing_helm_target_has_no_original_uid')
            if key==('serviceaccount','default'):
                require(actual['metadata']['uid']==uid,'original_default_service_account_changed');continue
            kube.owned(actual,uid)
    if release is not None:
        selected=kube.object('secret',release['secret_name'])
        require(selected and selected['metadata']['uid']==release['uid'] and selected['metadata'].get('labels',{}).get('owner')=='helm'
                and selected['metadata'].get('labels',{}).get('name')==kube.release,
                'original_helm_release_uid_changed')
        actual=json.loads(kube.infra.run(['helm','status',kube.release,'--namespace',kube.namespace,'-o','json']))
        require(actual['version']==release['revision'] and actual['info']['status']=='deployed','original_helm_release_revision_changed')
    else:require(not bookkeeping,'unbound_original_helm_release_exists')
    for kind,name in targets:
        current=kube.object(kind,name)
        if (kind,name) in expected:require(current is not None,'original_helm_target_disappeared_requires_review')
        if current is not None:require(current['metadata']['uid']==expected.get((kind,name)),'original_helm_target_uid_changed')
    kube._preflight_resources=expected;kube._preflight_helm_sha256=hashlib.sha256(rendered).hexdigest()
    kube._preflight_release_revision=0 if release is None else release['revision']
    proof=kube.directory/('helm-preflight-'+compact(kube.operation['id'])+'-'+stage+'.json')
    document={'version':1,'original_operation_sha256':operation_digest(kube.operation),'chart_source_sha256':source_sha,
        'helm_manifest_sha256':kube._preflight_helm_sha256,'namespace_uid':kube.spec['namespace_uid'],
        'values_sha256':sha(values),'storage_class_uid':storage['metadata']['uid'],
        'original_resources':[{'kind':k,'name':n,'uid':u} for (k,n),u in sorted(expected.items())],
        'original_release':release}
    if proof.exists():require(epochs.document(proof,16*1024*1024)==document,'original_helm_preflight_changed')
    else:kube.infra.private_write(proof,json.dumps(document,sort_keys=True).encode())
    return str(frozen)


def settings(config):
    value = config.get('kubernetes_execution')
    required = {'version','runtime_class','storage_class','node_selector','max_workspace_archives',
                'max_archive_bytes','max_workspace_archive_bytes','runner_tls','service_cidrs',
                'routing_namespace','runner_ca_file','runner_certificate_sha256','system_pod_uids','node_names','node_uids','workspace_claim_gib','runner_url','desktop_profile'}
    require(isinstance(value, dict) and required<=set(value) and set(value)<=required|{'bootstrap_resources','bootstrap_release'} and value['version'] == 1,
            'explicit_kubernetes_execution_configuration_required')
    for key in ('runtime_class','storage_class','routing_namespace'):
        require(isinstance(value[key],str) and re.fullmatch('[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?',value[key]),'invalid_kubernetes_operator_name')
    for key, bound in [('max_workspace_archives',10000),('max_archive_bytes',1<<44),('max_workspace_archive_bytes',1<<40)]:
        require(type(value[key]) is int and 0 < value[key] <= bound,'invalid_kubernetes_archive_bound')
    require(isinstance(value['node_selector'],dict) and value['node_selector'].get('cowork.role')=='executor',
            'dedicated_kubernetes_executor_nodes_required')
    selected=backend_profile.settings(config)
    require(selected is not None and selected['specification']['kind']=='kubernetes','original_kubernetes_backend_profile_required')
    spec=selected['specification']
    require(value['node_selector'].get('cowork.trust-domain')==spec['organisation_id']
            and value['node_selector'].get('cowork.host')==spec['host_id'],'kubernetes_node_trust_scope_differs')
    require(isinstance(value['service_cidrs'],list) and value['service_cidrs'],'kubernetes_service_networks_required')
    require(isinstance(value['runner_tls'],dict) and set(value['runner_tls'])=={'secret_name','certificate_uid'}
            and isinstance(value['system_pod_uids'],list),'kubernetes_original_tls_and_system_inventory_required')
    for uid in value['system_pod_uids']:epochs.identifier(uid)
    require(isinstance(value['node_names'],list) and value['node_names'] and len(value['node_names'])<=1000
            and len(value['node_names'])==len(set(value['node_names'])) and all(isinstance(n,str)
            and re.fullmatch('[a-z0-9][a-z0-9.-]{0,251}[a-z0-9]',n) for n in value['node_names']),
            'explicit_original_executor_node_inventory_required')
    require(isinstance(value['node_uids'],dict) and set(value['node_uids'])==set(value['node_names']),
            'original_executor_node_uids_required')
    for uid in value['node_uids'].values():epochs.identifier(uid)
    require(type(value['workspace_claim_gib']) is int and 0<value['workspace_claim_gib']<=1048576,'bounded_workspace_claim_size_required')
    endpoint=urlsplit(value['runner_url'])
    require(endpoint.scheme=='https' and endpoint.hostname and not endpoint.username and not endpoint.password
            and endpoint.path in ('','/') and not endpoint.query and not endpoint.fragment,'exact_https_runner_control_endpoint_required')
    desktop=value['desktop_profile']
    if desktop is not None:
        require(isinstance(desktop,dict) and set(desktop)=={'version','runtime_class','node_selector','node_names','node_uids'}
                and desktop['version']==1 and desktop['node_names'] and set(desktop['node_names'])==set(desktop['node_uids'])
                and not set(desktop['node_names'])&set(value['node_names']),'explicit_separate_native_desktop_node_profile_required')
        require(desktop['node_selector'].get('cowork.role')=='executor' and desktop['node_selector'].get('cowork.trust-domain')==spec['organisation_id']
                and desktop['node_selector'].get('cowork.host')==spec['host_id'],'original_native_desktop_node_trust_scope_differs')
        for uid in desktop['node_uids'].values():epochs.identifier(uid)
    epochs.identifier(value['runner_tls']['certificate_uid'])
    require(re.fullmatch('[a-f0-9]{64}',value['runner_certificate_sha256'] or '') is not None,'runner_serving_certificate_digest_required')
    return value


class Lifecycle:
    def __init__(self, infra, operation, directory,*,partial_delete=False):
        self.infra,self.operation,self.directory=infra,operation,directory
        self.instance=operation['instance'];self.settings=settings(infra.config)
        self.profile=backend_profile.validate(infra,operation,physical=not partial_delete)
        self.spec=self.profile['specification'];self.namespace=self.spec['namespace']
        self.release=self.namespace
        require(self.instance['backend']=='kubernetes' and self.instance['policy']['minimum_isolation']=='container',
                'kubernetes_instance_scope_differs')
        safe_tree(directory,infra.root)
        self.partial_delete=partial_delete
        if partial_delete:
            validate_deletion_plan(infra,operation,directory);deletion_scope(self)
        else:self.current()

    def command(self,*args,data=None,timeout=60):
        return self.infra.run(['kubectl','--namespace',self.namespace,'--request-timeout=30s',*args],data=data,timeout=timeout)

    def runner_active(self):
        if not (self.directory/'kubernetes.json').exists():return self.infra.active_directory(self.directory)
        generation=self.manifest()['values']['storageGeneration'];epochs.identifier(generation)
        candidate=self.directory/'generations'/compact(generation)
        if candidate.exists():
            require(candidate.is_dir() and not candidate.is_symlink() and epochs.document(candidate/'generation.json')['generation']==generation,
                    'original_runner_generation_directory_changed');return candidate
        require(self.generation(self.directory)==generation,'original_runner_storage_generation_absent');return self.directory

    def runner(self,path,payload=None,*,active=None):
        active=active or self.runner_active()
        body=json.dumps(payload,separators=(',',':')).encode() if payload is not None else b''
        method='POST' if payload is not None else 'GET';timestamp=str(int(time.time()));nonce=str(uuid.uuid4())
        key=(active/'secrets/runner_signing_key').read_bytes().strip()
        signature=hmac.new(key,timestamp.encode()+b'\n'+nonce.encode()+b'\n'+method.encode()+b'\n'+path.encode()+b'\n'+body,hashlib.sha256).hexdigest()
        class NoRedirect(HTTPRedirectHandler):
            def redirect_request(self,*args,**kwargs):raise ValueError('runner redirect forbidden')
        client=build_opener(ProxyHandler({}),NoRedirect(),HTTPSHandler(context=ssl.create_default_context(cafile=self.settings['runner_ca_file'])))
        request=Request(self.settings['runner_url'].rstrip('/')+path,data=body if payload is not None else None,method=method,
            headers={'content-type':'application/json','x-cowork-timestamp':timestamp,'x-cowork-nonce':nonce,'x-cowork-signature':signature})
        with client.open(request,timeout=120) as response:
            certificate=response.fp.raw._sock.getpeercert(binary_form=True)
            require(hashlib.sha256(certificate).hexdigest()==self.settings['runner_certificate_sha256'],
                    'original_runner_serving_certificate_changed')
            raw=response.read(16*1024*1024+1);require(len(raw)<=16*1024*1024,'original_runner_receipt_exceeds_bound')
            return json.loads(raw)

    def scope_request(self,active,previous):
        return {'version':1,'operation_id':self.operation['id'],'instance_id':self.instance['id'],
            'organisation_id':self.instance['organisation_id'],'namespace_uid':self.spec['namespace_uid'],
            'cluster_uid':self.spec['cluster_uid'],'generation':self.generation(active),'expected_quiesce_operation_id':previous}

    def quiesce_runner(self,stage='primary'):
        require(stage in ('primary','rollback','candidate-probe','candidate-stopped','source-probe','source-stopped'),'invalid_original_scope_stage')
        active=self.runner_active()
        actual=self.runner('/v1/kubernetes-scope',active=active)
        prior=actual.get('barrier');previous=prior['original']['operation_id'] if prior else None
        payload=self.scope_request(active,previous)
        marker=self.directory/('runner-quiesce-'+compact(self.operation['id'])+'-'+compact(payload['generation'])+'-'+stage+'.json')
        if marker.exists():
            original=epochs.document(marker,16*1024*1024)
            require(original['operation_sha256']==operation_digest(self.operation),'original_runner_quiesce_operation_changed')
            payload=original['payload']
            if (actual.get('barrier') or {}).get('original')==payload and actual['barrier']['state']=='closed':return actual['barrier']
            require(False,'original_runner_quiesce_ack_unknown_no_replay')
        self.infra.private_write(marker,json.dumps({'operation_sha256':operation_digest(self.operation),'payload':payload,'phase':'dispatched'}).encode())
        response=self.runner('/v1/kubernetes-quiesce',payload,active=active)
        require(response.get('original')==payload and response.get('state')=='closed','actual_runner_quiesce_receipt_differs')
        self.infra.private_write(marker,json.dumps({'operation_sha256':operation_digest(self.operation),'payload':payload,
            'phase':'closed','receipt':response},sort_keys=True).encode());return response

    def activate_runner(self,active,stage='primary'):
        require(stage in ('primary','candidate-probe','source-probe'),'invalid_original_activation_stage')
        actual=self.runner('/v1/kubernetes-scope',active=active);barrier=actual.get('barrier')
        previous=barrier['original']['operation_id'] if barrier else None;payload=self.scope_request(active,previous)
        marker=self.directory/('runner-activate-'+compact(self.operation['id'])+'-'+compact(payload['generation'])+'-'+stage+'.json')
        if marker.exists():
            original=epochs.document(marker,16*1024*1024)
            require(original['operation_sha256']==operation_digest(self.operation),'original_runner_activation_operation_changed')
            require((actual.get('activation') or {}).get('original')==original['payload'],'original_runner_activation_ack_unknown_no_replay');return actual
        self.infra.private_write(marker,json.dumps({'operation_sha256':operation_digest(self.operation),'payload':payload,'phase':'dispatched'}).encode())
        response=self.runner('/v1/kubernetes-activate',payload,active=active)
        require((response.get('activation') or {}).get('original')==payload and response.get('barrier') is None,
                'actual_runner_activation_receipt_differs')
        self.infra.private_write(marker,json.dumps({'operation_sha256':operation_digest(self.operation),'payload':payload,
            'phase':'active','receipt':response},sort_keys=True).encode());return response

    def object(self,kind,name):
        raw=self.command('get',kind,name,'--ignore-not-found=true','-o','json')
        return json.loads(raw) if raw.strip() else None

    def list(self,kind):
        value=json.loads(self.command('get',kind,'-o','json'))
        require(isinstance(value.get('items'),list) and len(value['items'])<=20000,'kubernetes_inventory_exceeds_bound')
        return value['items']

    def current(self):
        cluster=self.object('namespace','kube-system');namespace=self.object('namespace',self.namespace)
        require(cluster and cluster['metadata']['uid']==self.spec['cluster_uid'] and namespace
                and namespace['metadata']['uid']==self.spec['namespace_uid']
                and not namespace['metadata'].get('deletionTimestamp'),'original_kubernetes_cluster_or_namespace_changed')
        labels=namespace['metadata'].get('labels',{})
        require(labels.get('cowork.instance-id')==self.instance['id'] and labels.get('cowork.organisation-id')==self.instance['organisation_id']
                and labels.get('cowork.executor-host-id')==self.spec['host_id'],'original_kubernetes_namespace_assignment_changed')

    def labels(self,generation=None):
        value={'cowork.instance':self.instance['id'],'cowork.organisation':self.instance['organisation_id']}
        if generation:value['cowork.generation']=generation
        return value

    def owned(self,actual,uid,generation=None):
        require(actual and actual['metadata']['uid']==uid and all(actual['metadata'].get('labels',{}).get(k)==v
                for k,v in self.labels(generation).items()),'original_kubernetes_resource_uid_changed')

    def create(self,value):
        self.current()
        actual=json.loads(self.command('create','-f','-','-o','json',data=json.dumps(value).encode()))
        self.owned(actual,actual['metadata']['uid'])
        return actual

    def delete(self,kind,name,uid):
        self.current();actual=self.object(kind,name)
        physical_marker=self.directory/('kube-physical-stop-'+uid+'.json')
        terminated=physical_marker.exists()
        if actual is None:
            require(kind!='pod' or terminated,'original_pod_absent_without_physical_terminal_evidence');return
        self.owned(actual,uid)
        if kind=='pod':
            require(actual['metadata'].get('finalizers')==['cowork.opencowork.dev/physical-stop'] or terminated,
                    'original_pod_physical_stop_finalizer_changed')
        api={'pod':'/api/v1','pvc':'/api/v1','configmap':'/api/v1','secret':'/api/v1',
             'deployment':'/apis/apps/v1','service':'/api/v1','networkpolicy':'/apis/networking.k8s.io/v1',
             'resourcequota':'/api/v1','serviceaccount':'/api/v1','role':'/apis/rbac.authorization.k8s.io/v1',
             'rolebinding':'/apis/rbac.authorization.k8s.io/v1','poddisruptionbudget':'/apis/policy/v1',
             'clusterrole':'/apis/rbac.authorization.k8s.io/v1','clusterrolebinding':'/apis/rbac.authorization.k8s.io/v1'}
        plural={'pvc':'persistentvolumeclaims','networkpolicy':'networkpolicies','resourcequota':'resourcequotas',
                'poddisruptionbudget':'poddisruptionbudgets'}.get(kind,kind+'s')
        uri=api[kind]+('' if kind in ('clusterrole','clusterrolebinding') else '/namespaces/'+self.namespace)+'/'+plural+'/'+name
        self.command('delete','--raw',uri,'-f','-',data=json.dumps({'apiVersion':'v1','kind':'DeleteOptions',
            'gracePeriodSeconds':30 if kind=='pod' else 0,'preconditions':{'uid':uid},'propagationPolicy':'Foreground'}).encode())
        for _ in range(360):
            actual=self.object(kind,name)
            if actual is None:
                require(kind!='pod' or terminated,'original_pod_absent_without_physical_terminal_evidence');return
            self.owned(actual,uid);time.sleep(.25)
            if kind=='pod':
                statuses=actual.get('status',{}).get('containerStatuses',[])
                if (not terminated and actual.get('status',{}).get('phase') in ('Succeeded','Failed') and statuses
                    and all(v.get('state',{}).get('terminated',{}).get('finishedAt') for v in statuses)):
                    self.infra.private_write(physical_marker,json.dumps({'pod_uid':uid,'namespace_uid':self.spec['namespace_uid'],
                        'cluster_uid':self.spec['cluster_uid'],'container_statuses':statuses},sort_keys=True).encode());terminated=True
                if terminated and actual['metadata'].get('finalizers')==['cowork.opencowork.dev/physical-stop']:
                    patch=[{'op':'test','path':'/metadata/uid','value':uid},
                        {'op':'test','path':'/metadata/resourceVersion','value':actual['metadata']['resourceVersion']},
                        {'op':'test','path':'/metadata/finalizers','value':['cowork.opencowork.dev/physical-stop']},
                        {'op':'remove','path':'/metadata/finalizers'}]
                    self.command('patch','pod',name,'--type=json','-p',json.dumps(patch))
        require(False,'original_kubernetes_delete_absence_unproven')

    def generation(self,active):
        value=self.infra.resource_names(self.instance,active)['generation'] or epochs.read(self.instance,active)
        if value is None and (self.directory/'kubernetes.json').exists():value=self.manifest()['values']['storageGeneration']
        return epochs.identifier(value) if value is not None else None

    def images(self,active):
        return epochs.document(active/'images.json',1048576)

    def workspace_name(self,generation,workspace):
        epochs.identifier(generation);epochs.identifier(workspace)
        return 'ws-'+hashlib.sha256((generation+':'+workspace).encode()).hexdigest()[:48]

    def workspace_inventory(self,active):
        generation=self.generation(active);rows=[]
        for value in self.list('pvc'):
            labels=value['metadata'].get('labels',{})
            if labels.get('cowork.managed')=='runner-journal':continue
            require(all(labels.get(k)==v for k,v in self.labels().items()),'foreign_pvc_in_bound_namespace')
            if labels.get('cowork.generation')!=generation:continue
            workspace=labels.get('cowork.workspace');epochs.identifier(workspace)
            require(value['metadata']['name']==self.workspace_name(generation,workspace),'unrecognised_original_workspace_name')
            uid=value['metadata']['uid'];self.owned(value,uid,generation)
            receipt=self.object('configmap','receipt-'+value['metadata']['name'])
            require(receipt and receipt.get('immutable') is True,'original_workspace_create_receipt_missing')
            self.owned(receipt,receipt['metadata']['uid'],generation)
            original=json.loads(receipt['data']['receipt'])
            require(original=={'name':value['metadata']['name'],'uid':uid},'original_workspace_create_receipt_differs')
            pv=value.get('spec',{}).get('volumeName');require(isinstance(pv,str) and pv,'workspace_pv_not_bound')
            physical=self.object('pv',pv)
            require(physical and physical['spec']['claimRef']['uid']==uid
                    and physical['spec']['claimRef']['namespace']==self.namespace,'workspace_original_pv_claim_changed')
            rows.append({'workspace_id':workspace,'generation':generation,'name':value['metadata']['name'],'uid':uid,
                'pv_name':pv,'pv_uid':physical['metadata']['uid'],'receipt_uid':receipt['metadata']['uid']})
        rows.sort(key=lambda row:row['workspace_id'])
        require(len(rows)<=self.settings['max_workspace_archives'] and len({r['workspace_id'] for r in rows})==len(rows),
                'workspace_inventory_exceeds_bound')
        return rows

    def create_workspace(self,generation,workspace,operation_id):
        name=self.workspace_name(generation,workspace)
        require(self.object('pvc',name) is None and self.object('configmap','receipt-'+name) is None,'restored_workspace_creation_already_dispatched')
        labels={**self.labels(generation),'cowork.managed':'native-runner','cowork.workspace':workspace}
        actual=self.create({'apiVersion':'v1','kind':'PersistentVolumeClaim','metadata':{'name':name,'labels':labels},
            'spec':{'accessModes':['ReadWriteOnce'],'storageClassName':self.settings['storage_class'],
                    'resources':{'requests':{'storage':str(min(self.settings['workspace_claim_gib'],self.instance['policy']['limits']['storage_gib']))+'Gi'}}}})
        uid=actual['metadata']['uid']
        receipt=self.create({'apiVersion':'v1','kind':'ConfigMap','immutable':True,'metadata':{'name':'receipt-'+name,
            'labels':labels,'annotations':{'cowork.original-operation':operation_id}},'data':{'receipt':json.dumps({'name':name,'uid':uid})}})
        return {'workspace_id':workspace,'generation':generation,'name':name,'uid':uid,'receipt_uid':receipt['metadata']['uid']}

    def helper(self,original,*,restore=False,image=None,command=None):
        self.current()
        generation=original['generation'] if original else self.generation(self.infra.active_directory(self.directory))
        if original:self.owned(self.object('pvc',original['name']),original['uid'],original['generation'])
        else:require(command==['/usr/local/bin/cowork-server','--execution-admission-contract'] and not restore,
                     'credential_free_image_probe_only')
        name='helper-'+uuid.uuid4().hex;labels={**self.labels(generation),'cowork.managed':'native-runner',
            'cowork.network':'none','cowork.original-operation':self.operation['id']}
        value={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'labels':labels,'finalizers':['cowork.opencowork.dev/physical-stop']},'spec':{
            'runtimeClassName':self.settings['runtime_class'],'nodeSelector':self.settings['node_selector'],
            'restartPolicy':'Never','automountServiceAccountToken':False,'activeDeadlineSeconds':21630,
            'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
            'containers':[{'name':'sandbox','image':image or self.images(self.infra.active_directory(self.directory))['sandbox_core'],
                'command':command or ['python3','-c','import time;time.sleep(22000)'],
                'securityContext':{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},
                'resources':{'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'500m','memory':'256Mi'}},
                'volumeMounts':[{'name':'workspace','mountPath':'/workspace','readOnly':not restore},{'name':'tmp','mountPath':'/tmp'}]}],
            'volumes':[{'name':'workspace','persistentVolumeClaim':{'claimName':original['name'] if original else '', 'readOnly':not restore}},
                       {'name':'tmp','emptyDir':{'medium':'Memory','sizeLimit':'128Mi'}}]}}
        if original is None:
            value['spec']['containers'][0]['resources']={'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'100m','memory':'128Mi'}}
            value['spec']['volumes']=[v for v in value['spec']['volumes'] if v['name']!='workspace']
            value['spec']['containers'][0]['volumeMounts']=[v for v in value['spec']['containers'][0]['volumeMounts'] if v['name']!='workspace']
        marker=self.directory/('kube-helper-'+name.removeprefix('helper-')+'.json')
        self.infra.private_write(marker,json.dumps({'phase':'dispatched','operation_id':self.operation['id'],
            'manifest':value,'original_workspace':original},sort_keys=True).encode())
        actual=self.create(value);uid=actual['metadata']['uid']
        self.infra.private_write(marker,json.dumps({'phase':'created','operation_id':self.operation['id'],'name':name,'uid':uid,
            'original_workspace':original},sort_keys=True).encode())
        for _ in range(240):
            actual=self.object('pod',name);self.owned(actual,uid,generation)
            if actual.get('status',{}).get('phase')=='Running' or command and actual.get('status',{}).get('phase')=='Succeeded':return name,uid,marker
            require(actual.get('status',{}).get('phase') not in ('Failed','Succeeded'),'kubernetes_helper_terminated')
            time.sleep(.25)
        require(False,'kubernetes_helper_start_unproven')

    def workspace_archive(self,original,target,*,restore):
        self.owned(self.object('pvc',original['name']),original['uid'],original['generation'])
        if 'pv_uid' in original:
            pv=self.object('pv',original['pv_name'])
            require(pv and pv['metadata']['uid']==original['pv_uid'] and pv['spec']['claimRef']['uid']==original['uid'],
                    'original_workspace_physical_volume_changed')
        name,uid,marker=self.helper(original,restore=restore)
        try:
            command=['kubectl','--namespace',self.namespace,'exec']+(['-i'] if restore else [])+[name,'--container=sandbox','--',
                'tar','--numeric-owner','-C','/workspace']+(['-xpf','-','--no-same-owner'] if restore else ['-cf','-','.'])
            command=['kubectl','--kubeconfig',self.infra.config['kubeconfig_file'],'--context',self.infra.config['kubernetes_context'],*command[1:]]
            # Bounded direct stream avoids loading a multi-GiB workspace in RAM.
            if restore:
                require(target.is_file() and not target.is_symlink() and target.stat().st_size<=self.settings['max_workspace_archive_bytes'],
                        'restore_workspace_archive_exceeds_bound')
                with target.open('rb') as source:
                    child=subprocess.Popen(command,stdin=source,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                    try:require(child.wait(timeout=21600)==0,'original_workspace_restore_rpc_failed')
                    finally:
                        if child.poll() is None:child.kill();child.wait()
            else:
                child=subprocess.Popen(command,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
                deadline=threading.Timer(21600,lambda:child.kill() if child.poll() is None else None);deadline.daemon=True;deadline.start()
                try:
                    total=0
                    with target.open('xb') as output:
                        os.chmod(target,0o600)
                        while part:=child.stdout.read(1024*1024):
                            total+=len(part);require(total<=self.settings['max_workspace_archive_bytes'],'workspace_archive_exceeds_bound');output.write(part)
                        output.flush();os.fsync(output.fileno())
                    require(child.wait(timeout=60)==0,'original_workspace_capture_rpc_failed')
                finally:
                    deadline.cancel()
                    if child.poll() is None:child.kill();child.wait()
        finally:
            self.delete('pod',name,uid)
            self.infra.private_write(marker,json.dumps({'phase':'closed','operation_id':self.operation['id'],'name':name,'uid':uid}).encode())

    def manifest(self):
        return epochs.document(self.directory/'kubernetes.json',16*1024*1024)

    def quiescent(self):
        self.current()
        manifest=self.manifest()
        for name,original in manifest['deployments'].items():
            actual=self.object('deployment',name);self.owned(actual,original['uid'])
            target=1 if name=='runner' else 0
            require(actual['spec']['replicas']==target and actual.get('status',{}).get('replicas',0)==target
                    and actual.get('status',{}).get('observedGeneration',0)>=actual['metadata']['generation'],
                    'original_kubernetes_deployment_not_stopped')
        pods=self.list('pod')
        require(len(pods)==1 and pods[0]['metadata'].get('labels',{}).get('cowork.component')=='runner',
                'bound_namespace_workpods_not_quiescent')
        runner=pods[0];self.owned(runner,runner['metadata']['uid'])
        source=manifest['deployments']['runner']['uid']
        owner=next((o for o in runner['metadata'].get('ownerReferences',[]) if o.get('controller') and o['kind']=='ReplicaSet'),None)
        require(owner is not None,'original_runner_control_parent_absent')
        rs=self.object('replicaset',owner['name'])
        require(rs and rs['metadata']['uid']==owner['uid'] and any(o.get('controller') and o['kind']=='Deployment'
            and o['uid']==source for o in rs['metadata'].get('ownerReferences',[])),'original_runner_control_lineage_changed')
        current=self.runner('/v1/kubernetes-scope')
        require(current['generation']==manifest['values']['storageGeneration'] and current['namespace_uid']==self.spec['namespace_uid']
                and current['cluster_uid']==self.spec['cluster_uid'] and (current.get('barrier') or {}).get('state')=='closed',
                'original_runner_control_scope_is_not_physically_quiesced')

    def stop(self,stage='primary'):
        require(stage in ('primary','candidate-probe','source-probe'),'invalid_original_stop_stage')
        manifest=self.manifest();self.current()
        self.quiesce_runner(stage)
        originals=[]
        for pod in self.list('pod'):
            self.owned(pod,pod['metadata']['uid'])
            require(pod['metadata'].get('finalizers')==['cowork.opencowork.dev/physical-stop'],
                    'legacy_pod_without_original_physical_stop_fence_requires_review')
            if pod['metadata'].get('labels',{}).get('cowork.component')!='runner':
                originals.append({'name':pod['metadata']['name'],'uid':pod['metadata']['uid']})
        stop_marker=self.directory/('kube-stop-'+compact(self.operation['id'])+'-'+stage+'.json')
        if stop_marker.exists():
            prior=epochs.document(stop_marker,16*1024*1024)
            require(prior['operation_sha256']==operation_digest(self.operation),'original_stop_operation_changed')
            originals=prior['pods']
        else:self.infra.private_write(stop_marker,json.dumps({'operation_sha256':operation_digest(self.operation),'pods':originals,
            'deployments':manifest['deployments'],'namespace_uid':self.spec['namespace_uid']},sort_keys=True).encode())
        for name,original in manifest['deployments'].items():
            if name=='runner':continue
            actual=self.object('deployment',name);self.owned(actual,original['uid'])
            patch=[{'op':'test','path':'/metadata/uid','value':original['uid']},
                {'op':'test','path':'/metadata/resourceVersion','value':actual['metadata']['resourceVersion']},
                {'op':'replace','path':'/spec/replicas','value':0}]
            self.command('patch','deployment',name,'--type=json','-p',json.dumps(patch))
        # ReplicaSets are deleted by deployment scale. Standalone original runner
        # jobs are stopped by their frozen UID, never by namespace --all.
        for original in originals:self.delete('pod',original['name'],original['uid'])
        for _ in range(120):
            if all(p['metadata'].get('labels',{}).get('cowork.component')=='runner' for p in self.list('pod')):
                self.quiescent();return
            time.sleep(.5)
        require(False,'original_kubernetes_stop_readback_unproven')

    def promote(self,directory,generation):
        self.quiescent()
        epochs.identifier(generation)
        target=directory/'generations'/compact(generation)
        require(target.is_dir() and epochs.document(target/'generation.json')['generation']==generation,'original_candidate_generation_absent')
        self.infra.private_write(directory/'active.json',json.dumps({'generation':generation}).encode())

    def values(self,active,replicas):
        import organisation_mfa,resource_budget,instance_gateway
        from provisioner import egress_config
        images=self.images(active);values=self.infra.ensure_secrets(self.instance,active)
        epoch=epochs.read(self.instance,active);require(epoch is not None,'kubernetes_execution_epoch_required')
        resources=self.infra.resource_names(self.instance,active)
        return {'instanceId':self.instance['id'],'organisationId':self.instance['organisation_id'],'image':images['server'],
            'images':images,'replicas':replicas,'runnerUrl':'https://runner.'+self.namespace+'.svc:8090',
            'runnerCA':Path(self.settings['runner_ca_file']).read_text(), 'runnerTls':self.settings['runner_tls'],
            'executionEpoch':epoch,'storageGeneration':self.generation(active),'clusterUid':self.spec['cluster_uid'],
            'namespaceUid':self.spec['namespace_uid'],'runtimeClass':self.settings['runtime_class'],'storageClass':self.settings['storage_class'],
            'executorHostId':self.spec['host_id'],'egressConfiguration':egress_config(self.instance),
            'nodeNames':self.settings['node_names'],'nodeUids':self.settings['node_uids'],'workspaceClaimGiB':self.settings['workspace_claim_gib'],
            'desktopProfile':self.settings['desktop_profile'],
            'gatewayEnvironment':instance_gateway.environment(self.infra.config,self.instance),
            'nodeSelector':self.settings['node_selector'],'serviceResources':resource_budget.service_caps(self.infra.config),
            'secrets':values,'s3Endpoint':self.infra.config['s3_endpoint'],'s3Bucket':resources['bucket'],
            'resources':self.instance['policy']['limits'],'serviceCidrs':self.settings['service_cidrs'],
            'routingNamespace':self.settings['routing_namespace'],'organisationMfa':organisation_mfa.configuration(self.instance),
            'fleetManagementUrl':self.infra.config['fleet_management_url'],'originalOperationId':self.operation['id']}

    def deploy(self,active,replicas,stage='primary',*,maintenance=False):
        require(stage in ('primary','rollback','candidate-probe','candidate-stopped','source-probe','source-stopped'),'invalid_original_helm_dispatch_stage')
        require(type(maintenance) is bool,'invalid_original_maintenance_mode')
        self.current();values=self.values(active,replicas);values['maintenanceMode']=maintenance
        import resource_budget,host_capacity_attestation
        budget=resource_budget.instance_budget(self.infra.config,self.instance)
        allocation=host_capacity_attestation.operation_allocation(self.operation)
        require(allocation is not None and allocation['plan']['services']==budget['services']
                and all(budget['sandbox'][k]<=allocation['plan']['sandbox'][k] for k in budget['sandbox']),
                'original_kubernetes_host_allocation_required')
        self.infra.private_write(self.directory/'resource-budget.json',json.dumps(budget,sort_keys=True).encode())
        path=self.directory/'helm-values.json';self.infra.private_write(path,json.dumps(values).encode())
        chart=preflight_release(self,values,stage)
        rendered=self.infra.run(['helm','template',self.release,chart,'--namespace',self.namespace,'--values',str(path)])
        digest=hashlib.sha256(rendered).hexdigest()
        require(digest==self._preflight_helm_sha256,'original_rendered_helm_source_changed')
        if (self.directory/'kubernetes.json').exists():
            previous=self.manifest()
            changed=(previous['values']['storageGeneration']!=values['storageGeneration']
                or previous['values']['images']['runner']!=values['images']['runner']
                or previous['values']['secrets']['runner_signing_key']!=values['secrets']['runner_signing_key'])
            if changed:
                # Recreate an originally quiesced controller, preserving real
                # Pod terminal evidence before changing its secret/generation.
                self.quiescent()
                current=self.object('deployment','runner');uid=previous['deployments']['runner']['uid'];self.owned(current,uid)
                pods=self.list('pod');require(len(pods)==1 and pods[0]['metadata'].get('labels',{}).get('cowork.component')=='runner',
                    'original_runner_reconfiguration_not_quiescent')
                reconfigure=self.directory/('runner-reconfigure-'+compact(self.operation['id'])+'-'+stage+'.json')
                require(not reconfigure.exists(),'original_runner_reconfiguration_unsettled_no_replay')
                self.infra.private_write(reconfigure,json.dumps({'operation_sha256':operation_digest(self.operation),
                    'source_manifest_sha256':sha(previous),'values_sha256':sha(values),'pod_uid':pods[0]['metadata']['uid'],
                    'pod_name':pods[0]['metadata']['name'],'deployment_uid':uid,'phase':'dispatched'},sort_keys=True).encode())
                patch=[{'op':'test','path':'/metadata/uid','value':uid},{'op':'test','path':'/metadata/resourceVersion','value':current['metadata']['resourceVersion']},
                    {'op':'replace','path':'/spec/replicas','value':0}]
                self.command('patch','deployment','runner','--type=json','-p',json.dumps(patch))
                self.delete('pod',pods[0]['metadata']['name'],pods[0]['metadata']['uid'])
                require(not self.list('pod'),'original_runner_reconfiguration_absence_unproven')
        marker=self.directory/('helm-'+compact(self.operation['id'])+'-'+stage+'.json')
        require(not marker.exists(),'original_helm_dispatch_exists_requires_reconciliation')
        self.infra.private_write(marker,json.dumps({'phase':'dispatched','manifest_sha256':digest,'operation_id':self.operation['id'],
            'namespace_uid':self.spec['namespace_uid'],'values_sha256':sha(values)}).encode())
        self.infra.run(['helm','upgrade','--install',self.release,chart,'--namespace',self.namespace,'--values',str(path),
            '--wait','--timeout','5m','--history-max','20'],timeout=330)
        release=json.loads(self.infra.run(['helm','status',self.release,'--namespace',self.namespace,'-o','json']))
        require(release['info']['status']=='deployed','original_helm_release_not_deployed')
        require(release['version']==self._preflight_release_revision+1,'original_helm_release_revision_changed')
        secret=self.object('secret','sh.helm.release.v1.'+self.release+'.v'+str(release['version']))
        require(secret and secret['metadata'].get('labels',{}).get('owner')=='helm','original_helm_release_receipt_missing')
        deployments={}
        for name in ('api','worker','runner','egress-proxy'):
            actual=self.object('deployment',name);self.owned(actual,actual['metadata']['uid'])
            original_uid=self._preflight_resources.get(('deployment',name))
            require(original_uid is None or actual['metadata']['uid']==original_uid,'original_deployment_uid_changed_during_helm')
            require(actual['metadata'].get('annotations',{}).get('cowork.original-operation')==self.operation['id'],
                    'deployment_original_operation_changed')
            deployments[name]={'uid':actual['metadata']['uid'],'generation':actual['metadata']['generation']}
        manifest={'version':1,'namespace_uid':self.spec['namespace_uid'],'cluster_uid':self.spec['cluster_uid'],
            'release_uid':secret['metadata']['uid'],'release_revision':release['version'],'helm_manifest_sha256':digest,
            'values_sha256':sha(values),'values':values,'deployments':deployments,'resources':self.resource_inventory()}
        self.infra.private_write(self.directory/'kubernetes.json',json.dumps(manifest,sort_keys=True).encode())
        self.infra.private_write(marker,json.dumps({'phase':'acknowledged','operation_id':self.operation['id'],'manifest':manifest},sort_keys=True).encode())
        if replicas==0:self.quiesce_runner(stage);self.quiescent()
        return manifest

    def resource_inventory(self):
        result=[]
        for kind in ('deployment','service','secret','configmap','serviceaccount','role','rolebinding','networkpolicy','resourcequota','poddisruptionbudget','pvc'):
            for value in self.list(kind):
                # TLS material is pre-enrolled explicitly by its original UID;
                # Helm bookkeeping is bound separately to actual release UID.
                labels=value['metadata'].get('labels',{})
                if labels.get('owner')=='helm':continue
                original_uid=getattr(self,'_preflight_resources',{}).get((kind,value['metadata']['name']))
                require(original_uid is None or value['metadata']['uid']==original_uid,'original_helm_resource_uid_changed_during_dispatch')
                if kind=='serviceaccount' and value['metadata']['name']=='default':
                    originals=list(self.settings.get('bootstrap_resources',[]))
                    if (self.directory/'kubernetes.json').exists():originals+=self.manifest()['resources']
                    require(any(r['kind']=='serviceaccount' and r['name']=='default' and r['uid']==value['metadata']['uid'] for r in originals),
                            'original_default_serviceaccount_uid_changed')
                    result.append({'kind':kind,'name':'default','uid':value['metadata']['uid']});continue
                if kind=='secret' and value['metadata']['name']==self.settings['runner_tls']['secret_name']:
                    require(value['metadata']['uid']==self.settings['runner_tls']['certificate_uid'],'original_runner_tls_secret_changed');continue
                self.owned(value,value['metadata']['uid'])
                result.append({'kind':kind,'name':value['metadata']['name'],'uid':value['metadata']['uid']})
        for kind in ('clusterrole','clusterrolebinding'):
            name='cowork-native-runner-'+compact(self.instance['id'])
            value=self.object(kind,name)
            if value is not None:
                original_uid=getattr(self,'_preflight_resources',{}).get((kind,name))
                require(original_uid is None or value['metadata']['uid']==original_uid,'original_helm_cluster_resource_uid_changed_during_dispatch')
                self.owned(value,value['metadata']['uid'])
                result.append({'kind':kind,'name':name,'uid':value['metadata']['uid']})
        return sorted(result,key=lambda r:(r['kind'],r['name']))

    def runtime_probe(self,active):
        image=self.images(active)['server'];allowed=self.infra.config['execution_admission']['server_images']
        require(image in allowed,'kubernetes_server_image_not_reviewed')
        # Actual isolated image invocation, no namespace API/DB credentials.
        name,uid,marker=self.helper(None,image=image,command=['/usr/local/bin/cowork-server','--execution-admission-contract'])
        try:
            for _ in range(120):
                actual=self.object('pod',name);self.owned(actual,uid)
                if actual.get('status',{}).get('phase')=='Succeeded':break
                require(actual.get('status',{}).get('phase')!='Failed','kubernetes_image_probe_failed');time.sleep(.25)
            value=json.loads(self.command('logs',name,'--container=sandbox'))
            require(value==CONTRACT and all(type(value[k]) is type(v) for k,v in CONTRACT.items()),'kubernetes_server_image_contract_differs')
        finally:
            self.delete('pod',name,uid)
        return {'version':1,'server_image':image,'manifest_sha256':sha(self.manifest())}

    def startup_probe(self,active,stage):
        require(stage in ('candidate-probe','source-probe'),'invalid_original_startup_probe_stage')
        self.deploy(active,1,stage=stage,maintenance=True)
        self.activate_runner(active,stage)
        manifest=self.manifest();observed=[]
        try:
            for _ in range(3):
                rows=[]
                for component in ('api','worker'):
                    selected=[p for p in self.list('pod') if p['metadata'].get('labels',{}).get('cowork.component')==component]
                    require(len(selected)==1,'original_candidate_service_pod_unavailable')
                    pod=selected[0];self.owned(pod,pod['metadata']['uid'])
                    parent=next((o for o in pod['metadata'].get('ownerReferences',[]) if o.get('controller') and o['kind']=='ReplicaSet'),None)
                    replica=self.object('replicaset',parent['name']) if parent else None
                    require(replica and replica['metadata']['uid']==parent['uid'] and any(o.get('controller') and o['kind']=='Deployment'
                        and o['uid']==manifest['deployments'][component]['uid'] for o in replica['metadata'].get('ownerReferences',[])),
                        'original_candidate_service_lineage_changed')
                    statuses=pod.get('status',{}).get('containerStatuses',[])
                    require(pod.get('status',{}).get('phase')=='Running' and len(statuses)==1 and statuses[0].get('ready') is True
                        and statuses[0].get('restartCount')==0 and statuses[0].get('containerID'),'original_candidate_service_not_stable')
                    if component=='api':
                        actual=json.loads(self.command('exec',pod['metadata']['name'],'--container=api','--','curl','--fail','--silent',
                            '--max-time','10','http://127.0.0.1:8080/readyz'))
                        require(actual=={'status':'ready'},'original_candidate_database_or_api_not_ready')
                    rows.append({'component':component,'pod_uid':pod['metadata']['uid'],'container_id':statuses[0]['containerID']})
                if observed:require(observed==rows,'original_candidate_service_changed_during_probe')
                observed=rows;time.sleep(2)
            marker=self.directory/('kube-startup-probe-'+compact(self.operation['id'])+'-'+stage+'.json')
            require(not marker.exists(),'original_candidate_probe_dispatch_exists_no_replay')
            run,attempt,lease=[str(uuid.uuid4()) for _ in range(3)];text='cowork-kubernetes-probe-'+run
            spec={'schema_version':2,'run_id':run,'image':'core','network':'none','environment':{},'stdin_base64':None,
                'argv':['python3','-c','print('+repr(text)+')'],'limits':{'cpu_nanos':100000000,'memory_bytes':134217728,
                    'pids':64,'timeout_seconds':20,'tmpfs_bytes':4194304,'output_bytes':1024},
                '_execution':{'version':1,'attempt_id':attempt,'lease_token':lease}}
            self.infra.private_write(marker,json.dumps({'operation_sha256':operation_digest(self.operation),'generation':self.generation(active),
                'spec':spec,'phase':'dispatched'},sort_keys=True).encode())
            result=self.runner('/v1/jobs',spec,active=active)
            require(result.get('run_id')==run and result.get('exit_code')==0 and result.get('timed_out') is False
                and result.get('stdout','').strip()==text,'original_candidate_native_sandbox_probe_failed')
            proof={'operation_sha256':operation_digest(self.operation),'generation':self.generation(active),
                'phase':'completed','services':observed,'spec_sha256':sha(spec),'result_sha256':sha(result)}
            self.infra.private_write(marker,json.dumps(proof,sort_keys=True).encode());return proof
        finally:self.stop(stage)

    def receipt(self,result):
        expected=epochs.binding(self.operation)
        if expected:
            active=self.infra.active_directory(self.directory);epoch=epochs.read(self.instance,active)
            require(epoch in (expected['source_epoch'],expected['candidate_epoch']),'original_kubernetes_active_epoch_changed')
            execution={'binding':expected,'active_epoch':epoch}
            if self.operation.get('execution_runtime_required') and self.operation['action']!='delete':
                execution['runtime']=self.runtime_probe(active)
            result={**result,'execution':execution}
        return result


def execute(infra,operation,directory,*,enrollment_resume=False):
    action=operation['action']
    partial=enrollment_resume and action=='delete' and deletion_marker(directory,operation).exists()
    kube=Lifecycle(infra,operation,directory,partial_delete=partial);active=infra.active_directory(directory)
    marker=directory/('kubernetes-operation-'+compact(operation['id'])+'.json')
    original=operation_digest(operation)
    if marker.exists():
        prior=epochs.document(marker,16*1024*1024)
        require(prior['original_sha256']==original,'original_kubernetes_operation_changed')
        if prior['phase']=='completed':return prior['receipt']
        require(enrollment_resume and action in ('enroll_execution','stop','suspend','delete') and prior['phase'] in ('prepared','dispatched'),
                'original_kubernetes_operation_unsettled_no_replay')
    else:infra.private_write(marker,json.dumps({'version':1,'original_sha256':original,'phase':'prepared','operation_id':operation['id']}).encode())
    def checkpoint(phase,receipt=None):
        infra.private_write(marker,json.dumps({'version':1,'original_sha256':original,'phase':phase,'operation_id':operation['id'],
                                              **({'receipt':receipt} if receipt else {})},sort_keys=True).encode())
    checkpoint('dispatched')
    result={'receipt_id':operation['id'],'instance_id':kube.instance['id']}
    expected=epochs.binding(operation)
    if action=='provision':
        require(expected is not None and not (directory/'kubernetes.json').exists(),'fresh_kubernetes_execution_enrollment_required')
        epochs.write(infra,kube.instance,active,expected['source_epoch'])
        selected=kube.instance.get('provisioning_release')
        if selected:
            catalog=ReleaseCatalog(infra.config,infra.private_write);require(catalog.enabled(selected),'release_withdrawn')
            images=validate_release(catalog.read(catalog.root/(selected+'.json')),selected)['images'];result['release']=selected
        else:images=infra.config['images']
        infra.private_write(active/'images.json',json.dumps(images,sort_keys=True).encode())
        values=infra.ensure_secrets(kube.instance,active);infra.ensure_database(kube.instance,values,active);infra.ensure_bucket(kube.instance,values,active)
        kube.deploy(active,1)
        kube.activate_runner(active)
    elif action in ('start','resume'):
        require(expected and epochs.read(kube.instance,active)==expected['source_epoch'],'original_kubernetes_execution_epoch_changed')
        kube.deploy(active,1)
        kube.activate_runner(active)
    elif action in ('stop','suspend'):
        kube.stop();result['instance_state']='stopped'
        import execution_recovery
        recovery=execution_recovery.binding(operation)
        if recovery is not None:
            from instance_deletion import InstanceDeletion
            generations=InstanceDeletion(infra).generations(kube.instance,directory)
            environment=pg_environment(Path(infra.config['database_admin_url_file']).read_text().strip(),infra.config)
            for resources in generations:
                count=infra.run(['psql','-X','-qAt','-v','ON_ERROR_STOP=1','-c',
                    "SELECT count(*) FROM pg_stat_activity WHERE datname='"+resources['database']+"';"],env=environment)
                require(count.strip()==b'0','retained_kubernetes_database_still_active')
            physical=[epochs.document(p,1048576) for p in sorted(directory.glob('kube-physical-stop-*.json'))]
            result['execution_recovery']={'binding':recovery,'stopped':True,'inventory_sha256':sha({
                'binding':recovery,'manifest':kube.manifest(),'generations':generations,'physical':physical}),
                'verified_generations':len(generations)}
    elif action=='backup':result.update(Archives(kube).backup(operation,directory,active))
    elif action=='restore':result.update(Archives(kube).restore(operation,directory,active));kube.deploy(infra.active_directory(directory),0)
    elif action in ('upgrade','rollback'):
        result.update(update(kube,operation,directory,active))
    elif action=='enroll_execution':result.update(enroll(kube,operation,directory,active))
    elif action=='delete':result.update(delete(kube,operation,directory))
    else:require(False,'unsupported_kubernetes_lifecycle_action')
    if action!='delete':result=kube.receipt(result)
    elif expected:result['execution']={'binding':expected,'active_epoch':expected['source_epoch']}
    checkpoint('completed',result)
    return result


def enroll(kube,operation,directory,active):
    import execution_enrollment as preflight
    expected=epochs.binding(operation)
    require(expected and kube.instance.get('execution_required') is False and kube.instance['state']=='stopped'
            and operation['arguments'].get('acknowledge_execution_enrollment') is True,
            'explicit_kubernetes_enrollment_required')
    kube.quiescent();original=kube.manifest();archives=Archives(kube)
    env=pg_environment((active/'secrets/database_url').read_text().strip(),kube.infra.config)
    counts=preflight.counts(kube.infra,env)
    require(counts['unfinished_runs']==counts['preservable_server_runs'] and not any(v for k,v in counts.items()
        if k not in ('unfinished_runs','preservable_server_runs','legacy_assignments_requiring_review','enabled_schedules')),
        'kubernetes_enrollment_work_requires_reconciliation')
    backup=archives.backup(operation,directory,active);kube.quiescent()
    require(kube.manifest()==original,'original_kubernetes_enrollment_manifest_changed')
    epochs.write(kube.infra,kube.instance,active,expected['source_epoch'])
    kube.deploy(active,0)
    inventory=kube.workspace_inventory(active)
    return {**backup,'instance_state':'stopped','execution_enrollment':{'binding':expected,'outcome':'enrolled',
        'source_manifest_sha256':sha(original),'inventory_sha256':sha(inventory),'verified_generations':1}}


def update(kube,operation,directory,active):
    from instance_updates import PROMOTE_SCHEDULES_SQL
    kube.quiescent();archives=Archives(kube);source=kube.images(active);source_id=image_set_id(source)
    arguments=operation['arguments'];target=arguments['release'];catalog=ReleaseCatalog(kube.infra.config,kube.infra.private_write)
    require(arguments.get('source_release')==source_id and catalog.enabled(target),'kubernetes_update_original_release_changed')
    release=validate_release(catalog.read(catalog.root/(target+'.json')),target)
    canary=arguments.get('qualification_only',False);require(type(canary) is bool,'invalid_kubernetes_release_qualification')
    evidence=directory/('kube-qualified-'+source_id+'-'+target+'.json')
    if operation['action']=='upgrade' and not canary:
        proof=epochs.document(evidence)
        require(proof['backend_profile_sha256']==operation['backend_profile']['specification_sha256']
                and proof['source_release']==source_id and proof['release']==target and proof['rollback_verified'] is True,
                'original_kubernetes_release_canary_required')
    if operation['action']=='rollback':
        promotion=epochs.document(directory/('kubernetes-operation-'+compact(arguments['rollback_operation_id'])+'.json'),16*1024*1024)
        require(promotion['phase']=='completed' and promotion['receipt']['release']==source_id
                and promotion['receipt']['source_release']==target and promotion['receipt']['generation']==kube.generation(active)
                and operation['backup']['id']==promotion['receipt']['backup_id'],
                'original_kubernetes_rollback_promotion_changed')
    backup=archives.backup(operation,directory,active)
    if operation['action']=='rollback':restoration={**operation,'arguments':{'backup_id':operation['backup']['id']}}
    else:restoration={**operation,'arguments':{'backup_id':operation['id']},'backup':{'id':operation['id'],
        'instance_id':kube.instance['id'],'organisation_id':kube.instance['organisation_id'],
        'sha256':backup['backup_sha256'],'size_bytes':backup['backup_bytes']}}
    pointer=(directory/'active.json').read_bytes() if (directory/'active.json').exists() else None
    archives.restore(restoration,directory,active,images=release['images'],activate=True,upgrade=operation['action']=='upgrade',
        verification_images=release['images'] if operation['action']=='rollback' else None)
    candidate=kube.infra.active_directory(directory);candidate_probe=kube.startup_probe(candidate,'candidate-probe')
    kube.deploy(candidate,0,stage='candidate-stopped');kube.runtime_probe(candidate);kube.quiescent()
    receipt={**backup,'source_release':source_id,'release':target,'instance_state':'stopped','qualification_only':canary,
        'candidate_probe_sha256':sha(candidate_probe)}
    if canary:
        if pointer is None:(directory/'active.json').unlink(missing_ok=True)
        else:kube.infra.private_write(directory/'active.json',pointer)
        # Each Helm dispatch uses its own real suboperation ID while preserving
        # the source authority. It never retries an ambiguous original RPC.
        kube.deploy(active,0,stage='rollback');source_probe=kube.startup_probe(active,'source-probe')
        kube.deploy(active,0,stage='source-stopped');kube.runtime_probe(active)
        receipt['rollback_verified']=True
        kube.infra.private_write(evidence,json.dumps({'backend_profile_sha256':operation['backend_profile']['specification_sha256'],
            'source_release':source_id,'release':target,'rollback_verified':True,'original_operation_id':operation['id'],
            'backup_sha256':backup['backup_sha256'],'candidate_probe_sha256':sha(candidate_probe),
            'source_probe_sha256':sha(source_probe)},sort_keys=True).encode())
    else:
        if operation['action']=='upgrade':
            env=pg_environment((candidate/'secrets/database_url').read_text().strip(),kube.infra.config)
            kube.infra.run(['psql','-X','-v','ON_ERROR_STOP=1'],env=env,data=PROMOTE_SCHEDULES_SQL)
        receipt['generation']=operation['id']
        if operation['action']=='rollback':receipt.update(rollback_operation_id=arguments['rollback_operation_id'],restored_backup_id=operation['backup']['id'])
    return receipt


def keep_deleted_metadata(path):
    return (re.fullmatch(r'(?:operation|receipt|kubernetes-operation|kube-deletion|kube-deletion-progress|kube-stop)-[a-f0-9]{32}\.json',path.name)
            or re.fullmatch(r'kube-physical-stop-[a-f0-9-]{36}\.json',path.name)
            or path.name in ('executor-host.json','deleted.json','kubernetes-deleted.json'))


def cleanup_deleted(kube,receipt):
    for target in kube.directory.iterdir():
        if keep_deleted_metadata(target):continue
        safe_tree(target,kube.directory)
        if target.is_dir():shutil.rmtree(target)
        else:target.unlink()
    kube.infra.private_write(kube.directory/'deleted.json',json.dumps(receipt,sort_keys=True).encode())


def delete(kube,operation,directory):
    from instance_deletion import InstanceDeletion
    require(kube.infra.config.get('deletion_enabled') is True and operation['arguments'].get('purge_backups') is True
            and operation['arguments'].get('confirmation_name')==kube.instance['name'],'explicit_original_kubernetes_deletion_required')
    shared=InstanceDeletion(kube.infra);marker=deletion_marker(directory,operation)
    progress_path=directory/('kube-deletion-progress-'+compact(operation['id'])+'.json')
    if marker.exists():
        require(kube.partial_delete,'original_kubernetes_delete_requires_explicit_reconciliation')
        plan=validate_deletion_plan(kube.infra,operation,directory)
    else:
        # The signed original runner barrier is closed before its identities are
        # frozen. No storage mutation precedes this complete ownership inventory.
        kube.quiescent();manifest=kube.manifest();inventory=kube.resource_inventory()
        pods=[]
        for pod in kube.list('pod'):
            kube.owned(pod,pod['metadata']['uid'])
            require(pod['metadata'].get('labels',{}).get('cowork.component')=='runner'
                    and pod['metadata'].get('finalizers')==['cowork.opencowork.dev/physical-stop'],
                    'unbound_producer_in_original_deletion_scope')
            pods.append({'name':pod['metadata']['name'],'uid':pod['metadata']['uid']})
        pvs=[]
        for resource in list(inventory):
            if resource['kind']!='pvc':continue
            claim=kube.object('pvc',resource['name']);kube.owned(claim,resource['uid'])
            name=claim.get('spec',{}).get('volumeName');require(name,'original_claim_has_no_physical_volume')
            volume=kube.object('pv',name);storage=kube.object('storageclass',claim['spec']['storageClassName'])
            require(volume and volume['spec'].get('claimRef',{}).get('uid')==resource['uid']
                    and volume['spec']['claimRef'].get('namespace')==kube.namespace
                    and volume['spec'].get('persistentVolumeReclaimPolicy')=='Delete'
                    and storage and storage.get('reclaimPolicy','Delete')=='Delete'
                    and volume['spec'].get('csi',{}).get('driver')==storage.get('provisioner')
                    and volume['spec'].get('csi',{}).get('volumeHandle'),
                    'original_CSI_volume_delete_contract_unproven')
            pvs.append({'name':name,'uid':volume['metadata']['uid'],'claim_uid':resource['uid'],
                        'driver':volume['spec']['csi']['driver'],'volume_handle':volume['spec']['csi']['volumeHandle']})
        latest='sh.helm.release.v1.'+kube.release+'.v'+str(manifest['release_revision'])
        latest_secret=kube.object('secret',latest)
        require(latest_secret and latest_secret['metadata']['uid']==manifest['release_uid'],
                'original_helm_release_deletion_uid_changed')
        for secret in kube.list('secret'):
            labels=secret['metadata'].get('labels',{});name=secret['metadata']['name']
            if labels.get('owner')=='helm':
                require(labels.get('name')==kube.release and secret.get('type')=='helm.sh/release.v1'
                        and re.fullmatch(re.escape('sh.helm.release.v1.'+kube.release+'.v')+'[1-9][0-9]{0,9}',name),
                        'foreign_helm_bookkeeping_in_original_deletion_scope')
                inventory.append({'kind':'secret','name':name,'uid':secret['metadata']['uid']})
            elif name==kube.settings['runner_tls']['secret_name']:
                require(secret['metadata']['uid']==kube.settings['runner_tls']['certificate_uid'],'original_runner_TLS_deletion_uid_changed')
                inventory.append({'kind':'secret','name':name,'uid':secret['metadata']['uid']})
        resources=shared.generations(kube.instance,directory)
        backup_ids,staging=shared.backups(operation,directory)
        plan={'version':1,'operation_id':operation['id'],'original_operation_sha256':operation_digest(operation),
            'instance_id':kube.instance['id'],'organisation_id':kube.instance['organisation_id'],
            'namespace':kube.namespace,'namespace_uid':kube.spec['namespace_uid'],'cluster_uid':kube.spec['cluster_uid'],
            'backend_profile':operation['backend_profile'],'resources':resources,'kubernetes':inventory,'pods':pods,'pvs':pvs,
            'runner_deployment':manifest['deployments']['runner'],'backup_ids':backup_ids,'staging':staging,
            'database_endpoint':kube.infra.config['database_endpoint'],'s3_endpoint':kube.infra.config['s3_endpoint'],
            'backup_root':str(shared.root.resolve())}
        # Before freezing, validate the original external data principals too.
        users={r['accessKey']:r for r in shared.rows('admin','user','list',shared.alias)}
        for resource in resources:
            db=resource['database'];owner=shared.sql("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='"+db+"';").decode().strip()
            require(owner in ('',db) and (resource['s3_user'] not in users
                    or users[resource['s3_user']].get('policyName','') in ('',resource['bucket'])),'foreign_original_data_principal')
        kube.infra.private_write(marker,json.dumps(plan,sort_keys=True).encode())
        validate_deletion_plan(kube.infra,operation,directory)
    plan_sha=sha(plan)
    progress={'version':1,'original_plan_sha256':plan_sha,'phase':'prepared'}
    if progress_path.exists():
        progress=epochs.document(progress_path)
        require(progress['version']==1 and progress['original_plan_sha256']==plan_sha
                and progress['phase'] in ('prepared','producers_stopped','namespace_dispatched','infrastructure_complete','completed'),
                'original_kubernetes_delete_progress_changed')
    def checkpoint(phase):
        progress['phase']=phase;kube.infra.private_write(progress_path,json.dumps(progress,sort_keys=True).encode())
    namespace=deletion_scope(kube)
    if progress['phase']=='prepared':
        require(namespace is not None and not namespace['metadata'].get('deletionTimestamp'),
                'namespace_disappeared_before_original_producer_proof')
        runner=kube.object('deployment','runner');kube.owned(runner,plan['runner_deployment']['uid'])
        patch=[{'op':'test','path':'/metadata/uid','value':plan['runner_deployment']['uid']},
            {'op':'test','path':'/metadata/resourceVersion','value':runner['metadata']['resourceVersion']},
            {'op':'replace','path':'/spec/replicas','value':0}]
        kube.command('patch','deployment','runner','--type=json','-p',json.dumps(patch))
        for original in plan['pods']:kube.delete('pod',original['name'],original['uid'])
        require(not kube.list('pod'),'original_deletion_producers_remain')
        checkpoint('producers_stopped')
    # Recheck retained physical terminal evidence on every original recovery.
    for original in plan['pods']:
        proof=epochs.document(directory/('kube-physical-stop-'+original['uid']+'.json'),1048576)
        require(proof['pod_uid']==original['uid'] and proof['namespace_uid']==kube.spec['namespace_uid']
                and proof['cluster_uid']==kube.spec['cluster_uid'] and proof['container_statuses']
                and all(s.get('state',{}).get('terminated',{}).get('finishedAt') for s in proof['container_statuses']),
                'original_deletion_physical_stop_evidence_changed')
    users={r['accessKey']:r for r in shared.rows('admin','user','list',shared.alias)}
    for resource in plan['resources']:
        db=resource['database'];owner=shared.sql("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='"+db+"';").decode().strip()
        require(owner in ('',db),'foreign_kubernetes_database_owner')
        if shared.sql("SELECT 1 FROM pg_roles WHERE rolname='"+db+"';").strip():shared.sql('ALTER ROLE '+db+' NOLOGIN;')
        if owner:shared.sql('ALTER DATABASE '+db+' ALLOW_CONNECTIONS false;');shared.sql('DROP DATABASE '+db+' WITH (FORCE);')
        shared.sql('DROP ROLE IF EXISTS '+db+';')
        user=resource['s3_user']
        if user in users:
            require(users[user].get('policyName','') in ('',resource['bucket']),'foreign_kubernetes_storage_policy')
            shared.storage('admin','user','disable',shared.alias,user);shared.storage('admin','user','remove',shared.alias,user)
    buckets={r['key'].rstrip('/') for r in shared.rows('ls',shared.alias)};policies={r['policy'] for r in shared.rows('admin','policy','list',shared.alias)}
    for resource in plan['resources']:
        if resource['bucket'] in buckets:shared.storage('rb','--force',shared.alias+'/'+resource['bucket'])
        if resource['bucket'] in policies:shared.storage('admin','policy','remove',shared.alias,resource['bucket'])
    namespace=deletion_scope(kube)
    if progress['phase']=='producers_stopped':
        require(namespace is not None and not namespace['metadata'].get('deletionTimestamp'),'namespace_delete_dispatched_outside_original_plan')
        # Static objects are removed by original UID, never helm uninstall or a
        # namespace/name selector. Deployment GC has no live producer remaining.
        priority={'deployment':0,'service':1,'rolebinding':2,'clusterrolebinding':2,'role':3,'clusterrole':3,'pvc':4,'secret':5}
        for resource in sorted(plan['kubernetes'],key=lambda r:(priority.get(r['kind'],2),r['kind'],r['name'])):
            if resource['kind']=='serviceaccount' and resource['name']=='default':continue
            kube.delete(resource['kind'],resource['name'],resource['uid'])
        for kind in ('deployment','service','secret','configmap','serviceaccount','role','rolebinding','networkpolicy','resourcequota','poddisruptionbudget','pvc','pod','replicaset'):
            rows=kube.list(kind)
            if kind=='serviceaccount':
                original=next((r for r in plan['kubernetes'] if r['kind']=='serviceaccount' and r['name']=='default'),None)
                require(original and len(rows)==1 and rows[0]['metadata']['name']=='default'
                        and rows[0]['metadata']['uid']==original['uid'],'original_default_service_account_changed')
            else:require(not rows,'original_namespace_has_unproven_remaining_resources')
        # CSI Delete acknowledgement and original PV absence are required too.
        for original in plan['pvs']:
            for _ in range(600):
                actual=kube.object('pv',original['name'])
                if actual is None:break
                require(actual['metadata']['uid']==original['uid'] and actual['spec'].get('csi',{}).get('volumeHandle')==original['volume_handle'],
                        'original_volume_was_replaced_during_deletion')
                time.sleep(.5)
            require(actual is None,'original_CSI_physical_volume_deletion_unproven')
        checkpoint('namespace_dispatched')
        namespace=deletion_scope(kube)
        if namespace is not None and not namespace['metadata'].get('deletionTimestamp'):
            kube.command('delete','--raw','/api/v1/namespaces/'+kube.namespace,'-f','-',data=json.dumps({
                'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':kube.spec['namespace_uid'],
                'resourceVersion':namespace['metadata']['resourceVersion']},'propagationPolicy':'Foreground'}).encode())
    if progress['phase']=='namespace_dispatched':
        namespace=deletion_scope(kube)
        # An explicit recovery may resend only this idempotent exact-UID/RV
        # deletion after fresh readback proves the original object still exists.
        if namespace is not None and not namespace['metadata'].get('deletionTimestamp'):
            require(kube.partial_delete,'original_namespace_delete_ack_unknown_requires_reconciliation')
            kube.command('delete','--raw','/api/v1/namespaces/'+kube.namespace,'-f','-',data=json.dumps({
                'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':kube.spec['namespace_uid'],
                'resourceVersion':namespace['metadata']['resourceVersion']},'propagationPolicy':'Foreground'}).encode())
        for _ in range(600):
            namespace=deletion_scope(kube)
            if namespace is None:break
            time.sleep(.5)
        require(namespace is None,'original_namespace_deletion_absence_unproven')
        checkpoint('infrastructure_complete')
    require(deletion_scope(kube) is None,'deleted_original_namespace_reappeared')
    for original in plan['pvs']:require(kube.object('pv',original['name']) is None,'deleted_original_physical_volume_reappeared')
    for resource in plan['kubernetes']:
        if resource['kind'] in ('clusterrole','clusterrolebinding'):
            require(kube.object(resource['kind'],resource['name']) is None,'original_cluster_role_deletion_unproven')
    for resource in plan['resources']:
        require(not shared.sql("SELECT 1 FROM pg_database WHERE datname='"+resource['database']+"' UNION ALL SELECT 1 FROM pg_roles WHERE rolname='"+resource['database']+"';").strip(),
                'original_kubernetes_database_deletion_unproven')
    remaining={r['key'].rstrip('/') for r in shared.rows('ls',shared.alias)}
    remaining_users={r['accessKey'] for r in shared.rows('admin','user','list',shared.alias)}
    remaining_policies={r['policy'] for r in shared.rows('admin','policy','list',shared.alias)}
    require(not any(r['bucket'] in remaining or r['s3_user'] in remaining_users or r['bucket'] in remaining_policies for r in plan['resources']),
            'original_kubernetes_storage_deletion_unproven')
    for identifier in plan['backup_ids']:
        metadata=shared.root/(compact(identifier)+'.json')
        if metadata.exists():
            owner=epochs.document(metadata);require(owner['instance_id']==kube.instance['id'] and owner['organisation_id']==kube.instance['organisation_id'],
                'original_backup_deletion_owner_changed')
        for suffix in ('.age','.json'):
            target=shared.root/(compact(identifier)+suffix);safe_tree(target,shared.root);target.unlink(missing_ok=True)
    for name in plan['staging']:
        target=shared.root/name;safe_tree(target,shared.root)
        if target.exists():
            source=target/'owner.json';source=source if source.exists() else target/'payload/instance.json'
            owner=epochs.document(source);require(owner.get('instance_id',owner.get('id'))==kube.instance['id']
                and owner.get('organisation_id')==kube.instance['organisation_id'],'original_backup_staging_owner_changed')
            shutil.rmtree(target)
    receipt={'receipt_id':operation['id'],'instance_id':kube.instance['id'],'deletion_verified':True,
        'deletion_inventory_sha256':plan_sha,'deleted_backup_count':len(plan['backup_ids'])}
    tombstone={'format':'cowork-kubernetes-deletion-v1','original_operation_sha256':operation_digest(operation),
        'cluster_uid':kube.spec['cluster_uid'],'namespace':kube.namespace,'namespace_uid':kube.spec['namespace_uid'],
        'inventory_sha256':plan_sha,'receipt':receipt}
    kube.infra.private_write(directory/'kubernetes-deleted.json',json.dumps(tombstone,sort_keys=True).encode())
    cleanup_deleted(kube,receipt);checkpoint('completed')
    return receipt


def recover_completed(infra,operation,directory):
    """Readback-only recovery of one durably completed original operation."""
    profile=backend_profile.operation(infra.config,operation)
    if profile is None or profile['specification']['kind']!='kubernetes':return None
    path=directory/('kubernetes-operation-'+compact(operation['id'])+'.json')
    if not path.exists():return None
    prior=epochs.document(path,16*1024*1024)
    require(prior['original_sha256']==operation_digest(operation) and prior['operation_id']==operation['id'],
            'original_kubernetes_completed_operation_changed')
    if prior['phase']!='completed':
        if operation['action']!='delete' or not (directory/'kubernetes-deleted.json').exists():return None
        tombstone=epochs.document(directory/'kubernetes-deleted.json',16*1024*1024)
        receipt=tombstone['receipt']
        expected=epochs.binding(operation)
        if expected:receipt={**receipt,'execution':{'binding':expected,'active_epoch':expected['source_epoch']}}
        # A completed infrastructure tombstone alone cannot claim removal of
        # local credentials if the process crashed before its cleanup phase.
        if not all(keep_deleted_metadata(p) for p in directory.iterdir()):return None
    else:receipt=prior['receipt']
    require(receipt['receipt_id']==operation['id'] and receipt['instance_id']==operation['instance']['id'],
            'original_kubernetes_completed_receipt_changed')
    if operation['action']=='delete':
        tombstone=epochs.document(directory/'kubernetes-deleted.json',16*1024*1024)
        spec=profile['specification']
        require(tombstone['original_operation_sha256']==operation_digest(operation) and tombstone['cluster_uid']==spec['cluster_uid']
                and tombstone['namespace_uid']==spec['namespace_uid'] and tombstone['namespace']==spec['namespace']
                and tombstone['receipt']=={k:v for k,v in receipt.items() if k!='execution'}
                and receipt['deletion_verified'] is True,'original_kubernetes_deletion_tombstone_changed')
        raw=infra.run(['kubectl','get','namespace','kube-system','-o','json'])
        require(json.loads(raw)['metadata']['uid']==spec['cluster_uid'],'original_completed_deletion_cluster_changed')
        raw=infra.run(['kubectl','get','namespace',spec['namespace'],'--ignore-not-found=true','-o','json'])
        require(not raw.strip(),'deleted_original_namespace_reappeared_requires_review')
    else:
        backend_profile.validate(infra,operation,physical=True)
    return receipt


def installed_release_inventory(infra,directory):
    """Read actual original deployment/release identities without inventing an operation."""
    selected=backend_profile.settings(infra.config)
    require(selected and selected['specification']['kind']=='kubernetes','original_kubernetes_inventory_profile_required')
    spec=selected['specification']
    require(directory.name==compact(spec['instance_id']),'original_kubernetes_inventory_directory_changed')
    if (directory/'deleted.json').exists() or any(directory.glob('kube-deletion-*.json')):return None
    for path in directory.glob('operation-*.json'):
        journal=epochs.document(path,16*1024*1024)
        require(journal['binding']['instance_id']==spec['instance_id'],'original_inventory_journal_scope_changed')
        if journal.get('completion') is None:return None
    if not (directory/'kubernetes.json').exists():return None
    manifest=epochs.document(directory/'kubernetes.json',16*1024*1024)
    require(manifest['cluster_uid']==spec['cluster_uid'] and manifest['namespace_uid']==spec['namespace_uid'],
            'original_kubernetes_inventory_manifest_changed')
    def read(kind,name):
        raw=infra.run(['kubectl','--namespace',spec['namespace'],'get',kind,name,'--ignore-not-found=true','-o','json'])
        return json.loads(raw) if raw.strip() else None
    cluster=read('namespace','kube-system');namespace=read('namespace',spec['namespace'])
    require(cluster and cluster['metadata']['uid']==spec['cluster_uid'] and namespace
            and namespace['metadata']['uid']==spec['namespace_uid'] and not namespace['metadata'].get('deletionTimestamp'),
            'actual_original_kubernetes_inventory_scope_changed')
    labels=namespace['metadata'].get('labels',{})
    require(labels.get('cowork.instance-id')==spec['instance_id'] and labels.get('cowork.organisation-id')==spec['organisation_id']
            and labels.get('cowork.executor-host-id')==spec['host_id'],'original_kubernetes_inventory_assignment_changed')
    release=read('secret','sh.helm.release.v1.'+spec['namespace']+'.v'+str(manifest['release_revision']))
    require(release and release['metadata']['uid']==manifest['release_uid'] and release['metadata'].get('labels',{}).get('name')==spec['namespace'],
            'actual_original_helm_inventory_uid_changed')
    active=infra.active_directory(directory);images=epochs.document(active/'images.json',1048576)
    require(images==manifest['values']['images'],'original_installed_image_set_changed')
    for service,key in [('api','server'),('worker','server'),('runner','runner'),('egress-proxy','egress_proxy')]:
        deployment=read('deployment',service);original=manifest['deployments'][service]
        require(deployment and deployment['metadata']['uid']==original['uid'] and all(deployment['metadata'].get('labels',{}).get(k)==v
            for k,v in [('cowork.instance',spec['instance_id']),('cowork.organisation',spec['organisation_id'])]),
            'actual_original_deployment_inventory_uid_changed')
        containers=deployment['spec']['template']['spec']['containers']
        require(len(containers)==1 and containers[0]['image']==images[key],'actual_installed_deployment_digest_changed')
        if deployment['spec']['replicas']:
            pods=json.loads(infra.run(['kubectl','--namespace',spec['namespace'],'get','pods','-l',
                'cowork.instance='+spec['instance_id']+',cowork.component='+service,'-o','json']))['items']
            require(len(pods)==deployment['spec']['replicas'],'actual_installed_service_replica_inventory_changed')
            for pod in pods:
                owner=next((o for o in pod['metadata'].get('ownerReferences',[]) if o.get('controller') and o['kind']=='ReplicaSet'),None)
                require(owner is not None,'original_service_pod_lineage_absent')
                replica=read('replicaset',owner['name'])
                require(replica and replica['metadata']['uid']==owner['uid'] and any(o.get('controller') and o['kind']=='Deployment'
                    and o['uid']==original['uid'] for o in replica['metadata'].get('ownerReferences',[])),
                    'actual_installed_service_pod_lineage_changed')
                statuses=pod.get('status',{}).get('containerStatuses',[])
                require(pod.get('status',{}).get('phase')=='Running' and len(statuses)==1 and statuses[0].get('ready') is True
                    and statuses[0].get('imageID','').endswith('sha256:'+images[key].split('@sha256:')[1]),
                    'actual_installed_service_image_readback_unproven')
    return {'instance_id':spec['instance_id'],'organisation_id':spec['organisation_id'],'release':image_set_id(images)}
