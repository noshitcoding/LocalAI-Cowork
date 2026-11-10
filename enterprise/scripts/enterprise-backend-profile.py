#!/usr/bin/env python3
"""Prepare/inspect/activate one physically bound VM or Kubernetes instance.

Preparation emits original routing/service assets without installing them. Only
explicit activation contacts the manager; inspect executes read-only physical
QMP/SSH/mTLS or Kubernetes/runner and actual HTTPS routing probes.
"""
import argparse
from datetime import datetime,timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import socket
import ssl
import subprocess
import sys
import uuid
from urllib.parse import urlsplit
from urllib.request import Request,build_opener,HTTPSHandler,HTTPRedirectHandler

def require(ok,message):
    if not ok: raise RuntimeError(message)
def canonical(value):
    return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
def sha(data):return hashlib.sha256(data).hexdigest()
def private(path,directory=False):
    path=Path(path)
    require(path.is_absolute() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents)
            and (path.is_dir() if directory else path.is_file()),'unsafe operator path')
    if os.name=='posix':require(path.stat().st_uid==os.getuid() and path.stat().st_mode&0o077==0,'operator path requires owner-only permissions')
    return path
def write_new(path,data):
    require(Path(path).is_absolute() and not Path(path).is_symlink() and all(not p.is_symlink() for p in Path(path).parents),'new original output requires an absolute nonsymlink path')
    with os.fdopen(os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600),'wb') as output:
        output.write(data if isinstance(data,bytes) else canonical(data));output.flush();os.fsync(output.fileno())
def command(argv,timeout=30):
    env=dict(os.environ)
    for key in ('DOCKER_HOST','DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH','DOCKER_API_VERSION','KUBECONFIG'):
        env.pop(key,None)
    value=subprocess.run(argv,env=env,capture_output=True,timeout=timeout)
    require(value.returncode==0,'physical probe failed: '+Path(argv[0]).name)
    return value.stdout
class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*_):raise RuntimeError('original endpoint redirects are refused')
def https(url,*,ca=None,token=None,operator=False,body=None):
    target=urlsplit(url)
    require(target.scheme=='https' and target.hostname and not target.username and not target.fragment,'exact HTTPS endpoint required')
    ctx=ssl.create_default_context(cafile=ca)
    headers={'Content-Type':'application/json'}
    if token:headers['x-cowork-provisioner-token' if operator else 'Authorization']=token if operator else 'Bearer '+token
    request=Request(url,data=None if body is None else canonical(body),headers=headers)
    with build_opener(NoRedirect(),HTTPSHandler(context=ctx)).open(request,timeout=30) as response:
        payload=response.read(1048577)
        require(len(payload)<=1048576 and response.status==200,'bounded successful HTTPS readback required')
        return json.loads(payload)
def configuration(path):
    raw=private(path).read_bytes();config=json.loads(raw)
    require(config['format']==1 and config['kind'] in ('vm_compose','kubernetes'),'invalid backend profile configuration')
    for key in ('instance_id','organisation_id','host_id','operator_state_id'):
        require(str(uuid.UUID(config[key]))==config[key] and uuid.UUID(config[key]).int!=0,'noncanonical backend scope')
    origin=urlsplit(config['routing']['public_origin'])
    require(origin.scheme=='https' and origin.hostname and origin.path in ('','/') and not origin.username
            and not origin.query and not origin.fragment and origin.hostname.startswith(uuid.UUID(config['instance_id']).hex+'.'),'instance HTTPS routing scope differs')
    if config['kind']=='vm_compose':
        private(config['vm_root'],True)
        require(1024<=config['routing']['api_loopback_port']<=65535,'invalid exact VM API forward port')
    else:
        private(config['kubeconfig'])
        require(isinstance(config['context'],str) and config['context'] and config['namespace']=='cowork-'+uuid.UUID(config['instance_id']).hex,'invalid exact Kubernetes selection')
    return config,sha(raw)
def vm_module():
    path=Path(__file__).resolve().parents[1]/'deploy/executor-vm/local_executor_vm.py'
    spec=importlib.util.spec_from_file_location('_cowork_product_vm',path)
    value=importlib.util.module_from_spec(spec);sys.modules[spec.name]=value;spec.loader.exec_module(value)
    return value
