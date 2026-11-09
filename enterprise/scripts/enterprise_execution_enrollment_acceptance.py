"""Actual stopped legacy Compose instances, encrypted backups and PostgreSQL.

Only the separate test stack created with --enrollment-preflight or
--enrollment-lifecycle is supported. Instances initially run the current candidate
without admission; cross-version acceptance uses its own separate harness.
"""
import argparse
import copy
import json
from pathlib import Path
import secrets
import subprocess
import time
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import uuid

import provisioner as p
import execution_enrollment as enrollment
from instance_backups import pg_environment
from operation_journal import OperationJournal
from enterprise_execution_docker_acceptance import uid, totp, wait


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--config',required=True); parser.add_argument('--output',required=True)
    parser.add_argument('--management-container',required=True)
    parser.add_argument('--enroll',action='store_true')
    args=parser.parse_args()
    config=json.loads(Path(args.config).read_text())
    assert config['services_network'].startswith('cowork-admission-test-')
    scope=config['services_network'].removesuffix('-services')
    state=Path(config['state_directory']).parent
    assert state==Path('/var/lib/docker/volumes')/(scope+'-state')/'_data'
    labels=json.loads(p.run(['docker','inspect',args.management_container,'--format','{{json .Config.Labels}}']))
    assert labels['com.docker.compose.project']==scope
    legacy_config=copy.deepcopy(config); legacy_config.pop('execution_admission')
    manager=p.Manager(config); infra=p.Infrastructure(legacy_config); observer=p.Infrastructure(config)
    (state/'mc').mkdir(exist_ok=True,mode=0o700)
    p.run(['mc','--config-dir',str(state/'mc'),'alias','set','fixture',config['s3_endpoint'],
        Path('/run/secrets/s3_user').read_text().strip(),Path('/run/secrets/s3_password').read_text().strip()])
    p.run(['age-keygen','--output',config['backup_identity_file']])
    def sql(url,query):
        return p.run(['psql','-X','-qAt','-v','ON_ERROR_STOP=1'],env=pg_environment(url),data=query.encode()).decode().strip()
    def request(path,body=None,token=None,expected=None):
        req=Request(config['acceptance_management_url']+path,None if body is None else json.dumps(body).encode(),
                    {'Content-Type':'application/json',**({'Authorization':'Bearer '+token} if token else {})})
        try:
            with urlopen(req,timeout=30) as response:
                assert expected is None or response.status==expected
                return json.load(response)
        except HTTPError as error:
            if error.code!=expected:raise AssertionError(f'{req.method} {path}: {error.code}, expected {expected}') from None
            return None
    management_db=Path('/run/secrets/management_url').read_text().strip()
    assert sql(management_db,'SELECT count(*) FROM management_execution_activation;')=='0'
    bootstrap=Path('/run/secrets/bootstrap').read_text().strip()
    admin=request('/api/v1/auth/bootstrap',{'email':'enrollment@fixture.test','display_name':'Enrollment test',
        'password':secrets.token_urlsafe(32),'device_id':uid()},bootstrap)['access_token']
    setup=request('/api/v1/auth/totp/setup',{},admin)
    request('/api/v1/auth/totp/enable',{'code':totp(setup['secret'])},admin)
    org=request('/management/v1/organisations',{'name':'Enrollment preflight acceptance'},admin)['id']
    base='/management/v1/organisations/'+org
    checks=[]; operations=[]; descriptors={}
    def passed(name): checks.append(name); print(name+'=ok',flush=True)
    def current(instance): return next(v for v in request(base+'/instances',token=admin) if v['id']==instance['id'])
    def lifecycle(instance,action,**extra):
        ident=request(base+'/instances/'+instance['id']+'/operations',
            {'action':action,'expected_revision':current(instance)['revision'],'idempotency_key':uid(),**extra},admin)['id']
        op=manager.call('/claim',p.operator_contract(infra.config))
        assert op['id']==ident
        if action=='enroll_execution' or current(instance)['execution_required']:
            assert op['execution_binding'] and op['execution_runtime_required']
        else:assert op['execution_binding'] is None and not op['execution_runtime_required']
        descriptors[instance['id']]=op['instance']
        p.private_write(state/'acceptance-instances.json',json.dumps(descriptors).encode())
        journal=OperationJournal(infra,op); journal.begin()
        result=infra.execute(op); completion=journal.finish(True,result)
        manager.call('/operations/'+ident+'/complete',completion)
        operations.append({'id':ident,'action':action,'instance_id':instance['id']})
        return result
    def inspect(instance,backup): return enrollment.inspect(observer,current(instance),backup['backup_id'])
    def expect_failure(instance,backup,code):
        try: inspect(instance,backup)
        except Exception as error: assert str(error)==code,(type(error).__name__,str(error))
        else: raise AssertionError('unsafe preflight passed: '+code)
    with p.operator_lock(infra.root):
        first=request(base+'/instances',{'name':'Docker admission fixture enrollment','backend':'compose'},admin)
        lifecycle(first,'provision'); lifecycle(first,'stop')
        backup=lifecycle(first,'backup')
        directory=infra.directory(first['id'])
        original={str(path.relative_to(directory)):path.read_bytes() for path in directory.rglob('*') if path.is_file()}
        good=inspect(first,backup)
        assert good['checks_passed'] and good['blockers']==[] and good['verified_generations']==1
        assert good['eligible_for_enrollment'] is False and good['retained_containers']==4
        assert original=={str(path.relative_to(directory)):path.read_bytes() for path in directory.rglob('*') if path.is_file()}
        assert not (directory/'execution.json').exists()
        assert sql(management_db,"SELECT execution_required FROM management_instances WHERE id='"+first['id']+"';")=='f'
        assert not list(Path('/tmp').glob('cowork-enrollment-check-*'))
        passed('real_legacy_inventory_and_age_backup_verified_without_instance_or_management_changes')
        lifecycle(first,'restore',backup_id=backup['backup_id'])
        refreshed=lifecycle(first,'backup')
        good=inspect(first,refreshed)
        assert good['checks_passed'] and good['verified_generations']==2
        passed('active_and_retained_real_database_generations_are_both_inspected')
        retained_url=(directory/'secrets/database_url').read_text().strip()
        active=infra.active_directory(directory)
        active_url=(active/'secrets/database_url').read_text().strip()
        blocker=subprocess.Popen(['psql','-X','-tAc','SELECT pg_sleep(60);'],env={**pg_environment(retained_url),'PGAPPNAME':'enrollment-readonly-fixture'},
            stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            wait(lambda:sql(retained_url,"SELECT count(*) FROM pg_stat_activity WHERE application_name='enrollment-readonly-fixture';")=='1','missing retained connection')
            denied=inspect(first,refreshed)
            assert not denied['checks_passed'] and 'database_connections_present' in denied['blockers']
        finally:
            sql(retained_url,"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name='enrollment-readonly-fixture';")
            blocker.wait(timeout=10)
        passed('retained_generation_connection_is_not_mistaken_for_quiescence')
        run_id=uid()
        sql(retained_url,f"INSERT INTO runs(id,thread_id,project_id,creator_user_id,idempotency_key,target_kind,target_device_id,state,spec,assigned_executor_id,created_at,updated_at) VALUES('{run_id}','{uid()}','{uid()}','{uid()}','enrollment-fixture','personal_device','{uid()}','running','{{}}','{uid()}',now(),now());")
        denied=inspect(first,refreshed)
        assert 'unfinished_runs' in denied['blockers'] and 'external_assignment_history' in denied['blockers']
        sql(retained_url,f"UPDATE runs SET state='expired' WHERE id='{run_id}';")
        denied=inspect(first,refreshed)
        assert 'unfinished_runs' not in denied['blockers'] and 'external_assignment_history' in denied['blockers']
        sql(retained_url,f"UPDATE runs SET state='completed',target_kind='server_linux',target_device_id=NULL WHERE id='{run_id}';")
        denied=inspect(first,refreshed)
        assert 'legacy_assignments_requiring_review' in denied['blockers']
        assert 'external_assignment_history' not in denied['blockers']
        sql(retained_url,f"DELETE FROM runs WHERE id='{run_id}';")
        passed('offline_assignment_and_expired_legacy_assignment_require_separate_reconciliation')
        # This mutation creates only disposable test data. The inspection query
        # itself is executed under database-enforced read-only transactions.
        try: enrollment.sql(observer,pg_environment(active_url),'CREATE TABLE enrollment_must_not_write(id INT);')
        except p.ProvisioningError: pass
        else: raise AssertionError('preflight database session was writable')
        assert sql(active_url,"SELECT to_regclass('public.enrollment_must_not_write') IS NULL;")=='t'
        passed('database_server_enforces_read_only_inspection')
        archive=Path(config['backup_directory'])/(uuid.UUID(refreshed['backup_id']).hex+'.age')
        original_archive=archive.read_bytes()
        try:
            archive.write_bytes(original_archive[:-1]+bytes([original_archive[-1]^1]))
            expect_failure(first,refreshed,'enrollment_backup_integrity_failed')
        finally: archive.write_bytes(original_archive)
        assert not list(Path('/tmp').glob('cowork-enrollment-check-*'))
        second=request(base+'/instances',{'name':'Docker admission fixture foreign backup','backend':'compose'},admin)
        lifecycle(second,'provision'); lifecycle(second,'stop'); foreign=lifecycle(second,'backup')
        expect_failure(first,foreign,'enrollment_backup_scope_mismatch')
        passed('tampered_and_foreign_real_backups_are_rejected_without_plaintext_leaks')
        lifecycle(first,'start')
        denied=inspect(first,refreshed)
        assert not denied['checks_passed'] and 'containers_active' in denied['blockers'] and 'database_connections_present' in denied['blockers']
        lifecycle(first,'stop')
        assert inspect(first,refreshed)['checks_passed']
        passed('actually_running_containers_and_connections_block_preflight')
        # Real CLI refuses concurrent ownership instead of inspecting during an
        # operator mutation; no existing lock is stolen or treated as expired.
        cli=['python3','/opt/cowork-provisioner/execution_enrollment.py','--config',args.config,
             '--instance-id',first['id'],'--organisation-id',org,'--backup-id',refreshed['backup_id']]
        held=subprocess.run(cli,capture_output=True,text=True)
        assert held.returncode==2 and json.loads(held.stdout)['error_code']=='enrollment_preflight_unproven'
        assert held.stderr==''
        passed('cli_refuses_concurrent_operator_lock_without_secret_output')
    observed=subprocess.run(cli,capture_output=True,text=True)
    assert observed.returncode==0,observed.stdout
    assert json.loads(observed.stdout)['checks_passed'] and observed.stderr==''
    passed('standalone_operator_cli_reports_observations_without_activating_admission')
    model_requests=[]
    if args.enroll:
        from execution_generation import ExecutionGenerationError, read as read_epoch
        enrolled_config={**config,'execution_enrollment_enabled':True,'fleet_management_url':'http://fixture-management-active:8080'}
        infra=p.Infrastructure(enrolled_config)
        def begin_enrollment(instance):
            operation_id=request(base+'/instances/'+instance['id']+'/operations',
                {'action':'enroll_execution','expected_revision':current(instance)['revision'],'idempotency_key':uid(),'acknowledge_execution_enrollment':True},admin)['id']
            op=manager.call('/claim',p.operator_contract(infra.config))
            assert op['id']==operation_id and op['execution_binding'] and op['execution_runtime_required']
            descriptors[instance['id']]=op['instance'];p.private_write(state/'acceptance-instances.json',json.dumps(descriptors).encode())
            journal=OperationJournal(infra,op);journal.begin()
            return op,journal
        with p.operator_lock(infra.root):
            first_dir=infra.directory(first['id']);first_active=infra.active_directory(first_dir)
            data_url=(first_active/'secrets/database_url').read_text().strip()
            # Completed local history remains in place and in the fresh backup.
            historical=uid()
            sql(data_url,f"INSERT INTO runs(id,thread_id,project_id,creator_user_id,idempotency_key,target_kind,state,spec,assigned_executor_id,created_at,updated_at) VALUES('{historical}','{uid()}','{uid()}','{uid()}','historical-local','server_linux','completed','{{}}','{uid()}',now(),now());")
            external=uid()
            sql(data_url,f"INSERT INTO executors(id,kind,registration,protocol_version,max_concurrent_runs,last_seen_at) VALUES('{external}','server_linux','{{}}',2,1,now());")
            source_manifest=(first_dir/'compose.json').read_bytes()
            op,journal=begin_enrollment(first)
            try:infra.execute(op)
            except p.ProvisioningError as error:
                assert str(error)=='execution_enrollment_precheck_failed'
                completion=journal.finish(False,{**error.result,'error_code':str(error)})
                manager.call('/operations/'+op['id']+'/complete',completion)
                operations.append({'action':'enroll_execution','id':op['id'],'outcome':'unchanged','instance_id':first['id']})
            else:raise AssertionError('unreconciled executor was silently enrolled')
            assert not current(first)['execution_required'] and read_epoch(current(first),first_active) is None
            assert (first_dir/'compose.json').read_bytes()==source_manifest
            sql(data_url,f"DELETE FROM executors WHERE id='{external}';")
            passed('actual_enrollment_rejects_unreconciled_executor_without_rewriting_legacy_runtime')
            op,journal=begin_enrollment(first)
            original_write=infra.write_compose
            def fail_write(*unused):raise OSError('injected crash after epoch commit')
            infra.write_compose=fail_write
            try:
                try:infra.execute(op)
                except ExecutionGenerationError as error:assert str(error)=='execution_enrollment_requires_reconciliation'
                else:raise AssertionError('enrollment interruption not injected')
            finally:infra.write_compose=original_write
            epoch=read_epoch(op['instance'],first_active)
            assert epoch==op['execution_binding']['source_epoch'] and journal.recover() is None
            assert not current(first)['execution_required'] and (first_dir/'compose.json').read_bytes()==source_manifest
            manager.call('/operations/'+op['id']+'/reconciliation',{'lease_token':op['lease_token'],'reason':'operation_recovery_requires_review'})
            p.reconcile_operations(manager,infra)
            assert (first_dir/'compose.json').read_bytes()==source_manifest and journal.recover() is None
            request(base+'/instances/'+first['id']+'/operations/'+op['id']+'/recover',{'mode':'continue_enrollment'},admin)
            p.reconcile_operations(manager,infra)
            assert current(first)['execution_required'] and current(first)['state']=='stopped'
            assert journal.recover()['result']['execution_enrollment']['outcome']=='enrolled'
            manager.call('/operations/'+op['id']+'/complete',journal.recover())
            operations.append({'action':'enroll_execution','id':op['id'],'outcome':'enrolled_after_explicit_recovery','instance_id':first['id']})
            assert sql(data_url,f"SELECT state FROM runs WHERE id='{historical}';")=='completed'
            assert p.run(['docker','ps','--all','--filter','name=^/cowork-'+uuid.UUID(first['id']).hex+'-','--format','{{.ID}}']).strip()==b''
            request(base+'/instances/'+first['id']+'/operations',{'action':'start','expected_revision':current(first)['revision'],'idempotency_key':uid()},admin,503)
            passed('interrupted_real_enrollment_never_replays_automatically_and_recovers_same_epoch_and_backup')
            lifecycle(second,'enroll_execution',acknowledge_execution_enrollment=True)
            assert current(second)['execution_required'] and current(second)['state']=='stopped'
            assert sql(management_db,'SELECT count(*) FROM management_execution_activation;')=='0'
            Path('/fixture/activation-request.json').write_text(json.dumps({'scope':scope}))
            wait(lambda:Path('/fixture/activation-ready.json').is_file(),'activation service did not become ready',seconds=150)
            assert json.loads(Path('/fixture/activation-ready.json').read_text())=={'scope':scope}
            config['acceptance_management_url']='http://fixture-management-active:8080'
            manager=p.Manager({**enrolled_config,'management_url':'https://operator-control-active:8443'})
            assert sql(management_db,'SELECT count(*) FROM management_execution_activation;')=='1'
            for item in [first,second]:lifecycle(item,'start')
            passed('verified_legacy_instances_allow_durable_activation_then_start_with_bound_runtime')
            class Model(BaseHTTPRequestHandler):
                def log_message(self,*unused):pass
                def do_POST(self):
                    model_requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                    body=json.dumps({'choices':[{'index':0,'message':{'role':'assistant','content':'Enrollment succeeded.'},'finish_reason':'stop'}],
                        'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}).encode()
                    self.send_response(200);self.send_header('content-type','application/json');self.send_header('content-length',str(len(body)));self.end_headers();self.wfile.write(body)
            model=ThreadingHTTPServer(('0.0.0.0',9001),Model)
            threading.Thread(target=model.serve_forever,daemon=True).start()
            origin='http://cowork-'+uuid.UUID(first['id']).hex+'-api-1:8080'
            def tenant(path,body,token=None):
                req=Request(origin+path,json.dumps(body).encode(),{'content-type':'application/json',**({'authorization':'Bearer '+token} if token else {})})
                with urlopen(req,timeout=30) as response:return json.load(response)
            launch=request(base+'/instances/'+first['id']+'/launch',{},admin)
            access=tenant('/api/v1/auth/fleet',{'ticket':launch['ticket'],'device_id':uid()})['access_token']
            team=tenant('/api/v1/teams',{'name':'Enrolled fixture'},access)
            project=tenant('/api/v1/projects',{'name':'Enrolled fixture','privacy':'team_managed','team_id':team['id']},access)
            profile=tenant('/api/v1/provider-profiles',{'name':'Local fixture','provider_kind':'openai-compatible','team_id':None,
                'model_defaults':{'base_url':'http://fixture-model:9001/v1','model':'fixture','auth_mode':'none'}},access)
            thread=tenant('/api/v1/threads',{'project_id':project['id'],'title':'Enrollment test'},access)
            created=tenant('/api/v1/runs',{'project_id':project['id'],'thread_id':thread['id'],'project_revision':project['revision'],
                'project_privacy':'team_managed','executor_target':{'kind':'server_linux','pool_id':None},'model_profile_id':profile['id'],
                'required_capabilities':['model.external'],'input':{'prompt':'Reply with enrollment success.'},'idempotency_key':uid()},access)
            run_id=created['spec']['id']
            wait(lambda:sql(data_url,f"SELECT state FROM runs WHERE id='{run_id}';")=='completed','enrolled worker did not complete a task')
            wait(lambda:sql(management_db,f"SELECT state FROM management_execution_reservations WHERE run_id='{run_id}';")=='released','enrolled outbox did not settle')
            assert len(model_requests)==1 and sql(data_url,f"SELECT execution_epoch FROM instance_execution_reservations WHERE run_id='{run_id}';")==epoch
            assert sql(data_url,f"SELECT state FROM runs WHERE id='{historical}';")=='completed'
            model.shutdown()
            passed('formerly_legacy_worker_preserves_history_and_executes_once_through_central_admission')
    with p.operator_lock(infra.root):
        for instance in [first,second]:
            lifecycle(instance,'delete',confirmation_name=current(instance)['name'],purge_backups=True)
        assert sql(management_db,'SELECT count(*) FROM management_execution_activation;')==('1' if args.enroll else '0')
        assert sql(management_db,"SELECT count(*) FROM management_instances WHERE state<>'deleted';")=='0'
        passed('new_test_instances_and_retained_generations_deleted_with_real_receipts')
    Path(args.output).write_text(json.dumps({'checks':checks,'operations':operations,'instance_ids':[first['id'],second['id']],
        'migration':sql(management_db,'SELECT max(version) FROM _sqlx_migrations;'),'infrastructure_receipts':'real',
        'legacy_definition':'current candidate server with admission disabled, not a cross-version migration',
        'enrollment_implemented':args.enroll,'model_requests':len(model_requests),'live_modified':False,'completed_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())},indent=2))


if __name__=='__main__': main()
