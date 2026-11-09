"""Stage 1/2 historical V1 SQLite inventory/export. Native Windows/Linux local disks.
Original databases are NEVER opened by SQLite. This tool cannot grant or replay work.
"""
from __future__ import annotations
import argparse
import base64
import contextlib
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import sqlite3
import struct
import sys
import tempfile
import time
from datetime import datetime, timezone
from dataclasses import dataclass
import schema_profiles as profiles

FORMAT = 'localai-cowork-v1-sqlite-history/1'
POLICY = {'historical_only': True, 'activate': False, 'resume': False, 'replay': False,
          'regrant': False, 'issue_credentials': False, 'schedule_execution': False}
MAX_SOURCE = 512 * 1024 * 1024
MAX_ROWS = 1_000_000
MAX_CELL = 16 * 1024 * 1024
MAX_EXPORT = 4 * 1024 * 1024 * 1024
MAX_MANIFEST = 2 * 1024 * 1024
MAX_SECONDS = 120
CHUNK = 1024 * 1024

class Refused(Exception):
    """Fail closed; no export authority or partially published bundle."""

def linux_boundary():
    import linux_source_fence
    linux_source_fence.configure(Refused)
    return linux_source_fence

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True,
                      allow_nan=False).encode('ascii')

def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()

def hash_file(path, budget=None):
    result = hashlib.sha256()
    with open(path, 'rb') as stream:
        while chunk := stream.read(CHUNK):
            if budget is not None: budget.check()
            result.update(chunk)
    return result.hexdigest()

def quote(name):
    return '"' + name.replace('"', '""') + '"'

def bounded_path(value, existing=True):
    if sys.platform == 'linux':
        return linux_boundary().path(value, existing)
    path = Path(value)
    if not path.is_absolute() or '..' in path.parts or str(path).startswith('\\\\'):
        raise Refused('Absolute local path without traversal required')
    path = Path(os.path.abspath(path))
    if len(str(path)) > 240: raise Refused('Path too long')
    for component in [*reversed(path.parents), path] if existing else [*reversed(path.parents)]:
        if not component.exists(): raise Refused('Missing path component')
        st = component.lstat()
        if component.is_symlink() or getattr(st, 'st_file_attributes', 0) & 0x400:
            raise Refused('Symlinks and reparse points are forbidden')
    if existing and not path.is_file(): raise Refused('Regular file required')
    if os.name != 'nt': raise Refused('No enforced source write fence on this platform; Windows local disk required')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.GetDriveTypeW.argtypes = [ctypes.c_wchar_p]
    if kernel.GetDriveTypeW(path.anchor) not in (3, 6): raise Refused('Only local fixed/RAM disks are supported')
    return path

class FrozenDirectories:
    """Pin every ancestor namespace with no-delete sharing; allow ordinary child reads/writes."""
    def __init__(self, directories):
        self.directories = sorted({item for directory in directories for item in [directory,*directory.parents]}, key=lambda p: str(p).casefold())
        self.handles = []
        self.linux = None
    def __enter__(self):
        if sys.platform == 'linux':
            self.linux = linux_boundary().Namespaces(self.directories).__enter__()
            return self
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel = kernel
        try:
            for path in self.directories:
                st = path.lstat()
                if not path.is_dir() or path.is_symlink() or getattr(st,'st_file_attributes',0) & 0x400:
                    raise Refused('Directory namespace has a reparse point or wrong type')
                handle = kernel.CreateFileW(str(path),1,3,None,3,0x02200000,None)
                if handle == ctypes.c_void_p(-1).value: raise Refused('Could not fence directory namespace')
                self.handles.append(handle)
                # Directory was opened without following a reparse point; recheck after the handle fences rename.
                st = path.lstat()
                if getattr(st,'st_file_attributes',0) & 0x400: raise Refused('Directory namespace changed')
            return self
        except BaseException:
            self.close(); raise
    def close(self):
        if self.linux is not None:
            self.linux.close()
            return
        for handle in reversed(self.handles): self.kernel.CloseHandle(handle)
        self.handles.clear()
    def __exit__(self,*_): self.close()