def kube(config,*args):
    return json.loads(command(['kubectl','--kubeconfig',config['kubeconfig'],'--context',config['context'],*args,'-o','json']))
def physical(config,*,routing=True):
    if config['kind']=='vm_compose':
        controller=vm_module();root=Path(config['vm_root']);value=controller.load(root)
        require(value['organisation_id']==config['organisation_id'] and value['operator_state_id']==config['operator_state_id'],'VM original assignment differs')
        pid=controller.process(root);require(pid is not None,'owned VM is not running')
        require(controller.qmp(root,'query-kvm')['enabled'] is True,'real KVM acceleration is not enabled')
        require(controller.qmp(root,'query-status')['status']=='running','VM is not physically running')
        # The retained capacity ledger and exact inventory must still be valid.
        with controller.capacity_pool.locked_pool() as pool:
            pool.require_registered(root,value);pool.check_inventory(controller.load)
        known=root/'known_hosts';require(known.is_file() and not known.is_symlink(),'pinned SSH host identity missing')
        ssh=['ssh','-i',str(root/'ssh-key'),'-p',str(value['ssh_port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes',
             '-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(known),'-o','ConnectTimeout=5','cowork@127.0.0.1']
        identity=command([*ssh,'sudo cat /sys/class/dmi/id/product_uuid']).decode().strip().lower()
        require(identity==value['id'],'guest hardware UUID differs from original VM')
        args=['docker','--host',value['docker_endpoint'],'--tlsverify','--tlscacert',str(root/'ca.pem'),
              '--tlscert',str(root/'client.pem'),'--tlskey',str(root/'client-key.pem')]
        info=json.loads(command([*args,'info','--format','{{json .}}']))
        require(info['ID']==config['daemon_id'] and 'cowork.role=executor' in info.get('Labels',[])
                and 'cowork.trust-domain='+config['organisation_id'] in info.get('Labels',[]),'actual VM daemon identity/trust domain differs')
        if routing:
            tunnel=int(private(config['routing']['tunnel_pid_file']).read_text().strip())
            data=Path('/proc/'+str(tunnel)+'/cmdline').read_bytes().split(b'\0')
            exact='127.0.0.1:'+str(config['routing']['api_loopback_port'])+':127.0.0.1:'+str(config['routing']['api_loopback_port'])
            require(os.stat('/proc/'+str(tunnel)).st_uid==os.getuid() and b'-N' in data and exact.encode() in data
                    and str(root/'ssh-key').encode() in data and str(value['ssh_port']).encode() in data
                    and ('UserKnownHostsFile='+str(known)).encode() in data,'owned exact VM API forwarding process differs')
            with socket.create_connection(('127.0.0.1',config['routing']['api_loopback_port']),timeout=5):pass
        return {'vm_id':value['id'],'qemu_pid':pid,'qemu_owned':True,'kvm_enabled':True,'guest_hardware_uuid':identity,
                'daemon_id':info['ID'],'mutual_tls_verified':True,'capacity_registered':True}
    cluster=kube(config,'get','namespace','kube-system')['metadata']['uid']
    namespace=kube(config,'get','namespace',config['namespace'])
    labels=namespace['metadata'].get('labels',{})
    require(labels.get('cowork.instance-id')==config['instance_id'] and labels.get('cowork.organisation-id')==config['organisation_id']
            and labels.get('cowork.executor-host-id')==config['host_id'],'Kubernetes namespace original ownership differs')
    runner=https(config['runner_origin'].rstrip('/')+'/backend-profile',ca=config.get('runner_ca_file'))
    require(runner['backend']=='kubernetes' and runner['instance_id']==config['instance_id'] and runner['organisation_id']==config['organisation_id']
            and runner['cluster_uid']==cluster and runner['namespace_uid']==namespace['metadata']['uid'],'real Kubernetes runner scope/UID differs')
    return {'cluster_uid':cluster,'namespace_uid':namespace['metadata']['uid'],'runner':runner}
def specification(config,config_sha,actual):
    route=Path(config['routing']['configuration_file'])
    require(route.is_file() and not route.is_symlink() and all(not p.is_symlink() for p in route.parents),'prepared routing source missing')
    spec={'format':1,'kind':config['kind'],**{k:config[k] for k in ('instance_id','organisation_id','host_id','operator_state_id')},
          'public_origin':config['routing']['public_origin'].rstrip('/'),'operator_configuration_sha256':config_sha,
          'routing':{'target_kind':'owned_vm_api_forward' if config['kind']=='vm_compose' else 'owned_namespace_api_service','configuration_sha256':sha(route.read_bytes())}}
    if config['kind']=='vm_compose':spec.update(vm_id=actual['vm_id'],daemon_id=actual['daemon_id'])
    else:spec.update(cluster_uid=actual['cluster_uid'],namespace=config['namespace'],namespace_uid=actual['namespace_uid'],
                     runner={k:actual['runner'][k] for k in ('backend','instance_id','organisation_id')})
    return spec
def inspect(config,config_sha,challenge,spec_sha,runtime=False):
    actual=physical(config);spec=specification(config,config_sha,actual)
    require(sha(canonical(spec))==spec_sha,'original profile changed before physical inspection')
    marker=https(spec['public_origin']+'/__cowork_backend_profile',ca=config['routing'].get('ca_file'))
    require(marker['instance_id']==spec['instance_id'] and marker['host_id']==spec['host_id'] and marker['target_kind']==spec['routing']['target_kind']
            and marker['configuration_sha256']==spec['routing']['configuration_sha256'],'serving public route ownership/configuration differs')
    if runtime:
        https(spec['public_origin']+'/readyz',ca=config['routing'].get('ca_file'))
    binding={'version':1,'host_id':spec['host_id'],'trust_domain':spec['organisation_id'],'operator_state_id':spec['operator_state_id'],
             'daemon_id':actual['daemon_id'] if config['kind']=='vm_compose' else actual['cluster_uid']}
    return {'format':1,'challenge':challenge,'specification_sha256':spec_sha,'executor_host_binding':binding,
            'physical':actual,'routing':{'serving_https_verified':True,'runtime_ready_verified':runtime,'configuration_sha256':spec['routing']['configuration_sha256']},
            'observed_at':datetime.now(timezone.utc).isoformat()}
def prepare_routing(config,output):
    root=Path(output);require(root.is_absolute() and not root.exists(),'new private routing output required');root.mkdir(mode=0o700)
    host=urlsplit(config['routing']['public_origin']).hostname
    require(re.fullmatch('[a-z0-9.-]+',host),'invalid original routing host')
    marker={'instance_id':config['instance_id'],'host_id':config['host_id'],'target_kind':'owned_vm_api_forward' if config['kind']=='vm_compose' else 'owned_namespace_api_service'}
    if config['kind']=='vm_compose':
        controller=vm_module();value=controller.load(Path(config['vm_root']))
        port=config['routing']['api_loopback_port'];target='127.0.0.1:'+str(port)
        tunnel=['/usr/bin/ssh','-N','-L','127.0.0.1:'+str(port)+':127.0.0.1:'+str(port),'-i',config['vm_root']+'/ssh-key','-p',str(value['ssh_port']),
                '-o','ExitOnForwardFailure=yes','-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes',
                '-o','UserKnownHostsFile='+config['vm_root']+'/known_hosts','cowork@127.0.0.1']
        unit='[Unit]\nDescription=Original Cowork VM API forward '+value['id']+'\nAfter=network-online.target\n[Service]\nUser='+command(['id','-un']).decode().strip()+'\nExecStart='+shlex.join(tunnel)+'\nRestart=on-failure\nRestartSec=5\n[Install]\nWantedBy=multi-user.target\n'
        write_new(root/'cowork-vm-api-forward.service',unit.encode())
    else:
        target=config['routing']['api_service_endpoint']
        require(re.fullmatch('[a-z0-9.-]+:[0-9]{2,5}',target),'exact owned Kubernetes API service endpoint required')
    for name in ('certificate_file','private_key_file'):
        require(re.fullmatch('/[A-Za-z0-9_./-]+',config['routing'][name]),'simple absolute routing TLS path required')
    require(re.fullmatch('/[A-Za-z0-9_./-]+',config['routing']['web_root']),'reviewed absolute frontend root required')
    require(re.fullmatch('/[A-Za-z0-9_./-]+',str(root)),'simple immutable routing output path required')
    connection='$cowork_backend_connection_'+uuid.UUID(config['instance_id']).hex
    nginx='map $http_upgrade '+connection+' { default upgrade; \"\" close; }\nserver {\n listen 443 ssl;\n server_name '+host+';\n ssl_certificate '+config['routing']['certificate_file']+';\n ssl_certificate_key '+config['routing']['private_key_file']+';\n ssl_protocols TLSv1.2 TLSv1.3;\n include '+str(root/'backend-route-marker.conf')+';\n location ^~ /api/v1/ { proxy_http_version 1.1; proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection '+connection+'; proxy_set_header Host $host; proxy_set_header X-Forwarded-Proto https; proxy_set_header X-Forwarded-Host $host; proxy_set_header X-Forwarded-For $remote_addr; proxy_buffering off; proxy_read_timeout 3600s; proxy_pass http://'+target+'; }\n location = /readyz { proxy_pass http://'+target+'/readyz; }\n location / { root '+config['routing']['web_root']+'; try_files $uri $uri/ /index.html; }\n}\n'
    nginx=nginx.replace(' ssl_protocols TLSv1.2 TLSv1.3;\n',' ssl_protocols TLSv1.2 TLSv1.3;\n if ($ssl_server_name != $host) { return 421; }\n')
    nginx=nginx.replace(' location ^~ /api/v1/ {',' location ^~ /api/v1/management { return 404; }\n location ^~ /api/v1/ {')
    write_new(root/'backend-route.conf',nginx.encode())
    marker['configuration_sha256']=sha(nginx.encode())
    encoded=canonical(marker).decode().replace("'",'')
    write_new(root/'backend-route-marker.conf',('location = /__cowork_backend_profile { default_type application/json; return 200 \''+encoded+'\'; }\n').encode())
    write_new(root/'ROUTING-PLAN.json',{'format':1,'instance_id':config['instance_id'],'host_id':config['host_id'],'source_sha256':sha(nginx.encode()),'installed':False})
    print(str(root))
def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('action',choices=['prepare-routing','plan','inspect','activate','pair-provisioner'])
    parser.add_argument('--config',required=True);parser.add_argument('--state');parser.add_argument('--output')
    parser.add_argument('--challenge');parser.add_argument('--specification-sha256');parser.add_argument('--runtime-ready',action='store_true')
    parser.add_argument('--expected-instance-revision',type=int);parser.add_argument('--expected-profile-revision',type=int,default=0)
    parser.add_argument('--management');parser.add_argument('--management-ca');parser.add_argument('--management-token-file')
    parser.add_argument('--control');parser.add_argument('--control-ca');parser.add_argument('--operator-token-file')
    parser.add_argument('--provisioner-config')
    args=parser.parse_args();require(os.name=='posix','physical backend producer requires Linux')
    config,config_sha=configuration(args.config)
    if args.action=='prepare-routing':require(args.output,'routing output required');prepare_routing(config,args.output);return
    if args.action=='inspect':
        require(args.challenge and args.specification_sha256,'original physical challenge and profile hash required')
        print(canonical(inspect(config,config_sha,args.challenge,args.specification_sha256,args.runtime_ready)).decode());return
    root=private(args.state,True);plan_path=root/'ORIGINAL-BACKEND-PLAN.json'
    scope_path=root/'ORIGINAL-OPERATOR-SCOPE.json'
    scope={'configuration_sha256':config_sha,'instance_id':config['instance_id'],'organisation_id':config['organisation_id'],'host_id':config['host_id']}
    if scope_path.exists():require(json.loads(private(scope_path).read_bytes())==scope,'original backend operator scope differs')
    else:write_new(scope_path,scope)
    if args.action=='pair-provisioner':
        require(args.output and args.provisioner_config,'a new paired configuration output and original operator configuration are required')
        plan=json.loads(private(plan_path).read_bytes());receipt_path=root/'BACKEND-ACTIVATION-RECEIPT.json'
        receipt=json.loads(private(receipt_path).read_bytes())
        require(receipt['specification_sha256']==plan['specification_sha256'] and plan['specification']['operator_configuration_sha256']==config_sha,'original active backend pair differs')
        original=json.loads(private(args.provisioner_config).read_bytes())
        require(original['backend']==('compose' if config['kind']=='vm_compose' else 'kubernetes'),'original provisioner backend differs')
        origin='https://'+uuid.UUID(config['instance_id']).hex+'.'+original['instance_gateway']['domain']
        require(origin==plan['specification']['public_origin'],'paired runtime launch origin differs')
        if config['kind']=='vm_compose':
            host=original['executor_host'];bound=plan['executor_host_binding']
            require(host['id']==bound['host_id'] and host['trust_domain']==bound['trust_domain'] and host['operator_state_id']==bound['operator_state_id']
                    and host['daemon_id']==bound['daemon_id'] and original['guest_runtime_endpoints']['api_loopback_port']==config['routing']['api_loopback_port'], 'original VM provisioner/route assignment differs')
            state=Path(original['state_directory']);shared=Path(config['vm_root'])/'shared'
            require(state.is_absolute() and state.resolve().is_relative_to(shared.resolve()) and state.resolve()!=shared.resolve()
                    and not state.is_symlink() and all(not p.is_symlink() for p in state.parents),'original VM operator state must use its existing exact readonly guest share')
            require('remote_lifecycle_qualification' not in original,'a protected fixture profile cannot be reinterpreted as a product VM profile')
        else:
            original['kubeconfig_file']=config['kubeconfig'];original['kubernetes_context']=config['context']
        original['backend_profile']={'version':1,'specification':plan['specification'],'activation_receipt_file':str(receipt_path),
                                     'probe_command':[sys.executable,str(Path(__file__).resolve()),'inspect','--config',str(Path(args.config).resolve())],
                                     'probe_script_sha256':sha(Path(__file__).read_bytes())}
        output=Path(args.output);private(output.parent,True)
        write_new(output,original);print(str(output));return
    if args.action=='plan':
        require(args.management and args.management_token_file,'original management endpoint and administrator credential required')
        manager_scope=root/'ORIGINAL-MANAGEMENT-ENDPOINT.json'
        if manager_scope.exists():require(json.loads(private(manager_scope).read_bytes())=={'origin':args.management},'original management endpoint differs')
        else:write_new(manager_scope,{'origin':args.management})
        if plan_path.exists():print(canonical(json.loads(private(plan_path).read_bytes())).decode());return
        actual=physical(config);spec=specification(config,config_sha,actual)
        require(args.expected_instance_revision is not None and args.management and args.management_token_file,'original management plan inputs required')
        intent_path=root/'ORIGINAL-BACKEND-INTENT.json'
        intent={'request_id':str(uuid.uuid4()),'expected_instance_revision':args.expected_instance_revision,'expected_profile_revision':args.expected_profile_revision,
                'specification':spec,'confirmation':'PREPARE_EXACT_BACKEND_PROFILE'}
        if intent_path.exists():
            original=json.loads(private(intent_path).read_bytes());require(original['specification']==spec,'original backend intent differs');intent=original
        else:write_new(intent_path,intent)
        response=https(args.management.rstrip('/')+'/management/v1/organisations/'+config['organisation_id']+'/instances/'+config['instance_id']+'/backend-profile',
                       ca=args.management_ca,token=private(args.management_token_file).read_text().strip(),body=intent)
        write_new(plan_path,response);print(canonical(response).decode());return
    require(args.control and args.operator_token_file,'scoped control activation credential required')
    control_scope=root/'ORIGINAL-CONTROL-ENDPOINT.json'
    if control_scope.exists():require(json.loads(private(control_scope).read_bytes())=={'origin':args.control},'original control endpoint differs')
    else:write_new(control_scope,{'origin':args.control})
    plan=json.loads(private(plan_path).read_bytes());receipt_path=root/'BACKEND-ACTIVATION-RECEIPT.json'
    if receipt_path.exists():print(canonical(json.loads(private(receipt_path).read_bytes())).decode());return
    observation_path=root/'ORIGINAL-ACTIVATION-OBSERVATION.json'
    if observation_path.exists():observation=json.loads(private(observation_path).read_bytes())
    else:
        observation=inspect(config,config_sha,plan['challenge'],plan['specification_sha256']);write_new(observation_path,observation)
    require(args.control and args.operator_token_file,'scoped control activation credential required')
    response=https(args.control.rstrip('/')+'/management/runner/v1/backend-profiles/'+plan['id']+'/activate',ca=args.control_ca,
                   token=private(args.operator_token_file).read_text().strip(),operator=True,
                   body={'executor_host_binding':plan['executor_host_binding'],'challenge':plan['challenge'],'specification_sha256':plan['specification_sha256'],'observation':observation})
    write_new(receipt_path,response);print(canonical(response).decode())
if __name__=='__main__':main()
