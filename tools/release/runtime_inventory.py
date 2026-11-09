"""Read actual explicitly selected software trees; never execute their payloads.

This module also works when embedded next to the private tool configurator.
It reads PE/ELF import tables and Python distribution metadata without invoking
ldd, a loader, or an installed program. User data/configuration is not selected.
"""
from __future__ import annotations

from email.parser import BytesParser
import base64
import csv
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import struct

FORMAT = 'localai-cowork-runtime-inventory/2'
MAX_FILES = 150_000
MAX_BYTES = 24 * 1024**3
MAX_HEADER = 8 * 1024**2


def relative(name):
    if not isinstance(name, str) or not name or len(name) > 4096 or any(c in name for c in '\\\0\r\n:'):
        raise ValueError('Ordinary relative runtime filename required')
    if PurePosixPath(name).is_absolute() or any(part in ('', '.', '..') for part in name.split('/')):
        raise ValueError('Runtime filename escapes its selected root')
    return name


def ordinary(path, *, directory=False):
    path = Path(os.path.abspath(path))
    for part in (*reversed(path.parents), path):
        info = part.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            raise ValueError('Runtime root contains an alias or reparse point')
    if not (path.is_dir() if directory else path.is_file()):
        raise ValueError('Ordinary runtime input required')
    return path


def pin(path):
    path = ordinary(path)
    before = path.stat()
    with path.open('rb') as stream:
        sha = hashlib.file_digest(stream, 'sha256').hexdigest()
    after = path.stat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise ValueError('Runtime bytes changed while hashing')
    return {'size': before.st_size, 'sha256': sha}


def native_imports(path):
    """Bounded PE normal/delay imports or ELF DT_NEEDED, without code execution."""
    size = Path(path).stat().st_size
    with Path(path).open('rb') as file:
        def read(offset, count):
            if offset < 0 or count < 0 or count > MAX_HEADER or offset + count > size:
                raise ValueError('Native header exceeds its file bounds')
            file.seek(offset)
            value = file.read(count)
            if len(value) != count:
                raise ValueError('Truncated native file')
            return value
        head = read(0, min(size, 64))
        if head[:2] == b'MZ':
            if len(head) != 64:
                raise ValueError('Truncated PE header')
            offset = struct.unpack_from('<I', head, 60)[0]
            header = read(offset, 24)
            if header[:4] != b'PE\0\0':
                raise ValueError('Invalid PE signature')
            machine, count = struct.unpack_from('<HH', header, 4)
            optional_size = struct.unpack_from('<H', header, 20)[0]
            if not 1 <= count <= 96 or not 96 <= optional_size <= 4096:
                raise ValueError('Unsupported PE headers')
            optional = read(offset + 24, optional_size)
            magic = struct.unpack_from('<H', optional)[0]
            directory_at = 112 if magic == 0x20b else 96 if magic == 0x10b else None
            if directory_at is None or directory_at + 16 > len(optional):
                raise ValueError('Unsupported PE optional header')
            table = read(offset + 24 + optional_size, count * 40)
            sections = [struct.unpack_from('<IIII', table, i * 40 + 8) for i in range(count)]
            def mapped(address, amount):
                offsets = [raw + address - start for _, start, raw_size, raw in sections
                           if start <= address and address + amount <= start + raw_size]
                if len(offsets) != 1:
                    raise ValueError('Ambiguous or unmapped PE import')
                return offsets[0]
            def cstring(address):
                start = mapped(address, 1)
                raw = read(start, min(512, size - start)).split(b'\0', 1)
                if len(raw) != 2:
                    raise ValueError('Unterminated PE library name')
                name = raw[0].decode('ascii').lower()
                if not name or any(c in name for c in '/\\:\0\r\n'):
                    raise ValueError('Invalid PE library import')
                return name
            imports = set()
            for number, width in ((1, 20), (13, 32)):
                at = directory_at + number * 8
                if at + 8 > len(optional):
                    continue
                rva, amount = struct.unpack_from('<II', optional, at)
                if not rva:
                    continue
                if not width <= amount <= MAX_HEADER:
                    raise ValueError('PE import directory exceeds bound')
                data, terminated = read(mapped(rva, amount), amount), False
                for start in range(0, len(data) - width + 1, width):
                    entry = data[start:start + width]
                    if not any(entry):
                        terminated = True
                        break
                    if number == 1:
                        name = struct.unpack_from('<I', entry, 12)[0]
                    else:
                        if struct.unpack_from('<I', entry)[0] != 1:
                            raise ValueError('Legacy VA delay import unsupported')
                        name = struct.unpack_from('<I', entry, 4)[0]
                    imports.add(cstring(name))
                    if len(imports) > 4096:
                        raise ValueError('PE import count exceeds bound')
                if not terminated:
                    raise ValueError('Unterminated PE import table')
            return {'format': 'PE', 'architecture': hex(machine), 'imports': sorted(imports)}
        if head[:4] != b'\x7fELF':
            return None
        if len(head) < 52 or head[4] not in (1, 2) or head[5] not in (1, 2):
            raise ValueError('Unsupported ELF header')
        endian, is64 = ('<' if head[5] == 1 else '>'), head[4] == 2
        phoff = struct.unpack_from(endian + ('Q' if is64 else 'I'), head, 32 if is64 else 28)[0]
        width, count = struct.unpack_from(endian + 'HH', head, 54 if is64 else 42)
        if width != (56 if is64 else 32) or count > 1024:
            raise ValueError('Unsupported ELF program headers')
        table, loads, dynamic = read(phoff, width * count), [], []
        for i in range(count):
            entry = table[i * width:(i + 1) * width]
            if is64:
                kind, _, at, virtual, _, amount, _, _ = struct.unpack(endian + 'IIQQQQQQ', entry)
            else:
                kind, at, virtual, _, amount, _, _, _ = struct.unpack(endian + 'IIIIIIII', entry)
            if kind == 1:
                loads.append((virtual, amount, at))
            if kind == 2:
                dynamic.append((at, amount))
        if len(dynamic) > 1:
            raise ValueError('Ambiguous ELF dynamic table')
        offsets, string_address, string_size = [], None, None
        for at, amount in dynamic:
            data, terminated = read(at, amount), False
            width = 16 if is64 else 8
            for i in range(0, len(data) - width + 1, width):
                kind, value = struct.unpack_from(endian + ('qQ' if is64 else 'iI'), data, i)
                if kind == 0:
                    terminated = True
                    break
                if kind == 1:
                    offsets.append(value)
                elif kind == 5:
                    string_address = value
                elif kind == 10:
                    string_size = value
            if not terminated:
                raise ValueError('Unterminated ELF dynamic table')
        names = []
        if offsets:
            if string_address is None or string_size is None or string_size > MAX_HEADER or len(offsets) > 4096:
                raise ValueError('Invalid ELF dynamic strings')
            locations = [at + string_address - virtual for virtual, amount, at in loads
                         if virtual <= string_address and string_address + string_size <= virtual + amount]
            if len(locations) != 1:
                raise ValueError('Unmapped ELF dynamic strings')
            strings = read(locations[0], string_size)
            for offset in offsets:
                if offset >= len(strings) or b'\0' not in strings[offset:offset + 512]:
                    raise ValueError('Invalid ELF library name')
                name = strings[offset:].split(b'\0', 1)[0].decode('ascii')
                if not name or any(c in name for c in '/\\:\0\r\n'):
                    raise ValueError('Invalid ELF library path')
                names.append(name)
        return {'format': 'ELF', 'architecture': str(struct.unpack_from(endian + 'H', head, 18)[0]), 'imports': sorted(set(names))}


