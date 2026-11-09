"""Actual selected/default provisioning, durable receipts and restart in an isolated fleet."""
import argparse
import json
from pathlib import Path
import secrets
import time
from urllib.request import Request, build_opener, ProxyHandler

import provisioner as p
from operation_journal import OperationJournal
from release_catalog import ReleaseCatalog, image_set_id
from instance_backups import pg_environment
from enterprise_execution_docker_acceptance import uid, totp


def main():
    parser=argparse.ArgumentParser()
    for name in ['config','output','management-container']: parser.add_argument('--'+name,required=True)
    args=parser.parse_args()
    config=json.loads(Path(args.config).read_text())
    assert config['services_network'].startswith('cowork-admission-test-')
    scope=config['services_network'].removesuffix('-services')
    state=Path(config['state_directory']).parent
    assert state==Path('/var/lib/docker/volumes')/(scope+'-state')/'_data'
    labels=json.loads(p.run(['docker','inspect',args.management_container,'--format','{{json .Config.Labels}}']))
    assert labels['com.docker.compose.project']==scope and config['routing_network']==scope+'-routing'
    manager,infra=p.Manager(config),p.Infrastructure(config)
    http=build_opener(ProxyHandler({})); checks=[]; operations=[]; fixtures=[]
    def passed(name): checks.append(name);print(name+'=ok',flush=True)
    def request(path,body=None,token=None):
        req=Request(config['acceptance_management_url']+path,None if body is None else json.dumps(body).encode(),
                    {'Content-Type':'application/json',**({'Authorization':'Bearer '+token} if token else {})})
        with http.open(req,timeout=20) as response: return json.load(response)
    def sql(statement):
        return p.run(['psql','-X','-tA','-v','ON_ERROR_STOP=1'],env=pg_environment(Path('/run/secrets/management_url').read_text().strip()),data=statement.encode()).decode().strip()
    (state/'mc').mkdir(exist_ok=True,mode=0o700)
    p.run(['mc','--config-dir',str(state/'mc'),'alias','set','fixture',config['s3_endpoint'],
           Path('/run/secrets/s3_user').read_text().strip(),Path('/run/secrets/s3_password').read_text().strip()])
    p.run(['age-keygen','--output',config['backup_identity_file']])
    admin=request('/api/v1/auth/bootstrap',dict(email='release@docker.test',display_name='Fixture administrator',password=secrets.token_urlsafe(32),device_id=uid()),Path('/run/secrets/bootstrap').read_text().strip())
    token=admin['access_token'];setup=request('/api/v1/auth/totp/setup',{},token)
    request('/api/v1/auth/totp/enable',{'code':totp(setup['secret'])},token)
    org=request('/management/v1/organisations',{'name':'Initial release Docker acceptance'},token)['id']
    base='/management/v1/organisations/'+org
    selected_images={**config['images'],'server':config['acceptance_target_image']}
    defaults=dict(config['images'])
    catalog=ReleaseCatalog(config,p.private_write)
    with p.operator_lock(infra.root):
        selected=catalog.publish('Initial version fixture',selected_images,[image_set_id(defaults)])
        p.sync_release_catalog(manager,infra)
        assert request(base+'/provisioning-releases',token=token)==[{'id':selected['id'],'name':selected['name']}]
        def current(item): return next(row for row in request(base+'/instances',token=token) if row['id']==item['id'])
        def begin(item,action,**extra):
            op_id=request(base+'/instances/'+item['id']+'/operations',dict(action=action,expected_revision=current(item)['revision'],idempotency_key=uid(),**extra),token)['id']
            operation=manager.call('/claim',p.operator_contract(config));assert operation['id']==op_id
            journal=OperationJournal(infra,operation);journal.begin()
            return operation,journal
        def finish(op,journal,result):
            completion=journal.finish(True,result)
            manager.call('/operations/'+op['id']+'/complete',completion)
            manager.call('/operations/'+op['id']+'/complete',journal.recover())
            operations.append({'id':op['id'],'action':op['action'],'instance_id':op['instance_id']})
        def lifecycle(item,action,**extra):
            op,journal=begin(item,action,**extra);finish(op,journal,infra.execute(op))
        def actual_images(item,expected):
            directory=infra.directory(item['id'])
            assert infra.deployed_images(directory)==expected
            manifest=json.loads((directory/'compose.json').read_text())
            for service in ['api','worker','runner','egress-proxy']:
                container=infra.compose(current(item),directory,'ps','--quiet',service).decode().strip()
                detail=json.loads(p.run(['docker','inspect',container]))[0]
                assert detail['Config']['Image']==manifest['services'][service]['image']
                assert detail['State']['Running']
            return directory
        item=request(base+'/instances',{'name':'Explicit release','backend':'compose','release':selected['id']},token);fixtures.append(item)
        op,journal=begin(item,'provision');receipt=infra.execute(op)
        assert receipt['release']==selected['id'] and receipt['instance_id']==item['id']
        actual_images(item,selected_images)
        # Simulate losing the completion response: new objects recover durable
        # evidence. This is not a process-crash or offline-device acceptance.
        original_ids=infra.compose(op['instance'],infra.directory(item['id']),'ps','--quiet').decode().splitlines()
        restarted=p.Infrastructure(config)
        recovered=OperationJournal(restarted,op).recover()
        assert recovered['result']==receipt
        assert restarted.execute(op)==receipt
        finish(op,journal,receipt)
        assert current(item)['current_release']==selected['id']
        assert infra.compose(current(item),infra.directory(item['id']),'ps','--quiet').decode().splitlines()==original_ids
        passed('selected_six_image_set_runs_in_real_containers_independently_of_defaults')
        passed('lost_completion_reuses_bound_receipt_without_recreating_containers')

        legacy=request(base+'/instances',{'name':'Default release','backend':'compose'},token);fixtures.append(legacy)
        lifecycle(legacy,'provision');actual_images(legacy,defaults)
        assert current(legacy)['provisioning_release'] is None
        passed('legacy_creation_keeps_operator_default_images')
        catalog.withdraw(selected['id']);p.sync_release_catalog(manager,infra)
        assert request(base+'/provisioning-releases',token=token)==[]
        config['images']={key:'registry.invalid/unused@sha256:'+'f'*64 for key in defaults}
        lifecycle(item,'stop');lifecycle(item,'start');actual_images(item,selected_images)
        assert current(item)['provisioning_release']==selected['id']
        assert sql('SELECT count(*) FROM management_release_qualifications;')=='0'
        passed('restart_preserves_installed_selection_after_withdrawal_and_default_changes')
        for fixture in fixtures:
            row=current(fixture)
            lifecycle(fixture,'delete',confirmation_name=row['name'],purge_backups=True)
        assert request(base+'/instances',token=token)==[]
        passed('selected_and_default_instances_deleted_with_verified_real_receipts')
    Path(args.output).write_text(json.dumps(dict(checks=checks,operations=operations,instance_ids=[item['id'] for item in fixtures],
        selected_release=selected['id'],default_release=image_set_id(defaults),infrastructure_receipts='real',
        image_difference='distinct immutable packages containing the same server binary',
        migration=sql('SELECT max(version) FROM _sqlx_migrations;'),live_modified=False,
        completed_at=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())),indent=2))


if __name__=='__main__': main()
