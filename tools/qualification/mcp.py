"""Record MCP transport, durable journal and optional workspace regression; no live app changes."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import subprocess
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--workspace', action='store_true')
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[2]
destination = root / '.local' / ('mcp-' + uuid.uuid4().hex)
destination.mkdir(parents=True)
env = os.environ.copy()
if os.name == 'nt':
    env['CARGO_TARGET_DIR'] = str(root / '.local/windows-supervisor-build')
sources = [root / name for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']]
for folder in ['crates', 'services', 'tools/contracts', 'tools/qualification']:
    sources.extend(path for path in (root / folder).rglob('*') if path.is_file()
                   and not {'target', '__pycache__', 'node_modules'} & set(path.relative_to(root).parts)
                   and path.suffix in {'.rs', '.toml', '.sql', '.py', '.mjs', '.json'})
assert all(not path.is_symlink() for path in sources)
manifest = {path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(set(sources))}
(destination / 'sources.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
report = {
    'scope': 'MCP transport, durable journal, native key custody/setup and selected model-loop tools; synthetic stateful loopback servers',
    'platform': platform.platform(), 'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
    'source_files': len(manifest),
    'source_manifest_sha256': hashlib.sha256((destination / 'sources.json').read_bytes()).hexdigest(),
    'result': 'running', 'steps': [],
    'limitations': ['No live MCP provider acceptance, OAuth/discovery setup UI, public evidence reconciliation, server KMS or complete key recovery; manual catalogue configuration and selected model-loop tools are implemented',
                    'No MCP OAuth, stdio, legacy SSE, asynchronous tasks or real-server acceptance',
                    'No enterprise, load or full M4 acceptance; crash test uses a controlled synthetic remote server'],
}

def save():
    (destination / 'result.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')

selection = ['--workspace'] if args.workspace else ['-p', 'cowork-mcp', '-p', 'cowork-storage', '-p', 'cowork-runtime']
steps = [
    ('format', ['cargo', 'fmt', '--all', '--', '--check']),
    ('rust-tests', ['cargo', 'test', '--locked', '--offline'] + selection),
    ('clippy', ['cargo', 'clippy', '--locked', '--offline'] + selection + ['--all-targets', '--', '-D', 'warnings']),
]
save()
print(str(destination), flush=True)
for name, command in steps:
    start = time.monotonic()
    result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, encoding='utf-8', errors='replace')
    log = result.stdout + result.stderr
    (destination / (name + '.log')).write_text(log, encoding='utf-8')
    report['steps'].append({'name': name, 'command': command, 'exit_code': result.returncode,
                            'seconds': round(time.monotonic() - start, 2)})
    if name == 'rust-tests':
        report['rust_tests_passed'] = sum(map(int, re.findall(r'test result: ok\. (\d+) passed', log)))
    print(name + ': ' + str(result.returncode), flush=True)
    if result.returncode:
        print(log, flush=True)
        report['result'] = 'failed'
        save()
        raise SystemExit(result.returncode)
    save()
assert all(hashlib.sha256((root / path).read_bytes()).hexdigest() == digest for path, digest in manifest.items()), 'sources changed during qualification'
report['result'] = 'passed'
save()
print(json.dumps(report, indent=2), flush=True)
