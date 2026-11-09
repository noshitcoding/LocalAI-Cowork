#!/usr/bin/env python3
"""Plan/inspect/activate explicitly owned physical environment controllers."""
import argparse,hashlib,importlib.util,json,os,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'server/cowork-provisioner'))
import external_vm
spec=importlib.util.spec_from_file_location('_cowork_controller_https',ROOT/'scripts/enterprise-backend-profile.py');http=importlib.util.module_from_spec(spec);spec.loader.exec_module(http)
class Probe:
    def __init__(self,c=None):
        self.docker=None
        if c is not None and c['backend']=='open_shell':
            path=c['parent_backend_configuration_file'];http.require(path in c['credential_pins'],'original parent profile configuration must be independently pinned')
            cfg,sha=http.configuration(path);physical=http.physical(cfg,routing=False);m=http.vm_module();root=Path(cfg['vm_root']);metadata=m.load(root)
            http.require(sha==c['credential_pins'][path] and all(cfg[k]==c[k] for k in ('organisation_id','instance_id','host_id','operator_state_id')) and physical['vm_id']==c['vm_id'] and physical['daemon_id']==c['guest_daemon_id'] and metadata['disk_virtual_gib']==c['parent_disk_virtual_gib'],'actual original OpenShell parent differs')
            self.docker=['docker','--host',metadata['docker_endpoint'],'--tlsverify','--tlscacert',str(root/'ca.pem'),'--tlscert',str(root/'client.pem'),'--tlskey',str(root/'client-key.pem')]
    def run(self,argv,data=None,env=None,timeout=90):
        if argv and argv[0]=='docker':http.require(self.docker is not None,'default Docker context is forbidden for controller inspection');argv=[*self.docker,*argv[1:]]
        selected=dict(os.environ if env is None else env)
        for key in ('DOCKER_HOST','DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH','DOCKER_API_VERSION','KUBECONFIG','KUBERNETES_MASTER'):selected.pop(key,None)
        value=subprocess.run(argv,input=data,env=selected,capture_output=True,timeout=timeout)
        http.require(value.returncode==0,'original controller probe failed: '+Path(argv[0]).name);return value.stdout
def credentials(path):
    value=json.loads(http.private(path).read_bytes());http.require(set(value)=={'url','ca_file','access_token_file','operator_token_file'},'explicit controller HTTPS credentials required')
    for key in ('ca_file','access_token_file','operator_token_file'):http.private(value[key])
    http.require(value['url'].startswith('https://') and value['url'].rstrip('/').endswith('/management/v1'),'exact HTTPS management API required');return value
def callback(config,path,body,operator=False):
    public=config['url'].rstrip('/');base=public.removesuffix('/management/v1')+'/management/runner/v1' if operator else public
    token=Path(config['operator_token_file' if operator else 'access_token_file']).read_text().strip()
    return http.https(base+path,ca=config['ca_file'],token=token,operator=operator,body=body)
def specification(c,sha):return {**{k:c[k] for k in ('format','backend','organisation_id','instance_id','host_id','operator_state_id','controller_id','vm_id','guest_hardware_uuid','guest_daemon_id','controller_identity','limits')},'configuration_sha256':sha}
def observe(c,plan=None):
    if c['backend']=='open_shell':
        import openshell_environment
        physical=openshell_environment.controller_observation(Probe(c),c)
    else:physical=external_vm.observe(Probe(),c)
    value={**specification(c,hashlib.sha256(http.canonical(c)).hexdigest()),'actual_hypervisor_verified':True,'actual_guest_identity_verified':True,'physical_limits_verified':True,'physical_identity':physical}
    if plan:value.update(format='cowork-physical-environment-controller-v1',plan_id=plan['id'],challenge=plan['challenge'],configuration_sha256=plan['specification']['configuration_sha256'])
    return value
def original(args):
    value=json.loads(http.private(args.plan).read_bytes());c,sha=external_vm.configuration(args.config)
    http.require(value['format']=='cowork-environment-controller-plan-v1' and value['configuration_sha256']==sha and value['manager_plan']['specification']==specification(c,sha),'original physical controller plan/configuration changed')
    return value,c
def main():
    parser=argparse.ArgumentParser();sub=parser.add_subparsers(dest='command',required=True)
    for name in ('plan','inspect','activate','pair-provisioner'):
        p=sub.add_parser(name);p.add_argument('--config',required=True);p.add_argument('--output',required=True)
        if name=='plan':p.add_argument('--instance-revision',type=int,required=True);p.add_argument('--controller-revision',type=int,required=True);p.add_argument('--request-id',required=True)
        else:p.add_argument('--plan',required=True)
        if name in ('plan','activate'):p.add_argument('--manager-config',required=True)
        if name=='pair-provisioner':p.add_argument('--provisioner-config',required=True);p.add_argument('--activation-receipt',required=True)
    args=parser.parse_args();c,sha=external_vm.configuration(args.config)
    if args.command=='plan':
        external_vm.observe(Probe(),c) if c['backend']!='open_shell' else observe(c)
        request={'request_id':args.request_id,'expected_instance_revision':args.instance_revision,'expected_controller_revision':args.controller_revision,'specification':specification(c,sha),'confirmation':'ENROLL_EXACT_ENVIRONMENT_CONTROLLER'}
        plan=callback(credentials(args.manager_config),'/organisations/'+c['organisation_id']+'/instances/'+c['instance_id']+'/environment-controllers',request)
        http.require(plan['specification']==request['specification'],'original manager controller plan differs');http.write_new(args.output,{'format':'cowork-environment-controller-plan-v1','manager_plan':plan,'configuration_sha256':sha});return
    value,c=original(args);plan=value['manager_plan']
    if args.command=='pair-provisioner':
        receipt=json.loads(http.private(args.activation_receipt).read_bytes());http.require(receipt['format']=='cowork-environment-controller-activation-v1' and receipt['plan_id']==plan['id'] and receipt['observation']['configuration_sha256']==sha,'original controller activation receipt differs')
        provisioner=json.loads(http.private(args.provisioner_config).read_bytes());http.require(provisioner['backend_profile']['specification']==plan['actor_authority']['backend_profile']['specification'],'original provisioner parent profile differs')
        controllers=provisioner.setdefault('environment_controllers',{});http.require(c['backend'] not in controllers,'controller configuration already exists; explicit revision review required');controllers[c['backend']]={'configuration_file':str(Path(args.config)),'configuration_sha256':sha};http.write_new(args.output,provisioner);return
    observation_path=Path(args.output).with_suffix('.original-observation.json')
    if args.command=='activate' and observation_path.exists():observation=json.loads(http.private(observation_path).read_bytes());http.require(observation['plan_id']==plan['id'] and observation['configuration_sha256']==sha,'original activation observation differs')
    else:
        observation=observe(c,plan)
        if args.command=='activate':http.write_new(observation_path,observation)
    if args.command=='inspect':http.write_new(args.output,observation);return
    body={'executor_host_binding':plan['executor_host_binding'],'challenge':plan['challenge'],'specification_sha256':plan['specification_sha256'],'observation':observation}
    final=callback(credentials(args.manager_config),'/environment-controllers/'+plan['id']+'/activate',body,True);http.require(final==observation,'original controller activation readback differs')
    http.write_new(args.output,{'format':'cowork-environment-controller-activation-v1','plan_id':plan['id'],'specification_sha256':plan['specification_sha256'],'observation':final})
if __name__=='__main__':main()