def tree(root, *, exclusions=()):
    root = ordinary(root, directory=True)
    excluded, records, total = {relative(name) for name in exclusions}, {}, 0
    def visit(folder):
        nonlocal total
        for path in sorted(folder.iterdir()):
            name = relative(path.relative_to(root).as_posix())
            if name in excluded:
                continue
            info = path.lstat()
            if getattr(info, 'st_file_attributes', 0) & 0x400:
                raise ValueError('Runtime reparse point refused')
            if stat.S_ISLNK(info.st_mode):
                target = os.readlink(path)
                if Path(target).is_absolute() or not (path.parent / target).resolve().is_relative_to(root):
                    raise ValueError('Runtime link escapes the selected root')
                records[name] = {'link': target, 'sha256': hashlib.sha256(target.encode()).hexdigest(), 'size': len(target.encode())}
            elif stat.S_ISDIR(info.st_mode):
                visit(path)
            elif stat.S_ISREG(info.st_mode):
                record = pin(path)
                total += record['size']
                if total > MAX_BYTES:
                    raise ValueError('Runtime bytes exceed inventory bound')
                native = native_imports(path)
                if native:
                    record['native'] = native
                records[name] = record
            else:
                raise ValueError('Runtime contains a special file')
            if len(records) > MAX_FILES:
                raise ValueError('Runtime file count exceeds inventory bound')
    visit(root)
    return records


