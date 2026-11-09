"""Prepare or explicitly launch a disposable Windows VM using an existing V2 pairing.

No new queue or host-native execution path is created. The guest starts the same
device agent and consumes the existing protocol-2 queue. Host grants are explicit.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import xml.etree.ElementTree as ET
from urllib.parse import urlsplit

import configure

GUEST_ROOT = r'C:\Cowork'
BOOTSTRAP = r'C:\CoworkBootstrap'
SUPPORTED = {'terminal', 'tool.terminal', 'desktop.windows', 'model.chat', 'model.external'}


def powershell_path():
    return pathlib.Path(os.environ.get('SystemRoot', r'C:\Windows')) / 'System32/WindowsPowerShell/v1.0/powershell.exe'


def discovery():
    executable = pathlib.Path(os.environ.get('SystemRoot', r'C:\Windows')) / 'System32/WindowsSandbox.exe'
    return {'format': 'cowork-windows-sandbox-discovery/1', 'host_supported': os.name == 'nt',
            'launcher_installed': os.name == 'nt' and executable.is_file(),
            'executable': str(executable) if os.name == 'nt' and executable.is_file() else None,
            'runtime_qualified': False, 'ephemeral': True, 'max_concurrent_instances': 1,
            'host_apps_available': False, 'requires': ['supported_windows_edition', 'enabled_virtualization',
                                                       'explicit_folder_grants', 'separate_device_pairing']}


def overlaps(left, right):
    return left == right or left in right.parents or right in left.parents


def grants(read_only, read_write, excluded):
    result = []
    for writable, values in [(False, read_only), (True, read_write)]:
        for value in values:
            path = configure.selected_path(value, directory=True).resolve(strict=True)
            configure.require(path != pathlib.Path(path.anchor) and '%' not in str(path)
                              and not any(overlaps(path, other) for other in excluded),
                              'Do not map drive roots, variable paths, pairing credentials or the bootstrap directory')
            configure.require(not any(overlaps(path, pathlib.Path(item['host'])) for item in result),
                              'Folder grants must not overlap')
            count = 0
            for base, directories, files in os.walk(path, followlinks=False):
                for name in directories + files:
                    count += 1
                    configure.require(count <= 100000, 'Folder grant exceeds the bounded alias check; select a smaller folder')
                    child = pathlib.Path(base) / name
                    metadata = child.lstat()
                    configure.require(not child.is_symlink() and not getattr(metadata, 'st_file_attributes', 0) & 0x400,
                                      'Mapped folders must not contain symbolic links or reparse points')
            result.append({'host': str(path), 'guest': r'C:\CoworkShares\folder-' + str(len(result) + 1).zfill(3),
                           'read_only': not writable})
    configure.require(len(result) <= 16, 'Select at most 16 folder grants')
    return result


def configuration_xml(payload, folder_grants, network=False, memory_mb=4096):
    configure.require(type(memory_mb) is int and 2048 <= memory_mb <= 32768, 'VM memory must be 2048–32768 MiB')
    root = ET.Element('Configuration')
    for name, value in [('Networking', 'Enable' if network else 'Disable'), ('ClipboardRedirection', 'Disable'),
                        ('AudioInput', 'Disable'), ('VideoInput', 'Disable'), ('PrinterRedirection', 'Disable'),
                        ('vGPU', 'Disable'), ('MemoryInMB', str(memory_mb))]:
        ET.SubElement(root, name).text = value
    folders = ET.SubElement(root, 'MappedFolders')
    for item in [{'host': str(payload), 'guest': BOOTSTRAP, 'read_only': True}, *folder_grants]:
        mapped = ET.SubElement(folders, 'MappedFolder')
        ET.SubElement(mapped, 'HostFolder').text = item['host']
        ET.SubElement(mapped, 'SandboxFolder').text = item['guest']
        ET.SubElement(mapped, 'ReadOnly').text = 'true' if item['read_only'] else 'false'
    logon = ET.SubElement(root, 'LogonCommand')
    ET.SubElement(logon, 'Command').text = (r'powershell.exe -NoProfile -ExecutionPolicy Bypass -NoExit -File '
                                           + BOOTSTRAP + r'\bootstrap.ps1')
    return ET.tostring(root, encoding='unicode') + '\n'


BOOTSTRAP_SCRIPT = r'''$ErrorActionPreference = 'Stop'
$source = 'C:\CoworkBootstrap'
$root = 'C:\Cowork'
New-Item -ItemType Directory -Path $root | Out-Null
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
function Protect-Private([string]$path, [bool]$directory) {
    if ($directory) { $acl = New-Object Security.AccessControl.DirectorySecurity; $flags = 'OICI' }
    else { $acl = New-Object Security.AccessControl.FileSecurity; $flags = '' }
    $acl.SetSecurityDescriptorSddlForm(('O:' + $identity + 'D:P(A;' + $flags + ';FA;;;' + $identity + ')(A;' + $flags + ';FA;;;SY)'))
    Set-Acl -LiteralPath $path -AclObject $acl
}
Protect-Private $root $true
foreach ($name in @('state', 'toolbox')) { $path = Join-Path $root $name; New-Item -ItemType Directory -Path $path | Out-Null; Protect-Private $path $true }
foreach ($name in @('agent.exe', 'agent.json', 'agent-token.txt', 'toolbox.json', 'consent.ps1', 'folder-grants.json', 'model-key.txt')) {
    $inputPath = Join-Path $source $name
    if (Test-Path -LiteralPath $inputPath) {
        $target = if ($name -eq 'toolbox.json') { Join-Path $root 'toolbox\toolbox.json' } else { Join-Path $root $name }
        Copy-Item -LiteralPath $inputPath -Destination $target
        Protect-Private $target $false
    }
}
& (Join-Path $root 'agent.exe') --config (Join-Path $root 'agent.json') --check
if ($LASTEXITCODE -ne 0) { throw 'Guest capability validation failed. No agent was started.' }
Write-Host 'Disposable Windows test computer. Host applications are not available. Closing this Sandbox discards guest state.'
Get-Content -LiteralPath (Join-Path $root 'folder-grants.json') -Encoding UTF8
& (Join-Path $root 'consent.ps1')
'''

CONSENT_SCRIPT = r'''$ErrorActionPreference = 'Stop'
$root = 'C:\Cowork'
$agent = Start-Process -FilePath (Join-Path $root 'agent.exe') -ArgumentList @('--config', (Join-Path $root 'agent.json')) -PassThru -NoNewWindow
$seen = @{}
Write-Host 'This paired guest uses existing device tasks. Inspect each exact action before typing approve or reject.'
while (-not $agent.HasExited) {
    $directory = Join-Path $root 'state\local-consent'
    if (Test-Path -LiteralPath $directory) {
        foreach ($file in Get-ChildItem -LiteralPath $directory -Filter '*.request.json' -File) {
            $request = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
            $id = [guid]::Parse($request.request_id).ToString()
            if ($file.Name -ne ($id + '.request.json') -or $seen.ContainsKey($id)) { continue }
            if ([DateTimeOffset]::Parse($request.expires_at) -le [DateTimeOffset]::UtcNow) { continue }
            Write-Host ('Run: ' + $request.run_id + ' / request: ' + $id)
            $request.action | ConvertTo-Json -Depth 50 | Write-Host
            $original = $request.action.original_action
            if ($original) {
                if ($original.local_file -ne ($id + '.original.json') -or $original.bytes -le 0 -or $original.bytes -gt 4194304) { throw 'Original action reference invalid; approval blocked.' }
                $originalPath = Join-Path $directory $original.local_file
                $originalFile = Get-Item -LiteralPath $originalPath
                if ($originalFile.Length -ne $original.bytes -or (Get-FileHash -LiteralPath $originalPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $original.sha256) { throw 'Original action changed; approval blocked.' }
                Get-Content -LiteralPath $originalPath -Raw -Encoding UTF8 | Write-Host
            }
            $decision = Read-Host 'Type approve or reject (anything else rejects)'
            if ($decision -ne 'approve') { $decision = 'reject' }
            & (Join-Path $root 'agent.exe') consent --data (Join-Path $root 'state') --request $id --decision $decision
            if ($LASTEXITCODE -eq 0) { $seen[$id] = $true }
        }
    }
    Start-Sleep -Milliseconds 750
    $agent.Refresh()
}
'''


def prepare(args):
    configure.require(os.name == 'nt', 'Prepare a Windows test computer on its Windows host')
    profile_path = configure.selected_path(args.profile)
    profile = configure.read_json(profile_path)
    _, token, registration = configure.profile_configuration(profile)
    configure.require(registration['kind'] == 'personal_device'
                      and registration.get('personal_device_remote_control') in {'off', 'confirm_each_session'},
                      'Pair a separate personal test device with remote control off or confirm_each_session')
    configure.require(registration['executor_id'] != args.host_executor_id,
                      'The VM must use its own device pairing, never the running host executor identity')
    names = {item['name'] for item in registration['capabilities']}
    configure.require(names <= SUPPORTED, 'Pair only terminal, Windows desktop and optional external-model capabilities for this clean guest')
    configure.require(profile['server_url'].startswith('https://'), 'The guest requires a reachable HTTPS server; host localhost is not guest localhost')
    configure.require(args.network, 'The paired device transport requires explicit --network authorization')
    configure.require(args.allow_guest_tools or not names.intersection({'terminal', 'tool.terminal', 'desktop.windows'}),
                      'Guest native tools require explicit --allow-guest-tools; host rights are never granted')
    binary = configure.selected_path(args.agent_binary)
    configure.require(binary.read_bytes()[:2] == b'MZ', 'Supply the native Windows V2 agent executable')
    output = configure.selected_path(args.output, exists=False)
    configure.require(not output.exists() and '%' not in str(output), 'Select a new private bootstrap directory without environment variables')
    excluded = [output, profile_path, binary]
    if args.model_key_file:
        excluded.append(configure.selected_path(args.model_key_file))
    folders = grants(args.read_only, args.read_write, excluded)
    model = None
    model_key = None
    if args.model_url or args.model_name:
        endpoint = urlsplit(args.model_url or '')
        configure.require(endpoint.scheme == 'https' and endpoint.hostname and not endpoint.username
                          and not endpoint.password and not endpoint.query and not endpoint.fragment and args.model_name,
                          'Select an explicit HTTPS model endpoint and model name for this guest')
        configure.plain_text(args.model_name, 200, 'guest model name')
        model_key = configure.read_secret(args.model_key_file, 'guest model key') if args.model_key_file else None
        model = {'base_url': args.model_url, 'model_name': args.model_name, 'api_key_file': GUEST_ROOT + r'\model-key.txt' if model_key else None,
                 'api': 'openai', 'implementation': 'openai_compatible'}
    configure.require(not args.model_key_file or model is not None, 'A model key requires its explicit guest model endpoint')
    configure.require(not names.intersection({'model.chat', 'model.external'}) or model is not None,
                      'The paired model capability needs its explicit guest model configuration')
    config = {'format': 1, 'server_url': profile['server_url'], 'registration': registration,
              'agent_token_file': GUEST_ROOT + r'\agent-token.txt', 'model': model,
              'workspace_root': GUEST_ROOT + r'\state', 'toolbox_data_dir': GUEST_ROOT + r'\toolbox',
              'mcp_bindings_file': None, 'github_token_file': None, 'rfb_endpoint': None}
    toolbox = {'programs': {'powershell': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'} if args.allow_guest_tools else {},
               'native_owner_rights': bool(args.allow_guest_tools)}
    configure.create_directory(output)
    files = {'agent.json': json.dumps(config, indent=2), 'agent-token.txt': token + '\n',
             'toolbox.json': json.dumps(toolbox, indent=2), 'bootstrap.ps1': BOOTSTRAP_SCRIPT,
             'consent.ps1': CONSENT_SCRIPT, 'folder-grants.json': json.dumps(folders, indent=2),
             'test-computer.wsb': configuration_xml(output, folders, args.network, args.memory_mb)}
    for name, value in files.items():
        configure.create_file(output / name, value)
    if model_key:
        configure.create_file(output / 'model-key.txt', model_key + '\n')
    configure.windows_helper().create_file(output / 'agent.exe', binary.read_bytes())
    payload_names = [*files, 'agent.exe'] + (['model-key.txt'] if model_key else [])
    manifest = {'format': 'cowork-windows-sandbox-launch/1', 'executor_id': registration['executor_id'],
                'host_executor_id': args.host_executor_id, 'folders': folders, 'network': bool(args.network),
                'ephemeral': True, 'runtime_qualified': False, 'agent_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'payload_sha256': {name: hashlib.sha256((output / name).read_bytes()).hexdigest() for name in payload_names}}
    configure.create_file(output / 'launch-manifest.json', json.dumps(manifest, indent=2) + '\n')
    return output


def launch(output):
    availability = discovery()
    configure.require(availability['launcher_installed'], 'Windows Sandbox is unavailable; enable it on a supported Windows edition with virtualization')
    manifest = configure.read_json(output / 'launch-manifest.json')
    configure.require(manifest.get('format') == 'cowork-windows-sandbox-launch/1', 'Invalid VM launch manifest')
    for name, expected in manifest['payload_sha256'].items():
        configure.require(name in {'agent.json', 'agent-token.txt', 'toolbox.json', 'bootstrap.ps1', 'consent.ps1',
                                   'folder-grants.json', 'test-computer.wsb', 'agent.exe', 'model-key.txt'}, 'Invalid VM payload path')
        path = configure.selected_path(str(output / name))
        configure.require(hashlib.sha256(path.read_bytes()).hexdigest() == expected, 'VM payload changed; prepare a fresh launch bundle')
    grants([item['host'] for item in manifest['folders'] if item['read_only']],
           [item['host'] for item in manifest['folders'] if not item['read_only']], [output])
    return subprocess.Popen([availability['executable'], str(output / 'test-computer.wsb')], shell=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--discover', action='store_true')
    for name in ('profile', 'agent-binary', 'output', 'host-executor-id', 'model-url', 'model-name', 'model-key-file'):
        parser.add_argument('--' + name)
    parser.add_argument('--read-only', action='append', default=[])
    parser.add_argument('--read-write', action='append', default=[])
    parser.add_argument('--network', action='store_true', help='Permit unrestricted guest networking, including local networks; required for device transport')
    parser.add_argument('--allow-guest-tools', action='store_true', help='Permit selected PowerShell/desktop tools inside this guest only')
    parser.add_argument('--memory-mb', type=int, default=4096)
    parser.add_argument('--launch', action='store_true', help='Explicitly launch the newly prepared VM')
    args = parser.parse_args()
    if args.discover:
        print(json.dumps(discovery(), indent=2)); return
    configure.require(all([args.profile, args.agent_binary, args.output, args.host_executor_id]),
                      'Select the separate guest pairing profile, native Windows agent, new output directory and current host executor ID')
    configure.uuid_value(args.host_executor_id, 'host executor ID')
    output = prepare(args)
    if args.launch:
        launch(output)
    print(json.dumps({'prepared': True, 'started': args.launch, 'output': str(output), 'runtime_qualified': False,
                      'ephemeral': True, 'host_rights_granted': False}))


if __name__ == '__main__':
    try:
        main()
    except (configure.ConfigurationError, OSError, ValueError, KeyError) as error:
        print('Windows test computer refused: ' + (str(error) if isinstance(error, configure.ConfigurationError) else 'Local configuration could not be verified'), file=sys.stderr)
        raise SystemExit(1)
