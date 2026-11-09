"""Private proposal: verify an original unsigned-review ZIP; never execute packages.

The caller supplies unchanged GitHub API JSON and an already downloaded ZIP.
No source module is imported. No receipt is rewritten. Output is a separate binding.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import zipfile

HOSTS = {'windows': 'x86_64-pc-windows-msvc', 'linux': 'x86_64-unknown-linux-gnu'}
MAX_ARCHIVE = 4 * 1024**3
MAX_EXPANDED = 8 * 1024**3
MAX_FILES = 10000
MAX_JSON = 16 * 1024**2
DESKTOP_MARKER_POLICY = 'tauri-cli-2.11.4-single-bundle-marker/1'
DESKTOP_MARKER_PREFIX = b'__TAURI_BUNDLE_TYPE_VAR_'
DESKTOP_MARKERS = {'nsis': b'NSS', 'deb': b'DEB', 'appimage': b'APP'}
MAX_DESKTOP_MARKERS = 32


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024**2), b''):
            result.update(chunk)
    return result.hexdigest()


def ordinary(path):
    require(path.is_absolute(), 'An absolute ordinary path is required')
    for parent in [*reversed(path.parents), path]:
        info = parent.lstat()
        require(not parent.is_symlink() and not getattr(info, 'st_file_attributes', 0) & 0x400,
                'Symlinks/reparse points are refused')
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'An ordinary unlinked file is required')
    return path


def pin(path):
    ordinary(path)
    return {'size': path.stat().st_size, 'sha256': digest(path)}


def desktop_payload_pin(path, expected, kind):
    """Allow only the pinned CLI's one three-byte marker change; retain raw bytes."""
    require(kind in DESKTOP_MARKERS and expected.get('marker_policy') == DESKTOP_MARKER_POLICY,
            'Unreviewed desktop bundle-marker policy/kind')
    code = DESKTOP_MARKERS[kind]
    require('allowed_marker' not in expected or expected['allowed_marker'] == code.decode('ascii'),
            'Desktop marker differs from selected native bundle kind')
    require(type(expected.get('size')) is int and 0 < expected['size'] <= 512 * 1024**2
            and bool(re.fullmatch(r'[0-9a-f]{64}', expected.get('sha256', ''))),
            'Pinned original Cargo desktop bytes are required')
    ordinary(path)
    require(path.stat().st_size == expected['size'], 'Installed desktop length differs from Cargo artifact')
    with path.open('rb') as stream:
        data = stream.read(expected['size'] + 1)
    require(len(data) == expected['size'], 'Desktop changed while reading bounded payload')
    raw = {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
    metadata = {'policy': DESKTOP_MARKER_POLICY, 'marker': 'unchanged', 'changed_bytes': 0,
                'metadata_byte_offset': None, 'normalized_sha256': raw['sha256']}
    if raw['sha256'] != expected['sha256']:
        marker = DESKTOP_MARKER_PREFIX + code
        require(data.count(marker) <= MAX_DESKTOP_MARKERS, 'Desktop bundle-marker candidate count exceeds bound')
        matches = []
        offset = data.find(marker)
        while offset >= 0:
            normalized = hashlib.sha256()
            normalized.update(memoryview(data)[:offset])
            normalized.update(DESKTOP_MARKER_PREFIX + b'UNK')
            normalized.update(memoryview(data)[offset + len(marker):])
            if normalized.hexdigest() == expected['sha256']:
                matches.append(offset)
            offset = data.find(marker, offset + 1)
        require(len(matches) == 1, 'Desktop changed beyond one selected three-byte Tauri bundle marker')
        metadata.update(marker=code.decode('ascii'), changed_bytes=3,
                        metadata_byte_offset=matches[0] + len(DESKTOP_MARKER_PREFIX),
                        normalized_sha256=expected['sha256'])
    require(pin(path) == raw, 'Installed desktop changed during Cargo/raw binding')
    return {'raw': raw, 'normalization': metadata}


def portable(name):
    require(isinstance(name, str) and name and '\\' not in name and ':' not in name,
            'A relative portable path is required')
    value = PurePosixPath(name)
    require(bool(value.parts) and not value.is_absolute() and value.as_posix() == name, 'Noncanonical archive/source path')
    reserved = {'CON', 'PRN', 'AUX', 'NUL', *(f'COM{i}' for i in range(1, 10)), *(f'LPT{i}' for i in range(1, 10))}
    require(all(part not in {'.', '..'} and part == part.rstrip(' .')
                and part.split('.')[0].upper() not in reserved
                and not any(ord(char) < 32 for char in part) for part in value.parts),
            'Traversal, control characters or nonportable file names are refused')
    return value


def read_json(path):
    ordinary(path)
    require(path.stat().st_size <= MAX_JSON, 'JSON exceeds its limit')
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, 'Duplicate JSON key')
            result[key] = value
        return result
    def invalid_constant(value):
        raise ValueError('Non-finite JSON number: ' + value)
    result = json.loads(path.read_bytes(), object_pairs_hook=pairs, parse_constant=invalid_constant)
    require(isinstance(result, dict), 'A JSON object is required')
    return result


