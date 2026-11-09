"""Execute Linux core checks as an unprivileged user, with no external network."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys
import time

assert sys.platform == 'linux' and os.getuid() != 0
parser = argparse.ArgumentParser()
parser.add_argument('--suite', choices=['core', 'executor', 'environments'], default='core')
parser.add_argument('--native-secrets', action='store_true', help='Requires dbus-run-session and gnome-keyring in an isolated test image')
args = parser.parse_args()
root = pathlib.Path('/workspace')
output = pathlib.Path('/home/qualification/evidence')
output.mkdir(exist_ok=True)
steps = [
    ('format', ['cargo', 'fmt', '--all', '--', '--check']),
    ('rust-tests', ['cargo', 'test', '--locked', '--offline', '--workspace']),
    ('clippy', ['cargo', 'clippy', '--locked', '--offline', '--workspace', '--all-targets', '--', '-D', 'warnings']),
    ('daemon-build', ['cargo', 'build', '--locked', '--offline', '-p', 'cowork-daemon']),
]
binary = str(pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'debug/cowork-daemon')
steps += [(name, ['python3', f'tests/system/{name}.py', '--binary', binary]) for name in
          ['durability', 'files', 'workspaces', 'file_operations', 'uploads', 'native_folders', 'model_budgets']]
if args.suite == 'executor':
    steps = [('format', ['cargo', 'fmt', '--all', '--', '--check']),
             ('executor-tests', ['cargo', 'test', '--locked', '--offline', '-p', 'cowork-executor']),
             ('executor-clippy', ['cargo', 'clippy', '--locked', '--offline', '-p', 'cowork-executor', '--all-targets', '--', '-D', 'warnings'])]
if args.suite == 'environments':
    steps = [('format', ['cargo', 'fmt', '--all', '--', '--check']),
             ('environment-tests', ['cargo', 'test', '--locked', '--offline', '-p', 'cowork-storage', '--test', 'environments', '--test', 'migrations']),
             ('environment-clippy', ['cargo', 'clippy', '--locked', '--offline', '-p', 'cowork-domain', '-p', 'cowork-storage', '--all-targets', '--', '-D', 'warnings'])]
if args.native_secrets:
    steps.append(('native-secrets', ['dbus-run-session', '--', 'python3', 'tools/qualification/native_secrets.py']))
report = {
    'scope': 'Linux '+args.suite+' qualification in an unprivileged Docker container; no desktop, gVisor or enterprise acceptance',
    'suite': args.suite,
    'platform': platform.platform(), 'os_release': pathlib.Path('/etc/os-release').read_text(),
    'uid': os.getuid(), 'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
    'python': platform.python_version(), 'source_manifest_sha256': hashlib.sha256((root/'source-manifest.json').read_bytes()).hexdigest(),
    'steps': [], 'result': 'running',
}
def save():
    (output/'result.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
save()
for name, command in steps:
    print(f'Qualification: {name}', flush=True)
    start = time.monotonic()
    with (output/f'{name}.log').open('wb') as log:
        child = subprocess.Popen(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        for line in child.stdout:
            log.write(line)
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        code = child.wait()
        child.stdout.close()
    report['steps'].append({'name':name,'command':command,'exit_code':code,'seconds':round(time.monotonic()-start,2)})
    if code:
        report['result'] = 'failed'
        save()
        raise SystemExit(code)
    save()
report['result'] = 'passed'
if args.suite == 'core':
    report['daemon_sha256'] = hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()
elif args.suite == 'executor':
    report['synthetic_fixture_sha256'] = hashlib.sha256((pathlib.Path(os.environ['CARGO_TARGET_DIR'])/'debug/cowork-executor-fixture').read_bytes()).hexdigest()
save()
print(json.dumps(report), flush=True)
