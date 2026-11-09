#!/usr/bin/env python3
"""Plan, renew, roll out and reconcile exact operator-owned TLS identities.

Linux operator host only. No deployment occurs for plan. Apply is explicit;
maintain is intended for an operator-installed systemd timer. Unknown command
outcomes remain pending until physical readback succeeds, never replayed blindly.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import socket
import ssl
import subprocess
import time
import uuid
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

def require(value, error):
    if not value:
        raise RuntimeError(error)

def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode()

def sha(data):
    return hashlib.sha256(data).hexdigest()

def digest(path):
    return sha(path.read_bytes())

def safe(path, directory=False):
    path = Path(path)
    require(path.is_absolute() and not path.is_symlink() and all(not p.is_symlink() for p in path.parents), 'unsafe operator path')
    require(path.is_dir() if directory else path.is_file(), 'missing operator path')
    require(path.stat().st_uid == os.getuid() and path.stat().st_mode & (0o077 if directory else 0o022) == 0, 'unsafe operator ownership/permissions')
    return path

def private(path):
    path = safe(path)
    require(path.stat().st_mode & 0o077 == 0, 'private key/configuration requires mode 0600')
    return path

def material_path(config,path):
    if config['kind']!='control':
        return private(path)
    # Compose secrets are bind files: the non-root TLS gateway needs its
    # existing read mode. A 0444 source is safe only below an owner-only parent.
    path=safe(path)
    safe(path.parent,True)
    require(path.stat().st_mode & 0o777 in (0o600,0o444) and path.stat().st_nlink==1,'control TLS source requires private parent and reviewed runtime read mode')
    return path

def run(argv, data=None, timeout=120):
    env = dict(os.environ)
    for key in ('DOCKER_HOST','DOCKER_CONTEXT','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH','DOCKER_TLS','DOCKER_API_VERSION'):
        env.pop(key, None)
    result = subprocess.run(argv, input=data, capture_output=True, timeout=timeout, env=env)
    require(result.returncode == 0, 'operator command failed: ' + Path(argv[0]).name)
    return result.stdout

def atomic(path, data, mode=0o600):
    if not isinstance(data, bytes):
        data = canonical(data)
    temp = path.with_name('.' + path.name + '.' + uuid.uuid4().hex)
    with os.fdopen(os.open(temp, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'wb') as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    temp.chmod(mode)
    os.replace(temp, path)
    fd = os.open(path.parent, os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)

@contextmanager
def locked(path):
    import fcntl
    require(not path.is_symlink(), 'unsafe lifecycle lock')
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('operator state is in use; pause its provisioner before certificate maintenance') from None
        yield
    finally:
        os.close(fd)

def docker(*argv):
    return run(['docker','--host','unix:///var/run/docker.sock',*argv])

def compose(config, *argv):
    selected = config['consumer']
    args = ['docker','--host','unix:///var/run/docker.sock','compose','--project-name',selected['project']]
    for path in selected['files']:
        args += ['-f',str(safe(path))]
    if selected.get('env_file'):
        args += ['--env-file',str(private(selected['env_file']))]
    return run([*args,*argv], timeout=180)

def inventory(config):
    ids = compose(config,'ps','--all','--quiet',*config['consumer']['services']).decode().split()
    require(ids, 'selected deployment has no original containers')
    result = json.loads(docker('inspect',*ids))
    selected = config['consumer']
    for row in result:
        labels = row['Config'].get('Labels') or {}
        require(labels.get('com.docker.compose.project') == selected['project'] and labels.get('com.docker.compose.service') in selected['services'], 'container ownership mismatch')
        require(re.fullmatch('sha256:[a-f0-9]{64}',row['Image']), 'container image identity absent')
    require({r['Config']['Labels']['com.docker.compose.service'] for r in result} == set(selected['services']), 'selected services not completely installed')
    return [{'id':r['Id'],'image':r['Image'],'service':r['Config']['Labels']['com.docker.compose.service'],
             'running':r['State']['Running'],'mounts':sorted(m['Source'] for m in r['Mounts'])} for r in sorted(result,key=lambda r:r['Id'])]

def configuration(path):
    raw = private(path).read_bytes()
    config = json.loads(raw)
    require(config.get('format') == 1 and config.get('kind') in ('control','vm'), 'unsupported certificate lifecycle profile')
    selected = config['consumer']
    require(re.fullmatch('[a-z0-9][a-z0-9_-]{0,63}',selected['project']) and selected['files'] and selected['services']
            and all(re.fullmatch('[a-zA-Z0-9_-]{1,80}',v) for v in selected['services']), 'invalid exact deployment selection')
    require(1 <= config.get('renew_before_days',14) <= 60, 'invalid renewal window')
    safe(config['operator_state_directory'],True)
    if config['kind'] == 'control':
        require(re.fullmatch('[a-zA-Z0-9.-]{1,253}',config['hostname']), 'invalid control hostname')
        material_path(config,config['certificate_file'])
        material_path(config,config['private_key_file'])
        origin = urlsplit(config['probe_origin'])
        require(origin.scheme == 'https' and origin.hostname == config['hostname'] and origin.path in ('','/')
                and not origin.username and not origin.query and not origin.fragment, 'control probe must use exact verified HTTPS identity')
        require(config.get('probe_path','/readyz').startswith('/') and '\r' not in config.get('probe_path',''), 'invalid probe path')
    else:
        root = safe(config['vm_root'],True)
        require(re.fullmatch('cowork-executor-[a-f0-9]{32}',root.name), 'invalid VM root identity')
        value = json.loads(private(root/'vm.json').read_bytes())
        if value.get('version') == 1 and 'operator_state_id' not in value:
            value['operator_state_id'] = str(uuid.uuid5(uuid.UUID(value['id']), 'cowork-operator-state-v1'))
        require(str(uuid.UUID(value['id'])) == value['id'] and root.name.endswith(uuid.UUID(value['id']).hex)
                and value.get('bootstrap_state','ready') == 'ready', 'VM metadata scope mismatch')
        require(re.fullmatch('tcp://localhost:[0-9]+',value['docker_endpoint']), 'VM endpoint is not local')
        private(root/'ssh-key')
        safe(root/'known_hosts')
        for name in ('ca.pem','ca-key.pem','server.pem','server-key.pem','client.pem','client-key.pem'):
            private(root/name)
        executor = json.loads(private(config['provisioner_configuration_file']).read_bytes())['executor_host']
        require(executor['trust_domain'] == value['organisation_id'] and executor['operator_state_id'] == value['operator_state_id']
                and executor['endpoint'] == value['docker_endpoint']
                and all(executor[k] == str(root/n) for k,n in (('ca_file','ca.pem'),('cert_file','client.pem'),('key_file','client-key.pem'))), 'provisioner/VM TLS authority differs')
        config['_vm'],config['_executor'] = value,executor
    return config,sha(raw)

def vm_ssh(config, command, data=None):
    root,value = Path(config['vm_root']),config['_vm']
    return run(['ssh','-i',str(root/'ssh-key'),'-p',str(value['ssh_port']),'-o','BatchMode=yes','-o','IdentitiesOnly=yes',
                '-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(root/'known_hosts'),'-o','ConnectTimeout=5',
                'cowork@127.0.0.1',command], data=data, timeout=120)

def vm_docker(config, material, *argv):
    return json.loads(run(['docker','--host',config['_vm']['docker_endpoint'],'--tlsverify','--tlscacert',str(material/'ca.pem'),
                          '--tlscert',str(material/'client.pem'),'--tlskey',str(material/'client-key.pem'),*argv]))

def vm_inventory(config):
    # SSH remains available after certificate expiry and verifies the daemon's
    # original identity without trusting an expired public transport.
    info = json.loads(vm_ssh(config,"sudo docker info --format '{{json .}}'"))
    expected = config['_executor']
    require(info.get('ID') == expected['daemon_id'] and 'cowork.role=executor' in info.get('Labels',[])
            and 'cowork.trust-domain='+expected['trust_domain'] in info.get('Labels',[]), 'VM daemon/trust-domain identity mismatch')
    ids = vm_ssh(config,'sudo docker ps --all --quiet --no-trunc').decode().split()
    rows = json.loads(vm_ssh(config,'sudo docker inspect ' + ' '.join(ids))) if ids else []
    require(all(not r['State']['Running'] for r in rows), 'stop guest instances through management before VM certificate maintenance')
    return {'daemon_id':info['ID'],'containers':[{'id':r['Id'],'image':r['Image'],'running':r['State']['Running']} for r in sorted(rows,key=lambda r:r['Id'])]}

def material_files(config):
    if config['kind'] == 'control':
        return {'certificate.pem':Path(config['certificate_file']),'key.pem':Path(config['private_key_file'])}
    root = Path(config['vm_root'])
    return {name:root/name for name in ('ca.pem','ca-key.pem','server.pem','server-key.pem','client.pem','client-key.pem')}

def generate(config, stage):
    days = '397' if config['kind'] == 'control' else '90'
    if config['kind'] == 'control':
        run(['openssl','req','-x509','-newkey','rsa:3072','-nodes','-sha256','-days',days,'-subj','/CN='+config['hostname'],
             '-addext','subjectAltName=DNS:'+config['hostname'],'-addext','extendedKeyUsage=serverAuth','-keyout',str(stage/'key.pem'),'-out',str(stage/'certificate.pem')])
    else:
        run(['openssl','req','-x509','-newkey','rsa:3072','-nodes','-sha256','-days','397','-subj','/CN=Cowork executor CA',
             '-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',str(stage/'ca-key.pem'),'-out',str(stage/'ca.pem')])
        for name in ('server','client'):
            run(['openssl','req','-newkey','rsa:3072','-nodes','-sha256','-subj','/CN=Cowork executor '+name,'-keyout',str(stage/(name+'-key.pem')),'-out',str(stage/(name+'.csr'))])
            ext = 'extendedKeyUsage='+('serverAuth' if name=='server' else 'clientAuth')+'\n'
            if name=='server':
                ext += 'subjectAltName=DNS:localhost,IP:127.0.0.1\n'
            atomic(stage/(name+'.ext'),ext.encode())
            run(['openssl','x509','-req','-in',str(stage/(name+'.csr')),'-CA',str(stage/'ca.pem'),'-CAkey',str(stage/'ca-key.pem'),'-set_serial',str(uuid.uuid4().int),'-sha256','-days',days,'-extfile',str(stage/(name+'.ext')),'-out',str(stage/(name+'.pem'))])
    for path in stage.iterdir():
        path.chmod(0o600)

def validate(config, material):
    pairs = [('certificate.pem','key.pem','serverAuth',config['hostname'])] if config['kind']=='control' else [('server.pem','server-key.pem','serverAuth','localhost'),('client.pem','client-key.pem','clientAuth',None)]
    for cert,key,purpose,host in pairs:
        pub = run(['openssl','x509','-in',str(material/cert),'-pubkey','-noout'])
        require(pub == run(['openssl','pkey','-in',str(material/key),'-pubout']), 'renewed key/certificate mismatch')
        run(['openssl','x509','-in',str(material/cert),'-checkend','86400','-noout'])
        args = ['openssl','verify','-CAfile',str(material/('certificate.pem' if config['kind']=='control' else 'ca.pem')),'-purpose','sslserver' if purpose=='serverAuth' else 'sslclient']
        if host:
            args += ['-verify_hostname',host]
        run([*args,str(material/cert)])

def state(root):
    path = root/'lifecycle-state.json'
    return json.loads(private(path).read_bytes()) if path.exists() else {'format':1,'revision':0,'pending':None,'last_receipt':None}

def plan(config,config_sha,root,expected,force=False):
    current = state(root)
    require(current['revision']==expected and current['pending'] is None, 'certificate revision conflict or original rollout is still pending')
    certificates = [Path(config['certificate_file'])] if config['kind']=='control' else [Path(config['vm_root'])/n for n in ('ca.pem','server.pem','client.pem')]
    due = any(subprocess.run(['openssl','x509','-in',str(certificate),'-checkend',str(config.get('renew_before_days',14)*86400),'-noout'],capture_output=True).returncode != 0 for certificate in certificates)
    if not due and not force:
        print('certificate_renewal_due=false')
        return None
    ident = str(uuid.uuid4())
    operation = root/ident
    operation.mkdir(mode=0o700)
    (operation/'candidate').mkdir(mode=0o700)
    (operation/'original').mkdir(mode=0o700)
    original = material_files(config)
    for name,path in original.items():
        atomic(operation/'original'/name,material_path(config,path).read_bytes())
    generate(config,operation/'candidate')
    validate(config,operation/'candidate')
    consumers = inventory(config)
    mounted = set(v for r in consumers for v in r['mounts'])
    mounted_material = original if config['kind']=='control' else {n:original[n] for n in ('ca.pem','client.pem','client-key.pem')}
    require(all(str(p) in mounted or any(str(p).startswith(m.rstrip('/')+'/') for m in mounted) for p in mounted_material.values()), 'selected consumers do not mount the complete TLS material')
    rendered = json.loads(compose(config,'config','--format','json'))
    for service in config['consumer']['services']:
        image = rendered['services'][service]['image']
        require(re.fullmatch(r'(?:[a-zA-Z0-9./_-]+@)?sha256:[a-f0-9]{64}',image), 'TLS rollout requires digest-pinned existing consumer images')
    manifest = {'format':'cowork-certificate-plan-v1','id':ident,'kind':config['kind'],'expected_revision':expected,'config_sha256':config_sha,
                'original':{n:digest(p) for n,p in original.items()},'original_modes':{n:p.stat().st_mode&0o777 for n,p in original.items()},'candidate':{n:digest(operation/'candidate'/n) for n in original},
                'consumer_inventory':consumers,'compose_config_sha256':sha(compose(config,'config','--format','json')),
                'vm_inventory':vm_inventory(config) if config['kind']=='vm' else None,
                'vm_identity':config.get('_vm'),'created_at':datetime.now(timezone.utc).isoformat()}
    atomic(operation/'ORIGINAL-PLAN.json',manifest)
    print(canonical({'plan':str(operation/'ORIGINAL-PLAN.json'),'plan_sha256':sha(canonical(manifest))}).decode())
    return manifest

def verify_final(config,plan,operation):
    candidate = operation/'candidate'
    for name,path in material_files(config).items():
        require(digest(material_path(config,path))==plan['candidate'][name] and path.stat().st_mode&0o777==plan['original_modes'][name], 'installed TLS material/read mode differs from original candidate')
    after = inventory(config)
    expected = sorted((r['service'],r['image']) for r in plan['consumer_inventory'])
    require(sorted((r['service'],r['image']) for r in after)==expected and all(r['running'] for r in after), 'TLS consumer image/health rollout differs')
    require(not set(r['id'] for r in after)&set(r['id'] for r in plan['consumer_inventory']), 'original TLS consumer was not recreated')
    inspected = json.loads(docker('inspect',*(r['id'] for r in after)))
    require(all(r['State'].get('Health',{}).get('Status','healthy')=='healthy' for r in inspected), 'consumer health is not ready')
    if config['kind']=='control':
        origin = urlsplit(config['probe_origin'])
        ctx = ssl.create_default_context(cafile=str(candidate/'certificate.pem'))
        with socket.create_connection((origin.hostname,origin.port or 443),timeout=10) as sock:
            with ctx.wrap_socket(sock,server_hostname=origin.hostname) as tls:
                actual = sha(tls.getpeercert(binary_form=True))
        expected_cert = sha(run(['openssl','x509','-in',str(candidate/'certificate.pem'),'-outform','DER']))
        require(actual==expected_cert, 'serving TLS certificate did not rotate')
        with urlopen(Request(config['probe_origin'].rstrip('/')+config.get('probe_path','/readyz')),context=ctx,timeout=10) as response:
            require(response.status==200,'control HTTPS readiness failed')
        probe = {'serving_certificate_der_sha256':actual,'https_readiness':200}
    else:
        info = vm_docker(config,candidate,'info','--format','{{json .}}')
        require(info['ID']==plan['vm_inventory']['daemon_id'] and 'cowork.trust-domain='+config['_vm']['organisation_id'] in info.get('Labels',[]), 'rotated mTLS daemon identity differs')
        require(vm_inventory(config)==plan['vm_inventory'],'VM inventory changed during certificate maintenance')
        for name in ('ca.pem','server.pem','server-key.pem'):
            remote = vm_ssh(config,'sudo sha256sum /etc/docker/tls/'+shlex.quote(name)).decode().split()[0]
            require(remote==plan['candidate'][name],'guest TLS material did not rotate')
        probe = {'mutual_tls_verified':True,'daemon_id':info['ID'],'guest_inventory_unchanged':True}
    return {'consumer_inventory':after,'probe':probe}

def install_vm(config,plan,operation):
    # One durable guest journal records the original operation before any restart.
    # Its stdout is a physical receipt, not caller-supplied evidence.
    bundle = {'id':plan['id'],'files':{n:(operation/'candidate'/n).read_text() for n in ('ca.pem','server.pem','server-key.pem')},
              'original':{n:plan['original'][n] for n in ('ca.pem','server.pem','server-key.pem')},
              'candidate':{n:plan['candidate'][n] for n in ('ca.pem','server.pem','server-key.pem')}}
    script = '''import hashlib,json,os,pathlib,subprocess,sys
b=json.load(sys.stdin); root=pathlib.Path('/etc/docker/tls'); journal=root/'.cowork-certificate-rollout.json'
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
if journal.exists():
 j=json.loads(journal.read_text())
 if j['id']!=b['id'] and j['state']!='complete': raise SystemExit('another original guest rollout is pending')
 if j['id']==b['id']:
  if j['state']!='complete': raise SystemExit('guest rollout outcome unknown; reconcile original operation')
  if any(sha(root/n)!=b['candidate'][n] for n in b['files']): raise SystemExit('guest original receipt differs')
  print(json.dumps(j)); sys.exit(0)
if any(sha(root/n)!=b['original'][n] for n in b['files']): raise SystemExit('guest certificate CAS differs')
if subprocess.check_output(['docker','ps','--quiet']).strip(): raise SystemExit('guest has running workloads')
def write(p,v):
 t=p.with_name('.'+p.name+'.'+b['id']); fd=os.open(t,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
 with os.fdopen(fd,'wb') as f: f.write(v); f.flush(); os.fsync(f.fileno())
 os.replace(t,p)
j={'id':b['id'],'state':'restart_pending','candidate':b['candidate']}; write(journal,json.dumps(j).encode())
for n,v in b['files'].items(): write(root/n,v.encode())
subprocess.run(['systemctl','restart','docker.service'],check=True,timeout=90)
subprocess.run(['systemctl','is-active','--quiet','docker.service'],check=True)
if any(sha(root/n)!=b['candidate'][n] for n in b['files']): raise SystemExit('guest installed material mismatch')
j['state']='complete'; write(journal,json.dumps(j).encode()); print(json.dumps(j))
'''
    return json.loads(vm_ssh(config,'sudo python3 -c '+shlex.quote(script),canonical(bundle)))

def apply(config,config_sha,root,manifest,plan_sha,reconcile=False):
    require(sha(canonical(manifest))==plan_sha and manifest['config_sha256']==config_sha and manifest['kind']==config['kind'], 'original plan/configuration hash differs')
    operation = safe(root/str(uuid.UUID(manifest['id'])),True)
    require(json.loads(private(operation/'ORIGINAL-PLAN.json').read_bytes())==manifest,'original immutable plan differs')
    current = state(root)
    if current['last_receipt'] and current['last_receipt']['plan_id']==manifest['id']:
        print(canonical(current['last_receipt']).decode())
        return
    require(current['revision']==manifest['expected_revision'] and current['pending'] in (None,manifest['id']), 'certificate CAS revision differs')
    if (operation/'FINAL-RECEIPT.json').exists():
        receipt=json.loads(private(operation/'FINAL-RECEIPT.json').read_bytes())
        require(receipt['plan_id']==manifest['id'] and receipt['plan_sha256']==plan_sha and receipt['candidate']==manifest['candidate']
                and receipt['revision']==current['revision']+1 and current['pending']==manifest['id'], 'original final certificate receipt differs')
        current.update(revision=receipt['revision'],pending=None,last_receipt=receipt)
        atomic(root/'lifecycle-state.json',current)
        print(canonical(receipt).decode())
        return
    require(sha(compose(config,'config','--format','json'))==manifest['compose_config_sha256'],'original Compose configuration differs')
    for name,path in material_files(config).items():
        require(digest(private(operation/'candidate'/name))==manifest['candidate'][name], 'candidate TLS bytes differ')
        require(digest(material_path(config,path)) in (manifest['original'][name],manifest['candidate'][name]) and path.stat().st_mode&0o777==manifest['original_modes'][name], 'current TLS material/read mode fails CAS')
    validate(config,operation/'candidate')
    journal_path = operation/'rollout-journal.json'
    journal = json.loads(private(journal_path).read_bytes()) if journal_path.exists() else {'stage':'prepared'}
    if reconcile:
        require(current['pending']==manifest['id'],'no original rollout is pending')
    else:
        require(journal['stage']=='prepared' and current['pending'] is None,'original command outcome is pending; use reconcile')
        require(inventory(config)==manifest['consumer_inventory'],'original consumer identity/inventory differs')
        if config['kind']=='vm':
            require(vm_inventory(config)==manifest['vm_inventory'] and config['_vm']==manifest['vm_identity'],'original VM inventory/identity differs')
        current['pending']=manifest['id']
        atomic(root/'lifecycle-state.json',current)
        # Stop only positively identified selected consumers; the same operator
        # lock used by provisioner.py must become available before material changes.
        journal['stage']='consumer_stop_pending'; atomic(journal_path,journal)
        ids = [r['id'] for r in manifest['consumer_inventory']]
        docker('stop','--time','45',*ids)
        journal['stage']='consumers_stopped'; atomic(journal_path,journal)
    with locked(Path(config['operator_state_directory'])/'operator.lock'):
        if not reconcile:
            if config['kind']=='vm':
                journal['stage']='guest_rollout_pending'; atomic(journal_path,journal)
                journal['guest_receipt']=install_vm(config,manifest,operation)
                journal['stage']='guest_rotated'; atomic(journal_path,journal)
            for name,path in material_files(config).items():
                atomic(path,(operation/'candidate'/name).read_bytes(),manifest['original_modes'][name])
            journal['stage']='consumer_rollout_pending'; atomic(journal_path,journal)
            # --no-deps leaves databases, management API, foreign containers and
            # installed tenant images untouched. Images must remain original.
            compose(config,'up','-d','--no-deps','--force-recreate','--pull','never','--no-build',*config['consumer']['services'])
            journal['stage']='consumer_rollout_returned'; atomic(journal_path,journal)
        else:
            # A lost recreate response is reconciled by serving TLS and new exact
            # container IDs below. Earlier known-completed stages can continue.
            if journal['stage'] in ('consumer_stop_pending','consumers_stopped','guest_rotated'):
                stopped = inventory(config)
                require(all(not r['running'] for r in stopped) and sorted(r['id'] for r in stopped)==sorted(r['id'] for r in manifest['consumer_inventory']), 'original consumers are not positively stopped')
                if config['kind']=='vm' and journal['stage']!='guest_rotated':
                    journal['stage']='guest_rollout_pending'; atomic(journal_path,journal)
                    journal['guest_receipt']=install_vm(config,manifest,operation)
                    journal['stage']='guest_rotated'; atomic(journal_path,journal)
                for name,path in material_files(config).items():
                    atomic(path,(operation/'candidate'/name).read_bytes(),manifest['original_modes'][name])
                journal['stage']='consumer_rollout_pending'; atomic(journal_path,journal)
                compose(config,'up','-d','--no-deps','--force-recreate','--pull','never','--no-build',*config['consumer']['services'])
                journal['stage']='consumer_rollout_returned'; atomic(journal_path,journal)
            elif journal['stage']=='guest_rollout_pending':
                # The exact candidate must already serve real authenticated mTLS;
                # this proves the earlier guest restart outcome without replay.
                info = vm_docker(config,operation/'candidate','info','--format','{{json .}}')
                require(info.get('ID')==manifest['vm_inventory']['daemon_id'] and vm_inventory(config)==manifest['vm_inventory'], 'guest restart outcome is still unknown')
                readback = vm_ssh(config,"sudo cat /etc/docker/tls/.cowork-certificate-rollout.json")
                guest = json.loads(readback)
                require(guest['id']==manifest['id'] and guest['candidate']=={n:manifest['candidate'][n] for n in ('ca.pem','server.pem','server-key.pem')}, 'original guest rollout journal differs')
                finalize='''import hashlib,json,os,pathlib,sys
p=pathlib.Path('/etc/docker/tls/.cowork-certificate-rollout.json'); wanted=json.load(sys.stdin); j=json.loads(p.read_text())
if j['id']!=wanted['id'] or j['candidate']!=wanted['candidate']: raise SystemExit('original guest journal differs')
if any(hashlib.sha256((p.parent/n).read_bytes()).hexdigest()!=v for n,v in wanted['candidate'].items()): raise SystemExit('guest candidate readback differs')
j['state']='complete'; t=p.with_name('.reconciled-'+wanted['id']); fd=os.open(t,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
with os.fdopen(fd,'wb') as f: f.write(json.dumps(j).encode()); f.flush(); os.fsync(f.fileno())
os.replace(t,p)
'''
                vm_ssh(config,'sudo python3 -c '+shlex.quote(finalize),canonical({'id':manifest['id'],'candidate':guest['candidate']}))
                journal['guest_receipt']={'id':manifest['id'],'candidate':guest['candidate'],'state':'physically_reconciled'}
                journal['stage']='guest_rotated'; atomic(journal_path,journal)
                # No destructive command is retried. The next original reconcile
                # continues from this positively verified completed guest phase.
                raise RuntimeError('original guest restart physically reconciled; run the same reconcile again to complete its consumers')
    # The provisioner starts after the maintenance lock releases. Check actual
    # service health and serving TLS/mTLS until bounded readiness; no command retry.
    deadline = time.monotonic()+120
    while True:
        try:
            evidence = verify_final(config,manifest,operation)
            break
        except (RuntimeError,OSError,ssl.SSLError):
            require(time.monotonic()<deadline,'rollout remains pending; inspect original journal and reconcile after actual recovery')
            time.sleep(2)
    receipt = {'format':'cowork-certificate-receipt-v1','plan_id':manifest['id'],'plan_sha256':plan_sha,
               'revision':current['revision']+1,'candidate':manifest['candidate'],'physical_readback':evidence,
               'completed_at':datetime.now(timezone.utc).isoformat()}
    atomic(operation/'FINAL-RECEIPT.json',receipt)
    current.update(revision=receipt['revision'],pending=None,last_receipt=receipt)
    atomic(root/'lifecycle-state.json',current)
    print(canonical(receipt).decode())

def main():
    require(os.name=='posix','certificate operator requires Linux')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action',choices=['plan','apply','reconcile','maintain','status'])
    parser.add_argument('--config',required=True)
    parser.add_argument('--state',required=True)
    parser.add_argument('--expected-revision',type=int)
    parser.add_argument('--plan')
    parser.add_argument('--plan-sha256')
    parser.add_argument('--force',action='store_true')
    args = parser.parse_args()
    root = safe(args.state,True)
    config,config_sha = configuration(args.config)
    with locked(root/'.lifecycle.lock'):
        if args.action=='status':
            print(canonical(state(root)).decode()); return
        if args.action in ('plan','maintain'):
            require(args.expected_revision is not None or args.action=='maintain','plan requires original expected revision')
            if args.action=='maintain' and state(root)['pending']:
                original = root/state(root)['pending']/'ORIGINAL-PLAN.json'
                manifest = json.loads(private(original).read_bytes())
                apply(config,config_sha,root,manifest,sha(canonical(manifest)),True)
                return
            expected = state(root)['revision'] if args.action=='maintain' else args.expected_revision
            manifest = plan(config,config_sha,root,expected,args.force)
            if manifest is not None and args.action=='maintain':
                apply(config,config_sha,root,manifest,sha(canonical(manifest)))
            return
        require(args.plan and args.plan_sha256,'apply/reconcile requires original plan and its SHA256')
        manifest = json.loads(private(args.plan).read_bytes())
        apply(config,config_sha,root,manifest,args.plan_sha256,args.action=='reconcile')

if __name__=='__main__':
    main()