class FrozenFiles:
    """Deny write/delete native handles, held until validation and publication finish.
    Acquisition fails when any pre-existing writer/deleter owns an incompatible handle.
    No SQLite call touches these paths. All handles close even on failure.
    """
    def __init__(self, paths, budget=None):
        self.budget = budget
        self.paths = sorted(set(paths), key=lambda path: str(path).casefold())
        self.streams = {}
        self.before = {}
        self.closed = False
        self.namespace = None
        self.linux = None
    def __enter__(self):
        if sys.platform == 'linux':
            self.linux = linux_boundary().Files(self.paths, getattr(self, 'maximum', MAX_SOURCE)).__enter__()
            self.streams = self.linux.streams
            self.before = self.linux.before
            self.namespace = self.linux.namespace
            return self
        import msvcrt
        from ctypes import wintypes
        class FileInfo(ctypes.Structure):
            _fields_ = [('attributes', wintypes.DWORD), ('creation', wintypes.FILETIME),
                        ('access', wintypes.FILETIME), ('write', wintypes.FILETIME),
                        ('volume', wintypes.DWORD), ('size_high', wintypes.DWORD),
                        ('size_low', wintypes.DWORD), ('links', wintypes.DWORD),
                        ('index_high', wintypes.DWORD), ('index_low', wintypes.DWORD)]
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                      ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.GetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.POINTER(FileInfo)]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        try:
            self.namespace = FrozenDirectories([path.parent for path in self.paths]); self.namespace.__enter__()
            total = 0
            for path in self.paths:
                bounded_path(path)
                handle = kernel.CreateFileW(str(path), 0x80000000, 1, None, 3, 0x00200000, None)
                if handle == ctypes.c_void_p(-1).value:
                    raise Refused('Could not acquire deny-write/delete source fence')
                try:
                    info = FileInfo()
                    if not kernel.GetFileInformationByHandle(handle, ctypes.byref(info)):
                        raise Refused('Cannot bind file identity')
                    if info.attributes & (0x400 | 0x10) or info.links != 1:
                        raise Refused('Reparse points, directories and hard links are forbidden')
                    size = (info.size_high << 32) | info.size_low
                    total += size
                    if total > MAX_SOURCE: raise Refused('Source byte limit exceeded')
                    fd = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
                    handle = None
                    try: stream = os.fdopen(fd, 'rb')
                    except BaseException:
                        os.close(fd); raise
                    self.streams[path] = stream
                    self.before[path] = {'bytes': size, 'volume': info.volume,
                                         'file_id': str((info.index_high << 32) | info.index_low)}
                finally:
                    if handle is not None: kernel.CloseHandle(handle)
            return self
        except BaseException:
            self.close()
            raise
    def tick(self):
        if self.budget is not None: self.budget.check()
        elif self.linux is not None: self.linux.check()
    def read(self, path, maximum):
        self.tick()
        stream = self.streams[path]
        stream.seek(0)
        value = stream.read(maximum + 1)
        if len(value) > maximum: raise Refused('Bounded input limit exceeded')
        self.tick()
        return value
    def receipt(self, path):
        self.tick()
        bounded_path(path)
        stream = self.streams[path]
        held = os.fstat(stream.fileno()); current = os.stat(path)
        if (held.st_dev, held.st_ino, held.st_size) != (current.st_dev, current.st_ino, current.st_size):
            raise Refused('Source path identity changed under fence')
        stream.seek(0)
        hasher = hashlib.sha256()
        size = 0
        while chunk := stream.read(CHUNK):
            self.tick(); hasher.update(chunk); size += len(chunk)
        if size != self.before[path]['bytes']: raise Refused('Source size changed under fence')
        self.tick()
        return dict(self.before[path], sha256=hasher.hexdigest(), path=str(path))
    def copy(self, path, destination):
        self.tick()
        stream = self.streams[path]
        stream.seek(0)
        with open(destination, 'xb') as target:
            while chunk := stream.read(CHUNK):
                self.tick(); target.write(chunk)
            target.flush(); os.fsync(target.fileno())
        self.tick()
    def close(self):
        if self.linux is not None:
            self.linux.close()
            self.closed = True
            return
        for stream in self.streams.values(): stream.close()
        self.streams.clear()
        if self.namespace is not None: self.namespace.close()
        self.closed = True
    def __exit__(self, *_): self.close()

