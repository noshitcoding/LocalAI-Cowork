"""LocalAI Cowork document engine for the document_read and document_write tools.

Usage: python -I document_engine.py <stage>   |   python -I document_engine.py --probe

One self-contained file: standard library plus the pinned packages (python-docx, openpyxl,
python-pptx, pypdf, pypdfium2, Pillow, lxml, reportlab). No network and no subprocess; an audit
hook confines reads to the stage and the Python installation and writes to <stage>/work and
<stage>/out. Contract: .local/document-tools-20261018/INTERFACE.md. The model-facing spec
vocabulary is the `help` operation (HELP below) and ENGINE-NOTES.md.
"""
from __future__ import annotations

import copy
import contextlib
import datetime
import decimal
import hashlib
import io
import json
import math
import os
import re
import sys
import tempfile
import warnings
import zipfile

ENGINE_VERSION = '1'
REQUEST_FORMAT = 'cowork-document-request/1'
RECEIPT_FORMAT = 'cowork-document-receipt/1'
PROBE_FORMAT = 'cowork-document-probe/1'
MIB = 1024 * 1024
DEFAULT_LIMITS = {'max_output_bytes': 64 * MIB, 'max_outputs': 64, 'max_result_bytes': 32768,
                  'max_pages_read': 500, 'max_pages_render': 200, 'max_preview_images': 4,
                  'preview_max_edge': 1600, 'preview_max_bytes': 2 * MIB, 'max_image_pixels': 50_000_000}
LIMIT_CAPS = {'max_output_bytes': 256 * MIB, 'max_outputs': 256, 'max_result_bytes': 512 * 1024,
              'max_pages_read': 5000, 'max_pages_render': 1000, 'max_preview_images': 16,
              'preview_max_edge': 4096, 'preview_max_bytes': 8 * MIB, 'max_image_pixels': 200_000_000}
MAX_RECEIPT_BYTES = MIB
MAX_REQUEST_BYTES = 16 * MIB
MAX_INPUTS = 64
MAX_INPUT_BYTES = 64 * MIB
MAX_TEXT_INPUT_BYTES = 16 * MIB
MAX_LOG_BYTES = 16 * MIB
ZIP_MAX_ENTRIES = 10_000
ZIP_MAX_EXPANDED = 64 * MIB
MAX_STRING_CHARS = 128_000
MAX_BLOCKS = 5_000
MAX_LIST_ITEMS = 5_000
MAX_TABLE_CELLS = 50_000
MAX_SHEET_CELLS = 1_000_000
MAX_WRITE_CELLS = 1_000_000
MAX_STYLE_CELLS = 200_000
MAX_SLIDES = 500
MAX_PATCHES = 2_000
MAX_PDF_PAGES_WRITE = 5_000
MAX_DIAGNOSTICS = 40
ITEM_TEXT_CHARS = 4_000
CELL_TEXT_CHARS = 300
PINNED = {'docx': ('python-docx', '1.2.0'), 'openpyxl': ('openpyxl', '3.1.5'), 'pptx': ('python-pptx', '1.0.2'),
          'pypdf': ('pypdf', '6.20.0'), 'pypdfium2': ('pypdfium2', '5.12.1'), 'PIL': ('pillow', '12.3.0'),
          'lxml': ('lxml', '6.1.3')}

OCR_BINDING_VERSION = '2.12.0'
OCR_MODELS = {
    'deu': ('19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d', 1525436),
    'eng': ('7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2', 4113088),
}
OCR_MAX_PAGES = 16
OCR_MAX_PAGE_PIXELS = 12_000_000
OCR_MAX_TOTAL_PIXELS = 60_000_000
OCR_MAX_WORDS = 100_000

MEDIA_TYPES = {
    'docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    'xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    'pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
    'pdf': 'application/pdf', 'png': 'image/png', 'jpg': 'image/jpeg', 'jpeg': 'image/jpeg', 'gif': 'image/gif',
    'webp': 'image/webp', 'bmp': 'image/bmp', 'tif': 'image/tiff', 'tiff': 'image/tiff',
    'odt': 'application/vnd.oasis.opendocument.text', 'ods': 'application/vnd.oasis.opendocument.spreadsheet',
    'odp': 'application/vnd.oasis.opendocument.presentation', 'rtf': 'application/rtf', 'html': 'text/html',
    'txt': 'text/plain', 'md': 'text/markdown', 'csv': 'text/csv', 'tex': 'application/x-tex', 'json': 'application/json'}
EXTENSION_KINDS = {
    'docx': 'docx', 'docm': 'docx', 'dotx': 'docx', 'dotm': 'docx', 'xlsx': 'xlsx', 'xlsm': 'xlsx', 'xltx': 'xlsx',
    'xltm': 'xlsx', 'pptx': 'pptx', 'pptm': 'pptx', 'potx': 'pptx', 'potm': 'pptx', 'ppsx': 'pptx', 'pdf': 'pdf',
    'png': 'image', 'jpg': 'image', 'jpeg': 'image', 'gif': 'image', 'bmp': 'image', 'tif': 'image',
    'tiff': 'image', 'webp': 'image'}
TEXT_EXTENSIONS = {'txt', 'md', 'markdown', 'csv', 'tsv', 'tex', 'bib', 'sty', 'cls', 'json', 'log', 'xml',
                   'html', 'htm', 'yaml', 'yml', 'rst', 'ini', 'toml'}
LEGACY_OFFICE = {'doc', 'xls', 'ppt', 'odt', 'ods', 'odp', 'rtf'}
MACRO_EXTENSIONS = {'docm', 'dotm', 'xlsm', 'xltm', 'pptm', 'potm'}


class EngineError(Exception):
    """A handled failure that becomes an ok:false receipt with exit code 2."""

    def __init__(self, code, message, path=None):
        super().__init__(message)
        self.code, self.message, self.path = code, message, path


def fail(code, message, path=None):
    raise EngineError(code, message, path)


def need(condition, code, message, path=None):
    if not condition:
        raise EngineError(code, message, path)


# ---------------------------------------------------------------------------------------------
# Sandbox: an audit hook that keeps library code inside the stage.
# ---------------------------------------------------------------------------------------------

_WRITE_EVENTS = {'os.mkdir': (0,), 'os.remove': (0,), 'os.rmdir': (0,), 'os.rename': (0, 1), 'os.truncate': (0,),
                 'os.chmod': (0,), 'os.chown': (0,), 'os.utime': (0,), 'os.link': (0, 1), 'os.symlink': (0, 1),
                 'shutil.rmtree': (0,), 'shutil.copyfile': (1,), 'shutil.copytree': (1,), 'shutil.move': (1,),
                 'shutil.make_archive': (0,), 'shutil.unpack_archive': (1,)}
_READ_EVENTS = {'os.listdir': (0,), 'os.scandir': (0,), 'glob.glob': (0,), 'shutil.copyfile': (0,),
                'shutil.copytree': (0,), 'shutil.unpack_archive': (0,)}
_DENIED_EVENTS = ('subprocess.', 'os.system', 'os.exec', 'os.posix_spawn', 'os.spawn', 'os.startfile', 'os.fork',
                  'os.forkpty', 'os.kill', 'os.killpg', 'pty.', 'socket.', 'urllib.', 'http.client.', 'ftplib.',
                  'smtplib.', 'imaplib.', 'poplib.', 'nntplib.', 'telnetlib.', 'webbrowser.', 'winreg.Create',
                  'winreg.Delete', 'winreg.Set', 'winreg.Connect')
_WRITE_FLAGS = os.O_WRONLY | os.O_RDWR | os.O_APPEND | os.O_CREAT | os.O_TRUNC


class Sandbox:
    def __init__(self, stage):
        self.busy = False
        roots = set()
        for value in (sys.prefix, sys.base_prefix, sys.exec_prefix, sys.base_exec_prefix, *sys.path):
            if value:
                roots.add(self.norm(value))
        self.writable = ()
        if stage:
            self.writable = (self.norm(os.path.join(stage, 'work')), self.norm(os.path.join(stage, 'out')))
            roots.add(self.norm(stage))
        self.readable = tuple(sorted(roots))

    @staticmethod
    def norm(value):
        return os.path.normcase(os.path.realpath(os.fsdecode(value)))

    @staticmethod
    def inside(path, roots):
        for root in roots:
            if path == root or path.startswith(root.rstrip('\\/') + os.sep):
                return True
        return False

    def check(self, value, write, event):
        if value is None or isinstance(value, int) or not isinstance(value, (str, bytes, os.PathLike)):
            return
        path = self.norm(value)
        if write:
            if not self.inside(path, self.writable):
                raise PermissionError('sandbox: %s may only write below <stage>/work or <stage>/out' % event)
        elif not self.inside(path, self.readable):
            raise PermissionError('sandbox: %s may only read inside the stage' % event)

    def __call__(self, event, args):
        if self.busy:
            return
        self.busy = True
        try:
            if event == 'open':
                mode = args[1] if len(args) > 1 else None
                flags = args[2] if len(args) > 2 and isinstance(args[2], int) else 0
                write = (isinstance(mode, str) and any(c in mode for c in 'wax+')) or bool(flags & _WRITE_FLAGS)
                self.check(args[0] if args else None, write, event)
            elif event in _WRITE_EVENTS or event in _READ_EVENTS:
                for index in _WRITE_EVENTS.get(event, ()):
                    if len(args) > index:
                        self.check(args[index], True, event)
                for index in _READ_EVENTS.get(event, ()):
                    if len(args) > index:
                        self.check(args[index], False, event)
            elif event == 'ctypes.dlopen':
                name = args[0] if args else None
                if isinstance(name, (str, bytes)) and ('/' in os.fsdecode(name) or '\\' in os.fsdecode(name)):
                    self.check(name, False, event)
            elif event.startswith(_DENIED_EVENTS):
                raise PermissionError('sandbox: %s is not allowed in the document engine' % event)
        finally:
            self.busy = False


# ---------------------------------------------------------------------------------------------
# Small validation helpers. Paths are model-facing (spec.blocks[3].rows).
# ---------------------------------------------------------------------------------------------

def as_dict(value, path, allowed=None, required=()):
    need(isinstance(value, dict), 'invalid_spec', '%s must be an object' % path, path)
    if allowed is not None:
        extra = sorted(set(value) - set(allowed))
        if extra:
            fail('invalid_spec', 'unknown key %s.%s; allowed keys: %s' % (path, extra[0], ', '.join(sorted(allowed))),
                 '%s.%s' % (path, extra[0]))
    for key in required:
        need(key in value and value[key] is not None, 'invalid_spec', '%s.%s is required' % (path, key),
             '%s.%s' % (path, key))
    return value


def as_list(value, path, max_len, min_len=0):
    need(isinstance(value, list), 'invalid_spec', '%s must be a list' % path, path)
    need(len(value) >= min_len, 'invalid_spec', '%s needs at least %d entries' % (path, min_len), path)
    need(len(value) <= max_len, 'limit', '%s has %d entries; the limit is %d' % (path, len(value), max_len), path)
    return value


def as_int(value, path, lo=None, hi=None):
    need(type(value) is int, 'invalid_spec', '%s must be an integer' % path, path)
    need(lo is None or value >= lo, 'invalid_spec', '%s must be >= %s' % (path, lo), path)
    need(hi is None or value <= hi, 'invalid_spec', '%s must be <= %s' % (path, hi), path)
    return value


def as_num(value, path, lo=None, hi=None):
    need(type(value) in (int, float) and math.isfinite(value), 'invalid_spec', '%s must be a finite number' % path,
         path)
    need(lo is None or value >= lo, 'invalid_spec', '%s must be >= %s' % (path, lo), path)
    need(hi is None or value <= hi, 'invalid_spec', '%s must be <= %s' % (path, hi), path)
    return value


def as_bool(value, path):
    need(type(value) is bool, 'invalid_spec', '%s must be true or false' % path, path)
    return value


def as_str(value, path, max_chars=MAX_STRING_CHARS, choices=None, pattern=None):
    need(isinstance(value, str), 'invalid_spec', '%s must be a string' % path, path)
    need(len(value) <= max_chars, 'limit', '%s is longer than %d characters' % (path, max_chars), path)
    if choices is not None:
        need(value in choices, 'invalid_spec', '%s must be one of: %s' % (path, ', '.join(choices)), path)
    if pattern is not None:
        need(re.fullmatch(pattern, value) is not None, 'invalid_spec', '%s has an invalid format' % path, path)
    return value


def opt(obj, key, path, fn, *args, default=None, **kwargs):
    if key in obj and obj[key] is not None:
        return fn(obj[key], '%s.%s' % (path, key), *args, **kwargs)
    return default


_XML_INVALID = re.compile('[\x00-\x08\x0b\x0c\x0e-\x1f\ud800-\udfff\ufffe\uffff]')


def hex_color(value, path):
    text = as_str(value, path, 16).strip().lstrip('#')
    need(re.fullmatch(r'[0-9A-Fa-f]{6}', text) is not None, 'invalid_spec',
         '%s must be a hex color like "1F4E79"' % path, path)
    return text.upper()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def trunc(text, limit):
    return text if len(text) <= limit else text[:limit] + '…'


def ext_of(name):
    base = name.replace('\\', '/').rsplit('/', 1)[-1]
    return base.rsplit('.', 1)[-1].lower() if '.' in base else ''


def parse_pages(value, total, path, default=None):
    """1-based page selection ("1-3,5", [1, 2], "all") -> sorted unique 0-based indices."""
    if value is None:
        return list(range(total)) if default is None else default
    pages = []
    if isinstance(value, str):
        text = value.strip().lower()
        if text in ('all', '*', ''):
            return list(range(total))
        for part in text.split(','):
            part = part.strip()
            match = re.fullmatch(r'(\d+)?\s*-\s*(\d+)?|(\d+)', part)
            need(match is not None, 'invalid_spec', '%s: cannot read page range "%s"' % (path, part), path)
            if match.group(3):
                pages.append(int(match.group(3)))
            else:
                start = int(match.group(1) or 1)
                end = int(match.group(2) or total)
                need(start <= end and end - start <= 100_000, 'invalid_spec', '%s: bad range "%s"' % (path, part),
                     path)
                pages.extend(range(start, end + 1))
    else:
        for index, page in enumerate(as_list(value, path, 100_000)):
            pages.append(as_int(page, '%s[%d]' % (path, index), 1))
    for page in pages:
        need(1 <= page <= total, 'invalid_spec', '%s: page %d does not exist (the document has %d pages)' % (
            path, page, total), path)
    return sorted(set(page - 1 for page in pages))


# ---------------------------------------------------------------------------------------------
# Job: one request inside one stage.
# ---------------------------------------------------------------------------------------------

_RESERVED_NAME = re.compile(r'(?i)^(con|prn|aux|nul|com[0-9¹²³]|lpt[0-9¹²³]|conin\$|conout\$)(\..*)?$')


class Job:
    def __init__(self, stage, request):
        self.stage = stage
        self.diagnostics = []
        self.outputs = []
        self.previews = []
        self.qa = None
        self.output_total = 0
        self._data = {}
        self._warned = set()
        as_dict(request, 'request')
        need(request.get('format') == REQUEST_FORMAT, 'invalid_spec', 'request.format must be %s' % REQUEST_FORMAT,
             'format')
        self.tool = as_str(request.get('tool'), 'tool', 64, ('document_read', 'document_write', 'image_edit'))
        self.operation = as_str(request.get('operation'), 'operation', 64)
        phase = request.get('phase')
        self.phase = None if phase is None else as_str(phase, 'phase', 64)
        self.spec = request.get('spec') if request.get('spec') is not None else {}
        as_dict(self.spec, 'spec')
        self.limits = self._limits(request.get('limits'))
        self.options = self._options(request.get('options'))
        self.inputs = []
        for index, item in enumerate(as_list(request.get('inputs') or [], 'inputs', MAX_INPUTS)):
            path = 'inputs[%d]' % index
            as_dict(item, path, required=('name',))
            name = as_str(item['name'], path + '.name', 1024)
            self.resolve(name, ('in',), True)
            digest = opt(item, 'sha256', path, as_str, 64, pattern=r'[0-9a-fA-F]{64}')
            size = opt(item, 'bytes', path, as_int, 0)
            label = opt(item, 'path', path, as_str, 4096) or name
            self.inputs.append({'name': name, 'path': label, 'sha256': digest.lower() if digest else None,
                                'bytes': size})
        output = request.get('output')
        self.output = None
        if output is not None:
            as_dict(output, 'output', required=('name',))
            self.output = as_str(output['name'], 'output.name', 1024)

    def _limits(self, value):
        limits = dict(DEFAULT_LIMITS)
        if value is not None:
            as_dict(value, 'limits')
            for key, cap in LIMIT_CAPS.items():
                if value.get(key) is not None:
                    limits[key] = min(as_int(value[key], 'limits.' + key, 1), cap)
        return limits

    def _options(self, value):
        value = {} if value is None else as_dict(value, 'options')
        options = {
            'offset': opt(value, 'offset', 'options', as_int, 0, default=0),
            'limit': opt(value, 'limit', 'options', as_int, 1, 500,
                         default=100 if self.operation == 'ocr' else 20),
            'sheet': opt(value, 'sheet', 'options', as_str, 255),
            'pages': value.get('pages'),
            'topic': opt(value, 'topic', 'options', as_str, 32),
            'preview': opt(value, 'preview', 'options', as_bool, default=False),
            'render_source': opt(value, 'render_source', 'options', as_str, 1024),
            'converted': opt(value, 'converted', 'options', as_str, 1024),
            'compile_failed': opt(value, 'compile_failed', 'options', as_bool, default=False),
            'latex_available': opt(value, 'latex_available', 'options', as_bool, default=False),
            'log': opt(value, 'log', 'options', as_str, 1024),
            'pdf': opt(value, 'pdf', 'options', as_str, 1024),
            'stderr': opt(value, 'stderr', 'options', as_str, 1024),
            'languages': value.get('languages', ['deu', 'eng']),
            'dpi': opt(value, 'dpi', 'options', as_int, 100, 400, default=200),
            'rotation': opt(value, 'rotation', 'options', as_int, 0, 270, default=0),
        }
        pages = options['pages']
        need(pages is None or isinstance(pages, (str, list)), 'invalid_spec',
             'options.pages must be "1-3,5" or a list', 'options.pages')
        return options

    def resolve(self, name, areas, must_exist):
        need(isinstance(name, str) and name and '\0' not in name and len(name) <= 1024, 'invalid_spec',
             'invalid stage name', 'name')
        norm = name.replace('\\', '/')
        parts = norm.split('/')
        need(not norm.startswith('/') and ':' not in norm and all(p not in ('', '.', '..') for p in parts),
             'invalid_spec', 'stage name "%s" must be a relative path without ".."' % name, 'name')
        need(all(not _RESERVED_NAME.match(p) and not p.endswith((' ', '.')) and not _XML_INVALID.search(p)
                 for p in parts), 'invalid_spec', 'stage name "%s" is not portable' % name, 'name')
        need(parts[0] in areas, 'invalid_spec', 'stage name "%s" must be below %s/' % (name, ' or '.join(areas)),
             'name')
        real = os.path.realpath(os.path.join(self.stage, *parts))
        need(os.path.normcase(real).startswith(os.path.normcase(self.stage) + os.sep), 'invalid_spec',
             'stage name "%s" escapes the stage' % name, 'name')
        if must_exist:
            need(os.path.isfile(real), 'invalid_spec', 'stage file "%s" is missing' % name, 'name')
        return real

    def rel(self, path):
        return os.path.relpath(path, self.stage).replace('\\', '/')

    def input(self, index, path='inputs'):
        need(type(index) is int and 0 <= index < len(self.inputs), 'invalid_spec',
             '%s: input %r does not exist (%d inputs were given)' % (path, index, len(self.inputs)), path)
        return self.inputs[index]

    def data(self, index, path='inputs'):
        item = self.input(index, path)
        if index not in self._data:
            real = self.resolve(item['name'], ('in',), True)
            need(os.path.getsize(real) <= MAX_INPUT_BYTES, 'limit', '%s is larger than %d MiB' % (
                item['path'], MAX_INPUT_BYTES // MIB), 'inputs[%d]' % index)
            with open(real, 'rb') as handle:
                data = handle.read(MAX_INPUT_BYTES + 1)
            if item['sha256'] and sha256(data) != item['sha256']:
                fail('source_changed', '%s changed while it was staged; read it again' % item['path'],
                     'inputs[%d]' % index)
            if item['bytes'] is not None and item['bytes'] != len(data):
                fail('source_changed', '%s has an unexpected size; read it again' % item['path'],
                     'inputs[%d]' % index)
            self._data[index] = data
        return self._data[index]

    def kind(self, index, path='inputs'):
        item = self.input(index, path)
        extension = ext_of(item['name'])
        if extension in EXTENSION_KINDS:
            return EXTENSION_KINDS[extension]
        if extension in TEXT_EXTENSIONS:
            return 'text'
        if extension in LEGACY_OFFICE:
            fail('unsupported', '%s is a legacy or OpenDocument file; convert it to docx/xlsx/pptx/pdf first'
                 % item['path'], path)
        fail('unsupported', '%s has an unsupported file type ".%s"' % (item['path'], extension), path)

    def single_input(self):
        need(len(self.inputs) == 1, 'invalid_spec', '%s %s needs exactly one input' % (self.tool, self.operation),
             'inputs')
        return 0

    def warn(self, message, key=None, severity='warning', file=None, line=None):
        key = key or message
        if key in self._warned or len(self.diagnostics) >= MAX_DIAGNOSTICS:
            return
        self._warned.add(key)
        entry = {'severity': severity, 'message': trunc(message, 400)}
        if file:
            entry['file'] = file
        if line:
            entry['line'] = line
        self.diagnostics.append(entry)

    def text(self, value, path, max_chars=MAX_STRING_CHARS):
        """Validate a model string and remove characters that XML/Office cannot store."""
        value = as_str(value, path, max_chars)
        value = value.replace('\r\n', '\n').replace('\r', '\n')
        if _XML_INVALID.search(value):
            value = _XML_INVALID.sub('', value)
            self.warn('Removed control characters that documents cannot store (first at %s)' % path, 'xml-invalid')
        return value

    def output_path(self, name=None, extensions=None, directory=False):
        name = name or self.output
        need(name, 'invalid_spec', 'this operation needs output.name', 'output')
        if extensions is not None and not directory:
            need(ext_of(name) in extensions, 'invalid_spec',
                 'output.name must end with .%s' % ' or .'.join(extensions), 'output.name')
        real = self.resolve(name, ('out',), False)
        need(not os.path.lexists(real), 'invalid_spec', 'output "%s" already exists; choose a new name' % name,
             'output.name')
        return real

    def publish(self, name, data, media_type=None):
        real = self.output_path(name)
        need(len(self.outputs) < self.limits['max_outputs'], 'limit', 'too many outputs (limit %d)'
             % self.limits['max_outputs'], 'output')
        need(self.output_total + len(data) <= self.limits['max_output_bytes'], 'limit',
             'outputs exceed %d MiB' % (self.limits['max_output_bytes'] // MIB), 'output')
        os.makedirs(os.path.dirname(real), exist_ok=True)
        with open(real, 'xb') as handle:
            handle.write(data)
        self.output_total += len(data)
        self.outputs.append({'name': self.rel(real), 'sha256': sha256(data), 'bytes': len(data),
                             'media_type': media_type or MEDIA_TYPES.get(ext_of(name), 'application/octet-stream')})
        return real

    def preview_path(self, label):
        directory = os.path.join(self.stage, 'work', 'previews')
        os.makedirs(directory, exist_ok=True)
        for attempt in range(1000):
            path = os.path.join(directory, '%s.png' % label if attempt == 0 else '%s-%d.png' % (label, attempt))
            if not os.path.lexists(path):
                return path
        fail('limit', 'too many previews')

    def add_preview(self, image, label, page):
        """Save a PIL image as PNG within preview_max_edge / preview_max_bytes."""
        if len(self.previews) >= self.limits['max_preview_images']:
            return False
        Image = pil(self)
        edge = self.limits['preview_max_edge']
        if max(image.size) > edge:
            image = image.copy()
            image.thumbnail((edge, edge), Image.Resampling.LANCZOS)
        if image.mode not in ('RGB', 'RGBA', 'L', 'LA', 'P'):
            image = image.convert('RGBA' if 'A' in image.getbands() else 'RGB')
        for _ in range(8):
            buffer = io.BytesIO()
            image.save(buffer, 'PNG', optimize=True)
            data = buffer.getvalue()
            if len(data) <= self.limits['preview_max_bytes']:
                break
            image = image.resize((max(1, int(image.width * 0.75)), max(1, int(image.height * 0.75))),
                                 Image.Resampling.LANCZOS)
        else:
            self.warn('A preview was skipped because it stayed above preview_max_bytes', 'preview-size')
            return False
        path = self.preview_path(label)
        with open(path, 'xb') as handle:
            handle.write(data)
        self.previews.append({'name': self.rel(path), 'sha256': sha256(data), 'width': image.width,
                              'height': image.height, 'page': page})
        return True

    def capture_warning(self, message, category, filename, lineno, file=None, line=None):
        self.warn('%s: %s' % (category.__name__, str(message)[:300]), 'pywarn:%s' % str(message)[:120], 'info')


# ---------------------------------------------------------------------------------------------
# Result sizing and pagination.
# ---------------------------------------------------------------------------------------------

def json_size(value):
    # Receipts are ASCII JSON (ensure_ascii=True), so size is measured the same way.
    return len(json.dumps(value, separators=(',', ':')))


def fit_text(job, base, text, field='text'):
    """Return base + a chunk of text starting at options.offset that fits max_result_bytes."""
    offset = job.options['offset']
    total = len(text)
    need(offset <= total, 'invalid_spec', 'options.offset %d is beyond the end (%d characters)' % (offset, total),
         'options.offset')
    budget = job.limits['max_result_bytes'] - 64

    def build(count):
        end = offset + count
        result = dict(base)
        result.update({field: text[offset:end], 'offset': offset, 'next_offset': end if end < total else None,
                       'total_chars': total})
        result['truncated'] = bool(base.get('truncated')) or end < total
        return result

    lo, hi = 0, min(total - offset, budget)
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if json_size(build(mid)) <= budget:
            lo = mid
        else:
            hi = mid - 1
    count = lo
    if offset + count < total and count > 200:
        cut = text.rfind('\n', offset + int(count * 0.85), offset + count)
        if cut > offset:
            count = cut + 1 - offset
    need(count > 0 or total == offset, 'limit', 'max_result_bytes is too small for any text', 'limits')
    return build(count)


def shrink_item(item, budget):
    """Truncate the longest strings of one item until it fits."""
    item = copy.deepcopy(item)
    for _ in range(60):
        if json_size(item) <= budget:
            return item
        longest = None

        def visit(node, setter):
            nonlocal longest
            if isinstance(node, str):
                if setter is not None and (longest is None or len(node) > len(longest[0])):
                    longest = (node, setter)
            elif isinstance(node, dict):
                for key in list(node):
                    visit(node[key], lambda v, n=node, k=key: n.__setitem__(k, v))
            elif isinstance(node, list):
                for index in range(len(node)):
                    visit(node[index], lambda v, n=node, i=index: n.__setitem__(i, v))
        visit(item, None)
        if longest is None or len(longest[0]) < 8:
            break
        longest[1](longest[0][:len(longest[0]) // 2] + '…')
        item['truncated'] = True
    return item


def fit_items(job, base, items, key='items'):
    offset, limit = job.options['offset'], job.options['limit']
    need(offset <= len(items), 'invalid_spec', 'options.offset %d is beyond the end (%d items)' % (
        offset, len(items)), 'options.offset')
    selected = items[offset:offset + limit]
    budget = job.limits['max_result_bytes'] - 64

    def build(chosen):
        result = dict(base)
        end = offset + len(chosen)
        result.update({key: chosen, 'offset': offset, 'next_offset': end if end < len(items) else None,
                       'total': len(items)})
        return result
    while len(selected) > 1 and json_size(build(selected)) > budget:
        selected = selected[:max(1, len(selected) * 3 // 4)]
    result = build(selected)
    if json_size(result) > budget and selected:
        overhead = json_size(build([]))
        selected = [shrink_item(selected[0], max(256, budget - overhead - 16))]
        result = build(selected)
    need(json_size(result) <= job.limits['max_result_bytes'], 'limit', 'max_result_bytes is too small for one item',
         'limits')
    return result


# ---------------------------------------------------------------------------------------------
# Container safety: zip limits, DOCTYPE rejection, Pillow limits, encrypted PDFs.
# ---------------------------------------------------------------------------------------------

OOXML_MAIN = {
    'docx': ('application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml',
             ('application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml',
              'application/vnd.ms-word.document.macroEnabled.main+xml',
              'application/vnd.ms-word.template.macroEnabledTemplate.main+xml')),
    'xlsx': ('application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml',
             ('application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml',
              'application/vnd.ms-excel.sheet.macroEnabled.main+xml',
              'application/vnd.ms-excel.template.macroEnabled.main+xml')),
    'pptx': ('application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml',
             ('application/vnd.openxmlformats-officedocument.presentationml.template.main+xml',
              'application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml',
              'application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml',
              'application/vnd.ms-powerpoint.template.macroEnabled.main+xml',
              'application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml'))}
_DTD = re.compile(r'<!\s*(DOCTYPE|ENTITY)', re.I)


def xml_has_dtd(raw):
    if raw[:4] in (b'\x00\x00\xfe\xff', b'\xff\xfe\x00\x00'):
        return True
    if raw[:2] in (b'\xff\xfe', b'\xfe\xff') or (len(raw) > 1 and (raw[0] == 0 or raw[1] == 0)):
        try:
            text = raw.decode('utf-16')
        except UnicodeDecodeError:
            return True
    else:
        text = raw.decode('latin-1')
    return _DTD.search(text) is not None


def check_ooxml(job, data, kind, label):
    """Bound the package and reject DTDs before any library parses it. Returns normalized bytes."""
    if data[:8] == b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1':
        fail('encrypted', '%s is password protected or a legacy binary Office file' % label)
    need(data[:2] == b'PK', 'corrupt_input', '%s is not a valid %s file (not a zip package)' % (label, kind))
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
        infos = archive.infolist()
    except (zipfile.BadZipFile, zipfile.LargeZipFile, ValueError, OSError, EOFError):
        fail('corrupt_input', '%s is not a readable %s package' % (label, kind))
    with archive:
        need(len(infos) <= ZIP_MAX_ENTRIES, 'limit', '%s has more than %d package entries' % (
            label, ZIP_MAX_ENTRIES))
        need(sum(info.file_size for info in infos) <= ZIP_MAX_EXPANDED, 'limit',
             '%s expands to more than %d MiB' % (label, ZIP_MAX_EXPANDED // MIB))
        names = set()
        for info in infos:
            need(not info.flag_bits & 0x1, 'encrypted', '%s contains encrypted package entries' % label)
            need(info.filename not in names, 'corrupt_input',
                 '%s contains a duplicate package entry: %s' % (label, info.filename))
            names.add(info.filename)
            if info.filename.lower().endswith(('.xml', '.rels', '.vml')):
                try:
                    raw = archive.read(info)
                except (zipfile.BadZipFile, OSError, EOFError, NotImplementedError, RuntimeError, ValueError):
                    fail('corrupt_input', '%s has a damaged package entry %s' % (label, info.filename))
                if xml_has_dtd(raw):
                    fail('corrupt_input', '%s: %s contains a DOCTYPE or ENTITY declaration, which is refused'
                         % (label, info.filename))
        need('[Content_Types].xml' in names, 'corrupt_input', '%s has no [Content_Types].xml' % label)
        types = archive.read('[Content_Types].xml').decode('utf-8', 'replace')
        main, alternates = OOXML_MAIN[kind]
        if main in types:
            return data
        for alternate in alternates:
            if alternate in types:
                return rewrite_zip_entry(data, '[Content_Types].xml',
                                         types.replace(alternate, main).encode('utf-8'))
    fail('corrupt_input', '%s does not contain a %s main part' % (label, kind))


def rewrite_zip_entry(data, name, content):
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as target:
        for info in source.infolist():
            target.writestr(info, content if info.filename == name else source.read(info))
    return out.getvalue()


def safe_xml_parser():
    from lxml import etree
    return etree.XMLParser(resolve_entities=False, no_network=True, huge_tree=False, load_dtd=False,
                           dtd_validation=False)


def validate_package(job, data, kind):
    """Bug fix: generated OOXML must be well-formed and reopen in its library."""
    from lxml import etree
    parser = safe_xml_parser()
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        names = archive.namelist()
        need('[Content_Types].xml' in names, 'engine', 'generated %s lacks [Content_Types].xml' % kind)
        for name in names:
            if name.lower().endswith(('.xml', '.rels')):
                try:
                    etree.fromstring(archive.read(name), parser)
                except etree.XMLSyntaxError as error:
                    fail('engine', 'generated %s has malformed XML in %s: %s' % (kind, name, str(error)[:160]))
    try:
        if kind == 'docx':
            import docx
            docx.Document(io.BytesIO(data))
        elif kind == 'xlsx':
            import openpyxl
            openpyxl.load_workbook(io.BytesIO(data)).close()
        elif kind == 'pptx':
            import pptx
            pptx.Presentation(io.BytesIO(data))
    except EngineError:
        raise
    except Exception as error:
        fail('engine', 'generated %s does not reopen: %s' % (kind, str(error)[:200]))


_PIL = None


def pil(job):
    global _PIL
    if _PIL is None:
        from PIL import Image
        Image.MAX_IMAGE_PIXELS = job.limits['max_image_pixels']
        warnings.simplefilter('error', Image.DecompressionBombWarning)
        _PIL = Image
    return _PIL


def open_image(job, data, label):
    Image = pil(job)
    from PIL import UnidentifiedImageError
    try:
        image = Image.open(io.BytesIO(data), formats=('PNG', 'JPEG', 'GIF', 'BMP', 'TIFF', 'WEBP', 'ICO', 'PPM'))
    except (Image.DecompressionBombError, Image.DecompressionBombWarning):
        fail('limit', '%s is larger than the image pixel limit (%d)' % (label, job.limits['max_image_pixels']))
    except (UnidentifiedImageError, OSError, SyntaxError, ValueError, EOFError):
        fail('corrupt_input', '%s is not a readable image' % label)
    need(image.width * image.height <= job.limits['max_image_pixels'], 'limit',
         '%s has %d pixels; the limit is %d' % (label, image.width * image.height, job.limits['max_image_pixels']))
    return image


def load_image(job, data, label):
    image = open_image(job, data, label)
    Image = pil(job)
    try:
        image.load()
    except (Image.DecompressionBombError, Image.DecompressionBombWarning):
        fail('limit', '%s is larger than the image pixel limit' % label)
    except (OSError, SyntaxError, ValueError, EOFError):
        fail('corrupt_input', '%s is truncated or damaged' % label)
    return image


def prepare_image(job, index, path, allow_pdf=False):
    """Re-encode an input image (EXIF stripped, orientation applied). Returns (bytes, ext, w, h)."""
    item = job.input(index, path)
    kind = job.kind(index, path)
    data = job.data(index, path)
    if kind == 'pdf' and allow_pdf:
        reader = open_pdf(job, data, item['path'])
        need(not reader.is_encrypted, 'encrypted', '%s is encrypted' % item['path'], path)
        return data, 'pdf', None, None
    need(kind == 'image', 'invalid_spec', '%s: input %d (%s) is not an image' % (path, index, item['path']), path)
    from PIL import ImageOps
    image = load_image(job, data, item['path'])
    source_format = image.format
    if getattr(image, 'n_frames', 1) > 1:
        job.warn('%s is animated; only the first frame is used' % item['path'])
    image = ImageOps.exif_transpose(image)
    buffer = io.BytesIO()
    if source_format == 'JPEG' and image.mode in ('RGB', 'L', 'CMYK'):
        image.save(buffer, 'JPEG', quality=92, optimize=True)
        extension = 'jpg'
    else:
        if image.mode not in ('1', 'L', 'LA', 'RGB', 'RGBA'):
            image = image.convert('RGBA' if 'A' in image.getbands() or 'transparency' in image.info else 'RGB')
        image.save(buffer, 'PNG', optimize=True)
        extension = 'png'
    return buffer.getvalue(), extension, image.width, image.height


def open_pdf(job, data, label, allow_owner_only=True):
    from pypdf import PdfReader
    need(data[:1024].find(b'%PDF') >= 0, 'corrupt_input', '%s is not a PDF file' % label)
    try:
        reader = PdfReader(io.BytesIO(data), strict=False)
        if reader.is_encrypted:
            opened = False
            if allow_owner_only:
                try:
                    opened = bool(reader.decrypt(''))
                except Exception:
                    opened = False
            if not opened:
                fail('encrypted', '%s is password protected; remove the password first' % label)
        count = len(reader.pages)
    except EngineError:
        raise
    except Exception:
        fail('corrupt_input', '%s is a damaged PDF' % label)
    need(count > 0, 'corrupt_input', '%s has no pages' % label)
    return reader


def pdf_for_write(job, index, path='inputs'):
    item = job.input(index, path)
    need(job.kind(index, path) == 'pdf', 'invalid_spec', '%s: input %d (%s) is not a PDF' % (
        path, index, item['path']), path)
    reader = open_pdf(job, job.data(index, path), item['path'])
    need(not reader.is_encrypted, 'encrypted', '%s is encrypted; PDF operations refuse encrypted files'
         % item['path'], path)
    need(len(reader.pages) <= MAX_PDF_PAGES_WRITE, 'limit', '%s has more than %d pages' % (
        item['path'], MAX_PDF_PAGES_WRITE), path)
    return reader


def render_pdf_previews(job, data, label, pages=None):
    """Render pages (0-based indices) with pypdfium2 into work/previews/."""
    import pypdfium2 as pdfium
    try:
        document = pdfium.PdfDocument(data)
    except pdfium.PdfiumError as error:
        if 'password' in str(error).lower():
            fail('encrypted', '%s is password protected' % label)
        fail('corrupt_input', '%s cannot be rendered' % label)
    try:
        # PDFium renders widget appearances only after the form environment exists.
        # Initialize before creating any page handles, including for filled-form previews.
        document.init_forms()
        total = len(document)
        if pages is None:
            pages = list(range(min(total, job.limits['max_preview_images'])))
        need(len(pages) <= job.limits['max_pages_render'], 'limit', 'too many pages to render (limit %d)'
             % job.limits['max_pages_render'], 'options.pages')
        if len(pages) > job.limits['max_preview_images']:
            job.warn('Only the first %d selected pages were rendered (max_preview_images)'
                     % job.limits['max_preview_images'], 'preview-count')
            pages = pages[:job.limits['max_preview_images']]
        edge = job.limits['preview_max_edge']
        for index in pages:
            need(0 <= index < total, 'invalid_spec', 'page %d does not exist' % (index + 1), 'options.pages')
            page = document[index]
            try:
                width, height = page.get_size()
                scale = min(edge / max(width, height, 1.0), 4.0)
                need(width * height * scale * scale <= job.limits['max_image_pixels'], 'limit',
                     'page %d is too large to render' % (index + 1), 'options.pages')
                job.add_preview(page.render(scale=scale).to_pil(), 'page-%d' % (index + 1), index + 1)
            finally:
                page.close()
        return total
    finally:
        document.close()


# ---------------------------------------------------------------------------------------------
# Markdown (subset) -> block vocabulary shared by DOCX and LaTeX.
# ---------------------------------------------------------------------------------------------

URL_PATTERN = re.compile(r'(?i)^(https?://[^\s<>"]+|mailto:[^\s<>"]+)$')
_MD_INLINE = re.compile(
    r'\\\((?P<math>.+?)\\\)'
    r'|(?P<escape>\\[\\`*_{}\[\]()#+\-.!~$|<>])'
    r'|(?P<ticks>`+)(?P<code>.+?)(?P=ticks)'
    r'|\*\*(?P<bold>.+?)\*\*|__(?P<bold2>.+?)__'
    r'|~~(?P<strike>.+?)~~'
    r'|\*(?P<italic>[^\s*](?:.*?[^\s*])?)\*'
    r'|(?<![\w])_(?P<italic2>[^\s_](?:.*?[^\s_])?)_(?![\w])'
    r'|\[(?P<label>[^\]]+)\]\((?P<url>[^)\s]+)\)', re.S)


def md_inline(text, fmt=None):
    fmt = dict(fmt or {})
    runs = []
    position = 0

    def plain(chunk, style=None):
        style = fmt if style is None else style
        if not chunk:
            return
        if runs and set(runs[-1]) - {'text'} == set(style) and all(runs[-1].get(k) == v for k, v in style.items()):
            runs[-1]['text'] += chunk
        else:
            runs.append(dict(style, text=chunk))
    for match in _MD_INLINE.finditer(text):
        plain(text[position:match.start()])
        position = match.end()
        if match.group('escape'):
            plain(match.group('escape')[1])
        elif match.group('code') is not None:
            runs.append(dict(fmt, text=match.group('code'), code=True))
        elif match.group('math') is not None:
            runs.append(dict(fmt, text='', math=match.group('math')))
        elif match.group('bold') is not None or match.group('bold2') is not None:
            runs.extend(md_inline(match.group('bold') or match.group('bold2'), dict(fmt, bold=True)))
        elif match.group('strike') is not None:
            runs.extend(md_inline(match.group('strike'), dict(fmt, strike=True)))
        elif match.group('italic') is not None or match.group('italic2') is not None:
            runs.extend(md_inline(match.group('italic') or match.group('italic2'), dict(fmt, italic=True)))
        elif match.group('label') is not None:
            url = match.group('url')
            style = dict(fmt, link=url) if URL_PATTERN.match(url) else fmt
            runs.extend(md_inline(match.group('label'), style))
    plain(text[position:])
    return runs or [dict(fmt, text='')]


_MD_LIST = re.compile(r'^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$')
_MD_FENCE = re.compile(r'^\s{0,3}(```+|~~~+)\s*([\w+#.-]*)\s*$')
_MD_HEADING = re.compile(r'^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$')
_MD_RULE = re.compile(r'^\s{0,3}([-*_])(\s*\1){2,}\s*$')
_MD_IMAGE = re.compile(r'^\s{0,3}!\[(.*?)\]\(\s*input:(\d+)\s*\)\s*$')
_MD_TABLE_SEP = re.compile(r'^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$')
_MD_BREAKS = {'\\newpage', '\\pagebreak', '\\clearpage', '<!-- pagebreak -->', '<!--pagebreak-->'}


def md_split_row(line):
    line = line.strip()
    if line.startswith('|'):
        line = line[1:]
    if line.endswith('|') and not line.endswith('\\|'):
        line = line[:-1]
    cells, current, escaped = [], '', False
    for char in line:
        if escaped:
            current += char if char == '|' else '\\' + char
            escaped = False
        elif char == '\\':
            escaped = True
        elif char == '|':
            cells.append(current.strip())
            current = ''
        else:
            current += char
    cells.append(current.strip())
    return cells


def md_is_block_start(line, next_line):
    return bool(_MD_FENCE.match(line) or _MD_HEADING.match(line) or _MD_RULE.match(line) or _MD_LIST.match(line)
                or _MD_IMAGE.match(line) or line.lstrip().startswith('>') or line.strip() in _MD_BREAKS
                or line.strip().startswith('$$')
                or ('|' in line and next_line is not None and _MD_TABLE_SEP.match(next_line)))


def md_blocks(text):
    lines = text.replace('\r\n', '\n').replace('\r', '\n').split('\n')
    blocks = []
    i = 0
    while i < len(lines):
        line = lines[i]
        next_line = lines[i + 1] if i + 1 < len(lines) else None
        stripped = line.strip()
        if not stripped:
            i += 1
            continue
        fence = _MD_FENCE.match(line)
        if fence:
            marker, body = fence.group(1), []
            i += 1
            while i < len(lines) and not lines[i].strip().startswith(marker[:3]):
                body.append(lines[i])
                i += 1
            i += 1
            block = {'type': 'code', 'text': '\n'.join(body)}
            if fence.group(2):
                block['language'] = fence.group(2)
            blocks.append(block)
            continue
        if stripped.startswith('$$'):
            if len(stripped) > 4 and stripped.endswith('$$'):
                blocks.append({'type': 'math', 'latex': stripped[2:-2].strip()})
                i += 1
                continue
            body = [stripped[2:]]
            i += 1
            while i < len(lines) and '$$' not in lines[i]:
                body.append(lines[i])
                i += 1
            if i < len(lines):
                body.append(lines[i].split('$$', 1)[0])
            i += 1
            blocks.append({'type': 'math', 'latex': '\n'.join(body).strip()})
            continue
        heading = _MD_HEADING.match(line)
        if heading:
            blocks.append({'type': 'heading', 'level': len(heading.group(1)), 'runs': md_inline(heading.group(2))})
            i += 1
            continue
        if _MD_RULE.match(line):
            blocks.append({'type': 'horizontal_rule'})
            i += 1
            continue
        if stripped in _MD_BREAKS:
            blocks.append({'type': 'page_break'})
            i += 1
            continue
        image = _MD_IMAGE.match(line)
        if image:
            block = {'type': 'image', 'input': int(image.group(2))}
            if image.group(1):
                block['caption'] = image.group(1)
            blocks.append(block)
            i += 1
            continue
        if line.lstrip().startswith('>'):
            body = []
            while i < len(lines) and lines[i].lstrip().startswith('>'):
                body.append(re.sub(r'^\s*>\s?', '', lines[i]))
                i += 1
            blocks.append({'type': 'quote', 'runs': md_inline(' '.join(p.strip() for p in body if p.strip()))})
            continue
        if '|' in line and next_line is not None and _MD_TABLE_SEP.match(next_line):
            header = md_split_row(line)
            aligns = []
            for cell in md_split_row(next_line):
                cell = cell.strip()
                aligns.append('center' if cell.startswith(':') and cell.endswith(':') else
                              'right' if cell.endswith(':') else 'left')
            rows = [[{'runs': md_inline(cell)} for cell in header]]
            i += 2
            while i < len(lines) and '|' in lines[i] and lines[i].strip():
                rows.append([{'runs': md_inline(cell)} for cell in md_split_row(lines[i])])
                i += 1
            blocks.append({'type': 'table', 'rows': rows, 'header': True, 'align': aligns[:len(header)]})
            continue
        if _MD_LIST.match(line):
            block, i = md_list(lines, i)
            blocks.append(block)
            continue
        body = []
        while i < len(lines) and lines[i].strip():
            if body and md_is_block_start(lines[i], lines[i + 1] if i + 1 < len(lines) else None):
                break
            current = lines[i]
            hard = current.endswith('  ') or (current.rstrip().endswith('\\')
                                              and not current.rstrip().endswith('\\\\'))
            current = current.strip()
            if hard and current.endswith('\\'):
                current = current[:-1]
            body.append(current + ('\n' if hard else ' '))
            i += 1
        blocks.append({'type': 'paragraph', 'runs': md_inline(''.join(body).strip())})
    return blocks


def md_list(lines, i):
    root = None
    stack = []  # (indent, list_block)
    while i < len(lines):
        match = _MD_LIST.match(lines[i])
        if not match:
            if not lines[i].strip():
                j = i + 1
                while j < len(lines) and not lines[j].strip():
                    j += 1
                if j < len(lines) and _MD_LIST.match(lines[j]):
                    i = j
                    continue
                break
            indent = len(lines[i]) - len(lines[i].lstrip())
            if stack and indent > stack[-1][0]:
                item = stack[-1][1]['items'][-1]
                item['runs'] = item['runs'] + md_inline(' ' + lines[i].strip())
                i += 1
                continue
            break
        indent = len(match.group(1).expandtabs(4))
        marker, text = match.group(2), match.group(3)
        ordered = marker[0].isdigit()
        while stack and indent < stack[-1][0]:
            stack.pop()
        if not stack or indent > stack[-1][0]:
            block = {'type': 'list', 'ordered': ordered, 'items': []}
            if ordered and int(marker[:-1]) != 1:
                block['start'] = int(marker[:-1])
            if stack and stack[-1][1]['items']:
                parent = stack[-1][1]['items'][-1]
                parent['items'] = block['items']
                parent['ordered'] = ordered
            elif root is None:
                root = block
            stack.append((indent, block))
        task = re.match(r'^\[( |x|X)\]\s+(.*)$', text)
        if task:
            text = ('☒ ' if task.group(1).lower() == 'x' else '☐ ') + task.group(2)
        stack[-1][1]['items'].append({'runs': md_inline(text)})
        i += 1
    return root, i


BLOCK_TYPES = ('heading', 'paragraph', 'list', 'table', 'image', 'page_break', 'code', 'quote', 'horizontal_rule',
               'toc', 'math', 'markdown')
RUN_KEYS = {'text', 'bold', 'italic', 'underline', 'strike', 'code', 'color', 'highlight', 'size', 'font',
            'superscript', 'subscript', 'link', 'math'}


# ---------------------------------------------------------------------------------------------
# Shared text replacement across runs (placeholders, find/replace) for DOCX and PPTX.
# ---------------------------------------------------------------------------------------------

PLACEHOLDER = re.compile(r'\{\{\s*([A-Za-z0-9_.\-]{1,100})\s*\}\}')
XML_SPACE = '{http://www.w3.org/XML/1998/namespace}space'


def replace_in_nodes(nodes, pattern, replace, preserve_tag=None):
    """Replace matches across consecutive text nodes (runs) while keeping each run's formatting."""
    texts = [node.text or '' for node in nodes]
    full = ''.join(texts)
    if not full:
        return 0
    starts, position = [], 0
    for text in texts:
        starts.append(position)
        position += len(text)

    def locate(offset):
        for index, text in enumerate(texts):
            if text and starts[index] <= offset < starts[index] + len(text):
                return index
        return len(texts) - 1
    count = 0
    for match in reversed(list(pattern.finditer(full))):
        if match.end() == match.start():
            continue
        replacement = replace(match)
        if replacement is None:
            continue
        first, last = locate(match.start()), locate(match.end() - 1)
        for index in range(first, last + 1):
            text = nodes[index].text or ''
            local_start, local_end = match.start() - starts[index], match.end() - starts[index]
            if index == first and index == last:
                text = text[:local_start] + replacement + text[local_end:]
            elif index == first:
                text = text[:local_start] + replacement
            elif index == last:
                text = text[local_end:]
            else:
                text = ''
            nodes[index].text = text
            if preserve_tag is not None and nodes[index].tag == preserve_tag:
                nodes[index].set(XML_SPACE, 'preserve')
        count += 1
    return count


def values_of(job, values, path):
    as_dict(values, path)
    need(len(values) <= 5000, 'limit', '%s has too many entries' % path, path)
    result = {}
    for key, value in values.items():
        need(re.fullmatch(r'[A-Za-z0-9_.\-]{1,100}', key) is not None, 'invalid_spec',
             '%s: placeholder names use letters, digits, _ . -' % path, '%s.%s' % (path, key))
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            value = str(value)
        result[key] = job.text(value, '%s.%s' % (path, key), 20_000).replace('\n', ' ')
    return result


def core_properties(props):
    result = {}
    for key in ('title', 'author', 'creator', 'subject', 'keywords', 'category', 'comments', 'description',
                'last_modified_by', 'lastModifiedBy', 'revision', 'created', 'modified'):
        value = getattr(props, key, None)
        if value in (None, ''):
            continue
        result[key] = value.isoformat() if hasattr(value, 'isoformat') else value
    return result


def set_core_properties(job, props, spec, path, fresh):
    values = spec.get('properties') or {}
    as_dict(values, path, {'title', 'author', 'subject', 'keywords', 'description', 'category'})
    now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None, microsecond=0)
    if fresh:
        for key in ('title', 'subject', 'keywords', 'category', 'comments'):
            setattr(props, key, '')
        props.author = ''
        props.last_modified_by = ''
        props.created = now
        props.revision = 1
    props.modified = now
    for key, attr in (('title', 'title'), ('author', 'author'), ('subject', 'subject'), ('keywords', 'keywords'),
                      ('description', 'comments'), ('category', 'category')):
        if values.get(key) is not None:
            setattr(props, attr, job.text(values[key], '%s.%s' % (path, key), 1000))
    if values.get('author'):
        props.last_modified_by = props.author


# ---------------------------------------------------------------------------------------------
# DOCX
# ---------------------------------------------------------------------------------------------

W_NS = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
MC_FALLBACK = '{http://schemas.openxmlformats.org/markup-compatibility/2006}Fallback'


def wq(tag):
    return '{%s}%s' % (W_NS, tag)


W_P, W_TBL, W_TR, W_TC, W_R, W_T = wq('p'), wq('tbl'), wq('tr'), wq('tc'), wq('r'), wq('t')
_DOCX_SKIP = {wq(name) for name in ('pPr', 'rPr', 'sectPr', 'tblPr', 'trPr', 'tcPr', 'tblGrid', 'tblPrEx', 'delText',
                                     'instrText', 'delInstrText', 'del', 'moveFrom', 'fldData', 'footnoteReference',
                                     'endnoteReference', 'commentReference', 'annotationRef')}
_DOCX_TEXTLIKE = {wq(name) for name in ('t', 'tab', 'ptab', 'br', 'cr', 'noBreakHyphen', 'softHyphen', 'sym')}
PPR_AFTER_PBDR = ['w:shd', 'w:tabs', 'w:suppressAutoHyphens', 'w:kinsoku', 'w:wordWrap', 'w:overflowPunct',
                  'w:topLinePunct', 'w:autoSpaceDE', 'w:autoSpaceDN', 'w:bidi', 'w:adjustRightInd', 'w:snapToGrid',
                  'w:spacing', 'w:ind', 'w:contextualSpacing', 'w:mirrorIndents', 'w:suppressOverlap', 'w:jc',
                  'w:textDirection', 'w:textAlignment', 'w:textboxTightWrap', 'w:outlineLvl', 'w:divId',
                  'w:cnfStyle', 'w:rPr', 'w:sectPr', 'w:pPrChange']
TCPR_AFTER_SHD = ['w:noWrap', 'w:tcMar', 'w:textDirection', 'w:tcFitText', 'w:vAlign', 'w:hideMark', 'w:headers',
                  'w:cellIns', 'w:cellDel', 'w:cellMerge', 'w:tcPrChange']
PAGE_SIZES_MM = {'a3': (297, 420), 'a4': (210, 297), 'a5': (148, 210), 'letter': (215.9, 279.4),
                 'legal': (215.9, 355.6)}
HIGHLIGHTS = {'yellow': 'YELLOW', 'green': 'BRIGHT_GREEN', 'cyan': 'TURQUOISE', 'magenta': 'PINK', 'blue': 'BLUE',
              'red': 'RED', 'dark_blue': 'DARK_BLUE', 'dark_red': 'DARK_RED', 'dark_yellow': 'DARK_YELLOW',
              'teal': 'TEAL', 'violet': 'VIOLET', 'gray': 'GRAY_25', 'dark_gray': 'GRAY_50', 'black': 'BLACK',
              'white': 'WHITE'}


def docx_collect(element, out):
    """Text of a WordprocessingML subtree; w:tab -> tab, w:br/w:cr -> newline (QA bug: tabs/breaks were lost)."""
    for child in element:
        tag = child.tag
        if not isinstance(tag, str) or tag in _DOCX_SKIP or tag == MC_FALLBACK:
            continue
        if tag == W_T:
            out.append(child.text or '')
        elif tag in (wq('tab'), wq('ptab')):
            out.append('\t')
        elif tag in (wq('br'), wq('cr')):
            out.append('\n')
        elif tag == wq('noBreakHyphen'):
            out.append('-')
        elif tag in (wq('softHyphen'), wq('sym')):
            continue
        elif tag == W_P:
            if out and not out[-1].endswith('\n'):
                out.append('\n')
            docx_collect(child, out)
        elif tag == W_TBL:
            if out and not out[-1].endswith('\n'):
                out.append('\n')
            out.append(docx_table_text(child))
        else:
            docx_collect(child, out)


def docx_text_of(element):
    out = []
    docx_collect(element, out)
    return ''.join(out)


def docx_rows(table):
    return [list(row.iterchildren(W_TC)) for row in table.iterchildren(W_TR)]


def docx_cell_text(cell):
    parts = []
    for child in cell:
        if child.tag == W_P:
            parts.append(docx_text_of(child))
        elif child.tag == W_TBL:
            parts.append(docx_table_text(child))
    return ' '.join(part.replace('\n', ' ').replace('\t', ' ') for part in parts).strip()


def docx_table_text(table):
    return '\n'.join('\t'.join(docx_cell_text(cell) for cell in row) for row in docx_rows(table))


def docx_blocks(container):
    for child in container:
        tag = child.tag
        if tag in (W_P, W_TBL):
            yield child
        elif tag == wq('sdt'):
            content = child.find(wq('sdtContent'))
            if content is not None:
                yield from docx_blocks(content)
        elif tag in (wq('customXml'), wq('smartTag')):
            yield from docx_blocks(child)


def docx_open(job, index, path='inputs'):
    import docx
    item = job.input(index, path)
    data = check_ooxml(job, job.data(index, path), 'docx', item['path'])
    try:
        return docx.Document(io.BytesIO(data))
    except EngineError:
        raise
    except Exception as error:
        fail('corrupt_input', '%s cannot be opened as a Word document (%s)' % (item['path'], str(error)[:120]), path)


def docx_style_names(doc):
    names = {}
    for style in doc.styles:
        try:
            names[style.style_id] = style.name
        except Exception:
            continue
    return names


def docx_paragraph_style(paragraph, names):
    ppr = paragraph.find(wq('pPr'))
    if ppr is not None:
        style = ppr.find(wq('pStyle'))
        if style is not None:
            value = style.get(wq('val'))
            return names.get(value, value)
    return 'Normal'


def docx_text(job, index):
    doc = docx_open(job, index)
    return '\n'.join(docx_text_of(block) if block.tag == W_P else docx_table_text(block)
                     for block in docx_blocks(doc.element.body))


def docx_inspect(job, index):
    doc = docx_open(job, index)
    body = doc.element.body
    paragraphs = list(body.iter(W_P))
    tables = list(body.iter(W_TBL))
    p_index = {p: i for i, p in enumerate(paragraphs)}
    t_index = {t: i for i, t in enumerate(tables)}
    names = docx_style_names(doc)
    items = []
    for block in docx_blocks(body):
        if block.tag == W_P:
            item = {'kind': 'paragraph', 'paragraph': p_index[block], 'style': docx_paragraph_style(block, names),
                    'text': trunc(docx_text_of(block), ITEM_TEXT_CHARS)}
            if block.find('.//' + wq('numPr')) is not None:
                item['list'] = True
            if block.find('.//' + wq('drawing')) is not None or block.find('.//' + wq('pict')) is not None:
                item['has_image'] = True
            items.append(item)
        else:
            rows = docx_rows(block)
            first = next(block.iter(W_P), None)
            item = {'kind': 'table', 'table': t_index[block], 'rows': len(rows),
                    'cols': max((len(row) for row in rows), default=0),
                    'cells': [[trunc(docx_cell_text(cell), CELL_TEXT_CHARS) for cell in row] for row in rows[:60]],
                    'first_paragraph': p_index.get(first)}
            if len(rows) > 60:
                item['cells_truncated'] = True
            items.append(item)
    base = {'format': 'docx', 'source_sha256': sha256(job.data(index)), 'paragraphs': len(paragraphs),
            'tables': len(tables), 'sections': len(doc.sections)}
    return fit_items(job, base, items)


def docx_metadata(job, index):
    doc = docx_open(job, index)
    body = doc.element.body
    result = {'paragraphs': len(list(body.iter(W_P))), 'tables': len(list(body.iter(W_TBL))),
              'sections': len(doc.sections), 'images': len(list(body.iter(wq('drawing')))),
              'properties': core_properties(doc.core_properties)}
    with zipfile.ZipFile(io.BytesIO(job.data(index))) as archive:
        if 'docProps/app.xml' in archive.namelist():
            from lxml import etree
            try:
                app = etree.fromstring(archive.read('docProps/app.xml'), safe_xml_parser())
                for name in ('Pages', 'Words', 'Characters', 'Application'):
                    node = app.find('{http://schemas.openxmlformats.org/officeDocument/2006/extended-properties}'
                                    + name)
                    if node is not None and node.text:
                        result[name.lower()] = int(node.text) if node.text.isdigit() else node.text
            except etree.XMLSyntaxError:
                pass
    if len(doc.sections):
        section = doc.sections[0]
        if section.page_width and section.page_height:
            result['page_size_mm'] = [round(section.page_width.mm, 1), round(section.page_height.mm, 1)]
    return result


class DocxBuilder:
    """Builds the shared block vocabulary into a python-docx Document body."""

    def __init__(self, job, doc):
        self.job, self.doc = job, doc
        self.styles = set()
        for style in doc.styles:
            try:
                self.styles.add(style.name)
            except Exception:
                continue
        self.counts = {'blocks': 0, 'paragraphs': 0, 'tables': 0, 'images': 0}
        self.image_cache = {}

    def style(self, *names):
        for name in names:
            if name and name in self.styles:
                return name
        return None

    def text_width(self):
        from docx.shared import Cm
        section = self.doc.sections[-1]
        try:
            return int(section.page_width - section.left_margin - section.right_margin)
        except TypeError:
            return int(Cm(16))

    def runs_of(self, block, path):
        if block.get('runs') is not None:
            runs = as_list(block['runs'], path + '.runs', 10_000)
            return [{'text': r} if isinstance(r, str) else r for r in runs], path + '.runs'
        if block.get('text') is not None:
            return [{'text': block['text']}], path + '.text'
        return [], path

    def add_runs(self, paragraph, runs, path, base=None):
        from docx.enum.text import WD_COLOR_INDEX
        from docx.shared import Pt, RGBColor
        base = base or {}
        for index, run_spec in enumerate(runs):
            rpath = '%s[%d]' % (path, index) if path.endswith('.runs') else path
            if isinstance(run_spec, str):
                run_spec = {'text': run_spec}
            as_dict(run_spec, rpath, RUN_KEYS)
            spec = dict(base, **run_spec)
            if spec.get('math'):
                self.job.warn('Math is written as LaTeX source in DOCX', 'docx-math')
                text = self.job.text(spec['math'], rpath + '.math')
                spec['code'] = True
            else:
                text = self.job.text(spec.get('text', ''), rpath if rpath.endswith('.text') else rpath + '.text')
            if spec.get('link'):
                url = as_str(spec['link'], rpath + '.link', 2048)
                need(URL_PATTERN.match(url) is not None, 'invalid_spec',
                     '%s.link must be an http(s) or mailto URL' % rpath, rpath + '.link')
                run = self.hyperlink(paragraph, url, text)
            else:
                run = paragraph.add_run(text)
            font = run.font
            for key, attr in (('bold', 'bold'), ('italic', 'italic'), ('underline', 'underline'),
                              ('strike', 'strike'), ('superscript', 'superscript'), ('subscript', 'subscript')):
                if opt(spec, key, rpath, as_bool):
                    setattr(font, attr, True)
            if opt(spec, 'code', rpath, as_bool):
                font.name = 'Consolas'
            size = opt(spec, 'size', rpath, as_num, 4, 400)
            if size:
                font.size = Pt(size)
            name = opt(spec, 'font', rpath, as_str, 64, pattern=r'[\w .\-]+')
            if name:
                font.name = name
            color = opt(spec, 'color', rpath, hex_color)
            if color:
                font.color.rgb = RGBColor.from_string(color)
            highlight = opt(spec, 'highlight', rpath, as_str, 32, tuple(sorted(HIGHLIGHTS)))
            if highlight:
                font.highlight_color = getattr(WD_COLOR_INDEX, HIGHLIGHTS[highlight])

    def hyperlink(self, paragraph, url, text):
        from docx.opc.constants import RELATIONSHIP_TYPE
        from docx.oxml import OxmlElement
        from docx.oxml.ns import qn
        from docx.shared import RGBColor
        rid = paragraph.part.relate_to(url, RELATIONSHIP_TYPE.HYPERLINK, is_external=True)
        link = OxmlElement('w:hyperlink')
        link.set(qn('r:id'), rid)
        run = paragraph.add_run(text)
        style = self.style('Hyperlink')
        if style:
            run.style = style
        else:
            run.font.color.rgb = RGBColor(0x05, 0x63, 0xC1)
            run.font.underline = True
        link.append(run._r)
        paragraph._p.append(link)
        return run

    def blocks(self, blocks, path):
        as_list(blocks, path, MAX_BLOCKS)
        for index, block in enumerate(blocks):
            self.block(block, '%s[%d]' % (path, index))

    def block(self, block, path):
        if isinstance(block, str):
            block = {'type': 'paragraph', 'text': block}
        as_dict(block, path, required=('type',))
        kind = as_str(block['type'], path + '.type', 32)
        self.counts['blocks'] += 1
        need(self.counts['blocks'] <= MAX_BLOCKS * 4, 'limit', 'too many blocks', path)
        handler = getattr(self, 'block_' + kind, None) if kind in BLOCK_TYPES else None
        need(handler is not None, 'invalid_spec', '%s.type "%s" is unknown; use one of: %s' % (
            path, kind, ', '.join(BLOCK_TYPES)), path + '.type')
        handler(block, path)

    def paragraph_format(self, paragraph, block, path):
        from docx.enum.text import WD_ALIGN_PARAGRAPH
        from docx.shared import Cm, Pt
        align = opt(block, 'align', path, as_str, 16, ('left', 'center', 'right', 'justify'))
        if align:
            paragraph.alignment = {'left': WD_ALIGN_PARAGRAPH.LEFT, 'center': WD_ALIGN_PARAGRAPH.CENTER,
                                   'right': WD_ALIGN_PARAGRAPH.RIGHT, 'justify': WD_ALIGN_PARAGRAPH.JUSTIFY}[align]
        space = opt(block, 'space_after', path, as_num, 0, 200)
        if space is not None:
            paragraph.paragraph_format.space_after = Pt(space)
        indent = opt(block, 'indent_cm', path, as_num, -10, 20)
        if indent is not None:
            paragraph.paragraph_format.left_indent = Cm(indent)

    def block_paragraph(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs', 'style', 'align', 'space_after', 'indent_cm'})
        style_name = opt(block, 'style', path, as_str, 128)
        style = self.style(style_name) if style_name else None
        if style_name and not style:
            self.job.warn('Paragraph style "%s" is not defined; Normal is used' % style_name, 'style:' + style_name)
        paragraph = self.doc.add_paragraph(style=style)
        runs, rpath = self.runs_of(block, path)
        self.add_runs(paragraph, runs, rpath)
        self.paragraph_format(paragraph, block, path)
        self.counts['paragraphs'] += 1
        return paragraph

    def block_heading(self, block, path):
        from docx.shared import Pt
        as_dict(block, path, {'type', 'text', 'runs', 'level', 'align'})
        level = opt(block, 'level', path, as_int, 0, 9, default=1)
        style = self.style('Title') if level == 0 else self.style('Heading %d' % level)
        paragraph = self.doc.add_paragraph(style=style)
        runs, rpath = self.runs_of(block, path)
        self.add_runs(paragraph, runs, rpath, None if style else {'bold': True, 'size': max(12, 24 - 2 * level)})
        if not style:
            self.job.warn('Heading styles are missing in the template; bold text is used', 'heading-style')
            paragraph.paragraph_format.space_before = Pt(12)
        self.paragraph_format(paragraph, block, path)
        self.counts['paragraphs'] += 1

    def block_quote(self, block, path):
        from docx.shared import Cm
        as_dict(block, path, {'type', 'text', 'runs'})
        style = self.style('Quote', 'Intense Quote')
        paragraph = self.doc.add_paragraph(style=style)
        runs, rpath = self.runs_of(block, path)
        self.add_runs(paragraph, runs, rpath, None if style else {'italic': True})
        if not style:
            paragraph.paragraph_format.left_indent = Cm(1)
        self.counts['paragraphs'] += 1

    def block_code(self, block, path):
        from docx.oxml import OxmlElement
        from docx.oxml.ns import qn
        from docx.shared import Pt
        as_dict(block, path, {'type', 'text', 'language'})
        text = self.job.text(block.get('text') or '', path + '.text')
        paragraph = self.doc.add_paragraph(style=self.style('No Spacing'))
        run = paragraph.add_run(text)
        run.font.name = 'Consolas'
        run.font.size = Pt(9.5)
        shade = OxmlElement('w:shd')
        shade.set(qn('w:val'), 'clear')
        shade.set(qn('w:color'), 'auto')
        shade.set(qn('w:fill'), 'F2F2F2')
        paragraph._p.get_or_add_pPr().insert_element_before(shade, *PPR_AFTER_PBDR[1:])
        self.counts['paragraphs'] += 1

    def block_math(self, block, path):
        as_dict(block, path, {'type', 'latex'}, ('latex',))
        self.job.warn('Math is written as LaTeX source in DOCX', 'docx-math')
        self.block_code({'type': 'code', 'text': as_str(block['latex'], path + '.latex', 20_000)}, path)

    def block_page_break(self, block, path):
        as_dict(block, path, {'type'})
        self.doc.add_page_break()

    def block_horizontal_rule(self, block, path):
        from docx.oxml import OxmlElement
        from docx.oxml.ns import qn
        as_dict(block, path, {'type'})
        paragraph = self.doc.add_paragraph()
        border = OxmlElement('w:pBdr')
        bottom = OxmlElement('w:bottom')
        for key, value in (('w:val', 'single'), ('w:sz', '6'), ('w:space', '1'), ('w:color', 'auto')):
            bottom.set(qn(key), value)
        border.append(bottom)
        paragraph._p.get_or_add_pPr().insert_element_before(border, *PPR_AFTER_PBDR)

    def block_toc(self, block, path):
        from docx.oxml import OxmlElement
        from docx.oxml.ns import qn
        as_dict(block, path, {'type', 'levels', 'title'})
        levels = opt(block, 'levels', path, as_int, 1, 9, default=3)
        if block.get('title'):
            self.block_heading({'type': 'heading', 'level': 1, 'text': block['title']}, path)
        paragraph = self.doc.add_paragraph()
        for kind, text in (('begin', None), ('instr', ' TOC \\o "1-%d" \\h \\z \\u ' % levels), ('separate', None),
                           ('text', 'Right-click and choose "Update field" to build the table of contents.'),
                           ('end', None)):
            run = OxmlElement('w:r')
            if text is None:
                char = OxmlElement('w:fldChar')
                char.set(qn('w:fldCharType'), kind)
                run.append(char)
            else:
                node = OxmlElement('w:instrText' if kind == 'instr' else 'w:t')
                node.set(qn('xml:space'), 'preserve')
                node.text = text
                run.append(node)
            paragraph._p.append(run)

    def block_markdown(self, block, path):
        as_dict(block, path, {'type', 'text'})
        text = self.job.text(block.get('text') or '', path + '.text', 2_000_000)
        for index, child in enumerate(md_blocks(text)):
            self.block(child, '%s.markdown[%d]' % (path, index))

    def block_list(self, block, path, level=0, ordered=None):
        from docx.shared import Cm
        as_dict(block, path, {'type', 'items', 'ordered', 'start'})
        ordered = opt(block, 'ordered', path, as_bool, default=False) if ordered is None else ordered
        start = opt(block, 'start', path, as_int, 0, 10_000, default=1)
        items = as_list(block.get('items') or [], path + '.items', MAX_LIST_ITEMS)
        restart = {}
        for index, item in enumerate(items):
            ipath = '%s.items[%d]' % (path, index)
            if isinstance(item, str):
                item = {'text': item}
            as_dict(item, ipath, {'text', 'runs', 'items', 'ordered'})
            base = 'List Number' if ordered else 'List Bullet'
            depth = min(level, 2)
            style = self.style(base if depth == 0 else '%s %d' % (base, depth + 1), base)
            paragraph = self.doc.add_paragraph(style=style)
            runs, rpath = self.runs_of(item, ipath)
            if style is None:
                runs = [{'text': ('%d. ' % (start + index)) if ordered else '• '}] + runs
                paragraph.paragraph_format.left_indent = Cm(0.75 * (level + 1))
            else:
                if ordered:
                    self.restart_numbering(paragraph, style, start, restart)
                if level > 2:
                    paragraph.paragraph_format.left_indent = Cm(0.75 * (level + 1))
            self.add_runs(paragraph, runs, rpath)
            self.counts['paragraphs'] += 1
            if item.get('items'):
                child_ordered = opt(item, 'ordered', ipath, as_bool, default=ordered)
                self.block_list({'type': 'list', 'items': item['items']}, ipath, level + 1, child_ordered)

    def restart_numbering(self, paragraph, style_name, start, cache):
        """Give each ordered list its own w:num so its numbering starts at `start`."""
        if style_name not in cache:
            cache[style_name] = None
            element, num_id, ilvl, seen = self.doc.styles[style_name].element, None, 0, set()
            while element is not None and num_id is None and id(element) not in seen:
                seen.add(id(element))
                node = element.find('%s/%s' % (wq('pPr'), wq('numPr')))
                if node is not None and node.find(wq('numId')) is not None:
                    num_id = int(node.find(wq('numId')).get(wq('val')))
                    if node.find(wq('ilvl')) is not None:
                        ilvl = int(node.find(wq('ilvl')).get(wq('val')))
                based = element.find(wq('basedOn'))
                element = None
                if based is not None:
                    try:
                        element = self.doc.styles.element.get_by_id(based.get(wq('val')))
                    except Exception:
                        element = None
            if num_id is not None:
                numbering = self.doc.part.numbering_part.element
                num = numbering.num_having_numId(num_id)
                abstract = num.find(wq('abstractNumId')) if num is not None else None
                if abstract is not None:
                    new = numbering.add_num(int(abstract.get(wq('val'))))
                    new.add_lvlOverride(ilvl=ilvl).add_startOverride(start)
                    cache[style_name] = (new.numId, ilvl)
        target = cache[style_name]
        if target:
            num_pr = paragraph._p.get_or_add_pPr().get_or_add_numPr()
            num_pr.get_or_add_ilvl().val = target[1]
            num_pr.get_or_add_numId().val = target[0]

    def block_table(self, block, path):
        from docx.enum.text import WD_ALIGN_PARAGRAPH
        from docx.oxml import OxmlElement
        from docx.oxml.ns import qn
        from docx.shared import Cm
        as_dict(block, path, {'type', 'rows', 'header', 'style', 'widths_cm', 'align', 'caption'})
        rows = as_list(block.get('rows'), path + '.rows', 5_000, 1)
        width = 0
        for index, row in enumerate(rows):
            as_list(row, '%s.rows[%d]' % (path, index), 200)
            width = max(width, sum(cell.get('colspan', 1) if isinstance(cell, dict) and
                                   type(cell.get('colspan', 1)) is int else 1 for cell in row))
        need(width > 0, 'invalid_spec', '%s.rows has no cells' % path, path + '.rows')
        need(width * len(rows) <= MAX_TABLE_CELLS, 'limit', '%s has more than %d cells' % (path, MAX_TABLE_CELLS),
             path + '.rows')
        table = self.doc.add_table(rows=len(rows), cols=width)
        style = self.style(opt(block, 'style', path, as_str, 128) or 'Table Grid', 'Table Grid')
        if style:
            table.style = style
        header = opt(block, 'header', path, as_bool, default=True)
        aligns = opt(block, 'align', path, as_list, 200) or []
        alignment = {'left': WD_ALIGN_PARAGRAPH.LEFT, 'center': WD_ALIGN_PARAGRAPH.CENTER,
                     'right': WD_ALIGN_PARAGRAPH.RIGHT, 'justify': WD_ALIGN_PARAGRAPH.JUSTIFY}
        for r, row in enumerate(rows):
            column = 0
            for c, value in enumerate(row):
                cpath = '%s.rows[%d][%d]' % (path, r, c)
                spec = value if isinstance(value, dict) else {
                    'text': '' if value is None else value if isinstance(value, str) else json.dumps(value)}
                as_dict(spec, cpath, {'text', 'runs', 'bold', 'italic', 'align', 'fill', 'colspan', 'color'})
                span = opt(spec, 'colspan', cpath, as_int, 1, width, default=1)
                need(column + span <= width, 'invalid_spec', '%s spans past the table width' % cpath, cpath)
                cell = table.cell(r, column)
                if span > 1:
                    cell = cell.merge(table.cell(r, column + span - 1))
                paragraph = cell.paragraphs[0]
                runs, rpath = self.runs_of(spec, cpath)
                base = {key: spec[key] for key in ('bold', 'italic', 'color') if spec.get(key) is not None}
                if header and r == 0 and 'bold' not in base:
                    base['bold'] = True
                self.add_runs(paragraph, runs, rpath, base)
                align = opt(spec, 'align', cpath, as_str, 16, tuple(alignment))
                if align is None and column < len(aligns) and aligns[column] in alignment:
                    align = aligns[column]
                if align:
                    paragraph.alignment = alignment[align]
                fill = opt(spec, 'fill', cpath, hex_color)
                if fill:
                    shade = OxmlElement('w:shd')
                    shade.set(qn('w:val'), 'clear')
                    shade.set(qn('w:color'), 'auto')
                    shade.set(qn('w:fill'), fill)
                    cell._tc.get_or_add_tcPr().insert_element_before(shade, *TCPR_AFTER_SHD)
                column += span
        if header:
            mark = OxmlElement('w:tblHeader')
            mark.set(qn('w:val'), 'true')
            table.rows[0]._tr.get_or_add_trPr().append(mark)
        widths = opt(block, 'widths_cm', path, as_list, 200)
        if widths:
            table.autofit = False
            for c, value in enumerate(widths[:width]):
                size = Cm(as_num(value, '%s.widths_cm[%d]' % (path, c), 0.3, 60))
                for row in table.rows:
                    row.cells[c].width = size
        if block.get('caption') is not None:
            caption = self.doc.add_paragraph(style=self.style('Caption'))
            self.add_runs(caption, [{'text': block['caption']}], path + '.caption',
                          {} if self.style('Caption') else {'italic': True})
        self.counts['tables'] += 1

    def block_image(self, block, path):
        from docx.enum.text import WD_ALIGN_PARAGRAPH
        from docx.shared import Cm, Emu
        as_dict(block, path, {'type', 'input', 'width_cm', 'height_cm', 'caption', 'align'}, ('input',))
        index = as_int(block['input'], path + '.input', 0)
        if index not in self.image_cache:
            self.image_cache[index] = prepare_image(self.job, index, path + '.input')
        data, _, pixel_w, _ = self.image_cache[index]
        width = opt(block, 'width_cm', path, as_num, 0.2, 100)
        height = opt(block, 'height_cm', path, as_num, 0.2, 100)
        if width is None and height is None:
            self.doc.add_picture(io.BytesIO(data), width=Emu(min(int(pixel_w / 96 * 914400), self.text_width())))
        else:
            self.doc.add_picture(io.BytesIO(data), width=Cm(width) if width else None,
                                 height=Cm(height) if height else None)
        paragraph = self.doc.paragraphs[-1]
        align = opt(block, 'align', path, as_str, 16, ('left', 'center', 'right'), default='center')
        paragraph.alignment = {'left': WD_ALIGN_PARAGRAPH.LEFT, 'center': WD_ALIGN_PARAGRAPH.CENTER,
                               'right': WD_ALIGN_PARAGRAPH.RIGHT}[align]
        if block.get('caption') is not None:
            caption = self.doc.add_paragraph(style=self.style('Caption'))
            self.add_runs(caption, [{'text': block['caption']}], path + '.caption',
                          {} if self.style('Caption') else {'italic': True})
            caption.alignment = paragraph.alignment
        self.counts['images'] += 1


def docx_add_text_with_fields(paragraph, text):
    from docx.oxml import OxmlElement
    from docx.oxml.ns import qn
    for part in re.split(r'(\{page\}|\{pages\})', text):
        if part in ('{page}', '{pages}'):
            field = OxmlElement('w:fldSimple')
            field.set(qn('w:instr'), 'PAGE' if part == '{page}' else 'NUMPAGES')
            run = OxmlElement('w:r')
            node = OxmlElement('w:t')
            node.text = '1'
            run.append(node)
            field.append(run)
            paragraph._p.append(field)
        elif part:
            paragraph.add_run(part)


def docx_setup(job, doc, spec, template):
    from docx.enum.section import WD_ORIENT
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.shared import Cm, Mm, Pt
    page = spec.get('page')
    if page is not None:
        as_dict(page, 'spec.page', {'size', 'orientation', 'margins_cm'})
        size = opt(page, 'size', 'spec.page', as_str, 16, tuple(PAGE_SIZES_MM))
        orientation = opt(page, 'orientation', 'spec.page', as_str, 16, ('portrait', 'landscape'))
        margins = page.get('margins_cm')
        for section in doc.sections:
            if size:
                section.page_width, section.page_height = Mm(PAGE_SIZES_MM[size][0]), Mm(PAGE_SIZES_MM[size][1])
            elif template is None:
                section.page_width, section.page_height = Mm(210), Mm(297)
            if orientation:
                portrait = section.page_width < section.page_height
                if (orientation == 'landscape') == portrait:
                    section.page_width, section.page_height = section.page_height, section.page_width
                section.orientation = WD_ORIENT.LANDSCAPE if orientation == 'landscape' else WD_ORIENT.PORTRAIT
            if margins is not None:
                if isinstance(margins, dict):
                    as_dict(margins, 'spec.page.margins_cm', {'top', 'bottom', 'left', 'right'})
                    for key in ('top', 'bottom', 'left', 'right'):
                        if margins.get(key) is not None:
                            setattr(section, key + '_margin',
                                    Cm(as_num(margins[key], 'spec.page.margins_cm.' + key, 0, 10)))
                else:
                    value = Cm(as_num(margins, 'spec.page.margins_cm', 0, 10))
                    section.top_margin = section.bottom_margin = section.left_margin = section.right_margin = value
            elif template is None:
                section.left_margin = section.right_margin = section.top_margin = section.bottom_margin = Cm(2.5)
    elif template is None:
        for section in doc.sections:
            section.page_width, section.page_height = Mm(210), Mm(297)
            section.left_margin = section.right_margin = section.top_margin = section.bottom_margin = Cm(2.5)
    font = spec.get('font')
    if font is not None:
        as_dict(font, 'spec.font', {'name', 'size'})
        normal = next((s for s in doc.styles if getattr(s, 'name', None) == 'Normal'), None)
        if normal is not None:
            if font.get('name'):
                normal.font.name = as_str(font['name'], 'spec.font.name', 64, pattern=r'[\w .\-]+')
            if font.get('size'):
                normal.font.size = Pt(as_num(font['size'], 'spec.font.size', 6, 72))
    for key in ('header', 'footer'):
        value = spec.get(key)
        if value is None:
            continue
        if isinstance(value, str):
            value = {'text': value}
        as_dict(value, 'spec.' + key, {'text', 'align'})
        text = job.text(value.get('text') or '', 'spec.%s.text' % key, 2000)
        align = opt(value, 'align', 'spec.' + key, as_str, 16, ('left', 'center', 'right'), default='center')
        for section in doc.sections:
            part = section.header if key == 'header' else section.footer
            part.is_linked_to_previous = False
            paragraph = part.paragraphs[0] if part.paragraphs else part.add_paragraph()
            for child in list(paragraph._p):
                if child.tag != wq('pPr'):
                    paragraph._p.remove(child)
            docx_add_text_with_fields(paragraph, text)
            paragraph.alignment = {'left': WD_ALIGN_PARAGRAPH.LEFT, 'center': WD_ALIGN_PARAGRAPH.CENTER,
                                   'right': WD_ALIGN_PARAGRAPH.RIGHT}[align]


def docx_create(job, spec):
    import docx
    as_dict(spec, 'spec', {'format', 'template_input', 'keep_template_content', 'values', 'properties', 'page',
                           'font', 'header', 'footer', 'blocks', 'markdown'})
    template = spec.get('template_input')
    need(spec.get('blocks') is not None or spec.get('markdown') is not None or template is not None, 'invalid_spec',
         'spec.blocks or spec.markdown is required', 'spec.blocks')
    if template is not None:
        as_int(template, 'spec.template_input', 0)
        need(job.kind(template, 'spec.template_input') == 'docx', 'invalid_spec',
             'spec.template_input must be a docx/dotx', 'spec.template_input')
        need(ext_of(job.input(template)['name']) not in MACRO_EXTENSIONS, 'unsupported',
             'macro-enabled templates are not supported', 'spec.template_input')
        doc = docx_open(job, template, 'spec.template_input')
        if not opt(spec, 'keep_template_content', 'spec', as_bool, default=False):
            body = doc.element.body
            for child in list(body):
                if child.tag != wq('sectPr'):
                    body.remove(child)
    else:
        doc = docx.Document()
    set_core_properties(job, doc.core_properties, spec, 'spec.properties', template is None)
    docx_setup(job, doc, spec, template)
    builder = DocxBuilder(job, doc)
    if spec.get('markdown') is not None:
        builder.block_markdown({'type': 'markdown', 'text': spec['markdown']}, 'spec')
    if spec.get('blocks') is not None:
        builder.blocks(spec['blocks'], 'spec.blocks')
    result = dict(builder.counts)
    if spec.get('values') is not None:
        result.update(docx_fill(job, doc, values_of(job, spec['values'], 'spec.values')))
    return save_docx(job, doc), result


def save_docx(job, doc):
    body = doc.element.body
    if not any(child.tag in (W_P, W_TBL) for child in body):
        from docx.oxml import OxmlElement
        sect = body.find(wq('sectPr'))
        if sect is not None:
            sect.addprevious(OxmlElement('w:p'))
        else:
            body.append(OxmlElement('w:p'))
    buffer = io.BytesIO()
    doc.save(buffer)
    data = buffer.getvalue()
    validate_package(job, data, 'docx')
    return data


def docx_text_parents(doc):
    parents = [doc.element.body]
    for section in doc.sections:
        for name in ('header', 'footer', 'first_page_header', 'first_page_footer', 'even_page_header',
                     'even_page_footer'):
            try:
                part = getattr(section, name)
                if not part.is_linked_to_previous:
                    parents.append(part._element)
            except Exception:
                continue
    unique = []
    for parent in parents:
        if all(parent is not other for other in unique):
            unique.append(parent)
    return unique


def docx_paragraph_nodes(paragraph):
    return [node for node in paragraph.iter(W_T) if next(node.iterancestors(W_P), None) is paragraph]


def docx_fill(job, doc, values):
    filled, unfilled = {}, set()

    def replace(match):
        key = match.group(1)
        if key in values:
            filled[key] = filled.get(key, 0) + 1
            return values[key]
        unfilled.add(key)
        return None
    for parent in docx_text_parents(doc):
        for paragraph in parent.iter(W_P):
            replace_in_nodes(docx_paragraph_nodes(paragraph), PLACEHOLDER, replace, W_T)
    missing = sorted(set(values) - set(filled))
    if missing:
        job.warn('Placeholders not found in the document: %s' % ', '.join(missing[:20]), 'unused-values')
    return {'filled': sum(filled.values()), 'unfilled': sorted(unfilled)[:100]}


def docx_replace_text(doc, find, replacement, match_case, limit):
    pattern = re.compile(re.escape(find), 0 if match_case else re.I)
    total = 0
    for parent in docx_text_parents(doc):
        for paragraph in parent.iter(W_P):
            if limit and total >= limit:
                return total
            budget = [limit - total if limit else None]

            def replace(match):
                if budget[0] is not None:
                    if budget[0] <= 0:
                        return None
                    budget[0] -= 1
                return replacement
            total += replace_in_nodes(docx_paragraph_nodes(paragraph), pattern, replace, W_T)
    return total


def docx_set_text(paragraph_element, runs, path, builder):
    """Replace a paragraph's text; keeps paragraph properties, drawings and the first run's formatting."""
    from docx.text.paragraph import Paragraph
    own_runs = [r for r in paragraph_element.iter(W_R) if next(r.iterancestors(W_P), None) is paragraph_element]
    text_runs = [r for r in own_runs if any(child.tag in _DOCX_TEXTLIKE for child in r)]
    template = None
    if text_runs:
        props = text_runs[0].find(wq('rPr'))
        template = copy.deepcopy(props) if props is not None else None
        for run in text_runs:
            if all(child.tag in _DOCX_TEXTLIKE or child.tag == wq('rPr') for child in run):
                run.getparent().remove(run)
            else:
                for child in list(run):
                    if child.tag in _DOCX_TEXTLIKE:
                        run.remove(child)
    paragraph = Paragraph(paragraph_element, builder.doc._body)
    plain = all(isinstance(run, str) or set(run) <= {'text'} for run in runs)
    before = set(paragraph_element.iterchildren(W_R))
    builder.add_runs(paragraph, runs, path)
    if plain and template is not None:
        for run in paragraph_element.iterchildren(W_R):
            if run not in before:
                old = run.find(wq('rPr'))
                if old is not None:
                    run.remove(old)
                run.insert(0, copy.deepcopy(template))


def docx_move_new(body, before, anchor, where):
    """Move body children created after `before` next to the anchor paragraph."""
    created = [child for child in body if child not in before and child.tag != wq('sectPr')]
    if anchor is not None:
        if where == 'after':
            target = anchor
            for element in created:
                target.addnext(element)
                target = element
        else:
            for element in created:
                anchor.addprevious(element)
    return created


DOCX_EDIT_OPS = ('replace_paragraph', 'set_style', 'insert_paragraph', 'insert_blocks', 'insert_image',
                 'insert_page_break', 'append_blocks', 'delete_paragraph', 'delete_table', 'set_cell',
                 'add_table_row', 'delete_table_row', 'fill_placeholders', 'replace_text', 'set_properties')


def docx_edit(job, spec):
    from docx.oxml import OxmlElement
    as_dict(spec, 'spec', {'source_sha256', 'patches', 'properties'}, ('source_sha256', 'patches'))
    doc = docx_open(job, 0)
    body = doc.element.body
    paragraphs = list(body.iter(W_P))
    tables = list(body.iter(W_TBL))
    p_index = {p: i for i, p in enumerate(paragraphs)}
    removed = set()
    builder = DocxBuilder(job, doc)
    stats = {'patches_applied': 0}

    def forget(element):
        for nested in element.iter(W_P):
            if nested in p_index:
                removed.add(p_index[nested])

    def paragraph_at(value, path):
        index = as_int(value, path, 0)
        need(index < len(paragraphs), 'invalid_spec', '%s: paragraph %d does not exist (0..%d)' % (
            path, index, len(paragraphs) - 1), path)
        need(index not in removed, 'invalid_spec', '%s: paragraph %d was deleted by an earlier patch' % (
            path, index), path)
        return paragraphs[index]

    def table_at(value, path):
        index = as_int(value, path, 0)
        need(index < len(tables), 'invalid_spec', '%s: table %d does not exist' % (path, index), path)
        need(('table', index) not in removed, 'invalid_spec', '%s: table %d was deleted' % (path, index), path)
        return tables[index]

    def anchor_of(patch, path):
        if patch.get('after') is not None:
            return paragraph_at(patch['after'], path + '.after'), 'after'
        if patch.get('before') is not None:
            return paragraph_at(patch['before'], path + '.before'), 'before'
        fail('invalid_spec', '%s needs "after" or "before" (a paragraph index)' % path, path)

    def insert_blocks(blocks, path, anchor, where):
        as_list(blocks, path, MAX_BLOCKS)
        if next(anchor.iterancestors(W_TC), None) is not None:
            for block in blocks:
                kind = block.get('type') if isinstance(block, dict) else 'paragraph'
                need(kind in ('paragraph', 'heading', 'list', 'quote', 'code', 'page_break'), 'invalid_spec',
                     '%s: only paragraph-like blocks can be inserted inside a table cell' % path, path)
        before = set(body)
        builder.blocks(blocks, path)
        docx_move_new(body, before, anchor, where)

    for index, patch in enumerate(as_list(spec['patches'], 'spec.patches', MAX_PATCHES, 1)):
        path = 'spec.patches[%d]' % index
        as_dict(patch, path)
        op = patch.get('op') or ('replace_paragraph' if 'paragraph' in patch and 'text' in patch else None)
        need(isinstance(op, str), 'invalid_spec', '%s.op is required; use one of: %s' % (
            path, ', '.join(DOCX_EDIT_OPS)), path + '.op')
        if op == 'replace_paragraph':
            as_dict(patch, path, {'op', 'paragraph', 'text', 'runs', 'style'}, ('paragraph',))
            need('text' in patch or 'runs' in patch, 'invalid_spec', '%s needs text or runs' % path, path)
            element = paragraph_at(patch['paragraph'], path + '.paragraph')
            runs, rpath = builder.runs_of(patch, path)
            docx_set_text(element, runs, rpath, builder)
            if patch.get('style'):
                docx_set_style(builder, element, patch['style'], path + '.style')
        elif op == 'set_style':
            as_dict(patch, path, {'op', 'paragraph', 'style'}, ('paragraph', 'style'))
            docx_set_style(builder, paragraph_at(patch['paragraph'], path + '.paragraph'), patch['style'],
                           path + '.style')
        elif op in ('insert_paragraph', 'insert_blocks', 'insert_image', 'insert_page_break'):
            extra = {'insert_paragraph': {'text', 'runs', 'style', 'align'}, 'insert_blocks': {'blocks'},
                     'insert_image': {'input', 'width_cm', 'height_cm', 'caption', 'align'},
                     'insert_page_break': set()}[op]
            as_dict(patch, path, {'op', 'after', 'before'} | extra)
            anchor, where = anchor_of(patch, path)
            fields = {k: v for k, v in patch.items() if k not in ('op', 'after', 'before')}
            if op == 'insert_paragraph':
                insert_blocks([dict(fields, type='paragraph')], path, anchor, where)
            elif op == 'insert_image':
                insert_blocks([dict(fields, type='image')], path, anchor, where)
            elif op == 'insert_page_break':
                insert_blocks([{'type': 'page_break'}], path, anchor, where)
            else:
                need(patch.get('blocks') is not None, 'invalid_spec', '%s.blocks is required' % path, path)
                insert_blocks(patch['blocks'], path + '.blocks', anchor, where)
        elif op == 'append_blocks':
            as_dict(patch, path, {'op', 'blocks', 'markdown'})
            need(patch.get('blocks') is not None or patch.get('markdown') is not None, 'invalid_spec',
                 '%s needs blocks or markdown' % path, path)
            if patch.get('markdown') is not None:
                builder.block_markdown({'type': 'markdown', 'text': patch['markdown']}, path)
            if patch.get('blocks') is not None:
                builder.blocks(patch['blocks'], path + '.blocks')
        elif op == 'delete_paragraph':
            as_dict(patch, path, {'op', 'paragraph'}, ('paragraph',))
            element = paragraph_at(patch['paragraph'], path + '.paragraph')
            parent = element.getparent()
            forget(element)
            if parent.tag == W_TC and len([c for c in parent if c.tag == W_P]) == 1:
                for child in list(element):
                    if child.tag != wq('pPr'):
                        element.remove(child)
            else:
                parent.remove(element)
        elif op == 'delete_table':
            as_dict(patch, path, {'op', 'table'}, ('table',))
            element = table_at(patch['table'], path + '.table')
            forget(element)
            removed.add(('table', patch['table']))
            element.getparent().remove(element)
        elif op in ('set_cell', 'add_table_row', 'delete_table_row'):
            docx_table_patch(job, builder, op, patch, path, table_at, forget)
        elif op == 'fill_placeholders':
            as_dict(patch, path, {'op', 'values'}, ('values',))
            stats.update(docx_fill(job, doc, values_of(job, patch['values'], path + '.values')))
        elif op == 'replace_text':
            as_dict(patch, path, {'op', 'find', 'replace', 'match_case', 'max'}, ('find', 'replace'))
            find = job.text(patch['find'], path + '.find', 2000)
            need(find, 'invalid_spec', '%s.find must not be empty' % path, path + '.find')
            count = docx_replace_text(doc, find, job.text(patch['replace'], path + '.replace', 20_000).replace(
                '\n', ' '), opt(patch, 'match_case', path, as_bool, default=True),
                opt(patch, 'max', path, as_int, 1, 1_000_000))
            stats['replacements'] = stats.get('replacements', 0) + count
            if not count:
                job.warn('%s: "%s" was not found' % (path, trunc(find, 60)))
        elif op == 'set_properties':
            as_dict(patch, path, {'op', 'title', 'author', 'subject', 'keywords', 'description', 'category'})
            set_core_properties(job, doc.core_properties,
                                {'properties': {k: v for k, v in patch.items() if k != 'op'}}, path, False)
        else:
            fail('invalid_spec', '%s.op "%s" is unknown; use one of: %s' % (path, op, ', '.join(DOCX_EDIT_OPS)),
                 path + '.op')
        stats['patches_applied'] += 1
    set_core_properties(job, doc.core_properties, spec, 'spec.properties', False)
    for cell in body.iter(W_TC):
        children = [child for child in cell if child.tag in (W_P, W_TBL)]
        if not children or children[-1].tag == W_TBL:
            cell.append(OxmlElement('w:p'))
    stats.update({k: v for k, v in builder.counts.items() if v})
    return save_docx(job, doc), stats


def docx_set_style(builder, element, value, path):
    from docx.text.paragraph import Paragraph
    name = as_str(value, path, 128)
    style = builder.style(name)
    need(style is not None, 'invalid_spec', '%s: style "%s" is not defined in this document' % (path, name), path)
    Paragraph(element, builder.doc._body).style = builder.doc.styles[style]


def docx_table_patch(job, builder, op, patch, path, table_at, forget):
    from docx.oxml import OxmlElement
    if op == 'set_cell':
        as_dict(patch, path, {'op', 'table', 'row', 'col', 'text', 'runs'}, ('table', 'row', 'col'))
        rows = docx_rows(table_at(patch['table'], path + '.table'))
        row = as_int(patch['row'], path + '.row', 0, len(rows) - 1)
        col = as_int(patch['col'], path + '.col', 0, len(rows[row]) - 1)
        cell = rows[row][col]
        cell_paragraphs = [child for child in cell if child.tag == W_P]
        if not cell_paragraphs:
            cell.append(OxmlElement('w:p'))
            cell_paragraphs = [child for child in cell if child.tag == W_P]
        for extra in cell_paragraphs[1:]:
            forget(extra)
            cell.remove(extra)
        runs, rpath = builder.runs_of(patch, path)
        docx_set_text(cell_paragraphs[0], runs, rpath, builder)
    elif op == 'add_table_row':
        as_dict(patch, path, {'op', 'table', 'values', 'after_row'}, ('table', 'values'))
        table = table_at(patch['table'], path + '.table')
        rows = list(table.iterchildren(W_TR))
        after = opt(patch, 'after_row', path, as_int, 0, len(rows) - 1, default=len(rows) - 1)
        values = as_list(patch['values'], path + '.values', 200)
        new = copy.deepcopy(rows[after])
        for cell_index, cell in enumerate(new.iterchildren(W_TC)):
            cell_paragraphs = [child for child in cell if child.tag == W_P]
            if not cell_paragraphs:
                cell.append(OxmlElement('w:p'))
                cell_paragraphs = [child for child in cell if child.tag == W_P]
            for child in list(cell):
                if child.tag == W_TBL or (child.tag == W_P and child is not cell_paragraphs[0]):
                    cell.remove(child)
            value = values[cell_index] if cell_index < len(values) else ''
            text = value if isinstance(value, str) else '' if value is None else json.dumps(value)
            docx_set_text(cell_paragraphs[0], [{'text': text}], '%s.values[%d]' % (path, cell_index), builder)
        for mark in list(new.iter(wq('tblHeader'))):
            mark.getparent().remove(mark)
        rows[after].addnext(new)
    else:
        as_dict(patch, path, {'op', 'table', 'row'}, ('table', 'row'))
        table = table_at(patch['table'], path + '.table')
        rows = list(table.iterchildren(W_TR))
        need(len(rows) > 1, 'invalid_spec', '%s: a table needs at least one row; use delete_table' % path, path)
        row = rows[as_int(patch['row'], path + '.row', 0, len(rows) - 1)]
        forget(row)
        table.remove(row)


# ---------------------------------------------------------------------------------------------
# XLSX
# ---------------------------------------------------------------------------------------------

_FORMULA_DENY = re.compile(r'(?i)\b(WEBSERVICE|FILTERXML|RTD|CALL|REGISTER|REGISTER\.ID|EXEC|FOPEN|FWRITE|FREAD|'
                           r'IMAGE|SQL\.REQUEST)\s*\(')
_SHEET_NAME_BAD = re.compile(r'[\[\]:*?/\\]')
_COLUMN = re.compile(r'^[A-Z]{1,3}$')
_CELL = r'[A-Z]{1,3}[1-9]\d{0,6}'
_RANGE = re.compile(r"^(?:(?:'(?P<q>(?:[^']|'')+)'|(?P<s>[^'!:\s]+))!)?(?P<r>\$?[A-Z]{1,3}\$?\d{1,7}"
                    r"(?::\$?[A-Z]{1,3}\$?\d{1,7})?)$")
CHART_TYPES_XLSX = ('column', 'bar', 'stacked_column', 'stacked_bar', 'line', 'pie', 'doughnut', 'area', 'scatter')
XLSX_EDIT_OPS = ('set_cell', 'set_range', 'append_rows', 'append_table_rows', 'add_sheet', 'delete_sheet', 'rename_sheet', 'insert_rows',
                 'delete_rows', 'insert_cols', 'delete_cols', 'style', 'merge', 'unmerge', 'freeze', 'autofilter',
                 'widths', 'number_formats', 'add_chart', 'add_table', 'fill_placeholders')


def check_formula(value, path):
    text = as_str(value, path, 8192)
    need(text.startswith('='), 'invalid_spec', '%s: a formula must start with "="' % path, path)
    code = re.sub(r'"(?:[^"]|"")*"', '""', text)
    need(not _FORMULA_DENY.search(code), 'invalid_spec',
         '%s: this formula function is not allowed (network/macro)' % path, path)
    need('|' not in code, 'invalid_spec', '%s: DDE formulas are not allowed' % path, path)
    need(not re.search(r"\[[^\[\]]*\.(xls[xmb]?|csv|ods)\]", code, re.I) and '\\\\' not in code and ':\\' not in code,
         'invalid_spec', '%s: links to other workbooks are not allowed' % path, path)
    return text


def xlsx_open(job, index, data_only=False, read_only=False, keep_links=False):
    import openpyxl
    item = job.input(index)
    data = check_ooxml(job, job.data(index), 'xlsx', item['path'])
    try:
        return openpyxl.load_workbook(io.BytesIO(data), read_only=read_only, data_only=data_only,
                                      keep_links=keep_links)
    except EngineError:
        raise
    except Exception as error:
        fail('corrupt_input', '%s cannot be opened as a workbook (%s)' % (item['path'], str(error)[:120]))


def xlsx_value(value):
    if value is None or isinstance(value, (str, int, bool)):
        return value
    if isinstance(value, float):
        return value if math.isfinite(value) else str(value)
    if hasattr(value, 'isoformat'):
        return value.isoformat()
    return str(value)


def xlsx_text(job, index):
    book = xlsx_open(job, index, data_only=True, read_only=True)
    try:
        lines, cells = [], 0
        for sheet in book.worksheets:
            lines.append('## Sheet: %s' % sheet.title)
            pending = 0
            for row in sheet.iter_rows(values_only=True):
                cells += len(row)
                need(cells <= MAX_SHEET_CELLS * 2, 'limit', 'the workbook has too many cells to read as text')
                values = ['' if v is None else str(xlsx_value(v)) for v in row]
                while values and values[-1] == '':
                    values.pop()
                line = '\t'.join(value.replace('\t', ' ').replace('\n', ' ') for value in values)
                if line:
                    lines.extend([''] * min(pending, 1000))
                    pending = 0
                    lines.append(line)
                else:
                    pending += 1
            lines.append('')
        return '\n'.join(lines).rstrip('\n')
    finally:
        book.close()


def xlsx_calculation(data, recalculated=False):
    """Report actual OOXML caches without treating an existing cache as verified."""
    from lxml import etree
    namespace = '{http://schemas.openxmlformats.org/spreadsheetml/2006/main}'
    formulas = missing = errors = 0
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for name in archive.namelist():
            if not name.startswith('xl/worksheets/') or not name.endswith('.xml'):
                continue
            root = etree.fromstring(archive.read(name), parser=safe_xml_parser())
            for cell in root.iter(namespace + 'c'):
                if cell.find(namespace + 'f') is None:
                    continue
                formulas += 1
                value = cell.find(namespace + 'v')
                if value is None or (value.text is None and cell.get('t') != 'str'):
                    missing += 1
                if cell.get('t') == 'e':
                    errors += 1
    status = 'no_formulas' if not formulas else 'unverified'
    if formulas and recalculated:
        status = 'errors' if errors else 'incomplete' if missing else 'recalculated'
    return {'status': status, 'engine': 'libreoffice' if recalculated else None,
            'formula_cells': formulas, 'missing_values': missing, 'error_values': errors}


def xlsx_inspect(job, index):
    book = xlsx_open(job, index)
    try:
        selected = job.options['sheet']
        need(selected is None or selected in book.sheetnames, 'invalid_spec',
             'options.sheet "%s" does not exist; sheets: %s' % (selected, ', '.join(book.sheetnames)),
             'options.sheet')
        sheets, items = [], []
        for sheet in book.worksheets:
            rows, cols = sheet.max_row or 0, sheet.max_column or 0
            info = {'name': sheet.title, 'max_row': rows, 'max_column': cols, 'state': sheet.sheet_state}
            if sheet.merged_cells.ranges:
                info['merged'] = [str(r) for r in list(sheet.merged_cells.ranges)[:50]]
            if sheet.freeze_panes:
                info['freeze'] = sheet.freeze_panes
            sheets.append(info)
            if selected is not None and sheet.title != selected:
                continue
            need(rows * cols <= MAX_SHEET_CELLS, 'limit', 'sheet %s has more than %d cells; use text or pick a '
                 'smaller sheet with options.sheet' % (sheet.title, MAX_SHEET_CELLS), 'options.sheet')
            for row in sheet.iter_rows():
                for cell in row:
                    if cell.value is None:
                        continue
                    value = xlsx_value(cell.value)
                    kind = ('formula' if cell.data_type == 'f' else 'boolean' if isinstance(value, bool) else
                            'number' if isinstance(value, (int, float)) else
                            'date' if hasattr(cell.value, 'isoformat') else 'text')
                    item = {'sheet': sheet.title, 'cell': cell.coordinate,
                            'value': trunc(value, ITEM_TEXT_CHARS) if isinstance(value, str) else value, 'type': kind}
                    if cell.number_format and cell.number_format != 'General':
                        item['number_format'] = cell.number_format
                    items.append(item)
                    need(len(items) <= 200_000, 'limit', 'too many cells; inspect one sheet with options.sheet',
                         'options.sheet')
        result = fit_items(job, {'format': 'xlsx', 'source_sha256': sha256(job.data(index)), 'sheets': sheets,
                                'calculation': xlsx_calculation(job.data(index))},
                           items)
    finally:
        book.close()
    formulas = [item for item in result['items'] if item['type'] == 'formula']
    if formulas:
        cached = xlsx_open(job, index, data_only=True)
        try:
            for item in formulas:
                item['cached_value'] = xlsx_value(cached[item['sheet']][item['cell']].value)
        finally:
            cached.close()
        if json_size(result) > job.limits['max_result_bytes']:
            for item in formulas:
                item.pop('cached_value', None)
    return result


def xlsx_metadata(job, index):
    book = xlsx_open(job, index, read_only=True)
    try:
        result = {'sheets': [{'name': s.title, 'max_row': s.max_row, 'max_column': s.max_column,
                              'state': s.sheet_state} for s in book.worksheets],
                  'properties': core_properties(book.properties), 'defined_names': len(book.defined_names)}
    finally:
        book.close()
    with zipfile.ZipFile(io.BytesIO(job.data(index))) as archive:
        names = archive.namelist()
    result['has_macros'] = any(name.endswith('vbaProject.bin') for name in names)
    result['has_charts'] = any(name.startswith('xl/charts/') for name in names)
    result['has_drawings'] = any(name.startswith('xl/drawings/') for name in names)
    return result


def parse_range(value, path, book, default_sheet):
    from openpyxl.utils.cell import range_boundaries
    text = as_str(value, path, 300).strip()
    if '!' in text:
        prefix, rest = text.rsplit('!', 1)
        text = prefix + '!' + rest.upper()
    else:
        text = text.upper()
    match = _RANGE.match(text)
    need(match is not None, 'invalid_spec', '%s: "%s" is not a cell range like "A1:C10" or "Data!B2:B9"' % (
        path, value), path)
    sheet_name = match.group('q').replace("''", "'") if match.group('q') else match.group('s')
    sheet = default_sheet
    if sheet_name:
        need(sheet_name in book.sheetnames, 'invalid_spec', '%s: sheet "%s" does not exist' % (path, sheet_name),
             path)
        sheet = book[sheet_name]
    return sheet, range_boundaries(match.group('r').replace('$', ''))  # (min_col, min_row, max_col, max_row)


def xlsx_cell_value(job, cell, value, path, column_formats=None):
    """Typed write: strings stay text (never formulas) unless type is "formula"."""
    style, kind, number_format = None, None, None
    if isinstance(value, dict):
        as_dict(value, path, {'value', 'type', 'number_format', 'bold', 'italic', 'color', 'fill', 'align', 'wrap',
                              'border', 'size'})
        kind = opt(value, 'type', path, as_str, 16, ('text', 'number', 'boolean', 'formula', 'date', 'datetime',
                                                     'time', 'percent', 'empty'))
        number_format = opt(value, 'number_format', path, as_str, 200)
        style = {k: v for k, v in value.items() if k in ('bold', 'italic', 'color', 'fill', 'align', 'wrap',
                                                          'border', 'size')}
        value = value.get('value')
    if kind == 'formula':
        cell.value = check_formula(value, path + '.value')
    elif kind in ('date', 'datetime', 'time'):
        text = as_str(value, path + '.value', 64)
        try:
            parsed = (datetime.date.fromisoformat(text) if kind == 'date' else
                      datetime.time.fromisoformat(text) if kind == 'time' else datetime.datetime.fromisoformat(text))
        except ValueError:
            fail('invalid_spec', '%s: "%s" is not an ISO %s (e.g. 2026-10-18)' % (path, text, kind), path)
        if getattr(parsed, 'tzinfo', None) is not None:
            parsed = parsed.replace(tzinfo=None)
        cell.value = parsed
        number_format = number_format or {'date': 'yyyy-mm-dd', 'datetime': 'yyyy-mm-dd hh:mm',
                                          'time': 'hh:mm:ss'}[kind]
    elif kind in ('number', 'percent'):
        if isinstance(value, str):
            try:
                value = int(value) if re.fullmatch(r'-?\d+', value.strip()) else float(value.strip().replace(',', '.'))
            except ValueError:
                fail('invalid_spec', '%s: "%s" is not a number' % (path, value), path)
        cell.value = as_num(value, path + '.value')
        if kind == 'percent':
            number_format = number_format or '0%'
    elif kind == 'boolean':
        cell.value = as_bool(value, path + '.value')
    elif kind == 'empty' or value is None:
        cell.value = None
    elif isinstance(value, str) or kind == 'text':
        cell.value = job.text(value if isinstance(value, str) else json.dumps(value), path, 32_767)
        cell.data_type = 's'
    elif isinstance(value, bool):
        cell.value = value
    elif isinstance(value, (int, float)):
        cell.value = as_num(value, path)
    else:
        fail('invalid_spec', '%s must be a string, number, boolean, null or {value, type}' % path, path)
    if number_format is None and column_formats:
        number_format = column_formats.get(cell.column_letter)
    if number_format:
        cell.number_format = number_format
    if style:
        xlsx_apply_style([cell], style, path)


def xlsx_apply_style(cells, style, path):
    from openpyxl.styles import Alignment, Border, Font, PatternFill, Side
    as_dict(style, path, {'range', 'bold', 'italic', 'underline', 'size', 'color', 'font', 'fill', 'align', 'valign',
                          'wrap', 'border', 'number_format'})
    font = {}
    for key in ('bold', 'italic'):
        if style.get(key) is not None:
            font[key] = as_bool(style[key], '%s.%s' % (path, key))
    if style.get('underline') is not None:
        font['underline'] = 'single' if as_bool(style['underline'], path + '.underline') else None
    if style.get('size') is not None:
        font['size'] = as_num(style['size'], path + '.size', 4, 409)
    if style.get('color') is not None:
        font['color'] = 'FF' + hex_color(style['color'], path + '.color')
    if style.get('font') is not None:
        font['name'] = as_str(style['font'], path + '.font', 64, pattern=r'[\w .\-]+')
    fill = None
    if style.get('fill') is not None:
        color = 'FF' + hex_color(style['fill'], path + '.fill')
        fill = PatternFill(fill_type='solid', fgColor=color, bgColor=color)
    align = {}
    if style.get('align') is not None:
        align['horizontal'] = as_str(style['align'], path + '.align', 16, ('left', 'center', 'right', 'justify'))
    if style.get('valign') is not None:
        align['vertical'] = as_str(style['valign'], path + '.valign', 16, ('top', 'center', 'bottom'))
    if style.get('wrap') is not None:
        align['wrap_text'] = as_bool(style['wrap'], path + '.wrap')
    border = None
    if style.get('border') is not None:
        kind = as_str(style['border'], path + '.border', 16, ('thin', 'medium', 'thick', 'none'))
        side = Side(style=None if kind == 'none' else kind)
        border = Border(left=side, right=side, top=side, bottom=side)
    number_format = opt(style, 'number_format', path, as_str, 200)
    for cell in cells:
        if font:
            current = cell.font
            values = {'name': current.name, 'size': current.size, 'bold': current.bold, 'italic': current.italic,
                      'underline': current.underline, 'color': current.color}
            values.update(font)
            cell.font = Font(**values)
        if fill is not None:
            cell.fill = fill
        if align:
            current = cell.alignment
            values = {'horizontal': current.horizontal, 'vertical': current.vertical, 'wrap_text': current.wrap_text}
            values.update(align)
            cell.alignment = Alignment(**values)
        if border is not None:
            cell.border = border
        if number_format:
            cell.number_format = number_format


def xlsx_range_cells(sheet, bounds, path):
    min_col, min_row, max_col, max_row = bounds
    need((max_col - min_col + 1) * (max_row - min_row + 1) <= MAX_STYLE_CELLS, 'limit',
         '%s covers more than %d cells' % (path, MAX_STYLE_CELLS), path)
    return [cell for row in sheet.iter_rows(min_row=min_row, max_row=max_row, min_col=min_col, max_col=max_col)
            for cell in row]


def xlsx_sheet_name(value, path, book, exclude=None):
    name = as_str(value, path, 31)
    need(name.strip() and not _SHEET_NAME_BAD.search(name) and not name.startswith("'") and not name.endswith("'"),
         'invalid_spec', '%s: sheet names cannot contain [ ] : * ? / \\ and are at most 31 characters' % path, path)
    need(name.lower() not in [n.lower() for n in book.sheetnames if n != exclude], 'invalid_spec',
         '%s: sheet "%s" already exists' % (path, name), path)
    return name


def xlsx_column_formats(spec, path):
    formats = {}
    for column, fmt in as_dict(spec, path).items():
        need(_COLUMN.match(column) is not None, 'invalid_spec', '%s keys are column letters like "B"' % path, path)
        formats[column] = as_str(fmt, '%s.%s' % (path, column), 200)
    return formats


def xlsx_fill_sheet(job, book, sheet, spec, path, counter):
    from openpyxl.utils import get_column_letter
    from openpyxl.utils.cell import column_index_from_string, coordinate_from_string
    as_dict(spec, path, {'name', 'rows', 'start', 'cells', 'header', 'widths', 'number_formats', 'freeze',
                         'autofilter', 'styles', 'merge', 'charts', 'table'})
    column_formats = xlsx_column_formats(spec['number_formats'], path + '.number_formats') \
        if spec.get('number_formats') is not None else {}
    start = opt(spec, 'start', path, as_str, 12, pattern=_CELL, default='A1')
    letters, start_row = coordinate_from_string(start)
    start_col = column_index_from_string(letters)
    rows = opt(spec, 'rows', path, as_list, 1_048_576, default=[])
    width = 0
    for r, row in enumerate(rows):
        rpath = '%s.rows[%d]' % (path, r)
        as_list(row, rpath, 16_384)
        counter[0] += len(row)
        need(counter[0] <= MAX_WRITE_CELLS, 'limit', 'more than %d cells in one request' % MAX_WRITE_CELLS, rpath)
        width = max(width, len(row))
        for c, value in enumerate(row):
            if value is not None:
                xlsx_cell_value(job, sheet.cell(row=start_row + r, column=start_col + c), value,
                                '%s[%d]' % (rpath, c), column_formats)
    if rows and opt(spec, 'header', path, as_bool, default=False):
        from openpyxl.styles import Border, Font, PatternFill, Side
        for c in range(width):
            cell = sheet.cell(row=start_row, column=start_col + c)
            cell.font = Font(bold=True)
            cell.fill = PatternFill(fill_type='solid', fgColor='FFD9E1F2', bgColor='FFD9E1F2')
            cell.border = Border(bottom=Side(style='thin'))
    for index, item in enumerate(opt(spec, 'cells', path, as_list, MAX_WRITE_CELLS, default=[])):
        cpath = '%s.cells[%d]' % (path, index)
        as_dict(item, cpath, required=('cell',))
        coordinate = as_str(item['cell'], cpath + '.cell', 12, pattern=_CELL)
        counter[0] += 1
        need(counter[0] <= MAX_WRITE_CELLS, 'limit', 'too many cells', cpath)
        xlsx_cell_value(job, sheet[coordinate], {k: v for k, v in item.items() if k != 'cell'}, cpath,
                        column_formats)
    if spec.get('widths') is not None:
        for column, value in as_dict(spec['widths'], path + '.widths').items():
            need(_COLUMN.match(column) is not None, 'invalid_spec',
                 '%s.widths keys are column letters like "A"' % path, path + '.widths')
            sheet.column_dimensions[column].width = as_num(value, '%s.widths.%s' % (path, column), 0.5, 255)
    elif rows:
        for c in range(min(width, 200)):
            longest = 0
            for row in rows[:500]:
                if c < len(row) and row[c] is not None:
                    value = row[c].get('value') if isinstance(row[c], dict) else row[c]
                    longest = max(longest, len(str(xlsx_value(value))))
            sheet.column_dimensions[get_column_letter(start_col + c)].width = max(8, min(60, longest + 2))
    for index, style in enumerate(opt(spec, 'styles', path, as_list, 1000, default=[])):
        spath = '%s.styles[%d]' % (path, index)
        as_dict(style, spath, required=('range',))
        target, bounds = parse_range(style['range'], spath + '.range', book, sheet)
        xlsx_apply_style(xlsx_range_cells(target, bounds, spath + '.range'),
                         {k: v for k, v in style.items() if k != 'range'}, spath)
    for index, merged in enumerate(opt(spec, 'merge', path, as_list, 1000, default=[])):
        target, bounds = parse_range(merged, '%s.merge[%d]' % (path, index), book, sheet)
        target.merge_cells(start_row=bounds[1], start_column=bounds[0], end_row=bounds[3], end_column=bounds[2])
    if spec.get('freeze') is not None:
        sheet.freeze_panes = as_str(spec['freeze'], path + '.freeze', 12, pattern=_CELL)
    if spec.get('autofilter') is not None and spec['autofilter'] is not False:
        if spec['autofilter'] is True:
            sheet.auto_filter.ref = sheet.dimensions
        else:
            parse_range(spec['autofilter'], path + '.autofilter', book, sheet)
            sheet.auto_filter.ref = spec['autofilter'].split('!')[-1].replace('$', '').upper()
    if spec.get('table') is not None:
        from openpyxl.worksheet.table import Table, TableStyleInfo
        table = as_dict(spec['table'], path + '.table', {'name', 'range', 'style'}, ('name',))
        name = as_str(table['name'], path + '.table.name', 64, pattern=r'[A-Za-z_][A-Za-z0-9_.]{0,63}')
        ref = table.get('range') or sheet.dimensions
        parse_range(ref, path + '.table.range', book, sheet)
        item = Table(displayName=name, ref=ref.split('!')[-1].replace('$', '').upper())
        item.tableStyleInfo = TableStyleInfo(name=opt(table, 'style', path + '.table', as_str, 64,
                                                      pattern=r'TableStyle(Light|Medium|Dark)\d{1,2}',
                                                      default='TableStyleMedium2'), showRowStripes=True)
        sheet.add_table(item)
    for index, chart in enumerate(opt(spec, 'charts', path, as_list, 50, default=[])):
        xlsx_chart(job, book, sheet, chart, '%s.charts[%d]' % (path, index))


def xlsx_chart(job, book, sheet, spec, path):
    from openpyxl.chart import AreaChart, BarChart, DoughnutChart, LineChart, PieChart, Reference, ScatterChart, Series
    from openpyxl.utils import get_column_letter
    as_dict(spec, path, {'type', 'title', 'data', 'categories', 'anchor', 'x_title', 'y_title', 'width_cm',
                         'height_cm', 'titles_from_data', 'legend'}, ('type', 'data'))
    kind = as_str(spec['type'], path + '.type', 20, CHART_TYPES_XLSX)
    data_sheet, data = parse_range(spec['data'], path + '.data', book, sheet)
    titles = opt(spec, 'titles_from_data', path, as_bool, default=True)
    if kind in ('column', 'bar', 'stacked_column', 'stacked_bar'):
        chart = BarChart()
        chart.type = 'bar' if 'bar' in kind else 'col'
        if kind.startswith('stacked'):
            chart.grouping = 'stacked'
            chart.overlap = 100
    else:
        chart = {'line': LineChart, 'pie': PieChart, 'doughnut': DoughnutChart, 'area': AreaChart,
                 'scatter': ScatterChart}[kind]()
    if kind == 'scatter':
        need(spec.get('categories'), 'invalid_spec', '%s.categories (x values) is required for scatter' % path,
             path + '.categories')
        x_sheet, x_bounds = parse_range(spec['categories'], path + '.categories', book, sheet)
        xvalues = Reference(x_sheet, min_col=x_bounds[0], min_row=x_bounds[1], max_col=x_bounds[2],
                            max_row=x_bounds[3])
        for column in range(data[0], data[2] + 1):
            series = Series(Reference(data_sheet, min_col=column, min_row=data[1], max_row=data[3]), xvalues,
                            title_from_data=titles)
            series.marker.symbol = 'circle'
            chart.series.append(series)
    else:
        chart.add_data(Reference(data_sheet, min_col=data[0], min_row=data[1], max_col=data[2], max_row=data[3]),
                       titles_from_data=titles)
        if spec.get('categories'):
            c_sheet, c_bounds = parse_range(spec['categories'], path + '.categories', book, sheet)
            chart.set_categories(Reference(c_sheet, min_col=c_bounds[0], min_row=c_bounds[1], max_col=c_bounds[2],
                                           max_row=c_bounds[3]))
    if spec.get('title'):
        chart.title = job.text(spec['title'], path + '.title', 255)
    if kind not in ('pie', 'doughnut'):
        if spec.get('x_title'):
            chart.x_axis.title = job.text(spec['x_title'], path + '.x_title', 255)
        if spec.get('y_title'):
            chart.y_axis.title = job.text(spec['y_title'], path + '.y_title', 255)
    if spec.get('legend') is False:
        chart.legend = None
    chart.width = opt(spec, 'width_cm', path, as_num, 3, 60, default=16)
    chart.height = opt(spec, 'height_cm', path, as_num, 3, 60, default=8)
    anchor = opt(spec, 'anchor', path, as_str, 12, pattern=_CELL)
    sheet.add_chart(chart, anchor or '%s2' % get_column_letter(min(16_000, (sheet.max_column or 1) + 2)))


def xlsx_properties(job, book, spec, fresh):
    props = book.properties
    values = spec.get('properties') or {}
    as_dict(values, 'spec.properties', {'title', 'author', 'subject', 'keywords', 'description', 'category'})
    now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None, microsecond=0)
    if fresh:
        props.creator = ''
        props.lastModifiedBy = None
        props.created = now
    props.modified = now
    for key, attr in (('title', 'title'), ('author', 'creator'), ('subject', 'subject'), ('keywords', 'keywords'),
                      ('description', 'description'), ('category', 'category')):
        if values.get(key) is not None:
            setattr(props, attr, job.text(values[key], 'spec.properties.' + key, 1000))


def xlsx_create(job, spec):
    import openpyxl
    as_dict(spec, 'spec', {'format', 'sheets', 'properties', 'template_input', 'values'}, ('sheets',))
    template = spec.get('template_input')
    if template is not None:
        as_int(template, 'spec.template_input', 0)
        need(job.kind(template, 'spec.template_input') == 'xlsx', 'invalid_spec',
             'spec.template_input must be xlsx/xltx', 'spec.template_input')
        need(ext_of(job.input(template)['name']) not in MACRO_EXTENSIONS, 'unsupported',
             'macro-enabled templates are not supported', 'spec.template_input')
        book = xlsx_open(job, template, keep_links=True)
        protected = xlsx_roundtrip_source(job, job.data(template))
        book.template = False
    else:
        book = openpyxl.Workbook()
    counter = [0]
    default = book.active if template is None else None
    for index, sheet_spec in enumerate(as_list(spec['sheets'], 'spec.sheets', 200, 1 if template is None else 0)):
        path = 'spec.sheets[%d]' % index
        as_dict(sheet_spec, path)
        name = sheet_spec.get('name') or 'Sheet%d' % (index + 1)
        if template is not None and name in book.sheetnames:
            sheet = book[name]
        elif default is not None:
            default.title = xlsx_sheet_name(name, path + '.name', book, default.title)
            sheet, default = default, None
        else:
            sheet = book.create_sheet(xlsx_sheet_name(name, path + '.name', book))
        xlsx_fill_sheet(job, book, sheet, sheet_spec, path, counter)
    xlsx_properties(job, book, spec, template is None)
    result = {'sheets': len(book.sheetnames), 'cells': counter[0]}
    if spec.get('values') is not None:
        result.update(xlsx_fill(book, values_of(job, spec['values'], 'spec.values')))
    data = save_xlsx(job, book)
    if template is not None:
        xlsx_check_roundtrip(job, protected, data)
    return data, result


def save_xlsx(job, book):
    buffer = io.BytesIO()
    book.save(buffer)
    book.close()
    data = buffer.getvalue()
    validate_package(job, data, 'xlsx')
    if job.qa is None:
        job.qa = {}
    job.qa['calculation'] = xlsx_calculation(data)
    if job.qa['calculation']['formula_cells']:
        job.warn('Formula values have not been calculated; use an explicit LibreOffice recalculation '
                 'before relying on result values', 'calculation')
    return data


def xlsx_fill(book, values):
    filled, unfilled = [0], set()

    def replace(match):
        if match.group(1) in values:
            filled[0] += 1
            return values[match.group(1)]
        unfilled.add(match.group(1))
        return match.group(0)
    for sheet in book.worksheets:
        for row in sheet.iter_rows():
            for cell in row:
                if isinstance(cell.value, str) and '{{' in cell.value and cell.data_type == 's':
                    cell.value = PLACEHOLDER.sub(replace, cell.value)
                    cell.data_type = 's'
    return {'filled': filled[0], 'unfilled': sorted(unfilled)[:100]}


def xlsx_last_row(sheet):
    for row in range(sheet.max_row or 0, max(0, (sheet.max_row or 0) - 100_000), -1):
        if any(cell.value is not None for cell in sheet[row]):
            return row
    return 0


def xlsx_reference_parts(value, path):
    """Parse A1 cells, full rows and full columns; never guess at other syntax."""
    from openpyxl.utils.cell import column_index_from_string
    parts = []
    for item in value.split(':'):
        match = re.fullmatch(r'(\$?)([A-Za-z]{1,3})(\$?)([1-9][0-9]{0,6})', item)
        if match:
            column = column_index_from_string(match[2].upper())
            row = int(match[4])
            need(column <= 16384 and row <= 1048576, 'unsupported',
                 'Formula reference is outside the worksheet: ' + value, path)
            parts.append(['cell', column, row, match[1], match[3]])
            continue
        match = re.fullmatch(r'(\$?)([A-Za-z]{1,3})', item)
        if match and ':' in value:
            column = column_index_from_string(match[2].upper())
            need(column <= 16384, 'unsupported', 'Column reference is outside the worksheet', path)
            parts.append(['column', column, None, match[1], ''])
            continue
        match = re.fullmatch(r'(\$?)([1-9][0-9]{0,6})', item)
        if match and ':' in value:
            row = int(match[2])
            need(row <= 1048576, 'unsupported', 'Row reference is outside the worksheet', path)
            parts.append(['row', None, row, '', match[1]])
            continue
        return None
    if not 1 <= len(parts) <= 2 or any(part[0] != parts[0][0] for part in parts):
        return None
    return parts


def xlsx_shift_interval(first, last, index, count, deleting):
    reverse = first > last
    low, high = sorted((first, last))
    if deleting:
        end = index + count
        if high < index:
            pass
        elif low >= end:
            low, high = low - count, high - count
        elif low >= index and high < end:
            return None
        else:
            low = low if low < index else index
            high = high - count if high >= end else index - 1
    else:
        low += count if low >= index else 0
        high += count if high >= index else 0
    return (high, low) if reverse else (low, high)


def xlsx_rewrite_formula(formula, owner, target, operation, index, count, new_name, names, path):
    from openpyxl.formula import Tokenizer
    from openpyxl.utils.cell import get_column_letter
    need(isinstance(formula, str), 'unsupported',
         'Array/data-table formulas need a spreadsheet application for structural editing', path)
    try:
        tokens = Tokenizer(formula if formula.startswith('=') else '=' + formula).items
    except Exception:
        fail('unsupported', 'This formula syntax cannot be structurally edited: ' + formula[:120], path)
    result = []
    for token in tokens:
        value = token.value
        if token.type != 'OPERAND' or token.subtype != 'RANGE':
            result.append(value)
            continue
        qualified = '!' in value
        prefix, address = value.rsplit('!', 1) if qualified else ('', value)
        sheet = prefix[1:-1].replace("''", "'") if prefix.startswith("'") and prefix.endswith("'") else prefix
        need(not any(char in sheet for char in '[]:'), 'unsupported',
             'External and 3D formula references need a spreadsheet application for structural editing', path)
        parts = xlsx_reference_parts(address, path)
        if parts is None:
            # Named references remain valid because their definitions are rewritten too.
            need(not qualified and address.casefold() in names, 'unsupported',
                 'Unsupported structured, named or spill reference: ' + value[:120], path)
            result.append(value)
            continue
        need(qualified or owner is not None, 'unsupported',
             'A global defined name with a relative reference cannot be structurally edited', path)
        referenced = sheet if qualified else owner
        if referenced.casefold() != target.casefold():
            result.append(value)
            continue
        if operation == 'delete_sheet':
            result.append('#REF!')
            continue
        if operation == 'rename_sheet':
            result.append("'" + new_name.replace("'", "''") + "'!" + address if qualified else value)
            continue
        axis = 2 if operation.endswith('_rows') else 1
        if parts[0][axis] is None:
            result.append(value)
            continue
        shifted = xlsx_shift_interval(parts[0][axis], parts[-1][axis], index, count,
                                      operation.startswith('delete_'))
        if shifted is None:
            result.append('#REF!')
            continue
        bound = 1048576 if axis == 2 else 16384
        need(all(1 <= coordinate <= bound for coordinate in shifted), 'unsupported',
             'Structural editing would move a formula reference outside the worksheet', path)
        parts[0][axis], parts[-1][axis] = shifted
        converted = []
        for kind, column, row, absolute_column, absolute_row in parts:
            converted.append((absolute_column + get_column_letter(column) if column is not None else '') +
                             (absolute_row + str(row) if row is not None else ''))
        result.append((prefix + '!' if qualified else '') + ':'.join(converted))
    return '=' + ''.join(result)


def xlsx_structure_change(book, sheet, operation, path, index=None, count=None, new_name=None):
    """Rewrite the supported reference language or reject before publishing any output."""
    shifting = operation in ('insert_rows', 'delete_rows', 'insert_cols', 'delete_cols')
    for current in book.worksheets:
        need(not current._charts and (not shifting or not current._images), 'unsupported',
             'Structural editing with charts or anchored images needs a spreadsheet application', path)
        need(not current.tables and not current.data_validations.count and not len(current.conditional_formatting),
             'unsupported', 'Structural editing with tables, validation or conditional-format formulas is not supported', path)
        for cell in current._cells.values():
            link = cell.hyperlink
            need(not link or not (link.location or str(link.target or '').startswith('#') or
                 (shifting and current is sheet)), 'unsupported',
                 'Structural editing with internal or moved hyperlinks needs a spreadsheet application', path)
    need(not book._external_links, 'unsupported', 'Structural editing with external workbook links is not supported', path)
    if shifting:
        need(not sheet.tables and not sheet.merged_cells.ranges and not sheet.freeze_panes and
             not sheet.auto_filter.ref and not sheet.print_area and not sheet.print_title_rows and
             not sheet.print_title_cols, 'unsupported',
             'Row/column changes with tables, merged cells, panes, filters or print ranges need a '
             'spreadsheet application', path)
        bound = 1048576 if operation.endswith('_rows') else 16384
        need(index + count - 1 <= bound, 'invalid_spec', 'The structural range exceeds the worksheet', path)
        extent = sheet.max_row if operation.endswith('_rows') else sheet.max_column
        move_cells = max(0, extent - index + 1) * (sheet.max_column if operation.endswith('_rows') else sheet.max_row)
        need(move_cells <= MAX_SHEET_CELLS, 'limit',
             'The sparse worksheet would expand beyond the structural-editing cell limit', path)
        need(not operation.startswith('insert_') or extent < index or extent + count <= bound,
             'unsupported', 'Inserting would move existing cells outside the worksheet', path)
    definitions = [(name, None) for name in book.defined_names.values()]
    for current in book.worksheets:
        definitions.extend((name, current.title) for name in current.defined_names.values())
    names = {name.name.casefold() for name, _ in definitions}
    changes, formulas, defined_names = [], 0, 0
    for current in book.worksheets:
        for cell in list(current._cells.values()):
            if cell.data_type != 'f':
                continue
            value = xlsx_rewrite_formula(cell.value, current.title, sheet.title, operation,
                                         index, count, new_name, names, path)
            if value != cell.value:
                changes.append((cell, 'value', value))
                formulas += 1
    for name, owner in definitions:
        if not isinstance(name.attr_text, str):
            continue
        value = xlsx_rewrite_formula(name.attr_text, owner, sheet.title, operation,
                                     index, count, new_name, names, path)[1:]
        if value != name.attr_text:
            changes.append((name, 'attr_text', value))
            defined_names += 1
    for item, attribute, value in changes:
        setattr(item, attribute, value)
    if shifting:
        # openpyxl moves cells but not row/column dimensions. Move those explicitly.
        from copy import copy
        from openpyxl.utils.cell import column_index_from_string, get_column_letter
        dimensions = sheet.row_dimensions if operation.endswith('_rows') else sheet.column_dimensions
        moved = []
        for key, dimension in dimensions.items():
            first = int(key) if operation.endswith('_rows') else dimension.min or column_index_from_string(key)
            last = first if operation.endswith('_rows') else dimension.max or first
            shifted = xlsx_shift_interval(first, last, index, count, operation.startswith('delete_'))
            if shifted is None:
                continue
            need(all(1 <= coordinate <= bound for coordinate in shifted), 'unsupported',
                 'Structural editing would move a row/column dimension outside the worksheet', path)
            updated = copy(dimension)
            if operation.endswith('_rows'):
                updated.index = shifted[0]
                moved.append((shifted[0], updated))
            else:
                updated.index = get_column_letter(shifted[0])
                updated.min, updated.max = shifted
                moved.append((updated.index, updated))
        dimensions.clear()
        for key, dimension in moved:
            dimensions[key] = dimension
    return formulas, defined_names


def xlsx_roundtrip_source(job, data):
    from lxml import etree
    protected = {}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for name in archive.namelist():
            lower = name.lower()
            need(not lower.startswith(('xl/activex/', 'xl/ctrlprops/', 'xl/embeddings/', 'xl/pivottables/',
                 'xl/pivotcache/', 'xl/slicers/', 'xl/timelines/', 'xl/chartsheets/', '_xmlsignatures/', 'customxml/'))
                 and not lower.startswith('xl/vbaproject') and lower != 'xl/connections.xml', 'unsupported',
                 'This workbook contains a part the editor cannot safely preserve: ' + name, 'inputs[0]')
            if lower.endswith(('.xml', '.vml')):
                root = etree.fromstring(archive.read(name), parser=safe_xml_parser())
                for element in root.iter():
                    if not isinstance(element.tag, str):
                        continue
                    local = etree.QName(element).localname
                    need(local not in ('extLst', 'AlternateContent') and
                         not (name.startswith('xl/drawings/') and local in ('sp', 'grpSp', 'cxnSp', 'contentPart')),
                         'unsupported', 'This workbook contains extensions or drawing shapes that cannot be '
                         'safely preserved by the editor', 'inputs[0]')
            if name.startswith(('xl/charts/', 'xl/drawings/', 'xl/media/', 'xl/externalLinks/')):
                protected[name] = archive.read(name)
    return protected


def xlsx_check_roundtrip(job, protected, data):
    from lxml import etree
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        names = set(archive.namelist())
        for name, original in protected.items():
            need(name in names, 'unsupported',
                 'Editing would remove an existing workbook part: ' + name, 'inputs[0]')
            if name.startswith('xl/media/'):
                need(archive.read(name) == original, 'unsupported',
                     'Editing would change an existing embedded image', 'inputs[0]')
            elif name.startswith('xl/drawings/') and name.endswith('.xml'):
                def shapes(content):
                    root = etree.fromstring(content, parser=safe_xml_parser())
                    return sorted(etree.QName(element).localname for element in root.iter()
                                  if isinstance(element.tag, str) and etree.QName(element).localname in ('pic', 'graphicFrame', 'sp', 'grpSp', 'cxnSp'))
                need(shapes(original) == shapes(archive.read(name)), 'unsupported',
                     'Editing would change the set of existing drawing objects', 'inputs[0]')
    if job.qa is None:
        job.qa = {}
    job.qa['roundtrip'] = {'protected_parts_checked': len(protected), 'missing_parts': 0}


def xlsx_table_value(job, value, column, path):
    """A data-only column schema: callers cannot supply formulas or cell instructions."""
    kind = column['kind']
    if value is None:
        need(column['nullable'], 'invalid_spec', 'This column does not accept null', path)
        return None, 'n', 'General'
    number_format = 'General'
    if kind == 'text':
        value = as_str(value, path, 32767)
        need(bool(value), 'invalid_spec', 'Use explicit null instead of an empty string', path)
        need(job.text(value, path, 32767) == value, 'invalid_spec', 'Text contains unsupported characters', path)
        return value, 's', number_format
    if kind == 'boolean':
        return as_bool(value, path), 'b', number_format
    if kind == 'date':
        text = as_str(value, path, 10, pattern=r'\d{4}-\d{2}-\d{2}')
        try:
            value = datetime.date.fromisoformat(text)
        except ValueError:
            fail('invalid_spec', 'A real ISO date is required', path)
        need(value >= datetime.date(1900, 1, 1), 'invalid_spec', 'Excel dates must be 1900 or later', path)
        return value, 'd', 'yyyy-mm-dd'
    if kind in ('integer', 'money_minor'):
        bound = 9_999_999_999_999 if kind == 'money_minor' else 999_999_999_999_999
        value = as_int(value, path, -bound, bound)
        if kind == 'money_minor':
            # Keep the receipt/input in integer minor units, check cents again after saving.
            value = float(decimal.Decimal(value) / 100)
            number_format = '#,##0.00'
    else:
        value = as_num(value, path)
    return value, 'n', number_format


def xlsx_table_key(cell, column, path):
    """Compare composite keys by declared type; formulas and blank keys are ambiguous."""
    value = cell.value
    kind = column['kind']
    need(value is not None and cell.data_type != 'f', 'invalid_spec',
         'A table key must be present and cannot be a formula', path)
    if kind == 'text':
        need(isinstance(value, str) and bool(value.strip()), 'invalid_spec', 'A text key is required', path)
        return ('text', value)
    if kind == 'boolean':
        return ('boolean', as_bool(value, path))
    if kind == 'date':
        if isinstance(value, datetime.datetime):
            need(value.time() == datetime.time(), 'invalid_spec', 'A date key cannot include a time', path)
            value = value.date()
        need(isinstance(value, datetime.date), 'invalid_spec', 'A typed date key is required', path)
        return ('date', value.isoformat())
    value = as_num(value, path)
    number = decimal.Decimal(str(value))
    if kind == 'integer':
        need(number == number.to_integral_value(), 'invalid_spec', 'An integer key is required', path)
    if kind == 'money_minor':
        need(number * 100 == (number * 100).to_integral_value(), 'invalid_spec',
             'A money key must have whole minor units', path)
    return ('number', number)


def xlsx_append_table_rows(job, book, patch, path):
    from openpyxl.cell.cell import Cell
    from openpyxl.utils.cell import get_column_letter, range_boundaries
    as_dict(patch, path, {'op', 'sheet', 'table', 'columns', 'key_columns', 'rows'},
            ('sheet', 'table', 'columns', 'key_columns', 'rows'))
    sheet_name = as_str(patch['sheet'], path + '.sheet', 31)
    name = as_str(patch['table'], path + '.table', 255)
    need(sheet_name in book.sheetnames, 'invalid_spec', 'The selected sheet does not exist', path + '.sheet')
    sheet = book[sheet_name]
    need(name in sheet.tables, 'invalid_spec', 'The named Excel table does not exist', path + '.table')
    table = sheet.tables[name]
    need(not sheet.protection.sheet, 'unsupported', 'Protected sheets cannot be appended', path + '.sheet')
    need(table.headerRowCount == 1 and not table.totalsRowCount and not table.totalsRowShown,
         'unsupported', 'The table must have one header row and no totals row', path + '.table')
    need(not any(c.calculatedColumnFormula is not None or c.totalsRowFormula is not None
                 for c in table.tableColumns), 'unsupported',
         'Calculated table columns require a separate formula-aware operation', path + '.table')
    columns = as_list(patch['columns'], path + '.columns', 32, 1)
    for i, column in enumerate(columns):
        cpath = '%s.columns[%d]' % (path, i)
        as_dict(column, cpath, {'header', 'kind', 'nullable'}, ('header', 'kind', 'nullable'))
        as_str(column['header'], cpath + '.header', 255)
        as_str(column['kind'], cpath + '.kind', 20, ('text', 'integer', 'number', 'boolean', 'date', 'money_minor'))
        as_bool(column['nullable'], cpath + '.nullable')
    headers = [c['header'] for c in columns]
    need(all(headers) and len(set(h.casefold() for h in headers)) == len(headers), 'invalid_spec',
         'Column headers must be nonempty and unique', path + '.columns')
    left, top, right, bottom = range_boundaries(table.ref)
    need(right - left + 1 == len(columns) and bottom >= top and bottom - top <= 100_000
         and (bottom - top + 1) * len(columns) <= MAX_SHEET_CELLS, 'limit',
         'The table dimensions do not match the bounded schema', path + '.columns')
    need(headers == [c.name for c in table.tableColumns]
         and headers == [sheet.cell(top, col).value for col in range(left, right + 1)],
         'invalid_spec', 'Table headers changed; review the column mapping', path + '.columns')
    keys = as_list(patch['key_columns'], path + '.key_columns', 4, 1)
    for i, key in enumerate(keys):
        as_int(key, '%s.key_columns[%d]' % (path, i), 0, len(columns) - 1)
    need(len(set(keys)) == len(keys), 'invalid_spec', 'Key columns must be unique', path + '.key_columns')
    rows = as_list(patch['rows'], path + '.rows', 100, 1)
    prepared = []
    for i, row in enumerate(rows):
        rpath = '%s.rows[%d]' % (path, i)
        as_list(row, rpath, len(columns), len(columns))
        cells = []
        for j, (value, column) in enumerate(zip(row, columns)):
            cell = Cell(sheet, row=1, column=j + 1)
            cell.value, cell.data_type, cell.number_format = xlsx_table_value(job, value, column, '%s[%d]' % (rpath, j))
            cells.append(cell)
        prepared.append(cells)
    seen, last, blank = set(), top, False
    for row in sheet.iter_rows(min_row=top + 1, max_row=bottom, min_col=left, max_col=right):
        if all(cell.value is None for cell in row):
            blank = True
            continue
        need(not blank, 'invalid_spec', 'An internal empty table row requires review', path + '.table')
        key = tuple(xlsx_table_key(row[k], columns[k], path + '.table') for k in keys)
        need(key not in seen, 'duplicate_key', 'The existing table already contains a duplicate key', path + '.table')
        seen.add(key)
        last = row[0].row
    for i, row in enumerate(prepared):
        key = tuple(xlsx_table_key(row[k], columns[k], '%s.rows[%d]' % (path, i)) for k in keys)
        need(key not in seen, 'duplicate_key', 'A row key already exists in the table or this request',
             '%s.rows[%d]' % (path, i))
        seen.add(key)
    first, end = last + 1, last + len(rows)
    need(end <= 1_048_576 and (max(bottom, end) - top + 1) * len(columns) <= MAX_SHEET_CELLS,
         'limit', 'The expanded table is too large', path + '.rows')
    expanded = (left, top, right, max(bottom, end))
    def overlaps(bounds):
        a, b, c, d = bounds
        return a <= expanded[2] and c >= expanded[0] and b <= expanded[3] and d >= expanded[1]
    need(not any(overlaps(merged.bounds) for merged in sheet.merged_cells.ranges), 'unsupported',
         'Merged cells overlap the table', path + '.table')
    need(not any(other.name != table.name and overlaps(range_boundaries(other.ref))
                 for other in sheet.tables.values()), 'conflict', 'Another table overlaps the appended rows', path + '.table')
    for row in sheet.iter_rows(min_row=first, max_row=end, min_col=left, max_col=right):
        need(all(c.value is None and c.comment is None and c.hyperlink is None for c in row), 'conflict',
             'Appending would replace existing cell content or annotations', path + '.rows')
    old_ref = table.ref
    new_ref = '%s%d:%s%d' % (get_column_letter(left), top, get_column_letter(right), max(bottom, end))
    need(table.autoFilter is None or table.autoFilter.ref == old_ref, 'unsupported',
         'A custom table filter range requires review', path + '.table')
    for row_index, row in enumerate(prepared, first):
        for col_index, source in enumerate(row, left):
            target = sheet.cell(row_index, col_index)
            target.value, target.data_type, target.number_format = source.value, source.data_type, source.number_format
    table.ref = new_ref
    if table.autoFilter is not None:
        table.autoFilter.ref = new_ref
    if sheet.auto_filter.ref == old_ref:
        sheet.auto_filter.ref = new_ref
    result = {'sheet': sheet_name, 'table': name, 'rows_added': len(rows), 'first_row': first, 'last_row': end,
              'previous_range': old_ref, 'range': new_ref, 'key_columns': keys}
    return result, (columns, rows, prepared, left)


def xlsx_check_table_append(data, result, expected):
    import openpyxl
    columns, rows, prepared, left = expected
    book = openpyxl.load_workbook(io.BytesIO(data), keep_links=True)
    try:
        sheet = book[result['sheet']]
        table = sheet.tables[result['table']]
        need(table.ref == result['range'] and (table.autoFilter is None or table.autoFilter.ref == result['range']),
             'verification_failed', 'Saved table range differs from the reviewed append')
        for i, cells in enumerate(prepared, result['first_row']):
            for j, cell in enumerate(cells):
                actual = sheet.cell(i, left + j)
                value = actual.value
                if isinstance(value, datetime.datetime) and columns[j]['kind'] == 'date':
                    value = value.date() if value.time() == datetime.time() else value
                same = value == cell.value and actual.data_type == cell.data_type
                if columns[j]['kind'] == 'money_minor' and value is not None:
                    same = actual.data_type == 'n' and decimal.Decimal(str(value)) * 100 == rows[i - result['first_row']][j]
                need(same and actual.number_format == cell.number_format, 'verification_failed',
                     'Saved table cell differs in value, type or format')
    finally:
        book.close()


def xlsx_edit(job, spec):
    as_dict(spec, 'spec', {'source_sha256', 'patches', 'properties'}, ('source_sha256', 'patches'))
    book = xlsx_open(job, 0, keep_links=True)
    protected = xlsx_roundtrip_source(job, job.data(0))
    book.template = False
    counter = [0]
    stats = {'patches_applied': 0, 'formulas_updated': 0, 'defined_names_updated': 0}
    table_check = None

    def references(sheet, operation, path, index=None, count=None, new_name=None):
        formulas, names = xlsx_structure_change(book, sheet, operation, path, index, count, new_name)
        stats['formulas_updated'] += formulas
        stats['defined_names_updated'] += names

    def sheet_of(value, path):
        name = as_str(value, path, 31)
        need(name in book.sheetnames, 'invalid_spec', '%s: sheet "%s" does not exist; sheets: %s' % (
            path, name, ', '.join(book.sheetnames)), path)
        return book[name]
    for index, patch in enumerate(as_list(spec['patches'], 'spec.patches', MAX_PATCHES, 1)):
        path = 'spec.patches[%d]' % index
        as_dict(patch, path)
        op = patch.get('op') or ('set_cell' if 'cell' in patch else None)
        need(isinstance(op, str), 'invalid_spec', '%s.op is required; use one of: %s' % (
            path, ', '.join(XLSX_EDIT_OPS)), path + '.op')
        if op == 'set_cell':
            as_dict(patch, path, {'op', 'sheet', 'cell', 'value', 'type', 'number_format', 'bold', 'italic', 'color',
                                  'fill', 'align', 'wrap', 'border', 'size'}, ('sheet', 'cell'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            coordinate = as_str(patch['cell'], path + '.cell', 12, pattern=_CELL)
            xlsx_cell_value(job, sheet[coordinate],
                            {k: v for k, v in patch.items() if k not in ('op', 'sheet', 'cell')}, path)
        elif op == 'append_table_rows':
            need(len(spec['patches']) == 1, 'invalid_spec',
                 'A typed table append must be the only patch in its document action', path)
            result, expected = xlsx_append_table_rows(job, book, patch, path)
            stats['table_append'] = result
            table_check = (result, expected)
        elif op in ('set_range', 'append_rows'):
            as_dict(patch, path, {'op', 'sheet', 'start', 'rows'} if op == 'set_range' else {'op', 'sheet', 'rows'},
                    ('sheet', 'rows'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            start = patch.get('start') or 'A1' if op == 'set_range' else 'A%d' % (xlsx_last_row(sheet) + 1)
            xlsx_fill_sheet(job, book, sheet, {'rows': patch['rows'], 'start': start, 'widths': {}}, path, counter)
        elif op == 'add_sheet':
            as_dict(patch, path, required=('name',))
            sheet = book.create_sheet(xlsx_sheet_name(patch['name'], path + '.name', book),
                                      opt(patch, 'position', path, as_int, 0, len(book.sheetnames)))
            xlsx_fill_sheet(job, book, sheet, {k: v for k, v in patch.items() if k not in ('op', 'position')}, path,
                            counter)
        elif op == 'delete_sheet':
            as_dict(patch, path, {'op', 'sheet'}, ('sheet',))
            need(len(book.sheetnames) > 1, 'invalid_spec', '%s: a workbook needs at least one sheet' % path, path)
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            references(sheet, op, path)
            book.remove(sheet)
        elif op == 'rename_sheet':
            as_dict(patch, path, {'op', 'sheet', 'name'}, ('sheet', 'name'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            name = xlsx_sheet_name(patch['name'], path + '.name', book, sheet.title)
            references(sheet, op, path, new_name=name)
            sheet.title = name
        elif op in ('insert_rows', 'delete_rows', 'insert_cols', 'delete_cols'):
            as_dict(patch, path, {'op', 'sheet', 'index', 'count'}, ('sheet', 'index'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            position = as_int(patch['index'], path + '.index', 1, 1048576 if op.endswith('_rows') else 16384)
            count = opt(patch, 'count', path, as_int, 1, 100000, default=1)
            references(sheet, op, path, position, count)
            getattr(sheet, op)(position, count)
        elif op == 'style':
            as_dict(patch, path, required=('sheet', 'range'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            target, bounds = parse_range(patch['range'], path + '.range', book, sheet)
            xlsx_apply_style(xlsx_range_cells(target, bounds, path + '.range'),
                             {k: v for k, v in patch.items() if k not in ('op', 'sheet', 'range')}, path)
        elif op in ('merge', 'unmerge'):
            as_dict(patch, path, {'op', 'sheet', 'range'}, ('sheet', 'range'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            target, bounds = parse_range(patch['range'], path + '.range', book, sheet)
            getattr(target, op + '_cells')(start_row=bounds[1], start_column=bounds[0], end_row=bounds[3],
                                           end_column=bounds[2])
        elif op == 'number_formats':
            as_dict(patch, path, {'op', 'sheet', 'number_formats'}, ('sheet', 'number_formats'))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            for column, fmt in xlsx_column_formats(patch['number_formats'], path + '.number_formats').items():
                for cell in sheet[column]:
                    if cell.value is not None:
                        cell.number_format = fmt
        elif op in ('freeze', 'autofilter', 'widths', 'add_chart', 'add_table'):
            as_dict(patch, path, required=('sheet',))
            sheet = sheet_of(patch['sheet'], path + '.sheet')
            rest = {k: v for k, v in patch.items() if k not in ('op', 'sheet')}
            if op == 'freeze':
                as_dict(patch, path, {'op', 'sheet', 'cell'}, ('cell',))
                section = {'freeze': patch['cell']}
            elif op == 'autofilter':
                as_dict(patch, path, {'op', 'sheet', 'range'})
                section = {'autofilter': patch.get('range', True)}
            elif op == 'widths':
                as_dict(patch, path, {'op', 'sheet', 'widths'}, ('widths',))
                section = {'widths': patch['widths']}
            elif op == 'add_chart':
                section = {'charts': [rest]}
            else:
                section = {'table': rest}
            xlsx_fill_sheet(job, book, sheet, section, path, counter)
        elif op == 'fill_placeholders':
            as_dict(patch, path, {'op', 'values'}, ('values',))
            stats.update(xlsx_fill(book, values_of(job, patch['values'], path + '.values')))
        else:
            fail('invalid_spec', '%s.op "%s" is unknown; use one of: %s' % (path, op, ', '.join(XLSX_EDIT_OPS)),
                 path + '.op')
        stats['patches_applied'] += 1
    xlsx_properties(job, book, spec, False)
    data = save_xlsx(job, book)
    xlsx_check_roundtrip(job, protected, data)
    if table_check is not None:
        xlsx_check_table_append(data, *table_check)
        job.qa['table_append'] = {'status': 'verified', 'rows_checked': table_check[0]['rows_added']}
    return data, stats


# ---------------------------------------------------------------------------------------------
# PPTX
# ---------------------------------------------------------------------------------------------

A_NS = 'http://schemas.openxmlformats.org/drawingml/2006/main'
A_P, A_T, A_R, A_RPR, A_PPR = ('{%s}%s' % (A_NS, tag) for tag in ('p', 't', 'r', 'rPr', 'pPr'))
LAYOUT_ALIASES = {'title': ('Title Slide', 'Titelfolie'), 'section': ('Section Header', 'Abschnittsüberschrift'),
                  'content': ('Title and Content', 'Titel und Inhalt'),
                  'two_content': ('Two Content', 'Zwei Inhalte'), 'comparison': ('Comparison', 'Vergleich'),
                  'title_only': ('Title Only', 'Nur Titel'), 'blank': ('Blank', 'Leer')}
CHART_TYPES_PPTX = ('column', 'stacked_column', 'bar', 'stacked_bar', 'line', 'pie', 'doughnut', 'area', 'scatter')
SLIDE_KEYS = {'layout', 'layout_name', 'title', 'subtitle', 'bullets', 'text', 'image', 'table', 'chart', 'left',
              'right', 'notes'}
PPTX_EDIT_OPS = ('set_text', 'add_slide', 'delete_slide', 'move_slide', 'set_notes', 'delete_shape', 'insert_image',
                 'add_table', 'add_chart', 'add_textbox', 'fill_placeholders', 'replace_text', 'set_properties')


def pptx_open(job, index, path='inputs'):
    import pptx
    item = job.input(index, path)
    data = check_ooxml(job, job.data(index, path), 'pptx', item['path'])
    try:
        return pptx.Presentation(io.BytesIO(data))
    except EngineError:
        raise
    except Exception as error:
        fail('corrupt_input', '%s cannot be opened as a presentation (%s)' % (item['path'], str(error)[:120]), path)


def pptx_shapes(shapes):
    from pptx.enum.shapes import MSO_SHAPE_TYPE
    for shape in shapes:
        yield shape
        if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
            yield from pptx_shapes(shape.shapes)


def pptx_frame_text(frame):
    # python-pptx joins the runs of a paragraph (QA bug: words were split at run boundaries).
    return '\n'.join(paragraph.text.replace('\v', '\n') for paragraph in frame.paragraphs)


def pptx_shape_text(shape):
    if shape.has_text_frame:
        return pptx_frame_text(shape.text_frame)
    if getattr(shape, 'has_table', False) and shape.has_table:
        return '\n'.join('\t'.join(pptx_frame_text(cell.text_frame).replace('\n', ' ') for cell in row.cells)
                         for row in shape.table.rows)
    return ''


def pptx_notes(slide):
    if slide.has_notes_slide and slide.notes_slide.notes_text_frame is not None:
        return pptx_frame_text(slide.notes_slide.notes_text_frame)
    return ''


def pptx_text(job, index):
    prs = pptx_open(job, index)
    parts = []
    for number, slide in enumerate(prs.slides, 1):  # prs.slides follows sldIdLst (QA bug: file order was used)
        lines = ['## Slide %d' % number]
        shapes = list(pptx_shapes(slide.shapes))
        title = slide.shapes.title
        if title is not None:
            shapes = [title] + [shape for shape in shapes if shape.shape_id != title.shape_id]
        for shape in shapes:
            text = pptx_shape_text(shape)
            if text.strip():
                lines.append(text)
        notes = pptx_notes(slide)
        if notes.strip():
            lines.append('Notes: ' + notes)
        parts.append('\n'.join(lines))
    return '\n\n'.join(parts)


def pptx_shape_kind(shape):
    from pptx.enum.shapes import MSO_SHAPE_TYPE
    if shape.is_placeholder:
        try:
            return 'placeholder:' + str(shape.placeholder_format.type).split('.')[-1].split(' ')[0].lower()
        except Exception:
            return 'placeholder'
    if getattr(shape, 'has_table', False) and shape.has_table:
        return 'table'
    if getattr(shape, 'has_chart', False) and shape.has_chart:
        return 'chart'
    if shape.shape_type == MSO_SHAPE_TYPE.PICTURE:
        return 'picture'
    if shape.shape_type == MSO_SHAPE_TYPE.GROUP:
        return 'group'
    return 'text' if shape.has_text_frame else 'shape'


def pptx_inspect(job, index):
    prs = pptx_open(job, index)
    items = []
    for number, slide in enumerate(prs.slides):
        shapes = []
        for shape in list(pptx_shapes(slide.shapes))[:80]:
            entry = {'shape': shape.shape_id, 'name': shape.name, 'kind': pptx_shape_kind(shape)}
            text = pptx_shape_text(shape)
            if text:
                entry['text'] = trunc(text, ITEM_TEXT_CHARS)
            if getattr(shape, 'has_chart', False) and shape.has_chart:
                try:
                    entry['chart'] = {'type': str(shape.chart.chart_type).split('.')[-1].split(' ')[0],
                                      'series': [s.name for s in shape.chart.plots[0].series][:20]}
                except Exception:
                    pass
            shapes.append(entry)
        item = {'slide': number, 'layout': slide.slide_layout.name, 'shapes': shapes}
        notes = pptx_notes(slide)
        if notes.strip():
            item['notes'] = trunc(notes, ITEM_TEXT_CHARS)
        items.append(item)
    base = {'format': 'pptx', 'source_sha256': sha256(job.data(index)), 'slides': len(prs.slides),
            'slide_size_cm': [round(prs.slide_width / 360000, 2), round(prs.slide_height / 360000, 2)],
            'layouts': [layout.name for layout in prs.slide_layouts][:40]}
    return fit_items(job, base, items)


def pptx_metadata(job, index):
    prs = pptx_open(job, index)
    return {'slides': len(prs.slides),
            'slide_size_cm': [round(prs.slide_width / 360000, 2), round(prs.slide_height / 360000, 2)],
            'layouts': [layout.name for layout in prs.slide_layouts][:40],
            'notes': sum(1 for slide in prs.slides if pptx_notes(slide).strip()),
            'properties': core_properties(prs.core_properties)}


class PptxBuilder:
    def __init__(self, job, prs):
        self.job, self.prs = job, prs
        self.counts = {'slides': 0, 'images': 0, 'tables': 0, 'charts': 0}
        self.image_cache = {}

    def layout(self, spec, path):
        from pptx.enum.shapes import PP_PLACEHOLDER
        layouts = list(self.prs.slide_layouts)
        if spec.get('layout_name') is not None:
            name = as_str(spec['layout_name'], path + '.layout_name', 200)
            for layout in layouts:
                if layout.name.lower() == name.lower():
                    return layout
            fail('invalid_spec', '%s.layout_name "%s" does not exist; layouts: %s' % (
                path, name, ', '.join(layout.name for layout in layouts)), path + '.layout_name')
        kind = spec.get('layout')
        content = any(spec.get(k) is not None for k in ('bullets', 'text', 'image', 'table', 'chart'))
        if kind is None:
            if spec.get('left') is not None or spec.get('right') is not None:
                kind = 'two_content'
            elif spec.get('subtitle') is not None and not content:
                kind = 'title'
            elif content:
                kind = 'content'
            else:
                kind = 'title_only' if spec.get('title') is not None else 'blank'
        kind = as_str(kind, path + '.layout', 32, tuple(LAYOUT_ALIASES))
        for alias in LAYOUT_ALIASES[kind]:
            for layout in layouts:
                if layout.name.lower() == alias.lower():
                    return layout
        wanted = {'content': 1, 'two_content': 2, 'comparison': 2}.get(kind, 0)
        best, score = layouts[min(len(layouts) - 1, 1)], -1
        for layout in layouts:
            types = [ph.placeholder_format.type for ph in layout.placeholders]
            bodies = sum(1 for t in types if t in (PP_PLACEHOLDER.BODY, PP_PLACEHOLDER.OBJECT))
            titles = sum(1 for t in types if t in (PP_PLACEHOLDER.TITLE, PP_PLACEHOLDER.CENTER_TITLE))
            current = (10 if bodies == wanted else 0) + (5 if (titles > 0) == (kind != 'blank') else 0)
            if current > score:
                best, score = layout, current
        return best

    def body_placeholders(self, slide):
        from pptx.enum.shapes import PP_PLACEHOLDER
        result = [ph for ph in slide.placeholders if ph.placeholder_format.type in (
            PP_PLACEHOLDER.BODY, PP_PLACEHOLDER.OBJECT) and ph.placeholder_format.idx != 0]
        return sorted(result, key=lambda ph: (ph.left or 0, ph.top or 0))

    def default_box(self):
        width, height = self.prs.slide_width, self.prs.slide_height
        return int(width * 0.06), int(height * 0.22), int(width * 0.88), int(height * 0.70)

    def add_slide(self, spec, path):
        from pptx.enum.shapes import PP_PLACEHOLDER
        as_dict(spec, path, SLIDE_KEYS)
        need(len(self.prs.slides) < MAX_SLIDES, 'limit', 'a presentation can have at most %d slides' % MAX_SLIDES,
             path)
        self.counts['slides'] += 1
        slide = self.prs.slides.add_slide(self.layout(spec, path))
        used = set()
        if spec.get('title') is not None:
            title = slide.shapes.title
            text = as_str(spec['title'], path + '.title', 2000)
            if title is None:
                width, height = self.prs.slide_width, self.prs.slide_height
                title = self.add_textbox(slide, (int(width * 0.06), int(height * 0.05), int(width * 0.88),
                                                 int(height * 0.15)))
            self.set_text(title.text_frame, [{'text': text, 'path': path + '.title'}], path + '.title', False)
            used.add(title.shape_id)
        if spec.get('subtitle') is not None:
            subtitle = next((ph for ph in slide.placeholders
                             if ph.placeholder_format.type == PP_PLACEHOLDER.SUBTITLE), None)
            if subtitle is None:
                bodies = [ph for ph in self.body_placeholders(slide) if ph.shape_id not in used]
                subtitle = bodies[0] if bodies else None
            if subtitle is not None:
                self.set_text(subtitle.text_frame, [{'text': as_str(spec['subtitle'], path + '.subtitle', 2000),
                                                     'path': path + '.subtitle', 'bullet': False}],
                              path + '.subtitle')
                used.add(subtitle.shape_id)
            else:
                self.job.warn('%s: the layout has no subtitle area' % path)
        bodies = [ph for ph in self.body_placeholders(slide) if ph.shape_id not in used]
        if spec.get('left') is not None or spec.get('right') is not None:
            left, top, width, height = self.default_box()
            half = width // 2 - int(width * 0.02)
            boxes = [(left, top, half, height), (left + width - half, top, half, height)]
            for slot, key in enumerate(('left', 'right')):
                if spec.get(key) is None:
                    continue
                placeholder = bodies[slot] if slot < len(bodies) else None
                box = self.box_of(placeholder) if placeholder is not None else boxes[slot]
                self.fill_area(slide, spec[key], '%s.%s' % (path, key), placeholder, box, used)
        else:
            content = {k: spec[k] for k in ('bullets', 'text', 'image', 'table', 'chart') if spec.get(k) is not None}
            if content:
                placeholder = bodies[0] if bodies else None
                box = self.box_of(placeholder) if placeholder is not None else self.default_box()
                self.fill_area(slide, content, path, placeholder, box, used)
        for placeholder in list(slide.placeholders):
            if placeholder.shape_id not in used:
                placeholder._element.getparent().remove(placeholder._element)
        if spec.get('notes') is not None:
            slide.notes_slide.notes_text_frame.text = self.job.text(spec['notes'], path + '.notes')
        return slide

    @staticmethod
    def box_of(shape):
        return int(shape.left), int(shape.top), int(shape.width), int(shape.height)

    @staticmethod
    def add_textbox(slide, box):
        shape = slide.shapes.add_textbox(*box)
        shape.text_frame.word_wrap = True
        return shape

    def fill_area(self, slide, content, path, placeholder, box, used):
        if isinstance(content, list):
            content = {'bullets': content}
        elif isinstance(content, str):
            content = {'text': content}
        as_dict(content, path, {'bullets', 'text', 'image', 'table', 'chart'})
        visuals = [k for k in ('image', 'table', 'chart') if content.get(k) is not None]
        need(len(visuals) <= 1, 'invalid_spec', '%s: use at most one of image, table, chart per area' % path, path)
        texts = content.get('bullets') is not None or content.get('text') is not None
        left, top, width, height = box
        visual_box = box
        if texts:
            text_box = box
            if visuals:
                text_box = (left, top, int(width * 0.48), height)
                visual_box = (left + int(width * 0.52), top, int(width * 0.48), height)
            if placeholder is not None:
                placeholder.left, placeholder.top, placeholder.width, placeholder.height = text_box
                frame, bullets = placeholder.text_frame, True
                used.add(placeholder.shape_id)
            else:
                frame, bullets = self.add_textbox(slide, text_box).text_frame, False
            items = []
            if content.get('text') is not None:
                items.extend({'text': part, 'bullet': False, 'path': path + '.text'}
                             for part in self.job.text(content['text'], path + '.text').split('\n'))
            if content.get('bullets') is not None:
                items.extend(self.flatten_bullets(content['bullets'], path + '.bullets'))
            self.set_text(frame, items, path, bullets)
            size = sum(len(str(item.get('text', ''))) for item in items)
            if size > 900 or len(items) > 12:
                self.job.warn('%s: a lot of text for one slide; it may overflow' % path, 'overflow:' + path)
        if visuals:
            getattr(self, 'add_' + visuals[0])(slide, content[visuals[0]], '%s.%s' % (path, visuals[0]), visual_box)

    def flatten_bullets(self, bullets, path, level=0):
        result = []
        for index, item in enumerate(as_list(bullets, path, 500)):
            ipath = '%s[%d]' % (path, index)
            if isinstance(item, str):
                item = {'text': item}
            as_dict(item, ipath, {'text', 'runs', 'level', 'items'})
            entry = {k: v for k, v in item.items() if k in ('text', 'runs')}
            entry['level'] = opt(item, 'level', ipath, as_int, 0, 8, default=level)
            entry['path'] = ipath
            result.append(entry)
            if item.get('items'):
                result.extend(self.flatten_bullets(item['items'], ipath + '.items', entry['level'] + 1))
        return result

    def set_text(self, frame, paragraphs, path, bullets=True):
        from pptx.dml.color import RGBColor
        from pptx.oxml.ns import qn
        from pptx.util import Pt
        frame.clear()
        frame.word_wrap = True
        for index, item in enumerate(paragraphs):
            if isinstance(item, str):
                item = {'text': item}
            ipath = item.get('path') or '%s[%d]' % (path, index)
            paragraph = frame.paragraphs[0] if index == 0 else frame.add_paragraph()
            paragraph.level = item.get('level', 0)
            if item.get('bullet') is False and bullets:
                ppr = paragraph._p.get_or_add_pPr()
                ppr.set('marL', '0')
                ppr.set('indent', '0')
                ppr.insert(0, ppr.makeelement(qn('a:buNone'), {}))
            if item.get('runs') is not None:
                runs, rbase = as_list(item['runs'], ipath + '.runs', 1000), ipath + '.runs'
            else:
                runs, rbase = [{'text': item.get('text', '')}], None
            for rindex, spec in enumerate(runs):
                rpath = '%s[%d]' % (rbase, rindex) if rbase else (ipath if ipath.endswith(('.text', '.title',
                                                                                          '.subtitle')) else
                                                                  ipath + '.text')
                if isinstance(spec, str):
                    spec = {'text': spec}
                as_dict(spec, rpath, RUN_KEYS)
                text = self.job.text(spec['math'] if spec.get('math') else spec.get('text', ''), rpath)
                run = paragraph.add_run()
                run.text = text.replace('\n', '\v')
                font = run.font
                for key in ('bold', 'italic', 'underline'):
                    if opt(spec, key, rpath, as_bool):
                        setattr(font, key, True)
                if opt(spec, 'code', rpath, as_bool) or spec.get('math'):
                    font.name = 'Consolas'
                size = opt(spec, 'size', rpath, as_num, 4, 400)
                if size:
                    font.size = Pt(size)
                name = opt(spec, 'font', rpath, as_str, 64, pattern=r'[\w .\-]+')
                if name:
                    font.name = name
                color = opt(spec, 'color', rpath, hex_color)
                if color:
                    font.color.rgb = RGBColor.from_string(color)
                link = opt(spec, 'link', rpath, as_str, 2048)
                if link:
                    need(URL_PATTERN.match(link) is not None, 'invalid_spec',
                         '%s.link must be http(s) or mailto' % rpath, rpath + '.link')
                    run.hyperlink.address = link
                if opt(spec, 'strike', rpath, as_bool):
                    run._r.get_or_add_rPr().set('strike', 'sngStrike')

    def image_data(self, index, path):
        if index not in self.image_cache:
            self.image_cache[index] = prepare_image(self.job, index, path)
        return self.image_cache[index]

    def add_image(self, slide, spec, path, box):
        from pptx.util import Cm
        if type(spec) is int:
            spec = {'input': spec}
        as_dict(spec, path, {'input', 'left_cm', 'top_cm', 'width_cm', 'height_cm'}, ('input',))
        data, _, pixel_w, pixel_h = self.image_data(as_int(spec['input'], path + '.input', 0), path + '.input')
        left, top, width, height = box
        if any(spec.get(k) is not None for k in ('left_cm', 'top_cm', 'width_cm', 'height_cm')):
            left = int(Cm(opt(spec, 'left_cm', path, as_num, -100, 200, default=left / 360000)))
            top = int(Cm(opt(spec, 'top_cm', path, as_num, -100, 200, default=top / 360000)))
            width = int(Cm(opt(spec, 'width_cm', path, as_num, 0.2, 200, default=width / 360000)))
            height = int(Cm(opt(spec, 'height_cm', path, as_num, 0.2, 200, default=height / 360000)))
        ratio = min(width / pixel_w, height / pixel_h)
        fitted_w, fitted_h = int(pixel_w * ratio), int(pixel_h * ratio)
        slide.shapes.add_picture(io.BytesIO(data), left + (width - fitted_w) // 2, top + (height - fitted_h) // 2,
                                 fitted_w, fitted_h)
        self.counts['images'] += 1

    def add_table(self, slide, spec, path, box):
        from pptx.util import Pt
        if isinstance(spec, list):
            spec = {'rows': spec}
        as_dict(spec, path, {'rows', 'header', 'font_size'}, ('rows',))
        rows = as_list(spec['rows'], path + '.rows', 60, 1)
        cols = max(len(as_list(row, '%s.rows[%d]' % (path, i), 30)) for i, row in enumerate(rows))
        need(cols > 0, 'invalid_spec', '%s.rows has no cells' % path, path + '.rows')
        left, top, width, height = box
        row_height = min(height // len(rows), 457200)
        table = slide.shapes.add_table(len(rows), cols, left, top, width, row_height * len(rows)).table
        table.first_row = opt(spec, 'header', path, as_bool, default=True)
        size = opt(spec, 'font_size', path, as_num, 6, 40,
                   default=14 if len(rows) <= 8 else 12 if len(rows) <= 14 else 10)
        for r, row in enumerate(rows):
            for c in range(cols):
                value = row[c] if c < len(row) else ''
                text = value if isinstance(value, str) else '' if value is None else \
                    value.get('text', '') if isinstance(value, dict) else json.dumps(value)
                cell = table.cell(r, c)
                cell.text = self.job.text(text, '%s.rows[%d][%d]' % (path, r, c), 4000)
                for paragraph in cell.text_frame.paragraphs:
                    for run in paragraph.runs:
                        run.font.size = Pt(size)
        self.counts['tables'] += 1

    def add_chart(self, slide, spec, path, box):
        from pptx.chart.data import CategoryChartData, XyChartData
        from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION
        as_dict(spec, path, {'type', 'title', 'categories', 'series', 'legend', 'data_labels', 'number_format'},
                ('type', 'series'))
        kind = as_str(spec['type'], path + '.type', 20, CHART_TYPES_PPTX)
        series = as_list(spec['series'], path + '.series', 30, 1)
        if kind == 'scatter':
            data = XyChartData()
            for index, item in enumerate(series):
                spath = '%s.series[%d]' % (path, index)
                as_dict(item, spath, {'name', 'points'}, ('points',))
                entry = data.add_series(self.job.text(item.get('name') or 'Series %d' % (index + 1), spath + '.name',
                                                      255))
                for pindex, point in enumerate(as_list(item['points'], spath + '.points', 5000)):
                    ppath = '%s.points[%d]' % (spath, pindex)
                    as_list(point, ppath, 2, 2)
                    entry.add_data_point(as_num(point[0], ppath), as_num(point[1], ppath))
        else:
            categories = as_list(spec.get('categories') or [], path + '.categories', 1000, 1)
            data = CategoryChartData()
            data.categories = [self.job.text(str(c), path + '.categories', 255) for c in categories]
            for index, item in enumerate(series):
                spath = '%s.series[%d]' % (path, index)
                as_dict(item, spath, {'name', 'values'}, ('values',))
                values = as_list(item['values'], spath + '.values', 1000)
                need(len(values) == len(categories), 'invalid_spec', '%s.values needs one value per category' % spath,
                     spath + '.values')
                data.add_series(self.job.text(item.get('name') or 'Series %d' % (index + 1), spath + '.name', 255),
                                [None if v is None else as_num(v, spath + '.values') for v in values])
        chart_type = {'column': XL_CHART_TYPE.COLUMN_CLUSTERED, 'stacked_column': XL_CHART_TYPE.COLUMN_STACKED,
                      'bar': XL_CHART_TYPE.BAR_CLUSTERED, 'stacked_bar': XL_CHART_TYPE.BAR_STACKED,
                      'line': XL_CHART_TYPE.LINE_MARKERS, 'pie': XL_CHART_TYPE.PIE,
                      'doughnut': XL_CHART_TYPE.DOUGHNUT, 'area': XL_CHART_TYPE.AREA,
                      'scatter': XL_CHART_TYPE.XY_SCATTER}[kind]
        chart = slide.shapes.add_chart(chart_type, *box, data).chart
        if spec.get('title'):
            chart.has_title = True
            chart.chart_title.text_frame.text = self.job.text(spec['title'], path + '.title', 255)
        legend = opt(spec, 'legend', path, as_bool, default=len(series) > 1 or kind in ('pie', 'doughnut'))
        chart.has_legend = legend
        if legend:
            chart.legend.position = XL_LEGEND_POSITION.BOTTOM
            chart.legend.include_in_layout = False
        if opt(spec, 'data_labels', path, as_bool, default=False):
            plot = chart.plots[0]
            plot.has_data_labels = True
            if spec.get('number_format'):
                plot.data_labels.number_format = as_str(spec['number_format'], path + '.number_format', 64)
                plot.data_labels.number_format_is_linked = False
        self.counts['charts'] += 1


def pptx_widescreen(prs):
    """Switch the default 4:3 template to 16:9 and stretch master/layout shapes to match."""
    ratio = 12192000 / prs.slide_width
    prs.slide_width = 12192000
    for shapes in [prs.slide_master.shapes] + [layout.shapes for layout in prs.slide_layouts]:
        for shape in shapes:
            xfrm = shape._element.find('.//{%s}xfrm' % A_NS)
            if xfrm is None or xfrm.find('{%s}off' % A_NS) is None:
                continue
            shape.left = int(shape.left * ratio)
            shape.width = int(shape.width * ratio)


def pptx_slide_entry(prs, slide):
    for entry in prs.slides._sldIdLst:
        if prs.part.related_part(entry.rId) is slide.part:
            return entry
    fail('engine', 'slide not found in the slide list')


def pptx_delete_slide(prs, slide):
    entry = pptx_slide_entry(prs, slide)
    rid = entry.rId
    prs.slides._sldIdLst.remove(entry)
    prs.part.drop_rel(rid)


def pptx_create(job, spec):
    import pptx
    as_dict(spec, 'spec', {'format', 'template_input', 'keep_template_slides', 'aspect', 'slides', 'properties',
                           'values'}, ('slides',))
    template = spec.get('template_input')
    if template is not None:
        as_int(template, 'spec.template_input', 0)
        need(job.kind(template, 'spec.template_input') == 'pptx', 'invalid_spec',
             'spec.template_input must be pptx/potx', 'spec.template_input')
        need(ext_of(job.input(template)['name']) not in MACRO_EXTENSIONS, 'unsupported',
             'macro-enabled templates are not supported', 'spec.template_input')
        prs = pptx_open(job, template, 'spec.template_input')
        if not opt(spec, 'keep_template_slides', 'spec', as_bool, default=False):
            for slide in list(prs.slides):
                pptx_delete_slide(prs, slide)
    else:
        prs = pptx.Presentation()
        if opt(spec, 'aspect', 'spec', as_str, 8, ('16:9', '4:3'), default='16:9') == '16:9':
            pptx_widescreen(prs)
    set_core_properties(job, prs.core_properties, spec, 'spec.properties', template is None)
    builder = PptxBuilder(job, prs)
    for index, slide in enumerate(as_list(spec['slides'], 'spec.slides', MAX_SLIDES, 1)):
        builder.add_slide(slide, 'spec.slides[%d]' % index)
    result = dict(builder.counts)
    if spec.get('values') is not None:
        result.update(pptx_fill(job, prs, values_of(job, spec['values'], 'spec.values')))
    return save_pptx(job, prs), result


def save_pptx(job, prs):
    buffer = io.BytesIO()
    prs.save(buffer)
    data = buffer.getvalue()
    validate_package(job, data, 'pptx')
    return data


def pptx_text_frames(prs):
    for slide in prs.slides:
        for shape in pptx_shapes(slide.shapes):
            if shape.has_text_frame:
                yield shape.text_frame._txBody
            elif getattr(shape, 'has_table', False) and shape.has_table:
                for row in shape.table.rows:
                    for cell in row.cells:
                        yield cell.text_frame._txBody
        if slide.has_notes_slide and slide.notes_slide.notes_text_frame is not None:
            yield slide.notes_slide.notes_text_frame._txBody


def pptx_fill(job, prs, values):
    filled, unfilled = {}, set()

    def replace(match):
        if match.group(1) in values:
            filled[match.group(1)] = filled.get(match.group(1), 0) + 1
            return values[match.group(1)]
        unfilled.add(match.group(1))
        return None
    for body in pptx_text_frames(prs):
        for paragraph in body.iter(A_P):
            replace_in_nodes(list(paragraph.iter(A_T)), PLACEHOLDER, replace)
    missing = sorted(set(values) - set(filled))
    if missing:
        job.warn('Placeholders not found in the presentation: %s' % ', '.join(missing[:20]), 'unused-values')
    return {'filled': sum(filled.values()), 'unfilled': sorted(unfilled)[:100]}


def pptx_replace_frame_text(frame, text):
    """Plain set_text: replace the text but keep the first paragraph's and run's formatting."""
    body = frame._txBody
    paragraphs = list(body.iterchildren(A_P))
    template, run_props = None, None
    if paragraphs:
        template = copy.deepcopy(paragraphs[0])
        for child in list(template):
            if child.tag != A_PPR:
                template.remove(child)
        first_run = paragraphs[0].find(A_R)
        if first_run is not None and first_run.find(A_RPR) is not None:
            run_props = copy.deepcopy(first_run.find(A_RPR))
    for paragraph in paragraphs:
        body.remove(paragraph)
    for line in text.split('\n'):
        paragraph = copy.deepcopy(template) if template is not None else body.makeelement(A_P, {})
        run = paragraph.makeelement(A_R, {})
        if run_props is not None:
            run.append(copy.deepcopy(run_props))
        node = run.makeelement(A_T, {})
        node.text = line
        run.append(node)
        paragraph.append(run)
        body.append(paragraph)


def pptx_edit(job, spec):
    from pptx.util import Cm
    as_dict(spec, 'spec', {'source_sha256', 'patches', 'properties'}, ('source_sha256', 'patches'))
    prs = pptx_open(job, 0)
    slides = list(prs.slides)
    deleted = set()
    builder = PptxBuilder(job, prs)
    cursors = {}
    stats = {'patches_applied': 0}

    def slide_at(value, path):
        index = as_int(value, path, 0)
        need(index < len(slides), 'invalid_spec', '%s: slide %d does not exist (0..%d)' % (
            path, index, len(slides) - 1), path)
        need(index not in deleted, 'invalid_spec', '%s: slide %d was deleted by an earlier patch' % (path, index),
             path)
        return slides[index]

    def shape_at(slide, value, path):
        shape_id = as_int(value, path, 0)
        shape = next((s for s in pptx_shapes(slide.shapes) if s.shape_id == shape_id), None)
        need(shape is not None, 'invalid_spec', '%s: shape %d is not on this slide (see inspect)' % (
            path, shape_id), path)
        return shape

    for index, patch in enumerate(as_list(spec['patches'], 'spec.patches', MAX_PATCHES, 1)):
        path = 'spec.patches[%d]' % index
        as_dict(patch, path)
        op = patch.get('op') or ('set_text' if 'slide' in patch and 'shape' in patch else None)
        need(isinstance(op, str), 'invalid_spec', '%s.op is required; use one of: %s' % (
            path, ', '.join(PPTX_EDIT_OPS)), path + '.op')
        if op == 'set_text':
            as_dict(patch, path, {'op', 'slide', 'shape', 'text', 'paragraphs'}, ('slide', 'shape'))
            shape = shape_at(slide_at(patch['slide'], path + '.slide'), patch['shape'], path + '.shape')
            need(shape.has_text_frame, 'invalid_spec', '%s: shape %d has no text' % (path, shape.shape_id), path)
            if patch.get('paragraphs') is not None:
                builder.set_text(shape.text_frame, builder.flatten_bullets(patch['paragraphs'],
                                                                           path + '.paragraphs'), path)
            else:
                pptx_replace_frame_text(shape.text_frame, job.text(patch.get('text') or '', path + '.text'))
        elif op == 'add_slide':
            as_dict(patch, path, SLIDE_KEYS | {'op', 'after'})
            after = opt(patch, 'after', path, as_int, -1)
            slide = builder.add_slide({k: v for k, v in patch.items() if k not in ('op', 'after')}, path)
            if after is not None:
                entry = pptx_slide_entry(prs, slide)
                ids = prs.slides._sldIdLst
                anchor = cursors.get(after)
                if anchor is None and after >= 0:
                    anchor = pptx_slide_entry(prs, slide_at(after, path + '.after'))
                ids.remove(entry)
                if anchor is None:
                    ids.insert(0, entry)
                else:
                    anchor.addnext(entry)
                cursors[after] = entry
        elif op == 'delete_slide':
            as_dict(patch, path, {'op', 'slide'}, ('slide',))
            pptx_delete_slide(prs, slide_at(patch['slide'], path + '.slide'))
            deleted.add(patch['slide'])
            need(len(prs.slides) > 0, 'invalid_spec', '%s: a presentation needs at least one slide' % path, path)
        elif op == 'move_slide':
            as_dict(patch, path, {'op', 'slide', 'after'}, ('slide', 'after'))
            entry = pptx_slide_entry(prs, slide_at(patch['slide'], path + '.slide'))
            after = as_int(patch['after'], path + '.after', -1)
            ids = prs.slides._sldIdLst
            ids.remove(entry)
            if after == -1:
                ids.insert(0, entry)
            else:
                pptx_slide_entry(prs, slide_at(after, path + '.after')).addnext(entry)
        elif op == 'set_notes':
            as_dict(patch, path, {'op', 'slide', 'text'}, ('slide', 'text'))
            slide_at(patch['slide'], path + '.slide').notes_slide.notes_text_frame.text = job.text(
                patch['text'], path + '.text')
        elif op == 'delete_shape':
            as_dict(patch, path, {'op', 'slide', 'shape'}, ('slide', 'shape'))
            shape = shape_at(slide_at(patch['slide'], path + '.slide'), patch['shape'], path + '.shape')
            shape._element.getparent().remove(shape._element)
        elif op in ('insert_image', 'add_table', 'add_chart', 'add_textbox'):
            as_dict(patch, path, required=('slide',))
            slide = slide_at(patch['slide'], path + '.slide')
            content = {k: v for k, v in patch.items() if k not in ('op', 'slide')}
            box = builder.default_box()
            if op == 'insert_image':
                builder.add_image(slide, content, path, box)
            elif op == 'add_table':
                builder.add_table(slide, content, path, box)
            elif op == 'add_chart':
                builder.add_chart(slide, content, path, box)
            else:
                as_dict(content, path, {'text', 'left_cm', 'top_cm', 'width_cm', 'height_cm'}, ('text',))
                shape = builder.add_textbox(slide, (
                    int(Cm(opt(content, 'left_cm', path, as_num, -100, 200, default=2))),
                    int(Cm(opt(content, 'top_cm', path, as_num, -100, 200, default=2))),
                    int(Cm(opt(content, 'width_cm', path, as_num, 0.5, 200, default=10))),
                    int(Cm(opt(content, 'height_cm', path, as_num, 0.5, 200, default=3)))))
                builder.set_text(shape.text_frame, [{'text': as_str(content['text'], path + '.text'),
                                                     'path': path + '.text'}], path, False)
        elif op == 'fill_placeholders':
            as_dict(patch, path, {'op', 'values'}, ('values',))
            stats.update(pptx_fill(job, prs, values_of(job, patch['values'], path + '.values')))
        elif op == 'replace_text':
            as_dict(patch, path, {'op', 'find', 'replace', 'match_case'}, ('find', 'replace'))
            find = job.text(patch['find'], path + '.find', 2000)
            need(find, 'invalid_spec', '%s.find must not be empty' % path, path + '.find')
            pattern = re.compile(re.escape(find), 0 if opt(patch, 'match_case', path, as_bool, default=True)
                                 else re.I)
            replacement = job.text(patch['replace'], path + '.replace', 20_000).replace('\n', ' ')
            count = 0
            for body in pptx_text_frames(prs):
                for paragraph in body.iter(A_P):
                    count += replace_in_nodes(list(paragraph.iter(A_T)), pattern, lambda m: replacement)
            stats['replacements'] = stats.get('replacements', 0) + count
            if not count:
                job.warn('%s: "%s" was not found' % (path, trunc(find, 60)))
        elif op == 'set_properties':
            as_dict(patch, path, {'op', 'title', 'author', 'subject', 'keywords', 'description', 'category'})
            set_core_properties(job, prs.core_properties,
                                {'properties': {k: v for k, v in patch.items() if k != 'op'}}, path, False)
        else:
            fail('invalid_spec', '%s.op "%s" is unknown; use one of: %s' % (path, op, ', '.join(PPTX_EDIT_OPS)),
                 path + '.op')
        stats['patches_applied'] += 1
    set_core_properties(job, prs.core_properties, spec, 'spec.properties', False)
    stats.update({k: v for k, v in builder.counts.items() if v})
    return save_pptx(job, prs), stats


# ---------------------------------------------------------------------------------------------
# PDF operations (pypdf) and watermark overlays (reportlab).
# ---------------------------------------------------------------------------------------------

PDF_ACTIONS = ('merge', 'split', 'extract_pages', 'rotate', 'reorder', 'delete_pages', 'watermark', 'fill_form',
               'set_metadata')


@contextlib.contextmanager
def pdf_text_layer(data, label):
    """Use the renderer's font recovery for text; pypdf still validates input first."""
    import pypdfium2 as pdfium
    try:
        document = pdfium.PdfDocument(data)
    except pdfium.PdfiumError:
        fail('corrupt_input', '%s text layer cannot be opened' % label)
    try:
        yield document
    finally:
        document.close()


def pdf_page_text(document, index):
    page = document[index]
    try:
        layer = page.get_textpage()
        try:
            return layer.get_text_range().replace('\r\n', '\n').replace('\r', '\n').strip()
        finally:
            layer.close()
    finally:
        page.close()


def pdf_text(job, index):
    item = job.input(index)
    reader = open_pdf(job, job.data(index), item['path'])
    total = len(reader.pages)
    pages = parse_pages(job.options['pages'], total, 'options.pages')
    limited = len(pages) > job.limits['max_pages_read']
    if limited:
        pages = pages[:job.limits['max_pages_read']]
        job.warn('Only the first %d pages were read (max_pages_read)' % job.limits['max_pages_read'], 'page-limit')
    parts, found = [], False
    with pdf_text_layer(job.data(index), item['path']) as document:
        for page_index in pages:
            try:
                text = pdf_page_text(document, page_index)
            except Exception:
                text = ''
                job.warn('Text of page %d could not be extracted' % (page_index + 1), 'extract:%d' % page_index)
            found = found or bool(text)
            parts.append('## Page %d\n%s' % (page_index + 1, text))
    if not found:
        job.warn('No text layer found; use operation ocr after setting up the optional local OCR runtime', 'no-text')
    return '\n\n'.join(parts), {'pages': total, 'encrypted': bool(reader.is_encrypted), 'truncated': limited}


def pdf_inspect(job, index):
    item = job.input(index)
    reader = open_pdf(job, job.data(index), item['path'])
    total = len(reader.pages)
    items = []
    with pdf_text_layer(job.data(index), item['path']) as document:
        for page_index in range(min(total, job.limits['max_pages_read'])):
            page = reader.pages[page_index]
            box = page.mediabox
            try:
                text = pdf_page_text(document, page_index)
            except Exception:
                text = ''
            items.append({'page': page_index + 1, 'width_pt': round(float(box.width), 1),
                          'height_pt': round(float(box.height), 1), 'rotation': int(page.rotation or 0),
                          'chars': len(text), 'text': trunc(text, 300)})
    fields = []
    try:
        for name, field in (reader.get_fields() or {}).items():
            entry = {'name': name, 'type': str(field.get('/FT', '')), 'value': str(field.get('/V', ''))[:200]}
            if field.get('/_States_'):
                entry['states'] = [str(s) for s in field.get('/_States_')][:20]
            if field.get('/Opt'):
                entry['options'] = [str(o) for o in field.get('/Opt')][:50]
            fields.append(entry)
            if len(fields) >= 200:
                break
    except Exception:
        job.warn('Form fields could not be read')
    base = {'format': 'pdf', 'source_sha256': sha256(job.data(index)), 'pages': total,
            'encrypted': bool(reader.is_encrypted), 'form_fields': fields}
    return fit_items(job, base, items)


def pdf_metadata(job, index):
    item = job.input(index)
    try:
        reader = open_pdf(job, job.data(index), item['path'])
    except EngineError as error:
        if error.code == 'encrypted':
            return {'encrypted': True, 'readable': False}
        raise
    info = {}
    try:
        meta = reader.metadata or {}
        for key in ('/Title', '/Author', '/Subject', '/Keywords', '/Creator', '/Producer', '/CreationDate',
                    '/ModDate'):
            if meta.get(key):
                info[key[1:].lower()] = str(meta.get(key))[:500]
    except Exception:
        pass
    try:
        fields = len(reader.get_fields() or {})
    except Exception:
        fields = 0
    box = reader.pages[0].mediabox
    return {'pages': len(reader.pages), 'encrypted': bool(reader.is_encrypted), 'readable': True, 'info': info,
            'first_page_pt': [round(float(box.width), 1), round(float(box.height), 1)], 'form_fields': fields}


def pdf_bytes(job, writer):
    from pypdf import PdfReader
    buffer = io.BytesIO()
    writer.write(buffer)
    data = buffer.getvalue()
    try:
        pages = len(PdfReader(io.BytesIO(data)).pages)
    except Exception as error:
        fail('engine', 'the produced PDF does not reopen: %s' % str(error)[:160])
    return data, pages


def watermark_overlay(job, spec, width, height):
    from pypdf import PdfReader
    from reportlab.lib.colors import Color
    from reportlab.pdfgen import canvas
    text = job.text(spec['text'], 'spec.text', 200)
    try:
        text.encode('latin-1')
    except UnicodeEncodeError:
        fail('invalid_spec', 'spec.text: watermark text supports Latin-1 characters only', 'spec.text')
    size = opt(spec, 'size', 'spec', as_num, 6, 400, default=max(24.0, min(width, height) / 8))
    color = opt(spec, 'color', 'spec', hex_color, default='808080')
    opacity = opt(spec, 'opacity', 'spec', as_num, 0.02, 1, default=0.18)
    angle = opt(spec, 'angle', 'spec', as_num, -360, 360, default=45)
    buffer = io.BytesIO()
    drawing = canvas.Canvas(buffer, pagesize=(width, height), pageCompression=1)
    drawing.setFillColor(Color(int(color[0:2], 16) / 255, int(color[2:4], 16) / 255, int(color[4:6], 16) / 255,
                               alpha=opacity))
    drawing.setFont('Helvetica-Bold', size)
    drawing.translate(width / 2, height / 2)
    drawing.rotate(angle)
    drawing.drawCentredString(0, -size / 3, text)
    drawing.showPage()
    drawing.save()
    return PdfReader(io.BytesIO(buffer.getvalue())).pages[0]


def pdf_copy_metadata(reader, writer):
    try:
        if reader.metadata:
            writer.add_metadata({k: v for k, v in reader.metadata.items() if isinstance(k, str)})
    except Exception:
        pass


def pdf_properties(job, writer, spec):
    values = spec.get('properties')
    if values is None:
        return
    as_dict(values, 'spec.properties', {'title', 'author', 'subject', 'keywords', 'creator'})
    writer.add_metadata({'/' + key.capitalize(): job.text(values[key], 'spec.properties.' + key, 1000)
                         for key in ('title', 'author', 'subject', 'keywords', 'creator')
                         if values.get(key) is not None})


def pdf_publish(job, writer, result):
    job.output_path(extensions=('pdf',))
    data, pages = pdf_bytes(job, writer)
    job.publish(job.output, data, MEDIA_TYPES['pdf'])
    job.qa = {'pages': pages, 'warnings': []}
    result['pages'] = pages
    if job.options['preview']:
        render_pdf_previews(job, data, 'output')
    return result


def op_pdf(job):
    from pypdf import PdfWriter
    spec = job.spec
    action = as_str(spec.get('action'), 'spec.action', 32, PDF_ACTIONS)
    need(job.inputs, 'invalid_spec', 'pdf needs at least one input', 'inputs')
    result = {'action': action}
    if action == 'merge':
        as_dict(spec, 'spec', {'action', 'items', 'properties'})
        items = spec.get('items') or [{'input': i} for i in range(len(job.inputs))]
        writer, total = PdfWriter(), 0
        for index, item in enumerate(as_list(items, 'spec.items', MAX_INPUTS, 1)):
            ipath = 'spec.items[%d]' % index
            as_dict(item, ipath, {'input', 'pages'}, ('input',))
            reader = pdf_for_write(job, as_int(item['input'], ipath + '.input', 0), ipath + '.input')
            pages = parse_pages(item.get('pages'), len(reader.pages), ipath + '.pages')
            total += len(pages)
            need(total <= MAX_PDF_PAGES_WRITE, 'limit', 'the merged PDF exceeds %d pages' % MAX_PDF_PAGES_WRITE,
                 ipath)
            duplicate_fields = set(reader.get_fields() or {}) & set(writer.get_fields() or {})
            need(not duplicate_fields, 'invalid_spec',
                 'PDFs with duplicate form-field names cannot be merged without renaming the fields', ipath)
            # Import selected widgets with their AcroForm tree, not just page annotations.
            writer.append(reader, pages=pages, import_outline=False)
        pdf_properties(job, writer, spec)
        return pdf_publish(job, writer, result)
    reader = pdf_for_write(job, 0)
    total = len(reader.pages)
    if action in ('split', 'extract_pages'):
        as_dict(spec, 'spec', {'action', 'every', 'ranges'} if action == 'split' else {'action', 'pages'})
        if action == 'split':
            if spec.get('ranges') is not None:
                groups = [parse_pages(as_str(value, 'spec.ranges[%d]' % i, 100), total, 'spec.ranges[%d]' % i)
                          for i, value in enumerate(as_list(spec['ranges'], 'spec.ranges', 1000, 1))]
            else:
                every = opt(spec, 'every', 'spec', as_int, 1, total, default=1)
                groups = [list(range(start, min(total, start + every))) for start in range(0, total, every)]
        else:
            need(spec.get('pages') is not None, 'invalid_spec', 'spec.pages is required', 'spec.pages')
            pages = parse_pages(spec['pages'], total, 'spec.pages')
            if ext_of(job.output or '') == 'pdf':
                writer = PdfWriter()
                writer.append(reader, pages=pages, import_outline=False)
                pdf_copy_metadata(reader, writer)
                return pdf_publish(job, writer, result)
            groups = [[page] for page in pages]
        need(ext_of(job.output or '') != 'pdf', 'invalid_spec',
             'output.name must be a folder name (no .pdf) for %s' % action, 'output.name')
        folder = job.rel(job.output_path(directory=True))
        need(len(groups) <= job.limits['max_outputs'], 'limit', '%s would create %d files (limit %d)' % (
            action, len(groups), job.limits['max_outputs']), 'spec')
        names = []
        for number, group in enumerate(groups, 1):
            writer = PdfWriter()
            writer.append(reader, pages=group, import_outline=False)
            data, _ = pdf_bytes(job, writer)
            label = ('page-%0*d.pdf' % (len(str(total)), group[0] + 1) if action == 'extract_pages' else
                     'part-%0*d.pdf' % (max(2, len(str(len(groups)))), number))
            job.publish(folder + '/' + label, data, MEDIA_TYPES['pdf'])
            names.append(label)
        result.update({'files': names, 'folder': folder})
        return result
    if action in ('reorder', 'delete_pages'):
        if action == 'reorder':
            as_dict(spec, 'spec', {'action', 'order'}, ('order',))
            order = [as_int(v, 'spec.order[%d]' % i, 1, total) - 1
                     for i, v in enumerate(as_list(spec['order'], 'spec.order', MAX_PDF_PAGES_WRITE, 1))]
            need(sorted(order) == list(range(total)), 'invalid_spec', 'spec.order must list every page exactly once '
                 '(1..%d); use delete_pages or extract_pages to drop pages' % total, 'spec.order')
        else:
            as_dict(spec, 'spec', {'action', 'pages'}, ('pages',))
            drop = set(parse_pages(spec['pages'], total, 'spec.pages'))
            order = [page for page in range(total) if page not in drop]
            need(order, 'invalid_spec', 'spec.pages would delete every page', 'spec.pages')
        writer = PdfWriter()
        writer.append(reader, pages=order, import_outline=False)
        pdf_copy_metadata(reader, writer)
        return pdf_publish(job, writer, result)
    writer = PdfWriter(clone_from=reader)
    if action == 'rotate':
        as_dict(spec, 'spec', {'action', 'pages', 'degrees'}, ('degrees',))
        degrees = as_int(spec['degrees'], 'spec.degrees', -270, 270)
        need(degrees % 90 == 0 and degrees != 0, 'invalid_spec', 'spec.degrees must be 90, 180, 270 or -90',
             'spec.degrees')
        pages = parse_pages(spec.get('pages'), total, 'spec.pages')
        for page in pages:
            writer.pages[page].rotate(degrees)
        result['pages_rotated'] = len(pages)
    elif action == 'watermark':
        as_dict(spec, 'spec', {'action', 'text', 'stamp_input', 'pages', 'size', 'color', 'opacity', 'angle',
                               'position'})
        need(spec.get('text') or spec.get('stamp_input') is not None, 'invalid_spec',
             'watermark needs spec.text or spec.stamp_input', 'spec.text')
        over = opt(spec, 'position', 'spec', as_str, 8, ('over', 'under'), default='over') == 'over'
        stamp = None
        if spec.get('stamp_input') is not None:
            stamp = pdf_for_write(job, as_int(spec['stamp_input'], 'spec.stamp_input', 1),
                                  'spec.stamp_input').pages[0]
        cache = {}
        pages = parse_pages(spec.get('pages'), total, 'spec.pages')
        for page_index in pages:
            page = writer.pages[page_index]
            box = page.mediabox
            width, height = float(box.width), float(box.height)
            overlay = stamp
            if overlay is None:
                key = (round(width, 1), round(height, 1))
                if key not in cache:
                    cache[key] = watermark_overlay(job, spec, width, height)
                overlay = cache[key]
            page.merge_translated_page(overlay, float(box.left), float(box.bottom), over=over)
        result['pages_marked'] = len(pages)
    elif action == 'fill_form':
        as_dict(spec, 'spec', {'action', 'fields', 'flatten'}, ('fields',))
        fields = as_dict(spec['fields'], 'spec.fields')
        existing = reader.get_fields() or {}
        need(existing, 'invalid_spec', 'this PDF has no form fields', 'spec.fields')
        values = {}
        for name, value in fields.items():
            need(name in existing, 'invalid_spec', 'spec.fields: "%s" is not a form field; fields: %s' % (
                name, ', '.join(list(existing)[:40])), 'spec.fields.' + name)
            field = existing[name]
            if str(field.get('/FT')) == '/Btn':
                states = [str(s) for s in (field.get('/_States_') or []) if str(s) != '/Off']
                if isinstance(value, bool):
                    value = (states[0] if states else '/Yes') if value else '/Off'
                else:
                    value = as_str(value, 'spec.fields.' + name, 200)
                    value = value if value.startswith('/') else '/' + value
            elif isinstance(value, (int, float)) and not isinstance(value, bool):
                value = str(value)
            else:
                value = job.text(value, 'spec.fields.' + name, 10_000)
            values[name] = value
        writer.update_page_form_field_values(None, values, auto_regenerate=False,
                                             flatten=opt(spec, 'flatten', 'spec', as_bool, default=False))
        writer.set_need_appearances_writer(True)
        result['fields_filled'] = len(values)
    elif action == 'set_metadata':
        as_dict(spec, 'spec', {'action', 'properties'}, ('properties',))
        pdf_properties(job, writer, spec)
    return pdf_publish(job, writer, result)


# ---------------------------------------------------------------------------------------------
# LaTeX: templating (latex_prepare) and log parsing / QA (latex_finalize).
# ---------------------------------------------------------------------------------------------

LATEX_ESCAPES = {'\\': r'\textbackslash{}', '{': r'\{', '}': r'\}', '$': r'\$', '&': r'\&', '#': r'\#',
                 '^': r'\textasciicircum{}', '_': r'\_', '~': r'\textasciitilde{}', '%': r'\%',
                 '"': r'\textquotedbl{}', '<': r'\textless{}', '>': r'\textgreater{}', '|': r'\textbar{}',
                 '[': '{[}', ']': '{]}', '`': r'\textasciigrave{}', '\u00a0': '~', '\t': ' '}
_LATEX_SPECIAL = re.compile('[%s]' % re.escape(''.join(LATEX_ESCAPES)))
_CJK = re.compile('[\u1100-\u11ff\u2e80-\u2fdf\u3000-\u30ff\u3100-\u31ff\u3130-\u318f\u3400-\u4dbf\u4e00-\u9fff'
                  '\uac00-\ud7af\uf900-\ufaff\uff00-\uffef\U00020000-\U0002fa1f]')
CJK_FONTS = ((re.compile('[\u1100-\u11ff\u3130-\u318f\uac00-\ud7af]'), 'UnBatang.ttf', 'UnBatangBold.ttf',
              'UnDotum.ttf'),
             (re.compile('[\u3040-\u30ff\u31f0-\u31ff]'), 'HaranoAjiMincho-Regular.otf', 'HaranoAjiMincho-Bold.otf',
              'HaranoAjiGothic-Regular.otf'),
             (None, 'FandolSong-Regular.otf', 'FandolSong-Bold.otf', 'FandolHei-Regular.otf'))
MATH_DENY_PARTS = ('input', 'include', 'file', 'read', 'write', 'open', 'close', 'catcode', 'lccode', 'uccode',
                   'sfcode', 'delcode', 'mathcode', 'def', 'expand', 'special', 'xetex', 'pdf', 'primitive',
                   'scantokens', 'detokenize', 'package', 'documentclass', 'command', 'environment', 'declare',
                   'makeat', 'explsyntax', 'lua', 'shell', 'jobname', 'everyeof', 'everyjob', 'everypar',
                   'everymath', 'everydisplay', 'everycr', 'everyhbox', 'everyvbox', 'endlinechar', 'font',
                   'csname', 'usename', 'expandargs', 'newdocument', 'immediate', 'message', 'output', 'mark',
                   'insert', 'vadjust', 'afterassignment', 'aftergroup', 'futurelet', 'globaldefs', 'tracing')
MATH_DENY_EXACT = {'let', 'global', 'show', 'showthe', 'showbox', 'showlists', 'string', 'end', 'dump', 'errhelp',
                   'errmessage', 'batchmode', 'nonstopmode', 'scrollmode', 'errorstopmode', 'verb', 'url', 'href',
                   'hyperref', 'uppercase', 'lowercase', 'def', 'gdef', 'edef', 'xdef', 'par', 'newline',
                   'clearpage', 'newpage', 'begingroup', 'endgroup', 'ifeof', 'chardef', 'mathchardef', 'countdef',
                   'dimendef', 'skipdef', 'toksdef', 'box', 'setbox', 'vbox', 'hbox', 'vtop', 'unhbox', 'unvbox'}
MATH_ENVIRONMENTS = {'matrix', 'pmatrix', 'bmatrix', 'Bmatrix', 'vmatrix', 'Vmatrix', 'smallmatrix', 'cases',
                     'aligned', 'alignedat', 'gathered', 'split', 'array', 'subarray', 'dcases'}
LATEX_CLASSES = ('article', 'report', 'scrartcl', 'scrreprt')
LATEX_PAPERS = {'a4': 'a4paper', 'a5': 'a5paper', 'letter': 'letterpaper', 'legal': 'legalpaper'}
PAPER_WIDTH_CM = {'a4': 21.0, 'a5': 14.8, 'letter': 21.59, 'legal': 21.59}
NOTO_FONTS = (('setmainfont', 'NotoSerif', ('Regular', 'Bold', 'Italic', 'BoldItalic')),
              ('setsansfont', 'NotoSans', ('Regular', 'Bold', 'Italic', 'BoldItalic')),
              ('setmonofont', 'NotoSansMono', ('Regular', 'Bold')))


def latex_escape(text):
    text = _LATEX_SPECIAL.sub(lambda m: LATEX_ESCAPES[m.group(0)], text)
    return text.replace('--', '-{}-').replace("''", "'{}'").replace(',,', ',{},').replace('!`', '!{}`')


def latex_url(url):
    replacements = {'\\': '%5C', '{': '%7B', '}': '%7D', '^': '%5E', '~': '%7E', ' ': '%20', '#': '\\#',
                    '%': '\\%', '&': '\\&', '_': '\\_'}
    return ''.join(replacements.get(char, char) for char in url)


def latex_math(value, path):
    """Math is the only raw LaTeX the model can write; refuse anything that is not plain math."""
    text = _XML_INVALID.sub('', as_str(value, path, 20_000)).strip()
    need(text, 'invalid_spec', '%s: math is empty' % path, path)
    need('^^' not in text, 'invalid_spec', '%s: "^^" sequences are not allowed in math' % path, path)
    need(re.search(r'(?<!\\)%', text) is None, 'invalid_spec', '%s: use \\%% for a percent sign in math' % path,
         path)
    need(re.search(r'(?<!\\)\$|\\\[|\\\]|\\\(|\\\)', text) is None, 'invalid_spec',
         '%s: give math without $ or \\[ \\] delimiters' % path, path)
    need('\n\n' not in text, 'invalid_spec', '%s: math cannot contain empty lines' % path, path)
    for match in re.finditer(r'\\([A-Za-z@]+)\s*(\{?)', text):
        word, lowered = match.group(1), match.group(1).lower()
        if word in ('begin', 'end') and match.group(2):
            continue  # environments are checked against MATH_ENVIRONMENTS below
        need('@' not in word and word not in MATH_DENY_EXACT and not any(part in lowered for part in MATH_DENY_PARTS),
             'invalid_spec', '%s: the command \\%s is not allowed in math' % (path, word), path)
    for match in re.finditer(r'\\(begin|end)\s*\{([^}]*)\}', text):
        need(match.group(2).strip().rstrip('*') in MATH_ENVIRONMENTS, 'invalid_spec',
             '%s: environment "%s" is not allowed in math; use one of %s' % (
                 path, match.group(2), ', '.join(sorted(MATH_ENVIRONMENTS))), path)
    depth = 0
    for char in re.sub(r'\\[{}]', '', text):
        depth += (char == '{') - (char == '}')
        need(depth >= 0, 'invalid_spec', '%s: unbalanced braces in math' % path, path)
    need(depth == 0, 'invalid_spec', '%s: unbalanced braces in math' % path, path)
    return text


def latex_cjk_setup(text):
    for pattern, main, bold, sans in CJK_FONTS:
        if pattern is None or pattern.search(text):
            break
    fonts = ('\\IfFontExistsTF{%s}{\\setCJKmainfont{%s}[BoldFont=%s]}{\\setCJKmainfont{%s}}'
             '\\IfFontExistsTF{%s}{\\setCJKsansfont{%s}\\setCJKmonofont{%s}}{}') % (bold, main, bold, main, sans, sans,
                                                                                 sans)
    return '\\IfFileExists{xeCJK.sty}{\\IfFontExistsTF{%s}{\\usepackage{xeCJK}%s}{}}{}' % (main, fonts)


class LatexWriter:
    """Turns the block vocabulary into XeLaTeX. Every text character is escaped; math is filtered."""

    def __init__(self, job, spec):
        self.job, self.spec = job, spec
        self.body = []
        self.assets = {}
        self.counts = {'blocks': 0, 'tables': 0, 'images': 0}
        self.cls = opt(spec, 'document_class', 'spec', as_str, 16, LATEX_CLASSES, default='article')
        self.paper = opt(spec, 'paper', 'spec', as_str, 8, tuple(LATEX_PAPERS), default='a4')
        self.margin = opt(spec, 'margins_cm', 'spec', as_num, 1, 5, default=2.5)
        self.chapters = self.cls in ('report', 'scrreprt')

    def esc(self, value, path, max_chars=MAX_STRING_CHARS, newline='\\newline '):
        lines = [latex_escape(line) for line in self.job.text(value, path, max_chars).split('\n')]
        while lines and not lines[0].strip():
            lines.pop(0)
        while lines and not lines[-1].strip():
            lines.pop()
        return newline.join(lines)

    def runs(self, block, path, newline='\\newline '):
        if block.get('runs') is not None:
            runs, rbase = as_list(block['runs'], path + '.runs', 10_000), path + '.runs'
        else:
            runs, rbase = [{'text': block.get('text') or ''}], None
        out = []
        for index, run in enumerate(runs):
            rpath = '%s[%d]' % (rbase, index) if rbase else path + '.text'
            if isinstance(run, str):
                run = {'text': run}
            as_dict(run, rpath, RUN_KEYS)
            if run.get('math'):
                out.append('\\(%s\\)' % latex_math(run['math'], rpath + '.math'))
                continue
            text = self.esc(run.get('text', ''), rpath if not rbase else rpath + '.text', newline=newline)
            if not text:
                continue
            if run.get('code'):
                text = '\\texttt{%s}' % text
            if run.get('bold'):
                text = '\\textbf{%s}' % text
            if run.get('italic'):
                text = '\\emph{%s}' % text
            if run.get('underline'):
                text = '\\uline{%s}' % text
            if run.get('strike'):
                text = '\\sout{%s}' % text
            if run.get('superscript'):
                text = '\\textsuperscript{%s}' % text
            if run.get('subscript'):
                text = '\\textsubscript{%s}' % text
            if run.get('color'):
                text = '\\textcolor[HTML]{%s}{%s}' % (hex_color(run['color'], rpath + '.color'), text)
            if run.get('size'):
                size = as_num(run['size'], rpath + '.size', 4, 100)
                text = '{\\fontsize{%.1fpt}{%.1fpt}\\selectfont %s}' % (size, size * 1.2, text)
            if run.get('link'):
                url = as_str(run['link'], rpath + '.link', 2048)
                need(URL_PATTERN.match(url) is not None, 'invalid_spec', '%s.link must be http(s) or mailto' % rpath,
                     rpath + '.link')
                text = '\\href{%s}{%s}' % (latex_url(url), text)
            for key in ('font', 'highlight'):
                if run.get(key):
                    self.job.warn('PDF output ignores the run key "%s"' % key, 'latex-' + key)
            out.append(text)
        return ''.join(out)

    def blocks(self, blocks, path):
        for index, block in enumerate(as_list(blocks, path, MAX_BLOCKS)):
            self.block(block, '%s[%d]' % (path, index))

    def block(self, block, path):
        if isinstance(block, str):
            block = {'type': 'paragraph', 'text': block}
        as_dict(block, path, required=('type',))
        kind = as_str(block['type'], path + '.type', 32)
        self.counts['blocks'] += 1
        need(self.counts['blocks'] <= MAX_BLOCKS * 4, 'limit', 'too many blocks', path)
        handler = getattr(self, 'block_' + kind, None) if kind in BLOCK_TYPES else None
        need(handler is not None, 'invalid_spec', '%s.type "%s" is unknown; use one of: %s' % (
            path, kind, ', '.join(BLOCK_TYPES)), path + '.type')
        handler(block, path)

    def block_heading(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs', 'level', 'align'})
        level = opt(block, 'level', path, as_int, 0, 9, default=1)
        commands = (['chapter', 'section', 'subsection', 'subsubsection', 'paragraph', 'subparagraph']
                    if self.chapters else ['section', 'subsection', 'subsubsection', 'paragraph', 'subparagraph'])
        command = commands[min(max(level, 1), len(commands)) - 1]
        self.body.append('\\%s{%s}\n' % (command, self.runs(block, path, newline=' ')))

    def block_paragraph(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs', 'style', 'align', 'space_after', 'indent_cm'})
        text = self.runs(block, path)
        if not text.strip():
            return
        align = opt(block, 'align', path, as_str, 16, ('left', 'center', 'right', 'justify'))
        env = {'left': 'flushleft', 'center': 'center', 'right': 'flushright'}.get(align)
        self.body.append('\\begin{%s}\n%s\n\\end{%s}\n' % (env, text, env) if env else text + '\n\n')

    def block_quote(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs'})
        self.body.append('\\begin{quote}\n%s\n\\end{quote}\n' % self.runs(block, path))

    def block_code(self, block, path):
        as_dict(block, path, {'type', 'text', 'language'})
        lines = ['\\mbox{}%s\\par' % latex_escape(line.expandtabs(4)).replace(' ', '\\ ')
                 for line in self.job.text(block.get('text') or '', path + '.text').split('\n')]
        self.body.append('\\par\\begingroup\\ttfamily\\small\\setlength{\\parindent}{0pt}\\setlength{\\parskip}{0pt}'
                         '\\raggedright\n%s\n\\endgroup\\par\\medskip\n' % '\n'.join(lines))

    def block_math(self, block, path):
        as_dict(block, path, {'type', 'latex'}, ('latex',))
        self.body.append('\\[\n%s\n\\]\n' % latex_math(block['latex'], path + '.latex'))

    def block_page_break(self, block, path):
        as_dict(block, path, {'type'})
        self.body.append('\\clearpage\n')

    def block_horizontal_rule(self, block, path):
        as_dict(block, path, {'type'})
        self.body.append('\\par\\noindent\\rule{\\linewidth}{0.4pt}\\par\n')

    def block_toc(self, block, path):
        as_dict(block, path, {'type', 'levels', 'title'})
        levels = opt(block, 'levels', path, as_int, 1, 5, default=3)
        self.body.append('\\setcounter{tocdepth}{%d}\n\\tableofcontents\n\\clearpage\n' % (
            levels - 1 if self.chapters else levels))

    def block_markdown(self, block, path):
        as_dict(block, path, {'type', 'text'})
        text = self.job.text(block.get('text') or '', path + '.text', 2_000_000)
        for index, child in enumerate(md_blocks(text)):
            self.block(child, '%s.markdown[%d]' % (path, index))

    def block_list(self, block, path, level=0, ordered=None):
        as_dict(block, path, {'type', 'items', 'ordered', 'start'})
        ordered = opt(block, 'ordered', path, as_bool, default=False) if ordered is None else ordered
        start = opt(block, 'start', path, as_int, 0, 10_000, default=1)
        items = as_list(block.get('items') or [], path + '.items', MAX_LIST_ITEMS)
        if not items:
            return
        nested = level < 4
        if not nested:
            self.job.warn('Lists deeper than 4 levels are flattened in PDF output', 'latex-depth')
        env = 'enumerate' if ordered else 'itemize'
        if nested:
            self.body.append('\\begin{%s}%s\n' % (env, '[start=%d]' % start if ordered and start != 1 else ''))
        for index, item in enumerate(items):
            ipath = '%s.items[%d]' % (path, index)
            if isinstance(item, str):
                item = {'text': item}
            as_dict(item, ipath, {'text', 'runs', 'items', 'ordered'})
            self.body.append('\\item{} %s\n' % self.runs(item, ipath))
            if item.get('items'):
                child = opt(item, 'ordered', ipath, as_bool, default=ordered)
                self.block_list({'type': 'list', 'items': item['items']}, ipath, level + 1 if nested else level, child)
        if nested:
            self.body.append('\\end{%s}\n' % env)

    def block_table(self, block, path):
        as_dict(block, path, {'type', 'rows', 'header', 'style', 'widths_cm', 'align', 'caption'})
        rows = as_list(block.get('rows'), path + '.rows', 5_000, 1)
        width = 0
        for index, row in enumerate(rows):
            as_list(row, '%s.rows[%d]' % (path, index), 60)
            width = max(width, sum(cell.get('colspan', 1) if isinstance(cell, dict) and
                                   type(cell.get('colspan', 1)) is int else 1 for cell in row))
        need(width > 0, 'invalid_spec', '%s.rows has no cells' % path, path + '.rows')
        need(width * len(rows) <= MAX_TABLE_CELLS, 'limit', '%s has too many cells' % path, path)
        aligns = opt(block, 'align', path, as_list, 60) or []
        widths = opt(block, 'widths_cm', path, as_list, 60)
        columns = []
        for column in range(width):
            align = aligns[column] if column < len(aligns) else 'left'
            ragged = {'center': '\\centering', 'right': '\\raggedleft'}.get(align, '\\raggedright')
            if widths and column < len(widths):
                size = '%.2fcm' % as_num(widths[column], '%s.widths_cm[%d]' % (path, column), 0.3, 60)
            else:
                size = '\\dimexpr %.4f\\linewidth-2\\tabcolsep\\relax' % (1.0 / width)
            columns.append('>{%s\\arraybackslash}p{%s}' % (ragged, size))
        header = opt(block, 'header', path, as_bool, default=True)
        lines = ['\\begin{longtable}{@{}%s@{}}' % ''.join(columns)]
        if block.get('caption') is not None:
            lines.append('\\caption{%s}\\\\' % self.esc(block['caption'], path + '.caption', 1000, ' '))
        lines.append('\\toprule')
        for r, row in enumerate(rows):
            cells = []
            for c, value in enumerate(row):
                cpath = '%s.rows[%d][%d]' % (path, r, c)
                spec = value if isinstance(value, dict) else {
                    'text': '' if value is None else value if isinstance(value, str) else json.dumps(value)}
                as_dict(spec, cpath, {'text', 'runs', 'bold', 'italic', 'align', 'fill', 'colspan', 'color'})
                text = self.runs(spec, cpath)
                if text and (spec.get('bold') or (header and r == 0)):
                    text = '\\textbf{%s}' % text
                if text and spec.get('italic'):
                    text = '\\emph{%s}' % text
                if text and spec.get('color'):
                    text = '\\textcolor[HTML]{%s}{%s}' % (hex_color(spec['color'], cpath + '.color'), text)
                if spec.get('fill'):
                    self.job.warn('PDF tables ignore cell fill colors', 'latex-fill')
                span = opt(spec, 'colspan', cpath, as_int, 1, width, default=1)
                cells.append('\\multicolumn{%d}{l}{%s}' % (span, text) if span > 1 else text)
            lines.append(' & '.join(cells) + ' \\\\')
            if header and r == 0:
                lines += ['\\midrule', '\\endhead']
        lines += ['\\bottomrule', '\\end{longtable}']
        self.body.append('\n'.join(lines) + '\n')
        self.counts['tables'] += 1

    def block_image(self, block, path):
        as_dict(block, path, {'type', 'input', 'width_cm', 'height_cm', 'caption', 'align'}, ('input',))
        index = as_int(block['input'], path + '.input', 0)
        if index not in self.assets:
            data, extension, pixel_w, _ = prepare_image(self.job, index, path + '.input', allow_pdf=True)
            self.assets[index] = ('assets/image-%d.%s' % (index, extension), data, pixel_w)
        options = []
        text_width = PAPER_WIDTH_CM[self.paper] - 2 * self.margin
        width = opt(block, 'width_cm', path, as_num, 0.2, 100)
        height = opt(block, 'height_cm', path, as_num, 0.2, 100)
        if width:
            options.append('width=%.2fcm' % min(width, text_width))
        if height:
            options.append('height=%.2fcm' % height)
        if not options:
            pixel_w = self.assets[index][2]
            natural = pixel_w / 96 * 2.54 if pixel_w else text_width  # like DOCX: 96 dpi, never wider than the text
            options = ['width=%.2fcm' % min(natural, text_width), 'height=0.45\\textheight']
        lines = ['\\begin{figure}[H]', '\\centering',
                 '\\includegraphics[%s]{%s}' % (','.join(options + ['keepaspectratio']), self.assets[index][0])]
        if block.get('caption') is not None:
            lines.append('\\caption{%s}' % self.esc(block['caption'], path + '.caption', 1000, ' '))
        lines.append('\\end{figure}')
        self.body.append('\n'.join(lines) + '\n')
        self.counts['images'] += 1

    def document(self):
        spec = self.spec
        lang = opt(spec, 'lang', 'spec', as_str, 4, ('de', 'en'), default='de')
        size = opt(spec, 'font_size', 'spec', as_int, 10, 12, default=11)
        family = opt(spec, 'font', 'spec', as_str, 8, ('serif', 'sans'), default='serif')
        toc = opt(spec, 'toc', 'spec', as_bool, default=False)
        body = ''.join(self.body)
        title = self.esc(spec['title'], 'spec.title', 500, ' ') if spec.get('title') is not None else ''
        author = self.esc(spec['author'], 'spec.author', 500, ' ') if spec.get('author') is not None else ''
        lines = ['% Generated by LocalAI Cowork document engine ' + ENGINE_VERSION + '. Compile with XeLaTeX (Tectonic).',
                 '\\documentclass[%dpt,%s]{%s}' % (size, LATEX_PAPERS[self.paper], self.cls),
                 '\\usepackage{fontspec}', '\\defaultfontfeatures{Ligatures=TeX}']
        names = {'Regular': 'Upright', 'Bold': 'Bold', 'Italic': 'Italic', 'BoldItalic': 'BoldItalic'}
        for command, stem, styles in NOTO_FONTS:
            options = ['Extension=.ttf'] + ['%sFont=*-%s' % (names[s], s) for s in styles]
            if command == 'setmonofont':
                options.append('Scale=MatchLowercase')
            line = '\\%s{%s}[%s]' % (command, stem, ','.join(options))
            for style in reversed(styles):
                line = '\\IfFontExistsTF{%s-%s.ttf}{%s}{}' % (stem, style, line)
            lines.append(line)
        if family == 'sans':
            lines.append('\\renewcommand{\\familydefault}{\\sfdefault}')
        if _CJK.search(body + title + author):
            lines.append(latex_cjk_setup(body + title + author))
        lines += ['\\usepackage[%s]{babel}' % ('english,ngerman' if lang == 'de' else 'ngerman,english'),
                  '\\usepackage[%s,margin=%.2fcm]{geometry}' % (LATEX_PAPERS[self.paper], self.margin),
                  '\\usepackage{graphicx}', '\\usepackage{float}', '\\usepackage{array,booktabs,longtable}',
                  '\\usepackage{enumitem}', '\\usepackage{xcolor}', '\\usepackage{amsmath,amssymb}',
                  '\\IfFileExists{ulem.sty}{\\usepackage[normalem]{ulem}}{}',
                  '\\providecommand{\\uline}[1]{\\underline{#1}}',
                  '\\providecommand{\\sout}[1]{#1}',
                  '\\usepackage[hidelinks]{hyperref}',
                  '\\hypersetup{pdftitle={%s},pdfauthor={%s},pdflang={%s}}' % (
                      title, author, 'de-DE' if lang == 'de' else 'en-US'),
                  '\\tracinglostchars=2', '\\setlength{\\emergencystretch}{3em}', '\\setlength{\\parskip}{0.5em}',
                  '\\setlength{\\parindent}{0pt}']
        if title:
            date = spec.get('date')
            lines += ['\\title{%s}' % title, '\\author{%s}' % author,
                      '\\date{\\today}' if date is None or date == 'today' else
                      '\\date{%s}' % self.esc(as_str(date, 'spec.date', 100), 'spec.date', 100, ' ')]
        lines.append('\\begin{document}')
        if title:
            lines.append('\\maketitle')
        if toc:
            lines.append('\\tableofcontents\n\\clearpage')
        lines += [body.rstrip('\n'), '\\end{document}']
        return '\n'.join(lines) + '\n'


def op_latex_prepare(job):
    spec = job.spec
    as_dict(spec, 'spec', {'title', 'author', 'date', 'lang', 'toc', 'document_class', 'paper', 'margins_cm',
                           'font_size', 'font', 'markdown', 'blocks'})
    need(spec.get('markdown') is not None or spec.get('blocks') is not None, 'invalid_spec',
         'create_pdf needs spec.markdown or spec.blocks', 'spec.markdown')
    if job.output:
        job.output_path(extensions=('pdf',))
    writer = LatexWriter(job, spec)
    if spec.get('markdown') is not None:
        writer.block_markdown({'type': 'markdown', 'text': spec['markdown']}, 'spec')
    if spec.get('blocks') is not None:
        writer.blocks(spec['blocks'], 'spec.blocks')
    source = writer.document()
    directory = os.path.join(job.stage, 'work', 'latex')
    need(not os.path.lexists(os.path.join(directory, 'main.tex')), 'engine', 'work/latex/main.tex already exists')
    os.makedirs(os.path.join(directory, 'assets'), exist_ok=True)
    assets = []
    for name, data, _ in writer.assets.values():
        with open(os.path.join(directory, *name.split('/')), 'xb') as handle:
            handle.write(data)
        assets.append('work/latex/' + name)
    data = source.encode('utf-8')
    with open(os.path.join(directory, 'main.tex'), 'xb') as handle:
        handle.write(data)
    result = {'main': 'work/latex/main.tex', 'assets': assets, 'tex_bytes': len(data), 'tex_sha256': sha256(data),
              'cjk': bool(_CJK.search(source))}
    result.update(writer.counts)
    return result


_TEX_FILE = re.compile(r'(?:[A-Za-z]:[\\/]|\.{0,2}[\\/]|/)?[^\s()\[\]{}"]*\.[A-Za-z0-9]{1,8}')
_ERROR_HINTS = (
    (re.compile(r"File `([^']+)' not found"), 'the file or package is not available (packages must be in the '
                                              'offline cache; files must sit next to the main .tex file)'),
    (re.compile(r'Missing \$ inserted'), 'a math character (_ ^ etc.) was used outside math mode; escape it, e.g. \\_'),
    (re.compile(r'Undefined control sequence'), 'the command is misspelled or its package is not loaded'),
    (re.compile(r'Misplaced alignment tab character &'), 'escape & as \\& outside tables'),
    (re.compile(r'Too many }'), 'a closing brace } has no matching {'),
    (re.compile(r'Runaway argument|File ended while scanning'), 'a { or an environment is not closed'),
)


def parse_tex_log(text, user_files):
    # Prefer the most specific staged path when two sources share a basename.
    known_files = sorted(set(user_files), key=lambda name: (-len(name), name))
    lines = []
    for line in text.replace('\r\n', '\n').replace('\r', '\n').split('\n'):
        if lines and len(lines[-1]) == 79 and not lines[-1].startswith('!'):
            lines[-1] += line
        else:
            lines.append(line)
    stack = []
    errors, warnings_, overfull, missing = [], [], [], {}
    underfull, pages, undefined_refs = 0, None, 0

    def current():
        for name in reversed(stack):
            if name:
                return name
        return None

    def user_file(name, implicit_tex=False):
        name = name.replace('\\', '/').strip('"')
        while name.startswith('./'):
            name = name[2:]
        for user in known_files:
            if name == user or name.endswith('/' + user):
                return user
        if implicit_tex:
            for user in known_files:
                if user.lower().endswith('.tex') and (name == user[:-4] or name.endswith('/' + user[:-4])):
                    return user
        return None

    def norm(name):
        known = user_file(name)
        if known:
            return known
        name = name.replace('\\', '/').strip('"')
        return name.rsplit('/', 1)[-1]

    def scan(line):
        index = 0
        while index < len(line):
            char = line[index]
            if char == '(':
                # Tectonic logs \input{chapters/intro} as "(chapters/intro",
                # without .tex. Resolve only names in the staged source tree;
                # ordinary parenthetical log text must not become a source file.
                token = re.match(r'"[^"\r\n]+"|[^\s()\[\]{}"]+', line[index + 1:])
                known = user_file(token.group(0), implicit_tex=True) if token else None
                if known:
                    stack.append(known)
                    index += 1 + token.end()
                    continue
                match = _TEX_FILE.match(line, index + 1)
                if match and match.group(0):
                    stack.append(norm(match.group(0)))
                    index = match.end()
                    continue
                stack.append(None)
            elif char == ')' and stack:
                stack.pop()
            index += 1

    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith('! '):
            message = re.sub(r'^(?:!\s+)+', '', line).strip()  # Tectonic logs "! ! LaTeX Error: ..."
            message = re.sub(r'\.\.$', '.', message)
            file, number, context = current(), None, ''
            j = i + 1
            while j < len(lines) and j < i + 30 and not lines[j].startswith('! '):
                match = re.match(r'^l\.(\d+)\s?(.*)$', lines[j])
                if match:
                    number, context = int(match.group(1)), match.group(2)
                    break
                j += 1
            if message.startswith('Undefined control sequence') and context:
                tokens = re.findall(r'\\[A-Za-z@]+|\\.', context)
                if tokens:
                    message = 'Undefined control sequence %s' % tokens[-1]
            for pattern, hint in _ERROR_HINTS:
                if pattern.search(message):
                    message = '%s (%s)' % (message, hint)
                    break
            if context and not message.startswith('Undefined control sequence \\'):
                message = '%s; near: %s' % (message, context.strip()[-80:])
            errors.append({'severity': 'error', 'message': message, 'file': file, 'line': number})
            i = j + 2 if number is not None else i + 1
            continue
        if line.startswith('!  ==> Fatal error') or line.startswith('*** (job aborted'):
            if not errors:
                errors.append({'severity': 'error', 'message': line.strip('!* ').strip(), 'file': current(),
                               'line': None})
            i += 1
            continue
        box = re.match(r'^(Overfull|Underfull) \\[hv]box \((?:([\d.]+)pt too (?:wide|high)|badness (\d+))\)'
                       r'(?:.*?lines? (\d+)(?:--(\d+))?)?', line)
        if box:
            if box.group(1) == 'Overfull':
                overfull.append({'amount': float(box.group(2) or 0), 'file': current(),
                                 'line': int(box.group(4)) if box.group(4) else None})
            else:
                underfull += 1
            i += 1
            while i < len(lines) and lines[i].lstrip().startswith(('[]', '\\')):
                i += 1  # box display lines quote user text (unbalanced parentheses); never scan them
            continue
        lost = re.match(r'^Missing character: There is no (.+?)(?: \(U\+([0-9A-Fa-f]{4,6})\))? in font ([^!]*)!',
                        line)
        if lost:
            font = re.split(r'[/:]', lost.group(3).strip('"'))[0].strip() or 'font'
            code = lost.group(2).upper() if lost.group(2) else '%04X' % ord(lost.group(1)[0])
            missing.setdefault(font, {})[code] = lost.group(1)
            i += 1
            continue
        warning = re.match(r'^(LaTeX|Package ([\w@.-]+)|Class ([\w@.-]+))( Font)? Warning: (.*)$', line)
        if warning:
            text, j = warning.group(5), i + 1
            package = warning.group(2) or warning.group(3)
            while j < len(lines) and lines[j].strip() and j < i + 8 and (
                    (package and lines[j].startswith('(%s)' % package)) or lines[j].startswith('(Font)')
                    or not text.rstrip().endswith('.')):
                text += ' ' + re.sub(r'^\([\w@.-]+\)\s*', '', lines[j]).strip()
                j += 1
            number = re.search(r'on input line (\d+)', text)
            if re.search(r'(Reference|Citation) .* undefined', text):
                undefined_refs += 1
            if not re.search(r'Label\(s\) may have changed|Rerun to get|There were undefined references|'
                             r'Font shape .* undefined|Size substitutions|Some font shapes were not available|'
                             r'does not contain requested Script|float@addtolists', text):
                warnings_.append({'severity': 'warning', 'message': ('%s: ' % package if package else '') +
                                  text.strip(), 'file': current(), 'line': int(number.group(1)) if number else None})
            i = j
            continue
        output = re.match(r'^Output written on .*\((\d+) pages?', line)
        if output:
            pages = int(output.group(1))
        elif line.startswith('No pages of output'):
            pages = 0
        scan(line)
        i += 1
    for index, error in enumerate(errors[:-1]):
        following = errors[index + 1]
        if error['line'] is None and following['message'].startswith('Emergency stop'):
            error['line'], error['file'] = following['line'], following['file'] or error['file']
    if len(errors) > 1:
        errors = [e for e in errors if not e['message'].startswith(('Emergency stop', '==> Fatal error'))] or errors
    return {'errors': errors, 'warnings': warnings_, 'overfull': overfull, 'underfull': underfull,
            'missing': missing, 'pages': pages, 'undefined_refs': undefined_refs}


def tex_diagnostics(job, parsed, stderr_lines):
    items = list(parsed['errors'])
    if not items:
        for line in stderr_lines:
            match = re.match(r'^\s*error:\s*(.*)$', line)
            if match and 'halted on potentially-recoverable error' not in match.group(1):
                items.append({'severity': 'error', 'message': match.group(1).strip(), 'file': None, 'line': None})
    for font, chars in parsed['missing'].items():
        listed = ', '.join('%s (U+%s)' % (char, code) for code, char in list(chars.items())[:12])
        items.append({'severity': 'warning', 'message': 'Missing characters in font %s: %s%s' % (
            font, listed, ' …' if len(chars) > 12 else ''), 'file': None, 'line': None})
    for box in sorted(parsed['overfull'], key=lambda b: -b['amount'])[:10]:
        if box['amount'] >= 1.0:
            items.append({'severity': 'warning', 'message': 'Overfull box: %.1fpt too wide' % box['amount'],
                          'file': box['file'], 'line': box['line']})
    items.extend(parsed['warnings'])
    for item in items:
        if len(job.diagnostics) >= MAX_DIAGNOSTICS:
            break
        prefix = '%s:%s: ' % (item['file'], item['line'] or '?') if item.get('file') else ''
        entry = {'severity': item['severity'], 'message': trunc(prefix + item['message'], 400)}
        if item.get('file'):
            entry['file'] = item['file']
        if item.get('line'):
            entry['line'] = item['line']
        job.diagnostics.append(entry)


def user_tex_files(job):
    files = []
    for root in (os.path.join(job.stage, 'in', 'tex'), os.path.join(job.stage, 'work', 'latex')):
        if os.path.isdir(root):
            for directory, _, names in os.walk(root):
                for name in names:
                    files.append(os.path.relpath(os.path.join(directory, name), root).replace('\\', '/'))
                    if len(files) > 5000:
                        return files
    return files


def read_stage_text(job, name, areas):
    path = job.resolve(name, areas, False)
    if not os.path.isfile(path):
        return None
    need(os.path.getsize(path) <= MAX_LOG_BYTES, 'limit', '%s is too large' % name)
    with open(path, 'rb') as handle:
        return handle.read(MAX_LOG_BYTES).decode('utf-8', 'replace')


def find_stage_file(job, directory, extensions):
    root = os.path.join(job.stage, *directory.split('/'))
    if not os.path.isdir(root):
        return None
    found = sorted(name for name in os.listdir(root)
                   if ext_of(name) in extensions and os.path.isfile(os.path.join(root, name)))
    return '%s/%s' % (directory, found[0]) if len(found) == 1 else None


def pdf_empty_pages(reader, limit):
    empty = []
    for index, page in enumerate(reader.pages[:limit]):
        try:
            if (page.extract_text() or '').strip():
                continue
            resources = page.get('/Resources')
            resources = resources.get_object() if resources is not None else {}
            if resources.get('/XObject'):
                continue
            contents = page.get_contents()
            if contents is not None and len(contents.get_data()) > 200:
                continue
        except Exception:
            continue
        empty.append(index + 1)
    return empty


def op_latex_finalize(job):
    options = job.options
    log_name = options['log'] or find_stage_file(job, 'work/latex-out', ('log',))
    log = (read_stage_text(job, log_name, ('work',)) if log_name else None) or ''
    stderr = (read_stage_text(job, options['stderr'], ('work',)) or '') if options['stderr'] else ''
    parsed = parse_tex_log(log, user_tex_files(job))
    tex_diagnostics(job, parsed, stderr.splitlines())
    pdf_name = options['pdf'] or find_stage_file(job, 'work/latex-out', ('pdf',))
    pdf_path = job.resolve(pdf_name, ('work',), False) if pdf_name else None
    if options['compile_failed'] or not pdf_path or not os.path.isfile(pdf_path) or parsed['errors']:
        if not any(d['severity'] == 'error' for d in job.diagnostics):
            tail = [line for line in log.splitlines() if line.strip()][-3:]
            job.diagnostics.insert(0, {'severity': 'error', 'message': trunc(
                'LaTeX compilation failed' + (': ' + ' / '.join(tail) if tail else ' and produced no log'), 400)})
            del job.diagnostics[MAX_DIAGNOSTICS:]
        fail('latex_error', trunc(next(d for d in job.diagnostics if d['severity'] == 'error')['message'], 300))
    need(os.path.getsize(pdf_path) <= job.limits['max_output_bytes'], 'limit', 'the compiled PDF is too large')
    with open(pdf_path, 'rb') as handle:
        data = handle.read()
    reader = open_pdf(job, data, 'compiled PDF')
    pages = len(reader.pages)
    empty = pdf_empty_pages(reader, job.limits['max_pages_read'])
    job.output_path(extensions=('pdf',))
    job.publish(job.output, data, MEDIA_TYPES['pdf'])
    missing = sorted({'%s U+%s' % (char, code) for chars in parsed['missing'].values() for code, char in chars.items()})
    notes = []
    if parsed['overfull']:
        notes.append('%d overfull boxes (max %.1fpt too wide)' % (
            len(parsed['overfull']), max(b['amount'] for b in parsed['overfull'])))
    if missing:
        notes.append('%d missing characters: %s' % (len(missing), ', '.join(missing[:10])))
    if parsed['undefined_refs']:
        notes.append('%d undefined references or citations' % parsed['undefined_refs'])
    if empty:
        notes.append('empty pages: %s' % ', '.join(str(p) for p in empty[:20]))
    job.qa = {'pages': pages, 'warnings': notes, 'overfull_boxes': len(parsed['overfull']),
              'underfull_boxes': parsed['underfull'], 'missing_characters': missing[:50], 'empty_pages': empty[:50]}
    if options['preview']:
        render_pdf_previews(job, data, 'compiled PDF')
    return {'pages': pages, 'errors': 0, 'warnings': len(parsed['warnings']),
            'overfull_boxes': len(parsed['overfull']), 'missing_characters': len(missing)}


def op_recalculate_prepare(job):
    """Remove old formula values before Calc runs; preserve all original package parts."""
    from lxml import etree
    need(job.spec.get('recalculate') is True and len(job.inputs) == 1 and job.kind(0) == 'xlsx',
         'invalid_spec', 'Recalculation preparation needs one XLSX input and recalculate=true')
    data = check_ooxml(job, job.data(0), 'xlsx', 'recalculation source')
    xlsx_roundtrip_source(job, data)
    namespace = '{http://schemas.openxmlformats.org/spreadsheetml/2006/main}'
    prepared, formulas = io.BytesIO(), 0
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(prepared, 'w', zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            name = item.filename
            need(not name.startswith('xl/externalLinks/'), 'unsupported',
                 'Recalculation with external workbook links is not supported', 'inputs[0]')
            content = source.read(item)
            if name.endswith('.rels'):
                root = etree.fromstring(content, parser=safe_xml_parser())
                need(not any(str(node.get('TargetMode', '')).lower() == 'external' for node in root),
                     'unsupported', 'Recalculation with external package relationships is not supported', 'inputs[0]')
            if name.startswith('xl/') and name.endswith('.xml'):
                root = etree.fromstring(content, parser=safe_xml_parser())
                # Defined names and table formulas can also contain executable formulas.
                for local in ('definedName', 'calculatedColumnFormula', 'totalsRowFormula'):
                    for node in root.iter(namespace + local):
                        if node.text:
                            check_formula('=' + node.text.lstrip('='), 'inputs[0].formula')
                if name.startswith('xl/worksheets/'):
                    for cell in root.iter(namespace + 'c'):
                        formula = cell.find(namespace + 'f')
                        if formula is None:
                            continue
                        need(formula.get('t') not in ('array', 'dataTable'), 'unsupported',
                             'Array and data-table recalculation needs a qualified spreadsheet application', 'inputs[0]')
                        if formula.text:
                            check_formula('=' + formula.text.lstrip('='), 'inputs[0].formula')
                        formulas += 1
                        for child in list(cell):
                            if child.tag in (namespace + 'v', namespace + 'is'):
                                cell.remove(child)
                        cell.attrib.pop('t', None)
                    content = etree.tostring(root, encoding='UTF-8', xml_declaration=True)
                elif name == 'xl/workbook.xml':
                    calculation = root.find(namespace + 'calcPr')
                    if calculation is None:
                        calculation = etree.Element(namespace + 'calcPr')
                        preceding = {'fileVersion', 'fileSharing', 'workbookPr', 'workbookProtection', 'bookViews',
                                     'sheets', 'functionGroups', 'externalReferences', 'definedNames'}
                        position = next((i for i, node in enumerate(root) if isinstance(node.tag, str) and
                                         etree.QName(node).localname not in preceding), len(root))
                        root.insert(position, calculation)
                    calculation.set('calcMode', 'auto')
                    calculation.set('fullCalcOnLoad', '1')
                    calculation.set('forceFullCalc', '1')
                    calculation.set('calcId', '0')
                    content = etree.tostring(root, encoding='UTF-8', xml_declaration=True)
            target.writestr(item, content)
    content = prepared.getvalue()
    need(len(content) <= MAX_INPUT_BYTES, 'limit', 'The recalculation input exceeds the stage limit')
    output = 'work/recalculate/source.xlsx'
    destination = job.resolve(output, ('work',), False)
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    with open(destination, 'xb') as stream:
        stream.write(content)
    marker = {'format': 'cowork-recalculation-preparation/1', 'source_sha256': sha256(job.data(0)),
              'input_sha256': sha256(content), 'formula_cells': formulas}
    with open(job.resolve('work/recalculate/preparation.json', ('work',), False), 'x', encoding='utf-8') as stream:
        json.dump(marker, stream)
    return {'input': output, 'formula_caches_cleared': formulas}


def recalculation_preparation(job):
    path = job.resolve('work/recalculate/preparation.json', ('work',), True)
    need(os.path.getsize(path) <= 4096, 'limit', 'Invalid recalculation preparation')
    with open(path, encoding='utf-8') as stream:
        marker = json.load(stream)
    as_dict(marker, 'preparation', {'format', 'source_sha256', 'input_sha256', 'formula_cells'},
            ('format', 'source_sha256', 'input_sha256', 'formula_cells'))
    need(marker['format'] == 'cowork-recalculation-preparation/1' and
         marker['source_sha256'] == sha256(job.data(0)), 'source_changed',
         'The recalculation source does not match its preparation')
    with open(job.resolve('work/recalculate/source.xlsx', ('work',), True), 'rb') as stream:
        content = stream.read(MAX_INPUT_BYTES + 1)
    need(len(content) <= MAX_INPUT_BYTES and sha256(content) == marker['input_sha256'], 'source_changed',
         'The prepared recalculation input changed')
    as_int(marker['formula_cells'], 'preparation.formula_cells', 0, MAX_SHEET_CELLS * 200)
    return marker


def op_office_finalize(job):
    need(job.output, 'invalid_spec', 'convert needs output.name', 'output')
    extension = ext_of(job.output)
    name = job.options['converted'] or find_stage_file(job, 'work/office-out', (extension,))
    need(name, 'engine', 'LibreOffice produced no .%s file in work/office-out' % extension)
    need(ext_of(name) == extension, 'invalid_spec', 'the converted file type does not match output.name',
         'output.name')
    path = job.resolve(name, ('work',), True)
    need(os.path.getsize(path) <= job.limits['max_output_bytes'], 'limit', 'the converted file is too large')
    with open(path, 'rb') as handle:
        data = handle.read()
    qa = {'warnings': []}
    if extension == 'pdf':
        qa['pages'] = len(open_pdf(job, data, 'converted PDF').pages)
    elif extension in ('docx', 'xlsx', 'pptx'):
        validate_package(job, check_ooxml(job, data, extension, 'converted file'), extension)
        if extension == 'xlsx':
            recalculated = opt(job.spec, 'recalculate', 'spec', as_bool, default=False)
            qa['calculation'] = xlsx_calculation(data, recalculated)
            calculation = qa['calculation']
            if recalculated:
                marker = recalculation_preparation(job)
                need(calculation['formula_cells'] == marker['formula_cells'], 'unsupported',
                     'The spreadsheet application changed the number of formula cells', 'inputs[0]')
                calculation['input_caches_cleared'] = marker['formula_cells']
                xlsx_check_roundtrip(job, xlsx_roundtrip_source(job, job.data(0)), data)
                qa.update(job.qa or {})
            if calculation['missing_values'] or calculation['error_values']:
                job.warn('The spreadsheet has missing or error formula values; inspect the '
                         'calculation report before relying on its results', 'calculation')
    job.output_path(extensions=(extension,))
    job.publish(job.output, data)
    job.qa = qa
    if extension == 'pdf' and job.options['preview']:
        render_pdf_previews(job, data, 'converted PDF')
    result = {'format': extension, 'bytes': len(data)}
    if 'pages' in qa:
        result['pages'] = qa['pages']
    return result


# ---------------------------------------------------------------------------------------------
# create_pdf without LaTeX: ReportLab Platypus renderer (used while LaTeX is locked or missing).
# ---------------------------------------------------------------------------------------------

FALLBACK_FAMILIES = {
    'serif': (('NotoSerif', ('NotoSerif-Regular.ttf', 'NotoSerif-Bold.ttf', 'NotoSerif-Italic.ttf',
                             'NotoSerif-BoldItalic.ttf')),
              ('DejaVuSerif', ('DejaVuSerif.ttf', 'DejaVuSerif-Bold.ttf', 'DejaVuSerif-Italic.ttf',
                               'DejaVuSerif-BoldItalic.ttf'))),
    'sans': (('NotoSans', ('NotoSans-Regular.ttf', 'NotoSans-Bold.ttf', 'NotoSans-Italic.ttf',
                           'NotoSans-BoldItalic.ttf')),
             ('DejaVuSans', ('DejaVuSans.ttf', 'DejaVuSans-Bold.ttf', 'DejaVuSans-Oblique.ttf',
                             'DejaVuSans-BoldOblique.ttf')),
             ('Vera', ('Vera.ttf', 'VeraBd.ttf', 'VeraIt.ttf', 'VeraBI.ttf'))),
    'mono': (('NotoSansMono', ('NotoSansMono-Regular.ttf', 'NotoSansMono-Bold.ttf', None, None)),
             ('DejaVuSansMono', ('DejaVuSansMono.ttf', 'DejaVuSansMono-Bold.ttf', 'DejaVuSansMono-Oblique.ttf',
                                 'DejaVuSansMono-BoldOblique.ttf'))),
}
BUILTIN_FAMILIES = {'serif': ('Times-Roman', 'Times-Bold', 'Times-Italic', 'Times-BoldItalic'),
                    'sans': ('Helvetica', 'Helvetica-Bold', 'Helvetica-Oblique', 'Helvetica-BoldOblique'),
                    'mono': ('Courier', 'Courier-Bold', 'Courier-Oblique', 'Courier-BoldOblique')}
CJK_CID_FONTS = ((re.compile('[ᄀ-ᇿ㄰-㆏가-힯]'), 'HYSMyeongJo-Medium'),
                 (re.compile('[぀-ヿㇰ-ㇿ]'), 'HeiseiMin-W3'),
                 (None, 'STSong-Light'))
GERMAN_MONTHS = ('Januar', 'Februar', 'März', 'April', 'Mai', 'Juni', 'Juli', 'August', 'September', 'Oktober',
                 'November', 'Dezember')
_FONT_FILES = None


def fallback_font_files():
    """Font files shipped inside the Python environment: reportlab's own fonts plus <prefix>/share/fonts."""
    global _FONT_FILES
    if _FONT_FILES is None:
        import reportlab
        wanted = {name for families in FALLBACK_FAMILIES.values() for _, files in families for name in files if name}
        found = {}
        roots = [os.path.join(os.path.dirname(reportlab.__file__), 'fonts')]
        for prefix in dict.fromkeys((sys.prefix, sys.base_prefix)):
            roots += [os.path.join(prefix, 'fonts'), os.path.join(prefix, 'share', 'fonts')]
        for root in roots:
            if not os.path.isdir(root):
                continue
            seen = 0
            for directory, subdirs, names in os.walk(root):
                seen += len(names) + len(subdirs)
                if directory[len(root):].count(os.sep) >= 3 or seen > 20_000:
                    subdirs[:] = []
                for name in names:
                    if name in wanted and name not in found:
                        found[name] = os.path.join(directory, name)
        _FONT_FILES = found
    return _FONT_FILES


def fallback_family(role):
    """Register the best available family for a role; returns (regular, bold, italic, bold_italic, coverage)."""
    from reportlab.pdfbase import pdfmetrics
    from reportlab.pdfbase.ttfonts import TTFont
    files = fallback_font_files()
    chain = FALLBACK_FAMILIES[role] + (FALLBACK_FAMILIES['sans'] if role == 'serif' else ())
    for family, names in chain:
        if not names[0] or names[0] not in files:
            continue
        faces = []
        for suffix, name in zip(('', '-Bold', '-Italic', '-BoldItalic'), names):
            face = 'Cowork%s%s' % (family, suffix)
            if name and name in files:
                if face not in pdfmetrics.getRegisteredFontNames():
                    pdfmetrics.registerFont(TTFont(face, files[name]))
                faces.append(face)
            else:
                faces.append(faces[0] if suffix in ('-Italic', '') else faces[1] if len(faces) > 1 else faces[0])
        if len(faces) == 4 and faces[3] == faces[0] and faces[1] != faces[0]:
            faces[3] = faces[1]
        pdfmetrics.registerFontFamily('Cowork%s' % family, normal=faces[0], bold=faces[1], italic=faces[2],
                                      boldItalic=faces[3])
        coverage = set(pdfmetrics.getFont(faces[0]).face.charToGlyph)
        return faces[0], faces[1], faces[2], faces[3], family, coverage
    names = BUILTIN_FAMILIES[role]
    coverage = set()
    for code in range(32, 256):
        try:
            coverage.add(ord(bytes([code]).decode('cp1252')))
        except UnicodeDecodeError:
            continue
    return names[0], names[1], names[2], names[3], names[0], coverage


def rl_escape(text):
    return text.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;').replace('"', '&quot;')


class ReportLabWriter:
    """Renders the create_pdf block vocabulary with ReportLab Platypus (no LaTeX)."""

    def __init__(self, job, spec):
        from reportlab.lib.units import cm
        self.job, self.spec = job, spec
        self.story = []
        self.counts = {'blocks': 0, 'tables': 0, 'images': 0}
        self.used = {'main': set(), 'mono': set()}
        self.cjk_fonts = set()
        self.toc = opt(spec, 'toc', 'spec', as_bool, default=False)
        self.toc_levels = 3
        self.lang = opt(spec, 'lang', 'spec', as_str, 4, ('de', 'en'), default='de')
        paper = opt(spec, 'paper', 'spec', as_str, 8, tuple(LATEX_PAPERS), default='a4')
        self.pagesize = {'a4': (595.2756, 841.8898), 'a5': (419.5276, 595.2756), 'letter': (612.0, 792.0),
                         'legal': (612.0, 1008.0)}[paper]
        self.margin = opt(spec, 'margins_cm', 'spec', as_num, 1, 5, default=2.5) * cm
        self.size = opt(spec, 'font_size', 'spec', as_int, 9, 14, default=11)
        role = opt(spec, 'font', 'spec', as_str, 8, ('serif', 'sans'), default='serif')
        self.main = fallback_family(role)
        self.mono = fallback_family('mono')
        self.width = self.pagesize[0] - 2 * self.margin
        self.height = self.pagesize[1] - 2 * self.margin
        self.styles = self.make_styles()

    def make_styles(self):
        from reportlab.lib.enums import TA_CENTER, TA_LEFT
        from reportlab.lib.styles import ParagraphStyle
        size, regular, bold, italic = self.size, self.main[0], self.main[1], self.main[2]
        styles = {'body': ParagraphStyle('body', fontName=regular, fontSize=size, leading=size * 1.35,
                                         spaceAfter=size * 0.55, alignment=TA_LEFT),
                  'title': ParagraphStyle('title', fontName=bold, fontSize=size * 2.1, leading=size * 2.5,
                                          alignment=TA_CENTER, spaceAfter=size * 0.6),
                  'subtitle': ParagraphStyle('subtitle', fontName=regular, fontSize=size * 1.1, leading=size * 1.5,
                                             alignment=TA_CENTER, spaceAfter=size * 1.6),
                  'code': ParagraphStyle('code', fontName=self.mono[0], fontSize=size * 0.85, leading=size * 1.15,
                                         backColor='#F2F2F2', borderPadding=4, leftIndent=4, rightIndent=4,
                                         spaceBefore=4, spaceAfter=size * 0.9),
                  'quote': ParagraphStyle('quote', fontName=italic, fontSize=size, leading=size * 1.35,
                                          leftIndent=28, rightIndent=20, spaceAfter=size * 0.6),
                  'cell': ParagraphStyle('cell', fontName=regular, fontSize=size * 0.9, leading=size * 1.15),
                  'caption': ParagraphStyle('caption', fontName=italic, fontSize=size * 0.9, leading=size * 1.2,
                                            alignment=TA_CENTER, spaceAfter=size * 0.9)}
        for level, factor in enumerate((1.7, 1.4, 1.2, 1.08, 1.0, 1.0), 1):
            styles['h%d' % level] = ParagraphStyle('h%d' % level, fontName=bold, fontSize=size * factor,
                                                   leading=size * factor * 1.25, spaceBefore=size * (1.2 if level < 3
                                                                                                     else 0.8),
                                                   spaceAfter=size * 0.45, keepWithNext=1)
        for level in range(9):
            styles['list%d' % level] = ParagraphStyle('list%d' % level, parent=styles['body'],
                                                      leftIndent=16 + 16 * level, bulletIndent=4 + 16 * level,
                                                      spaceAfter=size * 0.2, bulletFontName=regular)
        return styles

    def inline(self, text, role='main'):
        """Escape text for Paragraph markup; CJK goes to a (non-embedded) Adobe CID font."""
        out = []
        position = 0
        for match in re.finditer(_CJK.pattern + '+', text):
            out.append(self.plain(text[position:match.start()], role))
            chunk = match.group(0)
            font = next(name for pattern, name in CJK_CID_FONTS if pattern is None or pattern.search(chunk))
            if font not in self.cjk_fonts:
                from reportlab.pdfbase import pdfmetrics
                from reportlab.pdfbase.cidfonts import UnicodeCIDFont
                if font not in pdfmetrics.getRegisteredFontNames():
                    pdfmetrics.registerFont(UnicodeCIDFont(font))
                    pdfmetrics.registerFontFamily(font, normal=font, bold=font, italic=font, boldItalic=font)
                self.cjk_fonts.add(font)
            out.append('<font face="%s">%s</font>' % (font, rl_escape(chunk)))
            position = match.end()
        out.append(self.plain(text[position:], role))
        return ''.join(out)

    def plain(self, text, role):
        if not text:
            return ''
        self.used[role].update(char for char in text if not char.isspace())
        return rl_escape(text).replace('\t', '&nbsp;' * 4).replace('\n', '<br/>')

    def runs(self, block, path, newline=True):
        if block.get('runs') is not None:
            runs, rbase = as_list(block['runs'], path + '.runs', 10_000), path + '.runs'
        else:
            runs, rbase = [{'text': block.get('text') or ''}], None
        out = []
        for index, run in enumerate(runs):
            rpath = '%s[%d]' % (rbase, index) if rbase else path + '.text'
            if isinstance(run, str):
                run = {'text': run}
            as_dict(run, rpath, RUN_KEYS)
            if run.get('math'):
                self.job.warn('Math is shown as LaTeX source while LaTeX is not available', 'fallback-math')
                text = '<font face="%s">%s</font>' % (self.mono[0], self.inline(
                    as_str(run['math'], rpath + '.math', 20_000), 'mono'))
                out.append(text)
                continue
            raw = self.job.text(run.get('text', ''), rpath if not rbase else rpath + '.text')
            if not newline:
                raw = raw.replace('\n', ' ')
            code = opt(run, 'code', rpath, as_bool)
            text = self.inline(raw, 'mono' if code else 'main')
            if not text:
                continue
            if code:
                text = '<font face="%s">%s</font>' % (self.mono[0], text)
            for key, tag in (('bold', 'b'), ('italic', 'i'), ('underline', 'u'), ('strike', 'strike'),
                             ('superscript', 'super'), ('subscript', 'sub')):
                if opt(run, key, rpath, as_bool):
                    text = '<%s>%s</%s>' % (tag, text, tag)
            if run.get('color'):
                text = '<font color="#%s">%s</font>' % (hex_color(run['color'], rpath + '.color'), text)
            if run.get('size'):
                text = '<font size="%.1f">%s</font>' % (as_num(run['size'], rpath + '.size', 4, 100), text)
            if run.get('link'):
                url = as_str(run['link'], rpath + '.link', 2048)
                need(URL_PATTERN.match(url) is not None, 'invalid_spec', '%s.link must be http(s) or mailto' % rpath,
                     rpath + '.link')
                text = '<a href="%s" color="#0563C1"><u>%s</u></a>' % (rl_escape(url), text)
            for key in ('font', 'highlight'):
                if run.get(key):
                    self.job.warn('PDF output ignores the run key "%s"' % key, 'fallback-' + key)
            out.append(text)
        return ''.join(out)

    def paragraph(self, markup, style, **extra):
        from reportlab.platypus import Paragraph
        try:
            return Paragraph(markup, self.styles[style] if isinstance(style, str) else style, **extra)
        except ValueError as error:
            fail('engine', 'the PDF renderer rejected generated markup: %s' % str(error)[:200])

    def blocks(self, blocks, path):
        for index, block in enumerate(as_list(blocks, path, MAX_BLOCKS)):
            self.block(block, '%s[%d]' % (path, index))

    def block(self, block, path):
        if isinstance(block, str):
            block = {'type': 'paragraph', 'text': block}
        as_dict(block, path, required=('type',))
        kind = as_str(block['type'], path + '.type', 32)
        self.counts['blocks'] += 1
        need(self.counts['blocks'] <= MAX_BLOCKS * 4, 'limit', 'too many blocks', path)
        handler = getattr(self, 'block_' + kind, None) if kind in BLOCK_TYPES else None
        need(handler is not None, 'invalid_spec', '%s.type "%s" is unknown; use one of: %s' % (
            path, kind, ', '.join(BLOCK_TYPES)), path + '.type')
        handler(block, path)

    def aligned(self, style, align):
        from reportlab.lib.enums import TA_CENTER, TA_JUSTIFY, TA_LEFT, TA_RIGHT
        from reportlab.lib.styles import ParagraphStyle
        if not align:
            return self.styles[style]
        return ParagraphStyle('%s-%s' % (style, align), parent=self.styles[style], alignment={
            'left': TA_LEFT, 'center': TA_CENTER, 'right': TA_RIGHT, 'justify': TA_JUSTIFY}[align])

    def block_heading(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs', 'level', 'align'})
        level = opt(block, 'level', path, as_int, 0, 9, default=1)
        align = opt(block, 'align', path, as_str, 16, ('left', 'center', 'right', 'justify'))
        paragraph = self.paragraph(self.runs(block, path, False),
                                   self.aligned('title' if level == 0 else 'h%d' % min(level, 6), align))
        paragraph._cowork_heading = max(level, 1)
        self.story.append(paragraph)

    def block_paragraph(self, block, path):
        from reportlab.lib.units import cm
        as_dict(block, path, {'type', 'text', 'runs', 'style', 'align', 'space_after', 'indent_cm'})
        markup = self.runs(block, path)
        if not markup.strip():
            return
        style = self.aligned('body', opt(block, 'align', path, as_str, 16, ('left', 'center', 'right', 'justify')))
        indent = opt(block, 'indent_cm', path, as_num, -10, 20)
        space = opt(block, 'space_after', path, as_num, 0, 200)
        if indent is not None or space is not None:
            from reportlab.lib.styles import ParagraphStyle
            style = ParagraphStyle(style.name + '-x', parent=style,
                                   leftIndent=(indent or 0) * cm if indent is not None else style.leftIndent,
                                   spaceAfter=space if space is not None else style.spaceAfter)
        self.story.append(self.paragraph(markup, style))

    def block_quote(self, block, path):
        as_dict(block, path, {'type', 'text', 'runs'})
        self.story.append(self.paragraph(self.runs(block, path), 'quote'))

    def block_code(self, block, path):
        as_dict(block, path, {'type', 'text', 'language'})
        text = self.job.text(block.get('text') or '', path + '.text').expandtabs(4)
        lines = [self.inline(line, 'mono').replace(' ', '&nbsp;') or '&nbsp;' for line in text.split('\n')]
        self.story.append(self.paragraph('<br/>'.join(lines), 'code'))

    def block_math(self, block, path):
        as_dict(block, path, {'type', 'latex'}, ('latex',))
        self.job.warn('Math is shown as LaTeX source while LaTeX is not available', 'fallback-math')
        self.block_code({'type': 'code', 'text': as_str(block['latex'], path + '.latex', 20_000)}, path)

    def block_page_break(self, block, path):
        from reportlab.platypus import PageBreak
        as_dict(block, path, {'type'})
        self.story.append(PageBreak())

    def block_horizontal_rule(self, block, path):
        from reportlab.platypus import HRFlowable
        as_dict(block, path, {'type'})
        self.story.append(HRFlowable(width='100%', thickness=0.6, color='#808080', spaceBefore=4, spaceAfter=8))

    def block_toc(self, block, path):
        as_dict(block, path, {'type', 'levels', 'title'})
        self.toc = True
        self.story.append(('toc', opt(block, 'levels', path, as_int, 1, 6, default=3)))

    def block_markdown(self, block, path):
        as_dict(block, path, {'type', 'text'})
        text = self.job.text(block.get('text') or '', path + '.text', 2_000_000)
        for index, child in enumerate(md_blocks(text)):
            self.block(child, '%s.markdown[%d]' % (path, index))

    def block_list(self, block, path, level=0, ordered=None):
        as_dict(block, path, {'type', 'items', 'ordered', 'start'})
        ordered = opt(block, 'ordered', path, as_bool, default=False) if ordered is None else ordered
        start = opt(block, 'start', path, as_int, 0, 10_000, default=1)
        for index, item in enumerate(as_list(block.get('items') or [], path + '.items', MAX_LIST_ITEMS)):
            ipath = '%s.items[%d]' % (path, index)
            if isinstance(item, str):
                item = {'text': item}
            as_dict(item, ipath, {'text', 'runs', 'items', 'ordered'})
            bullet = '%d.' % (start + index) if ordered else ('•', '–', '•')[level % 3]
            self.used['main'].update(bullet)
            self.story.append(self.paragraph(self.runs(item, ipath), 'list%d' % min(level, 8), bulletText=bullet))
            if item.get('items'):
                self.block_list({'type': 'list', 'items': item['items']}, ipath, level + 1,
                                opt(item, 'ordered', ipath, as_bool, default=ordered))

    def block_table(self, block, path):
        from reportlab.lib import colors
        from reportlab.lib.enums import TA_CENTER, TA_LEFT, TA_RIGHT
        from reportlab.lib.styles import ParagraphStyle
        from reportlab.lib.units import cm
        from reportlab.platypus import Table, TableStyle
        as_dict(block, path, {'type', 'rows', 'header', 'style', 'widths_cm', 'align', 'caption'})
        rows = as_list(block.get('rows'), path + '.rows', 5_000, 1)
        width = 0
        for index, row in enumerate(rows):
            as_list(row, '%s.rows[%d]' % (path, index), 60)
            width = max(width, sum(cell.get('colspan', 1) if isinstance(cell, dict) and
                                   type(cell.get('colspan', 1)) is int else 1 for cell in row))
        need(width > 0, 'invalid_spec', '%s.rows has no cells' % path, path + '.rows')
        need(width * len(rows) <= MAX_TABLE_CELLS, 'limit', '%s has too many cells' % path, path)
        header = opt(block, 'header', path, as_bool, default=True)
        aligns = opt(block, 'align', path, as_list, 60) or []
        alignment = {'left': TA_LEFT, 'center': TA_CENTER, 'right': TA_RIGHT, 'justify': TA_LEFT}
        commands = [('GRID', (0, 0), (-1, -1), 0.5, colors.HexColor('#9E9E9E')), ('VALIGN', (0, 0), (-1, -1), 'TOP'),
                    ('LEFTPADDING', (0, 0), (-1, -1), 4), ('RIGHTPADDING', (0, 0), (-1, -1), 4)]
        if header:
            commands.append(('BACKGROUND', (0, 0), (-1, 0), colors.HexColor('#D9E1F2')))
        data = []
        for r, row in enumerate(rows):
            cells, column = [], 0
            for c, value in enumerate(row):
                cpath = '%s.rows[%d][%d]' % (path, r, c)
                spec = value if isinstance(value, dict) else {
                    'text': '' if value is None else value if isinstance(value, str) else json.dumps(value)}
                as_dict(spec, cpath, {'text', 'runs', 'bold', 'italic', 'align', 'fill', 'colspan', 'color'})
                span = opt(spec, 'colspan', cpath, as_int, 1, width, default=1)
                need(column + span <= width, 'invalid_spec', '%s spans past the table width' % cpath, cpath)
                markup = self.runs(spec, cpath)
                if markup and (spec.get('bold') or (header and r == 0)):
                    markup = '<b>%s</b>' % markup
                if markup and spec.get('italic'):
                    markup = '<i>%s</i>' % markup
                if markup and spec.get('color'):
                    markup = '<font color="#%s">%s</font>' % (hex_color(spec['color'], cpath + '.color'), markup)
                align = opt(spec, 'align', cpath, as_str, 16, tuple(alignment))
                if align is None and column < len(aligns) and aligns[column] in alignment:
                    align = aligns[column]
                style = ParagraphStyle('cell-%s' % (align or 'left'), parent=self.styles['cell'],
                                       alignment=alignment[align or 'left'])
                cells.append(self.paragraph(markup, style))
                if span > 1:
                    commands.append(('SPAN', (column, r), (column + span - 1, r)))
                    cells.extend([''] * (span - 1))
                if spec.get('fill'):
                    commands.append(('BACKGROUND', (column, r), (column + span - 1, r),
                                     colors.HexColor('#' + hex_color(spec['fill'], cpath + '.fill'))))
                column += span
            cells.extend([''] * (width - len(cells)))
            data.append(cells)
        widths = opt(block, 'widths_cm', path, as_list, 60)
        if widths:
            col_widths = [as_num(v, '%s.widths_cm[%d]' % (path, i), 0.3, 60) * cm for i, v in enumerate(widths[:width])]
            col_widths += [self.width / width] * (width - len(col_widths))
        else:
            col_widths = [self.width / width] * width
        table = Table(data, colWidths=col_widths, repeatRows=1 if header else 0, hAlign='LEFT')
        table.setStyle(TableStyle(commands))
        if block.get('caption') is not None:
            self.story.append(self.paragraph(self.inline(self.job.text(block['caption'], path + '.caption', 1000)),
                                             'caption'))
        self.story.append(table)
        from reportlab.platypus import Spacer
        self.story.append(Spacer(1, self.size * 0.8))
        self.counts['tables'] += 1

    def block_image(self, block, path):
        from reportlab.lib.units import cm
        from reportlab.platypus import Image
        as_dict(block, path, {'type', 'input', 'width_cm', 'height_cm', 'caption', 'align'}, ('input',))
        index = as_int(block['input'], path + '.input', 0)
        need(self.job.kind(index, path + '.input') == 'image', 'unsupported',
             '%s: only PNG/JPEG/... images can be placed while LaTeX is not available' % path, path + '.input')
        data, _, pixel_w, pixel_h = prepare_image(self.job, index, path + '.input')
        width = opt(block, 'width_cm', path, as_num, 0.2, 100)
        height = opt(block, 'height_cm', path, as_num, 0.2, 100)
        if width and height:
            draw_w, draw_h = width * cm, height * cm
        elif width:
            draw_w, draw_h = width * cm, width * cm * pixel_h / pixel_w
        elif height:
            draw_w, draw_h = height * cm * pixel_w / pixel_h, height * cm
        else:
            draw_w, draw_h = pixel_w * 0.75, pixel_h * 0.75
        scale = min(1.0, self.width / draw_w, self.height * 0.75 / draw_h)
        image = Image(io.BytesIO(data), width=draw_w * scale, height=draw_h * scale)
        image.hAlign = opt(block, 'align', path, as_str, 16, ('left', 'center', 'right'), default='center').upper()
        self.story.append(image)
        if block.get('caption') is not None:
            self.story.append(self.paragraph(self.inline(self.job.text(block['caption'], path + '.caption', 1000)),
                                             'caption'))
        self.counts['images'] += 1

    def title_block(self):
        spec = self.spec
        if spec.get('title') is None:
            return []
        flowables = [self.paragraph(self.inline(self.job.text(spec['title'], 'spec.title', 500).replace('\n', ' ')),
                                    'title')]
        parts = []
        if spec.get('author'):
            parts.append(self.inline(self.job.text(spec['author'], 'spec.author', 500).replace('\n', ' ')))
        date = spec.get('date')
        if date is None or date == 'today':
            today = datetime.date.today()
            date = ('%d. %s %d' % (today.day, GERMAN_MONTHS[today.month - 1], today.year) if self.lang == 'de'
                    else today.strftime('%B %d, %Y').replace(' 0', ' '))
        if date:
            parts.append(self.inline(as_str(date, 'spec.date', 100)))
        if parts:
            flowables.append(self.paragraph('<br/>'.join(parts), 'subtitle'))
        return flowables

    def missing_characters(self):
        missing = set()
        for role, family in (('main', self.main), ('mono', self.mono)):
            missing.update(char for char in self.used[role] if ord(char) not in family[5] and ord(char) >= 32)
        return sorted(missing)

    def build(self):
        from reportlab.lib.styles import ParagraphStyle
        from reportlab.platypus import SimpleDocTemplate
        from reportlab.platypus.doctemplate import LayoutError
        from reportlab.platypus.tableofcontents import TableOfContents
        spec = self.spec
        writer = self

        class Document(SimpleDocTemplate):
            def beforeDocument(self):
                self._cowork_count, self._cowork_level = 0, -1

            def afterFlowable(self, flowable):
                level = getattr(flowable, '_cowork_heading', None)
                if level is None:
                    return
                text = flowable.getPlainText()
                key = 'heading-%d' % self._cowork_count
                self._cowork_count += 1
                outline = min(level - 1, self._cowork_level + 1)
                self.canv.bookmarkPage(key)
                self.canv.addOutlineEntry(text[:200], key, level=max(outline, 0), closed=False)
                self._cowork_level = max(outline, 0)
                if level <= writer.toc_levels:
                    self.notify('TOCEntry', (level - 1, text, self.page, key))

        def page_number(canvas, document):
            canvas.saveState()
            canvas.setFont(writer.main[0], 9)
            canvas.drawCentredString(writer.pagesize[0] / 2, writer.margin / 2, str(document.page))
            canvas.restoreState()

        buffer = io.BytesIO()
        title = self.job.text(spec.get('title') or '', 'spec.title', 500).replace('\n', ' ')
        author = self.job.text(spec.get('author') or '', 'spec.author', 500).replace('\n', ' ')
        document = Document(buffer, pagesize=self.pagesize, leftMargin=self.margin, rightMargin=self.margin,
                            topMargin=self.margin, bottomMargin=self.margin, title=title, author=author,
                            creator='LocalAI Cowork document engine %s' % ENGINE_VERSION, invariant=1)
        story = self.title_block()
        if self.toc and not any(isinstance(item, tuple) for item in self.story):
            self.story.insert(0, ('toc', 3))
        for item in self.story:
            if isinstance(item, tuple):
                from reportlab.platypus import PageBreak
                self.toc_levels = item[1]
                table = TableOfContents()
                table.levelStyles = [ParagraphStyle('toc%d' % i, fontName=self.main[0], fontSize=self.size,
                                                    leading=self.size * 1.4, leftIndent=14 * i, firstLineIndent=0)
                                     for i in range(6)]
                table.dotsMinLevel = 0
                story += [table, PageBreak()]
            else:
                story.append(item)
        if not story:
            story.append(self.paragraph('', 'body'))
        try:
            if self.toc:
                document.multiBuild(story, onFirstPage=page_number, onLaterPages=page_number, maxPasses=4)
            else:
                document.build(story, onFirstPage=page_number, onLaterPages=page_number)
        except LayoutError as error:
            fail('limit', 'an element is too large for one page (%s); split the table or make the image smaller'
                 % str(error)[:120])
        return buffer.getvalue()


def op_create_pdf_fallback(job):
    need(not job.options['latex_available'], 'unsupported',
         'with LaTeX available, create_pdf runs as phases latex_prepare and latex_finalize', 'phase')
    spec = job.spec
    as_dict(spec, 'spec', {'title', 'author', 'date', 'lang', 'toc', 'document_class', 'paper', 'margins_cm',
                           'font_size', 'font', 'markdown', 'blocks'})
    need(spec.get('markdown') is not None or spec.get('blocks') is not None, 'invalid_spec',
         'create_pdf needs spec.markdown or spec.blocks', 'spec.markdown')
    job.output_path(extensions=('pdf',))
    writer = ReportLabWriter(job, spec)
    if spec.get('markdown') is not None:
        writer.block_markdown({'type': 'markdown', 'text': spec['markdown']}, 'spec')
    if spec.get('blocks') is not None:
        writer.blocks(spec['blocks'], 'spec.blocks')
    data = writer.build()
    pages = len(open_pdf(job, data, 'generated PDF').pages)
    missing = writer.missing_characters()
    notes = []
    if missing:
        listed = ', '.join('%s U+%04X' % (char, ord(char)) for char in missing[:12])
        job.warn('%d characters are not in the available font (%s) and show as boxes: %s. Full Unicode output '
                 'needs the LaTeX renderer.' % (len(missing), writer.main[4], listed), 'fallback-missing')
        notes.append('%d missing characters: %s' % (len(missing), listed))
    if writer.cjk_fonts:
        job.warn('CJK text uses non-embedded Adobe fonts (%s); viewers without them substitute a font'
                 % ', '.join(sorted(writer.cjk_fonts)), 'fallback-cjk')
        notes.append('CJK text uses non-embedded fonts')
    job.publish(job.output, data, MEDIA_TYPES['pdf'])
    job.qa = {'pages': pages, 'warnings': notes, 'renderer': 'reportlab',
              'missing_characters': ['%s U+%04X' % (char, ord(char)) for char in missing[:50]]}
    if job.options['preview']:
        render_pdf_previews(job, data, 'generated PDF')
    result = {'pages': pages, 'renderer': 'reportlab', 'font': writer.main[4],
              'missing_characters': len(missing)}
    result.update(writer.counts)
    return result


# ---------------------------------------------------------------------------------------------
# document_read operations
# ---------------------------------------------------------------------------------------------

def plain_text(job, index):
    data = job.data(index)
    need(len(data) <= MAX_TEXT_INPUT_BYTES, 'limit', 'text files are read up to %d MiB' % (
        MAX_TEXT_INPUT_BYTES // MIB))
    if data.startswith(b'\xef\xbb\xbf'):
        return data[3:].decode('utf-8', 'replace'), 'utf-8-sig'
    if data.startswith((b'\xff\xfe', b'\xfe\xff')):
        return data.decode('utf-16', 'replace'), 'utf-16'
    try:
        return data.decode('utf-8'), 'utf-8'
    except UnicodeDecodeError:
        job.warn('The file is not UTF-8; it was read as Windows-1252')
        return data.decode('cp1252', 'replace'), 'cp1252'


def image_summary(job, index):
    image = open_image(job, job.data(index), job.input(index)['path'])
    info = {'format': image.format, 'width': image.width, 'height': image.height, 'mode': image.mode,
            'frames': getattr(image, 'n_frames', 1)}
    if image.info.get('dpi'):
        try:
            info['dpi'] = [round(float(v), 1) for v in image.info['dpi']][:2]
        except (TypeError, ValueError):
            pass
    try:
        exif = image.getexif()
        if exif:
            from PIL import ExifTags
            info['exif_keys'] = sorted({ExifTags.TAGS.get(key, str(key)) for key in exif.keys()})[:80]
            if exif.get(0x0112):
                info['orientation'] = int(exif.get(0x0112))
    except Exception:
        pass
    info['has_alpha'] = 'A' in image.getbands() or 'transparency' in image.info
    return info


def op_text(job):
    index = job.single_input()
    kind = job.kind(index)
    extra = {}
    if kind == 'docx':
        text = docx_text(job, index)
    elif kind == 'xlsx':
        text = xlsx_text(job, index)
    elif kind == 'pptx':
        text = pptx_text(job, index)
    elif kind == 'pdf':
        text, extra = pdf_text(job, index)
    elif kind == 'image':
        info = image_summary(job, index)
        text = ('Image %s, %dx%d pixels, mode %s. Images have no text layer here; use preview to look at it.'
                % (info['format'], info['width'], info['height'], info['mode']))
        extra = {'image': info}
    else:
        text, encoding = plain_text(job, index)
        extra = {'encoding': encoding}
        kind = ext_of(job.input(index)['name']) or 'text'
    base = {'format': kind, 'source_sha256': sha256(job.data(index))}
    base.update(extra)
    return fit_text(job, base, text)


def op_inspect(job):
    index = job.single_input()
    kind = job.kind(index)
    if kind == 'docx':
        return docx_inspect(job, index)
    if kind == 'xlsx':
        return xlsx_inspect(job, index)
    if kind == 'pptx':
        return pptx_inspect(job, index)
    if kind == 'pdf':
        return pdf_inspect(job, index)
    if kind == 'image':
        info = image_summary(job, index)
        return {'format': 'image', 'source_sha256': sha256(job.data(index)), 'items': [info], 'offset': 0,
                'next_offset': None, 'total': 1}
    text, encoding = plain_text(job, index)
    items = [{'line': number, 'text': trunc(line, ITEM_TEXT_CHARS)} for number, line in enumerate(text.split('\n'), 1)]
    return fit_items(job, {'format': ext_of(job.input(index)['name']) or 'text', 'encoding': encoding,
                           'source_sha256': sha256(job.data(index))}, items)


def op_metadata(job):
    index = job.single_input()
    kind = job.kind(index)
    data = job.data(index)
    result = {'format': kind, 'bytes': len(data), 'sha256': sha256(data), 'path': job.input(index)['path']}
    if kind == 'docx':
        result.update(docx_metadata(job, index))
    elif kind == 'xlsx':
        result.update(xlsx_metadata(job, index))
    elif kind == 'pptx':
        result.update(pptx_metadata(job, index))
    elif kind == 'pdf':
        result.update(pdf_metadata(job, index))
    elif kind == 'image':
        result.update(image_summary(job, index))
        result['image_format'], result['format'] = result['format'], 'image'
    else:
        text, encoding = plain_text(job, index)
        result.update({'format': ext_of(job.input(index)['name']) or 'text', 'encoding': encoding,
                       'characters': len(text), 'lines': text.count('\n') + 1})
    if json_size(result) > job.limits['max_result_bytes']:
        result = shrink_item(result, job.limits['max_result_bytes'] - 64)
    return result


def op_preview(job):
    index = job.single_input()
    kind = job.kind(index)
    item = job.input(index)
    if kind == 'pdf':
        data = job.data(index)
        total = len(open_pdf(job, data, item['path']).pages)
        pages = parse_pages(job.options['pages'], total, 'options.pages',
                            default=list(range(min(total, job.limits['max_preview_images']))))
        render_pdf_previews(job, data, item['path'], pages)
        return {'format': 'pdf', 'pages': total, 'rendered': [p['page'] for p in job.previews]}
    if kind == 'image':
        from PIL import ImageOps
        image = ImageOps.exif_transpose(load_image(job, job.data(index), item['path']))
        job.add_preview(image, 'image', 1)
        return {'format': 'image', 'width': image.width, 'height': image.height}
    if kind in ('docx', 'xlsx', 'pptx'):
        fail('unsupported', 'previews of %s files need LibreOffice; the runtime converts them first' % kind)
    fail('unsupported', 'previews are available for PDF, images and (with LibreOffice) Office files')


def op_preview_render(job):
    name = job.options['render_source'] or find_stage_file(job, 'work/office-out', ('pdf',))
    need(name, 'engine', 'phase render_pdf needs options.render_source (work/office-out/<stem>.pdf)')
    path = job.resolve(name, ('work',), True)
    need(os.path.getsize(path) <= MAX_INPUT_BYTES, 'limit', 'the converted PDF is too large')
    with open(path, 'rb') as handle:
        data = handle.read()
    total = len(open_pdf(job, data, 'converted document').pages)
    pages = parse_pages(job.options['pages'], total, 'options.pages',
                        default=list(range(min(total, job.limits['max_preview_images']))))
    render_pdf_previews(job, data, 'converted document', pages)
    return {'format': job.kind(0) if job.inputs else 'pdf', 'pages': total,
            'rendered': [p['page'] for p in job.previews]}


def ocr_models(job, languages):
    """Only the exact local language pack copied by the trusted runtime is accepted."""
    as_list(languages, 'options.languages', 2, 1)
    need(all(type(value) is str and value in OCR_MODELS for value in languages)
         and len(set(languages)) == len(languages), 'invalid_spec',
         'Choose one or both supported OCR languages: deu, eng', 'options.languages')
    for language in languages:
        name = 'in/ocr-models/%s.traineddata' % language
        try:
            path = job.resolve(name, ('in',), True)
        except EngineError:
            fail('ocr_unavailable', 'The local OCR language pack is missing; complete OCR setup first')
        expected, size = OCR_MODELS[language]
        need(os.path.getsize(path) == size, 'ocr_unavailable',
             'The local OCR language pack changed; repair OCR setup first')
        with open(path, 'rb') as handle:
            digest = sha256(handle.read(size + 1))
        need(digest == expected, 'ocr_unavailable',
             'The local OCR language pack changed; repair OCR setup first')
    return os.path.join(job.stage, 'in', 'ocr-models') + os.sep


def ocr_rasters(job, data, kind, selected):
    """Yield one bounded, upright raster at a time, keeping native owners alive."""
    from PIL import Image, ImageOps
    document = None
    image = None
    try:
        if kind == 'pdf':
            import pypdfium2 as pdfium
            document = pdfium.PdfDocument(data)
            document.init_forms()
        else:
            image = open_image(job, data, job.input(0)['path'])
        pixels = 0
        for index in selected:
            bitmap = None
            page = None
            try:
                if document is not None:
                    page = document[index]
                    width, height = page.get_size()
                    scale = job.options['dpi'] / 72
                    need(math.isfinite(width) and math.isfinite(height) and width > 0 and height > 0
                         and width * height * scale * scale <= OCR_MAX_PAGE_PIXELS, 'limit',
                         'OCR page is too large; select a lower DPI', 'options.dpi')
                    bitmap = page.render(scale=scale)
                    raster = bitmap.to_pil().copy()
                else:
                    image.seek(index)
                    need(image.width * image.height <= min(OCR_MAX_PAGE_PIXELS, job.limits['max_image_pixels']),
                         'limit', 'OCR page exceeds the supported pixel limit', 'inputs[0]')
                    raster = ImageOps.exif_transpose(image.copy())
                if 'A' in raster.getbands() or 'transparency' in raster.info:
                    rgba = raster.convert('RGBA')
                    background = Image.new('RGBA', rgba.size, 'white')
                    background.alpha_composite(rgba)
                    raster = background.convert('L')
                else:
                    raster = raster.convert('L')
                if job.options['rotation']:
                    raster = raster.rotate(-job.options['rotation'], expand=True, fillcolor='white')
                pixels += raster.width * raster.height
                need(pixels <= OCR_MAX_TOTAL_PIXELS, 'limit',
                     'The selected OCR pages exceed the total pixel limit; select fewer pages')
                yield index + 1, raster
            finally:
                if bitmap is not None:
                    bitmap.close()
                if page is not None:
                    page.close()
    finally:
        if document is not None:
            document.close()
        if image is not None:
            image.close()


def ocr_words(api, image, page):
    import tesserocr
    api.SetImage(image)
    api.Recognize()
    if not api.GetUTF8Text().strip():
        return []
    iterator = api.GetIterator()
    if iterator is None:
        return []
    words, line = [], 0
    while True:
        if iterator.IsAtBeginningOf(tesserocr.RIL.TEXTLINE):
            line += 1
        text = iterator.GetUTF8Text(tesserocr.RIL.WORD)
        if text and text.strip():
            text = text.strip()
            box = iterator.BoundingBox(tesserocr.RIL.WORD)
            score = iterator.Confidence(tesserocr.RIL.WORD)
            need(box is not None and all(type(n) is int for n in box)
                 and 0 <= box[0] < box[2] <= image.width and 0 <= box[1] < box[3] <= image.height
                 and math.isfinite(score) and 0 <= score <= 100, 'ocr_invalid_result',
                 'OCR returned invalid word coordinates or scores; review this page manually')
            need(len(words) < OCR_MAX_WORDS, 'limit', 'OCR page has too many words')
            words.append({'page': page, 'index': len(words), 'line': line, 'text': text[:256],
                          'text_truncated': len(text) > 256, 'x': box[0], 'y': box[1],
                          'width': box[2] - box[0], 'height': box[3] - box[1], 'score': score})
        if not iterator.Next(tesserocr.RIL.WORD):
            break
    return words


def ocr_text(words):
    chunks, previous = [], None
    for word in words:
        location = word['page'], word['line']
        separator = '' if previous is None else '\n\n' if location[0] != previous[0] else '\n' if location != previous else ' '
        chunks.append(separator + word['text'])
        previous = location
    return ''.join(chunks)


def op_ocr(job):
    index = job.single_input()
    need(job.output is None and not job.spec, 'invalid_spec', 'OCR is a read-only operation')
    kind = job.kind(index)
    need(kind in ('pdf', 'image'), 'unsupported', 'OCR supports PDF and image files', 'inputs[0]')
    need(job.options['rotation'] in (0, 90, 180, 270), 'invalid_spec',
         'OCR rotation must be 0, 90, 180 or 270 degrees clockwise', 'options.rotation')
    languages = job.options['languages']
    models = ocr_models(job, languages)
    try:
        import importlib.metadata
        import tesserocr
        need(importlib.metadata.version('tesserocr') == OCR_BINDING_VERSION, 'ocr_unavailable',
             'The OCR binding version changed; repair OCR setup first')
    except ImportError:
        fail('ocr_unavailable', 'The local OCR runtime is missing; complete OCR setup first')
    data = job.data(index)
    if kind == 'pdf':
        total = len(open_pdf(job, data, job.input(index)['path']).pages)
    else:
        with open_image(job, data, job.input(index)['path']) as image:
            total = getattr(image, 'n_frames', 1)
    selected = parse_pages(job.options['pages'], total, 'options.pages', default=list(range(min(total, 8))))
    need(0 < len(selected) <= OCR_MAX_PAGES, 'invalid_spec', 'Select between 1 and 16 OCR pages', 'options.pages')
    all_words, pages = [], []
    try:
        with tesserocr.PyTessBaseAPI(path=models, lang='+'.join(languages),
                                   oem=tesserocr.OEM.LSTM_ONLY, psm=tesserocr.PSM.AUTO) as api:
            with contextlib.closing(ocr_rasters(job, data, kind, selected)) as rasters:
                for page, image in rasters:
                    words = ocr_words(api, image, page)
                    need(len(all_words) + len(words) <= OCR_MAX_WORDS, 'limit', 'Selected OCR pages have too many words')
                    buffer = io.BytesIO()
                    image.save(buffer, 'PNG')
                    preview = job.add_preview(image, 'ocr-page-%d' % page, page)
                    pages.append({'page': page, 'width': image.width, 'height': image.height,
                                  'dpi': job.options['dpi'], 'rotation': job.options['rotation'],
                                  'raster_sha256': sha256(buffer.getvalue()),
                                  'preview_sha256': job.previews[-1]['sha256'] if preview else None,
                                  'word_count': len(words)})
                    all_words.extend(words)
                    api.Clear()
    except EngineError:
        raise
    except Exception as error:
        fail('ocr_failed', 'OCR could not process the selected page: %s' % type(error).__name__)
    offset = job.options['offset']
    need(offset <= len(all_words), 'invalid_spec', 'OCR offset is beyond the selected words', 'options.offset')
    pages_truncated = job.options['pages'] is None and len(selected) < total
    result = {'format': 'ocr', 'source_sha256': sha256(data),
              'engine': {'name': 'tesseract', 'version': tesserocr.tesseract_version().splitlines()[0],
                         'binding_version': OCR_BINDING_VERSION, 'languages': languages,
                         'models_sha256': {language: OCR_MODELS[language][0] for language in languages}},
              'total_pages': total, 'pages': pages, 'pages_truncated': pages_truncated,
              'next_page': selected[-1] + 2 if pages_truncated else None,
              'total_words': len(all_words), 'offset': offset, 'next_offset': None,
              'words': [], 'text': '', 'truncated': pages_truncated}
    need(json_size(result) <= job.limits['max_result_bytes'], 'limit', 'OCR page metadata exceeds the result budget')
    for word in all_words[offset:offset + job.options['limit']]:
        candidate = {**result, 'words': [*result['words'], word]}
        candidate['text'] = ocr_text(candidate['words'])
        candidate['next_offset'] = offset + len(candidate['words']) if offset + len(candidate['words']) < len(all_words) else None
        candidate['truncated'] = pages_truncated or candidate['next_offset'] is not None or any(w['text_truncated'] for w in candidate['words'])
        if json_size(candidate) > job.limits['max_result_bytes']:
            break
        result = candidate
    need(result['words'] or offset == len(all_words), 'limit', 'One OCR word exceeds the result budget')
    result['next_offset'] = offset + len(result['words']) if offset + len(result['words']) < len(all_words) else None
    result['truncated'] = pages_truncated or result['next_offset'] is not None or any(w['text_truncated'] for w in result['words'])
    if not all_words:
        job.warn('OCR found no readable words; review the source page manually', 'ocr-no-text')
    elif any(word['score'] < 70 for word in result['words']):
        job.warn('Some OCR words have low engine scores; verify them against their page locations. Scores are not probabilities.', 'ocr-low-score')
    if result['truncated']:
        job.warn('OCR result is incomplete; use next_offset with the same selected pages, or select the remaining pages', 'ocr-truncated')
    return result


def op_help(job):
    topic = job.options['topic'] or 'overview'
    need(topic in HELP, 'invalid_spec', 'options.topic must be one of: %s' % ', '.join(HELP), 'options.topic')
    return fit_text(job, {'topic': topic, 'topics': list(HELP)}, HELP[topic].strip() + '\n', field='reference')


# ---------------------------------------------------------------------------------------------
# document_write operations
# ---------------------------------------------------------------------------------------------

def warnings_for_qa(job):
    return [d['message'] for d in job.diagnostics if d['severity'] == 'warning'][:10]


def op_create(job):
    kind = as_str(job.spec.get('format'), 'spec.format', 8, ('docx', 'xlsx', 'pptx'))
    job.output_path(extensions=(kind,))
    data, result = {'docx': docx_create, 'xlsx': xlsx_create, 'pptx': pptx_create}[kind](job, job.spec)
    job.publish(job.output, data, MEDIA_TYPES[kind])
    result.update({'format': kind, 'bytes': len(data)})
    job.qa = {**(job.qa or {}), 'warnings': warnings_for_qa(job)}
    return result


def op_edit(job):
    need(job.inputs, 'invalid_spec', 'edit needs the source document as inputs[0]', 'inputs')
    kind = job.kind(0)
    need(kind in ('docx', 'xlsx', 'pptx'), 'unsupported',
         'edit supports docx, xlsx and pptx; use operation "pdf" for PDFs', 'inputs[0]')
    need(ext_of(job.input(0)['name']) not in MACRO_EXTENSIONS, 'unsupported',
         'macro-enabled documents cannot be edited', 'inputs[0]')
    digest = as_str(job.spec.get('source_sha256'), 'spec.source_sha256', 64).lower()
    if digest != sha256(job.data(0)):
        fail('source_changed', 'the source document changed; run document_read inspect again for the current '
             'source_sha256', 'spec.source_sha256')
    job.output_path(extensions=(kind,))
    data, result = {'docx': docx_edit, 'xlsx': xlsx_edit, 'pptx': pptx_edit}[kind](job, job.spec)
    job.publish(job.output, data, MEDIA_TYPES[kind])
    result.update({'format': kind, 'bytes': len(data), 'source_sha256': digest})
    job.qa = {**(job.qa or {}), 'warnings': warnings_for_qa(job)}
    return result


HANDLERS = {
    ('document_read', 'ocr', None): op_ocr,
    ('document_read', 'text', None): op_text,
    ('document_read', 'inspect', None): op_inspect,
    ('document_read', 'metadata', None): op_metadata,
    ('document_read', 'preview', None): op_preview,
    ('document_read', 'preview', 'render_pdf'): op_preview_render,
    ('document_read', 'help', None): op_help,
    ('document_write', 'create', None): op_create,
    ('document_write', 'edit', None): op_edit,
    ('document_write', 'pdf', None): op_pdf,
    ('document_write', 'create_pdf', None): op_create_pdf_fallback,
    ('document_write', 'create_pdf', 'latex_prepare'): op_latex_prepare,
    ('document_write', 'create_pdf', 'latex_finalize'): op_latex_finalize,
    ('document_write', 'compile_latex', 'latex_finalize'): op_latex_finalize,
    ('document_write', 'convert', 'recalculate_prepare'): op_recalculate_prepare,
    ('document_write', 'convert', 'office_finalize'): op_office_finalize,
}


# ---------------------------------------------------------------------------------------------
# help: the model's spec reference (English, compact, each topic <= 24 KiB).
# ---------------------------------------------------------------------------------------------

HELP = {
    'overview': r'''
# Document tools overview
Three tools work on files in the chat's folders. Outputs are always NEW files; nothing is overwritten.

document_read (read-only), operations:
- text: plain text with pagination. Long results return next_offset; call again with options.offset.
  docx keeps tabs and line breaks; xlsx gives "## Sheet: name" blocks with tab-separated rows;
  pptx follows the visible slide order ("## Slide n", then notes); pdf gives "## Page n" blocks.
- inspect: numbered items + source_sha256. The numbers are what document_write edit patches address.
  docx: paragraphs (global "paragraph" index) and tables; xlsx: cells (options.sheet picks a sheet);
  pptx: slides with shapes (shape ids); pdf: pages and form fields. Page with options.offset/limit (limit <= 500).
- metadata: format, size, pages/sheets/slides, title/author, image size.
- preview: PNG images of pages (options.pages "1-3"), of images, or of Office files (needs LibreOffice).
- ocr: recognize PDF/image pages with the optional local OCR runtime and exact deu/eng language pack.
  languages defaults to ["deu","eng"] (order matters); dpi 100..400 defaults to 200;
  rotation 0/90/180/270 degrees clockwise defaults to 0. PDF rotation and image EXIF are honored.
  Returns source_sha256, page raster sizes/hashes, word boxes and raw engine scores (not probabilities).
  offset/limit paginate WORDS, not characters. text contains only the returned words.
  Default: first 8 pages, at most 16 explicitly selected pages. Follow next_page/next_offset and
  truncated markers; verify low scores or absent words against the source. No automatic OCR fallback.
- help: this reference. options.topic: overview, docx, xlsx, pptx, pdf, latex, image.

document_write (changes files; follows the chat's access mode), operations:
- create: new docx/xlsx/pptx from spec (spec.format). See help docx / xlsx / pptx.
- edit: patch an existing docx/xlsx/pptx into a new file. ALWAYS inspect first and pass
  spec.source_sha256 from that result; if the file changed you get source_changed.
- pdf: merge, split, extract_pages, rotate, reorder, delete_pages, watermark, fill_form, set_metadata.
- create_pdf: a PDF from Markdown or blocks. With LaTeX installed it is typeset by XeLaTeX (full
  Unicode); otherwise a built-in renderer is used (see help latex).
- compile_latex: compile your own .tex file (only when LaTeX is available). Errors come back as
  "file:line: message"; fix and retry.
- convert: Office <-> PDF and other formats (only when LibreOffice is configured).
  For XLSX -> new XLSX use spec.recalculate=true. QA reports formula caches and errors.

image_edit: crop, resize, rotate, annotate and convert images (see help image).

Tips:
- Build long documents in steps: create with the first sections, then edit with append_blocks.
- After creating, check the result with document_read preview (PDF, images) or inspect.
- Strings are always text. In xlsx a value only becomes a formula with "type": "formula".
- Errors name the exact spec path (for example spec.blocks[3].rows) and how to fix it.
''',
    'docx': r'''
# DOCX (Word): create and edit
## create
{"format": "docx",
 "blocks": [ ...blocks... ],          or "markdown": "# Title\n\nText ..." (both may be given)
 "properties": {"title", "author", "subject", "keywords", "description", "category"},
 "page": {"size": "a4|a5|a3|letter|legal", "orientation": "portrait|landscape",
          "margins_cm": 2.5 or {"top", "bottom", "left", "right"}},
 "font": {"name": "Calibri", "size": 11},
 "header": "text" or {"text", "align": "left|center|right"},
 "footer": {"text": "Page {page} of {pages}"},
 "template_input": 1, "keep_template_content": false, "values": {"name": "Ada"}}
Defaults: A4, 2.5 cm margins. template_input is an index into the tool inputs (docx/dotx); its
styles are used and its body is cleared unless keep_template_content is true. values fills
{{name}} placeholders (also in headers and footers).

## Blocks (shared with create_pdf)
- {"type": "heading", "text": "Intro", "level": 1}         level 0 = title, 1..9
- {"type": "paragraph", "text": "Plain text", "align": "left|center|right|justify",
   "style": "Quote", "space_after": 6, "indent_cm": 1}
- {"type": "paragraph", "runs": [{"text": "Bold", "bold": true}, " and ",
                                 {"text": "a link", "link": "https://example.org"}]}
  Run keys: text, bold, italic, underline, strike, code, color ("1F4E79"), highlight (yellow,
  green, cyan, magenta, blue, red, gray, ...), size (pt), font, superscript, subscript,
  link (http, https or mailto), math. "\n" in text is a line break, "\t" a tab.
- {"type": "list", "ordered": false, "start": 1,
   "items": ["One", {"text": "Two", "items": ["Two a"], "ordered": true}]}
- {"type": "table", "rows": [["Name", "Qty"], ["Apple", 3]], "header": true,
   "style": "Table Grid", "widths_cm": [6, 3], "align": ["left", "right"], "caption": "Table 1"}
  A cell may be {"text" or "runs", "bold", "italic", "color", "fill": "D9E2F3", "align", "colspan": 2}.
- {"type": "image", "input": 1, "width_cm": 12, "caption": "Figure 1", "align": "center"}
- {"type": "page_break"}, {"type": "horizontal_rule"}, {"type": "toc", "levels": 3}
- {"type": "code", "text": "print(1)"}, {"type": "quote", "text": "..."},
  {"type": "math", "latex": "a^2+b^2"}  (stays LaTeX source in Word; use create_pdf for typeset math)
- {"type": "markdown", "text": "## Heading\n- item\n\n| a | b |\n|---|---|\n| 1 | 2 |"}
  Markdown subset: # headings, paragraphs, **bold**, *italic*, `code`, ~~strike~~, [text](url),
  - and 1. lists (indent 2+ spaces to nest), > quotes, ``` code, | tables |, ---,
  ![caption](input:1), \newpage, $$ math $$, \( inline math \).

## edit
{"source_sha256": "<from inspect>", "patches": [ ... ], "properties": {...}}
Indices come from document_read inspect: "paragraph" counts every paragraph of the body (also
those inside tables) in document order; "table" counts tables. Indices always refer to the
inspected source, even after earlier patches inserted or deleted content.
- {"op": "replace_paragraph", "paragraph": 4, "text": "New text"}  keeps the formatting;
  "runs" instead of "text" sets new formatting; optional "style".  ({"paragraph", "text"} also works)
- {"op": "insert_paragraph", "after": 4, "text": "...", "style": "Heading 2"}   (or "before")
- {"op": "insert_blocks", "after": 4, "blocks": [ ...blocks... ]}
- {"op": "append_blocks", "blocks": [...]} or {"op": "append_blocks", "markdown": "..."}
- {"op": "delete_paragraph", "paragraph": 7}, {"op": "delete_table", "table": 0}
- {"op": "set_style", "paragraph": 2, "style": "Heading 1"}
- {"op": "set_cell", "table": 0, "row": 1, "col": 2, "text": "42"}
- {"op": "add_table_row", "table": 0, "values": ["a", "b"], "after_row": 3}  (copies that row's format)
- {"op": "delete_table_row", "table": 0, "row": 2}
- {"op": "insert_image", "after": 3, "input": 1, "width_cm": 10}
- {"op": "insert_page_break", "after": 9}
- {"op": "fill_placeholders", "values": {"customer": "ACME", "date": "18.10.2026"}}   fills {{customer}}
- {"op": "replace_text", "find": "2025", "replace": "2026", "match_case": true, "max": 10}
- {"op": "set_properties", "title": "...", "author": "..."}
Placeholders and find text may span differently formatted runs.
''',
    'xlsx': r'''
# XLSX (Excel): create and edit
## create
{"format": "xlsx", "properties": {"title", "author", ...}, "template_input": 1, "values": {...},
 "sheets": [
  {"name": "Sales",
   "rows": [["Region", "Q1", "Q2", "Total"],
            ["North", 120, 98.5, {"value": "=SUM(B2:C2)", "type": "formula"}],
            ["Date", {"value": "2026-10-18", "type": "date"}, {"value": 0.25, "type": "percent"}, null]],
   "start": "A1",
   "header": true,                      first row bold with fill and border
   "cells": [{"cell": "F1", "value": "Note"}],
   "widths": {"A": 18, "B": 12},        default: fitted to the content
   "number_formats": {"B": "#,##0.00"}, applied to the cells written in that column
   "freeze": "A2", "autofilter": true or "A1:D20",
   "styles": [{"range": "A1:D1", "bold": true, "fill": "D9E1F2", "color": "000000", "align": "center",
               "valign": "center", "wrap": true, "border": "thin|medium|thick|none", "size": 12,
               "font": "Arial", "underline": true, "italic": true, "number_format": "0.0%"}],
   "merge": ["A10:D10"],
   "table": {"name": "SalesTable", "range": "A1:D20", "style": "TableStyleMedium2"},
   "charts": [{"type": "column|bar|stacked_column|stacked_bar|line|pie|doughnut|area|scatter",
               "data": "B1:C5", "categories": "A2:A5", "title": "Revenue", "anchor": "F2",
               "x_title": "Region", "y_title": "EUR", "width_cm": 16, "height_cm": 8,
               "titles_from_data": true, "legend": true}]}]}
Cell values: a JSON string is text (never a formula, even if it starts with "="), a number,
true/false, null (empty), or {"value", "type", "number_format", "bold", "italic", "color", "fill",
"align", "wrap", "border", "size"}. type: text | number | boolean | formula | date | datetime |
time | percent | empty. Dates are ISO (2026-10-18, 2026-10-18T14:30). Formulas start with "=" and
use English function names with commas (=SUM(A1:A3), =IF(B2>0,"yes","no")). Network, macro, DDE
and other-workbook formulas are refused. Ranges may name a sheet: "Data!B2:B9", "'My Sheet'!A1:B2".
For charts "data" includes the header row when titles_from_data is true (default); "categories"
are the labels (for scatter: the x values).

## edit
{"source_sha256": "<from inspect>", "patches": [...]}   Cells are addressed like "B7".
- {"op": "set_cell", "sheet": "Sales", "cell": "B7", "value": 12, "type": "number", "number_format": "0.00"}
  ({"sheet", "cell", "value"} also works; strings stay text unless "type": "formula")
- {"op": "set_range", "sheet": "Sales", "start": "A10", "rows": [["a", 1], ["b", 2]]}
- {"op": "append_rows", "sheet": "Sales", "rows": [["West", 80, 75]]}      after the last used row
- {"op": "append_table_rows", "sheet": "Invoices", "table": "InvoiceTable",
   "columns": [{"header": "Invoice", "kind": "text", "nullable": false},
               {"header": "Gross", "kind": "money_minor", "nullable": false}],
   "key_columns": [0], "rows": [["INV-002", 11900]]}
  Data-only append to an existing named Excel table, as the only patch in this action.
  Headers must match exactly in table order. 1-32 columns, 1-100 rows and 1-4 unique
  zero-based key columns; composite keys must be present and unique in the source and batch.
  Column kinds: text, integer, number, boolean, date (YYYY-MM-DD, 1900 or later), money_minor.
  money_minor takes whole minor units: 11900 writes numeric 119.00; at most 9999999999999
  minor units in either direction. Null needs nullable:true; empty text and formula
  instructions are refused. Strings beginning with '=' remain text.
  Protected, totals/calculated-column tables, merged/overlapping ranges, internal empty
  rows and occupied destination cells require review. The table/filter range is extended
  and saved values, types, formats and range are reopened and checked before publication.
  result.table_append identifies sheet/table, rows_added, first_row/last_row,
  previous_range/range and key_columns; QA reports the verified rows. The output is a new
  workbook: explicitly adopt its version before another event builds on the table.

- {"op": "add_sheet", "name": "Summary", "position": 0, ...same keys as a create sheet...}
- {"op": "delete_sheet", "sheet": "Old"}, {"op": "rename_sheet", "sheet": "Old", "name": "New"}
- {"op": "insert_rows"|"delete_rows"|"insert_cols"|"delete_cols", "sheet": "Sales", "index": 5, "count": 2}
  A1 references in all sheets and defined names are rewritten, including absolute references,
  row/column ranges and quoted sheet names. Deleted references become #REF!.
- {"op": "style", "sheet": "Sales", "range": "A1:D1", "bold": true, "fill": "FFF2CC"}
- {"op": "merge"|"unmerge", "sheet": "Sales", "range": "A1:D1"}
- {"op": "freeze", "sheet": "Sales", "cell": "A2"}, {"op": "autofilter", "sheet": "Sales", "range": "A1:D20"}
- {"op": "widths", "sheet": "Sales", "widths": {"A": 30}}
- {"op": "number_formats", "sheet": "Sales", "number_formats": {"C": "0%"}}   (existing cells)
- {"op": "add_chart", "sheet": "Sales", "type": "line", "data": "B1:C9", "categories": "A2:A9", "anchor": "F2"}
- {"op": "add_table", "sheet": "Sales", "name": "T1", "range": "A1:D9"}
- {"op": "fill_placeholders", "values": {"name": "value"}}     replaces {{name}} in text cells
Cell edits preserve supported charts/images; QA checks retained package parts and image bytes.
Unsupported extensions, shapes, embedded objects and lost parts are refused before publication.
Structural editing is refused for tables, charts, validation/conditional-format rules, external/3D/
spill references, global relative names and moved/internal hyperlinks. Row/column edits also refuse
anchored images, merged cells, panes, filters and print ranges; use a spreadsheet application for
those workbooks. Source files are never changed. Recalculate formula values explicitly afterwards.
''',
    'pptx': r'''
# PPTX (PowerPoint): create and edit
## create
{"format": "pptx", "aspect": "16:9|4:3", "properties": {...}, "template_input": 1,
 "keep_template_slides": false, "values": {...},
 "slides": [
  {"layout": "title", "title": "Quarterly review", "subtitle": "October 2026"},
  {"title": "Agenda", "bullets": ["Results", {"text": "Details", "level": 1}, "Next steps"],
   "notes": "Speaker notes"},
  {"title": "Revenue", "chart": {"type": "column", "categories": ["Q1", "Q2"],
            "series": [{"name": "2026", "values": [10, 12]}], "title": "Revenue",
            "legend": true, "data_labels": true, "number_format": "0"}},
  {"title": "Photo and text", "bullets": ["Left text"], "image": {"input": 1}},
  {"title": "Table", "table": {"rows": [["A", "B"], ["1", "2"]], "header": true, "font_size": 14}},
  {"layout": "two_content", "title": "Compare", "left": {"bullets": ["a"]}, "right": {"image": 2}},
  {"layout": "section", "title": "Part 2"}]}
Slide keys: layout (title | section | content | two_content | comparison | title_only | blank),
layout_name (exact layout name of a template), title, subtitle, bullets, text, image, table,
chart, left, right, notes. Without layout the engine picks one from the content. An area (the
body, left or right) holds bullets and/or text plus at most one of image, table, chart; with
both, the text goes left and the visual right. Bullets are strings or
{"text" or "runs", "level": 0-8, "items": [...]}. Run keys: text, bold, italic, underline,
strike, code, color, size, font, link. Chart types: column, stacked_column, bar, stacked_bar,
line, pie, doughnut, area, scatter (scatter series use "points": [[x, y], ...]).
Images: {"input": 1} or with left_cm/top_cm/width_cm/height_cm. Empty placeholders are removed.
Keep slides short; the engine warns when text may overflow.

## edit
{"source_sha256": "<from inspect>", "patches": [...]}
Slide indices are 0-based in the visible order shown by inspect; shape ids come from inspect.
Indices always refer to the inspected source, even after earlier patches.
- {"op": "set_text", "slide": 0, "shape": 2, "text": "New title"}       keeps the formatting
  or "paragraphs": ["line", {"text": "sub point", "level": 1}]   ({"slide", "shape", "text"} also works)
- {"op": "add_slide", "after": 2, ...create slide keys...}   after: -1 = first; default: at the end
- {"op": "delete_slide", "slide": 3}, {"op": "move_slide", "slide": 3, "after": 0}   (-1 = first)
- {"op": "set_notes", "slide": 1, "text": "..."}, {"op": "delete_shape", "slide": 1, "shape": 5}
- {"op": "insert_image", "slide": 1, "input": 1, "left_cm": 2, "top_cm": 4, "width_cm": 12}
- {"op": "add_table", "slide": 1, "rows": [[...]]}, {"op": "add_chart", "slide": 1, "type": "pie", ...}
- {"op": "add_textbox", "slide": 1, "text": "Note", "left_cm": 2, "top_cm": 15, "width_cm": 10}
- {"op": "fill_placeholders", "values": {...}}, {"op": "replace_text", "find": "a", "replace": "b"}
- {"op": "set_properties", "title": "..."}
''',
    'pdf': r'''
# PDF operations (document_write operation "pdf")
spec.action selects the operation. Pages are 1-based: "1-3,5", "4-" (to the end), [1, 2] or "all".
Inputs are PDFs in the order given to the tool. Password-protected PDFs are refused.
- merge: {"action": "merge"} merges all inputs in order, or
  {"action": "merge", "items": [{"input": 0, "pages": "1-2"}, {"input": 1}], "properties": {"title": "..."}}
- split: {"action": "split", "every": 2} or {"action": "split", "ranges": ["1-3", "4-10"]}
  The output name is a folder (no .pdf); part-01.pdf, part-02.pdf ... are created inside.
- extract_pages: {"action": "extract_pages", "pages": "2,5-6"}
  Output ending in .pdf: one PDF with these pages. Output folder: one file per page.
- rotate: {"action": "rotate", "degrees": 90, "pages": "1-2"}   90, 180, 270 or -90; default all pages
- reorder: {"action": "reorder", "order": [3, 1, 2]}             must list every page once
- delete_pages: {"action": "delete_pages", "pages": "2,4"}
- watermark: {"action": "watermark", "text": "DRAFT", "opacity": 0.18, "angle": 45, "size": 72,
   "color": "808080", "position": "over|under", "pages": "all"}   text: Latin-1 characters
   or {"action": "watermark", "stamp_input": 1}   stamps page 1 of PDF input 1 onto the pages
- fill_form: {"action": "fill_form", "fields": {"Name": "Ada", "Agree": true}, "flatten": false}
   Field names come from document_read inspect (form_fields). Checkboxes take true/false.
- set_metadata: {"action": "set_metadata", "properties": {"title", "author", "subject", "keywords", "creator"}}
Reading: document_read text gives "## Page n" blocks (scanned PDFs need operation ocr and its
optional local runtime), inspect lists pages and form fields, preview renders pages as PNG (options.pages).
''',
    'latex': r'''
# PDF through LaTeX
## create_pdf (recommended): Markdown or blocks -> typeset PDF
{"title": "Report", "author": "Ada", "date": "today" | "18. Oktober 2026" | "",
 "lang": "de|en", "toc": false, "document_class": "article|report|scrartcl|scrreprt",
 "paper": "a4|a5|letter|legal", "margins_cm": 2.5, "font_size": 10|11|12, "font": "serif|sans",
 "markdown": "# Intro\nText with **bold**, `code` and \\(E=mc^2\\).\n\n$$\\int_0^1 x\\,dx$$",
 "blocks": [ ...the same blocks as in help docx... ]}
All text is escaped for you: write plain text, never LaTeX commands, in text and Markdown.
Math is the only raw LaTeX: {"type": "math", "latex": "a^2+b^2=c^2"}, a run {"math": "x_i"},
or in Markdown \( inline \) and $$ display $$. Math may use matrix, pmatrix, bmatrix, cases,
aligned, gathered, split and array; file, definition and catcode commands are refused.
Images: {"type": "image", "input": 1, "width_cm": 10, "caption": "..."} (PNG, JPEG or PDF inputs).
Unicode works: Latin, Greek and Cyrillic (Noto fonts), Chinese (Fandol), Japanese (Harano Aji),
Korean (Un fonts). Characters a font lacks are reported as missing characters.
Ignored in PDFs: run font and highlight, table cell fill (LaTeX renderer).

Without LaTeX, create_pdf uses a built-in renderer with the same spec: headings, paragraphs and
runs, lists, tables (cell fill works), images, page breaks, table of contents, page numbers and
title/author metadata. Differences: math is shown as LaTeX source; the font covers Latin text
(more scripts only if Noto/DejaVu fonts are installed); Chinese/Japanese/Korean use fonts of
the PDF viewer. The result reports "renderer": "reportlab" and lists missing characters.

## compile_latex: your own .tex file
Write the .tex with the file tools, then call compile_latex with it (images and \input files
next to it). It is compiled with XeLaTeX offline: only packages in the local cache work
(article, report, scrartcl, scrreprt, beamer, amsmath, amssymb, graphicx, tikz, booktabs,
tabularx, longtable, siunitx, hyperref, babel, fontspec, geometry, xcolor, enumitem, fancyhdr,
caption, float, microtype ...). Use fontspec and \usepackage[ngerman]{babel}; inputenc and
fontenc are not needed. Load fonts by file name, e.g. \setmainfont{NotoSerif-Regular.ttf} or
\setmainfont{texgyretermes-regular.otf}; system font names do not work.
\input, \include and \includegraphics may only use files next to the main file (relative paths,
no "..", no absolute paths). Shell escape is disabled.
Errors come back as diagnostics like "main.tex:12: Undefined control sequence \foo"; fix that line
and compile again. Warnings report overfull boxes (text wider than the line), missing characters,
undefined references and empty pages. Look at pages with document_read preview on the PDF.
''',
    'image': r'''
# image_edit
{"steps": [ ... ], "format": "png|jpeg|webp|gif|bmp|tiff", "quality": 90, "keep_exif": false}
inputs[0] is the image to edit; inputs[1..] are optional overlay images. At most 50 steps.
The output format follows the output file extension unless "format" is given. EXIF metadata
(GPS position, camera) is removed unless keep_exif is true. Coordinates are pixels from the top
left corner. Colors: "#FF0000", "#FF000080" (with alpha) or names like "red", "white".
Steps:
- {"op": "crop", "box": [left, top, right, bottom]}
- {"op": "crop_relative", "box": [0.1, 0.1, 0.9, 0.9]}           fractions of width and height
- {"op": "resize", "width": 800, "height": 600, "fit": "contain|cover|stretch"}   one side alone keeps the aspect
- {"op": "thumbnail", "max_edge": 512}
- {"op": "rotate", "degrees": 90, "expand": true, "fill": "white"}  counter-clockwise
- {"op": "flip", "direction": "horizontal|vertical"}
- {"op": "auto_orient"}                                              applies the EXIF orientation
- {"op": "adjust", "brightness": 1.1, "contrast": 1.2, "saturation": 0.9, "sharpness": 1.5}  1 = unchanged
- {"op": "grayscale"}, {"op": "blur", "radius": 2}, {"op": "sharpen", "amount": 1.0}
- {"op": "text", "text": "Hello", "x": 20, "y": 20, "size": 32, "color": "white",
   "background": "#00000080", "anchor": "top_left|top|top_right|left|center|right|bottom_left|bottom|bottom_right"}
   (built-in font; best for Latin text)
- {"op": "arrow", "from": [10, 10], "to": [200, 120], "color": "red", "width": 6}
- {"op": "rectangle", "box": [left, top, right, bottom], "color": "red", "width": 4, "fill": "#FF000040"}
- {"op": "ellipse", "box": [left, top, right, bottom], "color": "red", "width": 4, "fill": null}
- {"op": "pad", "all": 20, "color": "white"} or {"left", "top", "right", "bottom"} or {"size": [1200, 800]}
- {"op": "border", "width": 10, "color": "black"}
- {"op": "overlay", "input": 1, "x": 10, "y": 10, "opacity": 0.8, "scale": 0.5}
- {"op": "convert", "mode": "RGB|RGBA|L|LA|P|1|CMYK"}
The result reports width, height, format and bytes; set options.preview for a preview image.
''',
}


# ---------------------------------------------------------------------------------------------
# Entry points
# ---------------------------------------------------------------------------------------------

def sanitize_message(message, stage):
    text = str(message)
    if stage:
        for variant in {stage, stage.replace('\\', '/'), stage.replace('/', '\\')}:
            text = text.replace(variant, '<stage>')
    return trunc(text, 500)


def emit(stream, receipt):
    data = json.dumps(receipt, ensure_ascii=True, separators=(',', ':'), allow_nan=False).encode('ascii')
    if len(data) > MAX_RECEIPT_BYTES:
        data = json.dumps({'format': RECEIPT_FORMAT, 'ok': False, 'engine_version': ENGINE_VERSION,
                           'error': {'code': 'limit', 'message': 'the receipt would exceed 1 MiB'},
                           'diagnostics': []}).encode('utf-8')
    stream.write(data + b'\n')
    stream.flush()


def probe():
    import importlib
    import importlib.metadata
    modules, missing, mismatched, checks = {}, [], {}, {}
    for module, (distribution, pinned) in PINNED.items():
        try:
            importlib.import_module(module)
            version = importlib.metadata.version(distribution)
            modules[module] = version
            if version != pinned:
                mismatched[module] = version
        except Exception:
            missing.append(module)
    if 'PIL' not in missing:
        from PIL import features
        checks['freetype'] = bool(features.check('freetype2'))
        if not checks['freetype']:
            missing.append('PIL.freetype')
    if 'pypdfium2' not in missing:
        try:
            import pypdfium2 as pdfium
            pdfium.PdfDocument.new().close()
            checks['pdfium'] = True
        except Exception:
            checks['pdfium'] = False
            missing.append('pypdfium2.pdfium')
    receipt = {'format': PROBE_FORMAT, 'ok': not missing, 'engine_version': ENGINE_VERSION,
               'python': '%d.%d.%d' % sys.version_info[:3], 'modules': modules, 'missing': missing,
               'checks': checks}
    if mismatched:
        receipt['version_mismatch'] = mismatched
    return receipt


def load_request(stage):
    path = os.path.join(stage, 'request.json')
    need(os.path.isfile(path), 'invalid_spec', 'request.json is missing')
    need(os.path.getsize(path) <= MAX_REQUEST_BYTES, 'limit', 'request.json is larger than 16 MiB')
    with open(path, 'rb') as handle:
        raw = handle.read(MAX_REQUEST_BYTES + 1)

    def constant(name):
        fail('invalid_spec', 'request.json contains %s, which is not valid JSON' % name)
    try:
        return json.loads(raw.decode('utf-8-sig'), parse_constant=constant)
    except EngineError:
        raise
    except RecursionError:
        fail('invalid_spec', 'request.json is nested too deeply')
    except (UnicodeDecodeError, ValueError) as error:
        fail('invalid_spec', 'request.json is not valid JSON: %s' % str(error)[:120])


def run(stage_argument):
    stage = os.path.realpath(stage_argument)
    job = None
    try:
        need(os.path.isdir(stage), 'engine', 'the stage directory does not exist')
        sys.addaudithook(Sandbox(stage))
        os.makedirs(os.path.join(stage, 'work', 'tmp'), exist_ok=True)
        tempfile.tempdir = os.path.join(stage, 'work', 'tmp')
        job = Job(stage, load_request(stage))
        warnings.showwarning = job.capture_warning
        handler = HANDLERS.get((job.tool, job.operation, job.phase))
        if handler is None:
            need(job.tool != 'image_edit', 'unsupported', 'image_edit runs in image_engine.py', 'tool')
            known = sorted('%s%s' % (op, '[%s]' % phase if phase else '') for tool, op, phase in HANDLERS
                           if tool == job.tool)
            fail('unsupported', '%s operation "%s"%s is not supported; known: %s' % (
                job.tool, job.operation, ' with phase "%s"' % job.phase if job.phase else '', ', '.join(known)),
                'operation')
        result = handler(job)
        need(json_size(result) <= job.limits['max_result_bytes'], 'limit', 'the result exceeds max_result_bytes')
        receipt = {'format': RECEIPT_FORMAT, 'ok': True, 'engine_version': ENGINE_VERSION, 'result': result,
                   'outputs': job.outputs, 'previews': job.previews, 'diagnostics': job.diagnostics}
        if job.qa is not None:
            receipt['qa'] = job.qa
        return receipt, 0
    except EngineError as error:
        code, message, path = error.code, error.message, error.path
    except MemoryError:
        code, message, path = 'limit', 'the document needs more memory than allowed', None
    except RecursionError:
        code, message, path = 'limit', 'the document is nested too deeply', None
    except PermissionError as error:
        code, message, path = 'engine', 'blocked: %s' % error, None
    except Exception as error:
        code, message, path = 'engine', 'internal engine error: %s: %s' % (type(error).__name__, error), None
    error = {'code': code, 'message': sanitize_message(message, stage)}
    if path:
        error['path'] = path
    if job is not None:
        out_root = os.path.join(stage, 'out')
        for output in job.outputs:
            target = os.path.join(stage, *output['name'].split('/'))
            try:
                os.remove(target)
            except OSError:
                pass
            parent = os.path.dirname(target)
            while parent.startswith(out_root + os.sep):
                try:
                    os.rmdir(parent)
                except OSError:
                    break
                parent = os.path.dirname(parent)
    return {'format': RECEIPT_FORMAT, 'ok': False, 'engine_version': ENGINE_VERSION, 'error': error,
            'diagnostics': job.diagnostics if job else []}, 2


def initialize_builtin_mime_types():
    # openpyxl creates a MimeTypes instance while importing. Python 3.12's first
    # init([]) still reads knownfiles and the Windows registry. Use only built-in
    # mappings, without adding host MIME files to the document read boundary.
    import mimetypes
    knownfiles = mimetypes.knownfiles
    registry_reader = mimetypes.MimeTypes.read_windows_registry
    try:
        mimetypes.knownfiles = []
        mimetypes.MimeTypes.read_windows_registry = lambda self, strict=True: None
        mimetypes.init(files=[])
    finally:
        mimetypes.knownfiles = knownfiles
        mimetypes.MimeTypes.read_windows_registry = registry_reader


def main(argv):
    sys.dont_write_bytecode = True
    initialize_builtin_mime_types()
    stream = sys.stdout.buffer
    sys.stdout = sys.stderr
    if len(argv) == 2 and argv[1] == '--probe':
        sys.addaudithook(Sandbox(None))
        receipt = probe()
        emit(stream, receipt)
        return 0 if receipt['ok'] else 3
    if len(argv) != 2 or argv[1].startswith('-'):
        sys.stderr.write('usage: python -I document_engine.py <stage> | --probe\n')
        return 64
    receipt, code = run(argv[1])
    emit(stream, receipt)
    return code


if __name__ == '__main__':
    sys.exit(main(sys.argv))
