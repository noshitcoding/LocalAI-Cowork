"""Qualify storage contracts on a disposable, loopback-only PostgreSQL cluster.

Uses installed PostgreSQL binaries as the invoking non-root Linux user. Never
connects to, initializes, stops, or edits the existing system cluster or Docker.
"""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import secrets
import socket
import subprocess
import tempfile
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--bin-dir', default='/usr/lib/postgresql/16/bin')
args = parser.parse_args()
assert os.name == 'posix' and os.getuid() != 0, 'Use a non-root Linux test user'
root = pathlib.Path(__file__).resolve().parents[2]
pg = pathlib.Path(args.bin_dir).resolve(strict=True)
for name in ['initdb', 'pg_ctl', 'createdb', 'postgres']:
    assert (pg/name).is_file(), 'PostgreSQL binary unavailable: '+name
out = root/'.local'/('postgres-isolated-'+uuid.uuid4().hex)
out.mkdir(parents=True)
paths = [root/name for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']]
for directory in ['crates', 'services', 'tools/contracts', 'tools/qualification']:
    paths.extend(p for p in (root/directory).rglob('*') if p.is_file()
                 and not {'target', '__pycache__', 'node_modules'} & set(p.relative_to(root).parts)
                 and p.suffix in {'.rs', '.toml', '.sql', '.py', '.mjs', '.json'})
assert all(not p.is_symlink() for p in paths)
manifest = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(set(paths))}
(out/'sources.json').write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
report = {'scope': 'Fourteen shared SQLite/PostgreSQL storage contracts including explicit human continuation and actual registry-lock waits past original deadlines; private PostgreSQL test cluster',
          'platform': platform.platform(),
          'postgres': subprocess.check_output([str(pg/'postgres'), '--version'], text=True).strip(),
          'source_files': len(manifest),
          'source_manifest_sha256': hashlib.sha256((out/'sources.json').read_bytes()).hexdigest(),
          'result': 'running', 'checks': [], 'cluster_stopped': False,
          'limitations': ['No existing system database or Docker fixture used',
                          'No enterprise/HA, process failover, real workflow provider/broker dispatch or load acceptance']}
