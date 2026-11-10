"""Real Windows file handles/namespace fixtures; no installed application is launched."""
from __future__ import annotations
import argparse
import contextlib
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import subprocess
import sys
import threading
import unittest
import zipfile
sys.path.insert(0, str(Path(__file__).resolve().parent))
import windows_runtime_inventory as inv

EVIDENCE = None


def sha(data): return hashlib.sha256(data).hexdigest()


def pe_fixture(version=(0, 3, 12, 0)):
    data = bytearray(1024); data[:2] = b'MZ'; struct.pack_into('<I', data, 60, 128)
    data[128:132] = b'PE\0\0'; struct.pack_into('<HH', data, 132, 0x8664, 1)
    struct.pack_into('<H', data, 148, 240)
    optional = 152; struct.pack_into('<H', data, optional, 0x20b)
    struct.pack_into('<I', data, optional+108, 16)
    struct.pack_into('<II', data, optional+128, 0x1000, 180)
    section = optional + 240; data[section:section+8] = b'.rsrc\0\0\0'
    struct.pack_into('<IIII', data, section+8, 512, 0x1000, 512, 512)
    base = 512
    for at in (0, 24, 48): struct.pack_into('<HH', data, base+at+12, 0, 1)
    struct.pack_into('<II', data, base+16, 16, 0x80000000 | 24)
    struct.pack_into('<II', data, base+40, 1, 0x80000000 | 48)
    struct.pack_into('<II', data, base+64, 1033, 72)
    struct.pack_into('<IIII', data, base+72, 0x1000+88, 92, 0, 0)
    struct.pack_into('<HHH', data, base+88, 92, 52, 0)
    key = 'VS_VERSION_INFO\0'.encode('utf-16le'); data[base+94:base+94+len(key)] = key
    ms = version[0] << 16 | version[1]; ls = version[2] << 16 | version[3]
    struct.pack_into('<13I', data, base+128, 0xfeef04bd, 0x10000, ms, ls, ms, ls, 0, 0, 0, 0, 0, 0, 0)
    return bytes(data)


