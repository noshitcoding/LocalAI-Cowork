#!/usr/bin/env python3
"""Real management and two instance workers with controlled local model responses.

Infrastructure readiness is seeded; this does not assert VM/Compose lifecycle.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import signal
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import hmac
import json
from enterprise_execution_coverage_acceptance import runtime
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid


def new_id():
    return str(uuid.uuid4())


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def totp(secret):
    key=base64.b32decode(secret + '=' * ((8-len(secret)%8)%8))
    digest=hmac.new(key,(int(time.time())//30).to_bytes(8,'big'),hashlib.sha1).digest()
    index=digest[-1]&15
    return f"{(int.from_bytes(digest[index:index+4],'big')&0x7fffffff)%1000000:06d}"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--server', required=True)
    parser.add_argument('--postgres-bin', default='/usr/lib/postgresql/16/bin')
    args = parser.parse_args()
    binary, pg = str(Path(args.server).resolve()), Path(args.postgres_bin)
    with tempfile.TemporaryDirectory(prefix='cowork-content-test-') as temp:
        root, db_port = Path(temp), port()
        data = root / 'pg'
        subprocess.run([str(pg/'initdb'), '-D', str(data), '-A', 'trust', '-U', 'postgres', '--no-sync'], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([str(pg/'pg_ctl'), '-D', str(data), '-l', str(root/'postgres.log'), '-o', f'-h 127.0.0.1 -p {db_port} -k {temp}', 'start', '-w'], check=True, stdout=subprocess.DEVNULL)
        processes = []
        starts = {}
        def sql(database, statement):
            result = subprocess.run([str(pg/'psql'), '-h', '127.0.0.1', '-p', str(db_port), '-U', 'postgres', '-d', database, '-At', '-v', 'ON_ERROR_STOP=1'], input=statement, text=True, capture_output=True)
            if result.returncode:
                raise AssertionError(result.stderr[-2000:])
            return result.stdout.strip()
        def request(origin, path, token=None, body=None, method=None, expected=200, headers=None):
            headers = {'Content-Type':'application/json', **(headers or {})}
            if token:
                headers['Authorization'] = 'Bearer ' + token
            req = Request(origin+path, None if body is None else json.dumps(body).encode(), headers, method=method or ('GET' if body is None else 'POST'))
            try:
                with urlopen(req, timeout=15) as response:
                    status, content = response.status, response.read()
            except HTTPError as error:
                status, content = error.code, error.read()
            assert status == expected, f'{req.get_method()} {path}: {status}, expected {expected}'
            return json.loads(content) if content and status < 400 else None
        def start(database, extra):
            sql('postgres', f'CREATE DATABASE {database}')
            api_port = port()
            env = {k:v for k,v in os.environ.items() if not k.startswith('COWORK_') and k != 'DATABASE_URL'}
            env.update({'DATABASE_URL':f'postgres://postgres@127.0.0.1:{db_port}/{database}', 'COWORK_LISTEN_ADDR':f'127.0.0.1:{api_port}', 'COWORK_BOOTSTRAP_TOKEN':secrets.token_hex(32), 'COWORK_WEB_PUSH_ENABLED':'false', **extra})
            logpath = root / (database+'.log')
            process = subprocess.Popen([binary], env=env, stdout=logpath.open('wb'), stderr=subprocess.STDOUT)
            processes.append(process)
            starts[database] = (env, logpath, process)
            origin = f'http://127.0.0.1:{api_port}'
            for _ in range(150):
                if process.poll() is not None:
                    raise AssertionError(logpath.read_text()[-4000:])
                try:
                    request(origin, '/readyz')
                    return origin, process
                except OSError:
                    time.sleep(.1)
            raise AssertionError('readiness timeout')
        gate = threading.Semaphore(0)
        active = 0
        peak = 0
        calls = 0
        counter_lock = threading.Lock()
        class Model(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_POST(self):
                nonlocal active, peak, calls
                if self.path != '/v1/chat/completions':
                    self.send_error(404); return
                self.rfile.read(int(self.headers['Content-Length']))
                with counter_lock:
                    calls += 1; active += 1; peak = max(peak, active)
                try:
                    if not gate.acquire(timeout=120): raise AssertionError('controlled model timeout')
                    body = json.dumps({'choices':[{'message':{'role':'assistant','content':'Capacity fixture finished.'},'finish_reason':'stop'}], 'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}).encode()
                    self.send_response(200); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError): pass
                finally:
                    with counter_lock: active -= 1
        model = ThreadingHTTPServer(('127.0.0.1',0), Model)
        threading.Thread(target=model.serve_forever,daemon=True).start()
        def wait_until(predicate, message, seconds=20):
            deadline=time.monotonic()+seconds
            while time.monotonic()<deadline:
                if predicate(): return
                time.sleep(.1)
            raise AssertionError(message)
        def restart(database):
            env, logpath, old = starts[database]
            old.terminate(); old.wait(timeout=10)
            process=subprocess.Popen([binary],env=env,stdout=logpath.open('ab'),stderr=subprocess.STDOUT)
            processes.append(process); starts[database]=(env,logpath,process)
            origin='http://'+env['COWORK_LISTEN_ADDR']
            def ready():
                if process.poll() is not None: raise AssertionError(logpath.read_text()[-4000:])
                try: request(origin,'/readyz'); return True
                except OSError: return False
            wait_until(ready,'restarted process readiness')
            return process
        try:
            broker, bootstrap = secrets.token_hex(32), secrets.token_hex(32)
            (root/'broker').write_text(broker)
            (root/'audit').write_bytes(secrets.token_bytes(32))
            (root/'execution-ceiling').write_text(json.dumps({'platform_concurrent_runs':2,'organisation_concurrent_runs':2,'user_concurrent_runs':1}))
            management, manager = start('management', {'COWORK_MODE':'management','COWORK_AUTH_MASTER_KEY':base64.b64encode(secrets.token_bytes(32)).decode(),'COWORK_BOOTSTRAP_TOKEN':bootstrap,'COWORK_MANAGEMENT_AUDIT_KEY_FILE':str(root/'audit'),'COWORK_PROVISIONER_TOKEN_FILE':str(root/'broker'),'COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED':'true','COWORK_MANAGEMENT_EXECUTION_CEILING_FILE':str(root/'execution-ceiling'),'COWORK_MANAGEMENT_BACKUPS_ENABLED':'true','COWORK_INSTANCE_URL_TEMPLATE':'http://{instance}.localhost:18084'})
            admin=request(management,'/api/v1/auth/bootstrap',bootstrap,{'email':'operator@example.test','display_name':'Operator','password':'Capacity-Testing-42-Long!','device_id':new_id()},expected=201)
            setup=request(management,'/api/v1/auth/totp/setup',admin['access_token'],{})
            request(management,'/api/v1/auth/totp/enable',admin['access_token'],{'code':totp(setup['secret'])})
            invitation=request(management,'/api/v1/auth/invitations',admin['access_token'],{'email':'owner@example.test'},expected=201)
            owner=request(management,'/api/v1/auth/invitations/accept',body={'token':invitation['token'],'display_name':'Owner','password':'Capacity-Testing-42-Long!','device_id':new_id()},expected=201)
            org=request(management,'/management/v1/organisations',admin['access_token'],{'name':'Isolated execution acceptance'})['id']
            base=f'/management/v1/organisations/{org}'
            request(management,base+'/members',admin['access_token'],{'email':'owner@example.test','role':'member'})
            instances=[]; tenants=[]; sessions=[]; projects=[]; workers=[]
            for n in range(2):
                instance=request(management,base+'/instances',owner['access_token'],{'name':f'Tenant {n}','backend':'compose'})['id']
                sql('management',f"UPDATE management_instances SET state='ready',execution_evidence=jsonb_build_object('active_epoch',execution_epoch,'runtime','{json.dumps(runtime())}'::jsonb) WHERE id='{instance}';")
                sql('management',f"UPDATE management_operations SET state='succeeded' WHERE instance_id='{instance}';")
                epoch=sql('management',f"SELECT execution_epoch FROM management_instances WHERE id='{instance}';")
                (root/f'key{n}').write_text(hmac.new(broker.encode(),f'cowork-federation-v1\0{org}\0{instance}'.encode(),hashlib.sha256).hexdigest())
                origin, worker = start('tenant'+str(n),{'COWORK_MODE':'all','COWORK_FLEET_KEY_FILE':str(root/f'key{n}'),'COWORK_ORGANISATION_ID':org,'COWORK_INSTANCE_ID':instance,'COWORK_FLEET_MANAGEMENT_URL':management,'COWORK_FLEET_EXECUTION_ADMISSION':'true','COWORK_EXECUTION_EPOCH':epoch,'COWORK_MODEL_BASE_URL':f'http://127.0.0.1:{model.server_port}/v1','COWORK_MODEL_NAME':'controlled-capacity-fixture','COWORK_SERVER_CAPABILITIES':'model.external','COWORK_WORKER_POLL_MS':'100'})
                instances.append(instance); tenants.append(origin); workers.append(worker)
                ticket=request(management,base+f'/instances/{instance}/launch',owner['access_token'],{})
                session=request(origin,'/api/v1/auth/fleet',body={'ticket':ticket['ticket'],'device_id':new_id()})['access_token']
                sessions.append(session)
                team=request(origin,'/api/v1/teams',session,{'name':'Capacity fixture'},expected=201)
                projects.append(request(origin,'/api/v1/projects',session,{'name':'Capacity fixture','privacy':'team_managed','team_id':team['id']},expected=201))
            def create(n, target=None):
                project=projects[n]
                thread=request(tenants[n],'/api/v1/threads',sessions[n],{'project_id':project['id'],'title':'Capacity acceptance'},expected=201)
                run=request(tenants[n],'/api/v1/runs',sessions[n],{'project_id':project['id'],'thread_id':thread['id'],'project_revision':project['revision'],'project_privacy':'team_managed','executor_target':target or {'kind':'server_linux','pool_id':None},'required_capabilities':[] if target else ['model.external'],'input':{'prompt':'Reply with the completion marker.'},'idempotency_key':new_id()},expected=201)
                return run['spec']['id']
            held=lambda:int(sql('management',"SELECT count(*) FROM management_execution_reservations WHERE state='held';"))
            state=lambda n,run:sql('tenant'+str(n),f"SELECT state FROM runs WHERE id='{run}';")
            first=create(0)
            wait_until(lambda:active==1,'first actual model execution did not start')
            second=create(1)
            time.sleep(1)
            assert calls==1 and held()==1 and state(1,second)=='queued'
            manager=restart('management')
            assert held()==1 and state(1,second)=='queued' and calls==1
            print('real_workers_two_databases_share_one_user_slot_and_survive_management_restart=ok')
            # Finish while management is unreachable. Local terminal evidence is
            # durable but cannot manufacture a central release during the outage.
            manager.send_signal(signal.SIGSTOP)
            gate.release()
            wait_until(lambda:state(0,first)=='completed','first worker did not finish during management outage')
            assert held()==1 and state(1,second)=='queued' and calls==1
            assert sql('tenant0',"SELECT count(*) FROM instance_execution_reservations WHERE completion IS NOT NULL AND acknowledged_at IS NULL;")=='1'
            workers[0]=restart('tenant0')
            assert held()==1 and calls==1
            manager.send_signal(signal.SIGCONT)
            wait_until(lambda:calls==2 and active==1,'queued work did not start after durable receipt delivery')
            assert state(1,second)=='running' and held()==1
            assert sql('tenant0',"SELECT count(*) FROM instance_execution_reservations WHERE acknowledged_at IS NOT NULL;")=='1'
            gate.release()
            wait_until(lambda:state(1,second)=='completed' and held()==0,'second completion did not release its slot')
            assert peak==1 and calls==2
            print('model_only_slots_completion_outbox_management_outage_and_instance_restart=ok')
            devices=[]; credentials=[]; local_runs=[]
            for n in range(2):
                device=new_id()
                request(tenants[n],'/api/v1/executors',sessions[n],{'schema_version':2,'protocol_version':2,'executor_id':device,'kind':'personal_device','pool_id':None,'owner_user_id':owner['user_id'],'display_name':'Simulated executor protocol peer','capabilities':[],'labels':{},'max_concurrent_runs':1},expected=201)
                credential=request(tenants[n],f'/api/v1/executors/{device}/credentials',sessions[n],{'label':'Capacity acceptance'},expected=201)['token']
                devices.append(device); credentials.append(credential)
                local_runs.append(create(n,{'kind':'personal_device','device_id':device}))
            agent=lambda n,route,body=None,expected=200:request(tenants[n],f'/api/v1/agent/executors/{devices[n]}'+route,credentials[n],body,expected=expected)
            first_lease=agent(0,'/claim',{})
            assert first_lease and first_lease['run']['spec']['id']==local_runs[0]
            assert agent(1,'/claim',{}) is None and held()==1
            agent(0,f'/runs/{local_runs[0]}/complete',{'lease_token':first_lease['lease_token'],'result':{'text':'Fixture finished'}})
            second_lease=[]
            def claim_second():
                value=agent(1,'/claim',{})
                if value: second_lease.append(value); return True
                return False
            wait_until(claim_second,'external completion did not unblock the other instance')
            agent(1,f'/runs/{local_runs[1]}/fail',{'lease_token':second_lease[0]['lease_token'],'error':{'code':'fixture_finished_with_error','message':'The simulated executor confirms completion.','retryable':False,'details':{}}})
            wait_until(lambda:held()==0,'external completion receipts were not relayed')
            assert calls==2
            print('external_executor_claims_share_capacity_and_authenticated_terminal_receipts_release_it=ok (simulated protocol peers)')
            # A killed worker has no completion proof. Its slot is not reclaimed
            # from heartbeat age, and the other instance cannot execute twice.
            third=create(0)
            wait_until(lambda:calls==3 and active==1,'third model did not start')
            fourth=create(1)
            workers[0].kill(); workers[0].wait(timeout=10)
            gate.release()
            wait_until(lambda:active==0,'disconnected controlled model did not exit')
            sql('tenant0',f"UPDATE runs SET lease_expires_at=now()-interval '1 hour' WHERE id='{third}';")
            workers[0]=restart('tenant0')
            wait_until(lambda:state(0,third)=='interrupted','worker lease was not reconciled')
            time.sleep(1)
            assert held()==1 and calls==3 and state(1,fourth)=='queued'
            assert sql('tenant0',f"SELECT completion IS NULL FROM instance_execution_reservations WHERE run_id='{third}';")=='t'
            print('killed_worker_and_expired_lease_retain_capacity_without_reassignment=ok')
            # A database failure during outbox persistence must also roll back
            # the terminal run update. Sequence increments survive rollback and
            # provide deterministic evidence that the injected failure happened.
            sql('tenant1', """
                CREATE SEQUENCE admission_receipt_failures;
                CREATE FUNCTION reject_admission_receipt() RETURNS trigger LANGUAGE plpgsql AS $$
                BEGIN
                    PERFORM nextval('admission_receipt_failures');
                    RAISE EXCEPTION 'injected admission receipt failure';
                END $$;
                CREATE TRIGGER reject_admission_receipt BEFORE UPDATE ON instance_execution_reservations
                    FOR EACH ROW WHEN (NEW.completion IS NOT NULL)
                    EXECUTE FUNCTION reject_admission_receipt();
            """)
            (root/'execution-ceiling').write_text(json.dumps({'revision':2,'platform_concurrent_runs':2,'organisation_concurrent_runs':2,'user_concurrent_runs':2}))
            manager=restart('management')
            wait_until(lambda:calls==4 and active==1,'increased operator ceiling did not admit queued work')
            assert held()==2 and state(0,third)=='interrupted'
            gate.release()
            wait_until(lambda:sql('tenant1','SELECT is_called FROM admission_receipt_failures;')=='t','outbox write was not attempted')
            wait_until(lambda:active==0,'controlled model did not finish')
            assert state(1,fourth)=='running' and held()==2
            assert sql('tenant1',f"SELECT completion IS NULL FROM instance_execution_reservations WHERE run_id='{fourth}';")=='t'
            assert sql('tenant1',f"SELECT count(*) FROM messages WHERE run_id='{fourth}' AND role='assistant';")=='0'
            print('completion_receipt_failure_rolls_back_terminal_state_and_assistant_message=ok')
            # Promote through the real lifecycle API, with simulated infrastructure
            # receipts. Leave the old actual worker alive: even after management
            # says ready, that process must not claim with its stale generation.
            (root/'execution-ceiling').write_text(json.dumps({'revision':3,'platform_concurrent_runs':3,'organisation_concurrent_runs':3,'user_concurrent_runs':3}))
            manager=restart('management')
            def lifecycle(action, **arguments):
                current=next(value for value in request(management,base+'/instances',owner['access_token']) if value['id']==instances[0])
                operation=request(management,base+f'/instances/{instances[0]}/operations',owner['access_token'],{'action':action,'expected_revision':current['revision'],'idempotency_key':new_id(),**arguments})
                control={'x-cowork-provisioner-token':broker}
                dispatched=request(management,'/management/runner/v1/claim',body={'backend':'compose','execution_admission_version':1},headers=control)
                assert dispatched['id']==operation['id']
                binding=dispatched['execution_binding']
                active_epoch=binding['candidate_epoch'] if action=='restore' else binding['source_epoch']
                result={'receipt_id':operation['id'],'execution':{'binding':binding,'active_epoch':active_epoch,'runtime':runtime()}}
                if action=='backup': result.update({'backup_id':operation['id'],'backup_sha256':'c'*64,'backup_bytes':1234,'manifest_version':1})
                if action=='restore': result['backup_id']=arguments['backup_id']
                request(management,f"/management/runner/v1/operations/{operation['id']}/complete",body={'lease_token':dispatched['lease_token'],'succeeded':True,'result':result},headers=control)
                return operation['id'],active_epoch
            lifecycle('stop')
            backup,_=lifecycle('backup')
            _,promoted_epoch=lifecycle('restore',backup_id=backup)
            lifecycle('start')
            fenced=create(0)
            time.sleep(1)
            assert state(0,fenced)=='queued' and calls==4 and held()==2
            assert sql('tenant0',f"SELECT count(*) FROM instance_execution_reservations WHERE run_id='{fenced}';")=='0'
            starts['tenant0'][0]['COWORK_EXECUTION_EPOCH']=promoted_epoch
            workers[0]=restart('tenant0')
            wait_until(lambda:calls==5 and active==1,'replacement worker did not claim with promoted epoch')
            gate.release()
            wait_until(lambda:state(0,fenced)=='completed' and held()==2,'promoted worker completion did not settle')
            print('real_old_worker_is_fenced_after_lifecycle_promotion_and_reconfigured_worker_executes_once=ok (simulated infrastructure receipts)')
            # Opt-out remains the default for an independent standalone server.
            # It must finish a real worker run without any management callback.
            standalone_bootstrap=secrets.token_hex(32)
            origin, _ = start('standalone', {'COWORK_MODE':'all','COWORK_BOOTSTRAP_TOKEN':standalone_bootstrap,'COWORK_MODEL_BASE_URL':f'http://127.0.0.1:{model.server_port}/v1','COWORK_MODEL_NAME':'controlled-capacity-fixture','COWORK_SERVER_CAPABILITIES':'model.external','COWORK_WORKER_POLL_MS':'100'})
            standalone=request(origin,'/api/v1/auth/bootstrap',standalone_bootstrap,{'email':'standalone@example.test','display_name':'Standalone','password':'Capacity-Testing-42-Long!','device_id':new_id()},expected=201)
            session=standalone['access_token']
            team=request(origin,'/api/v1/teams',session,{'name':'Standalone fixture'},expected=201)
            projects.append(request(origin,'/api/v1/projects',session,{'name':'Standalone fixture','privacy':'team_managed','team_id':team['id']},expected=201))
            tenants.append(origin); sessions.append(session)
            manager.send_signal(signal.SIGSTOP)
            standalone_run=create(2)
            wait_until(lambda:calls==6 and active==1,'standalone worker did not start without management')
            gate.release()
            wait_until(lambda:sql('standalone',f"SELECT state FROM runs WHERE id='{standalone_run}';")=='completed','standalone worker did not complete')
            assert held()==2 and sql('standalone','SELECT count(*) FROM instance_execution_reservations;')=='0'
            manager.send_signal(signal.SIGCONT)
            print('standalone_worker_without_admission_remains_independent_of_management=ok')
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.send_signal(signal.SIGCONT)
                    process.terminate()
                    try: process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
            for _ in range(10): gate.release()
            model.shutdown(); model.server_close()
            subprocess.run([str(pg/'pg_ctl'),'-D',str(data),'stop','-m','immediate','-w'],stdout=subprocess.DEVNULL,check=True)


if __name__ == '__main__':
    main()
