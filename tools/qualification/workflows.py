"""Qualify native workflows, durable queue and artifact tools, public versioned library/start contracts and transactional attention; no live app changes."""
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
destination = root / '.local' / ('workflows-' + uuid.uuid4().hex)
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
    'scope': 'Native workflow compiler/reducer, registry, queue, public APIs, journals, questions, answer drafts/intents and private workflow clocks; explicit whole-graph human continuation with atomic accepted receipt and exact-generation activation, preserving original claims, deadlines and reservations; optional full existing workspace regression',
    'platform': platform.platform(), 'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
    'source_files': len(manifest),
    'rust_min_stack_override': env.get('RUST_MIN_STACK'),
    'source_manifest_sha256': hashlib.sha256((destination / 'sources.json').read_bytes()).hexdigest(),
    'result': 'running', 'steps': [],
    'limitations': ['Public workflow APIs use the native coordinator and shared queue; external workflow file/MCP/process IO and hierarchical product execution remain unfinished; questions and answer drafts/intents are implemented in the single-owner scope',
                    'SQLite hard-process recovery includes a child-owned synthetic provider transport; not a real LLM, tool sandbox or OS restart',
                    'PostgreSQL contracts require the separate isolated database qualifier',
                    'Recovery interrupts active claims and preserves admissions; human continuation requires an explicit journal operation and known saved model response/question cursors, then fresh exact-generation execution claims; unknown effects, legacy missing clocks, hierarchical execution and unsupported cursors remain blocked',
                    'Workflow grants are opaque references from a trusted application service; reducer grants/permits do not authorize filesystem/network IO',
                    'Clock test sources are private per-store fixtures; UTC rollback tests do not change or qualify the operating-system clock; missing legacy clocks are never backfilled',
                    'UI requires separate frontend qualification; no enterprise, hierarchical product execution, scheduler, load or full M5 acceptance'],
}

def save():
    (destination / 'result.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')

selection = ['--workspace'] if args.workspace else ['-p', 'cowork-domain', '-p', 'cowork-storage']
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
source_drift = [path for path, digest in manifest.items() if not (root / path).is_file() or hashlib.sha256((root / path).read_bytes()).hexdigest() != digest]
if source_drift:
    report['result'] = 'failed'
    report['source_drift'] = source_drift
    save()
    raise SystemExit('Sources changed during qualification: ' + ', '.join(source_drift))
report['result'] = 'passed'
save()
print(json.dumps(report, indent=2), flush=True)