def save():
    (out/'result.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
save()
print(out, flush=True)
with tempfile.TemporaryDirectory(prefix='cowork-v2-postgres-', dir='/tmp') as directory:
    fixture = pathlib.Path(directory).resolve(strict=True)
    assert fixture.parent == pathlib.Path('/tmp') and fixture.name.startswith('cowork-v2-postgres-')
    assert fixture.stat().st_uid == os.getuid()
    data = fixture/'data'
    password = secrets.token_urlsafe(36)
    passfile = fixture/'password'
    passfile.write_text(password+'\n', encoding='utf-8')
    passfile.chmod(0o600)
    socketdir = fixture/'socket'
    socketdir.mkdir(mode=0o700)
    with socket.socket() as choice:
        choice.bind(('127.0.0.1', 0))
        port = choice.getsockname()[1]
    user = 'cowork_fixture'
    env = os.environ.copy()
    for name in ['PGSERVICE', 'PGSERVICEFILE', 'PGPASSFILE', 'PGOPTIONS']:
        env.pop(name, None)
    env.update(PGHOST='127.0.0.1', PGPORT=str(port), PGUSER=user,
               PGPASSWORD=password, PGDATABASE='postgres')
    started = False
    def fixture_command(command, name):
        result = subprocess.run(command, env=env, capture_output=True, text=True,
                                encoding='utf-8', errors='replace', timeout=45)
        (out/(name+'.log')).write_text((result.stdout+result.stderr).replace(password, '[redacted]'), encoding='utf-8')
        assert result.returncode == 0, 'Test fixture command failed: '+name
    try:
        fixture_command([str(pg/'initdb'), '-D', str(data), '--username='+user,
                         '--pwfile='+str(passfile), '--auth-local=trust',
                         '--auth-host=scram-sha-256', '--encoding=UTF8', '--locale=C.UTF-8'], 'initdb')
        fixture_command([str(pg/'pg_ctl'), '-D', str(data), '-l', str(fixture/'server.log'),
                         '-o', '-h 127.0.0.1 -p '+str(port)+' -k '+str(socketdir),
                         '-w', '-t', '30', 'start'], 'start')
        started = True
        report['port'] = port
        for suite, case in [
            ('contract', 'postgres_same_storage_contract'),
            ('environments', 'postgres_environment_contract'),
            ('processes', 'postgres_process_contract'),
            ('mcp', 'postgres_mcp_contract'),
            ('mcp_connections', 'postgres_mcp_connection_contract'),
            ('mcp_credentials', 'postgres_mcp_credential_contract'),
            ('native_secrets', 'postgres_native_key_setup_contract'),
            ('workflows', 'postgres_workflow_contract'),
            ('workflows', 'journal_cases::postgres_workflow_journal_contract'),
            ('workflows', 'input_cases::postgres_workflow_input_contract'),
            ('workflows', 'input_draft_cases::postgres_workflow_input_draft_contract'),
            ('workflows', 'resume_cases::postgres_workflow_input_resume_contract'),
            ('lib', 'workflow_clocks::tests::postgres_workflow_clock_contract'),
            ('lib', 'workflow_input_drafts::clock_tests::postgres_input_clock_fences_after_actual_run_and_quota_lock_waits'),
        ]:
            label = {
                'journal_cases::postgres_workflow_journal_contract': 'workflow-journal',
                'input_cases::postgres_workflow_input_contract': 'workflow-inputs',
                'input_draft_cases::postgres_workflow_input_draft_contract': 'workflow-input-drafts',
                'resume_cases::postgres_workflow_input_resume_contract': 'workflow-input-resume',
                'workflow_clocks::tests::postgres_workflow_clock_contract': 'workflow-clocks',
                'workflow_input_drafts::clock_tests::postgres_input_clock_fences_after_actual_run_and_quota_lock_waits': 'workflow-input-clocks',
            }.get(case, suite)
            database = 'cowork_'+suite+'_'+uuid.uuid4().hex
            fixture_command([str(pg/'createdb'), database], 'createdb-'+label)
            env['COWORK_TEST_POSTGRES'] = 'postgresql://'+user+':'+password+'@127.0.0.1:'+str(port)+'/'+database
            selection = ['--lib'] if suite == 'lib' else ['--test', suite]
            command = ['cargo', 'test', '-p', 'cowork-storage'] + selection + [case,
                       '--locked', '--offline', '--', '--exact', '--ignored']
            begin = time.monotonic()
            result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True,
                                    encoding='utf-8', errors='replace', timeout=600)
            log = (result.stdout+result.stderr).replace(env['COWORK_TEST_POSTGRES'], '[redacted test database]').replace(password, '[redacted]')
            (out/(label+'.log')).write_text(log, encoding='utf-8')
            passed = sum(map(int, re.findall(r'test result: ok\. (\d+) passed', log)))
            report['checks'].append({'suite': suite, 'label': label, 'case': case, 'exit_code': result.returncode,
                                     'passed': passed, 'seconds': round(time.monotonic()-begin, 2)})
            save()
            print(label+': '+str(result.returncode), flush=True)
            assert result.returncode == 0 and passed == 1, 'Contract failed: '+label
        source_drift = [p for p,h in manifest.items() if not (root/p).is_file() or hashlib.sha256((root/p).read_bytes()).hexdigest() != h]
        if source_drift:
            report['source_drift'] = source_drift
            raise RuntimeError('Sources changed during qualification: ' + ', '.join(source_drift))
        report['result'] = 'passed'
    finally:
        # A start error may have occurred after postgres launched; check only the
        # newly created private data directory. Never act on a global service.
        if (data/'postmaster.pid').exists():
            fixture_command([str(pg/'pg_ctl'), '-D', str(data), '-m', 'fast', '-w', '-t', '30', 'stop'], 'stop')
        report['cluster_stopped'] = not (data/'postmaster.pid').exists()
        if report['result'] == 'running':
            report['result'] = 'failed'
        save()
print(json.dumps(report, indent=2), flush=True)
