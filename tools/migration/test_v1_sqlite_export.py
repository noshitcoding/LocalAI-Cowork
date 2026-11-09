"""Private SQLite fixture qualification; never discovers or opens user application databases.
Run: python -B tools/migration/test_v1_sqlite_export.py [--evidence-dir NEW_ABSOLUTE_DIR]
"""
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
import v1_sqlite_export as adapter
import schema_profiles as profiles

PROOF = {'native_child_receipts': [], 'cases': [], 'sqlite_engine': sqlite3.sqlite_version,
         'inventory_receipts': [], 'instrumented_sqlite_connections': 0,
         'original_source_open_attempts': 0, 'original_source_opened_by_sqlite': False, 'm9_complete': False, 'activation': False}
BASE = None

def desktop(path, version=28, patch_backend=False):
    with contextlib.closing(sqlite3.connect(path)) as conn:
        conn.executescript('PRAGMA foreign_keys=OFF;PRAGMA legacy_alter_table=ON;CREATE TABLE schema_version(version INTEGER NOT NULL);INSERT INTO schema_version VALUES(0);')
        for events in profiles.DESKTOP_STAGES[:version]: adapter.apply_events(conn, events)
        if patch_backend: conn.executescript(profiles.DESKTOP_BACKPATCH)
        conn.commit()

def insert_required(conn, table, values):
    values = dict(values)
    for _, name, kind, required, default, pk, _ in conn.execute('PRAGMA table_xinfo('+adapter.quote(table)+')'):
        if name not in values and (required and default is None or pk):
            values[name] = 0 if kind == 'INTEGER' else 0.5 if kind == 'REAL' else 'fixture-'+name
    conn.execute('INSERT INTO '+adapter.quote(table)+'('+','.join(adapter.quote(name) for name in values)+') VALUES('+','.join('?' for _ in values)+')', list(values.values()))

def remove_owned(directory):
    if not directory.exists(): return
    if BASE not in directory.parents: raise AssertionError('Cleanup outside private fixture root')
    for path in directory.rglob('*'):
        if path.is_file(): os.chmod(path, 0o600)
    shutil.rmtree(directory)

