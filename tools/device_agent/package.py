"""Package already-built standalone V2 agents; never build, install or publish."""
import argparse, hashlib, importlib.util, json, pathlib, shutil, tarfile, zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def ordinary(path):
    for part in [path, *path.parents]:
        metadata = part.lstat()
        if part.is_symlink() or getattr(metadata, 'st_file_attributes', 0) & 0x400:
            raise ValueError('Package inputs cannot contain aliases')
    if not path.is_file() or path.stat().st_nlink != 1:
        raise ValueError('Package inputs must be ordinary single-link files')

def copy(source, target):
    ordinary(source)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)

def dump(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False)+'\n', encoding='utf-8')

def public_build(receipt, original):
    keys=('format','result','prepared_at','host','package_version','cargo_exit_code',
          'cargo_reported_success','artifact','sources','cargo_json_sha256',
          'source_before_after_equal','validation_scope','tests_run','installed',
          'signed','published','native_process_tree_qualified','runtime_qualified','release_qualified')
    return {**{key:receipt[key] for key in keys if key in receipt},
            'original_receipt_sha256':sha(original),'public_receipt':True}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('windows','linux','output','windows-receipt','linux-receipt'):
        parser.add_argument('--'+name, type=pathlib.Path, required=True)
    parser.add_argument('--source-before',type=pathlib.Path,help='Original compiler input manifest; optional for the current source-bound build receipts')
    parser.add_argument('--demo-receipt',type=pathlib.Path)
    parser.add_argument('--build-only',action='store_true',help='Package source-bound native builds without a demo; record build-only validation')
    args=parser.parse_args()
    if args.build_only and args.demo_receipt:raise ValueError('Select build-only or the original demo receipt')
    if not args.build_only and not args.demo_receipt:raise ValueError('The original demo receipt is required unless build-only is explicit')
    if not args.build_only and not args.source_before:raise ValueError('The original compiler input manifest is required for the legacy receipt format')
    for path in (args.windows,args.linux,args.windows_receipt,args.linux_receipt): ordinary(path)
    if args.source_before:ordinary(args.source_before)
    if args.demo_receipt:ordinary(args.demo_receipt)
    if args.output.exists() or not args.output.is_absolute():raise ValueError('Select a fresh absolute output directory')
    archive=args.output.with_name(args.output.name+'.zip')
    if archive.exists() or archive.is_symlink():raise ValueError('Package archive already exists')
    if args.windows.read_bytes()[:2]!=b'MZ' or args.linux.read_bytes()[:4]!=b'\x7fELF':raise ValueError('Supply actual native Windows/Linux binaries')
    spec=importlib.util.spec_from_file_location('v2_source_inventory',ROOT/'tools/desktop/release.py')
    release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)
    sources=release.source_manifest(ROOT)
    if args.source_before and sources!=json.loads(args.source_before.read_text(encoding='utf-8')):raise ValueError('Compiler inputs changed after final build admission')
    windows=json.loads(args.windows_receipt.read_text(encoding='utf-8'));linux=json.loads(args.linux_receipt.read_text(encoding='utf-8'))
    original_receipts={str(path):sha(path) for path in (args.windows_receipt,args.linux_receipt)}
    original_binaries={str(path):{'size':path.stat().st_size,'sha256':sha(path)} for path in (args.windows,args.linux)}
    demo=json.loads(args.demo_receipt.read_text(encoding='utf-8')) if args.demo_receipt else None
    if args.build_only:
        versions=[]
        for receipt,binary,host in [(windows,args.windows,'x86_64-pc-windows-msvc'),(linux,args.linux,'x86_64-unknown-linux-gnu')]:
            artifact=receipt.get('artifact',{})
            if (receipt.get('format')!='localai-cowork-native-device-agent-release/1' or receipt.get('result')!='passed'
                    or receipt.get('host')!=host or receipt.get('cargo_exit_code')!=0 or receipt.get('cargo_reported_success') is not True
                    or receipt.get('validation_scope')!='build_only' or receipt.get('source_before_after_equal') is not True
                    or receipt.get('sources')!=sources or artifact.get('filename')!=binary.name
                    or artifact.get('sha256')!=sha(binary) or artifact.get('size')!=binary.stat().st_size):
                raise ValueError('The exact native agent bytes and current original build receipt are required')
            release.validate_executable(binary,host);versions.append(receipt['package_version'])
        if len(set(versions))!=1:raise ValueError('Native agent versions differ')
        version=versions[0];validation_scope='build_only'
    else:
        if any(receipt['result']!='passed' for receipt in (windows,linux,demo)):raise ValueError('Build and isolated demonstration must have completed')
        if windows.get('forced_cleanup') or linux.get('forced_cleanup') or not windows.get('own_job_empty_before_close') or not linux.get('own_descendants_empty'):raise ValueError('Owned builders are not naturally empty')
        if windows['binary_sha256']!=sha(args.windows) or demo['agent_sha256']!=sha(args.windows):raise ValueError('Windows artifact differs from build/demo bytes')
        if linux.get('binary_sha256')!=sha(args.linux):raise ValueError('Linux artifact differs from its original build receipt')
        if any((item.get('receipt') or {}).get('forced') or not item.get('receipt') for item in demo['processes']):raise ValueError('Demo did not finish its own processes naturally')
        version='2.0.0-alpha.5';validation_scope='native builds and one isolated device run'
    args.output.mkdir(parents=True)
    for platform,binary in [('Windows',args.windows),('Linux',args.linux)]:
        copy(binary,args.output/platform/binary.name)
        native_receipt=windows if platform=='Windows' else linux
        browser_receipt=native_receipt.get('browser_host')
        if not browser_receipt:raise ValueError('The current device-agent package requires its source-bound browser host')
        browser=binary.parent/browser_receipt['filename'];ordinary(browser)
        if sha(browser)!=browser_receipt['sha256'] or browser.stat().st_size!=browser_receipt['size']:raise ValueError('Browser host differs from its build receipt')
        copy(browser,args.output/platform/browser.name)

        if (sha(args.output/platform/binary.name)!=original_binaries[str(binary)]['sha256']
                or (args.output/platform/binary.name).stat().st_size!=original_binaries[str(binary)]['size']
                or sha(binary)!=original_binaries[str(binary)]['sha256']):
            raise ValueError('Original agent artifact changed during its package copy')
        sbom_spec=importlib.util.spec_from_file_location('cowork_agent_sbom',ROOT/'tools/release/sbom.py')
        sbom_module=importlib.util.module_from_spec(sbom_spec);sbom_spec.loader.exec_module(sbom_module)
        dump(args.output/platform/'sbom.spdx.json',sbom_module.generate(ROOT,[],payloads=[args.output/platform/binary.name,args.output/platform/browser.name],
                                                                      include_desktop=False,include_npm=False))
        for name in ('configure.py','windows_private.py','windows_sandbox.py','ui.py'):copy(ROOT/'tools/device_agent'/name,args.output/platform/name)
        copy(ROOT/'services/device-agent/src/device_crew_bridge.py',args.output/platform/'crew-runtime-probe.py')
        (args.output/platform/'README.md').write_text(
            '# LocalAI Cowork V2 Geräteagent – '+platform+'\n\n'
            'Gerät im V2-Team mit den tatsächlich eingerichteten Modell-/Office-/Crew-Capabilities und einem parallelen Lauf koppeln; das geheime Profil speichern. '
            'Python 3 wird für den lokalen Profilkonfigurator benötigt.\n\n'
            '```text\npython configure.py --profile ABS/device-profile.json --output ABS/new-private-agent --model-url https://your-litellm/v1 --model-name your-model --model-key-file ABS/own-model-key.txt\n```\n\n'
            'Danach den Agenten mit `'+binary.name+' --config ABS/new-private-agent/agent.json` starten. '
            'Die Beispiele verwenden absolute Pfade; die eigene Serveradresse steht im exportierten Profil. '
            'Direkte Officeagenten dürfen Modell-URL/-Name weglassen. Ollama nutzt `--model-api ollama --model-implementation ollama`; vLLM wird ausdrücklich als Implementierung gewählt. '
            'Eigene Programme werden mit `--program alias=ABS/executable` und `--allow-native-tools` ausgewählt. '
            'Konkrete persönliche Aktionen brauchen zusätzlich eine lokale Entscheidung über den `consent`-Befehl.\n\n'
            'Siehe [vollständige Anleitung](../docs/DEVICE-AGENT-V2.md). '
            'Linux unterstützt Modell-/Werkzeugausführung und lokale X11- oder ausdrücklich freigegebene Wayland-Portal-Sitzungen. Systemabhängigkeiten und physische Desktopabnahme stehen in der Anleitung.\n',encoding='utf-8')
    for path in (ROOT/'tools/device_agent').iterdir():
        if path.is_file() and path.suffix in {'.py','.md'}:copy(path,args.output/'tools/device_agent'/path.name)
    copy(ROOT/'docs/DEVICE-AGENT-V2.md',args.output/'docs/DEVICE-AGENT-V2.md')
    copy(ROOT/'LICENSE',args.output/'LICENSE')
    for name in sources:copy(ROOT/name,args.output/'source'/name)
    for name in ('README.md','LICENSE','tools/device_agent/configure.py','tools/device_agent/windows_private.py','tools/device_agent/ui.py','tools/device_agent/README.md','tools/device_agent/package.py','docs/DEVICE-AGENT-V2.md','tools/demo/device_agent_v2_demo.py','tools/demo/alpha5_demo.py'):
        copy(ROOT/name,args.output/'source'/name)
    dump(args.output/'source/compiler-inputs.json',sources)
    (args.output/'README.md').write_text(
        '# LocalAI Cowork V2 – eigenständiger Geräteagent\n\n'
        'Enthalten sind native Windows-/Linux-x64-Agenten, der private Profilkonfigurator und die Quellen. '
        'Dieses zusätzliche Paket arbeitet mit dem V2-Server '+version+' und benötigt keinen V1-Daemon oder V1-IPC. '
        'Bisherige App-Auslieferungen und ihre Originalquittungen bleiben erhalten.\n\n'
        'Start und Einrichtung: [Windows](Windows/README.md), [Linux](Linux/README.md), [Details](docs/DEVICE-AGENT-V2.md). '
        'Persönliche Geräte nutzen eigene Modell-/LiteLLM-, GitHub- und MCP-Zugänge. '
        'Konkrete files/shell/git/web.fetch-, OOXML-/LibreOffice-/Microsoft-Office-, sichtbare Browser- und CrewAI-Adapter sind in den Quellen enthalten. '
        'Ihre tatsächlich ausgewählten externen Laufzeiten müssen vom Gerätebetreiber bereitgestellt werden. '
        'Native Programme laufen mit den Rechten des Gerätebetreibers und werden ausdrücklich ausgewählt.\n\n'
        'Windows-Desktopansicht und Fernsteuerung brauchen eine entsperrte interaktive Sitzung und eine passende Kopplung. '
        'Linux verwendet X11 oder eine frisch freigegebene Wayland-Portal-/PipeWire-Sitzung. Pro Agent wird ein gleichzeitiger Lauf unterstützt. Physische Desktopabnahme ist damit nicht behauptet. '
        'Die beigefügten Belege gelten ausschließlich für ihre exakten Binär- und Quellenhashes. Der ausdrücklich schnelle Paketbau enthält keine neue Laufzeit- oder Installationsabnahme. '
        'Das Paket ist unsigniert und wurde nicht öffentlich veröffentlicht.\n',encoding='utf-8')
    public=args.output/'PublicReceipts';public.mkdir()
    if args.build_only:
        dump(public/'builds.json',{'scope':'current native builds only; no runtime or installation qualification','version':version,'protocol':2,'compiler_inputs':len(sources),'windows':public_build(windows,args.windows_receipt),'linux':public_build(linux,args.linux_receipt)})
    else:
        dump(public/'builds.json',{'scope':'native release builds; no extensive test suite', 'version':'2.0.0-alpha.5','protocol':2,'compiler_inputs':len(sources),'windows':{'sha256':sha(args.windows),'exit_code':windows['exit_code'],'owned_processes_empty':True,'forced_cleanup':False},'linux':{'sha256':sha(args.linux),'exit_code':linux['qualifier_exit_code'],'owned_processes_empty':True,'forced_cleanup':False}})
    if demo:
        dump(public/'isolated-device-demo.json',{key:demo[key] for key in ('format','result','daemon_sha256','agent_sha256','harness_sha256','user_credentials_used','v1_daemon_started','formal_qualification','steps','limitations','fixture_thread_joined','fixture_calls','processes','seconds')})
    files={path.relative_to(args.output).as_posix():{'size':path.stat().st_size,'sha256':sha(path)} for path in sorted(args.output.rglob('*')) if path.is_file()}
    manifest={'format':'localai-cowork-v2-device-agent-delivery/1','version':version,'protocol':2,'minimum_server':version,'validation_scope':validation_scope,'tests_run':demo is not None,'runtime_qualified':False,'release_qualified':False,'signed':False,'published':False,'installed':False,'v1_required':False,'files':files}
    dump(args.output/'manifest.json',manifest)
    with zipfile.ZipFile(archive,'x',zipfile.ZIP_DEFLATED,compresslevel=6) as package:
        for path in sorted(args.output.rglob('*')):
            if path.is_file():package.write(path,path.relative_to(args.output).as_posix())
    with zipfile.ZipFile(archive) as package:
        for name,record in files.items():
            if hashlib.sha256(package.read(name)).hexdigest()!=record['sha256']:raise ValueError('Archive bytes differ')
    if release.source_manifest(ROOT)!=sources:raise ValueError('Source changed during packaging; original output retained')
    if any(sha(path)!=digest for path,digest in ((path,original_receipts[str(path)]) for path in (args.windows_receipt,args.linux_receipt))):raise ValueError('Original agent build receipts changed during packaging')
    if any(sha(path)!=original_binaries[str(path)]['sha256'] or path.stat().st_size!=original_binaries[str(path)]['size'] for path in (args.windows,args.linux)):raise ValueError('Original agent binaries changed during packaging')
    print(json.dumps({'output':str(args.output),'archive':str(archive),'sha256':sha(archive),'files':len(files),'compiler_inputs':len(sources)}))

if __name__=='__main__':main()
