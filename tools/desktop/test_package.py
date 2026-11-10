"""Preparation counterexamples. Synthetic fixtures do not qualify installers."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import package
import release

HOST = "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"


def executable(host=HOST, installer=False):
    raw = bytearray(2048)
    if host.endswith("msvc"):
        raw[:2] = b"MZ"; struct.pack_into("<I", raw, 60, 128); raw[128:132] = b"PE\0\0"
        struct.pack_into("<H", raw, 132, 0x14c if installer else 0x8664)
        struct.pack_into("<H", raw, 150, 2); struct.pack_into("<H", raw, 152, 0x10b if installer else 0x20b)
    else:
        raw[:6] = b"\x7fELF\x02\x01"; struct.pack_into("<H", raw, 16, 3); struct.pack_into("<H", raw, 18, 62)
        raw[8:11] = b"AI\x02"
    return bytes(raw)


class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.external=tempfile.TemporaryDirectory();self.addCleanup(self.external.cleanup)
        original_mkdtemp=tempfile.mkdtemp
        def external_context(*args,**kwargs):
            if kwargs.get('dir') is None:kwargs['dir']=self.external.name
            return original_mkdtemp(*args,**kwargs)
        self.context_patch=patch.object(package.tempfile,'mkdtemp',side_effect=external_context)
        self.root = Path(self.temp.name)
        for folder in ("crates", "services/daemon/src", "tools/contracts", "contracts", "extensions/browser", "tools/desktop", "tools/release", "deploy/v2", "docs", "apps/desktop/src-tauri/src", "apps/web/dist","packages/ui/src"):
            (self.root / folder).mkdir(parents=True)
        for name, raw in {"Cargo.toml": b'[workspace.package]\nversion="2.0.0-alpha.1"\n', "Cargo.lock":b"fixture", "rust-toolchain.toml":b"fixture",
                          "services/daemon/src/main.rs":b"fn main() {}", "apps/desktop/src-tauri/src/main.rs":b"fn main() {}",
                          "apps/web/dist/index.html":b"<html>fixture</html>", "tools/desktop/release.py":b"fixture",
                          "packages/ui/src/secretMessages.json":b'{}', "extensions/browser/identity.json":b'{}',
                          "tools/desktop/package.py":b"fixture", "tools/desktop/test_package.py":b"fixture"}.items():
            (self.root / name).write_bytes(raw)
        for name in ("sbom.py", "runtime_inventory.py", "container_inventory.py", "source_bundle.py", "delivery.py"):
            (self.root / "tools/release" / name).write_bytes(b"fixture")
        for name in ("tools/private_crew_setup.py", "docs/DEVICE-AGENT-V2.md", "tools/desktop/signing.py"):
            (self.root / name).write_bytes(b"fixture")
        (self.root / "apps/desktop/src-tauri/tauri.conf.json").write_text(json.dumps({"build":{"frontendDist":"../../web/dist"},"bundle":{"active":True,"externalBin":["binaries/cowork-daemon","binaries/cowork-browser-host"]}}))
        dev = {"@tauri-apps/cli":package.CLI_VERSION}
        (self.root / "package.json").write_text(json.dumps({"devDependencies":dev}))
        native = "cli-win32-x64-msvc" if HOST.endswith("msvc") else "cli-linux-x64-gnu"
        packages = {"":{"devDependencies":dev}}
        for name in ("cli", native):
            folder = self.root / "node_modules/@tauri-apps" / name; folder.mkdir(parents=True)
            (folder / "package.json").write_text(json.dumps({"name":"@tauri-apps/"+name,"version":package.CLI_VERSION}))
            (folder / ("tauri.js" if name == "cli" else "fixture.node")).write_bytes(b"fixture")
            packages["node_modules/@tauri-apps/"+name] = {"version":package.CLI_VERSION,"integrity":"sha512-fixture"}
        (self.root / "package-lock.json").write_text(json.dumps({"packages":packages}))
        self.prepared = self.root / "prepared"; self.prepared.mkdir()
        self.sidecar_name = "cowork-daemon-"+HOST+(".exe" if HOST.endswith("msvc") else "")
        (self.prepared / self.sidecar_name).write_bytes(executable())
        self.item = {"reason":"compiler-artifact", "package_id":"owned-daemon", "target":{"name":"cowork-daemon","kind":["bin"],"src_path":str(self.root / "services/daemon/src/main.rs")},
                     "executable":str(self.root / "retired-cache" / HOST / "release" / ("cowork-daemon.exe" if HOST.endswith("msvc") else "cowork-daemon")),
                     "profile":{"opt_level":"3","debug_assertions":False,"overflow_checks":False,"test":False}}
        self.receipt = {"format":"localai-cowork-native-release-sidecar/1","host":HOST,"profile":"release","cargo_exit_code":0,"source_before_after_equal":True,
                        "signed":False,"installed":False,"sources":release.source_manifest(self.root),"artifact":{"filename":self.sidecar_name,**package.pin(self.prepared / self.sidecar_name)},
                        "package_id":"owned-daemon","package_version":"2.0.0-alpha.1","cargo_reported_artifact":self.item['executable']}
        self.save_receipt()

    def save_receipt(self, items=None, success=True):
        raw = '\n'.join(json.dumps(item) for item in [*(items or [self.item]),{"reason":"build-finished","success":success}]).encode()
        (self.prepared / "cargo-build.jsonl").write_bytes(raw)
        self.receipt['cargo_json_sha256'] = hashlib.sha256(raw).hexdigest()
        (self.prepared / "receipt.json").write_text(json.dumps(self.receipt))

    def checked(self): return package.checked_sidecar(self.root,self.prepared,HOST)

    def frozen_sources(self):
        # Overlay only non-core packaging tools, just as a reviewed relocation
        # does. The pinned release helper is also the exact executing version.
        for name, source in (("package.py",package.__file__), ("test_package.py",__file__), ("release.py",release.__file__), ("signing.py",package.signing.__file__)):
            (self.root / "tools/desktop" / name).write_bytes(Path(source).read_bytes())
        self.receipt["sources"]=release.source_manifest(self.root)
        self.save_receipt()
        snapshot=Path(self.external.name)/"frozen-source"
        shutil.copytree(self.root,snapshot)
        return snapshot

    def test_frozen_origin_relocation_retains_exact_sidecar_and_original_cargo_evidence(self):
        snapshot=self.frozen_sources()
        original_receipt=(self.prepared/"receipt.json").read_bytes()
        original_cargo=(self.prepared/"cargo-build.jsonl").read_bytes()
        root,origin,tools=package.checked_source_roots(self.root,snapshot,self.root)
        self.assertEqual(root,snapshot);self.assertEqual(origin,self.root)
        self.assertEqual(tools["tools/desktop/package.py"],package.pin(Path(package.__file__)))
        accepted,binary,_=package.checked_sidecar(root,self.prepared,HOST,origin)
        self.assertEqual(accepted,self.receipt);self.assertEqual(binary,self.prepared/self.sidecar_name)
        # Ongoing edits in the original workspace cannot alter the frozen build.
        (self.root/"services/daemon/src/main.rs").write_text("parallel workspace edit")
        self.assertEqual(package.checked_sidecar(root,self.prepared,HOST,origin)[0],self.receipt)
        self.assertEqual((self.prepared/"receipt.json").read_bytes(),original_receipt)
        self.assertEqual((self.prepared/"cargo-build.jsonl").read_bytes(),original_cargo)

    def test_frozen_copy_or_original_absolute_build_root_cannot_be_substituted(self):
        snapshot=self.frozen_sources()
        with self.assertRaises(package.Refused):package.checked_sidecar(snapshot,self.prepared,HOST)
        with self.assertRaises(package.Refused):package.checked_sidecar(snapshot,self.prepared,HOST,snapshot)
        (snapshot/"services/daemon/src/main.rs").write_text("different frozen compiler input")
        with self.assertRaisesRegex(package.Refused,"core inputs differ"):package.checked_sidecar(snapshot,self.prepared,HOST,self.root)

    def test_frozen_tree_requires_explicit_origin_and_exact_executing_packaging_sources(self):
        snapshot=self.frozen_sources()
        for source,origin in ((snapshot,None),(None,self.root)):
            with self.assertRaises(package.Refused):package.checked_source_roots(self.root,source,origin)
        (snapshot/"tools/desktop/package.py").write_bytes(b"different executing tool")
        with self.assertRaisesRegex(package.Refused,"helper differs"):package.checked_source_roots(self.root,snapshot,self.root)

    def test_immutable_sidecar_survives_retirement_of_original_build_cache(self):
        self.assertFalse((self.root / 'retired-cache').exists())
        self.assertEqual(self.checked()[1],self.prepared / self.sidecar_name)

    def test_changed_core_or_sidecar_bytes_cannot_supply_an_installer(self):
        (self.root / 'services/daemon/src/main.rs').write_text('changed')
        with self.assertRaises(package.Refused): self.checked()
        (self.root / 'services/daemon/src/main.rs').write_text('fn main() {}')
        (self.prepared / self.sidecar_name).write_bytes(executable()+b'changed')
        with self.assertRaises(package.Refused): self.checked()

    def test_failed_ambiguous_foreign_or_debug_cargo_receipt_is_rejected(self):
        for mode in ('failed','ambiguous','foreign','debug'):
            items = [json.loads(json.dumps(self.item))]
            if mode == 'ambiguous':items.append(items[0])
            if mode == 'foreign':items[0]['package_id']='foreign'
            if mode == 'debug':items[0]['profile']['debug_assertions']=True
            self.save_receipt(items,mode!='failed')
            with self.assertRaises(package.Refused): self.checked()

    def test_cargo_evidence_and_nested_duplicate_json_fields_fail_closed(self):
        (self.prepared / 'cargo-build.jsonl').write_text('changed')
        with self.assertRaises(package.Refused):self.checked()
        with self.assertRaises(package.Refused):package.strict_json(b'{"artifact":{"size":1,"size":2}}')

    def test_receipt_filename_and_host_cannot_select_foreign_resources(self):
        self.receipt['artifact']['filename']='../outside.exe';self.save_receipt()
        with self.assertRaises(package.Refused):self.checked()
        self.receipt['artifact']['filename']=self.sidecar_name;self.receipt['host']='aarch64-unknown-linux-gnu';self.save_receipt()
        with self.assertRaises(package.Refused):self.checked()

    def test_cli_lock_version_and_actual_addon_are_required(self):
        package.checked_cli(self.root,HOST)
        data=json.loads((self.root/'package-lock.json').read_bytes());data['packages']['node_modules/@tauri-apps/cli']['version']='2.11.3'
        (self.root/'package-lock.json').write_text(json.dumps(data))
        with self.assertRaises(package.Refused):package.checked_cli(self.root,HOST)

    def test_frontend_dist_and_include_bytes_fixtures_are_bound(self):
        before=package.input_manifest(self.root)
        (self.root/'apps/web/dist/index.html').write_text('changed')
        self.assertNotEqual(package.input_manifest(self.root),before)
        fixture=self.root/'crates/embedded.sqlite3';fixture.write_bytes(b'fixture')
        self.assertIn('crates/embedded.sqlite3',package.input_manifest(self.root))
        self.assertIn('packages/ui/src/secretMessages.json',package.input_manifest(self.root))
        before=package.input_manifest(self.root)
        (self.root/'packages/ui/src/secretMessages.json').write_bytes(b'{"changed":"native Rust include_str input"}')
        self.assertNotEqual(package.input_manifest(self.root),before)

    def desktop_messages(self, target, context, profile=None, success=True):
        binary=target/HOST/'release'/(package.DESKTOP+('.exe' if HOST.endswith('msvc') else ''))
        binary.parent.mkdir(parents=True,exist_ok=True);binary.write_bytes(executable())
        item={'reason':'compiler-artifact','target':{'name':package.DESKTOP,'kind':['bin'],'src_path':str(context/'apps/desktop/src-tauri/src/main.rs')},'executable':str(binary),
              'profile':profile or {'opt_level':'3','debug_assertions':False,'test':False,'overflow_checks':False}}
        return '\n'.join(json.dumps(x) for x in [item,{'reason':'build-finished','success':success}])

    def test_desktop_requires_reported_successful_native_release_not_stale_file(self):
        target=self.root/'target';context=self.root/'context'
        raw=self.desktop_messages(target,context)
        package.desktop_artifact(raw,context,target,HOST)
        with self.assertRaises(package.Refused):package.desktop_artifact(self.desktop_messages(target,context,success=False),context,target,HOST)
        with self.assertRaises(package.Refused):package.desktop_artifact(raw,context,self.root/'foreign-target',HOST)
        with self.assertRaises(package.Refused):package.desktop_artifact(raw,context/'foreign',target,HOST)

    def test_native_bundle_architecture_is_checked_with_nsis_stub_distinction(self):
        artifact=self.root/'bundle.exe';artifact.write_bytes(executable('x86_64-pc-windows-msvc',installer=True))
        package.validate_bundle(artifact,'nsis','x86_64-pc-windows-msvc')
        bad=bytearray(artifact.read_bytes());struct.pack_into('<H',bad,132,0xaa64);artifact.write_bytes(bad)
        with self.assertRaises(package.Refused):package.validate_bundle(artifact,'nsis','x86_64-pc-windows-msvc')
        image=self.root/'bundle.AppImage';image.write_bytes(executable('x86_64-unknown-linux-gnu'))
        package.validate_bundle(image,'appimage','x86_64-unknown-linux-gnu')
        bad=bytearray(image.read_bytes());bad[8:11]=b'\0\0\0';image.write_bytes(bad)
        with self.assertRaises(package.Refused):package.validate_bundle(image,'appimage','x86_64-unknown-linux-gnu')

    def test_ambiguous_bundle_outputs_and_existing_publication_fail_closed(self):
        base=self.root/'fresh-bundle';kind='nsis' if HOST.endswith('msvc') else 'appimage'
        folder=base/kind;folder.mkdir(parents=True)
        extension='.exe' if kind=='nsis' else '.AppImage'
        for name in ('one','two'):(folder/(name+extension)).write_bytes(executable(installer=kind=='nsis'))
        with self.assertRaises(package.Refused):package.collect_bundles(base,[kind],HOST)
        staged=self.root/'publication';staged.mkdir();(staged/'receipt.json').write_text('fresh')
        output=self.root/'existing';output.mkdir();(output/'receipt.json').write_text('preserved')
        with self.assertRaises(OSError):package.publish(staged,output)
        self.assertEqual((output/'receipt.json').read_text(),'preserved')

    def test_addon_injection_is_refused_before_new_context_or_command(self):
        with patch.dict(os.environ,{'NAPI_RS_NATIVE_LIBRARY_PATH':'unqualified'}):
            with self.assertRaises(package.Refused):package.prepare(self.root,self.prepared,self.root/'new-output',self.root/'cache',['nsis' if HOST.endswith('msvc') else 'deb'])
        self.assertFalse((self.root/'new-output').exists())
        self.assertFalse(list(self.root.glob('.desktop-package-context-*')))

    def test_actual_native_command_has_positive_wait_and_private_process_scope(self):
        scope=package.ProcessScope()
        try:
            result=scope.run([sys.executable,'-I','-B','-S','-c','print("private native command")'],self.root,os.environ.copy(),self.root/'stdout.log',self.root/'stderr.log')
            self.assertEqual(result.strip(),'private native command')
            self.assertEqual(scope.records[0]['exit_code'],0)
            self.assertTrue(scope.records[0]['retained_popen_wait'])
            record=scope.close(True)
            if os.name=='nt':self.assertTrue(record['own_job_empty']);self.assertFalse(record['forced_cleanup'])
            else:self.assertTrue(scope.records[0]['retained_pidfd_completion']);self.assertTrue(scope.records[0]['own_session_empty'])
        finally:
            if scope.job is not None:scope.close(False)

    def preparation_scope(self, drift=None):
        owner=self
        class FixtureScope:
            def __init__(self):self.records=[]
            def run(self, command, cwd, env, stdout, stderr):
                stderr.write_bytes(b'');result=''
                if command[0]=='rustc':result='host: '+HOST+'\n'
                elif '--version' in command:result='tauri-cli '+package.CLI_VERSION
                elif 'build' in command:
                    target=Path(env['CARGO_TARGET_DIR']);context=cwd.parents[2]
                    result=owner.desktop_messages(target,context)
                    if drift=='failed-build':result=result.replace('"success": true','"success": false')
                    if drift=='missing-native-leaf':(context/'packages/ui/src/secretMessages.json').unlink()
                elif 'bundle' in command:
                    kind='nsis' if HOST.endswith('msvc') else 'appimage'
                    folder=Path(env['CARGO_TARGET_DIR'])/HOST/'release/bundle'/kind;folder.mkdir(parents=True)
                    (folder/('fixture.exe' if kind=='nsis' else 'fixture.AppImage')).write_bytes(executable(installer=kind=='nsis'))
                    if drift=='frontend':(owner.root/'apps/web/dist/index.html').write_text('changed during bundling')
                    if drift=='addon':(owner.root/'node_modules/@tauri-apps/cli/tauri.js').write_text('changed during bundling')
                    if drift=='staged':(cwd/'src/main.rs').write_text('CLI changed staged code')
                    if drift=='native-leaf':(owner.root/'packages/ui/src/secretMessages.json').write_text('{"changed":"during native build"}')
                    if drift=='bundle-payload':
                        payload=folder.parents[1]/(package.DESKTOP+('.exe' if HOST.endswith('msvc') else ''))
                        raw=payload.read_bytes();payload.write_bytes(raw[:-1]+bytes([raw[-1]^1]))
                stdout.write_text(result);self.records.append({'command':command,'exit_code':0});return result
            def close(self, success):return {'fixture_scope_only':True,'successful':success}
        return FixtureScope

    @unittest.skipIf(os.name=='nt','native Linux session/PIDfd counterexample')
    def test_native_surviving_descendant_is_stopped_and_cannot_report_success(self):
        scope=package.ProcessScope()
        code='import subprocess,sys;p=subprocess.Popen([sys.executable,"-I","-B","-S","-c","import time;time.sleep(60)"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);print(p.pid,flush=True)'
        with self.assertRaises(package.Refused):scope.run([sys.executable,'-I','-B','-S','-c',code],self.root,os.environ.copy(),self.root/'descendant.log',self.root/'descendant-error.log')
        pid=int((self.root/'descendant.log').read_text().strip())
        try:fd=os.pidfd_open(pid)
        except ProcessLookupError:fd=None
        if fd is not None:
            import select
            try:
                wait=select.poll();wait.register(fd,select.POLLIN);self.assertTrue(wait.poll(1000))
            finally:os.close(fd)
        self.assertFalse(scope.records)
        scope.close(False)

    def test_failed_build_with_stale_native_artifact_never_publishes(self):
        output=self.root/'no-publication';target=self.root/'cache'
        with self.context_patch,patch.object(package,'ProcessScope',self.preparation_scope('failed-build')):
            with self.assertRaises(package.Refused):package.prepare(self.root,self.prepared,output,target,['nsis' if HOST.endswith('msvc') else 'appimage'],node=Path(sys.executable))
        self.assertFalse(output.exists())
        attempt=json.loads(next(Path(self.external.name).glob('cowork-desktop-package-context-*/attempt.json')).read_bytes())
        self.assertEqual(attempt['result'],'failed')

    def test_actual_cargo_standalone_metadata_has_no_outer_workspace(self):
        (self.root/'Cargo.toml').write_text('[workspace]\nmembers=[]\nexclude=["apps/desktop/src-tauri"]\n')
        with self.context_patch:context=package.source_context()
        source=context/'source';app=source/'apps/desktop/src-tauri';(app/'src').mkdir(parents=True)
        (source/'Cargo.toml').write_bytes((self.root/'Cargo.toml').read_bytes())
        (app/'Cargo.toml').write_text('[package]\nname="fixture-desktop"\nversion="0.1.0"\nedition="2024"\n')
        (app/'src/main.rs').write_text('fn main() {}\n')
        scope=package.ProcessScope()
        try:
            cargo=Path.home()/'.cargo/bin'/('cargo.exe' if os.name=='nt' else 'cargo')
            raw=scope.run([str(cargo),'metadata','--offline','--no-deps','--format-version','1','--manifest-path',str(app/'Cargo.toml')],app,os.environ.copy(),context/'metadata.log',context/'metadata-error.log')
            self.assertEqual(Path(json.loads(raw)['workspace_root']),app)
            self.assertTrue(scope.records[0]['retained_popen_wait']);scope.close(True)
        finally:
            if scope.job is not None:scope.close(False)

    def test_late_frontend_addon_or_staged_source_drift_never_publishes(self):
        original_dist=(self.root/'apps/web/dist/index.html').read_bytes()
        original_addon=(self.root/'node_modules/@tauri-apps/cli/tauri.js').read_bytes()
        original_leaf=(self.root/'packages/ui/src/secretMessages.json').read_bytes()
        for drift in ('frontend','addon','staged','native-leaf','missing-native-leaf','bundle-payload'):
            with self.subTest(drift=drift),self.context_patch,patch.object(package,'ProcessScope',self.preparation_scope(drift)):
                output=self.root/('refused-'+drift)
                with self.assertRaises((package.Refused,FileNotFoundError)):package.prepare(self.root,self.prepared,output,self.root/'cache',['nsis' if HOST.endswith('msvc') else 'appimage'],node=Path(sys.executable))
                self.assertFalse(output.exists())
            (self.root/'apps/web/dist/index.html').write_bytes(original_dist)
            (self.root/'node_modules/@tauri-apps/cli/tauri.js').write_bytes(original_addon)
            (self.root/'packages/ui/src/secretMessages.json').write_bytes(original_leaf)

    def test_bundle_copy_only_allows_unique_selected_metadata_marker(self):
        original=self.root/'native-original';copied=self.root/'bundle-copy'
        marker=b'__TAURI_BUNDLE_TYPE_VAR_UNK';raw=executable()+marker
        original.write_bytes(raw);copied.write_bytes(raw)
        self.assertEqual(package.checked_bundle_copy(original,copied,['deb'])['marker'],'unchanged')
        for kind,code in [('nsis',b'NSS'),('deb',b'DEB'),('appimage',b'APP')]:
            transformed=raw.replace(marker,b'__TAURI_BUNDLE_TYPE_VAR_'+code)
            copied.write_bytes(transformed)
            self.assertEqual(package.checked_bundle_copy(original,copied,[kind])['marker'],code.decode())
            with self.assertRaises(package.Refused):package.checked_bundle_copy(original,copied,['deb' if kind!='deb' else 'nsis'])
            copied.write_bytes(transformed[:-1]+bytes([transformed[-1]^1]))
            with self.assertRaises(package.Refused):package.checked_bundle_copy(original,copied,[kind])
        original.write_bytes(raw+marker);copied.write_bytes((raw+marker).replace(marker,b'__TAURI_BUNDLE_TYPE_VAR_APP',1))
        with self.assertRaises(package.Refused):package.checked_bundle_copy(original,copied,['appimage'])


if __name__ == '__main__':unittest.main()
