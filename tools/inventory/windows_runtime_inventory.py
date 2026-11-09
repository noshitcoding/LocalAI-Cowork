"""M0 partial inventory: explicitly named installed Windows program artifacts only.

No subprocess, SQLite, application launch, package extraction, credentials or repair.
The source archive is a separately pinned, reviewed V1 source capture, not a build proof.
"""
from __future__ import annotations
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import time
import zipfile
from datetime import datetime, timezone

ROOT = next(p for p in Path(__file__).resolve().parents if (p / 'tools/migration/v1_sqlite_export.py').is_file())
sys.path.insert(0, str(ROOT / 'tools/migration'))
import v1_sqlite_export as frozen

FORMAT = 'localai-cowork-windows-program-inventory/1'
FROZEN_SHA256 = '618049c79925d9871cd7ead15fff2d87feb46a40cd400ed4dbded4504f979f92'
MAX_TOTAL = 2 * 1024**3
MAX_JSON = 16 * 1024**2
MAX_PE_RESOURCE = 2 * 1024**2
LOCAL_FILES = (
    'app.exe', 'WebView2Loader.dll', 'python/windows.zip',
    'python/crew_runtime/main.py', 'python/crew_runtime/crew_tools.py',
    'python/crew_runtime/requirements.txt', 'python/crew_runtime/requirements.lock',
    'python/crew_runtime/wheels.zip', 'python/crew_runtime/runtime-bundle-manifest.json',
    'codex/runtime-bundle-manifest.json', 'codex/LICENSE',
    'codex/vendor/x86_64-pc-windows-msvc/bin/codex.exe',
    'daemon/windows-x64/cowork-local-daemon.exe', 'daemon/windows-x64/manifest.json',
    'daemon/windows-x64/pdfium.dll', 'resources/pdfium/bin/pdfium.dll',
    'resources/pdfium/LICENSE', 'resources/pdfium/VERSION', 'runtime-layout.json',
)
PRODUCT_FILES = {'Local AI Cowork': LOCAL_FILES, 'Open Cowork': ('app.exe',)}
SOURCE_NAMES = ('app/package.json', 'app/src-tauri/tauri.conf.json')
SOURCE_MAP = {
    name: 'app/src-tauri/' + name for name in (
        'python/crew_runtime/main.py', 'python/crew_runtime/crew_tools.py',
        'python/crew_runtime/requirements.txt', 'python/crew_runtime/requirements.lock',
        'python/crew_runtime/runtime-bundle-manifest.json',
    )
}
SOURCE_MAP.update({
    'python/windows.zip': 'app/src-tauri/resources/python/windows.zip',
    'codex/runtime-bundle-manifest.json': 'app/src-tauri/resources/codex/runtime-bundle-manifest.json',
    'daemon/windows-x64/manifest.json': 'app/src-tauri/resources/daemon/windows-x64/manifest.json',
    'runtime-layout.json': 'app/src-tauri/resources/runtime-layout.json',
})
Refused = frozen.Refused


def windows_only():
    if os.name != 'nt':
        raise Refused('Windows native source write/delete and namespace fences required')
    if frozen.hash_file(Path(frozen.__file__)) != FROZEN_SHA256:
        raise Refused('Qualified native fence implementation changed')


class Budget:
    def __init__(self): self.deadline = time.monotonic() + 120
    def check(self):
        if time.monotonic() > self.deadline: raise Refused('Inventory time limit exceeded')


def pin(value):
    if not isinstance(value, str) or re.fullmatch('[0-9a-f]{64}', value) is None:
        raise Refused('Explicit lowercase SHA256 pin required')
    return value


def bounded_directory(value):
    # Reuse the qualified local-disk/reparse checks without creating or reading a leaf.
    path = frozen.bounded_path(str(Path(value) / '__inventory_path_validation__'), False).parent
    if path == Path(path.anchor) or not path.is_dir(): raise Refused('Explicit installation directory required')
    return path


