"""Bounded, deny-write/delete IO for the offline quarantine adapter.

No original database is opened by SQLite. Native handles stay held through
publication; only newly created, identity-checked private stages are writable.
"""
from __future__ import annotations
import base64
import ctypes
import hashlib
import json
import math
import os
import sys
from pathlib import Path
import re
import struct
import tempfile
import time
import v1_sqlite_export as v1

Refused = v1.Refused
MAX_BUNDLE = 4 * 1024 * 1024 * 1024
MAX_QUARANTINE = 12 * 1024 * 1024 * 1024
MAX_LINE = 256 * 1024 * 1024


class FileFence(v1.FrozenFiles):
    """Same native fence as Stage 1; bounded to the Stage 2 export byte limit."""
    def __init__(self, paths, budget=None, maximum=MAX_BUNDLE+4*v1.MAX_MANIFEST):
        super().__init__(paths,budget)
        self.maximum = maximum
    def __enter__(self):
        if sys.platform == 'linux':
            return super().__enter__()
        if os.name != 'nt':
            raise Refused('Windows enforced write/namespace fence required')
        import msvcrt
        from ctypes import wintypes as w
        class Info(ctypes.Structure):
            _fields_ = [('attributes', w.DWORD), ('creation', w.FILETIME),
                        ('access', w.FILETIME), ('write', w.FILETIME),
                        ('volume', w.DWORD), ('high', w.DWORD), ('low', w.DWORD),
                        ('links', w.DWORD), ('index_high', w.DWORD), ('index_low', w.DWORD)]
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p, w.DWORD, w.DWORD, w.HANDLE]
        kernel.CreateFileW.restype = w.HANDLE
        kernel.GetFileInformationByHandle.argtypes = [w.HANDLE, ctypes.POINTER(Info)]
        kernel.CloseHandle.argtypes = [w.HANDLE]
        try:
            self.namespace = v1.FrozenDirectories([p.parent for p in self.paths])
            self.namespace.__enter__()
            total = 0
            for path in self.paths:
                v1.bounded_path(path)
                handle = kernel.CreateFileW(str(path), 0x80000000, 1, None, 3, 0x00200000, None)
                if handle == ctypes.c_void_p(-1).value:
                    raise Refused('Cannot acquire input write/delete fence')
                try:
                    info = Info()
                    if not kernel.GetFileInformationByHandle(handle, ctypes.byref(info)):
                        raise Refused('Cannot bind input identity')
                    if info.attributes & (0x400 | 0x10) or info.links != 1:
                        raise Refused('Input reparse point, directory or hard link')
                    size = info.high << 32 | info.low
                    total += size
                    if total > self.maximum:
                        raise Refused('Complete bundle byte limit exceeded')
                    fd = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
                    handle = None
                    try:
                        self.streams[path] = os.fdopen(fd, 'rb')
                    except BaseException:
                        os.close(fd)
                        raise
                    self.before[path] = {'bytes': size, 'volume': info.volume,
                                         'file_id': str(info.index_high << 32 | info.index_low)}
                finally:
                    if handle is not None:
                        kernel.CloseHandle(handle)
            return self
        except BaseException:
            self.close()
            raise


def parse(raw, maximum=v1.MAX_MANIFEST):
    if len(raw) > maximum:
        raise Refused('JSON input exceeds bound')
    depth = 0
    inside = escaped = False
    for byte in raw:
        if inside:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                inside = False
        elif byte == 34:
            inside = True
        elif byte in (91, 123):
            depth += 1
            if depth > 32:
                raise Refused('JSON nesting exceeds bound')
        elif byte in (93, 125):
            depth -= 1
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise Refused('Duplicate JSON key')
            result[key] = value
        return result
    try:
        value = json.loads(raw, object_pairs_hook=unique,
                           parse_constant=lambda _: (_ for _ in ()).throw(Refused('Nonfinite JSON')))
    except (UnicodeError, ValueError, RecursionError) as error:
        raise Refused('Invalid JSON') from error
    if not isinstance(value, dict):
        raise Refused('JSON object required')
    return value


