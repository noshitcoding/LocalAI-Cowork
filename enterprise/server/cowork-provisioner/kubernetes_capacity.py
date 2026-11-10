"""Fresh exhaustive capacity evidence from bound nodes, namespaces and UIDs."""
from decimal import Decimal
import hashlib
import json
import re
import uuid

import backend_profile
import execution_generation as epochs
import resource_budget
from kubernetes_lifecycle import settings, sha


def quantity(value, memory=False):
    if not isinstance(value,str):raise ValueError('quantity is not text')
    match=re.fullmatch(r'([0-9]+(?:\.[0-9]+)?)([a-zA-Z]*)',value)
    if not match:raise ValueError('unsupported bounded Kubernetes quantity')
    number=Decimal(match[1]);suffix=match[2]
    multipliers={'':1,'Ki':1024,'Mi':1024**2,'Gi':1024**3,'Ti':1024**4,'k':1000,'M':1000**2,'G':1000**3,
                 'm':Decimal('.001'),'u':Decimal('.000001'),'n':Decimal('.000000001')}
    if suffix not in multipliers:raise ValueError('unsupported Kubernetes quantity suffix')
    actual=number*multipliers[suffix]
    return int(actual/(1024**2)) if memory else int(actual*1000)


def collect(infra,challenge):
    selected=backend_profile.settings(infra.config);config=settings(infra.config)
    spec=selected['specification'];host=challenge['executor_host_binding']
    if host!=backend_profile.manager_binding(infra.config):raise ValueError('original capacity host changed')
    def read(kind,*args):
        value=json.loads(infra.run(['kubectl','--request-timeout=10s','get',kind,*args,'-o','json'],timeout=12))
        return value
    cluster=read('namespace','kube-system')
    if cluster['metadata']['uid']!=spec['cluster_uid']:raise ValueError('original capacity cluster changed')
    selector=','.join(k+'='+v for k,v in sorted(config['node_selector'].items()))
    nodes=read('nodes','--selector',selector)['items']
    if not nodes or len(nodes)>1000:raise ValueError('bound node inventory incomplete')
    node_names={n['metadata']['name'] for n in nodes}
    if node_names!=set(config['node_names']):raise ValueError('original executor node inventory changed')
    node_uids={n['metadata']['name']:n['metadata']['uid'] for n in nodes}
    if node_uids!=config['node_uids']:raise ValueError('original executor node UID changed')
    gui=config['desktop_profile'];gui_names=set();gui_pid_limits={}
    if gui is not None:
        gui_selector=','.join(k+'='+v for k,v in sorted(gui['node_selector'].items()))
        gui_nodes=read('nodes','--selector',gui_selector)['items'];gui_names={n['metadata']['name'] for n in gui_nodes}
        if gui_names!=set(gui['node_names']) or {n['metadata']['name']:n['metadata']['uid'] for n in gui_nodes}!=gui['node_uids']:
            raise ValueError('original GUI node inventory changed')
        for node in gui_nodes:
            if node.get('spec',{}).get('unschedulable') or not any(c['type']=='Ready' and c['status']=='True' for c in node['status']['conditions']):
                raise ValueError('original GUI node unavailable')
            observed=json.loads(infra.run(['kubectl','get','--raw','/api/v1/nodes/'+node['metadata']['name']+'/proxy/configz'],timeout=12))
            pids=observed.get('kubeletconfig',{}).get('podPidsLimit')
            if type(pids) is not int or not 0<pids<=4096:raise ValueError('actual GUI Pod PID limit is unbounded')
            gui_pid_limits[node['metadata']['name']]=pids
        nodes.extend(gui_nodes)
    all_nodes=node_names|gui_names
    capacity={'cpu_millis':0,'memory_mib':0}
    for node in nodes:
        if node.get('spec',{}).get('unschedulable') or not any(c['type']=='Ready' and c['status']=='True' for c in node['status']['conditions']):
            raise ValueError('bound node unavailable')
        alloc=node['status']['allocatable'];capacity['cpu_millis']+=quantity(alloc['cpu']);capacity['memory_mib']+=quantity(alloc['memory'],True)
        observed=json.loads(infra.run(['kubectl','get','--raw','/api/v1/nodes/'+node['metadata']['name']+'/proxy/configz'],timeout=12))
        pids=observed.get('kubeletconfig',{}).get('podPidsLimit')
        if node['metadata']['name'] in node_names:
            minimum=min(v['pids'] for v in challenge['service_plan'].values())
            if type(pids) is not int or not 0<pids<=minimum:raise ValueError('actual kubelet Pod PID limit exceeds original service caps')
    namespaces=read('namespaces')['items'];pods=read('pods','--all-namespaces')['items']
    if len(namespaces)>10000 or len(pods)>20000:raise ValueError('exhaustive cluster inventory exceeds bound')
    original_nodes=[(n['metadata']['uid'],n['metadata']['resourceVersion']) for n in nodes]
    original_pods=[(p['metadata']['uid'],p['metadata']['resourceVersion']) for p in pods]
    unknown=False;by_instance={r['instance_id']:[] for r in challenge['allocations']}
    system=set(config['system_pod_uids'])
    for pod in pods:
        if pod.get('spec',{}).get('nodeName') not in all_nodes:continue
        labels=pod['metadata'].get('labels',{});instance=labels.get('cowork.instance')
        if instance not in by_instance or labels.get('cowork.organisation')!=host['trust_domain']:
            # System pods are individually enrolled by original UID. Their real
            # bounded requests are subtracted, not silently counted as free CPU.
            if pod['metadata']['uid'] not in system:unknown=True;continue
            for container in pod['spec']['containers']:
                limits=container.get('resources',{}).get('limits',{})
                if not all(k in limits for k in ('cpu','memory')):unknown=True;continue
                capacity['cpu_millis']-=quantity(limits['cpu']);capacity['memory_mib']-=quantity(limits['memory'],True)
            continue
        by_instance[instance].append(pod)
    reports=[]
    for row in challenge['allocations']:
        instance=row['instance_id'];directory=infra.root/uuid.UUID(instance).hex
        report={'instance_id':instance,'allocation_revision':row['allocation_revision'],'state':'unknown',
            'execution_epoch':row['execution_epoch'],'manifest_sha256':None,'resource_budget':None}
        try:
            expected_namespace='cowork-'+uuid.UUID(instance).hex
            namespace=next((n for n in namespaces if n['metadata']['name']==expected_namespace),None)
            budget=row['expected_resource_budget'];owned=by_instance[instance]
            if row['instance_state']=='pending' and row['expected_manifest_sha256'] is None:
                # An operator's bootstrap namespace/runner has its own original
                # profile; instance application allocations remain reserved.
                if owned or (directory/'kubernetes.json').exists():raise ValueError('pending instance already applied')
                report['state']='reserved';reports.append(report);continue
            manifest=epochs.document(directory/'kubernetes.json',16*1024*1024)
            local_budget=epochs.document(directory/'resource-budget.json',1048576)
            if local_budget!=budget or sha(manifest)!=row['expected_manifest_sha256'] or manifest['cluster_uid']!=spec['cluster_uid']:
                raise ValueError('original namespace manifest or allocation changed')
            labels=namespace['metadata'].get('labels',{}) if namespace else {}
            if not namespace or namespace['metadata']['uid']!=manifest['namespace_uid'] or labels.get('cowork.instance-id')!=instance \
                or labels.get('cowork.organisation-id')!=host['trust_domain'] or labels.get('cowork.executor-host-id')!=host['host_id']:
                raise ValueError('original namespace host assignment differs')
            values=manifest['values'];epoch=epochs.read({'id':instance,'organisation_id':host['trust_domain']},infra.active_directory(directory))
            if epoch!=row['execution_epoch'] or values['executionEpoch']!=epoch or values['serviceResources']!=budget['services']:
                raise ValueError('original generation or service allocation changed')
            actual_quota=read('resourcequota','instance-services','--namespace',expected_namespace)
            quota_receipt=next((v for v in manifest['resources'] if v['kind']=='resourcequota' and v['name']=='instance-services'),None)
            if not quota_receipt or actual_quota['metadata']['uid']!=quota_receipt['uid']:raise ValueError('original namespace quota UID changed')
            hard=actual_quota['spec']['hard']
            if quantity(hard['limits.cpu'])!=budget['total']['cpu_millis'] or quantity(hard['limits.memory'],True)!=budget['total']['memory_mib']:
                raise ValueError('actual namespace quota differs')
            services=set();sandbox_cpu=0;sandbox_memory=0;sandbox_count=0
            runner_pod=next((p for p in owned if p['metadata'].get('labels',{}).get('cowork.component')=='runner'),None)
            if runner_pod is None:raise ValueError('original runner journal producer unavailable')
            parent=next((v for v in runner_pod['metadata'].get('ownerReferences',[]) if v.get('controller') is True and v['kind']=='ReplicaSet'),None)
            if not parent:raise ValueError('original runner ledger producer parent absent')
            replica=read('replicaset',parent['name'],'--namespace',expected_namespace)
            if replica['metadata']['uid']!=parent['uid'] or not any(v.get('controller') is True and v['kind']=='Deployment'
                and v['uid']==manifest['deployments']['runner']['uid'] for v in replica['metadata'].get('ownerReferences',[])):
                raise ValueError('original runner ledger producer lineage differs')
            # Read-only execution of the native binary against its mounted,
            # original private ledger. No control mutation or synthetic receipt.
            ledger=json.loads(infra.run(['kubectl','--namespace',expected_namespace,'exec',runner_pod['metadata']['name'],
                '--container=runner','--','/usr/local/bin/cowork-runner','--kubernetes-journal-inspect'],timeout=12))
            if ledger['version']!=1 or ledger['scope']['namespace_uid']!=manifest['namespace_uid'] \
                or ledger['scope']['cluster_uid']!=spec['cluster_uid'] or ledger['scope']['generation']!=values['storageGeneration']:
                raise ValueError('actual runner original journal scope differs')
            for pod in owned:
                metadata=pod['metadata'];podspec=pod['spec'];labels=metadata.get('labels',{})
                desktop=labels.get('cowork.desktop') is not None
                selected_nodes=gui_names if desktop else node_names
                selected_selector=gui['node_selector'] if desktop and gui else config['node_selector']
                if podspec.get('nodeName') not in selected_nodes or podspec.get('nodeSelector')!=selected_selector:
                    raise ValueError('native pod escaped bound executor nodes')
                context=podspec.get('securityContext',{})
                if context.get('runAsNonRoot') is not True or context.get('runAsUser')!=10001 or context.get('seccompProfile')!={'type':'RuntimeDefault'}:
                    raise ValueError('native pod security context differs')
                cpu=memory=0
                for container in podspec['containers']:
                    security=container.get('securityContext',{});limits=container.get('resources',{}).get('limits',{})
                    if security.get('allowPrivilegeEscalation') is not False or security.get('readOnlyRootFilesystem') is not True \
                        or security.get('capabilities',{}).get('drop')!=['ALL'] or security.get('capabilities',{}).get('add'):
                        raise ValueError('native container security context differs')
                    cpu+=quantity(limits['cpu']);memory+=quantity(limits['memory'],True)
                component=labels.get('cowork.component')
                if component in resource_budget.DEFAULT_SERVICES:
                    if component in services or cpu!=budget['services'][component]['cpu_millis'] or memory!=budget['services'][component]['memory_mib']:
                        raise ValueError('actual service replica capacity changed')
                    actual=read('deployment',component,'--namespace',expected_namespace)
                    if actual['metadata']['uid']!=manifest['deployments'][component]['uid'] or actual['spec']['replicas']!=1:
                        raise ValueError('original deployment UID changed')
                    parent=next((v for v in pod['metadata'].get('ownerReferences',[]) if v.get('controller') is True and v['kind']=='ReplicaSet'),None)
                    if not parent:raise ValueError('service pod has no original controller')
                    replicaset=read('replicaset',parent['name'],'--namespace',expected_namespace)
                    if replicaset['metadata']['uid']!=parent['uid'] or not any(v.get('controller') is True
                        and v['kind']=='Deployment' and v['uid']==actual['metadata']['uid'] for v in replicaset['metadata'].get('ownerReferences',[])):
                        raise ValueError('service pod original deployment lineage changed')
                    services.add(component)
                else:
                    if labels.get('cowork.managed')=='environment-lifecycle':
                        import environment_lifecycle
                        try:environment_lifecycle.kubernetes_capacity(infra,pod,instance)
                        except (RuntimeError,KeyError,ValueError,OSError):raise ValueError('environment pod has no original receipt') from None
                        if podspec.get('runtimeClassName')!=config['runtime_class'] or podspec.get('automountServiceAccountToken') is not False:
                            raise ValueError('environment runtime scope differs')
                        sandbox_cpu+=cpu;sandbox_memory+=memory;sandbox_count+=1
                        continue
                    expected_runtime=gui['runtime_class'] if desktop and gui else config['runtime_class']
                    if labels.get('cowork.managed')!='native-runner' or podspec.get('runtimeClassName')!=expected_runtime \
                        or podspec.get('automountServiceAccountToken') is not False:
                        raise ValueError('unbound native workload')
                    admitted=next((r for r in ledger['entries'] if r['pod_uid']==metadata['uid'] and r['pod_name']==metadata['name']
                        and r['phase'] not in ('completed','closed','stopped') and r['kind']!='desktop_exec'),None)
                    if not admitted or admitted['resources']['cpu_nanos']!=cpu*1000000 or admitted['resources']['memory_bytes']!=memory*1024*1024:
                        raise ValueError('native workload has no original create ACK and reservation')
                    if desktop:
                        proof=next((r for r in ledger['desktop_pids'] if r['pod_uid']==metadata['uid'] and not r['stopped']),None)
                        if not proof or proof['pod_pids_limit']!=gui_pid_limits[podspec['nodeName']] \
                            or gui_pid_limits[podspec['nodeName']]>proof['original_pids']:
                            raise ValueError('actual GUI PID limit exceeds its original enrollment')
                    sandbox_cpu+=cpu;sandbox_memory+=memory;sandbox_count+=1
            if sandbox_cpu>budget['sandbox']['cpu_millis'] or sandbox_memory>budget['sandbox']['memory_mib'] \
                or sandbox_count>budget['sandbox']['concurrent_runs']:
                raise ValueError('native runtime exceeds original pool')
            if row['instance_state'] in ('ready','running') and services!=set(resource_budget.DEFAULT_SERVICES):
                raise ValueError('actual services incomplete')
            report.update(state='applied',manifest_sha256=sha(manifest),resource_budget=budget)
        except (KeyError,ValueError,OSError,epochs.ExecutionGenerationError):unknown=True
        reports.append(report)
    after_nodes=read('nodes','--selector',selector)['items']
    if gui is not None:after_nodes.extend(read('nodes','--selector',gui_selector)['items'])
    after_pods=read('pods','--all-namespaces')['items']
    complete=original_nodes==[(n['metadata']['uid'],n['metadata']['resourceVersion']) for n in after_nodes] \
        and original_pods==[(p['metadata']['uid'],p['metadata']['resourceVersion']) for p in after_pods]
    return {'observed_capacity':capacity,'inventory_complete':complete,'unknown_workloads':unknown or not complete,
            'allocations':reports}
