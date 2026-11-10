"""Private hosted-runner unsigned smoke coordinator. No build/sign/publish commands."""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

import verify_review as verify


def write_json(path, value):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')


def hosted_guard(environment=None):
    env = os.environ if environment is None else environment
    verify.require(env.get('GITHUB_ACTIONS') == 'true'
                   and env.get('GITHUB_EVENT_NAME') == 'workflow_dispatch'
                   and env.get('CI_RUNNER_ENVIRONMENT') == 'github-hosted',
                   'Only a manually dispatched GitHub-hosted disposable VM is permitted')
    event = verify.read_json(Path(env['GITHUB_EVENT_PATH']).absolute())
    branch = event.get('repository', {}).get('default_branch')
    verify.require(branch and env.get('GITHUB_REF') == 'refs/heads/' + branch,
                   'Dispatch the trusted smoke harness from its reviewed default branch')
    verify.require(event.get('repository', {}).get('full_name') == env.get('GITHUB_REPOSITORY')
                   and bool(re.fullmatch(r'[0-9a-f]{40}', env.get('GITHUB_SHA', '')))
                   and env.get('RUNNER_OS') in ('Linux', 'Windows')
                   and Path(env.get('RUNNER_TEMP', '')).is_absolute(), 'Missing exact runner/repository context')
    return env


