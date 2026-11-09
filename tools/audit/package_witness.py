"""Package the independent native audit witness; no keys, state, or installation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def module(name, path):
    specification = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(result)
    return result


files = module('cowork_witness_package_files', ROOT / 'tools/process_runner/package.py')


def sources():
    paths = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', ROOT / 'rust-toolchain.toml',
             ROOT / 'services/daemon/src/private_secret_file.rs',
             ROOT / 'tools/process_runner/package.py', ROOT / 'tools/release/sbom.py',
             ROOT / 'tools/release/runtime_inventory.py', Path(__file__)]
    for directory in ('services/audit-witness', 'crates/identity', 'crates/contracts', 'crates/secrets'):
        paths.extend(p for p in (ROOT / directory).rglob('*') if p.is_file() and p.suffix in {'.rs', '.toml', '.sql'})
    return {str(p.relative_to(ROOT)).replace('\\', '/'): files.sha(files.ordinary(p)) for p in sorted(paths)}


def build(cargo):
    executable = files.ordinary(cargo)
    suffix = '.exe' if os.name == 'nt' else ''
    environment = dict(os.environ)
    environment['RUSTC'] = str(files.ordinary(executable.with_name('rustc' + suffix)))
    environment['RUSTDOC'] = str(files.ordinary(executable.with_name('rustdoc' + suffix)))
    command = [str(executable), 'build', '--locked', '--offline', '--release', '-p',
               'cowork-audit-witness', '--bin', 'cowork-audit-witness', '--message-format=json']
    result = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True, env=environment)
    artifacts = []
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if (message.get('reason') == 'compiler-artifact' and
                message.get('target', {}).get('name') == 'cowork-audit-witness' and
                'bin' in message['target'].get('kind', []) and message.get('executable')):
            artifacts.append(files.ordinary(message['executable'], cargo=True))
    if len(artifacts) != 1:
        raise ValueError('Cargo must report exactly one original witness executable')
    return artifacts[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True)
    parser.add_argument('--binary', help='Reviewed native executable; provenance remains operator-supplied')
    parser.add_argument('--source-sbom', help='Original SPDX-2.3 source SBOM bound to the supplied executable SHA-256')
    parser.add_argument('--build', action='store_true')
    parser.add_argument('--cargo', help='Absolute Cargo executable for the explicit locked offline release build')
    args = parser.parse_args()
    output = Path(args.output)
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError('Select a new absolute archive path')
    files.ordinary(output.parent, directory=True)
    extension = '.zip' if os.name == 'nt' else '.tar.gz'
    if not str(output).endswith(extension):
        raise ValueError('Native archive extension required: ' + extension)
    frozen = sources() if args.build else None
    if args.build:
        if not args.cargo or args.binary or args.source_sbom:
            raise ValueError('--build requires --cargo and excludes supplied binary/SBOM inputs')
        executable = build(args.cargo)
        if sources() != frozen:
            raise ValueError('Build inputs changed; create a fresh stable package')
    else:
        if not args.binary or not args.source_sbom or args.cargo:
            raise ValueError('Supply --binary with its original --source-sbom, or explicitly request --build --cargo')
        executable = files.ordinary(args.binary)
        source_sbom = files.ordinary(args.source_sbom)
        if source_sbom.stat().st_size > 64 * 1024 * 1024:
            raise ValueError('Original source SBOM exceeds input limit')
        upstream = json.loads(source_sbom.read_text(encoding='utf-8'))
        recorded = {checksum.get('checksumValue') for file in upstream.get('files', [])
                    for checksum in file.get('checksums', []) if checksum.get('algorithm') == 'SHA256'}
        if upstream.get('spdxVersion') != 'SPDX-2.3' or files.sha(executable) not in recorded:
            raise ValueError('Original SPDX source graph does not bind the supplied executable SHA-256')
    files.binary(executable, cargo=args.build)
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    with tempfile.TemporaryDirectory(prefix='cowork-audit-witness-package-') as temporary:
        stage = Path(temporary)
        target = stage / ('cowork-audit-witness' + ('.exe' if os.name == 'nt' else ''))
        before = files.sha(executable)
        shutil.copyfile(executable, target)
        target.chmod(0o755)
        if files.sha(target) != before or files.sha(executable) != before:
            raise ValueError('Executable changed while copying')
        shutil.copyfile(files.ordinary(ROOT / 'docs/AUDIT-INSIGHTS-V2.md'), stage / 'README.md')
        shutil.copyfile(files.ordinary(ROOT / 'LICENSE'), stage / 'LICENSE')
        originals = []
        if not args.build:
            original_hash = files.sha(source_sbom)
            original = stage / 'original-source-sbom.spdx.json'
            shutil.copyfile(source_sbom, original)
            if files.sha(original) != original_hash or files.sha(source_sbom) != original_hash:
                raise ValueError('Original source SBOM changed while copying')
            originals = [(original, before)]
        sbom = module('cowork_witness_sbom', ROOT / 'tools/release/sbom.py').generate(
            ROOT, [], payloads=[target], include_desktop=False, include_npm=False,
            include_cargo=bool(args.build), upstream_documents=originals)
        (stage / 'sbom.spdx.json').write_text(json.dumps(sbom, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        manifest = {'format': 1, 'kind': 'independent_v2_audit_witness',
                    'source_application_version': version,
                    'application_version': version if args.build else None,
                    'profile': 'release' if args.build else 'operator_supplied',
                    'platform': 'windows_x64' if os.name == 'nt' else 'linux_x64',
                    'build_sources': frozen, 'cargo_lock_sha256': files.sha(ROOT / 'Cargo.lock') if args.build else None,
                    'sbom_included': True, 'installed': False, 'signed': False,
                    'qualified_independent_deployment': False,
                    'includes_keys_or_state': False,
                    'files': {p.name: {'bytes': p.stat().st_size, 'sha256': files.sha(p)} for p in sorted(stage.iterdir())}}
        (stage / 'MANIFEST.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        with output.open('xb') as stream:
            if os.name == 'nt':
                with zipfile.ZipFile(stream, 'w', zipfile.ZIP_DEFLATED) as archive:
                    for path in sorted(stage.iterdir()):
                        archive.write(path, path.name)
            else:
                with tarfile.open(fileobj=stream, mode='w:gz') as archive:
                    for path in sorted(stage.iterdir()):
                        archive.add(path, arcname=path.name, recursive=False)
            stream.flush()
            os.fsync(stream.fileno())
    print(json.dumps({'archive': str(output), 'sha256': files.sha(output),
                      'installed': False, 'qualified_independent_deployment': False}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError):
        print('Witness packaging failed; no installation or state export performed', file=sys.stderr)
        raise SystemExit(1)