def validate_wal(fence, source):
    """Reject truncated, invalid-checksum or uncommitted-tail WALs; never silently lose WAL rows."""
    wal = Path(str(source)+'-wal')
    if wal not in fence.streams or fence.before[wal]['bytes'] == 0: return
    stream = fence.streams[wal]; stream.seek(0)
    header = stream.read(32)
    if len(header) != 32: raise Refused('Truncated WAL header')
    magic, version, page, _, salt1, salt2, check1, check2 = struct.unpack('>8I', header)
    if magic not in (0x377f0682, 0x377f0683) or version != 3007000 or page < 512 or page > 65536 or page & (page-1):
        raise Refused('Unknown WAL format')
    db = fence.streams[source]; db.seek(16); dbpage = int.from_bytes(db.read(2), 'big')
    if (65536 if dbpage == 1 else dbpage) != page: raise Refused('WAL/database page size mismatch')
    def checksum(data, state=(0, 0)):
        words = struct.unpack(('<' if magic == 0x377f0682 else '>')+str(len(data)//4)+'I', data)
        a, b = state
        for i in range(0, len(words), 2):
            a = (a+words[i]+b) & 0xffffffff; b = (b+words[i+1]+a) & 0xffffffff
        return a, b
    state = checksum(header[:24])
    if state != (check1, check2): raise Refused('WAL header checksum mismatch')
    remaining = fence.before[wal]['bytes']-32
    if remaining % (page+24): raise Refused('Truncated WAL frame')
    last_commit = None
    while remaining:
        fence.tick()
        frame = stream.read(24); data = stream.read(page)
        page_number, last_commit, a, b, c, d = struct.unpack('>6I', frame)
        if not page_number or (a, b) != (salt1, salt2): raise Refused('Invalid WAL frame identity')
        state = checksum(data, checksum(frame[:8], state))
        if state != (c, d): raise Refused('WAL frame checksum mismatch')
        remaining -= page+24
    if last_commit == 0: raise Refused('Uncommitted WAL tail requires separate recovery')

def sql_tokens(sql):
    if sql is None: return None
    # Keep literal and identifier case; ignore SQL keyword case and whitespace only.
    tokens = re.findall(r"'(?:[^']|'')*'|\"(?:[^\"]|\"\")*\"|\[(?:[^\]])*\]|`[^`]*`|[A-Za-z_][A-Za-z_0-9]*|\d+(?:\.\d+)?|\S", sql)
    return [token if token[0] in "'\"[`" else token.lower() for token in tokens]

def schema(conn):
    objects = []
    for kind, name, table, sql in conn.execute('SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name'):
        objects.append({'type': kind, 'name': name, 'table': table, 'sql': sql})
    tables = {}
    for obj in objects:
        if obj['type'] == 'table':
            name = obj['name']
            tables[name] = {'columns': [list(row) for row in conn.execute('PRAGMA table_xinfo('+quote(name)+')')],
                            'foreign_keys': [list(row) for row in conn.execute('PRAGMA foreign_key_list('+quote(name)+')')],
                            'indexes': [list(row) for row in conn.execute('PRAGMA index_list('+quote(name)+')')]}
    return {'objects': objects, 'tables': tables}

def signature(value):
    return digest({'objects': [dict(obj, sql=sql_tokens(obj['sql'])) for obj in value['objects']],
                   'tables': value['tables']})

def apply_events(conn, events):
    for event in events:
        if isinstance(event, tuple):
            _, table, column, definition = event
            if column not in [row[1] for row in conn.execute('PRAGMA table_xinfo('+quote(table)+')')]:
                conn.execute('ALTER TABLE '+quote(table)+' ADD COLUMN '+definition)
        else: conn.executescript(event)

def reference_profiles():
    result = {}
    with contextlib.closing(sqlite3.connect(':memory:')) as conn:
        result[('desktop', 0, signature(schema(conn)))] = 'desktop-0-uninitialized'
        conn.executescript('PRAGMA foreign_keys=OFF;PRAGMA legacy_alter_table=ON;CREATE TABLE schema_version(version INTEGER NOT NULL);INSERT INTO schema_version VALUES(0);')
        result[('desktop', 0, signature(schema(conn)))] = 'desktop-0'
        for n, events in enumerate(profiles.DESKTOP_STAGES, 1):
            apply_events(conn, events)
            result[('desktop', n, signature(schema(conn)))] = 'desktop-'+str(n)
            if n in (27, 28):
                # Construct variant on a separate in-memory backup; no change to baseline stages.
                with contextlib.closing(sqlite3.connect(':memory:')) as variant:
                    conn.commit(); conn.backup(variant)
                    variant.executescript(profiles.DESKTOP_BACKPATCH)
                    result[('desktop', n, signature(schema(variant)))] = 'desktop-'+str(n)+'-backend-selection'
    with contextlib.closing(sqlite3.connect(':memory:')) as conn:
        conn.executescript(profiles.DAEMON_BASE)
        result[('daemon', None, signature(schema(conn)))] = 'daemon-base-13'
        conn.executescript(profiles.DAEMON_SYNC)
        result[('daemon', None, signature(schema(conn)))] = 'daemon-sync-15'
    return result

REFERENCE_PROFILES = reference_profiles()

@dataclass
class Budget:
    deadline: float
    rows: int = 0
    output: int = 0
    def check(self):
        if time.monotonic() > self.deadline: raise Refused('Operation time budget exceeded')
        if sys.platform == 'linux': linux_boundary().check_active()
    def sql_progress(self):
        try:
            self.check()
            return 0
        except (Refused, OSError):
            return 1
    def row(self):
        self.check(); self.rows += 1
        if self.rows > MAX_ROWS: raise Refused('Row budget exceeded')
    def emit(self, data):
        self.check(); self.output += len(data)
        if self.output > MAX_EXPORT: raise Refused('Export byte budget exceeded')

@contextlib.contextmanager
def database(path, immutable=True):
    uri = path.as_uri() + '?mode=ro' + ('&immutable=1' if immutable else '')
    with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=0)) as conn:
        conn.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, MAX_CELL)
        conn.setlimit(sqlite3.SQLITE_LIMIT_SQL_LENGTH, MAX_CELL)
        conn.execute('PRAGMA query_only=ON'); conn.execute('PRAGMA trusted_schema=OFF')
        conn.execute('PRAGMA temp_store=MEMORY'); conn.execute('PRAGMA cache_size=-8192')
        yield conn

