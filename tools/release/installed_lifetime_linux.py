"""Actual DEB/AppImage smoke, ONLY on a fresh hosted Linux VM; private proposal."""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

import unsigned_smoke as coordinator
import verify_review as verify

LOG_LIMIT = 8 * 1024**2
FAILED_STATES = {'failed', 'canceled', 'expired', 'interrupted'}


def package_commands(kind, package, name=None):
    verify.require(kind in ('deb', 'appimage'), 'Linux bundle kind must be explicit')
    if kind == 'appimage':
        return {'launch': [str(package)]}
    verify.require(name and bool(re.fullmatch(r'[a-z0-9][a-z0-9+.-]+', name)), 'Exact Debian package name required')
    return {'install': ['sudo', 'env', 'DEBIAN_FRONTEND=noninteractive', 'apt-get', 'install',
                        '--yes', '--no-install-recommends', str(package)],
            'remove': ['sudo', 'env', 'DEBIAN_FRONTEND=noninteractive', 'apt-get', 'remove', '--yes', name]}


def command(report, output, label, args, *, allowed=(0,), timeout=180, env=None):
    log = output / (label + '.log')
    started = time.monotonic()
    result_code = None
    timed_out = False
    with log.open('xb') as stream:
        try:
            result = subprocess.run(args, env=env or coordinator.clean_environment(),
                stdout=stream, stderr=subprocess.STDOUT, timeout=timeout)
            result_code = result.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
    record = {'label': label, 'command': args, 'exit_code': result_code, 'timed_out': timed_out,
              'elapsed_seconds': round(time.monotonic() - started, 3), 'log': verify.pin(log)}
    report['commands'].append(record)
    verify.require(log.stat().st_size <= LOG_LIMIT, 'Native command log exceeded its bound')
    verify.require(not timed_out and result_code in allowed, 'Native command failed or timed out: ' + label)
    return log.read_text(errors='replace')


def port_free():
    with socket.socket() as probe:
        verify.require(probe.connect_ex(('127.0.0.1', 43821)) != 0,
                       'Port 43821 is occupied; no existing daemon will be stopped')


def proc_info(pid):
    raw = Path(f'/proc/{pid}/stat').read_text()
    fields = raw[raw.rfind(')') + 2:].split()
    uid = next(line for line in Path(f'/proc/{pid}/status').read_text().splitlines() if line.startswith('Uid:')).split()[1]
    return {'pid': pid, 'session': int(fields[3]), 'start_ticks': int(fields[19]), 'uid': int(uid)}


def exited(process, timeout=0):
    return bool(select.select([process['fd']], [], [], timeout)[0])


def capture(pid, session, *, profile=None, expected=None, allowed_root=None, own_session_child=False):
    fd = os.pidfd_open(pid)
    try:
        before = proc_info(pid)
        verify.require(before['session'] == session and before['uid'] == os.getuid(), 'Foreign process session/owner')
        try:
            image = Path(f'/proc/{pid}/exe').resolve(strict=True)
        except PermissionError:
            if not own_session_child:
                raise
            # WebKit may mark a same-UID child nondumpable. Its new session,
            # TokenUser/UID and retained PIDfd still bind our exact own process.
            image = None
        if profile is not None:
            environment = Path(f'/proc/{pid}/environ').read_bytes().split(b'\0')
            verify.require(('COWORK_DATA_DIR=' + str(profile)).encode() in environment, 'Foreign process profile')
        if allowed_root is not None:
            verify.require(allowed_root == image.parent or allowed_root in image.parents, 'Process image escaped selected installation/extraction')
        actual = verify.pin(image) if expected is not None else None
        if expected is not None:
            verify.require(actual == {key: expected[key] for key in ('size', 'sha256')}, 'Installed process payload hash mismatch')
        verify.require(proc_info(pid) == before and not select.select([fd], [], [], 0)[0], 'Process identity changed during capture')
        return {**before, 'fd': fd, 'image': str(image) if image is not None else 'unavailable-own-session-child', 'payload': actual}
    except BaseException:
        os.close(fd)
        raise


def record_process(report, process, role):
    key = (process['pid'], process['start_ticks'])
    if not any((item['pid'], item['start_ticks']) == key for item in report['processes']):
        report['processes'].append({key: value for key, value in process.items() if key != 'fd'} | {'role': role, 'physical_exit': False})