class Fixtures(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='case-', dir=BASE))
        self.sources = self.root/'sources'; self.sources.mkdir()
        self.outputs = self.root/'outputs'; self.outputs.mkdir()
        self.authorities = self.root/'authorities'; self.authorities.mkdir()
        self.source = self.sources/'source.sqlite3'
        self.inv = self.outputs/'inventory'
        self.out = self.outputs/'export'
    def tearDown(self):
        # Positive handle-close witness: every frozen input may be write-opened after the operation.
        for path in self.root.rglob('*'):
            if path.is_file():
                os.chmod(path, 0o600)
                with open(path, 'r+b'): pass
        PROOF['cases'].append({'name': self.id(), 'write_reopen_after_operation': True})
        remove_owned(self.root)
    def inventory(self, family='desktop'):
        before = {str(path): adapter.hash_file(path) for path in self.sources.iterdir()}
        real_connect = sqlite3.connect
        def isolated_connect(database, *args, **kwargs):
            value = str(database)
            if str(self.sources).replace('\\','/').lower() in value.replace('\\','/').lower() or self.sources.as_uri().lower() in value.lower():
                PROOF['original_source_open_attempts'] += 1
                raise AssertionError('SQLite attempted to open original source')
            PROOF['instrumented_sqlite_connections'] += 1
            return real_connect(database, *args, **kwargs)
        succeeded=False
        try:
            with patch.object(sqlite3, 'connect', isolated_connect):
                receipt = adapter.inventory(self.source, family, self.inv)
                succeeded=True
        finally:
            after={str(path): adapter.hash_file(path) for path in self.sources.iterdir()}
            self.assertEqual(before,after)
            PROOF['inventory_receipts'].append({'case':self.id(),'success':succeeded,'before':before,'after':after,'unchanged':True,'original_sqlite_open_guard':True})
        return receipt
    def authority(self, receipt, state='approved'):
        value = {'format': adapter.FORMAT, 'state': state, 'purpose': 'historical_export',
                 'inventory_sha256': receipt['approved_inventory_sha256'],
                 'expires_at': (datetime.now(timezone.utc)+timedelta(minutes=5)).isoformat()}
        path = self.authorities/'authority.json'; adapter.write_json(path, value)
        return path, adapter.hash_file(path)
    def export(self, receipt, state='approved'):
        path, sha = self.authority(receipt, state)
        return adapter.export(self.inv/'inventory.json', receipt['approved_inventory_sha256'], path, sha, self.out)
    def refused(self, action):
        with self.assertRaises((adapter.Refused, sqlite3.Error, OSError)): action()
        self.assertFalse(self.out.exists())
        self.assertEqual([], list(self.outputs.glob('.m9-stage-*')))
    def test_all_29_desktop_schema_versions_and_backpatch(self):
        for version in range(29):
            with self.subTest(version=version):
                desktop(self.source, version)
                receipt = self.inventory()
                self.assertEqual('desktop-'+str(version), receipt['profile'])
                remove_owned(self.inv); self.source.unlink()
        for version in (27, 28):
            desktop(self.source, version, True)
            receipt = self.inventory()
            self.assertEqual('desktop-'+str(version)+'-backend-selection', receipt['profile'])
            remove_owned(self.inv); self.source.unlink()
    def test_uninitialized_real_sqlite_header(self):
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            conn.execute('PRAGMA user_version=0'); conn.commit()
        self.assertEqual('desktop-0-uninitialized', self.inventory()['profile'])
    def test_daemon_base_and_sync_profiles(self):
        for sync in (False, True):
            with contextlib.closing(sqlite3.connect(self.source)) as conn:
                conn.executescript(profiles.DAEMON_BASE)
                if sync: conn.executescript(profiles.DAEMON_SYNC)
                insert_required(conn,'daemon_runs',{'id':'active-run','state':'running','record_json':'{"state":"running"}'})
                insert_required(conn,'daemon_schedules',{'id':'active-schedule','enabled':1})
                conn.commit()
            receipt = self.inventory('daemon')
            self.assertEqual('daemon-sync-15' if sync else 'daemon-base-13', receipt['profile'])
            result = self.export(receipt)
            manifest = json.loads(Path(result['export']).read_text())
            self.assertEqual(adapter.POLICY, manifest['execution_policy'])
            self.assertEqual(1, manifest['content']['daemon_runs']['rows'])
            remove_owned(self.inv); remove_owned(self.out)
            for path in self.authorities.iterdir(): path.unlink()
            self.source.unlink()
    def test_typed_canonical_content_and_no_replay(self):
        desktop(self.source, 28, True)
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            insert_required(conn,'audit_events',{'id':'blob','event_type':'unknown','details_json':b'\x00\xff'})
            conn.execute("UPDATE audit_events SET ts=CAST(X'ff00' AS TEXT) WHERE id='blob'")
            insert_required(conn,'engine_runs',{'id':'active','status':'running','input_tokens':9223372036854775807,'output_tokens':-9223372036854775808,'cost_usd':0.125})
            insert_required(conn,'scheduled_tasks',{'id':'schedule','active':1})
            conn.commit()
        receipt = self.inventory(); result = self.export(receipt)
        manifest = json.loads(Path(result['export']).read_text())
        adapter.verify_manifest(manifest)
        self.assertEqual(adapter.POLICY, manifest['execution_policy'])
        self.assertTrue(manifest['execution_policy']['historical_only'])
        self.assertTrue(all(not value for key,value in manifest['execution_policy'].items() if key!='historical_only'))
        row = json.loads((self.out/manifest['content']['audit_events']['file']).read_text())
        columns = manifest['content']['audit_events']['columns']
        self.assertEqual({'type':'null'},row['cells'][columns.index('resource_id')])
        self.assertEqual({'type':'blob','base64':'AP8='}, row['cells'][columns.index('details_json')])
        self.assertEqual({'type':'text','base64':'/wA='}, row['cells'][columns.index('ts')])
        row = json.loads((self.out/manifest['content']['engine_runs']['file']).read_text())
        columns = manifest['content']['engine_runs']['columns']
        self.assertEqual({'type':'integer','decimal':'9223372036854775807'},row['cells'][columns.index('input_tokens')])
        self.assertEqual({'type':'integer','decimal':'-9223372036854775808'},row['cells'][columns.index('output_tokens')])
        self.assertEqual({'type':'real','ieee754_binary64_be':'3fc0000000000000'},row['cells'][columns.index('cost_usd')])
    def test_committed_wal_rows_exported_without_source_sqlite_open(self):
        origin = self.root/'writer.sqlite3'
        desktop(origin, 1)
        with contextlib.closing(sqlite3.connect(origin)) as conn:
            conn.execute('PRAGMA journal_mode=WAL'); conn.execute('PRAGMA wal_autocheckpoint=0')
            insert_required(conn,'chat_threads',{'id':'committed-only-in-wal'}); conn.commit()
            for suffix in ('','-wal','-shm'):
                shutil.copyfile(Path(str(origin)+suffix),Path(str(self.source)+suffix))
        self.assertGreater(Path(str(self.source)+'-wal').stat().st_size,32)
        receipt = self.inventory()
        manifest = json.loads((self.inv/'inventory.json').read_text())
        self.assertEqual(1,manifest['details']['row_counts']['chat_threads'])
        self.assertEqual(3,len(manifest['source_files']))
        self.export(receipt)
    def test_corrupt_wal_checksum_and_uncommitted_tail_rejected(self):
        origin = self.root/'writer.sqlite3';desktop(origin,1)
        with contextlib.closing(sqlite3.connect(origin)) as conn:
            conn.execute('PRAGMA journal_mode=WAL');conn.execute('PRAGMA wal_autocheckpoint=0')
            insert_required(conn,'chat_threads',{'id':'committed'});conn.commit()
            for suffix in ('','-wal','-shm'):
                shutil.copyfile(Path(str(origin)+suffix),Path(str(self.source)+suffix))
            wal=Path(str(self.source)+'-wal')
            with open(wal,'r+b') as stream:stream.seek(60);value=stream.read(1);stream.seek(60);stream.write(bytes([value[0]^1]))
            self.refused(lambda:self.inventory())
            for path in self.sources.iterdir():path.unlink()
            for suffix in ('','-shm'):
                shutil.copyfile(Path(str(origin)+suffix),Path(str(self.source)+suffix))
            conn.execute('PRAGMA cache_size=1');conn.execute('PRAGMA cache_spill=ON');conn.execute('BEGIN IMMEDIATE')
            for i in range(8):insert_required(conn,'audit_events',{'id':'pending-'+str(i),'details_json':b'x'*131072})
            shutil.copyfile(Path(str(origin)+'-wal'),Path(str(self.source)+'-wal'))
            conn.rollback()
        with open(Path(str(self.source)+'-wal'),'rb') as stream:
            header=stream.read(32);page=int.from_bytes(header[8:12],'big')
            stream.seek(-(page+24),2);self.assertEqual(0,int.from_bytes(stream.read(8)[4:8],'big'))
        self.refused(lambda:self.inventory())
    def test_approval_content_tamper_resealed_and_pinned_still_rejected(self):
        desktop(self.source,1);receipt=self.inventory()
        path=self.inv/'inventory.json';os.chmod(path,0o600)
        value=json.loads(path.read_text());value['content']['chat_threads']['sha256']='0'*64
        value['content_sha256']=adapter.digest(value['content'])
        value.pop('manifest_sha256');value=adapter.bundle_manifest(value)
        path.write_bytes(adapter.canonical(value)+b'\n')
        receipt['approved_inventory_sha256']=adapter.hash_file(path)
        self.refused(lambda:self.export(receipt))
    def test_source_and_output_directory_rename_denied_until_fence_release(self):
        desktop(self.source,1)
        with adapter.FrozenFiles([self.source]):
            with self.assertRaises(PermissionError):os.rename(self.sources,self.root/'renamed-sources')
        renamed=self.root/'renamed-sources';os.rename(self.sources,renamed);os.rename(renamed,self.sources)
        with adapter.FrozenDirectories([self.outputs]):
            with self.assertRaises(PermissionError):os.rename(self.outputs,self.root/'renamed-outputs')
        renamed=self.root/'renamed-outputs';os.rename(self.outputs,renamed);os.rename(renamed,self.outputs)
    def test_unknown_version_duplicate_empty_and_wrong_marker(self):
        for marker in ('UPDATE schema_version SET version=29','INSERT INTO schema_version VALUES(28)','DELETE FROM schema_version',"UPDATE schema_version SET version='bad'"):
            desktop(self.source)
            with contextlib.closing(sqlite3.connect(self.source)) as conn:
                conn.execute(marker);conn.commit()
            self.refused(lambda:self.inventory());self.source.unlink()
    def test_unknown_table_column_trigger_view_and_user_version(self):
        changes=['CREATE TABLE unknown(id TEXT)','ALTER TABLE chat_threads ADD COLUMN unknown TEXT',
                 'CREATE VIEW unknown AS SELECT id FROM chat_threads',
                 'CREATE TRIGGER unknown AFTER INSERT ON chat_threads BEGIN SELECT 1; END',
                 'PRAGMA user_version=1']
        for sql in changes:
            desktop(self.source)
            with contextlib.closing(sqlite3.connect(self.source)) as conn:conn.execute(sql);conn.commit()
            self.refused(lambda:self.inventory());self.source.unlink()
    def test_broken_foreign_key_rejected_with_integrity_ok_witness(self):
        desktop(self.source,1)
        with contextlib.closing(sqlite3.connect(self.source)) as conn:
            insert_required(conn,'chat_messages',{'id':'orphan','thread_id':'missing'});conn.commit()
            self.assertEqual([('ok',)],list(conn.execute('PRAGMA integrity_check')))
            self.assertIsNotNone(conn.execute('PRAGMA foreign_key_check').fetchone())
        self.refused(lambda:self.inventory())
    def test_dirty_journal_invalid_header_and_invalid_wal(self):
        desktop(self.source,1)
        journal=Path(str(self.source)+'-journal');journal.write_bytes(b'dirty')
        self.refused(lambda:self.inventory());journal.unlink()
        wal=Path(str(self.source)+'-wal');wal.write_bytes(b'invalid')
        self.refused(lambda:self.inventory());wal.unlink()
        self.source.write_bytes(b'invalid-sqlite')
        self.refused(lambda:self.inventory())
    def test_active_writer_and_native_denied_writer(self):
        desktop(self.source,1)
        with open(self.source,'r+b'):
            self.refused(lambda:self.inventory())
        before=adapter.hash_file(self.source)
        with adapter.FrozenFiles([self.source]) as frozen:
            command=[sys.executable,'-B','-c',"import sys;\ntry:\n f=open(sys.argv[1],'r+b'); f.close(); sys.exit(3)\nexcept PermissionError: sys.exit(0)",str(self.source)]
            process=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                stdout,stderr=process.communicate(timeout=15)
                code=process.wait(timeout=1)
                self.assertEqual(0,code,(stdout,stderr))
                PROOF['native_child_receipts'].append({'pid':process.pid,'exit_code':code,'wait_reaped':True,'timed_out':False})
            finally:
                if process.poll() is None:process.kill();process.wait(timeout=5)
            self.assertFalse(frozen.closed)
        self.assertTrue(frozen.closed)
        self.assertEqual(before,adapter.hash_file(self.source))
        with open(self.source,'r+b'):pass
    def test_manipulated_approved_manifest_and_snapshot(self):
        desktop(self.source,1);receipt=self.inventory()
        authority,sha=self.authority(receipt)
        path=self.inv/'inventory.json';os.chmod(path,0o600)
        value=json.loads(path.read_text());value['execution_policy']['resume']=True
        seal=dict(value);seal.pop('manifest_sha256');value['manifest_sha256']=adapter.digest(seal)
        path.write_bytes(adapter.canonical(value)+b'\n')
        self.refused(lambda:adapter.export(path,receipt['approved_inventory_sha256'],authority,sha,self.out))
        remove_owned(self.inv)
        receipt=self.inventory()
        authority.unlink();authority,sha=self.authority(receipt)
        snapshot=self.inv/'snapshot.sqlite3';os.chmod(snapshot,0o600)
        with open(snapshot,'r+b') as stream:stream.seek(-1,2);stream.write(b'X')
        self.refused(lambda:adapter.export(path,receipt['approved_inventory_sha256'],authority,sha,self.out))
    def test_revoked_expired_and_wrong_purpose_authority(self):
        desktop(self.source,1);receipt=self.inventory()
        for state,purpose,expiry in [('revoked','historical_export',datetime.now(timezone.utc)+timedelta(minutes=5)),
                                     ('approved','activate',datetime.now(timezone.utc)+timedelta(minutes=5)),
                                     ('approved','historical_export',datetime.now(timezone.utc)-timedelta(minutes=1))]:
            path,sha=self.authority(receipt,state)
            value=json.loads(path.read_text());value['purpose']=purpose;value['expires_at']=expiry.isoformat()
            path.write_bytes(adapter.canonical(value)+b'\n');sha=adapter.hash_file(path)
            self.refused(lambda:adapter.export(self.inv/'inventory.json',receipt['approved_inventory_sha256'],path,sha,self.out))
            path.unlink()
    def test_output_collision_overlap_and_traversal_rejected(self):
        desktop(self.source,1)
        self.inv.mkdir();self.refused(lambda:self.inventory());self.inv.rmdir()
        self.refused(lambda:adapter.inventory(self.source,'desktop',self.sources/'export'))
        self.refused(lambda:adapter.inventory(str(self.sources/'..'/'sources'/'source.sqlite3'),'desktop',self.inv))
    def test_limits_nonfinite_and_duplicate_json(self):
        desktop(self.source,1)
        with patch.object(adapter,'MAX_SOURCE',1):self.refused(lambda:self.inventory())
        with patch.object(adapter,'MAX_ROWS',0):self.refused(lambda:self.inventory())
        with patch.object(adapter,'MAX_EXPORT',1):self.refused(lambda:self.inventory())
        with patch.object(adapter,'MAX_SECONDS',-1):self.refused(lambda:self.inventory())
        with patch.object(adapter,'MAX_CELL',3):
            with self.assertRaises(adapter.Refused):adapter.cell('blob',b'four')
        with self.assertRaises(adapter.Refused):adapter.cell('real',float('inf'))
        path=self.authorities/'authority.json';path.write_bytes(b'{"state":"approved","state":"revoked"}')
        with adapter.FrozenFiles([path]) as fence:
            with self.assertRaises(adapter.Refused):adapter.read_json(fence,path)
        path.write_bytes(b'{\"nested\":'+b'['*64+b'0'+b']'*64+b'}')
        with adapter.FrozenFiles([path]) as fence:
            with self.assertRaises(adapter.Refused):adapter.read_json(fence,path)
    def test_cli_processes_positive_exit_and_reap(self):
        desktop(self.source,1)
        command=[sys.executable,'-B',str(Path(adapter.__file__)), 'inventory','--source',str(self.source),'--family','desktop','--output',str(self.inv)]
        process=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            stdout,stderr=process.communicate(timeout=30);code=process.wait(timeout=1)
            self.assertEqual(0,code,stderr)
            receipt=json.loads(stdout)
            PROOF['native_child_receipts'].append({'pid':process.pid,'exit_code':code,'wait_reaped':True,'timed_out':False})
        finally:
            if process.poll() is None:process.kill();process.wait(timeout=5)
        path,sha=self.authority(receipt)
        command=[sys.executable,'-B',str(Path(adapter.__file__)), 'export','--inventory',str(self.inv/'inventory.json'),
                 '--approved-inventory-sha256',receipt['approved_inventory_sha256'],'--authority',str(path),'--authority-sha256',sha,'--output',str(self.out)]
        process=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            stdout,stderr=process.communicate(timeout=30);code=process.wait(timeout=1)
            self.assertEqual(0,code,stderr)
            PROOF['native_child_receipts'].append({'pid':process.pid,'exit_code':code,'wait_reaped':True,'timed_out':False})
            self.assertIn('export_sha256',json.loads(stdout))
        finally:
            if process.poll() is None:process.kill();process.wait(timeout=5)