def inspect(conn, family, budget):
    conn.set_progress_handler(budget.sql_progress, 1000)
    integrity = list(conn.execute('PRAGMA integrity_check(1)'))
    if integrity != [('ok',)]: raise Refused('SQLite integrity check failed')
    if conn.execute('PRAGMA foreign_key_check').fetchone() is not None:
        raise Refused('Foreign key violation in historical source')
    value = schema(conn)
    version = None if family == 'daemon' else 0
    if family == 'desktop' and 'schema_version' in value['tables']:
        markers = conn.execute('SELECT version,typeof(version) FROM schema_version LIMIT 2').fetchall()
        if len(markers) != 1 or markers[0][1] != 'integer' or not 0 <= markers[0][0] <= 28:
            raise Refused('Invalid, duplicate or unknown desktop version marker')
        version = markers[0][0]
    match = REFERENCE_PROFILES.get((family, version, signature(value)))
    if match is None: raise Refused('Unknown schema structure or family')
    if conn.execute('PRAGMA user_version').fetchone()[0] != 0 or conn.execute('PRAGMA application_id').fetchone()[0] != 0:
        raise Refused('Unknown SQLite application/user version')
    counts = {}
    for table in value['tables']:
        budget.check()
        count = conn.execute('SELECT count(*) FROM '+quote(table)).fetchone()[0]
        if count > MAX_ROWS or sum(counts.values()) + count > MAX_ROWS: raise Refused('Row budget exceeded')
        counts[table] = count
    return {'family': family, 'version': version, 'profile': match, 'schema': value,
            'schema_sha256': signature(value), 'row_counts': counts,
            'sqlite_metadata': {name: conn.execute('PRAGMA '+name).fetchone()[0]
                                for name in ('user_version', 'application_id', 'schema_version', 'page_size')}}

def cell(kind, value):
    if kind == 'null': return {'type': 'null'}
    if kind == 'integer': return {'type': 'integer', 'decimal': str(value)}
    if kind == 'real':
        if not math.isfinite(value): raise Refused('Nonfinite SQLite REAL is not canonical')
        return {'type': 'real', 'ieee754_binary64_be': struct.pack('>d', value).hex()}
    if kind in ('text', 'blob'):
        if len(value) > MAX_CELL: raise Refused('Cell byte budget exceeded')
        return {'type': kind, 'base64': base64.b64encode(value).decode('ascii')}
    raise Refused('Unsupported SQLite storage class')

