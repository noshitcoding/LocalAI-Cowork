"""Build and bind one native standalone agent; no tests, installation or upload."""
from __future__ import annotations
import argparse
import hashlib
from datetime import datetime, timezone
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('agent_release', ROOT / 'tools/desktop/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def prepare(output: Path, target: Path, offline: bool) -> dict:
    output, target = output.absolute(), target.absolute()
    if output.exists() or output.is_symlink() or not output.parent.is_dir():
        raise release.Refused('select a new output directory with an existing parent')
    for name in ('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
                 'CARGO_ENCODED_RUSTDOCFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTC', 'RUST_MIN_STACK'):
        if os.environ.get(name):
            raise release.Refused('compiler/stack overrides require a separate build profile')
    if any(name.startswith('CARGO_PROFILE_RELEASE_') for name in os.environ):
        raise release.Refused('release profile overrides are not supported')
    before = release.source_manifest(ROOT)
    rustc = subprocess.check_output(['rustc', '-vV'], cwd=ROOT, text=True)
    host = release.native_host(rustc)
    meta_command = ['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1']
    if offline:
        meta_command.append('--offline')
    metadata = json.loads(subprocess.check_output(meta_command, cwd=ROOT, text=True))
    packages = [item for item in metadata['packages'] if item['name'] == 'cowork-device-agent-v2'
                and Path(item['manifest_path']) == ROOT / 'services/device-agent/Cargo.toml']
    if len(packages) != 1:
        raise release.Refused('the original device agent package must be unique')
    filename = 'cowork-device-agent-v2' + ('.exe' if os.name == 'nt' else '')
    command = ['cargo', 'build', '--locked', '--release', '-p', 'cowork-device-agent-v2', '-p', 'cowork-browser-host',
               '--bins', '--target', host, '--target-dir', str(target),
               '--message-format=json-diagnostic-short']
    if offline:
        command.append('--offline')
    result = subprocess.run(command, cwd=ROOT, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, text=True, check=True)
    artifacts, finished = [], []
    for line in result.stdout.splitlines():
        if not line.startswith('{'):
            continue
        item = json.loads(line)
        if item.get('reason') == 'build-finished':
            finished.append(item.get('success'))
        if (item.get('reason') == 'compiler-artifact' and item.get('package_id') == packages[0]['id']
                and item.get('target', {}).get('name') == 'cowork-device-agent-v2'
                and item.get('target', {}).get('kind') == ['bin'] and item.get('executable')):
            profile = item.get('profile', {})
            if (profile.get('opt_level') != '3' or profile.get('debug_assertions') is not False
                    or profile.get('test') is not False or profile.get('overflow_checks') is not False):
                raise release.Refused('the reported agent is not the original release profile')
            artifacts.append(Path(item['executable']))
    if finished != [True] or artifacts != [target / host / 'release' / filename]:
        raise release.Refused('exactly one successful native agent artifact is required')
    artifact = release.regular(artifacts[0], cargo_artifact=True)
    if not 0 < artifact.stat().st_size <= release.MAX_BINARY:
        raise release.Refused('bounded native agent required')
    release.validate_executable(artifact, host)
    if release.source_manifest(ROOT) != before:
        raise release.Refused('source changed during the build; original output retained')
    original = {'filename': filename, 'size': artifact.stat().st_size, 'sha256': release.sha256(artifact)}
    stage = Path(tempfile.mkdtemp(prefix='.agent-preparing-', dir=output.parent))
    staged = stage / filename
    shutil.copyfile(artifact, staged)
    shutil.copymode(artifact, staged)
    release.validate_executable(staged, host)
    if (release.sha256(staged) != original['sha256'] or release.sha256(artifact) != original['sha256']
            or staged.stat().st_size != original['size'] or release.source_manifest(ROOT) != before):
        raise release.Refused('source or agent bytes changed while staging; retained for inspection')
    receipt = {'format': 'localai-cowork-native-device-agent-release/1', 'result': 'passed',
               'prepared_at': datetime.now(timezone.utc).isoformat(), 'host': host,
               'package_version': packages[0]['version'], 'package_id': packages[0]['id'],
               'rustc': rustc, 'command': command, 'cargo_exit_code': result.returncode,
               'cargo_reported_success': True, 'artifact': original, 'sources': before,
               'cargo_json_sha256': hashlib.sha256(result.stdout.encode('utf-8')).hexdigest(),
               'source_before_after_equal': True, 'validation_scope': 'build_only',
               'tests_run': False, 'installed': False, 'signed': False, 'published': False,
               'native_process_tree_qualified': False, 'runtime_qualified': False, 'release_qualified': False}
    browser_records=[json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    browser_artifacts=[Path(item['executable']) for item in browser_records if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')=='cowork-browser-host' and item.get('executable')]
    if len(browser_artifacts)!=1:raise release.Refused('exactly one native browser host is required')
    browser=release.regular(browser_artifacts[0],cargo_artifact=True);release.validate_executable(browser,host)
    browser_record=next(item for item in browser_records if item.get('executable')==str(browser) and item.get('target',{}).get('name')=='cowork-browser-host')
    browser_profile=browser_record.get('profile',{})
    if (browser!=target/host/'release'/('cowork-browser-host.exe' if os.name=='nt' else 'cowork-browser-host')
            or browser_profile.get('opt_level')!='3' or browser_profile.get('debug_assertions') is not False or browser_profile.get('test') is not False):
        raise release.Refused('browser host must be the exact current native release artifact')
    browser_name='cowork-browser-host'+('.exe' if os.name=='nt' else '')
    browser_target=stage/browser_name;shutil.copyfile(browser,browser_target);shutil.copymode(browser,browser_target)
    receipt['browser_host']={'filename':browser_name,'size':browser_target.stat().st_size,'sha256':release.sha256(browser_target)}
    (stage / 'cargo-build.jsonl').write_text(result.stdout, encoding='utf-8', newline='\n')
    (stage / 'receipt.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n', encoding='utf-8', newline='\n')
    # No original output is replaced. A failed stage remains inspectable.
    stage.rename(output)
    return receipt


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    receipt = prepare(args.output_dir, args.target_dir, args.offline)
    print(json.dumps({'output': str(args.output_dir.absolute()), 'artifact': receipt['artifact'],
                      'validation_scope': receipt['validation_scope'], 'release_qualified': False}))