class Fixture:
    def __init__(self, path):
        self.path = path; path.mkdir(); self.local = path/'installed-local'; self.local.mkdir()
        self.open = path/'installed-open'; self.open.mkdir()
        self.reference = path/'reference'; self.reference.mkdir()
        self.roots = {'Local AI Cowork': self.local, 'Open Cowork': self.open}
        self.data = {}
        for relative in inv.LOCAL_FILES:
            if relative == 'runtime-layout.json': continue
            value = pe_fixture() if relative.endswith(('.dll','.exe')) else ('fixture '+relative+'\n').encode()
            self.put(relative, value)
        self.put('python/crew_runtime/runtime-bundle-manifest.json', json.dumps({
            'schemaVersion': 1, 'python': {'name':'CPython','version':'3.12.10','archive':self.archive_claim('python/windows.zip','src-tauri/resources/python/windows.zip')},
            'wheelhouse': {'requirementsSha256':sha(self.data['python/crew_runtime/requirements.txt']),
                          'lockSha256':sha(self.data['python/crew_runtime/requirements.lock']),
                          'archive': self.archive_claim('python/crew_runtime/wheels.zip','src-tauri/python/crew_runtime/wheels.zip')},
            'packages': [{'name':'synthetic','version':'1.0','filename':'synthetic.whl','license':'MIT','purl':'pkg:pypi/synthetic@1.0','sha256':'0'*64}],
            'credentials': 'DO_NOT_EXPORT_THIS_SYNTHETIC_MARKER',
        }).encode())
        self.put('codex/runtime-bundle-manifest.json', json.dumps({
            'version':'0.147.0','protocolSchema':'app-server-0.147.0','target':'windows-x64',
            'package':'@openai/codex@0.147.0-win32-x64','archiveSha256':'1'*64,
            'binary':'vendor/x86_64-pc-windows-msvc/bin/codex.exe',
            'sha256':sha(self.data['codex/vendor/x86_64-pc-windows-msvc/bin/codex.exe']),
            'binarySize':len(self.data['codex/vendor/x86_64-pc-windows-msvc/bin/codex.exe']),
            'license':'LICENSE','licenseSha256':sha(self.data['codex/LICENSE']),
        }).encode())
        self.put('daemon/windows-x64/manifest.json', json.dumps({
            'schemaVersion':2,'target':'windows-x64','binary':'cowork-local-daemon.exe',
            'sha256':sha(self.data['daemon/windows-x64/cowork-local-daemon.exe']),'version':'0.3.12',
            'files':[{'name':'pdfium.dll','sha256':sha(self.data['daemon/windows-x64/pdfium.dll'])}],
        }).encode())
        (self.open/'app.exe').write_bytes(pe_fixture((0,1,8,0)))
        # These unrelated files must never be selected, read, copied or enumerated.
        (self.local/'cowork.db').write_bytes(b'NOT_A_REAL_DATABASE_DO_NOT_READ')
        (self.local/'credentials.json').write_text('DO_NOT_EXPORT_THIS_SYNTHETIC_MARKER')
        (self.local/'User Data').mkdir(); (self.local/'User Data'/'Login Data').write_bytes(b'UNSELECTED')
        selected = {'app/package.json':json.dumps({'name':'localai-cowork','version':'0.3.12','irrelevant':'not_exported'}).encode(),
                    'app/src-tauri/tauri.conf.json':json.dumps({'productName':'Local AI Cowork','version':'0.3.12','identifier':'io.fixture'}).encode()}
        for relative in ('python/crew_runtime/main.py','python/crew_runtime/crew_tools.py','python/crew_runtime/requirements.txt','python/crew_runtime/requirements.lock'):
            selected[inv.SOURCE_MAP[relative]] = self.data[relative]
        self.archive = self.reference/'source.zip'
        with zipfile.ZipFile(self.archive,'w',compression=zipfile.ZIP_DEFLATED) as archive:
            for name, value in selected.items(): archive.writestr(name,value)
        self.archive_pin = sha(self.archive.read_bytes()); self.manifest = self.reference/'manifest.json'
        self.manifest.write_text(json.dumps({'schema':2,'archive_sha256':self.archive_pin,
                                           'files':[{'path':name,'size':len(value),'sha256':sha(value)} for name,value in selected.items()]}))
        self.manifest_pin = sha(self.manifest.read_bytes())
        self.registry = {name:{'hive':'HKCU','view':'64','subkey':'fixture/'+name,'last_write_100ns':1,
                               'fields':{'DisplayName':name,'DisplayVersion':'0.3.12' if name=='Local AI Cowork' else '0.1.8',
                                         'InstallLocation':str(root),'Publisher':'Fixture'}} for name,root in self.roots.items()}
    def put(self, relative, value):
        path = self.local/relative; path.parent.mkdir(parents=True,exist_ok=True); path.write_bytes(value); self.data[relative] = value
    def archive_claim(self, relative, name): return {'path':name,'bytes':len(self.data[relative]),'sha256':sha(self.data[relative])}
    def run(self, **kwargs):
        return inv.inventory(self.roots,self.manifest,self.manifest_pin,self.archive,self.archive_pin,
                             self.path/'output',registry_reader=lambda _:json.loads(json.dumps(self.registry)),**kwargs)


