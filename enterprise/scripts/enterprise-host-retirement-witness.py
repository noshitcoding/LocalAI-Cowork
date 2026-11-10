#!/usr/bin/env python3
"""Independent RSA witness for actual original QEMU VM physical retirement.

Operator-only plan/retire/verify commands. No polling disappearance, lease expiry,
replacement node or guest statement can produce this retirement witness.
"""
import argparse,base64,hashlib,importlib.util,json,os,re,subprocess,sys,tempfile,uuid
from datetime import datetime,timezone
from pathlib import Path

def require(ok,message):
    if not ok:raise RuntimeError(message)
def canonical(value):return json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
def sha(value):return hashlib.sha256(value).hexdigest()
def private(path,directory=False):
    path=Path(path);require(path.is_absolute() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents)
        and (path.is_dir() if directory else path.is_file()),'absolute original private path required')
    if os.name=='posix':require(path.stat().st_uid==os.getuid() and not path.stat().st_mode&0o077,'owner-only original controller path required')
    return path
def read(path):
    path=private(path);require(path.stat().st_size<=2097152,'bounded original controller document required');return json.loads(path.read_bytes())
def write(path,value):
    path=Path(path);private(path.parent,True);require(not path.is_symlink(),'unsafe new witness path')
    with os.fdopen(os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600),'wb') as stream:stream.write(canonical(value));stream.flush();os.fsync(stream.fileno())
def controller():
    path=Path(__file__).resolve().parents[1]/'deploy/executor-vm/local_executor_vm.py'
    spec=importlib.util.spec_from_file_location('_cowork_retirement_controller',path);module=importlib.util.module_from_spec(spec);sys.modules[spec.name]=module;spec.loader.exec_module(module)
    return module
def source_pins():
    root=Path(__file__).resolve().parents[1]
    return {name:sha((root/name).read_bytes()) for name in ('scripts/enterprise-host-retirement-witness.py','deploy/executor-vm/local_executor_vm.py','deploy/executor-vm/capacity_pool.py')}
def identity(value):
    require(value['format']=='cowork-original-host-retirement-intent-v1','original manager/controller retirement intent required')
    for key in ('request_id','operation_id','organisation_id','instance_id','host_id','vm_id','operator_state_id','execution_epoch'):
        require(str(uuid.UUID(value[key]))==value[key] and uuid.UUID(value[key]).int!=0,'original retirement UUID differs')
    require(isinstance(value['daemon_id'],str) and len(value['daemon_id'])>=12 and value['minimum_isolation']=='virtual_machine','original VM daemon/isolation binding required')
    require(isinstance(value['original_runtime_ids'],list) and value['original_runtime_ids'] and len(value['original_runtime_ids'])<=10000
        and all(isinstance(v,str) and re.fullmatch('[A-Za-z0-9:._/-]{1,128}',v) for v in value['original_runtime_ids'])
        and len(set(value['original_runtime_ids']))==len(value['original_runtime_ids']),'original held runtime IDs required')
def plan(args):
    module=controller();root=private(args.vm_root,True);intent=read(args.intent);identity(intent);value=module.load(root)
    require(value['id']==intent['vm_id'] and value['organisation_id']==intent['organisation_id'] and value['operator_state_id']==intent['operator_state_id'],'original VM assignment differs')
    activation=read(args.backend_receipt);observation=activation['observation'];binding=observation['executor_host_binding']
    require(activation['format']=='cowork-backend-activation-v1' and activation['instance_id']==intent['instance_id']
        and activation['organisation_id']==intent['organisation_id'] and observation['physical']['vm_id']==intent['vm_id']
        and observation['physical']['daemon_id']==intent['daemon_id'] and observation['physical']['mutual_tls_verified'] is True
        and binding['host_id']==intent['host_id'] and binding['operator_state_id']==intent['operator_state_id'],'original actual manager activation receipt differs')
    require(module.process(root) is None,'retirement plan requires the original VM already physically stopped')
    with module.capacity_pool.locked_pool() as pool:
        pool.require_registered(root,value);pool.check_inventory(module.load);allocation=pool.data['allocations'][value['id']]
        require(allocation['adopted'] is False and allocation['identity']['version']==2 and str(root) not in pool.data['plan']['existing_vm_roots'],'adopted/protected VM retirement forbidden')
        result={'format':'cowork-controller-retirement-plan-v1','intent':intent,'intent_sha256':sha(canonical(intent)),'vm_root':str(root),
            'metadata_sha256':module.retirement_metadata_sha(root),'capacity_revision':pool.data['revision'],'allocation':allocation,'source_pins':source_pins(),
            'backend_activation_receipt_sha256':sha(canonical(activation))}
    write(args.output,result)
