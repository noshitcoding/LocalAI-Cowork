"""Prepare a native V2 receiver archive. Does not install or start anything.

Pass the two reviewed release executables explicitly, or request --build to build
the exact workspace package with Cargo's locked offline release configuration.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
NAMES = ('cowork-process-runner', 'cowork-runner-probe')


def source_manifest():
    files = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', ROOT / 'rust-toolchain.toml', ROOT / 'services/daemon/src/private_secret_file.rs',
             ROOT / 'tools/release/sbom.py', ROOT / 'tools/release/runtime_inventory.py', Path(__file__)]
    for directory in ('services/process-runner', 'crates/contracts', 'crates/domain', 'crates/executor', 'crates/runner-protocol'):
        files.extend(p for p in (ROOT / directory).rglob('*') if p.is_file() and p.suffix in {'.rs', '.toml'})
    return {p.relative_to(ROOT).as_posix(): sha(ordinary(p)) for p in sorted(files)}


def ordinary(path, directory=False, cargo=False):
    path = Path(os.path.abspath(path))
    for item in (*reversed(path.parents), path):
        info = item.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            raise ValueError('Linked package path refused')
    info = path.stat()
    if not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise ValueError('Ordinary package input required')
    if not directory and not cargo and info.st_nlink != 1:
        raise ValueError('Linked package input refused')
    return path


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def binary(path, cargo=False):
    ordinary(path, cargo=cargo)
    if not 0 < path.stat().st_size <= 512 * 1024 * 1024:
        raise ValueError('Invalid native executable size')
    with path.open('rb') as file:
        header = file.read(64)
        if os.name == 'nt':
            if header[:2] != b'MZ':
                raise ValueError('Native Windows executable required')
            offset = struct.unpack_from('<I', header, 60)[0]
            if not 64 <= offset <= path.stat().st_size - 6:
                raise ValueError('Invalid PE header')
            file.seek(offset)
            if file.read(6) != b'PE\0\0\x64\x86':
                raise ValueError('Native x64 Windows executable required')
        elif not (header[:6] == b'\x7fELF\x02\x01' and struct.unpack_from('<H', header, 18)[0] == 62):
            raise ValueError('Native x64 Linux executable required')


def build(cargo):
    cargo = ordinary(cargo)
    suffix = '.exe' if os.name == 'nt' else ''
    compiler = ordinary(cargo.with_name('rustc' + suffix))
    documentation = ordinary(cargo.with_name('rustdoc' + suffix))
    environment = dict(os.environ)
    environment['RUSTC'] = str(compiler)
    environment['RUSTDOC'] = str(documentation)
    version = subprocess.run([str(cargo), '--version'], check=True, capture_output=True, text=True).stdout.strip()
    command = [str(cargo), 'build', '--locked', '--offline', '--release', '-p', 'cowork-process-runner', '--bins', '--message-format=json']
    completed = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True, env=environment)
    artifacts = {}
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') in NAMES and message.get('executable') and 'bin' in message['target'].get('kind', []):
            name = message['target']['name']
            if name in artifacts:
                raise ValueError('Ambiguous Cargo executable')
            artifacts[name] = ordinary(message['executable'], cargo=True)
    if set(artifacts) != set(NAMES):
        raise ValueError('Cargo did not report both exact receiver binaries')
    return artifacts, version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, help='Absolute NEW archive path, ending .zip on Windows or .tar.gz on Linux')
    parser.add_argument('--receiver', help='Absolute reviewed native receiver executable')
    parser.add_argument('--probe', help='Absolute reviewed native probe executable')
    parser.add_argument('--build', action='store_true', help='Explicitly build the native workspace release binaries, locked and offline')
    parser.add_argument('--source-sbom', type=Path, help='Original SPDX 2.3 graph bound to both supplied executables; required without --build')
    parser.add_argument('--cargo', help='Absolute Cargo executable for --build')
    args = parser.parse_args()
    output = Path(args.output)
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError('Select a new absolute archive path')
    ordinary(output.parent, True)
    extension = '.zip' if os.name == 'nt' else '.tar.gz'
    if not str(output).endswith(extension):
        raise ValueError('Use the native archive extension ' + extension)
    cargo_version = None
    sources = source_manifest() if args.build else None
    if args.build:
        if not args.cargo or args.receiver or args.probe:
            raise ValueError('--build requires --cargo and does not accept supplied binaries')
        artifacts, cargo_version = build(args.cargo)
        if source_manifest() != sources:
            raise ValueError('Receiver build sources changed; choose a fresh stable build')
    else:
        if not args.receiver or not args.probe or not args.source_sbom:
            raise ValueError('Supply both reviewed binaries and their original source SBOM, or explicitly request --build')
        artifacts = dict(zip(NAMES, (ordinary(args.receiver), ordinary(args.probe))))
        upstream=json.loads(ordinary(args.source_sbom).read_text(encoding='utf-8'))
        recorded={checksum.get('checksumValue') for file in upstream.get('files',[]) for checksum in file.get('checksums',[]) if checksum.get('algorithm')=='SHA256'}
        if upstream.get('spdxVersion')!='SPDX-2.3' or not {sha(path) for path in artifacts.values()}.issubset(recorded):
            raise ValueError('Original SPDX graph does not bind both supplied executable hashes')
    for value in artifacts.values():
        binary(value, cargo=args.build)
    with tempfile.TemporaryDirectory(prefix='cowork-runner-package-') as temporary:
        stage = Path(temporary)
        for name, source in artifacts.items():
            target = stage / (name + ('.exe' if os.name == 'nt' else ''))
            original = sha(source)
            shutil.copyfile(source, target)
            if sha(target) != original or sha(source) != original:
                raise ValueError('Executable changed during packaging')
            target.chmod(0o755)
        for name in ('configure.py', 'windows_private.py'):
            shutil.copyfile(ordinary(Path(__file__).with_name(name)), stage / name)
        shutil.copyfile(ordinary(ROOT / 'docs/PROCESS-RUNNER-V2.md'), stage / 'README.md')
        shutil.copyfile(ordinary(ROOT / 'LICENSE'), stage / 'LICENSE')
        specification = importlib.util.spec_from_file_location('cowork_runner_sbom', ROOT / 'tools/release/sbom.py')
        sbom_module = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(sbom_module)
        originals=[]
        if not args.build:
            shutil.copyfile(ordinary(args.source_sbom),stage/'original-source-sbom.spdx.json')
            originals=[(stage/'original-source-sbom.spdx.json',sha(path)) for path in artifacts.values()]
        sbom = sbom_module.generate(ROOT, [], payloads=[stage / (name + ('.exe' if os.name == 'nt' else '')) for name in NAMES],
                                    include_desktop=False, include_npm=False,include_cargo=bool(args.build),upstream_documents=originals)
        (stage / 'sbom.spdx.json').write_text(json.dumps(sbom, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
        manifest = {'format': 1, 'kind': 'standalone_v2_process_runner', 'application_version': version if args.build else None, 'source_application_version': version, 'runner_protocol_version': 1, 'platform': 'windows_x64' if os.name == 'nt' else 'linux_x64',
                    'cargo_lock_sha256': sha(ROOT / 'Cargo.lock') if args.build else None, 'build_sources': sources,
                    'cargo': cargo_version, 'profile': 'release' if args.build else 'operator_supplied',
                    'qualification_performed': False,
                    'sbom_included': True,
                    'files': {p.name: {'sha256': sha(p), 'bytes': p.stat().st_size} for p in sorted(stage.iterdir())}}
        (stage / 'MANIFEST.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        # Exclusive creation avoids overwriting any existing reviewed release.
        with output.open('xb') as file:
            if os.name == 'nt':
                with zipfile.ZipFile(file, 'w', zipfile.ZIP_DEFLATED) as archive:
                    for path in sorted(stage.iterdir()):
                        archive.write(path, path.name)
            else:
                with tarfile.open(fileobj=file, mode='w:gz') as archive:
                    for path in sorted(stage.iterdir()):
                        archive.add(path, arcname=path.name, recursive=False)
            file.flush()
            os.fsync(file.fileno())
    print(json.dumps({'archive': str(output), 'sha256': sha(output), 'installed': False, 'qualification_performed': False}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print('Runner packaging refused: ' + (str(error) if isinstance(error, ValueError) else 'Native build or file operation failed; no installation performed'), file=sys.stderr)
        raise SystemExit(1)