def clean_environment(environment=None):
    env = os.environ if environment is None else environment
    return {key: value for key, value in env.items()
            if not key.upper().startswith(('COWORK_', 'GH_', 'ACTIONS_'))
            and not any(word in key.upper() for word in ('TOKEN', 'PASSWORD', 'SECRET', 'CREDENTIAL'))
            and key.upper() != 'PSMODULEPATH'
            and key.upper() not in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY')}


def runner_path(path, *, short_windows=False):
    env = hosted_guard()
    selected = path.absolute()
    parent = selected.parent.resolve(strict=True)
    temporary = Path(env['RUNNER_TEMP']).resolve(strict=True)
    allowed = parent == temporary or temporary in parent.parents
    if short_windows and env['RUNNER_OS'] == 'Windows':
        expected = 'cw-smoke-' + env['GITHUB_RUN_ID'] + '-' + env['GITHUB_RUN_ATTEMPT'] + '-nsis'
        allowed |= parent == Path(temporary.anchor) and selected.name == expected
    verify.require(allowed and selected.name not in ('.', '..') and not selected.exists(),
                   'Choose a new runner-owned directory; never an existing/user directory')
    return parent / selected.name


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


def github_json(repository, resource):
    token = os.environ.get('GH_TOKEN')
    verify.require(token, 'A step-scoped read-only GitHub token is required for original metadata')
    verify.require(bool(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository))
                   and bool(re.fullmatch(r'actions/(runs/[1-9][0-9]*|artifacts/[1-9][0-9]*)', resource)),
                   'Only selected original Actions metadata can be fetched')
    request = urllib.request.Request('https://api.github.com/repos/' + repository + '/' + resource,
        headers={'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
                 'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'localai-cowork-unsigned-smoke'})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=30) as response:
        raw = response.read(2 * 1024**2 + 1)
        verify.require(len(raw) <= 2 * 1024**2, 'GitHub metadata exceeds its bound')
    value = json.loads(raw)
    verify.require(isinstance(value, dict), 'GitHub metadata must be an object')
    return value


def download_archive(repository, artifact_id, destination, expected):
    token = os.environ.get('GH_TOKEN')
    verify.require(token and isinstance(artifact_id, int) and artifact_id > 0, 'Exact artifact and scoped token required')
    url = f'https://api.github.com/repos/{repository}/actions/artifacts/{artifact_id}/zip'
    request = urllib.request.Request(url, headers={'Authorization': 'Bearer ' + token,
        'Accept': 'application/vnd.github+json', 'User-Agent': 'localai-cowork-unsigned-smoke'})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        response = opener.open(request, timeout=30)
    except urllib.error.HTTPError as error:
        verify.require(error.code == 302, 'Original artifact download did not return its expected redirect')
        location = error.headers.get('Location', '')
        error.close()
        parsed = urllib.parse.urlsplit(location)
        verify.require(parsed.scheme == 'https' and parsed.hostname and not parsed.username and not parsed.password,
                       'Artifact redirect must use ordinary HTTPS')
        # Follow the signed storage URL WITHOUT forwarding GitHub Authorization.
        plain = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        response = plain.open(urllib.request.Request(location,
            headers={'User-Agent': 'localai-cowork-unsigned-smoke'}), timeout=30)
    count = 0
    start = time.monotonic()
    value = hashlib.sha256()
    with response, destination.open('xb') as output:
        while True:
            chunk = response.read(1024**2)
            if not chunk:
                break
            count += len(chunk)
            verify.require(count <= verify.MAX_ARCHIVE and time.monotonic() - start <= 300,
                           'Artifact download exceeded byte/time bound')
            value.update(chunk)
            output.write(chunk)
    verify.require(count > 0 and value.hexdigest() == expected, 'Original archive SHA256 mismatch')


def selection(platform):
    env = os.environ
    prefix = platform.upper()
    return {'repository': env['GITHUB_REPOSITORY'], 'source_commit': env['SOURCE_COMMIT'],
            'producer_run_id': int(env['PRODUCER_RUN_ID']), 'platform': platform,
            'artifact_id': int(env[prefix + '_ARTIFACT_ID']),
            'archive_sha256': env[prefix + '_ARCHIVE_SHA256']}


def verify_source(source, commit):
    head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    verify.require(head == commit and not subprocess.check_output(
        ['git', '-C', str(source), 'status', '--porcelain', '--untracked-files=no']),
        'Source material must be the exact unchanged original producer commit')


def prepare(platform, source, output):
    hosted_guard()
    selected = selection(platform)
    run = github_json(selected['repository'], 'actions/runs/' + str(selected['producer_run_id']))
    artifact = github_json(selected['repository'], 'actions/artifacts/' + str(selected['artifact_id']))
    verify.checked_metadata(selected, run, artifact)
    verify_source(source, selected['source_commit'])
    output = runner_path(output)
    output.mkdir(mode=0o700)
    write_json(output / 'run.json', run)
    write_json(output / 'artifact.json', artifact)
    write_json(output / 'selection.json', selected)
    archive = output / 'original.zip'
    download_archive(selected['repository'], selected['artifact_id'], archive, selected['archive_sha256'])
    verify.safe_extract(archive, output / 'original', selected['archive_sha256'])
    binding = verify.checked_tree(output / 'original', source, selected, run)
    binding.update(archive=verify.pin(archive), producer_run_attempt=run['run_attempt'],
                   harness_commit=os.environ['GITHUB_SHA'], schema_version=8)
    for name, env_name in [('source_sha256', 'EXPECTED_SOURCE_SHA256'),
                           ('frontend_receipt_sha256', 'EXPECTED_FRONTEND_RECEIPT_SHA256')]:
        if os.environ.get(env_name):
            verify.require(binding[name] == os.environ[env_name], 'Policy common frontend/source pin mismatch')
    write_json(output / 'input-binding.json', binding)
    return binding, output


def github_output(values):
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as stream:
        for key, value in values.items():
            verify.require(not any(char in str(value) for char in '\r\n'), 'Non-scalar workflow output')
            stream.write(key + '=' + str(value) + '\n')


def native_command(kind, binding, original, work, output):
    here = Path(__file__).resolve().parent
    if kind == 'nsis':
        return ['powershell', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
                '-File', str(here / 'nsis_smoke.ps1'), '-Binding', str(binding),
                '-OriginalDir', str(original), '-WorkRoot', str(work), '-Output', str(output)]
    verify.require(kind in ('deb', 'appimage'), 'Only the three reviewed native bundle kinds are supported')
    return [sys.executable, '-B', str(here / 'installed_lifetime_linux.py'), '--kind', kind,
            '--binding', str(binding), '--original-dir', str(original), '--work-root', str(work), '--output', str(output)]


def execute(args):
    env = hosted_guard()
    verify.require((args.kind == 'nsis') == (env['RUNNER_OS'] == 'Windows'), 'Native bundle/runner mismatch')
    binding_path = args.prepared / 'input-binding.json'
    verify.require(verify.digest(binding_path) == args.binding_sha256, 'Prepared binding changed before execution')
    binding = verify.read_json(binding_path)
    selected = verify.read_json(args.prepared / 'selection.json')
    run = github_json(selected['repository'], 'actions/runs/' + str(selected['producer_run_id']))
    artifact = github_json(selected['repository'], 'actions/artifacts/' + str(selected['artifact_id']))
    verify.checked_metadata(selected, run, artifact)
    verify.require(run['run_attempt'] == binding['producer_run_attempt'], 'Original producer attempt changed')
    verify_source(args.source_root, selected['source_commit'])
    checked = verify.checked_tree(args.prepared / 'original', args.source_root, selected, run)
    verify.require(all(binding[key] == checked[key] for key in checked)
                   and verify.pin(args.prepared / 'original.zip') == binding['archive'],
                   'Original package/receipt/archive binding changed before execution')
    work = runner_path(args.work_root, short_windows=True)
    output = runner_path(args.output)
    verify.require(work != output and work not in output.parents and output not in work.parents,
                   'Keep evidence outside the installation/profile tree')
    output.mkdir(mode=0o700)
    command = native_command(args.kind, binding_path, args.prepared / 'original', work, output / 'native')
    started = datetime.now(timezone.utc).isoformat()
    result_code = None
    timed_out = False
    try:
        # No GitHub/provider/signing credentials reach installer/native helpers.
        with (output / 'coordinator.stdout.log').open('xb') as stdout, (output / 'coordinator.stderr.log').open('xb') as stderr:
            try:
                result = subprocess.run(command, env=clean_environment(), stdout=stdout, stderr=stderr, timeout=900)
            except subprocess.TimeoutExpired:
                timed_out = True
                raise
            result_code = result.returncode
        smoke = verify.read_json(output / 'native/smoke.json')
        verify.require(result_code == 0 and smoke.get('result') == 'passed'
                       and all(smoke.get(flag) is False for flag in ('signed', 'published', 'release_qualified')),
                       'Physical smoke or cleanup did not pass')
    finally:
        files = {}
        for path in output.rglob('*'):
            if path.is_file():
                verify.require(path.stat().st_size <= 64 * 1024**2 and len(files) < 1000,
                               'Smoke evidence exceeds its per-file/count bound')
                files[path.relative_to(output).as_posix()] = verify.pin(path)
        verify.require(sum(item['size'] for item in files.values()) <= 256 * 1024**2, 'Smoke evidence exceeds total bound')
        write_json(output / 'smoke-index.json', {'format': 'localai-cowork-unsigned-smoke-index/1',
            'kind': args.kind, 'binding': verify.pin(binding_path), 'harness_commit': env['GITHUB_SHA'],
            'started_at': started, 'finished_at': datetime.now(timezone.utc).isoformat(),
            'command': command, 'exit_code': result_code, 'timed_out': timed_out, 'evidence': files,
            'signed': False, 'published': False, 'release_qualified': False})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    for action in ('policy', 'prepare'):
        command = commands.add_parser(action)
        command.add_argument('--source-root', type=Path, required=True)
        command.add_argument('--output', type=Path, required=True)
        if action == 'prepare':
            command.add_argument('--platform', choices=verify.HOSTS, required=True)
    command = commands.add_parser('execute')
    command.add_argument('--kind', choices=('nsis', 'deb', 'appimage'), required=True)
    for name in ('prepared', 'source-root', 'work-root', 'output'):
        command.add_argument('--' + name, type=Path, required=True)
    command.add_argument('--binding-sha256', required=True)
    args = parser.parse_args()
    hosted_guard()
    if args.action == 'execute':
        execute(args)
    elif args.action == 'prepare':
        value, path = prepare(args.platform, args.source_root, args.output)
        github_output({'binding_sha256': verify.digest(path / 'input-binding.json')})
    else:
        output = runner_path(args.output)
        output.mkdir(mode=0o700)
        windows, _ = prepare('windows', args.source_root, output / 'windows')
        linux, _ = prepare('linux', args.source_root, output / 'linux')
        for key in ('source_commit', 'source_sha256', 'frontend_receipt_sha256', 'producer_run_id', 'producer_run_attempt'):
            verify.require(windows[key] == linux[key], 'Original platform packages do not share one build source/frontend')
        github_output({'source_sha256': windows['source_sha256'],
                       'frontend_receipt_sha256': windows['frontend_receipt_sha256']})
        write_json(output / 'policy-binding.json', {'windows': windows, 'linux': linux,
            'installer_execution_performed': False, 'signed': False, 'published': False, 'release_qualified': False})


if __name__ == '__main__':
    main()
