"""Real Docker provision/restart/backup/restore/deletion in a dedicated installation.

Invoked by test-enterprise-execution-docker.py, never against an existing fleet.
The model is a controlled local protocol fixture; lifecycle receipts are real.
"""
import argparse
import base64
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import secrets
import subprocess
import ssl
import socket
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler
import uuid
import provisioner as p
import execution_generation as epochs
from instance_backups import pg_environment
from operation_journal import OperationJournal
from release_catalog import ReleaseCatalog, image_set_id


def uid(): return str(uuid.uuid4())


def wait(predicate, message, seconds=60):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate(): return
        time.sleep(.2)
    raise AssertionError(message)


def totp(secret):
    key=base64.b32decode(secret+'='*((8-len(secret)%8)%8))
    digest=hmac.new(key,(int(time.time())//30).to_bytes(8,'big'),hashlib.sha1).digest()
    index=digest[-1]&15
    return f"{(int.from_bytes(digest[index:index+4],'big')&0x7fffffff)%1000000:06d}"


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--config',required=True); parser.add_argument('--output',required=True)
    parser.add_argument('--management-container',required=True)
    parser.add_argument('--cleanup',action='store_true')
    args=parser.parse_args()
    config=json.loads(Path(args.config).read_text())
    # Fail before any mutation if called with a live/general operator config.
    assert config['services_network'].startswith('cowork-admission-test-')
    scope=config['services_network'].removesuffix('-services')
    assert config['routing_network']==scope+'-routing'
    state=Path(config['state_directory']).parent
    assert state==Path('/var/lib/docker/volumes')/(scope+'-state')/'_data'
    labels=json.loads(p.run(['docker','inspect',args.management_container,'--format','{{json .Config.Labels}}']))
    assert labels['com.docker.compose.project']==scope
    manager=p.Manager(config); infra=p.Infrastructure(config)
    if args.cleanup:
        from instance_deletion import InstanceDeletion
        descriptors=json.loads((state/'acceptance-instances.json').read_text())
        with p.operator_lock(infra.root):
            for instance_id,instance in descriptors.items():
                assert instance_id==instance['id'] and instance['name'].startswith('Docker admission fixture ')
                directory=infra.root/uuid.UUID(instance_id).hex
                assert directory.is_dir() and not directory.is_symlink()
                if (directory/'deleted.json').exists(): continue
                operation={'id':uid(),'action':'delete','instance':instance,
                    'arguments':{'confirmation_name':instance['name'],'purge_backups':True}}
                InstanceDeletion(infra).execute(operation,directory)
        print('dedicated_fixture_infrastructure_cleanup=ok',flush=True)
        return
    (state/'mc').mkdir(exist_ok=True,mode=0o700)
    p.run(['mc','--config-dir',str(state/'mc'),'alias','set','fixture',config['s3_endpoint'],
        Path('/run/secrets/s3_user').read_text().strip(),Path('/run/secrets/s3_password').read_text().strip()])
    p.run(['age-keygen','--output',config['backup_identity_file']])
    checks=[]; fixtures=[]; operations=[]; managed_evidence=None
    descriptors={}
    def passed(name): checks.append(name); print(name+'=ok',flush=True)
    http=build_opener(ProxyHandler({}))
    def request(origin,path,token=None,body=None,expected=200,headers=None,method=None):
        headers={'content-type':'application/json',**(headers or {})}
        if token: headers['authorization']='Bearer '+token
        req=Request(origin+path,None if body is None else json.dumps(body).encode(),headers,method=method)
        try:
            with http.open(req,timeout=20) as response: status,content=response.status,response.read()
        except HTTPError as error: status,content=error.code,error.read()
        if status!=expected:
            # Only stable error metadata, never echoed request credentials.
            try: detail=json.loads(content).get('error',{})
            except ValueError: detail={}
            raise AssertionError(f'{req.get_method()} {path}: {status} expected {expected}; {str(detail)[:250]}')
        return json.loads(content) if content and status<400 else None
    management=config['acceptance_management_url']
    def sql(url,statement):
        return p.run(['psql','-X','-tA','-v','ON_ERROR_STOP=1'],env=pg_environment(url),data=statement.encode()).decode().strip()
    management_db=Path('/run/secrets/management_url').read_text().strip()
    held=lambda:int(sql(management_db,"SELECT count(*) FROM management_execution_reservations WHERE state='held';"))
    def control_ready():
        try:
            manager.call('/pending',{'backend':'compose','execution_admission_version':1})
            return True
        except OSError:return False
    wait(control_ready,'private TLS control channel did not become ready')
    untrusted=dict(config);untrusted.pop('management_ca_file')
    try:p.Manager(untrusted).call('/pending',{'backend':'compose','execution_admission_version':1})
    except OSError as error:assert isinstance(getattr(error,'reason',error),ssl.SSLCertVerificationError)
    else:raise AssertionError('untrusted control certificate accepted')
    with socket.create_connection(('operator-control',8443),timeout=5) as connection:
        context=ssl.create_default_context(cafile=config['management_ca_file'])
        try:context.wrap_socket(connection,server_hostname='wrong-host.example.test')
        except ssl.SSLCertVerificationError:pass
        else:raise AssertionError('wrong control hostname accepted')
    insecure={**config,'management_url':'http://fixture-management:8080'}
    try:p.Manager(insecure)
    except p.ProvisioningError as error:assert str(error)=='management_connection_requires_tls'
    else:raise AssertionError('non-loopback plaintext control accepted')
    passed('private_control_tls_verifies_certificate_and_hostname_without_plaintext_fallback')
    gate=threading.Semaphore(0); calls=[]; tool_outputs=[]; auto=False
    class Model(BaseHTTPRequestHandler):
        def log_message(self,*_): pass
        def do_POST(self):
            value=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            calls.append(value)
            if not auto and not gate.acquire(timeout=120): self.send_error(504); return
            shell=any('Check sandbox boundaries' in str(m.get('content')) for m in value.get('messages',[]))
            output=[m for m in value.get('messages',[]) if m.get('role')=='tool']
            message={'role':'assistant','content':'Docker capacity fixture completed.'}; finish='stop'
            if shell and not output:
                command="test ! -e /var/run/docker.sock && test ! -e /run/secrets/fleet_key && test ! -e /run/secrets/broker && printf sandbox-boundaries-ok"
                message={'role':'assistant','content':None,'tool_calls':[{'id':'fixture-shell','type':'function','function':{'name':'Bash','arguments':json.dumps({'command':command})}}]}
                finish='tool_calls'
            elif shell:
                tool_outputs.extend(output)
                assert 'sandbox-boundaries-ok' in json.dumps(output)
            body=json.dumps({'choices':[{'index':0,'message':message,'finish_reason':finish}],
                'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}).encode()
            self.send_response(200); self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(body))); self.end_headers()
            try: self.wfile.write(body)
            except (BrokenPipeError,ConnectionResetError): pass
    model=ThreadingHTTPServer(('0.0.0.0',9001),Model)
    threading.Thread(target=model.serve_forever,daemon=True).start()
    bootstrap=Path('/run/secrets/bootstrap').read_text().strip()
    admin=request(management,'/api/v1/auth/bootstrap',bootstrap,{'email':'operator@docker.test','display_name':'Acceptance operator',
        'password':secrets.token_urlsafe(30),'device_id':uid()},expected=201)
    setup=request(management,'/api/v1/auth/totp/setup',admin['access_token'],{})
    request(management,'/api/v1/auth/totp/enable',admin['access_token'],{'code':totp(setup['secret'])})
    account_file=config.get('acceptance_test_account_file')
    primary_account=json.loads(Path(account_file).read_text()) if account_file else {'email':'owner@docker.test','password':secrets.token_urlsafe(30)}
    invitation=request(management,'/api/v1/auth/invitations',admin['access_token'],{'email':primary_account['email']},expected=201)
    owner=request(management,'/api/v1/auth/invitations/accept',body={'token':invitation['token'],'display_name':'Fixture owner',
        'password':primary_account['password'],'device_id':uid()},expected=201)
    org=request(management,'/management/v1/organisations',admin['access_token'],{'name':'Docker execution acceptance'})['id']
    base='/management/v1/organisations/'+org
    request(management,base+'/members',admin['access_token'],{'email':primary_account['email'],'role':'member'})
    def current(item):
        return next(i for i in request(management,base+'/instances',owner['access_token']) if i['id']==item['id'])
    def begin(item,action,as_admin=False,**extra):
        op_id=request(management,base+'/instances/'+item['id']+'/operations',admin['access_token'] if as_admin else owner['access_token'],
            {'action':action,'expected_revision':current(item)['revision'],'idempotency_key':uid(),**extra})['id']
        op=manager.call('/claim',{'backend':'compose','execution_admission_version':1})
        assert op['id']==op_id and op['instance_id']==item['id'] and op['execution_binding'] and op['execution_runtime_required']
        descriptors[item['id']]=op['instance']
        p.private_write(state/'acceptance-instances.json',json.dumps(descriptors).encode())
        journal=OperationJournal(infra,op); journal.begin()
        return op,journal
    def lifecycle(item,action,**extra):
        op,journal=begin(item,action,**extra)
        op_id=op['id']
        result=infra.execute(op)
        completion=journal.finish(True,result)
        manager.call('/operations/'+op_id+'/complete',completion)
        # Exact duplicate delivery does not execute infrastructure or rotate twice.
        manager.call('/operations/'+op_id+'/complete',journal.recover())
        if action!='delete':
            evidence=json.loads(sql(management_db,f"SELECT execution_evidence FROM management_instances WHERE id='{item['id']}';"))
            manifest=json.loads((infra.directory(item['id'])/'compose.json').read_text())
            assert evidence==result['execution']
            assert evidence['runtime']['server_image']==manifest['services']['api']['image']
            assert evidence['runtime']['manifest_sha256']==hashlib.sha256(json.dumps(manifest,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        operations.append({'action':action,'id':op_id,'instance_id':item['id'],'epoch':result['execution']['active_epoch']})
        return result
    def directory(item): return infra.directory(item['id'])
    def active(item): return infra.active_directory(directory(item))
    def tenant_sql(item,statement): return sql((active(item)/'secrets/database_url').read_text().strip(),statement)
    def login(item):
        ticket=request(management,base+'/instances/'+item['id']+'/launch',owner['access_token'],{})
        item['token']=request(item['origin'],'/api/v1/auth/fleet',body={'ticket':ticket['ticket'],'device_id':uid()})['access_token']
    def create(item,shell=False):
        project=item['project']
        thread=request(item['origin'],'/api/v1/threads',item['token'],{'project_id':project['id'],'title':'Docker execution acceptance'},expected=201)
        return request(item['origin'],'/api/v1/runs',item['token'],{'project_id':project['id'],'thread_id':thread['id'],
            'project_revision':project['revision'],'project_privacy':'team_managed','executor_target':{'kind':'server_linux','pool_id':None},
            'model_profile_id':item['profile'],'required_capabilities':['model.external',*(['shell'] if shell else [])],
            'input':{'prompt':'Check sandbox boundaries' if shell else 'Reply with the completion marker.'},'idempotency_key':uid()},expected=201)['spec']['id']
    def run_state(item,run): return tenant_sql(item,f"SELECT state FROM runs WHERE id='{run}';")
    with p.operator_lock(infra.root):
        for n in range(2):
            instance=request(management,base+'/instances',owner['access_token'],{'name':f'Docker admission fixture {n}','backend':'compose'})
            item={'id':instance['id'],'name':f'Docker admission fixture {n}','origin':'http://cowork-'+uuid.UUID(instance['id']).hex+'-api-1:8080'}
            fixtures.append(item)
            lifecycle(item,'provision'); assert current(item)['state']=='ready'
            login(item)
            team=request(item['origin'],'/api/v1/teams',item['token'],{'name':'Acceptance'},expected=201)
            item['project']=request(item['origin'],'/api/v1/projects',item['token'],{'name':'Acceptance','privacy':'team_managed','team_id':team['id']},expected=201)
            item['profile']=request(item['origin'],'/api/v1/provider-profiles',item['token'],{'name':'Controlled local test model','provider_kind':'openai-compatible',
                'team_id':None,'model_defaults':{'base_url':'http://fixture-model:9001/v1','model':'fixture','auth_mode':'none'}},expected=201)['id']
            # Operator reaches APIs on their private services addresses. Workload
            # containers never join this shared trusted services network.
            manifest=json.loads((directory(item)/'compose.json').read_text())
            for service in ['api','worker']:
                container=infra.compose(current(item),directory(item),'ps','--quiet',service).decode().strip()
                env=json.loads(p.run(['docker','inspect',container,'--format','{{json .Config.Env}}']))
                assert 'COWORK_FLEET_EXECUTION_ADMISSION=true' in env
            assert 'fleet_key' not in manifest['services']['runner']['secrets']
        passed('real_provisioned_api_and_worker_use_admission_on_private_services_network')
        assert sql(management_db,'SELECT count(*) FROM management_execution_activation;')=='1'
        assert sql(management_db,"SELECT count(*) FROM management_instances WHERE execution_required AND execution_evidence IS NOT NULL;")=='2'
        p.run(['docker','restart',args.management_container])
        wait(lambda:p.run(['docker','inspect',args.management_container,'--format','{{.State.Health.Status}}']).strip()==b'healthy','management restart with covered fleet failed')
        assert request(management,'/readyz')['status']=='ready'
        passed('durable_activation_and_actual_runtime_attestations_survive_management_restart')
        first,second=fixtures
        if not config.get('acceptance_managed_resources_only'):
            run1=create(first); wait(lambda:len(calls)==1,'first model execution did not start')
            run2=create(second); time.sleep(2)
            assert held()==1 and len(calls)==1 and run_state(second,run2)=='queued'
            passed('two_docker_instances_share_one_user_slot')
            first_descriptor=current(first)
            paused=False
            try:
                p.run(['docker','pause',args.management_container]); paused=True
                gate.release(); wait(lambda:run_state(first,run1)=='completed','completion during management outage')
                assert held()==1 and run_state(second,run2)=='queued'
                assert tenant_sql(first,'SELECT count(*) FROM instance_execution_reservations WHERE completion IS NOT NULL AND acknowledged_at IS NULL;')=='1'
                infra.compose(first_descriptor,directory(first),'restart','worker')
                # current() needs management and is deliberately not called while it is paused.
                assert held()==1 and len(calls)==1
            finally:
                if paused: p.run(['docker','unpause',args.management_container])
            wait(lambda:len(calls)==2,'durable completion did not unblock second worker')
            gate.release(); wait(lambda:run_state(second,run2)=='completed' and held()==0,'second worker completion')
            auto=True
            passed('actual_management_outage_and_worker_restart_relay_completion_without_duplicate_execution')
            shell=create(first,True)
            wait(lambda:run_state(first,shell)=='completed' and held()==0,'real shell task did not finish')
            assert tool_outputs and len(calls)==4
            passed('real_runner_shell_has_no_docker_socket_or_management_secret')
            old_epoch=epochs.read(current(first),active(first))
            lifecycle(first,'stop'); backup=lifecycle(first,'backup')
            restored=lifecycle(first,'restore',backup_id=backup['backup_id'])
            new_epoch=restored['execution']['active_epoch']
            assert new_epoch!=old_epoch and current(first)['state']=='stopped'
            assert tenant_sql(first,'SELECT count(*) FROM projects;')=='1'
            lifecycle(first,'start'); login(first)
            assert tenant_sql(first,f"SELECT state FROM runs WHERE id='{shell}';")=='completed'
            passed('real_consistent_backup_and_restore_preserve_data_and_change_execution_epoch')
            stale={'organisation_id':org,'instance_id':first['id'],'execution_epoch':old_epoch,'run_id':uid(),
                'user_id':owner['user_id'],'target':{'kind':'server_linux','pool_id':None}}
            key=(active(first)/'runtime-secrets/fleet_key').read_text().strip()
            request(management,'/management/runner/v1/execution-reserve',body=stale,headers={'x-cowork-instance-key':key},expected=409)
            # Recreate only the dedicated test worker with the old process identity.
            # This proves an actual stale worker cannot execute after restoration.
            path=directory(first)/'compose.json'; original=path.read_bytes()
            bad=json.loads(original); bad['services']['worker']['environment']['COWORK_EXECUTION_EPOCH']=old_epoch
            p.private_write(path,json.dumps(bad).encode())
            try:
                infra.compose(current(first),directory(first),'up','--detach','--force-recreate','worker')
                before=len(calls); fresh=create(first); time.sleep(3)
                assert run_state(first,fresh)=='queued' and held()==0 and len(calls)==before
            finally:
                p.private_write(path,original)
                infra.compose(current(first),directory(first),'up','--detach','--force-recreate','worker')
            wait(lambda:run_state(first,fresh)=='completed' and held()==0,'corrected worker failed to execute')
            assert len(calls)==before+1
            assert tenant_sql(first,f"SELECT execution_epoch FROM instance_execution_reservations WHERE run_id='{fresh}';")==new_epoch
            passed('actual_stale_worker_is_fenced_then_current_worker_executes_exactly_once')
            lifecycle(first,'stop')
            source_images=infra.deployed_images(directory(first))
            target_images={**source_images,'server':config['acceptance_target_image']}
            catalog=ReleaseCatalog(config,p.private_write)
            catalog.publish('Admission source fixture',source_images,[image_set_id(target_images)])
            target=catalog.publish('Admission transition fixture (same binary)',target_images,[image_set_id(source_images)])
            p.sync_release_catalog(manager,infra)
            canary=lifecycle(first,'upgrade',release=target['id'],qualification_only=True)
            assert canary['execution']['active_epoch']==new_epoch and canary['rollback_verified'] is True
            assert infra.deployed_images(directory(first))==source_images
            promoted=lifecycle(first,'upgrade',release=target['id'],qualification_only=False)
            promoted_epoch=promoted['execution']['active_epoch']
            assert promoted_epoch!=new_epoch and infra.deployed_images(directory(first))==target_images
            lifecycle(first,'start'); login(first)
            promoted_run=create(first); before=len(calls)
            wait(lambda:run_state(first,promoted_run)=='completed' and held()==0,'promoted worker did not execute')
            assert tenant_sql(first,f"SELECT execution_epoch FROM instance_execution_reservations WHERE run_id='{promoted_run}';")==promoted_epoch
            passed('real_canary_and_promotion_keep_fences_and_promoted_worker_uses_new_epoch')
            lifecycle(first,'stop')
            rolled=lifecycle(first,'rollback',rollback_operation_id=promoted['receipt_id'])
            rollback_epoch=rolled['execution']['active_epoch']
            assert rollback_epoch not in [new_epoch,promoted_epoch,old_epoch]
            assert infra.deployed_images(directory(first))==source_images
            assert tenant_sql(first,f"SELECT count(*) FROM runs WHERE id='{promoted_run}';")=='0'
            lifecycle(first,'start'); login(first)
            rollback_run=create(first)
            wait(lambda:run_state(first,rollback_run)=='completed' and held()==0,'rollback worker did not execute')
            assert tenant_sql(first,f"SELECT execution_epoch FROM instance_execution_reservations WHERE run_id='{rollback_run}';")==rollback_epoch
            passed('real_rollback_restores_pre_update_data_and_executes_with_another_fresh_epoch')
            # Terminate a real worker mid-model-call. A retained-generation database
            # connection makes stop evidence incomplete even after containers vanish.
            auto=False
            before=len(calls); crashed=create(first)
            wait(lambda:len(calls)==before+1 and held()==1,'crash fixture did not start')
            waiting=create(second)
            worker=infra.compose(current(first),directory(first),'ps','--quiet','worker').decode().strip()
            p.run(['docker','stop','--time','0',worker]); gate.release()
            assert held()==1 and run_state(second,waiting)=='queued'
            reserved=json.loads(sql(management_db,f"SELECT row_to_json(r) FROM management_execution_reservations r WHERE run_id='{crashed}';"))
            args_recover={'reconcile_reservation_ids':[reserved['id']],'acknowledge_execution_stop':True}
            operation, journal=begin(first,'stop',as_admin=True,**args_recover)
            retained=directory(first)
            retained_url=(retained/'secrets/database_url').read_text().strip()
            blocker=subprocess.Popen(['psql','-X','-tAc','SELECT pg_sleep(90);'],env={**pg_environment(retained_url),'PGAPPNAME':'cowork-recovery-proof-fixture'},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            try:
                wait(lambda:sql(retained_url,"SELECT count(*) FROM pg_stat_activity WHERE application_name='cowork-recovery-proof-fixture';")=='1','retained database blocker missing')
                try:
                    infra.execute(operation)
                    raise AssertionError('active retained generation was accepted as stopped')
                except epochs.ExecutionGenerationError as error:
                    assert str(error)=='execution_recovery_database_still_active'
                assert journal.recover() is None and held()==1
                manager.call('/operations/'+operation['id']+'/reconciliation',{'lease_token':operation['lease_token'],'reason':'operation_recovery_requires_review'})
                request(management,base+'/instances/'+first['id']+'/operations/'+operation['id']+'/recover',admin['access_token'],{'mode':'stop_if_unproven'})
                pending=manager.call('/pending',{'backend':'compose','execution_admission_version':1})
                assert pending[0]['execution_recovery_binding']==operation['execution_recovery_binding']
            finally:
                sql(retained_url,"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name='cowork-recovery-proof-fixture';")
                blocker.wait(timeout=10)
            auto=True
            p.reconcile_operations(manager,infra)
            assert current(first)['state']=='stopped'
            operations.append({'action':'stop','id':operation['id'],'instance_id':first['id'],'capacity_recovery':True})
            wait(lambda:run_state(second,waiting)=='completed' and held()==0,'proven recovery did not unblock other instance')
            assert sql(management_db,f"SELECT recovery_operation_id FROM management_execution_reservations WHERE id='{reserved['id']}';")==operation['id']
            assert len(calls)==before+2
            assert p.run(['docker','ps','--all','--filter','name=^/cowork-'+uuid.UUID(first['id']).hex+'-','--format','{{.ID}}']).strip()==b''
            lifecycle(first,'start'); login(first)
            resumed=create(first)
            wait(lambda:run_state(first,resumed)=='completed' and held()==0,'new task after recovery failed')
            assert len(calls)==before+3
            passed('actual_crashed_worker_recovery_requires_all_generations_stopped_and_restarts_without_replay')
        if config.get('acceptance_managed_resources'):
            auto=True
            from enterprise_managed_resource_acceptance import verify_managed_resources
            managed_evidence=verify_managed_resources(request,wait,passed,management,base,admin,owner,first,tenant_sql)
            wait(lambda:held()==0,'a denied central dispatch retained its capacity slot')
            denied_receipt=json.loads(sql(management_db,f"SELECT completion FROM management_execution_reservations WHERE run_id='{managed_evidence['revoked_run_id']}';"))
            assert denied_receipt['outcome']=='failed' and denied_receipt['execution_stopped'] is True
            follow_up=create(second)
            wait(lambda:run_state(second,follow_up)=='completed' and held()==0,'another instance could not execute after central revocation')
            passed('central_denial_releases_only_proven_stopped_capacity_and_unblocks_another_instance')
        for item in fixtures:
            lifecycle(item,'delete',confirmation_name=item['name'],purge_backups=True)
            assert sql(management_db,f"SELECT state FROM management_instances WHERE id='{item['id']}';")=='deleted'
            assert all(row['id']!=item['id'] for row in request(management,base+'/instances',owner['access_token']))
        assert held()==0
        passed('new_test_instances_and_all_retained_generations_deleted_with_real_receipts')
    report={'checks':checks,'profile':'managed_resources' if config.get('acceptance_managed_resources_only') else 'lifecycle','managed_resources':managed_evidence,'operations':operations,'instance_ids':[i['id'] for i in fixtures],
        'model_requests':len(calls),'model_provider':'controlled local HTTP endpoint','infrastructure_receipts':'real',
        'release_transition':'different immutable package digests containing the same server binary',
        'migration':sql(management_db,'SELECT max(version) FROM _sqlx_migrations;'),
        'live_modified':False,'completed_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
    Path(args.output).write_text(json.dumps(report,indent=2))
    model.shutdown()


if __name__=='__main__': main()
