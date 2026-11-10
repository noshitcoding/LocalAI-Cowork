"""Real private SQLite tests. No live application directory discovery or DB access."""
from __future__ import annotations
import argparse
import contextlib
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import v1_quarantine as q
import v1_sqlite_export as v1
import schema_profiles as schemas
import v2_snapshot_profile as profile
from test_v1_sqlite_export import desktop, insert_required

BASE = None
PROOF = {'original_sqlite_open_attempts':0,'originals_unchanged':[],
         'native_children':[],'cases':[],'activation':False,'m9_complete':False,
         'target_credentials_exported':False,'sqlite_engine':sqlite3.sqlite_version}
P1 = '10000000-0000-0000-0000-000000000001'
P2 = '10000000-0000-0000-0000-000000000002'
R1 = '20000000-0000-0000-0000-000000000001'
R2 = '20000000-0000-0000-0000-000000000002'
O1 = '30000000-0000-0000-0000-000000000001'
ORG = '40000000-0000-0000-0000-000000000001'
ACTOR = '50000000-0000-0000-0000-000000000001'
PERSONA = '60000000-0000-0000-0000-000000000001'


class Fixtures(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='case-',dir=BASE))
        self.inputs = self.root/'inputs'; self.inputs.mkdir()
        self.outputs = self.root/'outputs'; self.outputs.mkdir()
        self.approvals = self.root/'approvals'; self.approvals.mkdir()
        self.source = self.inputs/'source.sqlite3'
        self.target = self.inputs/'target.sqlite3'
        self.export = self.outputs/'export'
        self.quarantine = self.outputs/'quarantine'
        self.result = self.outputs/'preview'
        self.mapping = self.approvals/'mapping.json'
    def tearDown(self):
        for path in self.root.rglob('*'):
            if path.is_file():
                os.chmod(path,0o600)
                with open(path,'r+b'): pass
        renamed = self.root.with_name(self.root.name+'-closed')
        os.rename(self.root,renamed)
        PROOF['cases'].append({'name':self.id(),'write_reopen_and_namespace_rename':True})
        if BASE not in renamed.parents: raise AssertionError('Private cleanup escaped fixture root')
        shutil.rmtree(renamed)
    def invoke(self, action, extras=()):
        originals = [path for path in (self.source,self.target,self.export/'export.json',self.quarantine/'quarantine.sqlite3',self.quarantine/'quarantine.json',self.mapping,*extras) if path.exists()]
        before = {str(p):v1.hash_file(p) for p in originals}
        forbidden = {str(p).replace('\\','/').lower() for p in originals} | {p.as_uri().lower() for p in originals}
        original_connect = sqlite3.connect
        def no_original(database,*args,**kwargs):
            value = str(database).replace('\\','/').lower().split('?')[0]
            if value in forbidden:
                PROOF['original_sqlite_open_attempts'] += 1
                raise AssertionError('Original input was opened by SQLite')
            return original_connect(database,*args,**kwargs)
        try:
            with patch.object(sqlite3,'connect',no_original): return action()
        finally:
            after = {str(p):v1.hash_file(p) for p in originals}
            self.assertEqual(before,after)
            PROOF['originals_unchanged'].append({'case':self.id(),'files':len(originals),'unchanged':True,'original_sqlite_guard':True})
    def source_export(self, family='desktop', version=28):
        if not self.source.exists():
            if family == 'desktop': desktop(self.source,version)
            else:
                with contextlib.closing(sqlite3.connect(self.source)) as conn: conn.executescript(schemas.DAEMON_BASE)
        inv = self.invoke(lambda:v1.inventory(self.source,family,self.outputs/'inventory'))
        approval = self.approvals/'export-authority.json'
        v1.write_json(approval,{'format':v1.FORMAT,'state':'approved','purpose':'historical_export',
                                'inventory_sha256':inv['approved_inventory_sha256'],
                                'expires_at':(datetime.now(timezone.utc)+timedelta(minutes=5)).isoformat()})
        return self.invoke(lambda:v1.export(Path(inv['inventory']),inv['approved_inventory_sha256'],approval,v1.hash_file(approval),self.export))
    def history(self, family='desktop'):
        receipt = self.source_export(family)
        return self.invoke(lambda:q.create_quarantine(Path(receipt['export']),receipt['export_sha256'],self.quarantine))
    def target_snapshot(self, tenant=False):
        with contextlib.closing(profile.reference()) as ref, contextlib.closing(sqlite3.connect(self.target)) as conn:
            ref.backup(conn)
            conn.execute('PRAGMA foreign_keys=ON')
            conn.execute('INSERT INTO projects VALUES(?,?)',(P1,json.dumps({'id':P1,'name':'existing personal'})))
            conn.execute('INSERT INTO projects VALUES(?,?)',(P2,json.dumps({'id':P2,'name':'other personal'})))
            conn.execute('INSERT INTO orders VALUES(?,?,?,?)',(O1,P1,1,'{}'))
            conn.execute('INSERT INTO runs(id,order_id,idempotency_key,request_hash,revision,state,payload,updated_at,execution_kind) VALUES(?,?,?,?,?,?,?,?,?)',(R1,O1,'current','f'*64,3,'interrupted','{}',1,'agent'))
            conn.execute('INSERT INTO identity_subjects VALUES(?,?,?,?,?,?,?,?)',(ACTOR,'fixture-human','Fixture',1,0,1,'DO_NOT_EXPORT_PASSWORD_SALT','DO_NOT_EXPORT_PASSWORD_HASH'))
            conn.execute('INSERT INTO identity_sessions VALUES(?,?,?,?,?,?)',('70000000-0000-0000-0000-000000000001',ACTOR,'DO_NOT_EXPORT_TOKEN_HASH',99999999,0,1))
            conn.execute('INSERT INTO identity_organizations VALUES(?,?,?,?,?)',(ORG,'Fixture org','active',1,1))
            if tenant:
                conn.execute('INSERT INTO identity_resources VALUES(?,?,?,?)',(PERSONA,ORG,'project',1))
                creation={'name':'Tenant fixture','idempotency_key':'fixture'}
                receipt={'id':PERSONA,'organization_id':ORG,'name':'Tenant fixture','revision':1,'created_at':1}
                encoded=lambda value:json.dumps(value,ensure_ascii=False,separators=(',',':'))
                request=hashlib.sha256(encoded(['tenant_project_create/v1',ORG,ACTOR,creation]).encode()).hexdigest()
                conn.execute('INSERT INTO tenant_projects VALUES(?,?,?,?,?,?,?,?,?)',(PERSONA,ORG,ACTOR,'fixture',request,encoded(creation),1,encoded(receipt),encoded(receipt)))
            conn.commit()
    def desktop_history(self, parent_conflict=False):
        desktop(self.source)
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            insert_required(conn,'projects',{'id':'old-project'})
            insert_required(conn,'projects',{'id':'old-other-project'})
            insert_required(conn,'chat_threads',{'id':'old-thread'})
            insert_required(conn,'chat_threads',{'id':'old-other-thread'})
            insert_required(conn,'project_threads',{'project_id':'old-project','thread_id':'old-thread'})
            insert_required(conn,'project_threads',{'project_id':'old-other-project','thread_id':'old-other-thread'})
            insert_required(conn,'engine_runs',{'id':'old-run','thread_id':'old-thread','status':'unknown','parent_run_id':'old-other-run' if parent_conflict else None})
            if parent_conflict: insert_required(conn,'engine_runs',{'id':'old-other-run','thread_id':'old-other-thread','parent_run_id':None})
            insert_required(conn,'agent_personalities',{'id':'old-persona'})
            insert_required(conn,'engine_run_artifacts',{'id':'old-artifact','run_id':'old-run','kind':'text'})
            conn.commit()
    def entries(self):
        return [self.entry('project','old-project','project',P1,P1),
                self.entry('engine_run','old-run','run',R1,P1)]
    def entry(self,kind,source,target_kind,dest,project=None,namespace='personal',org=None,actor=None):
        return {'source_kind':kind,'source_id':source,'target_kind':target_kind,'target_id':dest,
                'namespace':namespace,'project_id':project,'organization_id':org,'creator_identity_id':actor}
    def mapping_file(self, entries):
        v1.write_json(self.mapping,{'format':q.MAPPING_FORMAT,'purpose':'historical_preview',
                                   'export_sha256':json.loads((self.quarantine/'quarantine.json').read_text())['export_sha256'],
                                   'target_snapshot_sha256':v1.hash_file(self.target),'entries':entries})
    def preview(self, entries=None):
        if entries is not None: self.mapping_file(entries)
        return self.invoke(lambda:q.preview(self.quarantine/'quarantine.json',v1.hash_file(self.quarantine/'quarantine.json'),
                                            self.target,v1.hash_file(self.target),self.mapping,v1.hash_file(self.mapping),self.result))
    def reseal(self):
        path = self.quarantine/'quarantine.json'; value=json.loads(path.read_text())
        db = self.quarantine/'quarantine.sqlite3'
        value['database']['sha256']=v1.hash_file(db); value['database']['bytes']=db.stat().st_size
        value.pop('manifest_sha256'); value=v1.bundle_manifest(value)
        os.chmod(path,0o600); path.unlink();v1.write_json(path,value)
    def prepare(self):
        self.desktop_history(); self.history(); self.target_snapshot()

    def test_complete_history_preserves_unknown_runs_typed_rows_and_inactive_schema(self):
        self.desktop_history(); receipt=self.history()
        manifest=json.loads((self.quarantine/'quarantine.json').read_text())
        export=json.loads((self.export/'export.json').read_text())
        self.assertEqual(receipt['state'],'inactive')
        with v1.database(self.quarantine/'quarantine.sqlite3') as conn:
            self.assertEqual(set(v1.schema(conn)['tables']),{'quarantine_meta','source_tables','history_rows','history_projects','history_actors','history_entities','history_relations'})
            for table,item in export['content'].items():
                raw=b''.join(row[0] for row in conn.execute('SELECT canonical_row FROM history_rows WHERE table_name=? ORDER BY position',(table,)))
                self.assertEqual(hashlib.sha256(raw).hexdigest(),item['sha256'])
                if table=='engine_runs':
                    typed=json.loads(raw)
                    self.assertEqual(q.decode_cell(typed['cells'][item['columns'].index('status')])[0],b'unknown')
            self.assertEqual(conn.execute('PRAGMA foreign_key_check').fetchall(),[])
            self.assertEqual(conn.execute("SELECT project_source_id FROM history_entities WHERE kind='engine_run' AND source_id='old-run'").fetchone(),('old-project',))
        self.assertEqual(manifest['rows'],sum(item['rows'] for item in export['content'].values()))
    def test_preview_existing_ids_readonly_no_tokens_or_grants(self):
        self.prepare(); receipt=self.preview(self.entries())
        raw=Path(receipt['preview']).read_bytes(); value=json.loads(raw)
        self.assertEqual([e['diff'] for e in value['diff']['entries']],['existing_target_reference']*2)
        self.assertFalse(value['activation_allowed']); self.assertEqual(value['target_database_writes'],0)
        for secret in (b'DO_NOT_EXPORT_PASSWORD',b'DO_NOT_EXPORT_TOKEN',b'password_hash',b'token_sha256'):
            self.assertNotIn(secret,raw)
    def test_empty_desktop_zero_profile(self):
        self.source_export(version=0)
        self.invoke(lambda:q.create_quarantine(self.export/'export.json',v1.hash_file(self.export/'export.json'),self.quarantine))
    def test_desktop_zero_uninitialized_profile(self):
        with contextlib.closing(sqlite3.connect(self.source)) as conn:conn.execute('VACUUM')
        receipt=self.history()
        self.assertEqual(receipt['rows'],0)
    def test_missing_table_file_and_extra_bundle_file_rejected(self):
        receipt=self.source_export(); manifest=json.loads((self.export/'export.json').read_text())
        file=self.export/next(iter(manifest['content'].values()))['file'];os.chmod(file,0o600);file.unlink()
        with self.assertRaises((q.Refused,OSError)):q.create_quarantine(self.export/'export.json',receipt['export_sha256'],self.quarantine)
        self.assertFalse(self.quarantine.exists())
    def test_extra_bundle_member_rejected(self):
        receipt=self.source_export();(self.export/'unexpected.json').write_text('{}')
        with self.assertRaises(q.Refused):self.invoke(lambda:q.create_quarantine(self.export/'export.json',receipt['export_sha256'],self.quarantine))
        self.assertFalse(self.quarantine.exists())
    def test_unknown_schema_resealed_export_rejected(self):
        receipt=self.source_export();path=self.export/'export.json'; value=json.loads(path.read_text())
        value['details']['schema']['objects'].append({'type':'table','name':'unknown','table':'unknown','sql':'CREATE TABLE unknown(x)'})
        value.pop('manifest_sha256');os.chmod(path,0o600);path.unlink();v1.write_json(path,v1.bundle_manifest(value))
        with self.assertRaises(q.Refused):q.create_quarantine(path,v1.hash_file(path),self.quarantine)
        self.assertFalse(self.quarantine.exists())
    def test_tampered_table_bytes_rejected_and_stage_rolled_back(self):
        receipt=self.source_export();value=json.loads((self.export/'export.json').read_text()); item=next(x for x in value['content'].values() if x['rows'])
        path=self.export/item['file'];os.chmod(path,0o600)
        with open(path,'ab') as stream:stream.write(b'{}\n')
        with self.assertRaises(q.Refused):self.invoke(lambda:q.create_quarantine(self.export/'export.json',receipt['export_sha256'],self.quarantine))
        self.assertFalse(self.quarantine.exists());self.assertFalse(list(self.outputs.glob('.m9-quarantine-*')))
    def test_ambiguous_nullable_original_ids_rejected(self):
        desktop(self.source)
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            insert_required(conn,'agent_personalities',{'id':None,'name':'nullable-a'});insert_required(conn,'agent_personalities',{'id':None,'name':'nullable-b'});conn.commit()
        with self.assertRaises(q.Refused):self.history()
        self.assertFalse(self.quarantine.exists())
    def test_parent_run_cross_project_rejected(self):
        self.desktop_history(parent_conflict=True)
        with self.assertRaises(q.Refused):self.history()
        self.assertFalse(self.quarantine.exists())
    def test_mapping_duplicate_and_cross_project_rejected(self):
        self.prepare();entries=self.entries();entries[1]['project_id']=P2
        with self.assertRaises(q.Refused):self.preview(entries)
        self.assertFalse(self.result.exists());self.mapping.unlink()
        with self.assertRaises(q.Refused):self.preview([entries[0],entries[0]])
        self.assertFalse(self.result.exists())
    def test_agent_persona_cannot_become_human(self):
        self.prepare()
        with self.assertRaises(q.Refused):self.preview([self.entry('agent_persona','old-persona','identity_subject',ACTOR,namespace='history')])
    def test_pinned_unknown_source_id_rejected(self):
        self.prepare()
        with self.assertRaises(q.Refused):self.preview([self.entry('engine_run','absent','run',R2,P1)])
    def test_resealed_quarantine_row_tamper_rejected(self):
        self.prepare();path=self.quarantine/'quarantine.sqlite3';os.chmod(path,0o600)
        with contextlib.closing(sqlite3.connect(path)) as conn:
            conn.execute("UPDATE history_rows SET canonical_row=?,row_sha256=? WHERE table_name='engine_runs'",(b'{"cells":[],"rowid":"1"}\n','f'*64));conn.commit()
        self.reseal()
        with self.assertRaises(q.Refused):self.preview(self.entries())
        self.assertFalse(self.result.exists())
    def test_resealed_quarantine_actor_project_projection_rejected(self):
        self.prepare();path=self.quarantine/'quarantine.sqlite3';os.chmod(path,0o600)
        with contextlib.closing(sqlite3.connect(path)) as conn:
            conn.execute("UPDATE history_entities SET project_source_id='old-other-project' WHERE kind='engine_run'");conn.commit()
        self.reseal()
        with self.assertRaises(q.Refused):self.preview(self.entries())
    def test_target_unknown_table_and_changed_migration_checksum_rejected(self):
        self.prepare()
        with contextlib.closing(sqlite3.connect(self.target)) as conn:conn.execute('CREATE TABLE extra(x)');conn.commit()
        with self.assertRaises(q.Refused):self.preview(self.entries())
        self.mapping.unlink()
        with contextlib.closing(sqlite3.connect(self.target)) as conn:conn.execute('DROP TABLE extra');conn.execute("UPDATE schema_migrations SET checksum='unknown' WHERE version=20");conn.commit()
        with self.assertRaises(q.Refused):self.preview(self.entries())
    def test_cold_target_sidecar_rejected(self):
        self.prepare();Path(str(self.target)+'-wal').write_bytes(b'')
        with self.assertRaises(q.Refused):self.preview(self.entries())
    def test_target_fk_corruption_rejected(self):
        self.prepare()
        with contextlib.closing(sqlite3.connect(self.target)) as conn:conn.execute("UPDATE runs SET order_id='missing'");conn.commit()
        with self.assertRaises(q.Refused):self.preview(self.entries())
    def test_exclusive_publication_and_cancelled_transaction(self):
        self.prepare(); self.result.mkdir();(self.result/'sentinel').write_text('retained')
        with self.assertRaises(q.Refused):self.preview(self.entries())
        self.assertEqual((self.result/'sentinel').read_text(),'retained')
    def test_wrong_mapping_pins_and_authority_fields_rejected(self):
        self.prepare();self.mapping_file(self.entries())
        value=json.loads(self.mapping.read_text());value['export_sha256']='a'*64;self.mapping.unlink();v1.write_json(self.mapping,value)
        with self.assertRaises(q.Refused):self.preview()
        value['export_sha256']=json.loads((self.quarantine/'quarantine.json').read_text())['export_sha256'];value['activate']=True
        self.mapping.unlink();v1.write_json(self.mapping,value)
        with self.assertRaises(q.Refused):self.preview()
    def test_duplicate_target_ids_rejected(self):
        self.prepare();entries=self.entries();entries[1]['target_id']=P1
        with self.assertRaises(q.Refused):self.preview(entries)
    def test_daemon_actual_spec_projects_human_actors_are_history_only(self):
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            conn.executescript(schemas.DAEMON_BASE)
            insert_required(conn,'daemon_runs',{'id':'old-daemon-run','record_json':json.dumps({'id':'old-daemon-run','spec':{'project_id':P1,'creator_user_id':ACTOR,'project':{'id':P1,'revision':1}},'state':'unknown'}),'state':'unknown'});conn.commit()
        receipt=self.history('daemon');self.target_snapshot()
        entries=[self.entry('project',P1,'project',P1,P1),self.entry('daemon_run','old-daemon-run','run',R2,P1),self.entry('historical_actor',ACTOR,'identity_subject',ACTOR,namespace='history')]
        result=self.preview(entries);value=json.loads(Path(result['preview']).read_text())
        self.assertTrue(value['diff']['complete_mapping']);self.assertTrue(value['diff']['fresh_principal_required_for_activation'])
        self.assertEqual(receipt['entities']['historical_actor'],1)
    def test_tenant_namespace_cannot_adopt_personal_project(self):
        self.prepare()
        with self.assertRaises(q.Refused):self.preview([self.entry('project','old-project','project',P1,P1,'tenant',ORG,ACTOR)])
    def test_target_pin_change_and_reference_source_change_rejected(self):
        self.prepare();self.mapping_file(self.entries());old=v1.hash_file(self.target)
        with contextlib.closing(sqlite3.connect(self.target)) as conn:conn.execute('UPDATE runs SET revision=revision+1');conn.commit()
        with self.assertRaises(q.Refused):self.invoke(lambda:q.preview(self.quarantine/'quarantine.json',v1.hash_file(self.quarantine/'quarantine.json'),self.target,old,self.mapping,v1.hash_file(self.mapping),self.result))
        # A altered reference binding is rejected without modifying any product file.
        self.mapping.unlink();self.mapping_file(self.entries());bindings=dict(profile.source_paths());first=next(iter(bindings));bindings[first]='0'*64
        with patch.object(profile,'source_paths',lambda:bindings):
            with self.assertRaises(q.Refused):self.preview()
    def test_native_cli_actual_exit_wait_and_readonly_preview(self):
        self.prepare();self.mapping_file(self.entries())
        command=[sys.executable,'-B',str(Path(q.__file__)),'preview','--quarantine',str(self.quarantine/'quarantine.json'),'--quarantine-sha256',v1.hash_file(self.quarantine/'quarantine.json'),'--target-snapshot',str(self.target),'--target-snapshot-sha256',v1.hash_file(self.target),'--mapping',str(self.mapping),'--mapping-sha256',v1.hash_file(self.mapping),'--output',str(self.result)]
        before=v1.hash_file(self.target)
        child=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
        output,error=child.communicate(timeout=30);exit_code=child.wait(timeout=5)
        self.assertEqual(exit_code,0,error.decode());self.assertTrue(json.loads(output)['preview'])
        self.assertEqual(v1.hash_file(self.target),before)
        PROOF['native_children'].append({'pid':child.pid,'wait_exit_code':exit_code,'positive_wait_reap':True,'preview_no_target_write':True})

    def test_roundtrip_real_bits_invalid_utf8_blob_and_integer_extremes(self):
        self.desktop_history()
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            conn.execute("UPDATE projects SET instructions=CAST(x'80ff0061' AS TEXT) WHERE id='old-project'")
            insert_required(conn,'tasks',{'id':'typed-task','thread_id':'old-thread'})
            insert_required(conn,'task_steps',{'id':'typed-min','task_id':'typed-task','idx':-(1<<63)})
            insert_required(conn,'task_steps',{'id':'typed-max','task_id':'typed-task','idx':(1<<63)-1})
            insert_required(conn,'insights_events',{'id':'typed-real','value_num':0.125})
            insert_required(conn,'chat_messages',{'id':'typed-blob','thread_id':'old-thread','content':b'\x00\x80\xff'})
            conn.commit()
        self.history();self.target_snapshot();self.preview(self.entries())

    def test_tenant_existing_project_exact_resource_creator_and_receipt(self):
        self.desktop_history();self.history();self.target_snapshot(tenant=True)
        result=self.preview([self.entry('project','old-project','project',PERSONA,PERSONA,'tenant',ORG,ACTOR)])
        value=json.loads(Path(result['preview']).read_text())
        self.assertEqual(value['diff']['entries'][0]['target_facts']['creator_identity_id'],ACTOR)
        self.assertEqual(value['grants_created'],0)

    def test_tenant_resealed_creation_receipt_is_rejected(self):
        self.desktop_history();self.history();self.target_snapshot(tenant=True)
        with contextlib.closing(sqlite3.connect(self.target)) as conn:
            conn.execute("UPDATE tenant_projects SET initial_receipt='{}'");conn.commit()
        with self.assertRaises(q.Refused):self.preview([self.entry('project','old-project','project',PERSONA,PERSONA,'tenant',ORG,ACTOR)])

    def test_artifact_requires_original_run_and_exact_project_mapping(self):
        self.prepare();artifact=self.entry('engine_artifact','old-artifact','artifact','80000000-0000-0000-0000-000000000001',P1)
        with self.assertRaises(q.Refused):self.preview([self.entries()[0],artifact])
        self.mapping.unlink()
        result=self.preview([*self.entries(),artifact])
        self.assertEqual(result['mapped'],3)

    def test_all_exact_source_profiles_are_reconstructed(self):
        container=self.root/'profiles';container.mkdir()
        profiles=[]
        for version in range(29):profiles.append(('desktop',version,False))
        profiles.extend([('desktop',27,True),('desktop',28,True),('daemon',None,False),('daemon',None,True)])
        for ordinal,(family,version,extra) in enumerate(profiles):
            with self.subTest(family=family,version=version,extra=extra):
                case=container/str(ordinal);case.mkdir();inputs=case/'input';inputs.mkdir();out=case/'output';out.mkdir();approvals=case/'approval';approvals.mkdir()
                source=inputs/'source.sqlite3'
                if family=='desktop':desktop(source,version,extra)
                else:
                    with contextlib.closing(sqlite3.connect(source)) as conn:conn.executescript(schemas.DAEMON_BASE+(schemas.DAEMON_SYNC if extra else ''))
                inv=self.invoke(lambda:v1.inventory(source,family,out/'inventory'),extras=(source,))
                approval=approvals/'export.json'
                v1.write_json(approval,{'format':v1.FORMAT,'state':'approved','purpose':'historical_export','inventory_sha256':inv['approved_inventory_sha256'],'expires_at':(datetime.now(timezone.utc)+timedelta(minutes=5)).isoformat()})
                exported=self.invoke(lambda:v1.export(Path(inv['inventory']),inv['approved_inventory_sha256'],approval,v1.hash_file(approval),out/'export'),extras=(source,))
                before=v1.hash_file(source)
                result=self.invoke(lambda:q.create_quarantine(Path(exported['export']),exported['export_sha256'],out/'quarantine'),extras=(source,))
                self.assertEqual(before,v1.hash_file(source));self.assertEqual(result['state'],'inactive')
        PROOF['roundtrip_profiles']=len(profiles)

    def test_actual_native_new_fence_denies_writer_and_namespace_rename(self):
        self.source.write_bytes(b'private fixture')
        from migration_io import FileFence
        helper='''import json,os,sys
path=sys.argv[1]
checks={}
try:
    with open(path,'r+b'): pass
    checks['write_denied']=False
except PermissionError: checks['write_denied']=True
parent=os.path.dirname(path)
try:
    os.rename(parent,parent+'-renamed')
    checks['namespace_rename_denied']=False
except PermissionError: checks['namespace_rename_denied']=True
print(json.dumps(checks))
sys.exit(0 if all(checks.values()) else 1)
'''
        with FileFence([self.source]) as fence:
            child=subprocess.Popen([sys.executable,'-I','-B','-S','-c',helper,str(self.source)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
            output,error=child.communicate(timeout=15);code=child.wait(timeout=5)
            self.assertEqual(code,0,error.decode());checks=json.loads(output)
            self.assertTrue(checks['write_denied']);self.assertTrue(checks['namespace_rename_denied'])
            self.assertEqual(fence.receipt(self.source)['sha256'],hashlib.sha256(b'private fixture').hexdigest())
        with open(self.source,'r+b'):pass
        with open(self.source,'r+b'):
            with self.assertRaises(q.Refused):
                with FileFence([self.source]):pass
        PROOF['native_children'].append({'pid':child.pid,'wait_exit_code':code,'positive_wait_reap':True,'actual_write_and_namespace_denied':True})


def main():
    global BASE
    parser=argparse.ArgumentParser();parser.add_argument('--evidence-dir',required=True)
    args=parser.parse_args();BASE=Path(args.evidence_dir)
    if not BASE.is_absolute() or BASE.exists():raise ValueError('New absolute private evidence directory required')
    BASE.mkdir()
    source_paths=[*sorted(Path(__file__).parent.glob('*.py'))]
    before={p.name:v1.hash_file(p) for p in source_paths}
    reference_before={str(path):v1.hash_file(path) for path in profile.source_paths()}
    if os.name != 'nt':
        PROOF.update({'success':False,'reason':'Windows enforced fence required; this suite does not claim Linux import qualification'})
        v1.write_json(BASE/'qualification.json',PROOF)
        return 2
    result=unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Fixtures))
    after={p.name:v1.hash_file(p) for p in source_paths}
    reference_after={str(path):v1.hash_file(path) for path in profile.source_paths()}
    reference_matches=all(reference_after[str(path)]==expected for path,expected in profile.source_paths().items())
    PROOF.update({'tests':result.testsRun,'failures':len(result.failures),'errors':len(result.errors),
                  'source_before':before,'source_after':after,'source_unchanged':before==after,
                  'reference_before':reference_before,'reference_after':reference_after,
                  'reference_unchanged':reference_before==reference_after,'reference_bindings_match':reference_matches,
                  'success':result.wasSuccessful() and before==after and reference_before==reference_after and reference_matches,'source_bindings':profile.SOURCE_BINDINGS})
    v1.write_json(BASE/'qualification.json',PROOF)
    return 0 if PROOF['success'] else 1


if __name__=='__main__':raise SystemExit(main())
