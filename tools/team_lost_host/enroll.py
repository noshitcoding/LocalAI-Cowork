#!/usr/bin/env python3
"""Build an explicit private enrollment from actual manager/controller originals.

Read-only on controller data; never opens keys, creates a VM or invents an epoch.
The Rust host-trust command separately checks actual DockerInfo and RSA evidence.
"""
import argparse,base64,hashlib,json,os,re,stat,subprocess,tempfile,uuid
from pathlib import Path

def require(v,msg):
    if not v:raise RuntimeError(msg)
def canonical(v):return json.dumps(v,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode('utf-8')
def sha(v):return hashlib.sha256(v).hexdigest()
def uid(v):
    require(isinstance(v,str) and str(uuid.UUID(v))==v and uuid.UUID(v).int!=0,'exact nonnil original UUID required');return v
def read(path,private=True,limit=2097152):
    path=Path(path);require(path.is_absolute() and all(not p.is_symlink() for p in (path,*path.parents)),'original absolute no-symlink path required')
    flags=os.O_RDONLY|getattr(os,'O_NOFOLLOW',0)|getattr(os,'O_CLOEXEC',0)
    with os.fdopen(os.open(path,flags),'rb') as f:
        before=os.fstat(f.fileno());require(stat.S_ISREG(before.st_mode) and before.st_nlink==1 and before.st_size<=limit,'bounded original single-link file required')
        if os.name=='posix':require(before.st_uid==os.getuid() and not before.st_mode&(0o077 if private else 0o022),'owner-controlled original file required')
        raw=f.read(limit+1);after=os.fstat(f.fileno());require(len(raw)<=limit and len(raw)==before.st_size and (before.st_size,before.st_mtime_ns,before.st_ino)==(after.st_size,after.st_mtime_ns,after.st_ino),'original file changed while reading')
    return raw
def doc(path):return json.loads(read(path))
def run(args):return subprocess.run(args,capture_output=True,check=True,timeout=60).stdout

def main():
    p=argparse.ArgumentParser();p.add_argument('--identity',required=True);p.add_argument('--signature',required=True);p.add_argument('--backend-activation',required=True);p.add_argument('--capacity-ledger',required=True);p.add_argument('--enrolled-public-key',required=True);p.add_argument('--controller-source-root',required=True);p.add_argument('--manager-producer',required=True);p.add_argument('--profile-id',required=True);p.add_argument('--team-id',required=True);p.add_argument('--node-map');p.add_argument('--node-name');p.add_argument('--output',required=True);a=p.parse_args()
    identity=doc(a.identity);signature=doc(a.signature);activation=doc(a.backend_activation);pool=doc(a.capacity_ledger)
    require(identity['format']=='cowork-manager-host-runtime-identity-v1' and identity['execution_required'] is True,'actual enforced manager origin required')
    require(signature['format']=='cowork-manager-host-runtime-identity-signature-v1' and signature['identity_sha256']==sha(canonical(identity)),'original manager signature differs')
    public=Path(a.enrolled_public_key);read(public);der=run(['openssl','pkey','-pubin','-in',str(public),'-outform','DER'])
    bits=run(['openssl','rsa','-pubin','-in',str(public),'-text','-noout']).decode();m=re.search(r'Public-Key: \(([0-9]+) bit',bits);require(m and 3072<=int(m[1])<=8192,'independently enrolled RSA3072..8192 key required')
    require(signature['public_key_sha256']==sha(der),'enrolled key differs from manager signature')
    with tempfile.TemporaryDirectory(prefix='cowork-enroll-original-') as folder:
        data=Path(folder)/'identity';sig=Path(folder)/'signature';data.write_bytes(canonical(identity));sig.write_bytes(base64.b64decode(signature['signature'],validate=True));run(['openssl','dgst','-sha256','-verify',str(public),'-signature',str(sig),str(data)])
    producer=sha(read(a.manager_producer,False));require(signature['producer_source_sha256']==producer,'actual manager producer source changed')
    for field in ('organisation_id','instance_id','host_id','vm_id','operator_state_id','execution_epoch'):uid(identity[field])
    require(activation['format']=='cowork-backend-activation-v1' and sha(canonical(activation))==identity['backend_activation_receipt_sha256'] and activation['instance_id']==identity['instance_id'] and activation['organisation_id']==identity['organisation_id'] and activation['profile_revision']==identity['backend_profile_revision'],'actual original backend activation differs')
    require(activation['observation']['executor_host_binding']==identity['executor_host_binding'] and activation['observation']['physical']['vm_id']==identity['vm_id'] and activation['observation']['physical']['daemon_id']==identity['daemon_id'] and activation['observation']['physical']['mutual_tls_verified'] is True,'actual VM/engine observation differs')
    allocation=pool['allocations'][identity['vm_id']];meta=allocation['identity']
    require(allocation['adopted'] is False and meta['version']==2 and meta['id']==identity['vm_id'] and meta['organisation_id']==identity['organisation_id'] and meta['operator_state_id']==identity['operator_state_id'] and meta['root'] not in pool['plan']['existing_vm_roots'],'original owned v2 allocation required; protected/adopted VM forbidden')
    require(bool(a.node_map)==bool(a.node_name),'exact current lifecycle node map and name must be supplied together')
    node_uid=None
    if a.node_map:
        mapping=doc(a.node_map);require(mapping['format']==1,'original lifecycle map format differs');nodes=[n for n in mapping['nodes'] if n['name']==a.node_name];require(len(nodes)==1 and nodes[0]['host_id']==identity['host_id'],'original lifecycle node mapping differs');node_uid=uid(nodes[0]['uid'])
    source=Path(a.controller_source_root);pins={name:sha(read(source/name,False)) for name in ('scripts/enterprise-host-retirement-witness.py','deploy/executor-vm/local_executor_vm.py','deploy/executor-vm/capacity_pool.py')}
    raw=canonical(allocation);require(len(raw)<=65536,'original allocation exceeds bound')
    profile={'format':'localai-team-original-host-trust/1','id':uid(a.profile_id),'team_id':uid(a.team_id),'manager_organisation_id':identity['organisation_id'],'binding':{'format':1,'host_id':identity['host_id'],'engine_id':identity['daemon_id'],'node_name':a.node_name,'node_uid':node_uid},'instance_id':identity['instance_id'],'vm_id':identity['vm_id'],'operator_state_id':identity['operator_state_id'],'execution_epoch':identity['execution_epoch'],'daemon_id':identity['daemon_id'],'backend_activation':activation,'original_runtime_identity':identity,'original_runtime_signature':signature,'manager_identity_producer_sha256':producer,'allocation_base64':base64.b64encode(raw).decode(),'allocation_sha256':sha(raw),'controller_source_pins':pins,'public_key_der':base64.b64encode(der).decode(),'public_key_sha256':sha(der)}
    output=Path(a.output);require(output.is_absolute() and all(not x.is_symlink() for x in output.parents) and output.parent.is_dir(),'explicit new private destination required')
    if os.name=='posix':require(output.parent.stat().st_uid==os.getuid() and not output.parent.stat().st_mode&0o077,'private output directory required')
    raw=canonical(profile);require(len(raw)<=192*1024,'profile exceeds bound')
    with os.fdopen(os.open(output,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600),'wb') as f:f.write(raw);f.flush();os.fsync(f.fileno())
    print(json.dumps({'profile_id':profile['id'],'profile_file_sha256':sha(raw),'registered':False,'active':False}))
if __name__=='__main__':main()
