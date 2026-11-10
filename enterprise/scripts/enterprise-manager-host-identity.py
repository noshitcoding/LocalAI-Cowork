#!/usr/bin/env python3
"""Export the actual manager epoch after independent original VM inspection.

Run on the trusted parent controller before V2 original-host enrollment. The
consumer enrolls the actual exported bytes/key outside the guest; no guessed
epoch, replacement identity or witness-supplied trust root is accepted.
"""
import argparse,base64,hashlib,importlib.util,json,re,subprocess,tempfile
from pathlib import Path
spec=importlib.util.spec_from_file_location('_cowork_identity_backend',Path(__file__).with_name('enterprise-backend-profile.py'));backend=importlib.util.module_from_spec(spec);spec.loader.exec_module(backend)
def main():
    p=argparse.ArgumentParser();p.add_argument('--config',required=True);p.add_argument('--manager-url',required=True);p.add_argument('--manager-ca',required=True);p.add_argument('--operator-token-file',required=True);p.add_argument('--signing-key',required=True);p.add_argument('--enrolled-public-key',required=True);p.add_argument('--output',required=True);a=p.parse_args()
    journal=Path(a.output+'.original.json');config_sha=hashlib.sha256(backend.private(a.config).read_bytes()).hexdigest();public_sha=hashlib.sha256(backend.private(a.enrolled_public_key).read_bytes()).hexdigest()
    if journal.exists():
        original=json.loads(backend.private(journal).read_bytes());backend.require(original['configuration_sha256']==config_sha and original['enrolled_public_file_sha256']==public_sha and original['manager_url']==a.manager_url,'original manager identity export authority changed')
        for path,data in ((Path(a.output),backend.canonical(original['identity'])),(Path(a.output+'.signature.json'),backend.canonical(original['signature']))):
            if path.exists():backend.require(backend.private(path).read_bytes()==data,'original manager identity output differs')
            else:backend.write_new(path,data)
        return
    c,_=backend.configuration(a.config);backend.require(c['kind']=='vm_compose','original owned VM configuration required');physical=backend.physical(c)
    binding={'version':1,'host_id':c['host_id'],'trust_domain':c['organisation_id'],'daemon_id':physical['daemon_id'],'operator_state_id':c['operator_state_id']}
    value=backend.https(a.manager_url.rstrip('/')+'/management/runner/v1/host-runtime-identity/'+c['organisation_id']+'/'+c['instance_id'],ca=str(backend.private(a.manager_ca)),token=backend.private(a.operator_token_file).read_text().strip(),operator=True,body={'executor_host_binding':binding})
    backend.require(value['format']=='cowork-manager-host-runtime-identity-v1' and value['executor_host_binding']==binding and value['vm_id']==physical['vm_id'] and value['organisation_id']==c['organisation_id'] and value['instance_id']==c['instance_id'],'actual manager/parent VM assignment differs')
    import uuid
    backend.require(str(uuid.UUID(value['execution_epoch']))==value['execution_epoch'] and uuid.UUID(value['execution_epoch']).int!=0,'actual original manager epoch unavailable')
    key=backend.private(a.signing_key);public=backend.private(a.enrolled_public_key)
    run=lambda argv:subprocess.run(argv,check=True,capture_output=True,timeout=30).stdout
    description=run(['openssl','rsa','-in',str(key),'-text','-noout']).decode();bits=re.search(r'Private-Key: \(([0-9]+) bit',description);backend.require(bits and int(bits[1])>=3072,'independent RSA3072+ enrollment key required')
    derived=run(['openssl','pkey','-in',str(key),'-pubout','-outform','DER']);enrolled=run(['openssl','pkey','-pubin','-in',str(public),'-outform','DER']);backend.require(derived==enrolled,'separately enrolled parent key differs')
    data=backend.canonical(value)
    with tempfile.TemporaryDirectory(prefix='cowork-original-manager-identity-') as temporary:
        path=Path(temporary)/'identity';path.write_bytes(data);signature=run(['openssl','dgst','-sha256','-sign',str(key),str(path)])
    seal={'format':'cowork-manager-host-runtime-identity-signature-v1','identity_sha256':hashlib.sha256(data).hexdigest(),'public_key_sha256':hashlib.sha256(enrolled).hexdigest(),'signature':base64.b64encode(signature).decode(),'producer_source_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    backend.write_new(journal,{'configuration_sha256':config_sha,'enrolled_public_file_sha256':public_sha,'manager_url':a.manager_url,'identity':value,'signature':seal})
    backend.write_new(a.output,data);backend.write_new(a.output+'.signature.json',seal)
if __name__=='__main__':main()