def mark_exit(report, process):
    verify.require(exited(process), 'Retained PIDfd has not signaled physical exit')
    for item in report['processes']:
        if item['pid'] == process['pid'] and item['start_ticks'] == process['start_ticks']:
            item['physical_exit'] = True


def stop_ui(report, process):
    if not exited(process):
        signal.pidfd_send_signal(process['fd'], signal.SIGTERM)
    verify.require(exited(process, 10), 'Owned desktop did not physically stop')
    mark_exit(report, process)


def api_client(profile):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), coordinator.NoRedirect())
    def api(method, path, body=None, *, native=False):
        token = (profile / 'daemon.token').read_text().strip()
        verify.require(48 <= len(token) <= 4096, 'Private daemon token unavailable')
        headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
        if native:
            private = (profile / 'native.token').read_text().strip()
            verify.require(48 <= len(private) <= 4096, 'Private native token unavailable')
            headers['x-cowork-native'] = private
        request = urllib.request.Request('http://127.0.0.1:43821/api/v2' + path,
            method=method, data=None if body is None else json.dumps(body).encode(), headers=headers)
        with opener.open(request, timeout=5) as response:
            raw = response.read(2 * 1024**2 + 1)
            verify.require(len(raw) <= 2 * 1024**2, 'Local API response exceeded bound')
            return json.loads(raw)
    return api


def shutdown_daemon(report, api, process, binding):
    current = api('GET', '/native/lifecycle', native=True)
    verify.require(current.get('pid') == process['pid'] and current.get('product') == 'LocalAI Cowork V2'
        and current.get('version') == binding['version'] and current.get('schema_version') == 8
        and current.get('closing') is False and current.get('instance_id'), 'Native shutdown instance differs from retained process')
    request = {key: current[key] for key in ('version', 'schema_version', 'pid', 'instance_id')}
    reply = api('POST', '/native/lifecycle/shutdown', request, native=True)
    verify.require(all(reply.get(key) == value for key, value in request.items()) and reply.get('closing') is True,
                   'Native shutdown acknowledgement mismatch')
    verify.require(exited(process, 15), 'Native shutdown acknowledged without physical process exit')
    mark_exit(report, process)


def own_processes(launchers, retained, report):
    sessions = {process.pid for process in launchers}
    known = {(item['pid'], item['start_ticks']) for item in retained}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            info = proc_info(int(entry.name))
            if info['uid'] != os.getuid() or info['session'] not in sessions or (info['pid'], info['start_ticks']) in known:
                continue
            if len(retained) >= 256:
                raise RuntimeError('Own session process count exceeded bound')
            child = capture(info['pid'], info['session'], own_session_child=True)
            retained.append(child)
            known.add((child['pid'], child['start_ticks']))
            record_process(report, child, 'owned-session-child')
        except (FileNotFoundError, ProcessLookupError, PermissionError, ValueError):
            continue


def cleanup_processes(launchers, retained, report):
    # All sessions were created by this invocation with start_new_session=True.
    # Capture their exact native identities; never signal a group/name/re-found PID.
    own_processes(launchers, retained, report)
    for process in retained:
        if not exited(process):
            signal.pidfd_send_signal(process['fd'], signal.SIGTERM)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        own_processes(launchers, retained, report)
        alive = [process for process in retained if not exited(process)]
        if not alive:
            break
        for process in alive:
            signal.pidfd_send_signal(process['fd'], signal.SIGTERM)
        time.sleep(.1)
    forced = False
    for process in retained:
        if not exited(process):
            forced = True
            signal.pidfd_send_signal(process['fd'], signal.SIGKILL)
        if exited(process, 5):
            mark_exit(report, process)
    for process in launchers:
        process.wait(timeout=5)
    own_processes(launchers, retained, report)
    verify.require(not forced and all(exited(process) for process in retained), 'Owned cleanup required force or left a live process')


def find_payloads(paths, expected, kind):
    found = {}
    for role, basename in [('desktop', 'localai-cowork-v2-desktop'), ('daemon', 'cowork-daemon'), ('browser_host', 'cowork-browser-host')]:
        matches = [path.resolve(strict=True) for path in paths if path.name == basename and path.is_file()]
        verify.require(len(set(matches)) == 1, 'Expected one exact installed ' + role + ' payload')
        found[role] = matches[0]
        if role == 'desktop':
            verify.desktop_payload_pin(found[role], expected[role], kind)
        else:
            verify.require(verify.pin(found[role]) == {key: expected[role][key] for key in ('size', 'sha256')},
                           'Installed sidecar payload hash mismatch')
    verify.require(found['desktop'].parent == found['daemon'].parent == found['browser_host'].parent,
                   'Installed sidecars must be beside the exact desktop')
    return found


