#!/usr/bin/env python3
"""Real management + two isolated tenant APIs on disposable PostgreSQL.

Infrastructure readiness is seeded; this does not assert VM/Compose lifecycle.
"""
import argparse
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import hmac
import json
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
        try:
            broker, bootstrap = secrets.token_hex(32), secrets.token_hex(32)
            (root/'broker').write_text(broker)
            (root/'audit').write_bytes(secrets.token_bytes(32))
            (root/'ceiling').write_text(json.dumps({'content_access':'organisation_admin'}))
            management, manager = start('management', {'COWORK_MODE':'management','COWORK_AUTH_MASTER_KEY':base64.b64encode(secrets.token_bytes(32)).decode(),'COWORK_BOOTSTRAP_TOKEN':bootstrap,'COWORK_MANAGEMENT_AUDIT_KEY_FILE':str(root/'audit'),'COWORK_PROVISIONER_TOKEN_FILE':str(root/'broker'),'COWORK_MANAGEMENT_POLICY_CEILING_FILE':str(root/'ceiling'),'COWORK_INSTANCE_URL_TEMPLATE':'http://{instance}.localhost:18084'})
            admin = request(management,'/api/v1/auth/bootstrap',bootstrap,{'email':'admin@example.test','display_name':'Operator','password':'Testing-Content-Access-42!','device_id':new_id()},expected=201)
            setup=request(management,'/api/v1/auth/totp/setup',admin['access_token'],{})
            request(management,'/api/v1/auth/totp/enable',admin['access_token'],{'code':totp(setup['secret'])})
            def account(email):
                invite = request(management,'/api/v1/auth/invitations',admin['access_token'],{'email':email},expected=201)
                return request(management,'/api/v1/auth/invitations/accept',body={'token':invite['token'],'display_name':email,'password':'Testing-Content-Access-42!','device_id':new_id()},expected=201)
            owner, support, outsider = [account(email) for email in ('owner@example.test','support@example.test','outsider@example.test')]
            org = request(management,'/management/v1/organisations',admin['access_token'],{'name':'Content A'})['id']
            other = request(management,'/management/v1/organisations',admin['access_token'],{'name':'Content B'})['id']
            base = f'/management/v1/organisations/{org}'
            def member(person, role, content=False):
                request(management,base+'/members',admin['access_token'],{'email':person,'role':role,'content_admin':content})
            member('owner@example.test','member')
            member('support@example.test','admin')
            request(management,f'/management/v1/organisations/{other}/members',admin['access_token'],{'email':'outsider@example.test','role':'admin','content_admin':True})
            policy = next(o['policy'] for o in request(management,'/management/v1/organisations',admin['access_token'])['items'] if o['id']==org)
            policy.update(revision=2,content_access='organisation_admin')
            request(management,base+'/policy',admin['access_token'],policy,method='PUT')
            strict_policy = {**policy,'content_access':'support_grant'}
            instances = [request(management,base+'/instances',owner['access_token'],{'name':name,'backend':'compose','policy':chosen})['id'] for name,chosen in [('Approval',strict_policy),('Explicit readers',policy)]]
            tenants = []
            for index, instance in enumerate(instances):
                sql('management',f"UPDATE management_instances SET state='ready' WHERE id='{instance}'")
                key=hmac.new(broker.encode(),f'cowork-federation-v1\0{org}\0{instance}'.encode(),hashlib.sha256).hexdigest()
                (root/f'key{index}').write_text(key)
                origin, _ = start('tenant'+str(index),{'COWORK_MODE':'api','COWORK_FLEET_KEY_FILE':str(root/f'key{index}'),'COWORK_ORGANISATION_ID':org,'COWORK_INSTANCE_ID':instance,'COWORK_FLEET_MANAGEMENT_URL':management})
                tenants.append(origin)
            def launch(index, user):
                ticket=request(management,base+f'/instances/{instances[index]}/launch',user['access_token'],{})
                return request(tenants[index],'/api/v1/auth/fleet',body={'ticket':ticket['ticket'],'device_id':new_id()})
            for _ in range(100):
                rows=request(management,base+'/instances',support['access_token'])
                if all(row['can_open_support'] for row in rows): break
                time.sleep(.1)
            else: raise AssertionError('instance capability missing')
            scope={'organisation_id':org,'instance_id':instances[0],'user_id':support['user_id']}
            request(management,'/management/runner/v1/content-access',body={'scope':scope,'audit':None},headers={'x-cowork-instance-key':(root/'key1').read_text()},expected=401)
            legacy=request(management,'/management/runner/v1/instance-access',body=scope,headers={'x-cowork-instance-key':(root/'key0').read_text()})
            assert legacy=={'allowed':False}, 'legacy APIs must not receive administrative authority'
            request(management,'/management/runner/v1/content-access',body={'scope':scope,'audit':{'target_type':'project','target_id':new_id(),'grant_id':None}},headers={'x-cowork-instance-key':(root/'key0').read_text()},expected=404)
            own = launch(0,owner)['access_token']
            # The grantee need not have signed into the instance before approval.
            projects=[request(tenants[0],'/api/v1/projects',own,{'name':name,'description':'Project information outside the approved chat','policy':{'private_note':'Not shared by a chat grant'},'privacy':'private_local'},expected=201) for name in ('Approved project','Unshared project')]
            threads=[request(tenants[0],'/api/v1/threads',own,{'project_id':projects[0]['id'],'title':name},expected=201) for name in ('Approved chat','Other chat')]
            def grant(project=None, thread=None, recipient=support['user_id'], expected=201):
                return request(tenants[0],'/api/v1/support-grants',own,{'support_user_id':recipient,'project_id':project,'thread_id':thread,'reason':'Investigate selected data','expires_at':(datetime.now(timezone.utc)+timedelta(hours=1)).isoformat()},expected=expected)
            grant(project=projects[0]['id'],recipient=outsider['user_id'],expected=422)
            g=grant(thread=threads[0]['id'])
            reader=launch(0,support)['access_token']
            info=request(tenants[0],'/api/v1/content-access',reader)
            assert info['read_only'] and info['support_administrator'] and not info['automatic_content_access']
            request(tenants[0],f"/api/v1/threads/{threads[0]['id']}/messages",reader)
            request(tenants[0],f"/api/v1/threads/{threads[1]['id']}/messages",reader,expected=401)
            request(tenants[0],f"/api/v1/projects/{projects[0]['id']}",reader,expected=401)
            request(tenants[0],f"/api/v1/projects/{projects[1]['id']}",reader,expected=401)
            assert [t['id'] for t in request(tenants[0],f"/api/v1/projects/{projects[0]['id']}/threads",reader)]==[threads[0]['id']]
            assert len(request(tenants[0],'/api/v1/support-grants',reader))==1
            navigation=request(tenants[0],'/api/v1/projects',reader)
            assert len(navigation)==1 and navigation[0]['description']=='' and navigation[0]['policy']=={}
            request(tenants[0],'/api/v1/projects',reader,{'name':'Forbidden','privacy':'private_local'},expected=403)
            request(tenants[0],f"/api/v1/threads/{threads[0]['id']}",reader,{'title':'Forbidden','expected_revision':1},method='PUT',expected=403)
            request(tenants[0],'/api/v1/agent/pairing',reader,{},expected=404)
            print('scoped-chat-approval-cross-org-denial-and-read-only=ok')
            request(tenants[0],f"/api/v1/support-grants/{g['id']}",reader,method='DELETE',expected=204)
            request(tenants[0],f"/api/v1/threads/{threads[0]['id']}/messages",reader,expected=401)
            g=grant(project=projects[0]['id'])
            assert request(tenants[0],f"/api/v1/projects/{projects[0]['id']}",reader)['id']==projects[0]['id']
            assert request(tenants[0],'/api/v1/projects',reader)[0]['description']=='Project information outside the approved chat'
            assert len(request(tenants[0],f"/api/v1/projects/{projects[0]['id']}/threads",reader))==2
            # A real SSE connection must close after approval expiry. Events after
            # expiry must not be delivered through the existing connection.
            run_id=new_id()
            spec={'schema_version':2,'id':run_id,'thread_id':threads[0]['id'],'project_id':projects[0]['id'],'project':{'id':projects[0]['id'],'revision':1},'project_privacy':'private_local','task':None,'creator_user_id':owner['user_id'],'executor_target':{'kind':'server_linux'},'required_capabilities':[],'input':{},'model_profile_id':None,'snapshot_id':None,'idempotency_key':run_id,'created_at':datetime.now(timezone.utc).isoformat()}
            sql('tenant0',f"INSERT INTO runs(id,thread_id,project_id,creator_user_id,idempotency_key,target_kind,state,spec,created_at,updated_at) VALUES('{run_id}','{threads[0]['id']}','{projects[0]['id']}','{owner['user_id']}','{run_id}','server_linux','waiting_for_executor','{json.dumps(spec)}',now(),now()); INSERT INTO run_events VALUES('{run_id}',1,'{new_id()}','created','{{}}',now());")
            stream=urlopen(Request(tenants[0]+f'/api/v1/runs/{run_id}/events',headers={'Authorization':'Bearer '+reader}),timeout=8)
            while stream.readline().strip(): pass
            sql('tenant0',f"UPDATE support_grants SET expires_at=now()-interval '1 second' WHERE id='{g['id']}'")
            assert stream.read()==b'', 'expired grant kept a stream open'
            stream.close()
            request(tenants[0],f'/api/v1/runs/{run_id}',reader,expected=401)
            print('project-approval-expiry-and-open-stream-revocation=ok')
            g=grant(project=projects[0]['id'])
            # Losing instance membership must also remove local owner/editor
            # authority when the same identity remains an organisation admin.
            request(management,base+f'/instances/{instances[0]}/members',admin['access_token'],{'user_id':support['user_id'],'role':'member'})
            former=request(tenants[0],'/api/v1/projects',reader,{'name':'Former member project','privacy':'private_local'},expected=201)
            request(management,base+f"/instances/{instances[0]}/members/{support['user_id']}",admin['access_token'],method='DELETE')
            request(tenants[0],f"/api/v1/projects/{former['id']}",reader,expected=401)
            assert former['id'] not in [p['id'] for p in request(tenants[0],'/api/v1/projects',reader)]
            member('support@example.test','member')
            request(tenants[0],f"/api/v1/projects/{projects[0]['id']}",reader,expected=401)
            member('support@example.test','admin',True)
            # Explicit flag cannot override an instance's stricter policy.
            assert not request(tenants[0],'/api/v1/content-access',reader)['automatic_content_access']
            request(tenants[0],f"/api/v1/projects/{projects[1]['id']}",reader,expected=401)
            own2=launch(1,owner)['access_token']
            project2=request(tenants[1],'/api/v1/projects',own2,{'name':'Separate instance data','privacy':'private_local'},expected=201)
            reader2=launch(1,support)['access_token']
            assert request(tenants[1],'/api/v1/content-access',reader2)['automatic_content_access']
            assert request(tenants[1],f"/api/v1/projects/{project2['id']}",reader2)['id']==project2['id']
            request(tenants[1],f"/api/v1/projects/{projects[0]['id']}",reader2,expected=404)
            request(tenants[0],f"/api/v1/projects/{project2['id']}",reader,expected=404)
            member('support@example.test','admin',False)
            request(tenants[1],f"/api/v1/projects/{project2['id']}",reader2,expected=401)
            print('explicit-content-permission-policy-intersection-and-two-instance-isolation=ok')
            # Instance membership is a ceiling, not a grant to every project.
            viewer=account('viewer@example.test')
            member('viewer@example.test','member')
            membership_path=base+f'/instances/{instances[0]}/members'
            candidates=request(management,membership_path+'/candidates',owner['access_token'])
            assert viewer['user_id'] in [p['user_id'] for p in candidates]
            assert owner['user_id'] not in [p['user_id'] for p in candidates]
            assert outsider['user_id'] not in [p['user_id'] for p in candidates]
            request(management,membership_path,owner['access_token'],{'user_id':outsider['user_id'],'role':'member'},expected=422)
            request(management,membership_path,owner['access_token'],{'user_id':owner['user_id'],'role':'viewer'},expected=422)
            request(management,membership_path+'/'+owner['user_id'],owner['access_token'],method='DELETE',expected=422)
            request(management,membership_path,owner['access_token'],{'user_id':viewer['user_id'],'role':'member'})
            viewing=launch(0,viewer)['access_token']
            own_viewer_project=request(tenants[0],'/api/v1/projects',viewing,{'name':'Viewer owned before demotion','privacy':'private_local'},expected=201)
            device=new_id()
            request(tenants[0],'/api/v1/executors',viewing,{'schema_version':2,'executor_id':device,'kind':'personal_device','pool_id':None,'owner_user_id':viewer['user_id'],'display_name':'Viewer laptop','protocol_version':2,'capabilities':[],'labels':{},'max_concurrent_runs':1},expected=201)
            credential=request(tenants[0],f'/api/v1/executors/{device}/credentials',viewing,{'label':'Viewer test'},expected=201)['token']
            heartbeat={'protocol_version':2,'active_run_ids':[],'health':{}}
            request(tenants[0],f'/api/v1/agent/executors/{device}/heartbeat',credential,heartbeat)
            address=urlsplit(tenants[0])
            def websocket(path, token=None, expected=101):
                channel=socket.create_connection((address.hostname,address.port),timeout=8)
                key=base64.b64encode(secrets.token_bytes(16)).decode()
                auth=f'Authorization: Bearer {token}\r\n' if token else ''
                channel.sendall((f'GET {path} HTTP/1.1\r\nHost: {address.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n{auth}\r\n').encode())
                stream=channel.makefile('rb')
                assert int(stream.readline().split()[1])==expected
                while stream.readline().strip(): pass
                if expected!=101: stream.close(); channel.close(); return None,None
                return channel,stream
            agent_socket,agent_stream=websocket(f'/api/v1/agent/executors/{device}/connect',credential)
            def frame(stream=agent_stream):
                head=stream.read(2)
                if not head: return None
                assert len(head)==2 and not head[1]&128
                size=head[1]&127
                if size==126: size=int.from_bytes(stream.read(2),'big')
                elif size==127: size=int.from_bytes(stream.read(8),'big')
                data=stream.read(size)
                return head[0]&15,data
            hello=frame()
            assert hello and hello[0]==1 and json.loads(hello[1])['type']=='hello'
            # Seed only desktop metadata: real browser/agent WebSockets and
            # reverse relay carry bytes. No VM or graphical workload is claimed.
            desktop_run,desktop_id=new_id(),new_id()
            desktop_thread=request(tenants[0],'/api/v1/threads',viewing,{'project_id':own_viewer_project['id'],'title':'Desktop permission test'},expected=201)['id']
            desktop_spec={**spec,'id':desktop_run,'thread_id':desktop_thread,'project_id':own_viewer_project['id'],'project':{'id':own_viewer_project['id'],'revision':1},'creator_user_id':viewer['user_id'],'executor_target':{'kind':'personal_device','device_id':device},'idempotency_key':desktop_run}
            sql('tenant0',f"INSERT INTO runs(id,thread_id,project_id,creator_user_id,idempotency_key,target_kind,state,spec,created_at,updated_at,assigned_executor_id) VALUES('{desktop_run}','{desktop_thread}','{own_viewer_project['id']}','{viewer['user_id']}','{desktop_run}','personal_device','completed','{json.dumps(desktop_spec)}',now(),now(),'{device}'); INSERT INTO desktop_sessions(id,run_id,executor_id,state,stream_protocol,runner_metadata) VALUES('{desktop_id}','{desktop_run}','{device}','agent_controlled','rfb.binary.v1','{{\"transport\":\"executor_reverse_ws\"}}');")
            desktop_ticket=request(tenants[0],f'/api/v1/runs/{desktop_run}/desktop-sessions/{desktop_id}/tickets',viewing,{'control':False})['token']
            unused_ticket=request(tenants[0],f'/api/v1/runs/{desktop_run}/desktop-sessions/{desktop_id}/tickets',viewing,{'control':False})['token']
            browser_socket,browser_stream=websocket(f'/api/v1/desktop-sessions/{desktop_id}/stream?ticket={desktop_ticket}')
            command=json.loads(frame()[1]); assert command['type']=='desktop_stream_requested'
            reverse_socket,reverse_stream=websocket(f"/api/v1/agent/executors/{device}/desktop-streams/{command['stream_id']}",credential)
            relay_payload=b'test'; mask=secrets.token_bytes(4)
            reverse_socket.sendall(bytes([0x82,0x80|len(relay_payload)])+mask+bytes(value^mask[index%4] for index,value in enumerate(relay_payload)))
            assert frame(browser_stream)==(2,relay_payload), 'desktop relay was not active before revocation'
            request(management,membership_path,owner['access_token'],{'user_id':viewer['user_id'],'role':'viewer'})
            request(tenants[0],f'/api/v1/agent/executors/{device}/heartbeat',credential,heartbeat,expected=401)
            # The existing socket closes too; a reconnect is not required.
            ended=False
            for _ in range(8):
                item=frame()
                if item is None or item[0]==8: ended=True; break
            assert ended, 'demoted member retained an executor WebSocket'
            agent_stream.close(); agent_socket.close()
            closed=frame(browser_stream)
            assert closed is None or closed[0]==8, 'demoted member retained a desktop stream'
            browser_stream.close(); browser_socket.close(); reverse_stream.close(); reverse_socket.close()
            websocket(f'/api/v1/desktop-sessions/{desktop_id}/stream?ticket={unused_ticket}',expected=401)
            print('active-reverse-desktop-relay-and-unused-ticket-revocation=ok')
            viewer_scope={**scope,'user_id':viewer['user_id']}
            assert request(management,'/management/runner/v1/instance-access',body=viewer_scope,headers={'x-cowork-instance-key':(root/'key0').read_text()})=={'allowed':False}
            # The older ticket consumer must never receive a normal login ticket
            # for a viewer; only an instance advertising v2 can offer this role.
            sql('management',f"UPDATE management_instance_features SET content_access_version=1 WHERE instance_id='{instances[0]}'")
            request(management,base+f'/instances/{instances[0]}/launch',viewer['access_token'],{},expected=404)
            request(management,'/management/runner/v1/content-capability',body={'organisation_id':org,'instance_id':instances[0],'version':2},headers={'x-cowork-instance-key':(root/'key0').read_text()})
            ticket=request(management,base+f'/instances/{instances[0]}/launch',viewer['access_token'],{})['ticket']
            payload=ticket.split('.')[0]
            assert json.loads(base64.urlsafe_b64decode(payload+'='*((-len(payload))%4)))['purpose']=='instance_viewer'
            viewing=request(tenants[0],'/api/v1/auth/fleet',body={'ticket':ticket,'device_id':new_id()})['access_token']
            context=request(tenants[0],'/api/v1/content-access',viewing)
            assert context['instance_viewer'] and context['read_only'] and not context['support_administrator']
            request(tenants[0],f"/api/v1/projects/{own_viewer_project['id']}",viewing)
            assert projects[0]['id'] not in [p['id'] for p in request(tenants[0],'/api/v1/projects',viewing)]
            request(tenants[0],f"/api/v1/projects/{projects[0]['id']}",viewing,expected=401)
            # Even a local editor assignment cannot exceed the instance ceiling.
            request(tenants[0],f"/api/v1/projects/{projects[0]['id']}/members",own,{'user_id':viewer['user_id'],'role':'editor'},expected=204)
            request(tenants[0],f"/api/v1/projects/{projects[0]['id']}",viewing)
            request(tenants[0],f"/api/v1/threads/{threads[0]['id']}/messages",viewing)
            request(tenants[0],f'/api/v1/runs/{run_id}',viewing)
            request(tenants[0],'/api/v1/projects',viewing,{'name':'Blocked','privacy':'private_local'},expected=403)
            request(tenants[0],'/api/v1/runs',viewing,{},expected=403)
            request(tenants[0],'/api/v1/executors',viewing,expected=403)
            request(management,membership_path+'/candidates',viewer['access_token'],expected=404)
            request(management,membership_path,viewer['access_token'],{'user_id':viewer['user_id'],'role':'manager'},expected=404)
            rows=request(management,membership_path,owner['access_token'])
            assert next(p for p in rows if p['user_id']==owner['user_id'])['role']=='owner'
            assert next(p for p in rows if p['user_id']==viewer['user_id'])['email']=='viewer@example.test'
            stream=urlopen(Request(tenants[0]+f'/api/v1/runs/{run_id}/events',headers={'Authorization':'Bearer '+viewing}),timeout=8)
            while stream.readline().strip(): pass
            assert request(management,membership_path+'/'+viewer['user_id'],owner['access_token'],method='DELETE')['removed']
            assert not request(management,membership_path+'/'+viewer['user_id'],owner['access_token'],method='DELETE')['removed']
            assert stream.read()==b'', 'removed instance member retained an open event stream'
            stream.close()
            request(tenants[0],'/api/v1/projects',viewing,expected=401)
            request(management,base+f'/instances/{instances[0]}/launch',viewer['access_token'],{},expected=404)
            request(management,membership_path,owner['access_token'],{'user_id':viewer['user_id'],'role':'manager'})
            assert next(row for row in request(management,base+'/instances',viewer['access_token']) if row['id']==instances[0])['can_manage']
            request(management,membership_path+'/candidates',viewer['access_token'])
            request(management,membership_path,viewer['access_token'],{'user_id':support['user_id'],'role':'viewer'})
            request(management,membership_path+'/'+support['user_id'],owner['access_token'],method='DELETE')
            request(management,membership_path+'/'+viewer['user_id'],viewer['access_token'],method='DELETE')
            request(management,membership_path+'/candidates',viewer['access_token'],expected=404)
            member('viewer@example.test','auditor')
            request(management,membership_path,owner['access_token'],{'user_id':viewer['user_id'],'role':'member'},expected=422)
            print('instance-member-directory-viewer-ceiling-and-session-agent-revocation=ok')
            audit=request(management,base+'/audit',admin['access_token'])
            events=audit['items'] if isinstance(audit,dict) else audit
            reads=[e for e in events if e['action']=='content.read_authorized']
            assert reads and any(e['payload']['metadata']['support_grant_id'] is not None for e in reads)
            assert any(e['payload']['metadata']['support_grant_id'] is None and e['target_id']==project2['id'] for e in reads)
            assert all('reason' not in e['payload']['metadata'] and 'input' not in e['payload']['metadata'] for e in reads)
            manager.terminate(); manager.wait(timeout=10)
            request(tenants[0],'/api/v1/projects',own,expected=401)
            request(tenants[1],'/api/v1/projects',reader2,expected=401)
            print('chained-audit-and-management-outage-fail-closed=ok')
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
            subprocess.run([str(pg/'pg_ctl'),'-D',str(data),'stop','-m','immediate','-w'],stdout=subprocess.DEVNULL,check=True)


if __name__ == '__main__':
    main()
