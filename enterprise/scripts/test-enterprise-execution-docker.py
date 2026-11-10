#!/usr/bin/env python3
"""Create an isolated Docker Desktop installation for the admission acceptance.

Never connects to the live management API, databases or storage. Failure retains
the uniquely named test installation for inspection. Successful tests delete their
new instance fixtures through the production deletion implementation first.
"""
import argparse
import base64
import json
import ipaddress
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import copy
import shutil
import hashlib
import uuid
import csv
import os


def docker(*args):
    result=subprocess.run(['docker',*args],capture_output=True,text=True,encoding='utf-8')
    if result.returncode: raise RuntimeError('docker '+args[0]+' failed: '+result.stderr[-2500:])
    return result.stdout.strip()


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--server-image',default='cowork-server:execution-coverage-candidate')
    parser.add_argument('--provisioner-image',default='cowork-provisioner:execution-coverage-candidate')
    parser.add_argument('--provisioning-releases',action='store_true')
    parser.add_argument('--enrollment-preflight',action='store_true')
    parser.add_argument('--enrollment-lifecycle',action='store_true')
    parser.add_argument('--cross-version-source-image',help='Exact installed source release; requires --enrollment-lifecycle')
    parser.add_argument('--source-provisioner-image',default='cowork-provisioner:local-review')
    parser.add_argument('--source-instance-images',type=Path,help='JSON map of six immutable source tenant images')
    parser.add_argument('--source-instance-migration',type=int,default=43)
    parser.add_argument('--output-dir',type=Path,help='Separate directory for this acceptance report and fixture marker')
    parser.add_argument('--test-account-file',type=Path,help='Private JSON email/password for the primary owner; never written to reports')
    parser.add_argument('--managed-resources',action='store_true',help='Also qualify central role, skill, model and MCP dispatch with revocation')
    parser.add_argument('--managed-resources-only',action='store_true',help='Provision the private fleet and run only central resource acceptance')
    args=parser.parse_args()
    if args.managed_resources_only:
        args.managed_resources=True
    if args.managed_resources and (args.provisioning_releases or args.enrollment_preflight or args.enrollment_lifecycle):
        parser.error('managed resource acceptance requires the default lifecycle profile')
    if args.provisioning_releases and (args.cross_version_source_image or args.enrollment_lifecycle or args.enrollment_preflight):
        parser.error('initial release acceptance is a separate lifecycle profile')
    if args.cross_version_source_image and (not args.enrollment_lifecycle or args.enrollment_preflight):
        parser.error('cross-version acceptance requires only --enrollment-lifecycle')
    if args.source_instance_images and not args.cross_version_source_image:
        parser.error('source instance images require cross-version acceptance')
    repo=Path(__file__).resolve().parent.parent
    output_dir=(args.output_dir or repo/'.internal/enterprise-local').resolve()
    output_dir.mkdir(parents=True,exist_ok=True)
    test_account=None
    if args.test_account_file:
        if args.provisioning_releases or args.enrollment_preflight or args.enrollment_lifecycle:
            parser.error('test account override currently supports the default lifecycle profile')
        account=json.loads(args.test_account_file.read_text(encoding='utf-8-sig'))
        if not isinstance(account.get('email'),str) or not isinstance(account.get('password'),str) or len(account['password'])<12:
            parser.error('test account file must contain an email and a password of at least 12 characters')
        test_account={key:account[key] for key in ['email','password']}
    network_contract=json.loads(docker('run','--rm','--network','none','--entrypoint','python3',args.provisioner_image,
        '/opt/cowork-provisioner/local_entrypoint.py','--network-contract'))
    assert network_contract=={'version':1,'independent_namespace':True,'management_url':'https://operator-control:8443'}
    root=Path(tempfile.mkdtemp(prefix='cowork-admission-docker-'))
    if os.name=='nt':
        # Only this newly created directory is changed. Test credentials must
        # not inherit a workstation's broad Users permissions on TEMP.
        options={'creationflags':subprocess.CREATE_NO_WINDOW}
        identity=subprocess.check_output(['whoami','/user','/fo','csv','/nh'],text=True,**options)
        sid=next(csv.reader(identity.strip().splitlines()))[1]
        if not sid.startswith('S-1-5-'): raise RuntimeError('cannot bind private fixture permissions')
        subprocess.run(['icacls',str(root),'/inheritance:r','/grant:r',f'*{sid}:(OI)(CI)F',
            '*S-1-5-18:(OI)(CI)F','*S-1-5-32-544:(OI)(CI)F'],check=True,stdout=subprocess.DEVNULL,**options)
    # Docker Desktop can reject directory enumeration of a live Nextcloud mount.
    # Freeze test sources in the private local fixture before any containers start.
    test_sources=root/'tests';test_sources.mkdir()
    for path in (repo/'scripts').glob('*.py'): shutil.copyfile(path,test_sources/path.name)
    scope='cowork-admission-test-'+uuid.uuid4().hex[:12]
    network_ids=docker('network','ls','--quiet').splitlines()
    occupied=[ipaddress.ip_network(entry['Subnet']) for network in json.loads(docker('network','inspect',*network_ids))
              for entry in network.get('IPAM',{}).get('Config',[]) or [] if entry.get('Subnet')]
    networks={}
    for name in ['management','services','routing']:
        subnet=next((value for value in ipaddress.ip_network('10.249.0.0/16').subnets(new_prefix=27)
                     if not any(value.version==other.version and value.overlaps(other) for other in occupied)),None)
        if subnet is None: raise RuntimeError('dedicated acceptance address pool exhausted')
        occupied.append(subnet); networks[name]=str(subnet)
    def write(name,value):
        (root/name).write_text(value,encoding='utf-8')
    if test_account:
        write('test-account.json',json.dumps(test_account))
    def pinned(image):
        values=json.loads(docker('image','inspect',image,'--format','{{json .RepoDigests}}'))
        if not values: raise RuntimeError('local image has no immutable repository digest')
        if '@sha256:' in image:
            if image not in values: raise RuntimeError('requested repository digest not present')
            return image
        return values[0]
    images={key:pinned(image) for key,image in {
        'server':args.server_image,'runner':'cowork-runner:local-review','sandbox_core':'cowork-sandbox-core:local-review',
        'sandbox_gui':'cowork-sandbox-gui:local-review','sandbox_crew':'cowork-sandbox-crew:local-review','egress_proxy':'cowork-egress:local-review'}.items()}
    docker('run','--rm','--network','none','--mount','type=bind,src='+str(root)+',target=/out','--entrypoint','openssl',images['server'],
        'req','-x509','-newkey','rsa:3072','-nodes','-sha256','-days','2','-subj','/CN=operator-control',
        '-addext','subjectAltName=DNS:operator-control'+(',DNS:operator-control-active' if args.enrollment_lifecycle else ''),'-addext','extendedKeyUsage=serverAuth',
        '-keyout','/out/control_tls_key','-out','/out/control_tls_certificate')
    # Compose file-backed secrets retain source mode on Docker Desktop. Only the
    # private proxy mounts this key; it must be readable by its non-root user.
    docker('run','--rm','--network','none','--mount','type=bind,src='+str(root)+',target=/out',
        '--entrypoint','chmod',images['server'],'0444','/out/control_tls_key','/out/control_tls_certificate')
    write('operator-control-nginx.conf',(repo/'deploy/enterprise/operator-control-nginx.conf').read_text(encoding='utf-8'))
    if args.cross_version_source_image:
        target_images=copy.deepcopy(images)
        target_image=images['server']
        management_source_image=pinned(args.cross_version_source_image)
        images['server']=management_source_image
        if args.source_instance_images:
            source_images=json.loads(args.source_instance_images.read_text(encoding='utf-8-sig'))
            if set(source_images)!=set(images) or any(not isinstance(v,str) or '@sha256:' not in v for v in source_images.values()):
                raise RuntimeError('source image set must contain six immutable repository digests')
            images={key:pinned(value) for key,value in source_images.items()}
    else:
        # Same-binary lifecycle test, explicitly separate from release qualification.
        target_context=root/'target-image'; target_context.mkdir()
        (target_context/'Dockerfile').write_text('FROM '+args.server_image+'\nLABEL cowork.acceptance.transition="v1"\n',encoding='utf-8')
        transition_tag=scope+'-transition:local'
        docker('build','--tag',transition_tag,str(target_context))
        target_image=pinned(transition_tag)
        assert pinned(args.server_image)==images['server'], 'source build tag changed during fixture preparation'
    hashes=[docker('run','--rm','--network','none','--entrypoint','sha256sum',image,'/usr/local/bin/cowork-server') for image in [images['server'],target_image]]
    assert (hashes[0]!=hashes[1]) if args.cross_version_source_image else (hashes[0]==hashes[1]), 'unexpected binary relationship'
    values={name:secrets.token_hex(32) for name in ['management_password','fleet_password','bootstrap','broker','s3_password']}
    values.update({'s3_user':'admission_fixture','management_url':f"postgres://fixture:{values['management_password']}@management-db:5432/fixture",
        'fleet_url':f"postgres://fixture:{values['fleet_password']}@fleet-db:5432/fixture",'auth_key':base64.b64encode(secrets.token_bytes(32)).decode(),
        'ceiling':json.dumps({'platform_concurrent_runs':2,'organisation_concurrent_runs':2,'user_concurrent_runs':1})})
    for name,value in values.items(): write(name,value)
    (root/'audit_key').write_bytes(secrets.token_bytes(32))
    write('empty.env','')
    state_volume=scope+'-state'
    docker('volume','create','--label','cowork.acceptance='+scope,state_volume)
    state=docker('volume','inspect',state_volume,'--format','{{.Mountpoint}}')
    config={'backend':'compose','management_url':'https://operator-control:8443','management_ca_file':'/run/secrets/control_tls_certificate',
        'acceptance_management_url':'http://fixture-management:8080','fleet_management_url':'http://fixture-management:8080',
        'provisioner_token_file':'/run/secrets/broker','state_directory':state+'/instances','backup_directory':state+'/backups',
        'backup_identity_file':state+'/backup-identity','release_directory':state+'/releases','deletion_enabled':True,
        'database_endpoint':'fleet-db:5432','database_admin_url_file':'/run/secrets/fleet_url','s3_endpoint':'http://fleet-storage:9000',
        's3_admin_alias':'fixture','s3_admin_config_directory':state+'/mc','services_network':scope+'-services','routing_network':scope+'-routing',
        'images':images,'compose_network_pool':'10.248.0.0/16','sandbox_security_profile':'docker_desktop_seccomp',
        'execution_admission':{'server_images':[images['server'],target_image]},'acceptance_target_image':target_image}
    if test_account:
        config['acceptance_test_account_file']='/fixture/test-account.json'
    if args.managed_resources:
        config['acceptance_managed_resources']=True
        config['acceptance_managed_resources_only']=args.managed_resources_only
    if args.cross_version_source_image:
        config.update(acceptance_target_images=target_images,acceptance_source_migration=args.source_instance_migration,
                      acceptance_dormant_history=bool(args.source_instance_images))
    if args.provisioning_releases:
        config['release_selection_enabled']=True
        config.pop('execution_admission',None)
    write('config.json',json.dumps(config))
    def pg(password,network):
        return {'image':pinned('postgres:16.10-bookworm'),'environment':{'POSTGRES_USER':'fixture','POSTGRES_DB':'fixture','POSTGRES_PASSWORD_FILE':'/run/secrets/'+password},
            'secrets':[password],'networks':[network], 'volumes':[password+':/var/lib/postgresql/data'],
            'healthcheck':{'test':['CMD-SHELL','pg_isready -U fixture -d fixture'],'interval':'2s','timeout':'3s','retries':30}}
    management={'image':management_source_image if args.cross_version_source_image else images['server'],'read_only':True,'tmpfs':['/tmp'],'cap_drop':['ALL'],'security_opt':['no-new-privileges:true'],
        'environment':{'COWORK_MODE':'management','DATABASE_URL_FILE':'/run/secrets/management_url','COWORK_BOOTSTRAP_TOKEN_FILE':'/run/secrets/bootstrap',
            'COWORK_AUTH_MASTER_KEY_FILE':'/run/secrets/auth_key','COWORK_MANAGEMENT_AUDIT_KEY_FILE':'/run/secrets/audit_key',
            'COWORK_PROVISIONER_TOKEN_FILE':'/run/secrets/broker','COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED':'true',
            'COWORK_MANAGEMENT_EXECUTION_CEILING_FILE':'/run/secrets/ceiling','COWORK_MANAGEMENT_PROVISIONING_ENABLED':'true',
            'COWORK_MANAGEMENT_BACKUPS_ENABLED':'true','COWORK_MANAGEMENT_UPDATES_ENABLED':'true','COWORK_MANAGEMENT_DELETION_ENABLED':'true',
            'COWORK_INSTANCE_URL_TEMPLATE':'http://{instance}.localhost:18084','COWORK_WEB_PUSH_ENABLED':'false'},
        'secrets':['management_url','bootstrap','auth_key','audit_key','broker','ceiling'],
        'networks':{'management':{},'services':{'aliases':['fixture-management']}},
        'depends_on':{'management-db':{'condition':'service_healthy'}},
        'healthcheck':{'test':['CMD','curl','--fail','--silent','http://127.0.0.1:8080/readyz'],'interval':'2s','timeout':'3s','retries':40}}
    manifest={'name':scope,'services':{'management-db':pg('management_password','management'),'fleet-db':pg('fleet_password','services'),
        'management':management,'fleet-storage':{'image':pinned('cowork-storage:local-review'),'command':['server','/data'],
            'environment':{'MINIO_ROOT_USER_FILE':'/run/secrets/s3_user','MINIO_ROOT_PASSWORD_FILE':'/run/secrets/s3_password'},
            'secrets':['s3_user','s3_password'],'volumes':['storage:/data'],'networks':['services'],
            'healthcheck':{'test':['CMD','curl','--fail','--silent','http://127.0.0.1:9000/minio/health/live'],'interval':'2s','timeout':'3s','retries':30}},
        'operator-control':{'image':pinned('cowork-web:local-review'),'read_only':True,'tmpfs':['/tmp'],'cap_drop':['ALL'],'security_opt':['no-new-privileges:true'],
            'networks':['management'],'secrets':['control_tls_key','control_tls_certificate'],
            'volumes':[{'type':'bind','source':str(root/'operator-control-nginx.conf'),'target':'/etc/nginx/conf.d/default.conf','read_only':True}],
            'depends_on':{'management':{'condition':'service_healthy'}}},
        'operator':{'image':pinned(args.provisioner_image),'entrypoint':['sleep','infinity'],'networks':{'management':{},'services':{'aliases':['fixture-model']}},
            'read_only':True,'tmpfs':['/tmp'],'cap_drop':['ALL'],'security_opt':['no-new-privileges:true'],
            'volumes':['/var/run/docker.sock:/var/run/docker.sock',state_volume+':'+state,
                {'type':'bind','source':str(root),'target':'/fixture'},
                {'type':'bind','source':str(test_sources),'target':'/tests','read_only':True}],
            'secrets':['bootstrap','broker','fleet_url','management_url','s3_user','s3_password','control_tls_certificate'],
            'depends_on':{**{name:{'condition':'service_healthy'} for name in ['management','fleet-db','fleet-storage']},'operator-control':{'condition':'service_started'}}}},
        'networks':{name:{'name':scope+'-'+name,'internal':True,'ipam':{'config':[{'subnet':subnet}]}}
                    for name,subnet in networks.items() if name!='routing'},
        'volumes':{'management_password':{},'fleet_password':{},'storage':{},state_volume:{'external':True}},
        'secrets':{name:{'file':str(root/name)} for name in [*values,'audit_key','control_tls_key','control_tls_certificate']}}
    if args.provisioning_releases:
        management['environment']['COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED']='true'
    if args.enrollment_preflight or args.enrollment_lifecycle or args.provisioning_releases:
        management['environment']['COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED']='false'
    if args.enrollment_lifecycle:
        management['environment']['COWORK_MANAGEMENT_EXECUTION_ENROLLMENT_ENABLED']='true'
        active=copy.deepcopy(management)
        if args.cross_version_source_image: active['image']=target_image
        active['profiles']=['activation']
        active['environment']['COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED']='true'
        active['networks']['services']['aliases']=['fixture-management-active']
        manifest['services']['management-active']=active
        active_proxy=copy.deepcopy(manifest['services']['operator-control'])
        active_proxy['profiles']=['activation']
        active_proxy['depends_on']={'management-active':{'condition':'service_healthy'}}
        active_proxy['volumes'][0]['source']=str(root/'operator-control-active-nginx.conf')
        write('operator-control-active-nginx.conf',(repo/'deploy/enterprise/operator-control-nginx.conf').read_text(encoding='utf-8').replace('management:8080','management-active:8080'))
        manifest['services']['operator-control-active']=active_proxy
    if args.cross_version_source_image:
        # Keep the actual installed operator for source provisioning and journals.
        source_operator=copy.deepcopy(manifest['services']['operator'])
        source_operator['image']=pinned(args.source_provisioner_image)
        source_operator['networks']['services']['aliases']=['fixture-model-source']
        source_operator['environment']={'SSL_CERT_FILE':'/run/secrets/control_tls_certificate'}
        manifest['services']['operator-source']=source_operator
        write('recovery_management_url',values['management_url'].rsplit('/',1)[0]+'/fixture_recovery')
        manifest['secrets']['recovery_management_url']={'file':str(root/'recovery_management_url')}
        recovery=copy.deepcopy(management)
        recovery['profiles']=['backout']
        recovery['environment']['DATABASE_URL_FILE']='/run/secrets/recovery_management_url'
        recovery['secrets'].append('recovery_management_url')
        recovery['networks']['services']['aliases']=['fixture-management-recovery']
        manifest['services']['management-recovery']=recovery
    write('compose.json',json.dumps(manifest))
    marker={'scope':scope,'root':str(root),'state_volume':state_volume,'images':images,'provisioner_image':manifest['services']['operator']['image'],'live_modified':False}
    if args.cross_version_source_image:
        marker.update(target_server_image=target_image,target_images=target_images,source_management_image=management_source_image,
                      source_provisioner_image=source_operator['image'],binary_sha256=[h.split()[0] for h in hashes])
    report_prefix='managed-resources-docker' if args.managed_resources_only else 'provisioning-release-docker' if args.provisioning_releases else 'execution-cross-version-docker' if args.cross_version_source_image else 'execution-enrollment-docker' if args.enrollment_lifecycle else 'execution-enrollment-preflight-docker' if args.enrollment_preflight else 'execution-docker'
    if args.source_instance_images:
        source_id=hashlib.sha256(json.dumps(images,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        report_prefix='execution-tenant-'+source_id[:12]+'-docker'
    marker_path=output_dir/(report_prefix+'-fixture.json')
    marker_path.write_text(json.dumps(marker,indent=2),encoding='utf-8')
    print('Isolated acceptance installation: '+scope,flush=True)
    compose=['compose','--env-file',str(root/'empty.env'),'--project-name',scope,'--file',str(root/'compose.json')]
    try:
        docker(*compose,'up','--detach','--wait','--wait-timeout','150')
        # Routing is deliberately absent from management and operator. It is used
        # only by tenant APIs; workers must reach callbacks via services instead.
        docker('network','create','--internal','--subnet',networks['routing'],'--label','cowork.acceptance='+scope,scope+'-routing')
        operator=docker(*compose,'ps','--quiet','operator')
        manager=docker(*compose,'ps','--quiet','management')
        if args.cross_version_source_image:
            source_operator_id=docker(*compose,'ps','--quiet','operator-source')
            def phase(container,name):
                result=subprocess.run(['docker','exec','--env','PYTHONPATH=/opt/cowork-provisioner',container,'python3','-u',
                    '/tests/enterprise_execution_cross_version_acceptance.py','--phase',name])
                if result.returncode: raise RuntimeError('cross-version phase failed: '+name)
            phase(source_operator_id,'source')
            docker(*compose,'stop','operator-source','management')
            phase(operator,'backup')
            docker(*compose,'--profile','backout','up','--detach','--wait','management-recovery')
            phase(operator,'verify-backout')
            docker(*compose,'--profile','backout','stop','management-recovery')
            management['image']=target_image
            write('compose.json',json.dumps(manifest))
            docker(*compose,'up','--detach','--no-deps','--wait','--wait-timeout','120','management')
            # nginx resolves upstream at startup, so renew its connection after recreation.
            docker(*compose,'restart','operator-control')
            manager=docker(*compose,'ps','--quiet','management')
        acceptance='enterprise_provisioning_release_docker.py' if args.provisioning_releases else 'enterprise_execution_enrollment_acceptance.py' if args.enrollment_preflight or args.enrollment_lifecycle else 'enterprise_execution_docker_acceptance.py'
        command=['docker','exec','--env','PYTHONPATH=/opt/cowork-provisioner',operator,'python3','-u',
            '/tests/'+acceptance,'--config','/fixture/config.json','--management-container',manager,
            '--output','/fixture/results.json',*(['--enroll'] if args.enrollment_lifecycle else [])]
        if args.cross_version_source_image:
            command=['docker','exec','--env','PYTHONPATH=/opt/cowork-provisioner',operator,'python3','-u',
                     '/tests/enterprise_execution_cross_version_acceptance.py','--phase','target']
        process=subprocess.Popen(command)
        activated=False
        while process.poll() is None:
            if args.enrollment_lifecycle and not activated and (root/'activation-request.json').exists():
                activated=True
                docker(*compose,'--profile','activation','up','--detach','--wait','--wait-timeout','90','management-active','operator-control-active')
                write('activation-ready.json',json.dumps({'scope':scope}))
            time.sleep(.2)
        if process.returncode: raise RuntimeError('real Docker acceptance failed; isolated fixtures retained')
        report=json.loads((root/'results.json').read_text(encoding='utf-8'))
        docker(*compose,*(['--profile','activation'] if args.enrollment_lifecycle else []),
               *(['--profile','backout'] if args.cross_version_source_image else []),'down','--volumes')
        docker('network','rm',scope+'-routing')
        docker('volume','rm',state_volume)
        report.update(marker); report['test_installation_removed']=True
        (output_dir/(report_prefix+'-results.json')).write_text(json.dumps(report,indent=2),encoding='utf-8')
        print('Acceptance passed; dedicated Docker installation removed.',flush=True)
    except Exception:
        print('Retained fixture metadata: '+str(marker_path),flush=True)
        raise


if __name__=='__main__': main()