def run(args):return subprocess.run(args,check=True,capture_output=True,timeout=60).stdout
def retire(args):
    require(args.confirmation=='RETIRE_EXACT_ORIGINAL_STOPPED_VM','explicit original stopped VM retirement confirmation required')
    value=read(args.plan);require(value['format']=='cowork-controller-retirement-plan-v1' and value['source_pins']==source_pins(),'original controller sources changed')
    identity(value['intent']);require(sha(canonical(value['intent']))==value['intent_sha256'],'original retirement intent changed')
    module=controller();root=Path(value['vm_root'])
    durable=Path(args.plan).with_name(Path(args.plan).name+'.witness.json')
    if durable.exists():
        original=read(durable);require(original['payload']['plan_sha256']==sha(canonical(value)),'durable original witness differs')
        if Path(args.output).exists():require(read(args.output)==original,'original output witness differs')
        else:write(args.output,original)
        return
    revision=args.expected_capacity_revision if args.expected_capacity_revision is not None else value['capacity_revision']
    require(type(revision) is int and revision>=value['capacity_revision'],'explicit current original retirement CAS required')
    verifier=lambda record,data,phase:(require(record['allocation']==value['allocation'] and record['metadata_sha256']==value['metadata_sha256'],'original retirement allocation changed'),module.retirement_quiet(record))
    with module.capacity_pool.locked_pool(recover_pending_retirement=True,expected_retirement_revision=revision,
        expected_retirement_binding={'id':value['intent']['vm_id'],'root':str(root),'metadata_sha256':value['metadata_sha256']},retirement_recovery_verifier=verifier) as pool:
        prior=pool.data.get('retirements',{}).get(value['intent']['vm_id'])
        already=prior is not None and prior['state']=='retired'
        if already:pool.check_tombstones()
    if not already:module.retire(root,value['intent']['vm_id'],value['metadata_sha256'],revision)
    with module.capacity_pool.locked_pool() as pool:
        pool.check_tombstones();record=pool.data['retirements'][value['intent']['vm_id']]
        require(record['state']=='retired' and record['allocation']==value['allocation'] and record['metadata_sha256']==value['metadata_sha256']
            and record['retirement_id'],'actual original controller retirement differs')
        deletion=module.capacity_pool.require_root_deletion_witness(pool.root,record)
        require(record['deletion_witness_sha256']==deletion,'actual inode deletion witness differs')
        # The controller auditor checks its original process inventory, file
        # descriptors, mounts, QMP socket and inode deletion witnesses.
        module.retirement_quiet(record)
        payload={'format':'cowork-physical-host-retirement-v1','intent':value['intent'],'intent_sha256':value['intent_sha256'],
            'plan_sha256':sha(canonical(value)),'retirement_id':record['retirement_id'],'deletion_witness_sha256':deletion,
            'original_allocation_sha256':sha(canonical(record['allocation'])),'controller_source_pins':value['source_pins'],
            'controller_capacity_revision':pool.data['revision'],'physical_retirement_verified':True,'observed_at':datetime.now(timezone.utc).isoformat()}
    key=private(args.signing_key)
    description=run(['openssl','rsa','-in',str(key),'-text','-noout']).decode()
    bits=re.search(r'Private-Key: \(([0-9]+) bit',description);require(bits and int(bits[1])>=3072,'independent RSA3072+ key required')
    with tempfile.TemporaryDirectory(prefix='cowork-original-retirement-') as temporary:
        path=Path(temporary);data=path/'payload';data.write_bytes(canonical(payload));signature=run(['openssl','dgst','-sha256','-sign',str(key),str(data)])
        public=run(['openssl','pkey','-in',str(key),'-pubout','-outform','DER'])
    witness={'payload':payload,'signature':base64.b64encode(signature).decode(),'public_key_sha256':sha(public)}
    write(durable,witness);write(args.output,witness)
def verify(value,intent,public_key):
    """Consumer: the expected intent/key must come from trusted original state."""
    identity(intent);payload=value['payload']
    require(payload['format']=='cowork-physical-host-retirement-v1' and payload['intent']==intent and payload['intent_sha256']==sha(canonical(intent))
        and payload['physical_retirement_verified'] is True,'witness differs from original manager runtime/host/lease scope')
    public_key=Path(public_key);require(public_key.is_absolute() and public_key.is_file() and not public_key.is_symlink(),'trusted enrolled controller public key required')
    for key in ('intent_sha256','plan_sha256','deletion_witness_sha256','original_allocation_sha256'):
        require(re.fullmatch('[a-f0-9]{64}',payload[key]),'invalid original retirement digest')
    require(str(uuid.UUID(payload['retirement_id']))==payload['retirement_id'] and uuid.UUID(payload['retirement_id']).int!=0
        and type(payload['controller_capacity_revision']) is int and payload['controller_capacity_revision']>0,'invalid actual controller retirement identity/revision')
    if os.name=='posix':require(not public_key.stat().st_mode&0o022,'trusted controller key writable by other users')
    description=run(['openssl','rsa','-pubin','-in',str(public_key),'-text','-noout']).decode()
    bits=re.search(r'Public-Key: \(([0-9]+) bit',description);require(bits and int(bits[1])>=3072,'trusted enrolled RSA3072+ key required')
    public=run(['openssl','pkey','-pubin','-in',str(public_key),'-outform','DER']);require(sha(public)==value['public_key_sha256'],'independent enrolled controller key differs')
    with tempfile.TemporaryDirectory(prefix='cowork-verify-retirement-') as temporary:
        path=Path(temporary);(path/'payload').write_bytes(canonical(payload));(path/'signature').write_bytes(base64.b64decode(value['signature'],validate=True))
        run(['openssl','dgst','-sha256','-verify',str(public_key),'-signature',str(path/'signature'),str(path/'payload')])
    return payload
def main():
    parser=argparse.ArgumentParser();sub=parser.add_subparsers(dest='command',required=True)
    prepare=sub.add_parser('plan');prepare.add_argument('--vm-root',required=True);prepare.add_argument('--intent',required=True);prepare.add_argument('--backend-receipt',required=True);prepare.add_argument('--output',required=True)
    apply=sub.add_parser('retire');apply.add_argument('--plan',required=True);apply.add_argument('--signing-key',required=True);apply.add_argument('--confirmation',required=True);apply.add_argument('--expected-capacity-revision',type=int);apply.add_argument('--output',required=True)
    inspect=sub.add_parser('verify');inspect.add_argument('--witness',required=True);inspect.add_argument('--intent',required=True);inspect.add_argument('--public-key',required=True)
    args=parser.parse_args()
    if args.command=='plan':plan(args)
    elif args.command=='retire':retire(args)
    else:print(json.dumps(verify(read(args.witness),read(args.intent),args.public_key)))
if __name__=='__main__':main()