def canonical(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def checked_metadata(selection, run, artifact):
    repo = selection.get('repository', '')
    commit = selection.get('source_commit', '')
    require(bool(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo)), 'Invalid repository')
    require(bool(re.fullmatch(r'[0-9a-f]{40}', commit)), 'Select the exact original commit')
    require(selection.get('platform') in HOSTS, 'Unsupported native platform')
    require(type(selection.get('producer_run_id')) is int and selection['producer_run_id'] > 0,
            'Select a positive original producer run ID')
    require(type(selection.get('artifact_id')) is int and selection['artifact_id'] > 0,
            'Select an immutable positive artifact ID')
    require(bool(re.fullmatch(r'[0-9a-f]{64}', selection.get('archive_sha256', ''))), 'Select an archive SHA256')
    require(run.get('id') == selection['producer_run_id'] and run.get('head_sha') == commit
            and run.get('path') == '.github/workflows/preview-build-review.yml'
            and run.get('event') == 'workflow_dispatch' and run.get('status') == 'completed'
            and run.get('conclusion') == 'success' and type(run.get('run_attempt')) is int
            and run['run_attempt'] > 0 and run.get('repository', {}).get('full_name') == repo
            and type(run.get('repository', {}).get('id')) is int and run['repository']['id'] > 0
            and run.get('head_repository', {}).get('full_name') == repo,
            'A successful original same-repository unsigned producer on the exact commit is required')
    require(artifact.get('id') == selection['artifact_id'] and artifact.get('expired') is False
            and artifact.get('digest') == 'sha256:' + selection['archive_sha256']
            and type(artifact.get('size_in_bytes')) is int and 0 < artifact['size_in_bytes'] <= MAX_ARCHIVE
            and artifact.get('name') == f"v2-unsigned-review-{selection['platform']}-{run['id']}-{run['run_attempt']}"
            and artifact.get('workflow_run', {}).get('id') == run['id']
            and artifact.get('workflow_run', {}).get('head_sha') == commit
            and artifact.get('workflow_run', {}).get('repository_id') == run.get('repository', {}).get('id')
            and artifact.get('workflow_run', {}).get('head_repository_id') == run.get('repository', {}).get('id'),
            'Artifact ID, producer, attempt, repository, digest or expiry mismatch')


def safe_extract(archive, target, expected_sha256):
    ordinary(archive)
    require(0 < archive.stat().st_size <= MAX_ARCHIVE, 'Archive size is outside the bound')
    require(digest(archive) == expected_sha256, 'Downloaded original ZIP digest mismatch')
    require(not target.exists() and target.parent.is_dir(), 'Choose a new extraction directory')
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        require(0 < len(entries) <= MAX_FILES, 'ZIP entry count exceeds its bound')
        require(sum(item.file_size for item in entries) <= MAX_EXPANDED, 'ZIP expansion exceeds its bound')
        names = set()
        for item in entries:
            raw = item.filename[:-1] if item.is_dir() else item.filename
            portable(raw)
            require(item.orig_filename == item.filename and '\x00' not in item.orig_filename, 'NUL archive name')
            require(raw.casefold() not in names, 'Duplicate or case-colliding ZIP path')
            names.add(raw.casefold())
            mode = (item.external_attr >> 16) & 0xFFFF
            require(stat.S_IFMT(mode) in (0, stat.S_IFDIR if item.is_dir() else stat.S_IFREG),
                    'ZIP symlinks/special files are refused')
            require(not item.flag_bits & 1 and item.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED),
                    'Encrypted or unsupported ZIP encoding')
        target.mkdir(mode=0o700)
        actual = 0
        for item in entries:
            selected = target / item.filename
            if item.is_dir():
                selected.mkdir(parents=True, exist_ok=True)
                continue
            selected.parent.mkdir(parents=True, exist_ok=True)
            count = 0
            with source.open(item) as stream, selected.open('xb') as output:
                for chunk in iter(lambda: stream.read(1024**2), b''):
                    count += len(chunk)
                    actual += len(chunk)
                    require(count <= item.file_size and actual <= MAX_EXPANDED, 'Actual ZIP expansion exceeded bound')
                    output.write(chunk)
            require(count == item.file_size, 'Truncated ZIP member')


