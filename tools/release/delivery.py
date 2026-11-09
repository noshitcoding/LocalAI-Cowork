"""Assemble current native desktop, server and agent builds without running them.

Every input must match the current source and its original successful build
receipt. Public receipts contain curated fields; original machine paths stay in
the private preparation directories. No test, install, signing or upload occurs.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/desktop'))
import release
import package as desktop
import frontend

HOSTS = {'windows': 'x86_64-pc-windows-msvc', 'linux': 'x86_64-unknown-linux-gnu'}
TREES = ('.github', 'apps', 'contracts', 'crates', 'deploy', 'docs', 'packages', 'services', 'tests', 'tools')
EXCLUDED = {'.git', '.local', '.reference', '.evidence', 'node_modules', 'target', 'dist', '__pycache__', 'binaries', 'gen'}
PRIVATE = {'.key', '.pem', '.pfx', '.p12', '.token'}
BINARIES = {'.exe', '.dll', '.pdb', '.so', '.dylib', '.rlib', '.rmeta', '.a', '.o', '.obj', '.bin', '.pyc', '.pyo', '.log', '.zip', '.gz', '.deb', '.appimage', '.msi'}


def name(value: str) -> str:
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or path.as_posix() != value or '\\' in value
            or any(part in ('', '.', '..') for part in path.parts)
            or any(char in value for char in '\r\n:')):
        raise release.Refused('ordinary portable relative names are required')
    return value


def pin(path: Path) -> dict:
    path = release.regular(path)
    return {'size': path.stat().st_size, 'sha256': release.sha256(path)}


def read(path: Path) -> dict:
    path = release.regular(path)
    if path.stat().st_size > 8 * 1024 * 1024:
        raise release.Refused('receipt exceeds its bound')
    return desktop.strict_json(path.read_bytes())


def tree(directory: Path) -> dict:
    desktop.directory(directory)
    result = {}
    for parent, directories, files in os.walk(directory, followlinks=False):
        for entry in directories:
            desktop.directory(Path(parent) / entry)
        for entry in files:
            path = Path(parent) / entry
            result[name(path.relative_to(directory).as_posix())] = pin(path)
    return dict(sorted(result.items()))


def sources() -> dict:
    records = release.source_manifest(ROOT)
    paths = {ROOT / item for item in ('README.md', 'LICENSE', 'package.json', 'package-lock.json', 'tsconfig.json')}
    fixtures = tuple(PurePosixPath(item) for item in release.HISTORICAL_DATA_DIRECTORIES)
    for folder in TREES:
        for parent, directories, files in os.walk(ROOT / folder, followlinks=False):
            directories[:] = sorted(item for item in directories if item not in EXCLUDED)
            for entry in directories:
                desktop.directory(Path(parent) / entry)
            for entry in files:
                path = Path(parent) / entry
                relative = PurePosixPath(path.relative_to(ROOT).as_posix())
                fixture = any(relative.is_relative_to(item) for item in fixtures)
                suffix = path.suffix.lower()
                if (suffix in PRIVATE or suffix in BINARIES or entry == '.env'
                        or entry.startswith('.env.') and not entry.endswith('.example')
                        or entry.endswith(('-wal', '-shm'))
                        or suffix in {'.db', '.sqlite', '.sqlite3'} and not fixture
                        or entry in {'daemon.token', 'native.token', 'toolbox.json'}):
                    continue
                paths.add(path)
    for path in sorted(paths):
        records[name(path.relative_to(ROOT).as_posix())] = pin(path)
    if len(records) > 16384 or sum(item['size'] for item in records.values()) > 512 * 1024 * 1024:
        raise release.Refused('source delivery exceeds its bounded closure')
    return dict(sorted(records.items()))


def copy(source: Path, target: Path, expected: dict) -> None:
    if pin(source) != expected:
        raise release.Refused('original input changed before copying')
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)
    shutil.copymode(source, target)
    if pin(source) != expected or pin(target) != expected:
        raise release.Refused('original bytes changed during copying')


def write(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('x', encoding='utf-8', newline='\n') as output:
        output.write(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + '\n')


def public(receipt: dict, keys: tuple[str, ...], original: dict) -> dict:
    result = {key: receipt[key] for key in keys if key in receipt}
    result.update(original_receipt=original, public_receipt=True, validation_scope='build_only',
                  signed=False, installed=False, published=False, runtime_qualified=False, release_qualified=False)
    return result


def verify_zip(path: Path, records: dict, prefix: str = '') -> None:
    with zipfile.ZipFile(release.regular(path)) as archive:
        names = [item.filename for item in archive.infolist()]
        if len(names) != len(set(names)) or set(names) != {prefix + item for item in records}:
            raise release.Refused('archive members differ from the original manifest')
        for relative, original in records.items():
            item = archive.getinfo(prefix + relative)
            if item.file_size != original['size']:
                raise release.Refused('archive member size differs')
            digest = hashlib.sha256()
            with archive.open(item) as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(chunk)
            if digest.hexdigest() != original['sha256']:
                raise release.Refused('archive member bytes differ')


def verify_tar(path: Path, root_name: str, records: dict) -> dict:
    found, modes = set(), {}
    with tarfile.open(release.regular(path), 'r:gz') as archive:
        for item in archive:
            parts = PurePosixPath(item.name).parts
            if not parts or parts[0] != root_name:
                raise release.Refused('server archive root differs')
            relative = '/'.join(parts[1:])
            if item.isdir():
                if relative:
                    name(relative)
                continue
            name(relative)
            if not item.isfile() or relative in found or relative not in records or item.size != records[relative]['size']:
                raise release.Refused('server archive has unexpected members')
            stream = archive.extractfile(item)
            if stream is None:
                raise release.Refused('server archive member is unreadable')
            digest = hashlib.sha256()
            with stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(chunk)
            if digest.hexdigest() != records[relative]['sha256'] or item.mode & ~0o777:
                raise release.Refused('server archive bytes or modes differ')
            modes[relative] = item.mode
            found.add(relative)
    if found != set(records):
        raise release.Refused('server archive omits original files')
    return modes


def native(directory: Path, host: str, core: dict, version: str) -> dict:
    receipt = read(directory / 'receipt.json')
    if (receipt.get('format') != 'localai-cowork-native-release-sidecar/1'
            or receipt.get('host') != host or receipt.get('profile') != 'release'
            or receipt.get('cargo_exit_code') != 0 or receipt.get('package_version') != version
            or receipt.get('source_before_after_equal') is not True or receipt.get('sources') != core
            or any(receipt.get(flag) is not False for flag in ('signed', 'installed', 'release_qualified'))):
        raise release.Refused('current successful native release receipts are required')
    artifact = receipt.get('artifact', {})
    expected_name = 'cowork-daemon-' + host + ('.exe' if host.endswith('msvc') else '')
    if artifact.get('filename') != expected_name or pin(directory / expected_name) != {key: artifact.get(key) for key in ('size', 'sha256')}:
        raise release.Refused('original daemon bytes differ')
    release.validate_executable(directory / expected_name, host)
    raw = release.regular(directory / 'cargo-build.jsonl').read_bytes()
    if hashlib.sha256(raw).hexdigest() != receipt.get('cargo_json_sha256'):
        raise release.Refused('original Cargo evidence differs')
    messages = desktop.cargo_messages(raw.decode('utf-8'))
    candidates = [item for item in messages if item.get('reason') == 'compiler-artifact'
                  and item.get('package_id') == receipt.get('package_id')
                  and item.get('target', {}).get('name') == 'cowork-daemon'
                  and item.get('target', {}).get('kind') == ['bin'] and item.get('executable')]
    if len(candidates) != 1 or [item.get('success') for item in messages if item.get('reason') == 'build-finished'] != [True]:
        raise release.Refused('one successful original release Cargo artifact is required')
    artifact_message = candidates[0]
    profile = artifact_message.get('profile', {})
    expected_source = (ROOT / 'services/daemon/src/main.rs').as_posix()
    if os.name == 'nt' and host == HOSTS['linux']:
        expected_source = '/mnt/' + ROOT.drive[0].lower() + ROOT.as_posix()[2:] + '/services/daemon/src/main.rs'
    if (profile.get('opt_level') != '3' or profile.get('debug_assertions') is not False
            or profile.get('test') is not False or profile.get('overflow_checks') is not False
            or artifact_message['executable'] != receipt.get('cargo_reported_artifact')
            or artifact_message.get('target', {}).get('src_path', '').replace('\\', '/') != expected_source):
        raise release.Refused('Cargo evidence does not bind this actual native product')
    return receipt


def checked(args) -> dict:
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    if json.loads((ROOT / 'package.json').read_text(encoding='utf-8'))['version'] != version:
        raise release.Refused('Rust and frontend versions differ')
    core = release.source_manifest(ROOT)
    front = read(args.frontend_receipt)
    web = {'apps/web/dist/' + relative: expected for relative, expected in tree(ROOT / 'apps/web/dist').items()}
    if (front.get('format') != 'localai-cowork-frontend-build/1' or front.get('exit_code') != 0
            or front.get('version') != version or front.get('validation_scope') != 'build_only'
            or front.get('sources_unchanged') is not True or front.get('sources') != frontend.sources()
            or front.get('outputs') != web or front.get('tests_run') is not False):
        raise release.Refused('current actual frontend build is required')
    packages = {}
    for platform, host in HOSTS.items():
        prepared = getattr(args, platform + '_prepared')
        native_receipt = native(prepared, host, core, version)
        package_dir = getattr(args, platform + '_package')
        receipt = read(package_dir / 'receipt.json')
        expected_kinds = {'nsis'} if platform == 'windows' else {'deb', 'appimage'}
        sidecar_files = tree(prepared)
        if (receipt.get('format') != 'localai-cowork-native-desktop-package/1' or receipt.get('result') != 'passed'
                or receipt.get('host') != host or receipt.get('product_version') != version
                or receipt.get('validation_scope') != 'build_only'
                or receipt.get('source_before_after_equal') is not True or receipt.get('staged_source_bytes_equal') is not True
                or receipt.get('sources') != desktop.input_manifest(ROOT) or receipt.get('sidecar_preparation') != sidecar_files
                or set(receipt.get('bundles', {})) != expected_kinds
                or any(receipt.get(flag) is not False for flag in ('signed', 'installed', 'published', 'release_qualified'))):
            raise release.Refused('current original desktop package receipts are required')
        for kind, artifact in receipt['bundles'].items():
            relative = name(artifact['filename'])
            if '/' in relative or pin(package_dir / relative) != {key: artifact[key] for key in ('size', 'sha256')}:
                raise release.Refused('desktop artifact bytes differ')
            desktop.validate_bundle(package_dir / relative, kind, host)
        if pin(package_dir / 'sbom.spdx.json') != {key: receipt.get('sbom', {}).get(key) for key in ('size', 'sha256')}:
            raise release.Refused('desktop SBOM bytes differ')
        packages[platform] = {'native': native_receipt, 'native_pin': pin(prepared / 'receipt.json'),
                              'receipt': receipt, 'receipt_pin': pin(package_dir / 'receipt.json'), 'directory': package_dir}
    server_files = tree(args.server)
    server = read(args.server / 'bundle-receipt.json')
    server_native = read(args.server / 'daemon-release-receipt.json')
    if (server.get('format') != 'localai-cowork-personal-server-bundle/1'
            or server.get('source_before_after_equal') is not True or server.get('source_files') != len(core)
            or server_native != packages['linux']['native'] or server.get('daemon') != server_native['artifact']
            or server.get('daemon_release_receipt_sha256') != packages['linux']['native_pin']['sha256']
            or pin(args.server / 'bin/cowork-daemon') != {key: server_native['artifact'][key] for key in ('size', 'sha256')}
            or server.get('web_files') != {relative.removeprefix('apps/web/dist/'): expected for relative, expected in web.items()}
            or server.get('package_helper_sha256') != pin(ROOT / 'tools/server/package.py')['sha256']):
        raise release.Refused('current server payload and native build must match')
    build_source = read(args.server / 'build-source/BUILD-SOURCE-RECEIPT.json')
    source_spec = importlib.util.spec_from_file_location('delivery_build_source', ROOT / 'tools/release/source_bundle.py')
    source_module = importlib.util.module_from_spec(source_spec)
    source_spec.loader.exec_module(source_module)
    expected_source = source_module.manifest()
    if (build_source.get('files') != expected_source or build_source.get('source_before_after_equal') is not True
            or {relative.removeprefix('build-source/'): expected for relative, expected in server_files.items()
                if relative.startswith('build-source/') and relative != 'build-source/BUILD-SOURCE-RECEIPT.json'} != expected_source):
        raise release.Refused('server build context differs from its actual product source')
    modes = verify_tar(args.server_archive, args.server.name, server_files)
    checks = (args.server / 'SHA256SUMS').read_text(encoding='utf-8').splitlines()
    if checks != [expected['sha256'] + '  ' + relative for relative, expected in server_files.items() if relative != 'SHA256SUMS']:
        raise release.Refused('server checksums do not bind all original bytes')
    agents = read(args.agents / 'manifest.json')
    agent_files = tree(args.agents)
    if (agents.get('format') != 'localai-cowork-v2-device-agent-delivery/1' or agents.get('version') != version
            or agents.get('validation_scope') != 'build_only' or agents.get('protocol') != 2
            or agents.get('files') != {relative: expected for relative, expected in agent_files.items() if relative != 'manifest.json'}):
        raise release.Refused('current original standalone agents are required')
    agent_builds = read(args.agents / 'PublicReceipts/builds.json')
    for platform, host in HOSTS.items():
        entry = agent_builds.get(platform, {})
        filename = 'cowork-device-agent-v2' + ('.exe' if platform == 'windows' else '')
        binary = args.agents / platform.capitalize() / filename
        if (entry.get('host') != host or entry.get('sources') != core or entry.get('package_version') != version
                or entry.get('result') != 'passed' or entry.get('cargo_exit_code') != 0
                or pin(binary) != {key: entry.get('artifact', {}).get(key) for key in ('size', 'sha256')}):
            raise release.Refused('agent native builds differ from the shared source closure')
        release.validate_executable(binary, host)
    return {'version': version, 'core': core, 'front': front, 'front_pin': pin(args.frontend_receipt),
            'packages': packages, 'server': server, 'server_files': server_files, 'server_modes': modes,
            'server_pin': pin(args.server / 'bundle-receipt.json'), 'server_archive_pin': pin(args.server_archive),
            'agent_files': agent_files, 'source': sources()}


def sums(directory: Path) -> None:
    records = tree(directory)
    with (directory / 'SHA256SUMS').open('x', encoding='utf-8', newline='\n') as output:
        output.write(''.join(item['sha256'] + '  ' + relative + '\n' for relative, item in records.items() if relative != 'SHA256SUMS'))


def assemble(args) -> dict:
    inputs = checked(args)
    output = args.output.absolute()
    archive_path = output.with_name(output.name + '-installation.zip')
    checksum_path = archive_path.with_name(archive_path.name + '.sha256')
    if any(path.exists() or path.is_symlink() for path in (output, archive_path, checksum_path)):
        raise release.Refused('select a new delivery directory and archive')
    desktop.directory(output.parent)
    stage = Path(tempfile.mkdtemp(prefix='.current-delivery-', dir=output.parent))
    native_keys = ('format', 'host', 'package_version', 'profile', 'cargo_exit_code', 'artifact', 'sources', 'source_before_after_equal', 'cargo_json_sha256')
    for platform, entry in inputs['packages'].items():
        for artifact in entry['receipt']['bundles'].values():
            relative = artifact['filename']
            copy(entry['directory'] / relative, stage / 'installers' / platform / relative, {key: artifact[key] for key in ('size', 'sha256')})
        copy(entry['directory'] / 'sbom.spdx.json', stage / 'installers' / platform / 'sbom.spdx.json', pin(entry['directory'] / 'sbom.spdx.json'))
        write(stage / 'PublicReceipts' / ('native-' + platform + '.json'), public(entry['native'], native_keys, entry['native_pin']))
        write(stage / 'PublicReceipts' / ('desktop-' + platform + '.json'), public(entry['receipt'],
              ('format', 'result', 'host', 'product_version', 'bundles', 'sources', 'source_before_after_equal',
               'staged_source_bytes_equal', 'cli_version', 'native_payload_architecture', 'native_installer_contract', 'sbom'), entry['receipt_pin']))
    write(stage / 'PublicReceipts/frontend.json', public(inputs['front'],
          ('format', 'version', 'exit_code', 'sources_unchanged', 'sources', 'outputs', 'tests_run'), inputs['front_pin']))
    for relative, expected in inputs['front']['outputs'].items():
        copy(ROOT / relative, stage / 'web' / relative.removeprefix('apps/web/dist/'), expected)
    for relative, expected in inputs['agent_files'].items():
        copy(args.agents / relative, stage / 'device-agent' / relative, expected)
    server = stage / 'server/localai-cowork-server'
    for relative, expected in inputs['server_files'].items():
        if relative not in {'bundle-receipt.json', 'daemon-release-receipt.json', 'SHA256SUMS'}:
            copy(args.server / relative, server / relative, expected)
    write(server / 'daemon-release-receipt.json', public(inputs['packages']['linux']['native'], native_keys, inputs['packages']['linux']['native_pin']))
    public_server = public(inputs['server'], tuple(key for key in inputs['server'] if key not in {'prepared_at', 'daemon_release_receipt_sha256'}), inputs['server_pin'])
    public_server.update(daemon_release_receipt_sha256=pin(server / 'daemon-release-receipt.json')['sha256'],
                         original_server_archive=inputs['server_archive_pin'], portable_receipts_only_change=True)
    write(server / 'bundle-receipt.json', public_server)
    sums(server)
    def tar_member(item):
        relative = PurePosixPath(item.name).relative_to(server.name).as_posix()
        item.mode = 0o700 if item.isdir() else inputs['server_modes'][relative]
        item.uid = item.gid = 0
        item.uname = item.gname = ''
        return item
    with tarfile.open(stage / 'server/localai-cowork-server-linux-x64.tar.gz', 'x:gz') as archive:
        archive.add(server, arcname=server.name, filter=tar_member)
    verify_tar(stage / 'server/localai-cowork-server-linux-x64.tar.gz', server.name, tree(server))
    with zipfile.ZipFile(stage / 'SOURCES.zip', 'x', zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for relative, expected in inputs['source'].items():
            if pin(ROOT / relative) != expected:
                raise release.Refused('source changed before archiving')
            archive.write(ROOT / relative, relative)
            if pin(ROOT / relative) != expected:
                raise release.Refused('source changed during archiving')
    verify_zip(stage / 'SOURCES.zip', inputs['source'])
    write(stage / 'SOURCE-PINS.json', {'format': 'localai-cowork-delivery-source-pins/1', 'sources': inputs['source'], 'core': inputs['core']})
    for relative, expected in inputs['source'].items():
        if relative.startswith(('deploy/v2/', 'tools/toolbox/', 'tools/device_agent/')) or relative in {'LICENSE', 'docs/FEATURE-MATRIX.md', 'docs/INSTALLATION-DELIVERY-2026-10-10.md', 'docs/DEVICE-AGENT-V2.md', 'docs/DESKTOP-RELEASE.md'}:
            copy(ROOT / relative, stage / relative, expected)
    (stage / 'README.md').write_text(
        '# LocalAI Cowork ' + inputs['version'] + '\n\n'
        'Windows-Installer: installers/windows. Linux: installers/linux (DEB und AppImage). '
        'Eigener Linux-Server: server/localai-cowork-server-linux-x64.tar.gz. '
        'Eigenständiger Geräteagent: device-agent/README.md. Docker-Compose: deploy/v2/README.md.\n\n'
        'Die Lieferbelege binden aktuelle native Builds, Webfrontend, vollständige Quellen, SBOMs und Prüfsummen. '
        'Dieser schnelle Durchlauf führt keine umfangreichen Tests, Installation oder produktive Laufzeitabnahme aus. '
        'Die Pakete sind unsigniert; ein öffentlicher Upload ist nicht erfolgt. '
        'LiteLLM-Zugänge werden pro Team eingerichtet.\n\n'
        'Installationsanleitung: docs/INSTALLATION-DELIVERY-2026-10-10.md. '
        'Funktionsstand: docs/FEATURE-MATRIX.md. SOURCES.zip enthält die vollständigen eingefrorenen Produktquellen. '
        'SHA256SUMS bindet alle Lieferdateien.\n', encoding='utf-8', newline='\n')
    final = checked(args)
    if (final['core'] != inputs['core'] or final['source'] != inputs['source'] or final['front_pin'] != inputs['front_pin']
            or final['server_files'] != inputs['server_files'] or final['server_archive_pin'] != inputs['server_archive_pin']
            or final['agent_files'] != inputs['agent_files']
            or any(final['packages'][platform]['receipt_pin'] != entry['receipt_pin']
                   or final['packages'][platform]['native_pin'] != entry['native_pin'] for platform, entry in inputs['packages'].items())):
        raise release.Refused('delivery inputs changed during assembly; staged bytes retained')
    write(stage / 'DELIVERY-RECEIPT.json', {'format': 'localai-cowork-current-delivery/1', 'version': inputs['version'],
          'created_at': datetime.now(timezone.utc).isoformat(), 'validation_scope': 'build_only',
          'tests_run_by_aggregator': False, 'signed': False, 'installed': False, 'published': False,
          'runtime_qualified': False, 'release_qualified': False, 'source_before_after_equal': True,
          'core_sources_equal_across_platforms': True, 'source_count': len(inputs['source']),
          'core_source_count': len(inputs['core']), 'files': tree(stage)})
    sums(stage)
    delivered = tree(stage)
    temporary = output.parent / ('.' + output.name + '-' + os.urandom(8).hex() + '.zip')
    with zipfile.ZipFile(temporary, 'x', zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for relative, expected in delivered.items():
            if pin(stage / relative) != expected:
                raise release.Refused('staged delivery changed')
            archive.write(stage / relative, output.name + '/' + relative)
    verify_zip(temporary, delivered, output.name + '/')
    if tree(stage) != delivered or sources() != inputs['source']:
        raise release.Refused('delivery or sources changed before publication')
    archive_pin = pin(temporary)
    desktop.publish(stage, output)
    desktop.publish(temporary, archive_path)
    with checksum_path.open('x', encoding='utf-8', newline='\n') as checksum:
        checksum.write(archive_pin['sha256'] + '  ' + archive_path.name + '\n')
    return {'delivery': str(output), 'installation_zip': {'filename': archive_path.name, **archive_pin},
            'files': len(delivered), 'version': inputs['version'], 'validation_scope': 'build_only',
            'signed': False, 'installed': False, 'published': False, 'release_qualified': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for entry in ('windows-prepared', 'linux-prepared', 'windows-package', 'linux-package',
                  'frontend-receipt', 'server', 'server-archive', 'agents', 'output'):
        parser.add_argument('--' + entry, type=Path, required=True)
    print(json.dumps(assemble(parser.parse_args()), indent=2))
