"""Actual installed-release upgrade in a disposable, explicitly scoped fleet.

Source runs inside the installed provisioner image; later phases use the candidate.
Private context and encrypted backup never leave the generated test installation.
"""
import argparse
import base64
import copy
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import secrets
import re
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler
import uuid
from datetime import datetime

import provisioner as p
from instance_backups import pg_environment
from operation_journal import OperationJournal


def uid(): return str(uuid.uuid4())


def wait(predicate, message, seconds=90):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        if predicate(): return
        time.sleep(.2)
    raise AssertionError(message)


def totp(secret):
    digest=hmac.new(base64.b32decode(secret+'='*((8-len(secret)%8)%8)),
                    (int(time.time())//30).to_bytes(8,'big'),hashlib.sha1).digest()
    i=digest[-1]&15
    return f'{(int.from_bytes(digest[i:i+4],"big")&0x7fffffff)%1000000:06d}'


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--phase',choices=['source','backup','verify-backout','target'],required=True)
    args=parser.parse_args()
    config=json.loads(Path('/fixture/config.json').read_text())
    scope=config['services_network'].removesuffix('-services')
    assert scope.startswith('cowork-admission-test-') and config['routing_network']==scope+'-routing'
    state=Path(config['state_directory']).parent
    assert state==Path('/var/lib/docker/volumes')/(scope+'-state')/'_data'
    network=json.loads(p.run(['docker','network','inspect',config['services_network']]))[0]
    assert network['Labels']['com.docker.compose.project']==scope
    context_path=state/'cross-version-context.json'
    context=json.loads(context_path.read_text()) if context_path.exists() else {'checks':[],'operations':[],'instances':[],'model_requests':0}
    def save(): p.private_write(context_path,json.dumps(context).encode())
    def passed(name): context['checks'].append(name);save();print(name+'=ok',flush=True)
    def sql(url,statement):
        return p.run(['psql','-X','-qAt','-v','ON_ERROR_STOP=1'],env=pg_environment(url),data=statement.encode()).decode().strip()
    db=Path('/run/secrets/management_url').read_text().strip()
    http=build_opener(ProxyHandler({}))
    admin_aliases={context['admin']} if context.get('admin') else set()
    def request(origin,path,body=None,token=None,expected=None,raw=False):
        if token in admin_aliases:
            if origin=='http://fixture-management-recovery:8080':
                key='backout_tokens'
                if key not in context: context[key]=copy.deepcopy(context['management_tokens'])
            else:
                assert origin in [config['acceptance_management_url'],'http://fixture-management-active:8080']
                key='management_tokens'
            tokens=context[key]
            if datetime.fromisoformat(tokens['access_expires_at'].replace('Z','+00:00')).timestamp()<time.time()+60:
                updated=request(origin,'/api/v1/auth/refresh',{'refresh_token':tokens['refresh_token']})
                assert updated['user_id']==tokens['user_id'] and updated['session_id']==tokens['session_id']
                context[key]=tokens=updated
                if key=='management_tokens':
                    context['admin']=updated['access_token'];admin_aliases.add(context['admin'])
                save()
            token=tokens['access_token']
        req=Request(origin+path,None if body is None else body if isinstance(body,bytes) else json.dumps(body).encode(),
                    {'content-type':'application/octet-stream' if isinstance(body,bytes) else 'application/json',**({'authorization':'Bearer '+token} if token else {})})
        try:
            with http.open(req,timeout=30) as response:
                assert expected is None or response.status==expected
                return response.read() if raw else json.load(response)
        except HTTPError as error:
            if error.code!=expected: raise AssertionError(f'{req.get_method()} {path}: {error.code}, expected {expected}') from None
            return None
    origin=config['acceptance_management_url']
    def metadata(url):
        # Migration adds nullable/defaulted operational fields. Compare all source
        # columns, not a row encoding that necessarily changes with the schema.
        added="ARRAY['execution_binding','execution_recovery_binding','execution_runtime_required']"
        return {table:sql(url,f"SELECT md5(coalesce(string_agg((to_jsonb(t)-{added})::text,E'\\n' ORDER BY {'sequence' if table=='management_audit' else 'id'}),'')) FROM {table} t;")
                for table in ['management_organisations','management_operations','management_audit']}
    if args.phase=='backup':
        assert sql(db,'SELECT max(version) FROM _sqlx_migrations;')=='43'
        assert metadata(db)==context['management_metadata']
        assert sql(db,"SELECT count(*) FROM management_operations WHERE state IN ('queued','running','reconciling');")=='0'
        backup=state/'management-source.dump.age'
        assert not backup.exists()
        with tempfile.TemporaryDirectory(prefix='cross-version-backup-') as temporary:
            plain=Path(temporary)/'management.dump'
            p.private_write(plain,p.run(['pg_dump','--format=custom','--no-owner','--no-acl'],env=pg_environment(db)))
            digest=hashlib.sha256(plain.read_bytes()).hexdigest()
            recipient=p.run(['age-keygen','-y',config['backup_identity_file']]).decode().strip()
            p.run(['age','-r',recipient,'-o',str(backup),str(plain)])
            verified=Path(temporary)/'verified.dump'
            p.run(['age','-d','-i',config['backup_identity_file'],'-o',str(verified),str(backup)])
            assert hashlib.sha256(verified.read_bytes()).hexdigest()==digest
            assert sql(db,"SELECT count(*) FROM pg_database WHERE datname='fixture_recovery';")=='0'
            sql(db,'CREATE DATABASE fixture_recovery;')
            recovery=db.rsplit('/',1)[0]+'/fixture_recovery'
            p.run(['pg_restore','--exit-on-error','--no-owner','--no-acl','--dbname','fixture_recovery',str(verified)],env=pg_environment(recovery))
        assert metadata(recovery)==context['management_metadata']
        context['management_backup_sha256']=hashlib.sha256(backup.read_bytes()).hexdigest()
        passed('actual_source_management_database_encrypted_backup_restores_identical_metadata')
        return
    if args.phase=='verify-backout':
        recovery=db.rsplit('/',1)[0]+'/fixture_recovery'
        assert sql(recovery,'SELECT max(version) FROM _sqlx_migrations;')=='43'
        recovered='http://fixture-management-recovery:8080'
        assert request(recovered,'/readyz')['status']=='ready'
        rows=request(recovered,'/management/v1/organisations/'+context['org']+'/instances',token=context['admin'])
        assert {v['id'] for v in rows}=={v['id'] for v in context['instances']}
        assert all(v['state']=='stopped' for v in rows)
        assert metadata(recovery)==context['management_metadata']
        passed('installed_management_image_starts_from_restored_backup_and_accepts_existing_identity')
        return
    source=args.phase=='source'
    working=copy.deepcopy(config);working.pop('execution_admission')
    infra=p.Infrastructure(working)
    manager=p.Manager(config)
    if source:
        assert sql(db,'SELECT max(version) FROM _sqlx_migrations;')=='43'
        (state/'mc').mkdir(mode=0o700,exist_ok=True)
        p.run(['mc','--config-dir',str(state/'mc'),'alias','set','fixture',config['s3_endpoint'],
            Path('/run/secrets/s3_user').read_text().strip(),Path('/run/secrets/s3_password').read_text().strip()])
        p.run(['age-keygen','--output',config['backup_identity_file']])
        context['management_tokens']=request(origin,'/api/v1/auth/bootstrap',{'email':'cross-version@fixture.test',
            'display_name':'Cross-version fixture','password':secrets.token_urlsafe(32),'device_id':uid()},
            Path('/run/secrets/bootstrap').read_text().strip())
        context['admin']=context['management_tokens']['access_token'];admin_aliases.add(context['admin'])
        setup=request(origin,'/api/v1/auth/totp/setup',{},context['admin'])
        request(origin,'/api/v1/auth/totp/enable',{'code':totp(setup['secret'])},context['admin'])
        context['org']=request(origin,'/management/v1/organisations',{'name':'Cross-version fixture'},context['admin'])['id']
        save()
    else:
        assert sql(db,'SELECT max(version) FROM _sqlx_migrations;')=='48'
        assert metadata(db)==context['management_metadata']
        assert sql(db,'SELECT count(*) FROM management_execution_activation;')=='0'
        passed('candidate_management_migrates_43_to_48_without_changing_organisations_operations_or_audit')
        previous=context['management_tokens']
        updated=request(origin,'/api/v1/auth/refresh',{'refresh_token':previous['refresh_token']})
        assert updated['session_id']==previous['session_id'] and updated['user_id']==previous['user_id']
        assert updated['refresh_token']!=previous['refresh_token']
        context['management_tokens']=updated;context['admin']=updated['access_token'];admin_aliases.add(context['admin'])
        passed('original_management_session_refreshes_after_upgrade_with_its_existing_identity')
    admin=context['admin'];base='/management/v1/organisations/'+context['org']
    def current(item): return next(v for v in request(origin,base+'/instances',token=admin) if v['id']==item['id'])
    def active(item): return infra.active_directory(infra.directory(item['id']))
    def tenant_sql(item,statement): return sql((active(item)/'secrets/database_url').read_text().strip(),statement)
    def lifecycle(item,action,expected_error=None,**extra):
        if action in ['backup','upgrade','enroll_execution']:
            # Direct SQL fixture setup/inspection can exit just before PostgreSQL
            # removes its backend. Wait for those sessions; never terminate them
            # or weaken the provisioner's independent quiescence check.
            database=infra.resource_names(current(item),active(item))['database']
            assert re.fullmatch('ci_[a-f0-9]{32}',database)
            admin_url=Path(config['database_admin_url_file']).read_text().strip()
            wait(lambda:sql(admin_url,f"SELECT count(*) FROM pg_stat_activity WHERE datname='{database}';")=='0',
                 'fixture inspection connection did not close')
        ident=request(origin,base+'/instances/'+item['id']+'/operations',
            {'action':action,'expected_revision':current(item)['revision'],'idempotency_key':uid(),**extra},admin)['id']
        op=manager.call('/claim',{'backend':'compose'} if source else p.operator_contract(infra.config))
        assert op['id']==ident
        descriptor_path=state/'acceptance-instances.json'
        descriptors=json.loads(descriptor_path.read_text()) if descriptor_path.exists() else {}
        # Deleted fixtures disappear from the API; keep their last scoped record
        # for cleanup and update only the instance this claim actually describes.
        descriptors[item['id']]=op['instance']
        p.private_write(descriptor_path,json.dumps(descriptors).encode())
        journal=OperationJournal(infra,op);journal.begin()
        succeeded=True
        try: result=infra.execute(op)
        except p.ProvisioningError as error:
            if expected_error is None or str(error)!=expected_error: raise
            succeeded=False;result={**error.result,'error_code':str(error)}
        else: assert expected_error is None, 'expected infrastructure rejection did not occur'
        completion=journal.finish(succeeded,result)
        manager.call('/operations/'+ident+'/complete',completion)
        manager.call('/operations/'+ident+'/complete',journal.recover())
        context['operations'].append({'id':ident,'action':action,'instance_id':item['id']});save()
        return result
    def login(item):
        ticket=request(origin,base+'/instances/'+item['id']+'/launch',{},admin)['ticket']
        item['token']=request(item['origin'],'/api/v1/auth/fleet',{'ticket':ticket,'device_id':uid()})['access_token']
    calls=[]
    class Model(BaseHTTPRequestHandler):
        def log_message(self,*unused): pass
        def do_POST(self):
            calls.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            body=json.dumps({'choices':[{'index':0,'message':{'role':'assistant','content':'Cross-version preserved.'},'finish_reason':'stop'}],
                             'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}).encode()
            self.send_response(200);self.send_header('content-type','application/json');self.send_header('content-length',str(len(body)));self.end_headers();self.wfile.write(body)
    model=ThreadingHTTPServer(('0.0.0.0',9001),Model)
    threading.Thread(target=model.serve_forever,daemon=True).start()
    def profile(item):
        return request(item['origin'],'/api/v1/provider-profiles',{'name':'Controlled source' if source else 'Controlled target',
            'provider_kind':'openai-compatible','team_id':None,'model_defaults':{'base_url':('http://fixture-model-source:9001/v1' if source else 'http://fixture-model:9001/v1'),
            'model':'fixture','auth_mode':'none'}},item['token'])['id']
    def run_task(item):
        thread=request(item['origin'],'/api/v1/threads',{'project_id':item['project']['id'],'title':'Cross-version task'},item['token'])
        run=request(item['origin'],'/api/v1/runs',{'project_id':item['project']['id'],'thread_id':thread['id'],
            'project_revision':item['project']['revision'],'project_privacy':'team_managed','executor_target':{'kind':'server_linux','pool_id':None},
            'model_profile_id':item['profile'],'required_capabilities':['model.external'],'input':{'prompt':'Reply with the preservation marker.'},
            'idempotency_key':uid()},item['token'])['spec']['id']
        wait(lambda:tenant_sql(item,f"SELECT state FROM runs WHERE id='{run}';")=='completed','actual worker did not complete the model task')
        return run
    def shell_task(item):
        thread=request(item['origin'],'/api/v1/threads',{'project_id':item['project']['id'],'title':'Persistent workspace'},item['token'])
        command="from pathlib import Path; Path('/workspace/upgrade-marker.txt').write_text("+repr(item['file_marker'])+"); print('workspace-ready')"
        run=request(item['origin'],'/api/v1/runs',{'project_id':item['project']['id'],'thread_id':thread['id'],
            'project_revision':item['project']['revision'],'project_privacy':'team_managed','executor_target':{'kind':'server_linux','pool_id':None},
            'required_capabilities':['shell'],'input':{'prompt':'Write the workspace preservation fixture.','sandbox':{
                'schema_version':2,'run_id':uid(),'image':'core','argv':['python3','-c',command],'network':'none'}},'idempotency_key':uid()},item['token'])['spec']['id']
        wait(lambda:tenant_sql(item,f"SELECT state FROM runs WHERE id='{run}';")=='completed','actual runner did not complete the shell task')
        return run
    def workspace_preserved(item,run):
        resource=infra.resource_names(current(item),active(item))
        volume=resource['volume_prefix']+uuid.UUID(run).hex
        p.run(['docker','volume','inspect',volume]) # Never auto-create a missing volume while inspecting.
        value=p.run(['docker','run','--rm','--pull','never','--network','none','--read-only','--cap-drop','ALL',
            '--security-opt','no-new-privileges:true','--mount','type=volume,src='+volume+',dst=/workspace,readonly',
            config['images']['sandbox_core'],'cat','/workspace/upgrade-marker.txt'])
        assert value.decode()==item['file_marker'], 'persistent workspace file changed'
    def content(item):
        excluded=','.join("'"+v['id']+"'" for v in item.get('dormant',[]))
        return {table:tenant_sql(item,f"SELECT md5(coalesce(string_agg(row_to_json(t)::text,E'\\n' ORDER BY id),'')) FROM {table} t"
                +(f' WHERE id NOT IN ({excluded})' if table=='runs' and excluded else '')+';')
                for table in ['projects','threads','runs','messages']}
    def preserved(item):
        assert content(item)==item['content'], 'source project/chat/run content changed'
        assert hashlib.sha256((active(item)/'secrets/storage_master_key').read_bytes()).hexdigest()==item['storage_key_sha256']
        if item.get('source_shell'): workspace_preserved(item,item['source_shell'])
        for row in item.get('dormant',[]):
            value=json.loads(tenant_sql(item,f"SELECT json_build_object('state',state,'spec',spec,'clear_lease',lease_owner IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL) FROM runs WHERE id='{row['id']}';"))
            assert value['state']==('interrupted' if item.get('promoted') else row['state']) and value['clear_lease']
            assert value['spec']==row['spec'], 'dormant task content changed'
    with p.operator_lock(infra.root):
        if source:
            for n in range(2):
                name=f'Docker admission fixture cross-version {n}'
                item=request(origin,base+'/instances',{'name':name,'backend':'compose'},admin)
                item={'id':item['id'],'name':name,'origin':'http://cowork-'+uuid.UUID(item['id']).hex+'-api-1:8080'}
                context['instances'].append(item);save()
                lifecycle(item,'provision');login(item)
                assert tenant_sql(item,'SELECT max(version) FROM _sqlx_migrations;')==str(config.get('acceptance_source_migration',43))
                team=request(item['origin'],'/api/v1/teams',{'name':'Source data'},item['token'])
                item['project']=request(item['origin'],'/api/v1/projects',{'name':'Keep this project','privacy':'team_managed','team_id':team['id']},item['token'])
                item['profile']=profile(item);item['source_run']=run_task(item)
                if config.get('acceptance_dormant_history'):
                    item['file_marker']='cross-version-file-'+uid()
                    item['source_shell']=shell_task(item)
                    artifact=request(item['origin'],'/api/v1/runs/'+item['source_shell']+'/attachments?name=upgrade-marker.txt',
                        item['file_marker'].encode(),item['token'])
                    assert artifact['storage']=='object_store_encrypted'
                    item['artifact_id']=artifact['id']
                    workspace_preserved(item,item['source_shell'])
                lifecycle(item,'stop')
                if config.get('acceptance_dormant_history'):
                    # Deliberate SQL fixtures for historical states, not simulated
                    # claims of a real worker crash. Actual old/new tasks above
                    # and below run through their respective application binaries.
                    item['dormant']=[]
                    for status in ['interrupted','waiting_for_snapshot']:
                        ident=uid()
                        changes={'id':ident,'state':status,'idempotency_key':ident,'lease_owner':None,'lease_token':None,
                                 'lease_expires_at':None,'result':None,'finished_at':None}
                        if status=='waiting_for_snapshot':changes.update(assigned_executor_id=None,started_at=None)
                        value=json.dumps(changes).replace("'","''")
                        tenant_sql(item,f"INSERT INTO runs SELECT (jsonb_populate_record(NULL::runs,to_jsonb(r)||'{value}'::jsonb||jsonb_build_object('spec',jsonb_set(spec,'{{id}}',to_jsonb('{ident}'::text))))).* FROM runs r WHERE id='{item['source_run']}';")
                        spec=json.loads(tenant_sql(item,f"SELECT spec FROM runs WHERE id='{ident}';"))
                        item['dormant'].append({'id':ident,'state':status,'spec':spec})
                item['content']=content(item)
                item['storage_key_sha256']=hashlib.sha256((active(item)/'secrets/storage_master_key').read_bytes()).hexdigest()
                lifecycle(item,'backup')
                save()
            assert len(calls)==2
            context['management_metadata']=metadata(db)
            passed('installed_images_create_two_real_source_instances_with_completed_chats_and_backups')
        else:
            from release_catalog import ReleaseCatalog, image_set_id
            enrolled_config={**config,'execution_enrollment_enabled':True,'fleet_management_url':'http://fixture-management-active:8080'}
            infra=p.Infrastructure(enrolled_config)
            for item in context['instances']:preserved(item)
            source_images=infra.deployed_images(infra.directory(context['instances'][0]['id']))
            target_images=config.get('acceptance_target_images',{**source_images,'server':config['acceptance_target_image']})
            catalog=ReleaseCatalog(config,p.private_write)
            catalog.publish('Installed source release',source_images,[image_set_id(target_images)])
            target=catalog.publish('Admission enrollment candidate',target_images,[image_set_id(source_images)])
            p.sync_release_catalog(manager,infra)
            first=context['instances'][0]
            canary=lifecycle(first,'upgrade',release=target['id'],qualification_only=True)
            assert canary['rollback_verified'] and infra.deployed_images(infra.directory(first['id']))==source_images
            assert tenant_sql(first,'SELECT max(version) FROM _sqlx_migrations;')==str(config.get('acceptance_source_migration',43))
            preserved(first)
            passed('real_cross_version_canary_migrates_copy_and_verifies_return_to_unchanged_source')
            for item in context['instances']:
                lifecycle(item,'upgrade',release=target['id'],qualification_only=False)
                assert infra.deployed_images(infra.directory(item['id']))==target_images
                assert tenant_sql(item,'SELECT max(version) FROM _sqlx_migrations;')=='48'
                item['promoted']=True
                preserved(item)
            passed('qualified_cross_version_promotions_preserve_chat_content_and_storage_keys')
            if first.get('dormant'):
                ident=first['dormant'][0]['id']
                directory=infra.directory(first['id']);before=(directory/'compose.json').read_bytes()
                tenant_sql(first,f"UPDATE runs SET state='queued' WHERE id='{ident}';")
                result=lifecycle(first,'enroll_execution',acknowledge_execution_enrollment=True,expected_error='execution_enrollment_precheck_failed')
                assert result['execution_enrollment']['outcome']=='unchanged' and not current(first)['execution_required']
                assert (directory/'compose.json').read_bytes()==before
                tenant_sql(first,f"UPDATE runs SET state='interrupted' WHERE id='{ident}';")
                passed('real_runnable_task_blocks_enrollment_while_dormant_server_history_stays_preserved')
            # Old journal and retained old database formats must remain inspectable.
            for item in context['instances']:
                proof=lifecycle(item,'enroll_execution',acknowledge_execution_enrollment=True)
                assert proof['execution_enrollment']['verified_generations']>=2
                assert current(item)['execution_required'] and current(item)['state']=='stopped'
                preserved(item)
                request(origin,base+'/instances/'+item['id']+'/operations',{'action':'start',
                    'expected_revision':current(item)['revision'],'idempotency_key':uid()},admin,503)
            passed('upgraded_instances_enroll_with_real_legacy_journals_and_retained_source_generations')
            Path('/fixture/activation-request.json').write_text(json.dumps({'scope':scope}))
            wait(lambda:Path('/fixture/activation-ready.json').is_file(),'activation did not become ready',150)
            assert json.loads(Path('/fixture/activation-ready.json').read_text())=={'scope':scope}
            origin='http://fixture-management-active:8080'
            manager=p.Manager({**enrolled_config,'management_url':'https://operator-control-active:8443'})
            for item in context['instances']:
                lifecycle(item,'start');login(item)
                # Old completed tasks cannot be replayed by the replacement worker.
                preserved(item)
                if item.get('artifact_id'):
                    assert request(item['origin'],'/api/v1/runs/'+item['source_shell']+'/artifacts/'+item['artifact_id'],token=item['token'],raw=True)==item['file_marker'].encode()
                    new_shell=shell_task(item);workspace_preserved(item,new_shell)
                    wait(lambda:sql(db,f"SELECT state FROM management_execution_reservations WHERE run_id='{new_shell}';")=='released','new shell task did not settle central admission')
                item['profile']=profile(item)
                run=run_task(item)
                wait(lambda:sql(db,f"SELECT state FROM management_execution_reservations WHERE run_id='{run}';")=='released','new task did not settle central admission')
                assert tenant_sql(item,f"SELECT state FROM runs WHERE id='{item['source_run']}';")=='completed'
            assert len(calls)==2
            passed('upgraded_workers_execute_new_tasks_once_with_central_admission_without_replaying_source_history')
            if config.get('acceptance_dormant_history'):
                passed('old_workspace_files_and_encrypted_attachments_survive_upgrade_and_new_runner_executes_with_central_admission')
            for item in context['instances']:
                lifecycle(item,'delete',confirmation_name=item['name'],purge_backups=True)
            assert sql(db,"SELECT count(*) FROM management_execution_reservations WHERE state='held';")=='0'
            passed('all_cross_version_fixture_instances_and_retained_generations_deleted_with_verified_receipts')
    model.shutdown()
    context['model_requests']+=len(calls);save()
    if not source:
        report={k:context[k] for k in ['checks','operations','model_requests','management_backup_sha256']}
        report.update(instance_ids=[v['id'] for v in context['instances']],source_migration=config.get('acceptance_source_migration',43),target_migration=48,
                      dormant_history='SQL-seeded interrupted/snapshot states, preserved without automatic replay' if config.get('acceptance_dormant_history') else None,
                      workspace_and_encrypted_files=bool(config.get('acceptance_dormant_history')),
                      infrastructure_receipts='real',model_endpoint='controlled private test endpoint',
                      live_modified=False,completed_at=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()))
        Path('/fixture/results.json').write_text(json.dumps(report,indent=2))


if __name__=='__main__': main()