def fields(value, expected):
    if not isinstance(value, dict) or set(value) != set(expected):
        raise Refused('Unknown or missing format fields')


def sha(value):
    if not isinstance(value, str) or re.fullmatch('[0-9a-f]{64}', value) is None:
        raise Refused('Canonical SHA-256 required')
    return value


def integer(value, minimum=-(1 << 63), maximum=(1 << 63)-1):
    if not isinstance(value, str) or re.fullmatch(r'0|-?[1-9][0-9]*', value) is None:
        raise Refused('Canonical integer required')
    number = int(value)
    if not minimum <= number <= maximum:
        raise Refused('Integer out of range')
    return number


def natural(value, maximum=v1.MAX_ROWS):
    if type(value) is not int or not 0 <= value <= maximum:
        raise Refused('Invalid count or size')
    return value


def decode_cell(value):
    if not isinstance(value, dict):
        raise Refused('Typed cell object required')
    kind = value.get('type')
    if kind == 'null':
        fields(value, ['type'])
        return None, None
    if kind == 'integer':
        fields(value, ['type', 'decimal'])
        return integer(value['decimal']), None
    if kind == 'real':
        fields(value, ['type', 'ieee754_binary64_be'])
        bits = value['ieee754_binary64_be']
        if not isinstance(bits, str) or re.fullmatch('[0-9a-f]{16}', bits) is None:
            raise Refused('Invalid real bit encoding')
        result = struct.unpack('>d', bytes.fromhex(bits))[0]
        if not math.isfinite(result):
            raise Refused('Nonfinite real')
        return result, None
    if kind in ('text', 'blob'):
        fields(value, ['type', 'base64'])
        encoded = value['base64']
        if not isinstance(encoded, str) or len(encoded) > (v1.MAX_CELL+2)//3*4:
            raise Refused('Cell exceeds byte bound')
        try:
            result = base64.b64decode(encoded, validate=True)
        except (ValueError, UnicodeError) as error:
            raise Refused('Invalid cell base64') from error
        if len(result) > v1.MAX_CELL or base64.b64encode(result).decode('ascii') != encoded:
            raise Refused('Noncanonical or oversized cell base64')
        # CAST(? AS TEXT) preserves even invalid SQLite UTF-8 bytes.
        return result, kind
    raise Refused('Unknown cell type')


class Stage:
    def __init__(self, output, inputs):
        self.output = v1.output_path(output, inputs)
        self.directory = None
        self.lock = None
        self.identity = None
        self.namespace = None
    def __enter__(self):
        self.namespace = v1.FrozenDirectories([self.output.parent])
        self.namespace.__enter__()
        try:
            self.directory = Path(tempfile.mkdtemp(prefix='.m9-quarantine-', dir=self.output.parent))
            self.identity = v1.stage_identity(self.directory)
            self.lock = v1.FrozenDirectories([self.directory])
            self.lock.__enter__()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise
    def publish(self, operation_budget=None):
        if self.lock is not None:
            self.lock.close()
        v1.publish(self.directory, self.output, self.identity, operation_budget)
        self.directory = None
    def __exit__(self, *_):
        if self.lock is not None:
            self.lock.close()
        try:
            if self.directory is not None:
                v1.cleanup_stage(self.directory, self.identity)
        finally:
            if self.namespace is not None:
                self.namespace.close()


def budget():
    return v1.Budget(time.monotonic()+v1.MAX_SECONDS)


def receipt_set(fence):
    return {str(path): fence.receipt(path) for path in fence.paths}


def cold(path):
    for suffix in ('-wal', '-shm', '-journal'):
        if Path(str(path)+suffix).exists():
            raise Refused('An explicitly cold standalone snapshot without sidecars is required')