def strict_json(raw):
    if len(raw) > MAX_JSON: raise Refused('JSON byte limit exceeded')
    depth = 0; inside = False; escaped = False
    for byte in raw:
        if inside:
            if escaped: escaped = False
            elif byte == 92: escaped = True
            elif byte == 34: inside = False
        elif byte == 34: inside = True
        elif byte in (91, 123):
            depth += 1
            if depth > 32: raise Refused('JSON depth limit exceeded')
        elif byte in (93, 125): depth -= 1
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result: raise Refused('Duplicate JSON key')
            result[key] = value
        return result
    try:
        value = json.loads(raw, object_pairs_hook=unique,
                           parse_constant=lambda _: (_ for _ in ()).throw(Refused('Nonfinite JSON')))
    except (ValueError, UnicodeError, RecursionError) as error:
        raise Refused('Invalid package JSON') from error
    if not isinstance(value, dict): raise Refused('JSON object required')
    return value


def text(value):
    if not isinstance(value, str) or not value or len(value) > 256 or any(ord(c) < 32 for c in value):
        raise Refused('Invalid bounded package metadata string')
    return value


def size(value):
    if type(value) is not int or not 0 <= value <= frozen.MAX_SOURCE: raise Refused('Invalid artifact size')
    return value


def status_path(path):
    try: st = path.lstat()
    except FileNotFoundError: return 'missing'
    if path.is_symlink() or getattr(st, 'st_file_attributes', 0) & 0x400:
        raise Refused('Reparse point at explicit artifact path')
    if not path.is_file(): raise Refused('Artifact must be a regular file')
    return 'present'


def selected_namespace(roots):
    # Never list installation directories. Inspect only the fixed artifact names/parents.
    state = {}; directories = set(roots.values())
    for product, root in roots.items():
        for relative in PRODUCT_FILES[product]:
            path = root / relative
            for directory in [root, *[p for p in reversed(path.parent.parents) if root in p.parents], path.parent]:
                if directory in state: continue
                try: st = directory.lstat()
                except FileNotFoundError: state[directory] = 'missing'; continue
                if not directory.is_dir() or directory.is_symlink() or getattr(st, 'st_file_attributes', 0) & 0x400:
                    raise Refused('Artifact ancestor is a reparse point or not a directory')
                state[directory] = 'present'; directories.add(directory)
            state[path] = status_path(path)
    return state, directories


class Fences:
    """Hold every file concurrently, without relaxing the qualified per-file 512 MiB cap."""
    def __init__(self, paths, budget): self.paths = sorted(set(paths)); self.budget = budget
    def __enter__(self):
        self.stack = contextlib.ExitStack(); self.files = {}
        total = 0; identities = set()
        try:
            for path in self.paths:
                item = self.stack.enter_context(frozen.FrozenFiles([path], self.budget))
                total += item.before[path]['bytes']
                if total > MAX_TOTAL: raise Refused('Combined inventory byte limit exceeded')
                identity = (item.before[path]['volume'], item.before[path]['file_id'])
                if identity in identities: raise Refused('Duplicate physical program artifact')
                identities.add(identity); self.files[path] = item
            return self
        except BaseException: self.stack.close(); raise
    def read(self, path, maximum): return self.files[path].read(path, maximum)
    def receipt(self, path): return self.files[path].receipt(path)
    def stream(self, path): return self.files[path].streams[path]
    def __exit__(self, *_): self.stack.close()