def junction(path, target):
    """Actual NTFS mount-point reparse fixture; no shell command or link privilege needed."""
    path.mkdir(); substitute = ('\\??\\'+str(target)).encode('utf-16le'); display = str(target).encode('utf-16le')
    strings = substitute+b'\0\0'+display+b'\0\0'
    payload = struct.pack('<HHHH',0,len(substitute),len(substitute)+2,len(display))+strings
    data = struct.pack('<IHH',0xA0000003,len(payload),0)+payload
    kernel = ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.CreateFileW.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
    kernel.CreateFileW.restype=wintypes.HANDLE; kernel.CloseHandle.argtypes=[wintypes.HANDLE]
    kernel.DeviceIoControl.argtypes=[wintypes.HANDLE,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,ctypes.POINTER(wintypes.DWORD),ctypes.c_void_p]
    handle=kernel.CreateFileW(str(path),0x40000000,0,None,3,0x02200000,None)
    if handle==ctypes.c_void_p(-1).value: raise ctypes.WinError(ctypes.get_last_error())
    try:
        returned=wintypes.DWORD(); buffer=ctypes.create_string_buffer(data)
        if not kernel.DeviceIoControl(handle,0x000900a4,buffer,len(data),None,0,ctypes.byref(returned),None): raise ctypes.WinError(ctypes.get_last_error())
    finally: kernel.CloseHandle(handle)


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.fixture=Fixture(EVIDENCE/sha(self._testMethodName.encode())[:10])
    def rejected(self, reason=None, **kwargs):
        with self.assertRaises(inv.Refused) as caught: self.fixture.run(**kwargs)
        if reason is not None: self.assertIn(reason,str(caught.exception))
        self.assertFalse((self.fixture.path/'output').exists())
        self.assertEqual([],list(self.fixture.path.glob('.output-*')))
    def test_inventory_roundtrip_without_execution_userdata_or_sqlite(self):
        fixture=self.fixture; expected={str(root/name) for product,root in fixture.roots.items() for name in inv.PRODUCT_FILES[product]}
        violations=[]
        def guard(event,args):
            if event in ('open','os.listdir','os.scandir') and args and isinstance(args[0],(str,bytes,os.PathLike)):
                path=Path(os.fsdecode(args[0]))
                if any(root==path or root in path.parents for root in fixture.roots.values()):
                    if event!='open' or str(path) not in expected:
                        violations.append((event,str(path))); raise AssertionError('Unselected installed path touched')
            if event=='subprocess.Popen': raise AssertionError('No process execution in inventory')
        # Audit hook is disabled once the local fixture call ends; unrelated later test IO is allowed.
        enabled=[True]
        sys.addaudithook(lambda event,args:guard(event,args) if enabled[0] else None)
        old=sqlite3.connect; sqlite3.connect=lambda *a,**k: (_ for _ in ()).throw(AssertionError('SQLite opened'))
        try: report=fixture.run()
        finally: enabled[0]=False; sqlite3.connect=old
        self.assertEqual([],violations); self.assertTrue(report['byte_and_physical_identity_receipts_equal'])
        encoded=(fixture.path/'output/inventory.json').read_text()
        self.assertNotIn('DO_NOT_EXPORT_THIS_SYNTHETIC_MARKER',encoded); self.assertNotIn('not_exported',encoded)
        self.assertFalse(report['m0_complete']); self.assertFalse(report['reference']['build_parity_proven'])
        local=report['products'][0]
        self.assertTrue(local['registry_matches_pe_product_version']); self.assertTrue(local['original_version_matches'])
        layout=next(i for i in local['artifacts'] if i['relative_path']=='runtime-layout.json'); self.assertEqual('missing',layout['status'])
        self.assertEqual(4,sum(i['original_reference']['status']=='match' for i in local['artifacts']))
        for product in report['products']:
            for item in product['artifacts']:
                if item['status']=='present': self.assertEqual(item['before'],item['after'])
    def test_real_native_write_and_rename_races_denied_then_reopened(self):
        results=[]
        def probe(fences,roots):
            def contender():
                for path in (roots['Local AI Cowork']/'app.exe',roots['Local AI Cowork']/'python/crew_runtime/main.py'):
                    try:
                        with open(path,'r+b') as stream: stream.write(b'X')
                    except PermissionError: results.append('write_denied')
                    else: results.append('WRITE_SUCCEEDED')
                    try: path.rename(Path(str(path)+'.renamed'))
                    except PermissionError: results.append('rename_denied')
                    else: results.append('RENAME_SUCCEEDED')
                for path in (roots['Local AI Cowork'],roots['Local AI Cowork']/'python/crew_runtime'):
                    try: path.rename(Path(str(path)+'.renamed'))
                    except PermissionError: results.append('directory_rename_denied')
                    else: results.append('DIRECTORY_RENAME_SUCCEEDED')
            thread=threading.Thread(target=contender); thread.start(); thread.join(timeout=10)
            self.assertFalse(thread.is_alive())
        self.fixture.run(probe=probe)
        self.assertEqual(['write_denied','rename_denied','write_denied','rename_denied','directory_rename_denied','directory_rename_denied'],results)
        path=self.fixture.local/'app.exe'
        with open(path,'r+b') as stream: self.assertEqual(b'MZ',stream.read(2))
        moved=Path(str(path)+'.after'); path.rename(moved); moved.rename(path)
    def test_preexisting_native_writer_refuses_fence(self):
        with open(self.fixture.local/'app.exe','r+b'): self.rejected('deny-write/delete')
    def test_preexisting_native_delete_handle_refuses_fence(self):
        kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        kernel.CreateFileW.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,ctypes.c_void_p,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
        kernel.CreateFileW.restype=wintypes.HANDLE; kernel.CloseHandle.argtypes=[wintypes.HANDLE]
        handle=kernel.CreateFileW(str(self.fixture.local/'app.exe'),0x00010000,7,None,3,0x00200000,None)
        if handle==ctypes.c_void_p(-1).value: raise ctypes.WinError(ctypes.get_last_error())
        try:self.rejected('deny-write/delete')
        finally:kernel.CloseHandle(handle)
    def test_hardlink_rejected(self):
        os.link(self.fixture.local/'app.exe',self.fixture.path/'same-app.exe'); self.rejected('hard links')
    def test_root_reparse_rejected(self):
        link=self.fixture.path/'root-junction'; junction(link,self.fixture.local)
        try:
            self.fixture.roots['Local AI Cowork']=link; self.rejected('reparse points')
        finally: os.rmdir(link)
    def test_artifact_parent_reparse_rejected(self):
        original=self.fixture.local/'python'; moved=self.fixture.path/'moved-python'; original.rename(moved); junction(original,moved)
        try: self.rejected('reparse point')
        finally: os.rmdir(original); moved.rename(original)
    def test_wrong_installed_manifest_hash_rejected(self):
        path=self.fixture.local/'daemon/windows-x64/manifest.json'; value=json.loads(path.read_text()); value['sha256']='0'*64; path.write_text(json.dumps(value)); self.rejected('hash/size claim mismatch')
    def test_wrong_manifest_path_cannot_select_userdata(self):
        path=self.fixture.local/'codex/runtime-bundle-manifest.json'; value=json.loads(path.read_text()); value['binary']='../../credentials.json'; path.write_text(json.dumps(value)); self.rejected('Unknown Codex bundle layout')
    def test_unknown_package_schema_rejected(self):
        path=self.fixture.local/'python/crew_runtime/runtime-bundle-manifest.json'; value=json.loads(path.read_text()); value['schemaVersion']=2; path.write_text(json.dumps(value)); self.rejected('Unknown Python bundle schema')
    def test_duplicate_json_key_rejected(self):
        self.fixture.put('daemon/windows-x64/manifest.json',b'{"schemaVersion":2,"schemaVersion":2}'); self.rejected('Duplicate JSON key')
    def test_original_manifest_pin_rejected(self):
        self.fixture.manifest_pin='0'*64; self.rejected('SHA pin mismatch')
    def test_original_archive_pin_rejected(self):
        self.fixture.archive_pin='0'*64; self.rejected('SHA pin mismatch')
    def test_original_selected_member_hash_rejected(self):
        value=json.loads(self.fixture.manifest.read_text()); value['files'][0]['sha256']='0'*64
        self.fixture.manifest.write_text(json.dumps(value)); self.fixture.manifest_pin=sha(self.fixture.manifest.read_bytes()); self.rejected('member hash mismatch')
    def test_changed_original_script_is_reported_without_claiming_build_parity(self):
        self.fixture.put('python/crew_runtime/main.py',b'changed nonexecuted source')
        report=self.fixture.run(); item=next(i for i in report['products'][0]['artifacts'] if i['relative_path']=='python/crew_runtime/main.py')
        self.assertEqual('different_or_missing',item['original_reference']['status']); self.assertFalse(report['products'][0]['build_parity_proven'])
    def test_expected_missing_path_creation_race_rejected(self):
        self.rejected('presence/namespace changed',probe=lambda _,roots:(roots['Local AI Cowork']/'runtime-layout.json').write_text('{}'))
    def test_registry_drift_rejected(self):
        self.rejected('uninstall metadata changed',probe=lambda *_:self.fixture.registry['Local AI Cowork']['fields'].update(DisplayVersion='9.9.9'))
    def test_registry_pe_version_mismatch_reported(self):
        self.fixture.registry['Local AI Cowork']['fields']['DisplayVersion']='9.9.9'; report=self.fixture.run()
        self.assertFalse(report['products'][0]['registry_matches_pe_product_version']); self.assertFalse(report['products'][0]['original_version_matches'])
    def test_invalid_pe_rejected(self):
        self.fixture.put('app.exe',b'not PE'); self.rejected('Truncated PE artifact')
    def test_existing_output_exclusive(self):
        output=self.fixture.path/'output'; output.mkdir(); (output/'sentinel').write_text('keep')
        with self.assertRaises(inv.Refused): self.fixture.run()
        self.assertEqual('keep',(output/'sentinel').read_text())
    def test_publication_race_preserves_existing_output_and_cleans_owned_stage(self):
        output=self.fixture.path/'output'
        def contender(*_):
            output.mkdir(); (output/'sentinel').write_text('keep')
        with self.assertRaises(OSError): self.fixture.run(probe=contender)
        self.assertEqual('keep',(output/'sentinel').read_text()); self.assertFalse((output/'inventory.json').exists())
        self.assertEqual([],list(self.fixture.path.glob('.output-*')))
    def test_output_overlaps_installation_rejected(self):
        with self.assertRaises(inv.Refused):
            inv.inventory(self.fixture.roots,self.fixture.manifest,self.fixture.manifest_pin,self.fixture.archive,self.fixture.archive_pin,self.fixture.local/'output',registry_reader=lambda _:self.fixture.registry)
        self.assertFalse((self.fixture.local/'output').exists())
    def test_linux_fail_closed_before_selected_path_access(self):
        previous=inv.os.name; inv.os.name='posix'
        try:
            with self.assertRaisesRegex(inv.Refused,'Windows native'):
                inv.inventory({},'NOT_OPENED','0'*64,'NOT_OPENED','0'*64,'NOT_CREATED')
        finally: inv.os.name=previous
    def test_native_cli_parse_and_actual_registry_refusal_exit(self):
        args=[sys.executable,'-I','-B','-S',str(Path(inv.__file__)),
              '--local-ai-cowork-root',str(self.fixture.local),'--source-manifest',str(self.fixture.manifest),
              '--source-manifest-sha256',self.fixture.manifest_pin,'--source-archive',str(self.fixture.archive),
              '--source-archive-sha256',self.fixture.archive_pin,'--output',str(self.fixture.path/'cli-output')]
        child=subprocess.Popen(args,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
        stdout,stderr=child.communicate(timeout=30); self.assertEqual(2,child.wait(timeout=1)); self.assertIn(b'REFUSED',stderr)
        self.assertFalse((self.fixture.path/'cli-output').exists())


def main():
    global EVIDENCE
    parser=argparse.ArgumentParser(); parser.add_argument('--evidence-dir',required=True); args=parser.parse_args()
    EVIDENCE=Path(args.evidence_dir); EVIDENCE.mkdir(exist_ok=False)
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(InventoryTests)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    report={'success':result.wasSuccessful(),'cases':result.testsRun,'failures':len(result.failures),'errors':len(result.errors),
            'real_windows_fixture_handles':True,'installed_programs_executed':False,'m0_complete':False,
            'test_source_sha256':sha(Path(__file__).read_bytes()),'inventory_source_sha256':sha(Path(inv.__file__).read_bytes())}
    (EVIDENCE/'qualification.json').write_text(json.dumps(report,sort_keys=True)+'\n')
    return 0 if result.wasSuccessful() else 1


if __name__=='__main__':raise SystemExit(main())
