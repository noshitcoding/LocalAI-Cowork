"""Prepare a private V2 runner installation; never start/install a global service.

Run from the unpacked native release. Generates reviewed explicit service install
scripts, private authority and an optional digest-pinned environment registry.
"""
import argparse
import hashlib
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from urllib.parse import urlsplit


def require(condition, message):
    if not condition:
        raise ValueError(message)


def ordinary(path, directory=False):
    path = Path(path)
    require(path.is_absolute() and not str(path).startswith(('\\\\', '//')), 'Absolute local path required')
    for parent in (*reversed(path.parents), path):
        info = parent.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400, 'Linked/reparse path refused')
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode), 'Ordinary path required')
    require(directory or info.st_nlink == 1, 'Hardlinked input refused')
    return path


def windows():
    source = ordinary(Path(os.path.abspath(__file__)).with_name('windows_private.py'))
    spec = importlib.util.spec_from_file_location('cowork_runner_private', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def mkdir(path):
    ordinary(path.parent, True)
    if os.name == 'nt':
        windows().create_directory(path)
    else:
        os.mkdir(path, 0o700)


def write(path, data, executable=False):
    ordinary(path.parent, True)
    data = data.encode() if isinstance(data, str) else data
    if os.name == 'nt':
        windows().create_file(path, data)
    else:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
        fd = os.open(path, flags, 0o700 if executable else 0o600)
        with os.fdopen(fd, 'wb') as target:
            target.write(data)
            target.flush()
            os.fsync(target.fileno())
        sync(path.parent)


def sync(path):
    if os.name != 'nt':
        fd = os.open(path, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def read(path, maximum):
    path = ordinary(path)
    size = path.stat().st_size
    require(0 < size <= maximum, 'Selected file exceeds the byte limit')
    with path.open('rb') as source:
        data = source.read(maximum + 1)
    require(len(data) == size, 'Input changed while copying')
    return data


def sha(data):
    return hashlib.sha256(data).hexdigest()


def json_read(path, maximum=65536):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, 'Duplicate JSON field')
            result[key] = value
        return result
    return json.loads(read(path, maximum), object_pairs_hook=pairs)


def copy_tree(source, target):
    ordinary(source, True)
    pending = [(source, '')]
    manifest = {}
    total = 0
    while pending:
        parent, prefix = pending.pop()
        for path in sorted(parent.iterdir()):
            name = (prefix + '/' if prefix else '') + path.name
            require(len(name) <= 1024 and not any(c in name for c in '\\:\0') and all(p not in {'', '.', '..'} for p in name.split('/')), 'Invalid bundle entry')
            directory = path.is_dir()
            ordinary(path, directory)
            require(len(manifest) < 10000, 'Bundle path limit exceeded')
            if directory:
                manifest[name] = {'sha256': sha(b''), 'bytes': 0, 'directory': True}
                pending.append((path, name))
            else:
                # Empty regular files are valid bundle entries.
                size = path.stat().st_size
                require(size <= 64 * 1024 * 1024, 'Bundle file exceeds 64 MiB')
                data = path.read_bytes()
                require(len(data) == size, 'Bundle changed while copying')
                total += size
                require(total <= 512 * 1024 * 1024, 'Bundle exceeds 512 MiB')
                manifest[name] = {'sha256': sha(data), 'bytes': size}
    digest = sha(json.dumps(manifest, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode())
    destination = target / digest
    require(not destination.exists(), 'Duplicate selected bundle')
    mkdir(destination)
    for name, entry in sorted(manifest.items()):
        path = destination / name
        if entry.get('directory'):
            mkdir(path)
        else:
            source_path = ordinary(source / name)
            data = source_path.read_bytes()
            require(len(data) == entry['bytes'] and sha(data) == entry['sha256'], 'Bundle changed after manifest')
            write(path, data)
    return digest, destination


def powershell(value):
    require(not any(c in value for c in '\0\r\n'), 'Control character in installation path')
    return "'" + value.replace("'", "''") + "'"


def systemd(value):
    require(not any(c in value for c in '\0\r\n'), 'Control character in installation path')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$','$$') + '"'


def service_files(output, receiver, configuration, workspace):
    if os.name == 'nt':
        name = 'CoworkV2Runner-' + sha(str(output).encode())[:12]
        script = '''$ErrorActionPreference = 'Stop'
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run installation as the same operator with administrative privileges; VHD and volume flush need them.' }
$name = NAME
if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { throw 'An existing task is never overwritten. Uninstall that exact task first.' }
$action = New-ScheduledTaskAction -Execute RECEIVER -Argument ARGUMENTS -WorkingDirectory WORKING
$trigger = New-ScheduledTaskTrigger -AtStartup
$owner = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType S4U -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $owner -Settings $settings -Description 'Standalone V2 isolated process receiver; private authority, no V1 IPC' | Out-Null
Start-ScheduledTask -TaskName $name
'''
        values = {'NAME': powershell(name), 'RECEIVER': powershell(str(receiver)),
                  'ARGUMENTS': powershell(subprocess.list2cmdline(['serve', str(configuration)])), 'WORKING': powershell(str(output))}
        for key, value in values.items():
            script = script.replace(key, value)
        write(output / 'install-service.ps1', script)
        write(output / 'uninstall-service.ps1', "$ErrorActionPreference = 'Stop'\n$name = " + powershell(name) + "\nStop-ScheduledTask -TaskName $name\nUnregister-ScheduledTask -TaskName $name -Confirm:$false\n# Private authority and execution evidence remain intact.\n")
    else:
        name = 'cowork-v2-runner-' + sha(str(output).encode())[:12] + '.service'
        unit = '[Unit]\nDescription=Standalone V2 isolated process receiver\nAfter=network.target\n\n[Service]\nType=simple\nExecStart=' + systemd(str(receiver)) + ' serve ' + systemd(str(configuration)) + '\nWorkingDirectory=' + systemd(str(output)) + '\nRestart=always\nRestartSec=3\nTimeoutStopSec=50\nKillMode=mixed\nUMask=0077\nNoNewPrivileges=true\n\n[Install]\nWantedBy=default.target\n'
        write(output / name, unit)
        installer = '''#!/usr/bin/env python3
import os,pathlib,shutil,subprocess,sys
source=pathlib.Path(__file__).resolve().parent / UNIT
target=pathlib.Path.home()/'.config/systemd/user'/UNIT
if target.exists() or target.is_symlink(): raise SystemExit('Existing service is never overwritten')
target.parent.mkdir(parents=True,exist_ok=True)
for p in (target.parent,*target.parent.parents):
    if p.is_symlink(): raise SystemExit('Linked service directory refused')
with target.open('xb') as f: f.write(source.read_bytes());f.flush();os.fsync(f.fileno())
subprocess.run(['systemctl','--user','daemon-reload'],check=True)
subprocess.run(['systemctl','--user','enable','--now',UNIT],check=True)
print('User service installed. For boot without login, explicitly enable linger for this dedicated operator using your system administrator.')
'''.replace('UNIT', repr(name))
        write(output / 'install-service.py', installer, True)
        write(output / 'uninstall-service.py', "import subprocess,pathlib\nname=" + repr(name) + "\nsubprocess.run(['systemctl','--user','disable','--now',name],check=True)\np=pathlib.Path.home()/'.config/systemd/user'/name\nif p.is_symlink():raise SystemExit('Linked service refused')\np.unlink()\nsubprocess.run(['systemctl','--user','daemon-reload'],check=True)\n# Private state and execution evidence are retained.\n", True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, help='Absolute NEW dedicated private installation directory; parent must exist')
    parser.add_argument('--workspace-state', required=True, help='Exact existing FileBroker state directory used by the daemon; same OS operator')
    parser.add_argument('--tls-cert', required=True, help='Existing PEM certificate chain for this receiver hostname')
    parser.add_argument('--tls-key', required=True, help='Existing PEM PKCS8 private TLS key, copied privately')
    parser.add_argument('--public-origin', required=True, help='Canonical HTTPS origin trusted by the daemon')
    parser.add_argument('--listen', default='127.0.0.1:7439', help='Explicit IP:port (bracket IPv6); default loopback')
    parser.add_argument('--docker', help='Absolute existing Docker executable on Linux, using actual runsc runtime and quota-capable overlay2')
    parser.add_argument('--windows-bundle', action='append', default=[], help='Existing ordinary complete Windows EXE/runtime bundle directory, copied privately; repeatable')
    parser.add_argument('--linux-image', action='append', default=[], help='Pinned image registry/name@sha256:digest; generates an explicit image-install script, never pulls now')
    parser.add_argument('--retention-seconds', type=int, default=604800)
    parser.add_argument('--max-retained-attempts', type=int, default=256)
    args = parser.parse_args()
    output = Path(args.output)
    require(output.is_absolute() and not output.exists() and not output.is_symlink(), 'Select a new absolute private installation directory')
    ordinary(output.parent, True)
    workspace = ordinary(Path(args.workspace_state), True)
    origin = urlsplit(args.public_origin)
    require(origin.scheme == 'https' and origin.hostname and not origin.username and not origin.password and not origin.query and not origin.fragment and origin.path in {'', '/'}, 'Canonical HTTPS origin required')
    origin.port
    host, port = args.listen.rsplit(':', 1)
    ipaddress.ip_address(host.strip('[]'))
    require(1 <= int(port) <= 65535, 'Invalid listener port')
    require(3600 <= args.retention_seconds <= 90 * 86400 and 8 <= args.max_retained_attempts <= 10000, 'Invalid retention bounds')
    require(not args.windows_bundle or os.name == 'nt', 'Register Windows bundles on the actual Windows receiver')
    require(not args.linux_image or os.name != 'nt', 'Linux images need the actual Linux receiver')
    require(len(args.windows_bundle) <= 32 and len(args.linux_image) <= 32, 'Environment registration limit exceeded')
    images = []
    for image in args.linux_image:
        require(re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._/:\-]*@sha256:[0-9a-f]{64}', image), 'Use a pinned image digest')
        require(image not in images, 'Duplicate image digest')
        images.append(image)
    docker = ordinary(Path(args.docker)) if args.docker else None
    require(os.name == 'nt' or docker is not None, 'Linux setup requires the explicitly selected Docker executable')
    tls_certificate = read(Path(args.tls_cert), 1024 * 1024)
    tls_key = read(Path(args.tls_key), 65536)
    require(b'-----BEGIN CERTIFICATE-----' in tls_certificate and b'-----BEGIN PRIVATE KEY-----' in tls_key, 'PEM certificate and PKCS8 key required')
    package = Path(os.path.abspath(__file__)).parent
    manifest = json_read(package / 'MANIFEST.json')
    platform = 'windows_x64' if os.name == 'nt' else 'linux_x64'
    require(manifest.get('format') == 1 and manifest.get('kind') == 'standalone_v2_process_runner' and manifest.get('platform') == platform, 'Use the matching native release package')
    suffix = '.exe' if os.name == 'nt' else ''
    selected = {}
    for name in ('cowork-process-runner', 'cowork-runner-probe'):
        file = name + suffix
        data = read(package / file, 512 * 1024 * 1024)
        expected = manifest.get('files', {}).get(file, {})
        require(expected.get('bytes') == len(data) and expected.get('sha256') == sha(data), 'Release executable differs from package manifest')
        selected[file] = data
    mkdir(output)
    binary_dir = output / 'bin'
    mkdir(binary_dir)
    for file, data in selected.items():
        write(binary_dir / file, data, True)
    write(output / 'tls-cert.pem', tls_certificate)
    write(output / 'tls-key.pem', tls_key)
    data_dir = output / 'state'
    mkdir(data_dir)
    receiver = binary_dir / ('cowork-process-runner' + suffix)
    command = [str(receiver), 'init', '--data', str(data_dir), '--workspace-state', str(workspace), '--tls-cert', str(output / 'tls-cert.pem'), '--tls-key', str(output / 'tls-key.pem'), '--probe', str(binary_dir / ('cowork-runner-probe' + suffix)), '--listen', args.listen, '--public-origin', args.public_origin.rstrip('/')]
    if docker:
        command += ['--docker', str(docker)]
    subprocess.run(command, check=True, capture_output=True)
    config = json_read(data_dir / 'runner.json')
    config['retention_seconds'] = args.retention_seconds
    config['max_retained_attempts'] = args.max_retained_attempts
    hashes = []
    if args.windows_bundle:
        bundles = output / 'bundles'
        mkdir(bundles)
        for source in args.windows_bundle:
            digest, path = copy_tree(ordinary(Path(source), True), bundles)
            config['windows_bundles'][digest] = str(path)
            hashes.append(digest)
    replacement = data_dir / 'runner.updated.json'
    write(replacement, json.dumps(config, ensure_ascii=False, indent=2) + '\n')
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        move = ctypes.WinDLL('kernel32', use_last_error=True).MoveFileExW
        move.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD]
        move.restype = wintypes.BOOL
        require(move(str(replacement), str(data_dir / 'runner.json'), 0x1 | 0x8), 'Private configuration durable replacement failed')
    else:
        os.replace(replacement, data_dir / 'runner.json')
    sync(data_dir)
    service_files(output, receiver, data_dir / 'runner.json', workspace)
    if images:
        installer = 'import subprocess,json\ndocker=' + repr(str(docker)) + '\nimages=' + repr(images) + '''
for image in images:
    subprocess.run([docker,'image','pull',image],check=True)
    result=subprocess.run([docker,'image','inspect',image],check=True,capture_output=True,text=True)
    records=json.loads(result.stdout)
    if len(records)!=1 or image not in records[0].get('RepoDigests',[]):raise SystemExit('Installed digest differs')
print('Images installed. Physical qualification still runs separately through the environment API/UI.')
'''
        write(output / 'install-images.py', installer, True)
    write(output / 'installation.json', json.dumps({'format': 1, 'application_version':manifest.get('application_version'), 'runner_protocol_version':manifest.get('runner_protocol_version'), 'platform': platform, 'receiver_sha256': sha(selected['cowork-process-runner' + suffix]), 'probe_sha256': sha(selected['cowork-runner-probe' + suffix]), 'windows_bundle_sha256': hashes, 'linux_images': images, 'workspace_state': str(workspace), 'service_installed': False, 'qualification_performed': False}, indent=2) + '\n')
    print('Private receiver installation prepared: ' + str(output))
    print('Daemon COWORK_PROCESS_RUNNER_CONFIG=' + str(data_dir / 'daemon-runner.json'))
    if hashes:
        print('Private Windows bundle manifest digests: ' + ', '.join(hashes))
    print('Review and explicitly run the generated service/image installation script. No service, image, network connection or qualification was started.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError):
        print('Runner setup refused. Any newly created directory remains private; inspect it before using a new output path. No existing installation was overwritten.', file=sys.stderr)
        raise SystemExit(1)