def rows(conn, table, columns, budget):
    names = [column[1] for column in columns]
    # Known V1 rowid tables only. Include physical rowid to disambiguate nullable TEXT PKs.
    if any(name.lower() in ('rowid', '_rowid_', 'oid') for name in names):
        raise Refused('Rowid shadowing is unsupported')
    pk = [column[1] for column in sorted(columns, key=lambda column: column[5]) if column[5]]
    selection = ['_rowid_']
    for name in names:
        q = quote(name)
        selection.extend(['typeof('+q+')', "CASE WHEN typeof("+q+")='text' THEN CAST("+q+' AS BLOB) ELSE '+q+' END'])
    order = [quote(name) for name in pk] + ['_rowid_']
    query = 'SELECT '+','.join(selection)+' FROM '+quote(table)+' ORDER BY '+','.join(order)
    for row in conn.execute(query):
        budget.row()
        yield canonical({'rowid': str(row[0]), 'cells': [cell(row[i], row[i+1]) for i in range(1, len(row), 2)]}) + b'\n'

def content_receipts(conn, details, budget, output=None):
    receipts = {}
    for ordinal, (table, metadata) in enumerate(sorted(details['schema']['tables'].items())):
        name = f'table-{ordinal:03d}.jsonl'
        hasher = hashlib.sha256(); count = 0; size = 0
        with open(output/name, 'xb') if output else contextlib.nullcontext(None) as stream:
            for data in rows(conn, table, metadata['columns'], budget):
                hasher.update(data); count += 1; size += len(data); budget.emit(data)
                if stream: stream.write(data)
            if stream: stream.flush(); os.fsync(stream.fileno())
        if count != details['row_counts'][table]: raise Refused('Content count differs from inventory')
        receipts[table] = {'file': name, 'rows': count, 'bytes': size, 'sha256': hasher.hexdigest(),
                           'columns': [column[1] for column in metadata['columns']]}
    return receipts

def output_path(value, inputs):
    path = bounded_path(value, False)
    if path.exists(): raise Refused('Output must be a new exclusive directory')
    for source in inputs:
        if path == source.parent or source.parent in path.parents or path in source.parents:
            raise Refused('Output overlaps a source tree')
    return path

def write_json(path, value):
    data = canonical(value) + b'\n'
    with open(path, 'xb') as stream: stream.write(data); stream.flush(); os.fsync(stream.fileno())
    return hashlib.sha256(data).hexdigest()

def read_json(fence, path):
    raw = fence.read(path, MAX_MANIFEST)
    depth = 0; inside = False; escaped = False
    for byte in raw:
        if inside:
            if escaped: escaped = False
            elif byte == 92: escaped = True
            elif byte == 34: inside = False
        elif byte == 34: inside = True
        elif byte in (91,123):
            depth += 1
            if depth > 32: raise Refused('JSON nesting limit exceeded')
        elif byte in (93,125): depth -= 1
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value: raise Refused('Duplicate JSON key')
            value[key] = item
        return value
    try: value = json.loads(raw, object_pairs_hook=unique, parse_constant=lambda _: (_ for _ in ()).throw(Refused('Nonfinite JSON')))
    except (UnicodeError, ValueError, RecursionError) as error: raise Refused('Invalid JSON input') from error
    if not isinstance(value, dict): raise Refused('Object manifest required')
    return value, hashlib.sha256(raw).hexdigest()

def bundle_manifest(value):
    value = dict(value)
    value['manifest_sha256'] = digest(value)
    return value

def verify_manifest(value):
    copy = dict(value); seal = copy.pop('manifest_sha256', None)
    if not isinstance(seal, str) or seal != digest(copy): raise Refused('Manifest content seal mismatch')
    if value.get('format') != FORMAT or value.get('execution_policy') != POLICY:
        raise Refused('Unsupported manifest or replay authority')

def stage_identity(path):
    st = path.lstat()
    if path.is_symlink() or getattr(st, 'st_file_attributes', 0) & 0x400:
        raise Refused('Private stage namespace changed to a reparse point')
    return st.st_dev, st.st_ino

def publish(stage, output, identity, budget=None, final_check=None):
    if sys.platform == 'linux':
        return linux_boundary().publish(stage, output, identity, None if budget is None else budget.deadline, final_check)
    if budget is not None: budget.check()
    if final_check is not None: final_check()
    if stage_identity(stage) != identity: raise Refused('Private stage identity changed')
    for file in stage.iterdir():
        if file.is_file(): os.chmod(file, 0o444)
    # Windows rename refuses an existing destination, including directories.
    os.rename(stage, output)

