"""Owned Kubernetes lifecycle: deploy, encrypted backup/restore, update/rollback, delete.

Every operation binds an explicit kubectl context, a new deployment namespace,
its server UID and an ownership ConfigMap. No global context is switched. The
namespace is exclusive to this installation; no existing namespace is adopted.
"""
from __future__ import annotations
import argparse
import base64
import contextlib
import json
import os
from pathlib import Path
import re
import tarfile
import tempfile
import time
import uuid

import lifecycle as core
import workspace_backup as workspaces
from s3_backup import ObjectBackup, credential

LABEL='cowork.local/deployment-id'
WRITERS={'cowork','cowork-api','cowork-worker'}
RESOURCES={'Deployment','Service','PersistentVolumeClaim','Ingress','NetworkPolicy','PodDisruptionBudget','ConfigMap'}


class Cluster:
    def __init__(self, root, value, verify=True):
        self.root, self.value = root, value
        self.context, self.namespace = value['kubernetes']['context'], value['kubernetes']['namespace']
        core.require(self.context and '\n' not in self.context, 'Select an explicit kubectl context')
        if verify:
            namespace=self.json('get','namespace',self.namespace)
            core.require(namespace['metadata']['uid']==value['kubernetes']['namespace_uid']
                         and namespace['metadata'].get('labels',{}).get(LABEL)==value['id'], 'Cluster namespace ownership changed')
            marker=self.json('get','configmap','cowork-lifecycle-owner')
            core.require(marker['data']['deployment_id']==value['id'], 'Cluster installation marker changed')

    def call(self,*arguments,capture=False,output=None,input_file=None):
        return core.command(['kubectl','--context',self.context,'--namespace',self.namespace,*arguments],
                            capture=capture,output=output,input_file=input_file)

    def json(self,*arguments):
        return json.loads(self.call(*arguments,'-o','json',capture=True))

    def apply(self,resource):
        with tempfile.TemporaryDirectory(prefix='.cowork-kube-',dir=self.root) as temporary:
            path=Path(temporary)/'resource.json';core.write_json(path,resource)
            self.call('apply','-f',str(path))

    def writers(self):
        result={}
        for item in self.json('get','deployments','-l',LABEL+'='+self.value['id'])['items']:
            name=item['metadata']['name']
            if name in WRITERS:
                result[name]=item['spec'].get('replicas',1)
        core.require(result,'No owned Cowork deployment is installed')
        return result

    def scale(self,replicas):
        for name,count in replicas.items():
            core.require(name in WRITERS,'Unknown writer deployment')
            self.call('scale','deployment/'+name,'--replicas='+str(count))
        deadline=time.monotonic()+180
        while time.monotonic()<deadline:
            items=self.json('get','deployments','-l',LABEL+'='+self.value['id'])['items']
            counts_ready=all(item['status'].get('replicas',0)==replicas[item['metadata']['name']]
                   for item in items if item['metadata']['name'] in replicas)
            pods_stopped=True
            if not any(replicas.values()):
                pods=self.json('get','pods','-l',LABEL+'='+self.value['id'])['items']
                pods_stopped=not any(item['metadata'].get('labels',{}).get('app') in WRITERS
                                     and item.get('status',{}).get('phase') not in {'Succeeded','Failed'} for item in pods)
            if counts_ready and pods_stopped:
                return
            time.sleep(1)
        raise ValueError('Owned deployment did not reach its selected replica count')

    def ready(self,replicas):
        for name,count in replicas.items():
            if count:
                self.call('rollout','status','deployment/'+name,'--timeout=180s')

    @contextlib.contextmanager
    def maintenance(self,read_only=False):
        configuration=self.value['kubernetes']
        name='cowork-maintenance-'+uuid.uuid4().hex[:16]
        secret=configuration['secret'];ha=configuration['ha']
        env=[{'name':'PGHOST','value':configuration['database_host']},
             {'name':'PGUSER','value':'cowork'},{'name':'PGDATABASE','value':'cowork'},
             {'name':'PGPASSWORD','valueFrom':{'secretKeyRef':{'name':secret,'key':'postgres_password'}}}]
        if ha:
            env += [{'name':'PGSSLMODE','value':'verify-full'},
                    {'name':'PGSSLROOTCERT','value':'/keys/postgres_ca.pem'}]
        pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'namespace':self.namespace,'labels':{LABEL:self.value['id']}},
             'spec':{'restartPolicy':'Never','automountServiceAccountToken':False,
                     'securityContext':{'runAsUser':1000,'runAsGroup':1000},
                     'containers':[{'name':'maintenance','image':configuration['maintenance_image'],
                                    'command':['sleep','3600'],'env':env,
                                    'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
                                    'volumeMounts':[{'name':'data','mountPath':'/data','readOnly':read_only},
                                                    {'name':'keys','mountPath':'/keys','readOnly':True},
                                                    {'name':'tmp','mountPath':'/tmp'}],
                                    'resources':{'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'1','memory':'1Gi'}}}],
                     'volumes':[{'name':'data','persistentVolumeClaim':{'claimName':configuration['data_claim']}},
                                {'name':'keys','secret':{'secretName':secret,'defaultMode':0o444}},
                                {'name':'tmp','emptyDir':{'sizeLimit':'256Mi'}}]}}
        self.apply(pod)
        uid=self.json('get','pod',name)['metadata']['uid']
        try:
            self.call('wait','--for=condition=Ready','pod/'+name,'--timeout=180s')
            yield name
        finally:
            observed=self.json('get','pod',name)
            core.require(observed['metadata']['uid']==uid and observed['metadata']['labels'][LABEL]==self.value['id'], 'Maintenance pod identity changed')
            self.call('delete','pod',name,'--wait=true','--timeout=180s')

    def secrets(self):
        items=self.json('get','secrets')['items']
        return [{'apiVersion':'v1','kind':'Secret','metadata':{'name':item['metadata']['name'],'namespace':self.namespace,
                 'labels':{LABEL:self.value['id']}},'type':item.get('type','Opaque'),'data':item.get('data',{})}
                for item in items if item.get('type')!='kubernetes.io/service-account-token']

    def workspace_configuration(self):
        deployments=self.json('get','deployments','-l',LABEL+'='+self.value['id'])['items']
        roots=set();socket=None;gid=None;image=None
        for item in deployments:
            spec=item['spec']['template']['spec']
            volumes={entry['name']:entry for entry in spec.get('volumes',[])}
            if 'tool-workspaces' not in volumes:continue
            volume=volumes['tool-workspaces']
            core.require(volume.get('hostPath',{}).get('type')=='Directory','Team workspace volume must be an explicit host directory')
            roots.add(volume['hostPath']['path'])
            if item['metadata']['name']=='cowork-worker':
                socket=volumes.get('docker-control',{}).get('hostPath',{}).get('path')
                groups=spec.get('securityContext',{}).get('supplementalGroups',[])
                core.require(len(groups)==1 and isinstance(groups[0],int),'Select the exact worker Docker socket GID')
                gid=groups[0]
                image=next(container['image'] for container in spec['containers'] if container['name']=='worker')
        if not roots:return None
        core.require(len(roots)==1 and socket=='/var/run/docker.sock' and image is not None,'Team workspace topology has inconsistent host roots or no selected Docker worker')
        host_root=next(iter(roots));path=Path(host_root)
        core.require(path.is_absolute() and str(path)==host_root and '..' not in path.parts
                     and len(path.parts)>2 and re.fullmatch(r'/[A-Za-z0-9_./-]+',host_root),'Select a canonical bounded team-workspace host path')
        current={node['metadata']['name']:node for node in self.json('get','nodes')['items']}
        inventory=self.workspace_host_inventory(image)
        core.require(inventory['unbound_records']==0,'Legacy guest records have no durable Docker node binding; retain them for explicit recovery')
        binding_resource=self.json('get','configmap','cowork-docker-hosts')
        core.require(binding_resource['metadata'].get('labels',{}).get(LABEL)==self.value['id'],'Docker node map ownership changed')
        bindings=json.loads(binding_resource['data']['bindings.json'])
        core.require(bindings['format']==1 and bindings['deployment_id']==self.value['id'],'Docker node map belongs to another installation')
        core.require(isinstance(bindings['nodes'],list) and 0<len(bindings['nodes'])<=64,'Docker node map budget exceeded')
        for entry in bindings['nodes']:
            core.require(set(entry)=={'name','uid','host_id'} and re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',entry['name']), 'Unknown Docker node binding')
            for key in ['uid','host_id']:
                workspaces.canonical_uuid(entry[key]);core.require(entry[key]!=str(uuid.UUID(int=0)),'Nil Docker node binding')
        mapped={item['name']:item for item in bindings['nodes']}
        core.require(len(mapped)==len(bindings['nodes']) and len({item['uid'] for item in bindings['nodes']})==len(mapped)
                     and len({item['host_id'] for item in bindings['nodes']})==len(mapped),'Docker node map aliases')
        previous=self.value['kubernetes'].get('workspace_nodes',[])
        selected={item['name'] for item in previous}
        for host in inventory['hosts']:
            core.require(host['node_name'] is not None and host['node_uid'] is not None,'A retained Dockerhost has no Kubernetes node identity')
            selected.add(host['node_name'])
        selected.update(name for name,node in current.items()
                        if node['metadata'].get('labels',{}).get('cowork.local/docker-tools-ready')=='true')
        selected.update(item['spec']['nodeName'] for item in self.json('get','pods')['items']
                        if item['metadata'].get('labels',{}).get('app')=='cowork-worker' and item['spec'].get('nodeName'))
        core.require(selected and len(selected)<=64,'Select at least one, at most64, enumerated Docker worker node')
        witnesses=[]
        for name in sorted(selected):
            node=current.get(name)
            core.require(node is not None and any(condition.get('type')=='Ready' and condition.get('status')=='True'
                         for condition in node.get('status',{}).get('conditions',[])), 'Every previously selected Docker node must be present and Ready for workspace quiescence')
            uid=node['metadata']['uid']
            core.require(all(old['name']!=name or old['uid']==uid for old in previous),'Docker node identity changed; prior guest quiescence requires explicit recovery')
            core.require(all(host['node_name']!=name or host['node_uid']==uid for host in inventory['hosts']),'Historical guest Docker node UID changed')
            if name in mapped:core.require(mapped[name]['uid']==uid,'Operator Docker node map UID changed')
            else:mapped[name]={'name':name,'uid':uid,'host_id':str(uuid.uuid4())}
            for host in inventory['hosts']:
                if host['node_name']==name:core.require(host['host_id']==mapped[name]['host_id'],'Historical guest Dockerhost UUID changed')
            witnesses.append({'name':name,'uid':uid,'host_id':mapped[name]['host_id']})
        updated={**bindings,'nodes':sorted(mapped.values(),key=lambda item:item['name'])}
        core.require(len(updated['nodes'])<=64,'Append-only Docker node map budget exceeded')
        if updated!=bindings:
            self.apply({'apiVersion':'v1','kind':'ConfigMap','metadata':{'name':'cowork-docker-hosts','namespace':self.namespace,
                       'resourceVersion':binding_resource['metadata']['resourceVersion'],'labels':{LABEL:self.value['id']}},
                       'data':{'bindings.json':json.dumps(updated,separators=(',',':'))}})
        self.value['kubernetes']['workspace_nodes']=witnesses
        core.save_marker(self.root,self.value)
        return {'host_root':host_root,'docker_gid':gid,'image':image,'nodes':witnesses}

    def workspace_host_inventory(self,image):
        name='cowork-host-inventory-'+uuid.uuid4().hex[:16]
        pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'namespace':self.namespace,'labels':{LABEL:self.value['id']}},
            'spec':{'restartPolicy':'Never','automountServiceAccountToken':False,'securityContext':{'runAsUser':1000,'runAsGroup':1000},
                'containers':[{'name':'inventory','image':image,'command':['python3','-c','import time; time.sleep(600)'],
                    'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
                    'volumeMounts':[{'name':'data','mountPath':'/data','readOnly':True},{'name':'tmp','mountPath':'/tmp'}],
                    'resources':{'requests':{'cpu':'100m','memory':'64Mi'},'limits':{'cpu':'1','memory':'256Mi'}}}],
                'volumes':[{'name':'data','persistentVolumeClaim':{'claimName':self.value['kubernetes']['data_claim']}},
                           {'name':'tmp','emptyDir':{'sizeLimit':'16Mi'}}]}}
        self.apply(pod);uid=self.json('get','pod',name)['metadata']['uid']
        try:
            self.call('wait','--for=condition=Ready','pod/'+name,'--timeout=180s')
            with (core.HERE/'workspace_backup.py').open('rb') as source:
                self.call('exec','-i',name,'--','python3','-c',"import pathlib,sys; pathlib.Path('/tmp/workspace_backup.py').write_bytes(sys.stdin.buffer.read())",input_file=source)
            observed=self.json('get','pod',name);core.require(observed['metadata']['uid']==uid,'Host inventory pod identity changed')
            return json.loads(self.call('exec',name,'--','python3','/tmp/workspace_backup.py','inventory-hosts','--data','/data','--deployment-id',self.value['id'],capture=True))
        finally:
            observed=self.json('get','pod',name);core.require(observed['metadata']['uid']==uid and observed['metadata']['labels'].get(LABEL)==self.value['id'],'Host inventory pod identity changed; retaining it')
            self.call('delete','pod',name,'--wait=true','--timeout=180s')

    @contextlib.contextmanager
    def workspace_maintenance(self):
        configuration=self.workspace_configuration()
        if configuration is None:
            yield [];return
        pods=[]
        try:
            for node in configuration['nodes']:
                name='cowork-workspace-'+uuid.uuid4().hex[:16]
                host_root=configuration['host_root']
                pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'namespace':self.namespace,'labels':{LABEL:self.value['id']}},
                    'spec':{'nodeName':node['name'],'restartPolicy':'Never','automountServiceAccountToken':False,
                        'securityContext':{'runAsUser':1000,'runAsGroup':1000,'supplementalGroups':[configuration['docker_gid']]},
                        'containers':[{'name':'workspace','image':configuration['image'],'command':['python3','-c','import time; time.sleep(3600)'],
                            'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
                            'volumeMounts':[{'name':'data','mountPath':'/data','readOnly':True},
                                           {'name':'workspace','mountPath':host_root},
                                           {'name':'docker','mountPath':'/var/run/docker.sock','readOnly':True},
                                           {'name':'tmp','mountPath':'/tmp'}],
                            'resources':{'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'1','memory':'512Mi'}}}],
                        'volumes':[{'name':'data','persistentVolumeClaim':{'claimName':self.value['kubernetes']['data_claim']}},
                                   {'name':'workspace','hostPath':{'path':host_root,'type':'Directory'}},
                                   {'name':'docker','hostPath':{'path':'/var/run/docker.sock','type':'Socket'}},
                                   {'name':'tmp','emptyDir':{'sizeLimit':'20Gi'}}]}}
                self.apply(pod)
                uid=self.json('get','pod',name)['metadata']['uid']
                item={**configuration,'pod':name,'uid':uid,'node':node}
                pods.append(item)
                self.call('wait','--for=condition=Ready','pod/'+name,'--timeout=180s')
                with (core.HERE/'workspace_backup.py').open('rb') as source:
                    self.call('exec','-i',name,'--','python3','-c',
                              "import pathlib,sys; pathlib.Path('/tmp/workspace_backup.py').write_bytes(sys.stdin.buffer.read())",input_file=source)
                self.workspace_call(item,'claim')
            yield pods
        finally:
            for item in reversed(pods):
                observed=self.json('get','pod',item['pod'])
                core.require(observed['metadata']['uid']==item['uid'] and observed['metadata']['labels'].get(LABEL)==self.value['id'],
                             'Workspace maintenance pod identity changed; retaining it')
                self.call('delete','pod',item['pod'],'--wait=true','--timeout=180s')

    def workspace_call(self,item,action,*extra,capture=True):
        observed=self.json('get','pod',item['pod'])
        core.require(observed['metadata']['uid']==item['uid'] and observed['spec']['nodeName']==item['node']['name'],
                     'Workspace maintenance pod identity changed')
        raw=self.call('exec',item['pod'],'--','python3','/tmp/workspace_backup.py',action,
                      '--root',item['host_root'],'--deployment-id',self.value['id'],'--data','/data',
                      '--host-id',item['node']['host_id'],'--node-name',item['node']['name'],'--node-uid',item['node']['uid'],*extra,capture=capture)
        return json.loads(raw) if capture else None

    def quiesce_workspaces(self,pods):
        receipts=[{'node':item['node'],**self.workspace_call(item,'quiesce')} for item in pods]
        core.require(len({receipt['workspace_id'] for receipt in receipts})<=1,
                     'Docker nodes do not share the same owned team-workspace filesystem')
        witnesses={receipt['host']['host_id'] for receipt in receipts}
        core.require(all(set(receipt['needed_host_ids'])<=witnesses for receipt in receipts),'Not every retained guest Dockerhost positively quiesced')
        return receipts


def initialize(root,hostname,image,context,namespace,manifest,ha=False,database_host=None,maintenance_image='postgres:17-bookworm'):
    core.require(re.fullmatch(r'[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?',namespace),'Invalid installation namespace')
    core.require(manifest.is_absolute() and manifest.is_file() and not manifest.is_symlink(),'Select the rendered installation manifest')
    existing=core.command(['kubectl','--context',context,'get','namespace',namespace,'--ignore-not-found','-o','json'],capture=True)
    core.require(not existing.strip(),'Initialize a NEW namespace; existing namespaces are never adopted')
    value=core.init(root,hostname,image,False,False)
    value['kubernetes']={'context':context,'namespace':namespace,'ha':ha,'database_host':database_host or 'postgres',
                         'data_claim':'cowork-ha-data' if ha else 'cowork-data',
                         'secret':'cowork-ha-keys' if ha else 'cowork-database','maintenance_image':core.image(maintenance_image)}
    cluster=Cluster(root,value,verify=False)
    converted=json.loads(cluster.call('create','--dry-run=client','-f',str(manifest),'-o','json',capture=True))
    items=converted.get('items',[converted])
    for item in items:
        core.require(item['kind']=='Namespace' or item['kind'] in RESOURCES,'Installation manifest contains an unsupported resource')
        metadata=item['metadata']
        core.require((item['kind']=='Namespace' and metadata['name']==namespace) or
                     (item['kind']!='Namespace' and metadata.get('namespace',namespace)==namespace),'Manifest targets another namespace')
        metadata.setdefault('labels',{})[LABEL]=value['id']
        if item['kind']=='Deployment':
            item['spec']['template']['metadata'].setdefault('labels',{})[LABEL]=value['id']
    if any(item['kind']=='Deployment' and any(volume.get('name')=='tool-workspaces'
           for volume in item['spec']['template']['spec'].get('volumes',[])) for item in items):
        nodes=cluster.json('get','nodes')['items']
        value['kubernetes']['workspace_nodes']=[{'name':node['metadata']['name'],'uid':node['metadata']['uid']}
            for node in nodes if node['metadata'].get('labels',{}).get('cowork.local/docker-tools-ready')=='true']
        core.require(0<len(value['kubernetes']['workspace_nodes'])<=64,'Enumerate the operator Docker worker nodes before deploying team tools')
    cluster.apply({'apiVersion':'v1','kind':'Namespace','metadata':{'name':namespace,'labels':{LABEL:value['id']}}})
    value['kubernetes']['namespace_uid']=cluster.json('get','namespace',namespace)['metadata']['uid']
    core.save_marker(root,value)
    cluster.apply({'apiVersion':'v1','kind':'ConfigMap','metadata':{'name':'cowork-lifecycle-owner','namespace':namespace,'labels':{LABEL:value['id']}},
                   'data':{'deployment_id':value['id']}})
    if value['kubernetes'].get('workspace_nodes'):
        nodes=[{**node,'host_id':str(uuid.uuid4())} for node in value['kubernetes']['workspace_nodes']]
        cluster.apply({'apiVersion':'v1','kind':'ConfigMap','metadata':{'name':'cowork-docker-hosts','namespace':namespace,'labels':{LABEL:value['id']}},
                       'data':{'bindings.json':json.dumps({'format':1,'deployment_id':value['id'],'nodes':nodes},separators=(',',':'))}})
    cluster.apply({'apiVersion':'v1','kind':'List','items':[item for item in items if item['kind']!='Namespace']})
    return value


def backup(cluster,recipient,resume=True):
    core.require(re.fullmatch(r'age1[0-9a-z]{50,100}|ssh-(?:ed25519|rsa) [A-Za-z0-9+/=]+(?: .*)?',recipient),'Select an explicit age public recipient')
    original=cluster.writers();cluster.scale({name:0 for name in original})
    path=cluster.root/'backups'/('kubernetes-'+uuid.uuid4().hex+'.tar.age')
    completed=False
    try:
        with tempfile.TemporaryDirectory(prefix='.cowork-kube-backup-',dir=cluster.root/'backups') as temporary:
            stage=Path(temporary)
            with cluster.workspace_maintenance() as workspace_pods:
                workspace_receipts=cluster.quiesce_workspaces(workspace_pods)
                if workspace_pods:
                    item=workspace_pods[0]
                    cluster.workspace_call(item,'archive','--archive','/tmp/team-workspaces.tar')
                    with (stage/'team-workspaces.tar').open('xb') as output:
                        cluster.call('exec',item['pod'],'--','cat','/tmp/team-workspaces.tar',output=output)
                    workspaces.validate_archive(stage/'team-workspaces.tar')
                with cluster.maintenance(read_only=True) as pod:
                    for name,arguments in [('database.dump',['pg_dump','--format=custom']),
                                           ('data.tar',['tar','--one-file-system','-C','/data','-cf','-','.'])]:
                        with (stage/name).open('xb') as output:
                            cluster.call('exec',pod,'--',*arguments,output=output)
                cluster.quiesce_workspaces(workspace_pods)
            secrets=cluster.secrets();core.write_json(stage/'secrets.json',secrets)
            objects=ObjectBackup(cluster.value['external_s3']).backup(stage/'external-s3') if cluster.value.get('external_s3') else None
            files={file.relative_to(stage).as_posix():{'bytes':file.stat().st_size,'sha256':core.sha(file)}
                   for file in stage.rglob('*') if file.is_file()}
            manifest={'format':core.FORMAT+'/kubernetes-backup','deployment_id':cluster.value['id'],
                      'kubernetes':cluster.value['kubernetes'],'replicas':original,'image':cluster.value['image'],
                      'files':files,'external_s3_objects_included':objects is not None,'external_s3':objects}
            manifest['team_workspace_included']=bool(workspace_receipts)
            manifest['team_workspace_quiescence']=workspace_receipts
            core.write_json(stage/'manifest.json',manifest)
            plain=stage/'backup.tar'
            with tarfile.open(plain,'w') as archive:
                for name in [*files,'manifest.json']:
                    archive.add(stage/name,arcname=name,recursive=False)
            core.require(cluster.secrets()==secrets,'Cluster secrets changed during backup')
            core.command(['age','-r',recipient,'-o',str(path),str(plain)])
        core.private(path)
        receipt={'format':core.FORMAT+'/kubernetes-backup-receipt','deployment_id':cluster.value['id'],
                 'file':str(path),'sha256':core.sha(path),'replicas':original,'external_s3_objects_included':objects is not None,
                 'team_workspace_included':bool(workspace_receipts),'team_workspace_quiescence':workspace_receipts}
        core.write_json(path.with_suffix(path.suffix+'.json'),receipt)
        completed=True
        return receipt
    finally:
        if resume and completed:
            cluster.scale(original);cluster.ready(original)


@contextlib.contextmanager
def decode(cluster,path,identity):
    core.require(path.is_absolute() and path.is_file() and not path.is_symlink(),'Select the encrypted Kubernetes backup')
    core.require(identity.is_absolute() and identity.is_file() and not identity.is_symlink(),'Select a private age identity')
    if os.name=='posix':core.require(identity.stat().st_mode&0o077==0,'age identity must be private')
    with tempfile.TemporaryDirectory(prefix='.cowork-kube-restore-',dir=cluster.root/'backups') as temporary:
        stage=Path(temporary);plain=stage/'backup.tar'
        core.command(['age','-d','-i',str(identity),'-o',str(plain),str(path)])
        with tarfile.open(plain,'r:') as archive:
            items=archive.getmembers();core.require(len(items)<=100005,'Kubernetes backup entry limit exceeded')
            names=set()
            for item in items:
                name=Path(item.name)
                core.require(item.isfile() and not name.is_absolute() and '..' not in name.parts and '\\' not in item.name
                             and item.name not in names,'Unsafe Kubernetes backup member')
                names.add(item.name)
            member=archive.getmember('manifest.json');core.require(member.size<=32*1024*1024,'Oversized Kubernetes backup manifest')
            manifest=json.load(archive.extractfile(member))
            core.require(manifest['format']==core.FORMAT+'/kubernetes-backup','Unknown Kubernetes backup format')
            core.require(names==set(manifest['files'])|{'manifest.json'},'Kubernetes backup manifest mismatch')
            for name,item in manifest['files'].items():
                core.require(name in {'database.dump','data.tar','secrets.json','team-workspaces.tar'} or name.startswith('external-s3/'),'Unknown Kubernetes backup payload')
                member=archive.getmember(name);core.require(member.size==item['bytes'],'Kubernetes backup size changed')
                target=stage/name;target.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
                with archive.extractfile(member) as source,target.open('xb') as output:
                    __import__('shutil').copyfileobj(source,output,1024*1024)
                core.private(target);core.require(core.sha(target)==item['sha256'],'Kubernetes backup hash changed')
            core.require(bool(manifest.get('team_workspace_included'))==('team-workspaces.tar' in manifest['files']),
                         'Kubernetes team workspace manifest/member mismatch')
            if manifest.get('team_workspace_included'):workspaces.validate_archive(stage/'team-workspaces.tar')
        yield stage,manifest


def restore(cluster,path,identity,acknowledge,external_configuration=None):
    core.require(acknowledge==cluster.value['id'],'Restore requires the exact target deployment UUID')
    replicas=cluster.writers();cluster.scale({name:0 for name in replicas})
    # On failure writers stay stopped. The selected backup and retained files remain.
    with decode(cluster,path,identity) as (stage,manifest),cluster.workspace_maintenance() as workspace_pods:
        cluster.quiesce_workspaces(workspace_pods)
        core.require(bool(workspace_pods)==bool(manifest.get('team_workspace_included')),
                     'Target team-workspace topology does not match the complete selected backup')
        if manifest['external_s3_objects_included']:
            core.require(external_configuration is not None,'Select an empty external S3 restore target')
            s3_manifest=json.loads((stage/'external-s3'/'manifest.json').read_bytes())
            core.require(external_configuration.get('prefix','')==s3_manifest['prefix'],'Restore must preserve database object-key prefixes')
            ObjectBackup(external_configuration).restore(stage/'external-s3')
        current_database_secret=cluster.json('get','secret',cluster.value['kubernetes']['secret'])
        for secret in json.loads((stage/'secrets.json').read_bytes()):
            # A restored database uses the target server's current connection
            # authority. Restore application key custody from the data volume;
            # never replace a new target's DB password/CA or its TLS certificate.
            if secret['metadata']['name']=='cowork-tls':continue
            if secret['metadata']['name'] in {'cowork-database','cowork-ha-keys'}:
                secret['metadata']['name']=cluster.value['kubernetes']['secret']
                for name in ['postgres_password','postgres_ca.pem']:
                    if name in current_database_secret.get('data',{}):
                        secret.setdefault('data',{})[name]=current_database_secret['data'][name]
            secret['metadata']['namespace']=cluster.namespace
            secret['metadata']['labels']={LABEL:cluster.value['id']}
            cluster.apply(secret)
        if external_configuration:configure_external_s3(cluster,external_configuration)
        # Validate the inner volume before moving any current data.
        with tarfile.open(stage/'data.tar','r:') as archive:
            for member in archive.getmembers():
                name=Path(member.name)
                core.require(not name.is_absolute() and '..' not in name.parts and not member.issym() and not member.islnk()
                             and (member.isfile() or member.isdir()),'Unsafe data-volume archive')
        with cluster.maintenance() as pod:
            retained='retained-before-restore-'+uuid.uuid4().hex
            cluster.call('exec',pod,'--','sh','-ec',
                         'mkdir /data/'+retained+'; find /data -mindepth 1 -maxdepth 1 ! -name '+retained+' -exec mv -t /data/'+retained+' -- {} +')
            with (stage/'data.tar').open('rb') as source:
                cluster.call('exec','-i',pod,'--','tar','-C','/data','-xf','-',input_file=source)
            cluster.call('exec',pod,'--','psql','-v','ON_ERROR_STOP=1','-c','DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION cowork;')
            with (stage/'database.dump').open('rb') as source:
                cluster.call('exec','-i',pod,'--','pg_restore','--exit-on-error',input_file=source)
        if workspace_pods:
            item=workspace_pods[0]
            with (stage/'team-workspaces.tar').open('rb') as source:
                cluster.call('exec','-i',item['pod'],'--','python3','-c',
                             "import pathlib,sys,shutil; f=pathlib.Path('/tmp/team-workspaces.tar').open('xb'); shutil.copyfileobj(sys.stdin.buffer,f,1048576); f.close()",input_file=source)
            cluster.workspace_call(item,'restore','--archive','/tmp/team-workspaces.tar','--source-deployment-id',manifest['deployment_id'])
            cluster.quiesce_workspaces(workspace_pods)
        cluster.scale(replicas);cluster.ready(replicas)
        return {'state':'restored','retained_previous_files':True}


def configure_external_s3(cluster,configuration):
    core.require(not any(cluster.writers().values()),'Stop owned writers before changing S3')
    core.require(configuration.get('prefix','') in ('','cowork/v1/'),'Preserve the runtime object-key namespace')
    ObjectBackup(configuration)
    secret=cluster.json('get','secret',cluster.value['kubernetes']['secret'])
    data=secret.setdefault('data',{})
    data['s3_access_key']=base64.b64encode(credential(configuration['access_key_file']).encode()).decode()
    data['s3_secret_key']=base64.b64encode(credential(configuration['secret_key_file']).encode()).decode()
    cluster.apply(secret)
    settings={'COWORK_S3_ENDPOINT':configuration['endpoint'],'COWORK_S3_BUCKET':configuration['bucket'],
              'COWORK_S3_REGION':configuration.get('region','us-east-1'),
              'COWORK_S3_ACCESS_KEY_ID_FILE':'/run/private/keys/s3_access_key',
              'COWORK_S3_SECRET_ACCESS_KEY_FILE':'/run/private/keys/s3_secret_key'}
    if cluster.value['kubernetes']['ha']:
        resource=cluster.json('get','configmap','cowork-ha-operator');resource['data'].update(settings);cluster.apply(resource)
    else:
        resource=cluster.json('get','deployment','cowork')
        for container in resource['spec']['template']['spec']['containers']:
            current={entry['name']:entry for entry in container.get('env',[])}
            current.update({key:{'name':key,'value':value} for key,value in settings.items()})
            container['env']=list(current.values())
        for container in resource['spec']['template']['spec'].get('initContainers',[]):
            if container['name']=='private-database-password':
                container['command']=['/bin/sh','-ec','umask 077; mkdir -p /private/keys; chown 1000:1000 /private /private/keys; chmod 700 /private /private/keys; for f in postgres_password s3_access_key s3_secret_key; do cp /projected/$f /private/keys/$f; chown 1000:1000 /private/keys/$f; chmod 600 /private/keys/$f; done']
        cluster.apply(resource)
    cluster.value['external_s3']=configuration;core.save_marker(cluster.root,cluster.value)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--root',type=Path,required=True)
    sub=parser.add_subparsers(dest='command',required=True)
    command=sub.add_parser('init');command.add_argument('--context',required=True);command.add_argument('--namespace',required=True)
    command.add_argument('--manifest',type=Path,required=True);command.add_argument('--site',required=True);command.add_argument('--image',required=True)
    command.add_argument('--ha',action='store_true');command.add_argument('--database-host');command.add_argument('--maintenance-image',default='postgres:17-bookworm')
    sub.add_parser('status')
    sub.add_parser('stop')
    sub.add_parser('start')
    command=sub.add_parser('configure-secrets');command.add_argument('--private-json',type=Path,required=True)
    command=sub.add_parser('backup');command.add_argument('--recipient',required=True)
    command=sub.add_parser('restore');command.add_argument('--input',type=Path,required=True);command.add_argument('--identity',type=Path,required=True)
    command.add_argument('--deployment-id',required=True);command.add_argument('--external-s3-target',type=Path)
    command=sub.add_parser('update');command.add_argument('--image',required=True);command.add_argument('--recipient',required=True)
    command=sub.add_parser('rollback');command.add_argument('--checkpoint',type=Path,required=True);command.add_argument('--identity',type=Path,required=True)
    command.add_argument('--deployment-id',required=True);command.add_argument('--external-s3-target',type=Path)
    command=sub.add_parser('configure-external-s3');command.add_argument('--configuration',type=Path,required=True)
    command=sub.add_parser('delete');command.add_argument('--deployment-id',required=True);command.add_argument('--data-policy',choices=['retain','purge'],required=True)
    command.add_argument('--backup',type=Path);command.add_argument('--backup-sha256')
    args=parser.parse_args()
    if args.command=='init':
        value=initialize(args.root,args.site,args.image,args.context,args.namespace,args.manifest,args.ha,args.database_host,args.maintenance_image)
        print(json.dumps({'state':'initialized','deployment_id':value['id'],'namespace':value['kubernetes']['namespace']}));return
    root,value=core.load(args.root)
    if value.get('state')=='deleted':
        core.require(args.command=='status','Deleted Kubernetes installations cannot be restarted')
        print(json.dumps({'state':'deleted','deployment_id':value['id'],'data_policy':value['data_policy']}));return
    cluster=Cluster(root,value)
    if args.command=='status':
        print(json.dumps({'deployment_id':value['id'],'namespace':cluster.namespace,'writers':cluster.writers()}))
    elif args.command=='stop':
        value['kubernetes']['stopped_replicas']=cluster.writers();core.save_marker(root,value)
        cluster.scale({name:0 for name in value['kubernetes']['stopped_replicas']})
        with cluster.workspace_maintenance() as pods:cluster.quiesce_workspaces(pods)
        print(json.dumps({'state':'stopped'}))
    elif args.command=='start':
        replicas=value['kubernetes'].get('stopped_replicas') or {name:1 for name in cluster.writers()}
        cluster.scale(replicas);cluster.ready(replicas);print(json.dumps({'state':'started'}))
    elif args.command=='configure-secrets':
        core.require(args.private_json.is_absolute() and args.private_json.is_file() and not args.private_json.is_symlink()
                     and args.private_json.stat().st_size<=1024*1024,'Select a private operator secret JSON')
        if os.name=='posix':core.require(args.private_json.stat().st_mode&0o077==0,'Operator secret JSON must be private')
        entries=json.loads(args.private_json.read_bytes())
        core.require(isinstance(entries,dict) and 'postgres_password' in entries and all(re.fullmatch(r'[a-z0-9_]+',key) and isinstance(item,str) for key,item in entries.items()),'Invalid operator secret entries')
        resource={'apiVersion':'v1','kind':'Secret','metadata':{'name':value['kubernetes']['secret'],'namespace':cluster.namespace,'labels':{LABEL:value['id']}},
                  'type':'Opaque','data':{key:base64.b64encode(item.encode()).decode() for key,item in entries.items()}}
        cluster.apply(resource);print(json.dumps({'state':'secrets_configured'}))
    elif args.command=='configure-external-s3':
        configuration=json.loads(args.configuration.read_bytes());configure_external_s3(cluster,configuration)
        print(json.dumps({'state':'external_s3_configured'}))
    elif args.command=='backup':print(json.dumps(backup(cluster,args.recipient)))
    elif args.command=='restore':
        configuration=json.loads(args.external_s3_target.read_bytes()) if args.external_s3_target else None
        print(json.dumps(restore(cluster,args.input,args.identity,args.deployment_id,configuration)))
    elif args.command=='update':
        previous=value['image'];receipt=backup(cluster,args.recipient,resume=False)
        checkpoint={'format':core.FORMAT+'/kubernetes-update','deployment_id':value['id'],'previous_image':previous,
                    'image':core.image(args.image),'backup':receipt,'state':'prepared'}
        path=root/'backups'/('kubernetes-update-'+uuid.uuid4().hex+'.json');core.write_json(path,checkpoint)
        for name in receipt['replicas']:
            resource=cluster.json('get','deployment',name)
            for container in resource['spec']['template']['spec']['containers']:
                if container['name'] in {'api','worker'}:container['image']=args.image
            cluster.apply(resource)
        cluster.scale(receipt['replicas']);cluster.ready(receipt['replicas'])
        value['image']=args.image;core.save_marker(root,value)
        checkpoint['state']='updated';temporary=path.with_suffix('.new');core.write_json(temporary,checkpoint);os.replace(temporary,path)
        print(json.dumps({'state':'updated','checkpoint':str(path)}))
    elif args.command=='rollback':
        checkpoint=json.loads(args.checkpoint.read_bytes())
        core.require(checkpoint['format']==core.FORMAT+'/kubernetes-update' and checkpoint['deployment_id']==value['id'],'Checkpoint belongs to another deployment')
        path=Path(checkpoint['backup']['file']);core.require(core.sha(path)==checkpoint['backup']['sha256'],'Rollback backup changed')
        configuration=json.loads(args.external_s3_target.read_bytes()) if args.external_s3_target else None
        original_replicas=cluster.writers()
        cluster.scale({name:0 for name in original_replicas})
        for name in cluster.writers():
            resource=cluster.json('get','deployment',name)
            resource['spec']['replicas']=0
            for container in resource['spec']['template']['spec']['containers']:
                if container['name'] in {'api','worker'}:container['image']=checkpoint['previous_image']
            cluster.apply(resource)
        restore(cluster,path,args.identity,args.deployment_id,configuration)
        cluster.scale(checkpoint['backup']['replicas']);cluster.ready(checkpoint['backup']['replicas'])
        value['image']=checkpoint['previous_image'];core.save_marker(root,value)
        print(json.dumps({'state':'rolled_back','retained_previous_files':True}))
    elif args.command=='delete':
        core.require(args.deployment_id==value['id'],'Deletion requires exact deployment UUID')
        replicas=cluster.writers();cluster.scale({name:0 for name in replicas})
        with cluster.workspace_maintenance() as pods:cluster.quiesce_workspaces(pods)
        if args.data_policy=='purge':
            core.require(args.backup and args.backup.is_absolute() and args.backup.is_file() and core.sha(args.backup)==args.backup_sha256,'Purge requires a pinned retained encrypted backup')
            receipt=json.loads(args.backup.with_suffix(args.backup.suffix+'.json').read_bytes())
            core.require(receipt['deployment_id']==value['id'] and receipt['sha256']==args.backup_sha256,'Purge backup belongs to another deployment')
            # The namespace is exclusive, but operators may have added resources.
            # Every remaining resource must carry this installation's ownership.
            for resource in ['deployments','statefulsets','daemonsets','jobs','cronjobs','pods','persistentvolumeclaims','services','configmaps','secrets','ingresses','networkpolicies','poddisruptionbudgets']:
                for item in cluster.json('get',resource)['items']:
                    metadata=item['metadata']
                    system_secret=item.get('type')=='kubernetes.io/service-account-token'
                    system_config=resource=='configmaps' and metadata['name']=='kube-root-ca.crt'
                    generated_pod=resource=='pods' and bool(metadata.get('ownerReferences')) and metadata.get('labels',{}).get(LABEL)==value['id']
                    core.require(system_secret or system_config or generated_pod or metadata.get('labels',{}).get(LABEL)==value['id'],'Namespace contains an unowned resource; retaining the namespace')
            cluster.call('delete','namespace',cluster.namespace,'--wait=true','--timeout=180s')
        else:
            replicas=cluster.writers();cluster.scale({name:0 for name in replicas})
            for resource in ['deployments','services','ingresses','networkpolicies','poddisruptionbudgets']:
                cluster.call('delete',resource,'-l',LABEL+'='+value['id'],'--wait=true','--timeout=180s')
        value['state']='deleted';value['data_policy']=args.data_policy;core.save_marker(root,value)
        print(json.dumps({'state':'deleted','data_policy':args.data_policy,'external_s3_deleted':False,'team_workspace_retained':bool(value['kubernetes'].get('workspace_nodes'))}))


if __name__=='__main__':
    try:main()
    except (ValueError,OSError,KeyError,__import__('subprocess').SubprocessError) as error:
        print('Kubernetes lifecycle refused: '+str(error),file=__import__('sys').stderr);raise SystemExit(1)