def capture(root, *, name, version=None, components=(), exclusions=()):
    root = ordinary(root, directory=True)
    before = tree(root, exclusions=exclusions)
    candidates = []
    for filename, record in before.items():
        if not filename.endswith('.dist-info/METADATA'):
            continue
        if record['size'] > MAX_HEADER or record.get('link'):
            raise ValueError('Invalid Python distribution metadata')
        metadata = BytesParser().parsebytes((root / filename).read_bytes())
        package, package_version = metadata.get('Name'), metadata.get('Version')
        if not package or not package_version:
            raise ValueError('Python distribution lacks name/version')
        requires = []
        for requirement in metadata.get_all('Requires-Dist', []):
            match = re.match(r'([A-Za-z0-9][A-Za-z0-9._-]*)', requirement)
            if not match:
                raise ValueError('Unsupported Python dependency metadata')
            requires.append({'name': match[1], 'requirement': requirement})
        candidates.append({'name': package, 'version': package_version, 'ecosystem': 'pypi', 'license': metadata.get('License-Expression') or 'NOASSERTION',
                           'metadata': filename, 'metadata_sha256': record['sha256'], 'requires': requires})

    def metadata_site(component):
        return PurePosixPath(component['metadata']).parent.parent

    # A nested wheel's original metadata is bundled data, not an installation
    # manifest. Establish installed owners first, independently of traversal order.
    sites = {metadata_site(component) for component in candidates}
    installed_sites = {value for value in sites if value == PurePosixPath('.')
                       or value.name in ('site-packages', 'dist-packages')}
    installed_sites.update(value for value in sites
                           if not any(parent != value and value.is_relative_to(parent)
                                      for parent in sites | installed_sites)
                           and not any(parent.name in ('site-packages', 'dist-packages')
                                       for parent in value.parents))
    found = []
    strong_files = {}
    for component in candidates:
        if metadata_site(component) not in installed_sites:
            continue
        filename = component['metadata']
        record_name = filename.rsplit('/', 1)[0] + '/RECORD'
        if record_name not in before:
            raise ValueError('Installed Python distribution lacks RECORD')
        if record_name in before:
            if before[record_name]['size']>MAX_HEADER or before[record_name].get('link'):
                raise ValueError('Invalid Python RECORD metadata')
            package_files=[]
            hashed_files=set()
            site=(root/filename).parent.parent
            with (root/record_name).open(encoding='utf-8',newline='') as stream:
                for row in csv.reader(stream):
                    if len(row)!=3:raise ValueError('Invalid Python RECORD row')
                    entry_name,hash_value,byte_count=row
                    if Path(entry_name).is_absolute() or '\\' in entry_name or '\0' in entry_name:
                        raise ValueError('Python RECORD escapes its selected root')
                    target=Path(os.path.abspath(site/entry_name))
                    if not target.is_relative_to(root):raise ValueError('Python RECORD escapes its runtime')
                    actual=target.relative_to(root).as_posix()
                    if actual not in before:raise ValueError('Python RECORD claims an absent file')
                    if byte_count and int(byte_count)!=before[actual]['size']:raise ValueError('Python RECORD size differs')
                    if hash_value:
                        algorithm,value=hash_value.split('=',1)
                        if algorithm!='sha256':raise ValueError('Unsupported Python RECORD digest')
                        expected=base64.urlsafe_b64decode(value+'='*((4-len(value)%4)%4)).hex()
                        if expected!=before[actual]['sha256']:raise ValueError('Python RECORD hash differs')
                        if byte_count:hashed_files.add(actual)
                    package_files.append(actual)
            if filename not in hashed_files or record_name not in package_files:
                raise ValueError('Installed Python RECORD does not bind its metadata')
            component['files']=sorted(set(package_files))
            strong_files[filename]=hashed_files
            found.append(component)

    for component in candidates:
        if component in found:
            continue
        nested_site = PurePosixPath(component['metadata']).parent.parent
        record_name = component['metadata'].rsplit('/', 1)[0] + '/RECORD'
        if record_name not in before or before[record_name].get('link') or before[record_name]['size'] > MAX_HEADER:
            raise ValueError('Bundled Python metadata lacks an ordinary RECORD')
        nested_files = {filename for filename in before
                        if PurePosixPath(filename).is_relative_to(nested_site)}
        owners = [owner for owner in found
                  if nested_site != PurePosixPath(owner['metadata']).parent.parent
                  and nested_site.is_relative_to(PurePosixPath(owner['metadata']).parent.parent)
                  and nested_files.issubset(owner['files'])
                  and {component['metadata'], record_name}.issubset(strong_files[owner['metadata']])]
        if len(owners) != 1:
            raise ValueError('Bundled Python metadata requires one complete validated owner')
        owner = owners[0]
        owner.setdefault('vendored_metadata', []).append({
            'name': component['name'], 'version': component['version'],
            'metadata': component['metadata'], 'metadata_sha256': component['metadata_sha256'],
            'record': record_name, 'record_sha256': before[record_name]['sha256']})
    for owner in found:
        if owner.get('vendored_metadata'):
            owner['source'] = 'Nested bundled metadata, no separate installed dist: ' + '; '.join(
                item['name'] + ' ' + item['version'] for item in owner['vendored_metadata'])
    selected = []
    for component in components:
        component = dict(component)
        for filename in component.get('files', []):
            if relative(filename) not in before:
                raise ValueError('Component claims an absent runtime file')
        selected.append(component)
    if tree(root, exclusions=exclusions) != before:
        raise ValueError('Runtime changed during inventory capture')
    return {'format': FORMAT, 'name': name, 'version': version, 'files': before, 'components': [*found, *selected],
            'namespace_stable': True, 'executed_payload': False}


def canonical_digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