def checked_pin(root, record, name=None):
    require(isinstance(record, dict), 'A hash-bound record is required')
    filename = name if name is not None else record.get('filename')
    portable(filename)
    require(type(record.get('size')) is int and record['size'] > 0
            and bool(re.fullmatch(r'[0-9a-f]{64}', record.get('sha256', ''))), 'Invalid file pin')
    require(pin(root / filename) == {key: record[key] for key in ('size', 'sha256')}, 'Original bytes changed: ' + filename)


def checked_tree(root, source_root, selection, run):
    review = read_json(root / 'UNSIGNED-BUILD-REVIEW.json')
    package = read_json(root / 'receipt.json')
    provenance = read_json(root / 'source-provenance.json')
    front = read_json(root / 'frontend/receipt.json')
    sidecar = read_json(root / 'sidecar-receipt.json')
    host = HOSTS[selection['platform']]
    version = read_json(source_root / 'package.json')['version']
    require(version == '2.0.0-alpha.8', 'This bounded proposal targets alpha.8 only')
    require(review.get('format') == 'localai-cowork-unsigned-build-review/1'
            and review.get('commit') == selection['source_commit'] and review.get('host') == host
            and str(review.get('run_id')) == str(run['id'])
            and str(review.get('run_attempt')) == str(run['run_attempt'])
            and review.get('package_receipt_sha256') == digest(root / 'receipt.json')
            and review.get('frontend_receipt_sha256') == digest(root / 'frontend/receipt.json')
            and review.get('installer_execution_performed') is False
            and review.get('installed_acceptance') == 'pending', 'Original unsigned review receipt mismatch')
    require(all(value.get(flag) is False for value in (review, package, provenance)
                for flag in ('signed', 'installed', 'published', 'release_qualified')), 'Review must remain unsigned/uninstalled/unqualified')
    bundles = {'nsis'} if selection['platform'] == 'windows' else {'deb', 'appimage'}
    require(package.get('format') == 'localai-cowork-native-desktop-package/1'
            and package.get('result') == 'passed' and package.get('host') == host
            and package.get('product_version') == version
            and package.get('validation_scope') == 'format_tests_clippy_build'
            and package.get('desktop_format_test_clippy_exit_codes') == [0, 0, 0]
            and package.get('source_before_after_equal') is True
            and package.get('staged_source_bytes_equal') is True
            and set(package.get('bundles', {})) == bundles, 'Complete original native package checks are required')
    require(provenance.get('format') == 'localai-cowork-unsigned-review-source/1'
            and provenance.get('commit') == selection['source_commit']
            and provenance.get('sources') == package.get('sources')
            and provenance.get('source_sha256') == canonical(package['sources']) == review.get('source_sha256')
            and provenance.get('verification') == review.get('verification')
            and review.get('verification', {}).get('head_sha') == selection['source_commit']
            and review.get('verification', {}).get('path') == '.github/workflows/verify.yml'
            and review.get('verification', {}).get('conclusion') == 'success', 'Original source/verify binding mismatch')
    require(front.get('format') == 'localai-cowork-frontend-build/1' and front.get('version') == version
            and front.get('exit_code') == 0 and front.get('sources_unchanged') is True,
            'Original frontend build is incomplete')
    outputs = {name: record for name, record in package['sources'].items() if name.startswith('apps/web/dist/')}
    require(outputs and outputs == front.get('outputs'), 'Native package does not bind the common frontend outputs')
    require(front.get('sources') and sidecar.get('sources'), 'Original frontend and core manifests are required')
    for manifest in (package['sources'], front['sources'], sidecar['sources']):
        require(isinstance(manifest, dict) and manifest, 'Source manifest is missing')
        for name, record in manifest.items():
            portable(name)
            if name.startswith('apps/web/dist/'):
                continue
            checked_pin(source_root, record, name)
    require(sidecar.get('format') == 'localai-cowork-native-release-sidecar/1'
            and sidecar.get('host') == host and sidecar.get('package_version') == version
            and sidecar.get('profile') == 'release' and sidecar.get('cargo_exit_code') == 0
            and sidecar.get('source_before_after_equal') is True
            and sidecar.get('signed') is False and sidecar.get('installed') is False,
            'Original release sidecar evidence is incomplete')
    prepared = package.get('sidecar_preparation', {})
    checked_pin(root, prepared.get('receipt.json'), 'sidecar-receipt.json')
    checked_pin(root, prepared.get('cargo-build.jsonl'), 'sidecar-cargo-build.jsonl')
    require(digest(root / 'sidecar-cargo-build.jsonl') == sidecar.get('cargo_json_sha256'), 'Sidecar Cargo digest mismatch')
    artifact = sidecar.get('artifact', {})
    require(artifact.get('filename') == 'cowork-daemon-' + host + ('.exe' if selection['platform'] == 'windows' else '')
            and prepared.get(artifact['filename']) == {key: artifact.get(key) for key in ('size', 'sha256')}, 'Sidecar payload pin mismatch')
    for record in [*package['bundles'].values(), package['sbom']]:
        require('/' not in record.get('filename', ''), 'Package/SBOM must be at archive root')
        checked_pin(root, record)
    if selection['platform'] == 'windows':
        contract = package.get('native_installer_contract', {})
        require(contract.get('contract') == 'nsis.currentUser.originalProcess.v1'
                and contract.get('product_version') == version
                and all(contract.get(key) == package['bundles']['nsis'][key] for key in ('size', 'sha256')),
                'Original NSIS contract is missing')
    logs = review.get('build_logs', {})
    require(logs and len(logs) <= 100 and sum(record.get('size', MAX_EXPANDED) for record in logs.values()) <= 256 * 1024**2,
            'Bounded original native logs are required')
    for name, record in logs.items():
        require('/' not in name, 'Build log must be a basename')
        checked_pin(root / 'build-logs', record, name)
    for record in [package.get('desktop_artifact', {}), package.get('bundle_input_desktop', {}), package.get('browser_host', {})]:
        require(type(record.get('size')) is int and 0 < record['size'] <= 512 * 1024**2
                and bool(re.fullmatch(r'[0-9a-f]{64}', record.get('sha256', ''))),
                'Valid original installed desktop/browser payload pins are required')
    cargo_desktop = package['desktop_artifact']
    bundle_input = package['bundle_input_desktop']
    require(package.get('cli_version') == '2.11.4' and bundle_input.get('other_bytes_unchanged') is True
            and bundle_input['size'] == cargo_desktop['size']
            and bundle_input.get('marker') in ({'unchanged', 'NSS'} if selection['platform'] == 'windows'
                                               else {'unchanged', 'DEB', 'APP'})
            and (bundle_input['marker'] != 'unchanged' or bundle_input['sha256'] == cargo_desktop['sha256']),
            'Original pinned Tauri Cargo/bundle-input marker evidence is required')
    desktop = {**cargo_desktop, 'marker_policy': DESKTOP_MARKER_POLICY,
               'original_bundle_input': bundle_input}
    if selection['platform'] == 'windows':
        desktop['allowed_marker'] = 'NSS'
    return {'format': 'localai-cowork-unsigned-installer-smoke-input/1', **selection,
            'host': host, 'version': version, 'source_sha256': review['source_sha256'],
            'frontend_receipt_sha256': review['frontend_receipt_sha256'],
            'package_receipt': pin(root / 'receipt.json'), 'review_receipt': pin(root / 'UNSIGNED-BUILD-REVIEW.json'),
            'packages': package['bundles'], 'expected_payloads': {'desktop': desktop,
                'daemon': {key: artifact[key] for key in ('size', 'sha256')}, 'browser_host': package['browser_host']},
            'original_receipts_unchanged': True, 'installer_execution_performed': False,
            'installed_smoke': 'not_run', 'signed': False, 'published': False, 'release_qualified': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('selection', 'run-json', 'artifact-json', 'archive', 'source-root', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    selection = read_json(args.selection.absolute())
    run = read_json(args.run_json.absolute())
    artifact = read_json(args.artifact_json.absolute())
    checked_metadata(selection, run, artifact)
    source_root = args.source_root.absolute()
    head = subprocess.check_output(['git', '-C', str(source_root), 'rev-parse', 'HEAD'], text=True).strip()
    require(head == selection['source_commit'], 'Selected source checkout must equal original producer commit')
    require(not subprocess.check_output(['git', '-C', str(source_root), 'status', '--porcelain', '--untracked-files=no']),
            'Selected source checkout has tracked edits')
    output = args.output.absolute()
    require(not output.exists() and output.parent.is_dir(), 'Choose a new evidence output directory')
    output.mkdir(mode=0o700)
    safe_extract(args.archive.absolute(), output / 'original', selection['archive_sha256'])
    record = checked_tree(output / 'original', source_root, selection, run)
    record['archive'] = pin(args.archive.absolute())
    record['api_metadata'] = {'run': pin(args.run_json.absolute()), 'artifact': pin(args.artifact_json.absolute())}
    with (output / 'input-binding.json').open('x', encoding='utf-8') as stream:
        json.dump(record, stream, indent=2, sort_keys=True)
        stream.write('\n')
    print(json.dumps(record, indent=2, sort_keys=True))


if __name__ == '__main__':
    main()