def main():
    global BASE
    parser=argparse.ArgumentParser();parser.add_argument('--evidence-dir')
    args=parser.parse_args()
    if os.name!='nt':
        print('REFUSED: This qualification requires native Windows deny-write handles.',file=sys.stderr);return 2
    BASE=Path(args.evidence_dir) if args.evidence_dir else Path(tempfile.mkdtemp(prefix='cowork-m9-fixtures-'))
    if args.evidence_dir:BASE.mkdir(parents=False,exist_ok=False)
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(Fixtures)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    PROOF.update({'tests_run':result.testsRun,'failures':len(result.failures),'errors':len(result.errors),
                  'success':result.wasSuccessful(),'source_bindings':profiles.SOURCE_BINDINGS,
                  'product_source_sha256':{path.name:adapter.hash_file(path) for path in Path(__file__).parent.glob('*.py')},
                  'failure_details':[{'test':str(test),'traceback':trace} for test,trace in result.failures],
                  'error_details':[{'test':str(test),'traceback':trace} for test,trace in result.errors],
                  'fixture_cleanup_complete':not list(BASE.glob('case-*'))})
    (BASE/'qualification.json').write_bytes(adapter.canonical(PROOF)+b'\n')
    print('Evidence: '+str(BASE/'qualification.json'))
    return 0 if result.wasSuccessful() else 1

if __name__=='__main__':raise SystemExit(main())