def registry_snapshot(roots):
    """HKCU uninstall only; never read uninstall commands or arbitrary registry values."""
    import winreg
    base = r'Software\Microsoft\Windows\CurrentVersion\Uninstall'
    result = {}
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, base, 0, winreg.KEY_READ | winreg.KEY_WOW64_64KEY) as parent:
        for index in range(winreg.QueryInfoKey(parent)[0]):
            name = winreg.EnumKey(parent, index)
            with winreg.OpenKey(parent, name, 0, winreg.KEY_READ | winreg.KEY_WOW64_64KEY) as key:
                try: display, kind = winreg.QueryValueEx(key, 'DisplayName')
                except FileNotFoundError: continue
                if display not in roots: continue
                if kind != winreg.REG_SZ or display in result: raise Refused('Ambiguous installed product metadata')
                fields = {'DisplayName': display}
                for field in ('DisplayVersion', 'InstallLocation', 'Publisher'):
                    try: value, kind = winreg.QueryValueEx(key, field)
                    except FileNotFoundError: value = None; kind = winreg.REG_SZ
                    if kind != winreg.REG_SZ or (value is not None and (not isinstance(value, str) or len(value) > 512)):
                        raise Refused('Unsupported uninstall metadata type')
                    fields[field] = value
                location = fields['InstallLocation']
                # NSIS stores a quoted directory literal, not a command line. Strip exactly
                # one balanced outer pair; never parse/execute uninstall command arguments.
                if isinstance(location, str) and location.startswith('"') and location.endswith('"'):
                    location = location[1:-1]
                if isinstance(location, str) and '"' in location: raise Refused('Invalid quoted installation location')
                if not location or os.path.normcase(os.path.abspath(location)) != os.path.normcase(str(roots[display])):
                    raise Refused('Explicit root does not match HKCU installation metadata')
                result[display] = {'hive': 'HKCU', 'view': '64', 'subkey': base + '\\' + name,
                                   'last_write_100ns': winreg.QueryInfoKey(key)[2], 'fields': fields,
                                   'normalized_install_location': str(roots[display])}
    if set(result) != set(roots): raise Refused('Selected product not found in HKCU uninstall metadata')
    return result


def pe_version(fences, path):
    """Parse PE headers/RT_VERSION from held bytes; never LoadLibrary/version DLL/MUI."""
    stream = fences.stream(path); length = fences.files[path].before[path]['bytes']
    def read_at(offset, amount):
        if offset < 0 or amount < 0 or offset + amount > length: raise Refused('Truncated PE artifact')
        stream.seek(offset); data = stream.read(amount)
        if len(data) != amount: raise Refused('Truncated PE read')
        return data
    dos = read_at(0, 64)
    if dos[:2] != b'MZ': raise Refused('Program artifact has no DOS/PE header')
    offset = struct.unpack_from('<I', dos, 60)[0]
    if offset > 1024**2: raise Refused('PE header offset limit')
    header = read_at(offset, 24)
    if header[:4] != b'PE\0\0': raise Refused('Program artifact has no PE signature')
    machine, count = struct.unpack_from('<HH', header, 4); optional_size = struct.unpack_from('<H', header, 20)[0]
    if not 1 <= count <= 96 or not 96 <= optional_size <= 4096: raise Refused('Unsupported PE header')
    optional = read_at(offset + 24, optional_size); magic = struct.unpack_from('<H', optional)[0]
    directory_offset = {0x10b: 96, 0x20b: 112}.get(magic)
    if directory_offset is None: raise Refused('Unknown PE optional header format')
    result = {'machine': machine, 'optional_header_magic': magic, 'fixed_file_version': None,
              'fixed_product_version': None, 'version_resource': 'missing'}
    if optional_size < directory_offset + 24: return result
    rva, resource_size = struct.unpack_from('<II', optional, directory_offset + 16)
    if not rva: return result
    if not 16 <= resource_size <= MAX_PE_RESOURCE: raise Refused('PE resource byte limit')
    sections = read_at(offset + 24 + optional_size, count * 40)
    def file_offset(address, amount):
        matches = []
        for i in range(count):
            virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from('<IIII', sections, i*40+8)
            if virtual_address <= address and address + amount <= virtual_address + min(max(virtual_size, raw_size), raw_size):
                matches.append(raw_pointer + address - virtual_address)
        if len(matches) != 1: raise Refused('Ambiguous/unmapped PE resource address')
        return matches[0]
    resource = read_at(file_offset(rva, resource_size), resource_size)
    def entries(at):
        if at + 16 > len(resource): raise Refused('Truncated PE resource directory')
        named, ids = struct.unpack_from('<HH', resource, at+12)
        if named + ids > 128 or at+16+8*(named+ids) > len(resource): raise Refused('PE resource entry limit')
        return [struct.unpack_from('<II', resource, at+16+8*i) for i in range(named+ids)]
    versions = []
    types = [dest for key, dest in entries(0) if key == 16]
    if len(types) > 1: raise Refused('Duplicate PE version type')
    for directory in types:
        if not directory & 0x80000000: raise Refused('Invalid PE version type directory')
        for _, name in entries(directory & 0x7fffffff):
            if not name & 0x80000000: raise Refused('Invalid PE version name directory')
            for _, language in entries(name & 0x7fffffff):
                if language & 0x80000000 or language + 16 > len(resource): raise Refused('Invalid PE version leaf')
                address, amount = struct.unpack_from('<II', resource, language)
                if not 6 <= amount <= 65536: raise Refused('PE version leaf limit')
                value = read_at(file_offset(address, amount), amount)
                total, fixed_len, kind = struct.unpack_from('<HHH', value)
                marker = 'VS_VERSION_INFO\0'.encode('utf-16le')
                if total > len(value) or value[6:6+len(marker)] != marker or kind != 0:
                    raise Refused('Unknown PE version structure')
                fixed_at = (6+len(marker)+3) & ~3
                if fixed_len != 52 or fixed_at+52 > total: raise Refused('Invalid PE fixed version length')
                fixed = struct.unpack_from('<13I', value, fixed_at)
                if fixed[:2] != (0xfeef04bd, 0x10000): raise Refused('Unknown fixed PE version')
                def version(ms, ls): return '.'.join(str(v) for v in (ms >> 16, ms & 65535, ls >> 16, ls & 65535))
                versions.append((version(fixed[2], fixed[3]), version(fixed[4], fixed[5])))
    if versions:
        if len(set(versions)) != 1: raise Refused('Conflicting PE version resources')
        result.update(version_resource='present', fixed_file_version=versions[0][0], fixed_product_version=versions[0][1])
    return result


