"""Build/package the owned model gateway. No backend, model, config or token is bundled.

The operator supplies independently reviewed native llama.cpp files, GGUF weights
and qualification later through configure.py. This command never installs or starts
a service and only creates a new native archive.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
NAME = 'cowork-model-gateway'


def ordinary(path, directory=False, cargo_artifact=False):
    path = Path(os.path.abspath(path))
    for item in (*reversed(path.parents), path):
        info = item.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            raise ValueError('Linked package path refused')
    info = path.stat()
    if not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise ValueError('Ordinary package input required')
    if not directory and not cargo_artifact and info.st_nlink != 1:
        raise ValueError('Hard-linked package input refused')
    return path


def sha(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def source_manifest():
    paths = [ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
                                      'services/daemon/src/private_secret_file.rs', 'docs/MODEL-ACCOUNTING.md',
                                      'tools/release/sbom.py', 'tools/release/runtime_inventory.py', 'LICENSE')]
    for name in ('services/model-gateway', 'crates/contracts', 'crates/model-accounting', 'crates/executor'):
        paths.extend(p for p in (ROOT / name).rglob('*') if p.is_file() and p.suffix in {'.rs', '.toml'})
    paths.extend(Path(__file__).parent.glob('*.py'))
    return {p.relative_to(ROOT).as_posix(): sha(ordinary(p)) for p in sorted(set(paths))}


def load_sbom():
    path = ordinary(ROOT / 'tools/release/sbom.py')
    spec = importlib.util.spec_from_file_location('cowork_gateway_sbom', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build(cargo):
    cargo = ordinary(cargo, cargo_artifact=True)
    environment = dict(os.environ)
    if 'RUST_MIN_STACK' in environment:
        raise ValueError('Gateway delivery requires the normal platform thread stack')
    suffix = '.exe' if os.name == 'nt' else ''
    environment['RUSTC'] = str(ordinary(cargo.with_name('rustc' + suffix), cargo_artifact=True))
    environment['RUSTDOC'] = str(ordinary(cargo.with_name('rustdoc' + suffix), cargo_artifact=True))
    version = subprocess.run([str(cargo), '--version'], check=True, capture_output=True, text=True).stdout.strip()
    command = [str(cargo), 'build', '--release', '--locked', '--offline', '-p', NAME, '--bin', NAME, '--message-format=json']
    completed = subprocess.run(command, cwd=ROOT, env=environment, check=True, capture_output=True, text=True)
    result = []
    for line in completed.stdout.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if (value.get('reason') == 'compiler-artifact' and value.get('target', {}).get('name') == NAME
                and 'bin' in value['target'].get('kind', []) and value.get('executable')):
            result.append(ordinary(value['executable'], cargo_artifact=True))
    if len(result) != 1:
        raise ValueError('Cargo did not identify one exact native gateway executable')
    return result[0], version


def copy(source, destination, cargo_artifact=False):
    source = ordinary(source, cargo_artifact=cargo_artifact)
    original = sha(source)
    shutil.copyfile(source, destination)
    if sha(source) != original or sha(destination) != original:
        raise ValueError('Original package input changed while copying')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--gateway', type=Path, help='Explicit reviewed native release executable')
    parser.add_argument('--build', action='store_true', help='Build this workspace release executable, locked/offline')
    parser.add_argument('--cargo', type=Path, help='Absolute Cargo executable for --build')
    args = parser.parse_args()
    if args.output.exists() or args.output.is_symlink() or not args.output.is_absolute():
        raise ValueError('New absolute package output required')
    ordinary(args.output.parent, True)
    extension = '.zip' if os.name == 'nt' else '.tar.gz'
    if not str(args.output).endswith(extension):
        raise ValueError('Use native archive extension ' + extension)
    if args.build:
        if not args.cargo or args.gateway:
            raise ValueError('--build requires --cargo and excludes supplied executables')
    elif not args.gateway or args.cargo:
        raise ValueError('Supply --gateway or use --build with --cargo')
    sources = source_manifest()
    executable, cargo_version = build(args.cargo) if args.build else (ordinary(args.gateway), None)
    if source_manifest() != sources:
        raise ValueError('Gateway sources changed during native build')
    spec = importlib.util.spec_from_file_location('cowork_gateway_configure', Path(__file__).with_name('configure.py'))
    configure = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(configure)
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    with tempfile.TemporaryDirectory(prefix='cowork-model-gateway-package-') as temporary:
        stage = Path(temporary)
        receiver = stage / (NAME + ('.exe' if os.name == 'nt' else ''))
        copy(executable, receiver, args.build)
        receiver.chmod(0o755)
        configure.native_binary(receiver)
        for name in ('configure.py', 'windows_private.py'):
            copy(Path(__file__).with_name(name), stage / name)
        for name in ('sbom.py', 'runtime_inventory.py'):
            copy(ROOT / 'tools/release' / name, stage / name)
        copy(ROOT / 'docs/MODEL-ACCOUNTING.md', stage / 'README.md')
        copy(ROOT / 'LICENSE', stage / 'LICENSE')
        sbom = load_sbom().generate(ROOT, [], payloads=sorted(stage.iterdir()),
                                   include_cargo=args.build, include_desktop=False, include_npm=False)
        (stage / 'SBOM.spdx.json').write_text(json.dumps(sbom, indent=2) + '\n', encoding='utf-8')
        if source_manifest() != sources:
            raise ValueError('Gateway source/package inventory changed before publication')
        manifest = {'format': 1, 'kind': 'standalone_v2_owned_model_gateway',
                    'source_application_version': version, 'application_version': version if args.build else None,
                    'platform': 'windows_x64' if os.name == 'nt' else 'linux_x64',
                    'profile': 'release' if args.build else 'operator_supplied', 'cargo': cargo_version,
                    'cargo_lock_sha256': sources['Cargo.lock'] if args.build else None,
                    'build_sources': sources if args.build else None, 'packaging_sources': sources,
                    'backend_bundled': False, 'model_bundled': False, 'credentials_bundled': False,
                    'python_setup_requirement': 'Python 3.11+ supplied by the operator; not bundled',
                    'installed': False, 'started': False, 'real_model_qualified': False,
                    'files': {p.name: {'sha256': sha(p), 'bytes': p.stat().st_size} for p in sorted(stage.iterdir())}}
        (stage / 'MANIFEST.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        with args.output.open('xb') as file:
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
    print(json.dumps({'archive': str(args.output), 'sha256': sha(args.output),
                      'installed': False, 'started': False, 'real_model_qualified': False}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print('Gateway packaging refused: ' + (str(error) if isinstance(error, ValueError)
                                             else 'Native build or file operation failed; no installation performed'), file=sys.stderr)
        raise SystemExit(1)