def cleanup_stage(stage, identity):
    if sys.platform == 'linux':
        return linux_boundary().cleanup(stage, identity)
    if not stage.exists(): return
    if stage_identity(stage) != identity: raise Refused('Refuse cleanup of unowned directory')
    for file in stage.iterdir():
        if file.is_file(): os.chmod(file, 0o600)
    shutil.rmtree(stage)

def inventory(source, family, output):
    source = bounded_path(source)
    related = lambda: [path for path in [source, *[Path(str(source)+suffix) for suffix in ('-wal', '-shm', '-journal')]] if path.exists()]
    paths = related()
    output = output_path(output, paths)
    namespace = FrozenDirectories([output.parent]); namespace.__enter__()
    stage = None; stage_lock = None; identity = None
    budget = Budget(time.monotonic()+MAX_SECONDS)
    try:
        stage = Path(tempfile.mkdtemp(prefix='.m9-stage-', dir=output.parent))
        identity = stage_identity(stage)
        stage_lock = FrozenDirectories([stage]); stage_lock.__enter__()
        with FrozenFiles(paths, budget) as fence:
            before = [fence.receipt(path) for path in paths]
            fence.streams[source].seek(0)
            if fence.streams[source].read(16) != b'SQLite format 3\x00': raise Refused('SQLite header required')
            journal = Path(str(source)+'-journal')
            if journal in paths and fence.before[journal]['bytes']:
                raise Refused('Nonempty rollback journal requires separate verified recovery')
            validate_wal(fence, source)
            copies = stage/'copy'; copies.mkdir()
            copied = copies/'source.sqlite3'
            for path in paths: fence.copy(path, Path(str(copied)+str(path)[len(str(source)):]))
            snapshot = stage/'snapshot.sqlite3'
            with database(copied, immutable=False) as conn:
                with contextlib.closing(sqlite3.connect(snapshot)) as target:
                    conn.backup(target, pages=256, progress=lambda *_: budget.check(), sleep=0)
            with database(snapshot) as conn:
                details = inspect(conn, family, budget)
                contents = content_receipts(conn, details, budget)
            if paths != related() or before != [fence.receipt(path) for path in paths]:
                raise Refused('Source provenance changed under fence')
            # Only private staging is removed; original source has never been SQLite-opened.
            shutil.rmtree(copies)
            value = bundle_manifest({'format': FORMAT, 'stage': 'source_inventory',
                'execution_policy': POLICY, 'source_files': before,
                'snapshot': {'file': 'snapshot.sqlite3', 'bytes': snapshot.stat().st_size, 'sha256': hash_file(snapshot, budget)},
                'source_bindings': profiles.SOURCE_BINDINGS, 'details': details,
                'content': contents, 'content_sha256': digest(contents),
                'canonical_cells': 'integer-decimal/null/text-utf8-bytes-base64/blob-base64/finite-real-ieee754-binary64-be',
                'sqlite_engine': sqlite3.sqlite_version, 'freeze': linux_boundary().FREEZE if sys.platform == 'linux' else 'windows-deny-write-and-delete-held-through-publication'})
            file_sha = write_json(stage/'inventory.json', value)
            budget.check()
            stage_lock.close()
            publish(stage, output, identity, budget)
            stage = None
            return {'inventory': str(output/'inventory.json'), 'approved_inventory_sha256': file_sha,
                    'manifest_sha256': value['manifest_sha256'], 'profile': details['profile'], 'rows': sum(details['row_counts'].values())}
    finally:
        if stage_lock is not None: stage_lock.close()
        try:
            if stage is not None and identity is not None: cleanup_stage(stage, identity)
        finally: namespace.close()

def authority_check(authority, inventory_sha):
    if set(authority) != {'format', 'state', 'purpose', 'inventory_sha256', 'expires_at'}:
        raise Refused('Unknown approval authority fields')
    if authority['format'] != FORMAT or authority['state'] != 'approved' or authority['purpose'] != 'historical_export':
        raise Refused('Approval revoked or wrong purpose')
    if authority['inventory_sha256'] != inventory_sha: raise Refused('Approval binds another inventory')
    try:
        expiry = datetime.fromisoformat(authority['expires_at'].replace('Z', '+00:00'))
        if expiry.tzinfo is None or datetime.now(timezone.utc) >= expiry: raise Refused('Approval expired')
    except (ValueError, TypeError, AttributeError) as error: raise Refused('Invalid approval expiry') from error