def source_reference(fences, manifest, archive, manifest_pin, archive_pin):
    if fences.receipt(manifest)['sha256'] != pin(manifest_pin) or fences.receipt(archive)['sha256'] != pin(archive_pin):
        raise Refused('Original source manifest/archive SHA pin mismatch')
    metadata = strict_json(fences.read(manifest, MAX_JSON))
    if type(metadata.get('schema')) is not int or metadata['schema'] not in (1, 2) or metadata.get('archive_sha256') != archive_pin:
        raise Refused('Unsupported original source capture or unbound archive')
    files = metadata.get('files')
    if not isinstance(files, list) or len(files) > 20000: raise Refused('Source capture file-list limit')
    allowed = {*SOURCE_NAMES, *SOURCE_MAP.values()}; selected = {}
    for item in files:
        if not isinstance(item, dict): raise Refused('Invalid source manifest entry')
        name = item.get('path')
        if name not in allowed: continue
        if name in selected: raise Refused('Duplicate selected original artifact')
        selected[name] = {'bytes': size(item.get('size')), 'sha256': pin(item.get('sha256'))}
    if not set(SOURCE_NAMES).issubset(selected): raise Refused('Pinned original V1 product metadata missing')
    archive_stream = fences.stream(archive); archive_stream.seek(0)
    with zipfile.ZipFile(archive_stream) as zipped:
        entries = zipped.infolist()
        if len(entries) > 20000: raise Refused('Original archive member limit')
        for name, expected in selected.items():
            matches = [item for item in entries if item.filename == name]
            if len(matches) != 1: raise Refused('Original selected archive member ambiguous/missing')
            entry = matches[0]
            if entry.file_size != expected['bytes'] or entry.file_size > MAX_JSON or entry.flag_bits & 1 or entry.is_dir():
                raise Refused('Original selected archive member invalid')
            raw = zipped.read(entry)
            if hashlib.sha256(raw).hexdigest() != expected['sha256']: raise Refused('Original selected archive member hash mismatch')
            if name in SOURCE_NAMES: selected[name]['metadata'] = strict_json(raw)
    package = selected[SOURCE_NAMES[0]]['metadata']; tauri = selected[SOURCE_NAMES[1]]['metadata']
    version = text(tauri.get('version')); product = text(tauri.get('productName'))
    if product != 'Local AI Cowork' or text(package.get('version')) != version: raise Refused('Inconsistent original product metadata')
    for name in SOURCE_NAMES: selected[name].pop('metadata')
    return {'manifest_sha256': manifest_pin, 'archive_sha256': archive_pin, 'selected_files': selected,
            'productName': product, 'version': version, 'packageName': text(package.get('name')),
            'identifier': text(tauri.get('identifier')), 'build_parity_proven': False}