def boot(profile, label, launcher_command, env, allowed_root, binding, output, report, launchers, retained):
    port_free()
    if not profile.exists():
        profile.mkdir(mode=0o700)
    log = output / (label + '-desktop.log')
    selected_env = {**env, 'COWORK_DATA_DIR': str(profile), 'COWORK_ENABLE_TEST_PROVIDER': '1'}
    args = [*launcher_command, '--cowork-data-dir', str(profile)]
    with log.open('xb') as stream:
        launcher = subprocess.Popen(args, env=selected_env, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
    launchers.append(launcher)
    report['launches'].append({'phase': label, 'command': args, 'launcher_pid': launcher.pid,
                              'profile': str(profile), 'mode': report['mode']})
    gui = None
    daemon = None
    desktop_identity = None
    api = api_client(profile)
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        verify.require(log.stat().st_size <= LOG_LIMIT, 'Desktop log exceeded bound')
        for entry in Path('/proc').iterdir():
            if not entry.name.isdigit():
                continue
            try:
                info = proc_info(int(entry.name))
                if info['session'] != launcher.pid or info['uid'] != os.getuid():
                    continue
                path = Path(f'/proc/{entry.name}/exe').resolve(strict=True)
                for role, basename in [('desktop', 'localai-cowork-v2-desktop'), ('daemon', 'cowork-daemon')]:
                    if path.name != basename or (role == 'desktop' and gui) or (role == 'daemon' and daemon):
                        continue
                    expected = binding['expected_payloads'][role]
                    if role == 'desktop':
                        verify.require(allowed_root == path.parent or allowed_root in path.parents,
                                       'Desktop escaped selected installation/extraction')
                        desktop_identity = verify.desktop_payload_pin(path, expected, report['kind'])
                        expected = desktop_identity['raw']
                    process = capture(int(entry.name), launcher.pid, profile=profile,
                        expected=expected, allowed_root=allowed_root)
                    retained.append(process)
                    record_process(report, process, label + '-' + role)
                    if role == 'desktop':
                        gui = process
                    else:
                        daemon = process
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                continue
        try:
            health = api('GET', '/health')
            verify.require(health.get('product') == 'LocalAI Cowork V2' and health.get('version') == binding['version']
                and health.get('schema_version') == 8 and health.get('deployment') == 'single_owner_preview'
                and 'personal_money_status' in health.get('capabilities', []), 'Installed health contract mismatch')
            if gui and daemon:
                browser = Path(daemon['image']).parent / 'cowork-browser-host'
                verify.require(verify.pin(browser) == {key: binding['expected_payloads']['browser_host'][key] for key in ('size', 'sha256')},
                               'Installed browser-host payload differs from original package receipt')
                if report['kind'] == 'appimage':
                    for process in (gui, daemon):
                        relative = Path(process['image']).relative_to(allowed_root)
                        extraction = allowed_root / relative.parts[0]
                        verify.require(extraction.name.startswith('appimage_extracted_') and extraction.is_dir()
                            and not extraction.is_symlink(), 'Original AppImage runtime extraction root is unrecognized')
                        info = extraction.stat()
                        report['extractions'][str(extraction)] = {'device': info.st_dev, 'inode': info.st_ino}
                report['verified_payloads'].append({'phase': label, 'desktop': gui['payload'],
                    'desktop_cargo_binding': desktop_identity['normalization'],
                    'daemon': daemon['payload'], 'browser_host': verify.pin(browser)})
                report['health'].append({'phase': label, 'value': health})
                own_processes(launchers, retained, report)
                return gui, daemon, api
        except (FileNotFoundError, urllib.error.URLError, TimeoutError, ConnectionError):
            pass
        time.sleep(.1)
    raise ValueError('Installed frontend did not start its exact matching daemon')


def durable_run(api, gui, report):
    project = api('POST', '/projects', {'name': 'Disposable installed package smoke'})
    order = api('POST', '/orders', {'project_id': project['id'], 'title': 'Installed lifetime'})
    provider = api('POST', '/providers', {'name': 'Slow deterministic fixture', 'kind': 'test',
        'endpoint': '', 'model': 'slow-test', 'context_tokens': 32768})
    request = {'order_id': order['id'], 'base_revision': 1, 'idempotency_key': str(uuid.uuid4()),
        'prompt': 'Create one deterministic artifact.', 'provider_id': provider['id'],
        'provider_revision': 1, 'access_mode': 'autonomous'}
    prepared = api('POST', '/execution-plans', request)
    plan = prepared['plan']
    verify.require(not prepared.get('retired') and plan['order_id'] == request['order_id']
        and plan['idempotency_key'] == request['idempotency_key'], 'Execution plan differs from original fixture request')
    run = api('POST', '/orders/' + urllib.parse.quote(order['id'], safe='') + '/execution-plans/'
        + urllib.parse.quote(plan['id'], safe='') + '/commit', {'sha256': prepared['sha256']})
    before = api('GET', '/runs/' + urllib.parse.quote(run['id'], safe=''))
    verify.require(before['run']['state'] not in FAILED_STATES | {'completed'}, 'Slow fixture ended before desktop stop')
    stop_ui(report, gui)
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        detail = api('GET', '/runs/' + urllib.parse.quote(run['id'], safe=''))
        verify.require(detail['run']['state'] not in FAILED_STATES, 'Accepted run failed after desktop exit')
        if detail['run']['state'] == 'completed':
            verify.require(len(detail['artifacts']) == 1, 'Expected exactly one durable artifact')
            report['durable_run'] = {'project_id': project['id'], 'run_id': run['id'], 'state_before_ui_exit': before['run']['state'],
                'state_after_ui_exit': 'completed', 'artifact_count': 1}
            return project['id'], run['id']
        time.sleep(.1)
    raise ValueError('Accepted fixture run did not finish after desktop physical exit')


def registration_snapshot(home):
    paths = [home / '.config/google-chrome/NativeMessagingHosts/dev.localai.cowork.browser.json',
             home / '.mozilla/native-messaging-hosts/dev.localai.cowork.browser.json']
    return {str(path): verify.pin(path) if path.exists() else None for path in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kind', choices=('deb', 'appimage'), required=True)
    for name in ('binding', 'original-dir', 'work-root', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    env = coordinator.hosted_guard()
    verify.require(env['RUNNER_OS'] == 'Linux' and os.name == 'posix' and os.geteuid() != 0
        and hasattr(os, 'pidfd_open') and hasattr(signal, 'pidfd_send_signal') and os.environ.get('DISPLAY'),
        'A non-root native hosted Linux user, display and PIDfd support are required')
    binding = verify.read_json(args.binding.absolute())
    verify.require(binding.get('format') == 'localai-cowork-unsigned-installer-smoke-input/1'
        and binding.get('platform') == 'linux' and binding.get('version') == '2.0.0-alpha.8'
        and all(binding.get(flag) is False for flag in ('signed', 'published', 'release_qualified')),
        'Exact reviewed Linux alpha.8 binding required')
    record = binding['packages'][args.kind]
    package = args.original_dir.absolute() / record['filename']
    verify.checked_pin(args.original_dir.absolute(), record)
    work = coordinator.runner_path(args.work_root)
    output = coordinator.runner_path(args.output)
    verify.require(work != output and work not in output.parents and output not in work.parents, 'Evidence must be outside work tree')
    work.mkdir(mode=0o700)
    output.mkdir(mode=0o700)
    report = {'format': 'localai-cowork-unsigned-installed-smoke/1', 'kind': args.kind,
        'result': 'failed', 'started_at': datetime.now(timezone.utc).isoformat(), 'binding': verify.pin(args.binding.absolute()),
        'package': record, 'mode': 'dpkg-installed' if args.kind == 'deb' else 'original-runtime-extract-and-run-no-cleanup',
        'commands': [], 'processes': [], 'launches': [], 'health': [], 'extractions': {}, 'verified_payloads': [],
        'signed': False, 'published': False, 'release_qualified': False,
        'full_installed_product_acceptance': 'pending', 'provider': 'deterministic; no real model or login'}
    launchers = []
    retained = []
    pkg_name = None
    installed_admitted = False
    removed = False
    payloads = None
    image = None
    home = work / 'home'
    home.mkdir(mode=0o700)
    temporary = work / 'extract'
    temporary.mkdir(mode=0o700)
    runtime = work / 'runtime'
    runtime.mkdir(mode=0o700)
    sentinel = home / '.config/synthetic-v1-profile/sentinel.bin'
    sentinel.parent.mkdir(parents=True)
    sentinel.write_bytes(b'synthetic-v1-sentinel-never-real-user-data\n')
    before_sentinel = verify.pin(sentinel)
    selected_env = {**coordinator.clean_environment(), 'HOME': str(home), 'XDG_CONFIG_HOME': str(home / '.config'),
        'XDG_DATA_HOME': str(home / '.local/share'), 'XDG_RUNTIME_DIR': str(runtime),
        'TMPDIR': str(temporary), 'GDK_BACKEND': 'x11', 'APPIMAGE_EXTRACT_AND_RUN': '1', 'NO_CLEANUP': '1'}
    try:
        port_free()
        libc = command(report, output, 'glibc', ['getconf', 'GNU_LIBC_VERSION']).strip()
        match = re.fullmatch(r'glibc (\d+)\.(\d+)', libc)
        verify.require(match and tuple(map(int, match.groups())) >= (2, 39), 'glibc >=2.39 is required')
        report['glibc'] = libc
        report['os'] = {'release': os.uname().release, 'version': os.uname().version, 'machine': os.uname().machine,
            'os_release': Path('/etc/os-release').read_text()[:8192]}
        command(report, output, 'runtime-dependencies', ['dpkg-query', '-W', 'libwebkit2gtk-4.1-0', 'libgtk-3-0t64', 'xvfb', 'xauth'])
        report['registrations_before'] = registration_snapshot(home)
        if args.kind == 'deb':
            fields = command(report, output, 'deb-control', ['dpkg-deb', '-f', str(package), 'Package', 'Version', 'Architecture']).splitlines()
            # Named multiple fields use "Field: value" output; do not assume filename/version syntax.
            control = dict(line.split(': ', 1) for line in fields)
            pkg_name = control.get('Package')
            verify.require(control.get('Architecture') == 'amd64' and control.get('Version'), 'Original amd64 DEB control required')
            commands = package_commands('deb', package, pkg_name)
            before = command(report, output, 'deb-before', ['dpkg-query', '-W', '-f=${db:Status-Status}', pkg_name], allowed=(0, 1))
            verify.require(report['commands'][-1]['exit_code'] == 1, 'The selected Debian package already has package-manager state')
            installed_admitted = True
            command(report, output, 'deb-install', commands['install'], timeout=300)
            after = command(report, output, 'deb-installed', ['dpkg-query', '-W', '-f=${db:Status-Status}\t${Version}', pkg_name]).strip()
            verify.require(after == 'installed\t' + control['Version'], 'Installed DEB version/status differs from original control')
            manifest = command(report, output, 'deb-file-manifest', ['dpkg-query', '-L', pkg_name]).splitlines()
            payloads = find_payloads([Path(line) for line in manifest if line.startswith('/')], binding['expected_payloads'], 'deb')
            launcher_command = [str(payloads['desktop'])]
            allowed_root = payloads['desktop'].parent
            report['installed_payloads'] = {role: {'path': str(path), **verify.pin(path)} for role, path in payloads.items()}
            report['deb_control'] = control
        else:
            image = work / 'original.AppImage'
            shutil.copyfile(package, image)
            verify.require(verify.pin(image) == {key: record[key] for key in ('size', 'sha256')}, 'AppImage copy changed bytes')
            image.chmod(0o700)
            launcher_command = package_commands('appimage', image)['launch']
            allowed_root = temporary
        first = work / 'profile-one'
        gui, daemon, api = boot(first, 'first', launcher_command, selected_env, allowed_root, binding, output, report, launchers, retained)
        project_id, run_id = durable_run(api, gui, report)
        shutdown_daemon(report, api, daemon, binding)
        gui, daemon, api = boot(first, 'reopen', launcher_command, selected_env, allowed_root, binding, output, report, launchers, retained)
        verify.require(any(item['id'] == project_id for item in api('GET', '/projects'))
            and api('GET', '/runs/' + urllib.parse.quote(run_id, safe=''))['run']['state'] == 'completed',
            'Installed same-profile relaunch lost durable synthetic state')
        stop_ui(report, gui)
        shutdown_daemon(report, api, daemon, binding)
        fresh = work / 'profile-two'
        gui, daemon, api = boot(fresh, 'fresh', launcher_command, selected_env, allowed_root, binding, output, report, launchers, retained)
        verify.require(not any(item['id'] == project_id for item in api('GET', '/projects')), 'Fresh profile exposed first-profile synthetic state')
        stop_ui(report, gui)
        shutdown_daemon(report, api, daemon, binding)
        report['profiles'] = {'same_profile_reopened': True, 'fresh_profile_isolated': True,
            'databases': {str(path.relative_to(work)): verify.pin(path) for path in [first / 'cowork-v2.sqlite3', fresh / 'cowork-v2.sqlite3']}}
        cleanup_processes(launchers, retained, report)
        report['registrations_after_launch'] = registration_snapshot(home)
        if args.kind == 'deb':
            command(report, output, 'deb-remove', package_commands('deb', package, pkg_name)['remove'], timeout=180)
            removed = True
            status = command(report, output, 'deb-removed-status', ['dpkg-query', '-W', '-f=${db:Status-Status}', pkg_name], allowed=(0, 1))
            verify.require(status.strip() != 'installed' and all(not path.exists() for path in payloads.values()),
                           'DEB removal left installed payloads or package status')
        else:
            verify.require(verify.pin(image) == {key: record[key] for key in ('size', 'sha256')}, 'Executed original image changed before removal')
            image.unlink()
            removed = True
        report['registrations_after_package_removal'] = registration_snapshot(home)
        # Capture actual package-removal behavior BEFORE explicit user-level cleanup.
        manifests = [Path(name) for name, value in report['registrations_after_package_removal'].items() if value is not None]
        if manifests:
            host_paths = {Path(verify.read_json(path)['path']) for path in manifests}
            verify.require(len(host_paths) == 1, 'Registered browser integration has ambiguous payload owner')
            host = next(iter(host_paths))
            verify.require(work in host.resolve(strict=True).parents
                and verify.pin(host) == {key: binding['expected_payloads']['browser_host'][key] for key in ('size', 'sha256')},
                'Browser cleanup executable is not this exact own immutable payload')
            command(report, output, 'explicit-user-unregister', [str(host), '--unregister'], timeout=30, env=selected_env)
        report['registrations_after_explicit_cleanup'] = registration_snapshot(home)
        verify.require(not any(report['registrations_after_explicit_cleanup'].values()), 'Explicit own browser unregister left manifests')
        report['synthetic_v1_sentinel'] = {'before': before_sentinel, 'after': verify.pin(sentinel), 'synthetic_only': True}
        verify.require(report['synthetic_v1_sentinel']['after'] == before_sentinel, 'Synthetic V1 sentinel changed')
        report['original_package_preserved'] = verify.pin(package) == {key: record[key] for key in ('size', 'sha256')}
        report['install_or_runtime_launch'] = 'passed'
        report['payload_removal'] = 'passed'
        report['result'] = 'passed'
    except BaseException as error:
        report['failure'] = type(error).__name__ + ': ' + str(error)
    finally:
        try:
            cleanup_processes(launchers, retained, report)
        except BaseException as error:
            report['result'] = 'failed'
            report['cleanup_failure'] = type(error).__name__ + ': ' + str(error)
        if installed_admitted and not removed:
            try:
                command(report, output, 'failure-deb-remove', package_commands('deb', package, pkg_name)['remove'], timeout=180)
            except BaseException as error:
                report['cleanup_package_failure'] = type(error).__name__ + ': ' + str(error)
        for process in retained:
            os.close(process['fd'])
        if args.kind == 'appimage' and all(item['physical_exit'] for item in report['processes']):
            try:
                for name, expected in report['extractions'].items():
                    extraction = Path(name)
                    info = extraction.lstat()
                    verify.require(extraction.is_dir() and not extraction.is_symlink()
                        and extraction.resolve(strict=True).parent == temporary.resolve(strict=True)
                        and {'device': info.st_dev, 'inode': info.st_ino} == expected,
                        'Captured extraction identity changed before cleanup')
                    shutil.rmtree(extraction)
                if image is not None and image.exists():
                    verify.require(verify.pin(image) == {key: record[key] for key in ('size', 'sha256')},
                                   'Own AppImage changed before failure cleanup')
                    image.unlink()
                report['own_extraction_cleanup'] = 'passed'
            except BaseException as error:
                report['result'] = 'failed'
                report['cleanup_extraction_failure'] = type(error).__name__ + ': ' + str(error)
        report['finished_at'] = datetime.now(timezone.utc).isoformat()
        report['evidence'] = {path.name: verify.pin(path) for path in output.iterdir() if path.is_file()}
        coordinator.write_json(output / 'smoke.json', report)
    print(json.dumps({'result': report['result'], 'kind': args.kind, 'evidence': str(output), 'release_qualified': False}))
    if report['result'] != 'passed':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