def export(inventory_path, approved_inventory_sha, authority_path, authority_sha, output):
    inventory_path = bounded_path(inventory_path)
    authority_path = bounded_path(authority_path)
    snapshot = bounded_path(inventory_path.parent/'snapshot.sqlite3')
    output = output_path(output, [inventory_path, authority_path, snapshot])
    namespace = FrozenDirectories([output.parent]); namespace.__enter__()
    stage = None; stage_lock = None; identity = None
    budget = Budget(time.monotonic()+MAX_SECONDS)
    try:
        stage = Path(tempfile.mkdtemp(prefix='.m9-stage-', dir=output.parent))
        identity = stage_identity(stage)
        stage_lock = FrozenDirectories([stage]); stage_lock.__enter__()
        with FrozenFiles([inventory_path, authority_path, snapshot], budget) as fence:
            before = {str(path): fence.receipt(path) for path in fence.paths}
            value, inventory_sha = read_json(fence, inventory_path)
            authority, actual_authority_sha = read_json(fence, authority_path)
            if inventory_sha != approved_inventory_sha or actual_authority_sha != authority_sha:
                raise Refused('Pinned inventory or authority byte digest mismatch')
            verify_manifest(value)
            if value.get('stage') != 'source_inventory' or value.get('source_bindings') != profiles.SOURCE_BINDINGS:
                raise Refused('Unknown inventory stage or source binding')
            expected_snapshot = {'file': 'snapshot.sqlite3', 'bytes': before[str(snapshot)]['bytes'], 'sha256': before[str(snapshot)]['sha256']}
            if value.get('snapshot') != expected_snapshot: raise Refused('Snapshot provenance mismatch')
            authority_check(authority, inventory_sha)
            copy = stage/'copy.sqlite3'; fence.copy(snapshot, copy)
            with database(copy) as conn:
                details = inspect(conn, value['details']['family'], budget)
                if details != value['details']: raise Refused('Inventory schema/count metadata mismatch')
                contents = content_receipts(conn, details, budget, stage)
            copy.unlink()
            if contents != value.get('content') or digest(contents) != value.get('content_sha256'):
                raise Refused('Inventory content mismatch')
            if before != {str(path): fence.receipt(path) for path in fence.paths}: raise Refused('Approved inputs changed')
            authority_check(authority, inventory_sha)
            manifest = bundle_manifest({'format': FORMAT, 'stage': 'readonly_export', 'execution_policy': POLICY,
                'inventory_sha256': inventory_sha, 'authority_sha256': actual_authority_sha,
                'inventory_manifest_sha256': value['manifest_sha256'], 'source_files': value['source_files'],
                'snapshot': value['snapshot'], 'source_bindings': profiles.SOURCE_BINDINGS,
                'details': details, 'content': contents, 'content_sha256': digest(contents),
                'canonical_cells': value['canonical_cells']})
            manifest_sha = write_json(stage/'export.json', manifest)
            budget.check()
            stage_lock.close()
            publish(stage, output, identity, budget, lambda:authority_check(authority,inventory_sha))
            stage = None
            return {'export': str(output/'export.json'), 'export_sha256': manifest_sha,
                    'manifest_sha256': manifest['manifest_sha256'], 'rows': sum(details['row_counts'].values())}
    finally:
        if stage_lock is not None: stage_lock.close()
        try:
            if stage is not None and identity is not None: cleanup_stage(stage, identity)
        finally: namespace.close()

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    first = sub.add_parser('inventory'); first.add_argument('--source', required=True)
    first.add_argument('--family', choices=['desktop','daemon'], required=True); first.add_argument('--output', required=True)
    second = sub.add_parser('export'); second.add_argument('--inventory', required=True)
    second.add_argument('--approved-inventory-sha256', required=True); second.add_argument('--authority', required=True)
    second.add_argument('--authority-sha256', required=True); second.add_argument('--output', required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == 'inventory': result = inventory(args.source, args.family, args.output)
        else: result = export(args.inventory, args.approved_inventory_sha256, args.authority, args.authority_sha256, args.output)
        print(canonical(result).decode('ascii')); return 0
    except (Refused, sqlite3.Error, OSError, KeyError, TypeError, ValueError) as error:
        print('REFUSED: '+str(error), file=sys.stderr); return 2

if __name__ == '__main__': raise SystemExit(main())