def package_claims(relative, value, receipts):
    """Known versioned fields only. Manifest paths never select additional filesystem reads."""
    checks = []
    def claim(path, sha, byte_count=None):
        expected = {'sha256': pin(sha)}
        if byte_count is not None: expected['bytes'] = size(byte_count)
        observed = receipts.get(path)
        if observed is None or any(observed[key] != expected[key] for key in expected):
            raise Refused('Installed package manifest hash/size claim mismatch for ' + path)
        checks.append({'artifact': path, 'expected': expected, 'matched': True})
    if relative == 'python/crew_runtime/runtime-bundle-manifest.json':
        if type(value.get('schemaVersion')) is not int or value['schemaVersion'] != 1: raise Refused('Unknown Python bundle schema')
        python = value.get('python'); wheelhouse = value.get('wheelhouse')
        if not isinstance(python, dict) or not isinstance(wheelhouse, dict): raise Refused('Incomplete Python bundle metadata')
        archive = python.get('archive'); wheels = wheelhouse.get('archive')
        if not isinstance(archive, dict) or not isinstance(wheels, dict): raise Refused('Incomplete Python bundle archives')
        if archive.get('path') != 'src-tauri/resources/python/windows.zip' or wheels.get('path') != 'src-tauri/python/crew_runtime/wheels.zip':
            raise Refused('Unknown Python archive layout')
        claim('python/windows.zip', archive.get('sha256'), archive.get('bytes'))
        claim('python/crew_runtime/wheels.zip', wheels.get('sha256'), wheels.get('bytes'))
        claim('python/crew_runtime/requirements.txt', wheelhouse.get('requirementsSha256'))
        claim('python/crew_runtime/requirements.lock', wheelhouse.get('lockSha256'))
        packages = value.get('packages')
        if not isinstance(packages, list) or len(packages) > 2048: raise Refused('Python package-count limit')
        package_metadata = []; names = set()
        for item in packages:
            if not isinstance(item, dict): raise Refused('Invalid Python package metadata')
            record = {k: text(item.get(k)) for k in ('name', 'version', 'filename', 'license', 'purl')}
            record['sha256'] = pin(item.get('sha256'))
            if record['name'] in names: raise Refused('Duplicate Python package name')
            names.add(record['name']); package_metadata.append(record)
        return {'known_schema': 'crew-runtime-bundle/1', 'python_name': text(python.get('name')),
                'python_version': text(python.get('version')), 'package_count': len(packages),
                'package_metadata_sha256': frozen.digest(package_metadata), 'artifact_claims': checks,
                'wheel_members_independently_verified': False, 'smoke_reexecuted': False}
    if relative == 'codex/runtime-bundle-manifest.json':
        if value.get('version') != '0.147.0' or value.get('protocolSchema') != 'app-server-0.147.0' or value.get('target') != 'windows-x64':
            raise Refused('Unknown Codex bundle version/schema/target')
        if value.get('binary') != 'vendor/x86_64-pc-windows-msvc/bin/codex.exe' or value.get('license') != 'LICENSE':
            raise Refused('Unknown Codex bundle layout')
        claim('codex/' + value['binary'], value.get('sha256'), value.get('binarySize'))
        claim('codex/LICENSE', value.get('licenseSha256'))
        return {'known_schema': 'codex-app-server-0.147.0/windows-x64', 'version': value['version'],
                'package': text(value.get('package')), 'protocolSchema': value['protocolSchema'],
                'archive_claim_sha256': pin(value.get('archiveSha256')), 'archive_independently_verified': False,
                'artifact_claims': checks}
    if relative == 'daemon/windows-x64/manifest.json':
        if type(value.get('schemaVersion')) is not int or value['schemaVersion'] != 2 or value.get('target') != 'windows-x64':
            raise Refused('Unknown daemon bundle schema/target')
        if value.get('binary') != 'cowork-local-daemon.exe': raise Refused('Unknown daemon bundle layout')
        claim('daemon/windows-x64/cowork-local-daemon.exe', value.get('sha256'))
        if not isinstance(value.get('files'), list) or len(value['files']) != 1 or not isinstance(value['files'][0], dict) or value['files'][0].get('name') != 'pdfium.dll':
            raise Refused('Unknown daemon auxiliary artifacts')
        claim('daemon/windows-x64/pdfium.dll', value['files'][0].get('sha256'))
        return {'known_schema': 'local-daemon-bundle/2/windows-x64', 'version': text(value.get('version')), 'artifact_claims': checks}
    raise Refused('Unrecognized semantic manifest')


def equal_version(observed, expected):
    def parts(value):
        if not isinstance(value, str) or re.fullmatch(r'\d+(?:\.\d+){0,3}', value) is None: return None
        result = tuple(int(part) for part in value.split('.'))
        return result + (0,) * (4-len(result))
    left = parts(observed); right = parts(expected)
    return left is not None and right is not None and left == right


def inventory(roots, manifest, manifest_pin, archive, archive_pin, output, *, registry_reader=registry_snapshot, probe=None):
    windows_only(); budget = Budget()
    roots = {name: bounded_directory(path) for name, path in roots.items()}
    if not roots or not set(roots).issubset(PRODUCT_FILES): raise Refused('Known explicitly selected products required')
    for a, root in roots.items():
        for b, other in roots.items():
            if a != b and (root == other or root in other.parents or other in root.parents): raise Refused('Installation roots overlap')
    manifest = frozen.bounded_path(manifest); archive = frozen.bounded_path(archive)
    output = frozen.output_path(output, [manifest, archive, *[r/'app.exe' for r in roots.values()]])
    if any(root == path or root in path.parents for root in roots.values() for path in (manifest, archive)):
        raise Refused('Original reference must be separate from selected installation')
    registry_before = registry_reader(roots)
    initial, directories = selected_namespace(roots)
    with frozen.FrozenDirectories([*directories, manifest.parent, archive.parent, output.parent]):
        states, _ = selected_namespace(roots)
        if states != initial: raise Refused('Selected artifact namespace changed during fencing')
        paths = [manifest, archive, *[p for p, state in states.items() if state == 'present' and p.is_file()]]
        with Fences(paths, budget) as fences:
            before = {path: fences.receipt(path) for path in paths}
            reference = source_reference(fences, manifest, archive, manifest_pin, archive_pin)
            products = []
            for name, root in roots.items():
                receipts = {rel: before[root/rel] for rel in PRODUCT_FILES[name] if root/rel in before}
                artifacts = []
                for relative in PRODUCT_FILES[name]:
                    item = {'relative_path': relative, 'status': states[root/relative]}
                    if relative in receipts:
                        item['before'] = receipts[relative]
                        if relative.endswith(('.exe', '.dll')): item['pe'] = pe_version(fences, root/relative)
                        if relative in ('python/crew_runtime/runtime-bundle-manifest.json', 'codex/runtime-bundle-manifest.json', 'daemon/windows-x64/manifest.json'):
                            item['package_semantics'] = package_claims(relative, strict_json(fences.read(root/relative, MAX_JSON)), receipts)
                        if relative == 'runtime-layout.json': item['semantics'] = 'present_unparsed_no_reconstruction'
                    if name == reference['productName']:
                        expected = reference['selected_files'].get(SOURCE_MAP.get(relative))
                        item['original_reference'] = {'status': 'not_available_in_pinned_source_capture'} if expected is None else {
                            'status': 'match' if relative in receipts and all(receipts[relative][key] == expected[key] for key in ('sha256','bytes')) else 'different_or_missing',
                            'source_path': SOURCE_MAP[relative], 'expected': expected}
                    else: item['original_reference'] = {'status': 'no_matching_original_product_capture'}
                    artifacts.append(item)
                app = next(item for item in artifacts if item['relative_path'] == 'app.exe')
                if app['status'] != 'present': raise Refused('Selected installed app.exe missing')
                registry_version = registry_before[name]['fields']['DisplayVersion']
                products.append({'product': name, 'explicit_root': str(root), 'registry': registry_before[name],
                                 'registry_matches_pe_product_version': equal_version(app['pe']['fixed_product_version'], registry_version),
                                 'original_version_matches': name == reference['productName'] and equal_version(registry_version, reference['version']),
                                 'build_parity_proven': False, 'artifacts': artifacts})
            if probe is not None: probe(fences, roots)  # Private test hook; no CLI option or installed-program execution.
            after = {path: fences.receipt(path) for path in paths}
            if before != after: raise Refused('Program/reference bytes or physical identity changed')
            if selected_namespace(roots)[0] != states: raise Refused('Selected artifact presence/namespace changed')
            if registry_reader(roots) != registry_before: raise Refused('Selected uninstall metadata changed')
            for product in products:
                for item in product['artifacts']:
                    if item['status'] == 'present': item['after'] = after[roots[product['product']]/item['relative_path']]
            report = {'format': FORMAT, 'captured_at_utc': datetime.now(timezone.utc).isoformat(),
                      'scope': 'partial M0 installed program artifacts; no installation backup/build parity/M0 completion',
                      'read_only_sources': True, 'programs_executed': False, 'original_sqlite_opened': False,
                      'user_data_or_credentials_selected': False, 'm0_complete': False,
                      'native_fence_sha256': FROZEN_SHA256, 'reference': reference, 'products': products,
                      'reference_before': [before[manifest], before[archive]], 'reference_after': [after[manifest], after[archive]],
                      'byte_and_physical_identity_receipts_equal': True,
                      'absence_observation': 'explicit expected paths checked before and after; no directory enumeration'}
            stage = Path(tempfile.mkdtemp(prefix='.'+output.name+'-', dir=output.parent)); identity = frozen.stage_identity(stage)
            try:
                frozen.write_json(stage/'inventory.json', report)
                budget.check(); frozen.publish(stage, output, identity)
            finally: frozen.cleanup_stage(stage, identity)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--local-ai-cowork-root'); parser.add_argument('--open-cowork-root')
    parser.add_argument('--source-manifest', required=True); parser.add_argument('--source-manifest-sha256', required=True)
    parser.add_argument('--source-archive', required=True); parser.add_argument('--source-archive-sha256', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args(argv)
    roots = {name: value for name, value in [('Local AI Cowork', args.local_ai_cowork_root), ('Open Cowork', args.open_cowork_root)] if value}
    try:
        report = inventory(roots, args.source_manifest, args.source_manifest_sha256, args.source_archive, args.source_archive_sha256, args.output)
        print(json.dumps({'format': FORMAT, 'products': len(report['products']), 'output': str(Path(args.output)/'inventory.json'), 'm0_complete': False}))
        return 0
    except (Refused, OSError, zipfile.BadZipFile, KeyError, TypeError, ValueError) as error:
        print('REFUSED: '+str(error), file=sys.stderr); return 2


if __name__ == '__main__': raise SystemExit(main())
