"""LocalAI Cowork image engine for the image_edit tool.

Usage: python -I image_engine.py <stage>   |   python -I image_engine.py --probe

One self-contained file: standard library plus Pillow. No network and no subprocess; an audit hook
confines reads to the stage and the Python installation and writes to <stage>/work and <stage>/out.
Contract: .local/document-tools-20261018/INTERFACE.md (operation "pipeline"). EXIF and other
metadata are removed unless spec.keep_exif is true. Text uses Pillow's built-in font only.
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import os
import re
import sys
import tempfile
import warnings

ENGINE_VERSION = '1'
REQUEST_FORMAT = 'cowork-document-request/1'
RECEIPT_FORMAT = 'cowork-document-receipt/1'
PROBE_FORMAT = 'cowork-document-probe/1'
MIB = 1024 * 1024
DEFAULT_LIMITS = {'max_output_bytes': 64 * MIB, 'max_outputs': 64, 'max_result_bytes': 32768,
                  'max_preview_images': 4, 'preview_max_edge': 1600, 'preview_max_bytes': 2 * MIB,
                  'max_image_pixels': 50_000_000}
LIMIT_CAPS = {'max_output_bytes': 256 * MIB, 'max_outputs': 256, 'max_result_bytes': 512 * 1024,
              'max_preview_images': 16, 'preview_max_edge': 4096, 'preview_max_bytes': 8 * MIB,
              'max_image_pixels': 200_000_000}
MAX_REQUEST_BYTES = 4 * MIB
MAX_INPUT_BYTES = 64 * MIB
MAX_INPUTS = 16
MAX_STEPS = 50
MAX_DIAGNOSTICS = 40
INPUT_FORMATS = ('PNG', 'JPEG', 'GIF', 'BMP', 'TIFF', 'WEBP', 'ICO', 'PPM')
OUTPUT_FORMATS = {'png': 'PNG', 'jpg': 'JPEG', 'jpeg': 'JPEG', 'webp': 'WEBP', 'gif': 'GIF', 'bmp': 'BMP',
                  'tif': 'TIFF', 'tiff': 'TIFF'}
MEDIA_TYPES = {'PNG': 'image/png', 'JPEG': 'image/jpeg', 'WEBP': 'image/webp', 'GIF': 'image/gif', 'BMP': 'image/bmp',
               'TIFF': 'image/tiff'}
ANCHORS = {'top_left': ('l', 't'), 'top': ('m', 't'), 'top_right': ('r', 't'), 'left': ('l', 'm'),
           'center': ('m', 'm'), 'right': ('r', 'm'), 'bottom_left': ('l', 'b'), 'bottom': ('m', 'b'),
           'bottom_right': ('r', 'b')}
STEP_KEYS = {
    'crop': {'box'}, 'crop_relative': {'box'}, 'resize': {'width', 'height', 'fit'}, 'thumbnail': {'max_edge'},
    'rotate': {'degrees', 'expand', 'fill'}, 'flip': {'direction'}, 'auto_orient': set(),
    'adjust': {'brightness', 'contrast', 'saturation', 'sharpness'}, 'grayscale': set(), 'blur': {'radius'},
    'sharpen': {'amount'}, 'text': {'text', 'x', 'y', 'size', 'color', 'background', 'anchor', 'padding'},
    'arrow': {'from', 'to', 'color', 'width'}, 'rectangle': {'box', 'color', 'width', 'fill'},
    'ellipse': {'box', 'color', 'width', 'fill'}, 'pad': {'all', 'left', 'top', 'right', 'bottom', 'size', 'color'},
    'border': {'width', 'color'}, 'overlay': {'input', 'x', 'y', 'opacity', 'scale'}, 'convert': {'mode'}}


class EngineError(Exception):
    def __init__(self, code, message, path=None):
        super().__init__(message)
        self.code, self.message, self.path = code, message, path


def fail(code, message, path=None):
    raise EngineError(code, message, path)


def need(condition, code, message, path=None):
    if not condition:
        raise EngineError(code, message, path)


# ---------------------------------------------------------------------------------------------
# Sandbox (same policy as document_engine.py; each adapter must be one self-contained file).
# ---------------------------------------------------------------------------------------------

_WRITE_EVENTS = {'os.mkdir': (0,), 'os.remove': (0,), 'os.rmdir': (0,), 'os.rename': (0, 1), 'os.truncate': (0,),
                 'os.chmod': (0,), 'os.chown': (0,), 'os.utime': (0,), 'os.link': (0, 1), 'os.symlink': (0, 1),
                 'shutil.rmtree': (0,), 'shutil.copyfile': (1,), 'shutil.copytree': (1,), 'shutil.move': (1,)}
_READ_EVENTS = {'os.listdir': (0,), 'os.scandir': (0,), 'glob.glob': (0,), 'shutil.copyfile': (0,),
                'shutil.copytree': (0,)}
_DENIED_EVENTS = ('subprocess.', 'os.system', 'os.exec', 'os.posix_spawn', 'os.spawn', 'os.startfile', 'os.fork',
                  'os.forkpty', 'os.kill', 'os.killpg', 'pty.', 'socket.', 'urllib.', 'http.client.', 'ftplib.',
                  'smtplib.', 'imaplib.', 'poplib.', 'nntplib.', 'telnetlib.', 'webbrowser.', 'winreg.Create',
                  'winreg.Delete', 'winreg.Set', 'winreg.Connect')
_WRITE_FLAGS = os.O_WRONLY | os.O_RDWR | os.O_APPEND | os.O_CREAT | os.O_TRUNC


class Sandbox:
    def __init__(self, stage):
        self.busy = False
        roots = {self.norm(value) for value in (sys.prefix, sys.base_prefix, sys.exec_prefix, sys.base_exec_prefix,
                                                *sys.path) if value}
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
        return any(path == root or path.startswith(root.rstrip('\\/') + os.sep) for root in roots)

    def check(self, value, write, event):
        if value is None or isinstance(value, int) or not isinstance(value, (str, bytes, os.PathLike)):
            return
        path = self.norm(value)
        if write and not self.inside(path, self.writable):
            raise PermissionError('sandbox: %s may only write below <stage>/work or <stage>/out' % event)
        if not write and not self.inside(path, self.readable):
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
                raise PermissionError('sandbox: %s is not allowed in the image engine' % event)
        finally:
            self.busy = False


# ---------------------------------------------------------------------------------------------
# Validation helpers
# ---------------------------------------------------------------------------------------------

def as_dict(value, path, allowed=None, required=()):
    need(isinstance(value, dict), 'invalid_spec', '%s must be an object' % path, path)
    if allowed is not None:
        extra = sorted(set(value) - set(allowed))
        if extra:
            fail('invalid_spec', 'unknown key %s.%s; allowed keys: %s' % (path, extra[0], ', '.join(sorted(allowed))),
                 '%s.%s' % (path, extra[0]))
    for key in required:
        need(value.get(key) is not None, 'invalid_spec', '%s.%s is required' % (path, key), '%s.%s' % (path, key))
    return value


def as_list(value, path, max_len, min_len=0):
    need(isinstance(value, list), 'invalid_spec', '%s must be a list' % path, path)
    need(len(value) >= min_len, 'invalid_spec', '%s needs at least %d entries' % (path, min_len), path)
    need(len(value) <= max_len, 'limit', '%s has %d entries; the limit is %d' % (path, len(value), max_len), path)
    return value


def as_int(value, path, lo=None, hi=None):
    if type(value) is float and value.is_integer():
        value = int(value)
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


def as_str(value, path, max_chars=10_000, choices=None):
    need(isinstance(value, str), 'invalid_spec', '%s must be a string' % path, path)
    need(len(value) <= max_chars, 'limit', '%s is longer than %d characters' % (path, max_chars), path)
    if choices is not None:
        need(value in choices, 'invalid_spec', '%s must be one of: %s' % (path, ', '.join(choices)), path)
    return value


def opt(obj, key, path, fn, *args, default=None):
    if obj.get(key) is not None:
        return fn(obj[key], '%s.%s' % (path, key), *args)
    return default


def ext_of(name):
    base = name.replace('\\', '/').rsplit('/', 1)[-1]
    return base.rsplit('.', 1)[-1].lower() if '.' in base else ''


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def trunc(text, limit):
    return text if len(text) <= limit else text[:limit] + '...'


_RESERVED_NAME = re.compile(r'(?i)^(con|prn|aux|nul|com[0-9]|lpt[0-9]|conin\$|conout\$)(\..*)?$')


# ---------------------------------------------------------------------------------------------
# Job
# ---------------------------------------------------------------------------------------------

class Job:
    def __init__(self, stage, request):
        self.stage = stage
        self.diagnostics, self.outputs, self.previews = [], [], []
        self._warned = set()
        as_dict(request, 'request')
        need(request.get('format') == REQUEST_FORMAT, 'invalid_spec', 'request.format must be %s' % REQUEST_FORMAT,
             'format')
        self.tool = as_str(request.get('tool'), 'tool', 64)
        self.operation = as_str(request.get('operation'), 'operation', 64)
        need(self.tool == 'image_edit' and self.operation == 'pipeline' and request.get('phase') is None,
             'unsupported', 'image_engine.py runs image_edit operation "pipeline" only', 'operation')
        self.spec = as_dict(request.get('spec') or {}, 'spec')
        self.limits = dict(DEFAULT_LIMITS)
        if request.get('limits') is not None:
            for key, cap in LIMIT_CAPS.items():
                if as_dict(request['limits'], 'limits').get(key) is not None:
                    self.limits[key] = min(as_int(request['limits'][key], 'limits.' + key, 1), cap)
        options = as_dict(request.get('options') or {}, 'options')
        self.preview = opt(options, 'preview', 'options', as_bool, default=False)
        self.inputs = []
        for index, item in enumerate(as_list(request.get('inputs') or [], 'inputs', MAX_INPUTS)):
            path = 'inputs[%d]' % index
            as_dict(item, path, required=('name',))
            name = as_str(item['name'], path + '.name', 1024)
            self.resolve(name, ('in',), True)
            digest = item.get('sha256')
            need(digest is None or (isinstance(digest, str) and re.fullmatch(r'[0-9a-fA-F]{64}', digest)),
                 'invalid_spec', '%s.sha256 is invalid' % path, path)
            self.inputs.append({'name': name, 'path': item.get('path') or name,
                                'sha256': digest.lower() if digest else None, 'bytes': item.get('bytes')})
        output = request.get('output')
        need(output is not None, 'invalid_spec', 'image_edit needs output.name', 'output')
        self.output = as_str(as_dict(output, 'output', required=('name',))['name'], 'output.name', 1024)

    def resolve(self, name, areas, must_exist):
        norm = name.replace('\\', '/')
        parts = norm.split('/')
        need(name and '\0' not in name and not norm.startswith('/') and ':' not in norm and
             all(p not in ('', '.', '..') for p in parts), 'invalid_spec',
             'stage name "%s" must be a relative path without ".."' % name, 'name')
        need(all(not _RESERVED_NAME.match(p) and not p.endswith((' ', '.')) for p in parts), 'invalid_spec',
             'stage name "%s" is not portable' % name, 'name')
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

    def data(self, index, path):
        need(type(index) is int and 0 <= index < len(self.inputs), 'invalid_spec',
             '%s: input %r does not exist (%d inputs were given)' % (path, index, len(self.inputs)), path)
        item = self.inputs[index]
        real = self.resolve(item['name'], ('in',), True)
        need(os.path.getsize(real) <= MAX_INPUT_BYTES, 'limit', '%s is larger than 64 MiB' % item['path'], path)
        with open(real, 'rb') as handle:
            data = handle.read()
        if item['sha256'] and sha256(data) != item['sha256']:
            fail('source_changed', '%s changed while it was staged; read it again' % item['path'], path)
        return data

    def warn(self, message, key=None):
        key = key or message
        if key not in self._warned and len(self.diagnostics) < MAX_DIAGNOSTICS:
            self._warned.add(key)
            self.diagnostics.append({'severity': 'warning', 'message': trunc(message, 400)})

    def capture_warning(self, message, category, filename, lineno, file=None, line=None):
        self.warn('%s: %s' % (category.__name__, str(message)[:300]))


# ---------------------------------------------------------------------------------------------
# Image helpers
# ---------------------------------------------------------------------------------------------

def pillow(job):
    from PIL import Image
    Image.MAX_IMAGE_PIXELS = job.limits['max_image_pixels']
    warnings.simplefilter('error', Image.DecompressionBombWarning)
    return Image


def load(job, index, path):
    Image = pillow(job)
    from PIL import UnidentifiedImageError
    data = job.data(index, path)
    label = job.inputs[index]['path']
    try:
        image = Image.open(io.BytesIO(data), formats=INPUT_FORMATS)
        need(image.width * image.height <= job.limits['max_image_pixels'], 'limit',
             '%s has %d pixels; the limit is %d' % (label, image.width * image.height,
                                                    job.limits['max_image_pixels']), path)
        if getattr(image, 'n_frames', 1) > 1:
            job.warn('%s has several frames; only the first one is edited' % label, 'frames')
        image.load()
    except EngineError:
        raise
    except (Image.DecompressionBombError, Image.DecompressionBombWarning):
        fail('limit', '%s is larger than the image pixel limit (%d)' % (label, job.limits['max_image_pixels']), path)
    except (UnidentifiedImageError, OSError, SyntaxError, ValueError, EOFError):
        fail('corrupt_input', '%s is not a readable PNG, JPEG, GIF, BMP, TIFF or WebP image' % label, path)
    return image


def working_mode(image):
    """Normalize to a mode the steps can draw on: RGB, RGBA, L or LA."""
    if image.mode in ('RGB', 'RGBA', 'L', 'LA'):
        return image
    if image.mode in ('P', 'PA'):
        return image.convert('RGBA' if 'transparency' in image.info or image.mode == 'PA' else 'RGB')
    if image.mode == '1':
        return image.convert('L')
    if image.mode in ('I', 'I;16', 'I;16B', 'I;16L', 'F'):
        return image.convert('I').point(lambda v: v * (1 / 257) if v > 255 else v).convert('L')
    return image.convert('RGBA' if 'A' in image.getbands() else 'RGB')


def color(value, path, default=None):
    from PIL import ImageColor
    if value is None:
        value = default
    if value is None:
        return None
    text = as_str(value, path, 64).strip()
    if re.fullmatch(r'[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?', text):
        text = '#' + text
    try:
        return ImageColor.getcolor(text, 'RGBA')
    except ValueError:
        fail('invalid_spec', '%s: "%s" is not a color (use "#RRGGBB", "#RRGGBBAA" or a name like "red")' % (
            path, text), path)


def in_mode(rgba, mode):
    if rgba is None:
        return None
    gray = int(round(0.299 * rgba[0] + 0.587 * rgba[1] + 0.114 * rgba[2]))
    return {'RGBA': rgba, 'RGB': rgba[:3], 'L': gray, 'LA': (gray, rgba[3])}.get(mode, rgba[:3])


def point(value, path):
    as_list(value, path, 2, 2)
    return as_num(value[0], path + '[0]', -1_000_000, 1_000_000), as_num(value[1], path + '[1]', -1_000_000,
                                                                          1_000_000)


def box(value, path, relative=False):
    as_list(value, path, 4, 4)
    limit = (0, 1) if relative else (-1_000_000, 1_000_000)
    left, top, right, bottom = (as_num(v, '%s[%d]' % (path, i), *limit) for i, v in enumerate(value))
    need(right > left and bottom > top, 'invalid_spec', '%s must be [left, top, right, bottom] with right > left '
         'and bottom > top' % path, path)
    return left, top, right, bottom


def check_size(job, width, height, path):
    need(width >= 1 and height >= 1, 'invalid_spec', '%s would produce an empty image' % path, path)
    need(width * height <= job.limits['max_image_pixels'], 'limit', '%s would produce %dx%d pixels; the limit is %d'
         % (path, width, height, job.limits['max_image_pixels']), path)


def draw_layer(image, paint):
    """Paint on a transparent RGBA layer and composite it, so colors with alpha work in every mode."""
    from PIL import Image, ImageDraw
    base = image.convert('RGBA')
    layer = Image.new('RGBA', base.size, (0, 0, 0, 0))
    paint(ImageDraw.Draw(layer))
    result = Image.alpha_composite(base, layer)
    return result if image.mode == 'RGBA' else result.convert(image.mode)


def with_alpha(image, operation):
    """Apply an RGB/L operation while keeping the alpha band untouched."""
    if image.mode in ('RGBA', 'LA'):
        bands = image.split()
        color_part = operation(image.convert('RGB' if image.mode == 'RGBA' else 'L'))
        color_part.putalpha(bands[-1])
        return color_part
    return operation(image)


# ---------------------------------------------------------------------------------------------
# Steps
# ---------------------------------------------------------------------------------------------

def apply_step(job, image, step, path, state):
    from PIL import Image, ImageEnhance, ImageFilter, ImageOps
    as_dict(step, path, required=('op',))
    op = as_str(step['op'], path + '.op', 32, tuple(STEP_KEYS))
    as_dict(step, path, STEP_KEYS[op] | {'op'})
    width, height = image.size
    if op in ('crop', 'crop_relative'):
        left, top, right, bottom = box(step.get('box'), path + '.box', op == 'crop_relative')
        if op == 'crop_relative':
            left, top, right, bottom = left * width, top * height, right * width, bottom * height
        clipped = (max(0, int(round(left))), max(0, int(round(top))), min(width, int(round(right))),
                   min(height, int(round(bottom))))
        need(clipped[2] > clipped[0] and clipped[3] > clipped[1], 'invalid_spec',
             '%s.box lies outside the %dx%d image' % (path, width, height), path + '.box')
        if clipped != (int(round(left)), int(round(top)), int(round(right)), int(round(bottom))):
            job.warn('%s: the crop box was clipped to the image (%dx%d)' % (path, width, height))
        return image.crop(clipped)
    if op == 'resize':
        target_w = opt(step, 'width', path, as_int, 1, 100_000)
        target_h = opt(step, 'height', path, as_int, 1, 100_000)
        need(target_w or target_h, 'invalid_spec', '%s needs width and/or height' % path, path)
        fit = opt(step, 'fit', path, as_str, 16, ('contain', 'cover', 'stretch'), default='contain')
        if not target_h:
            target_h = max(1, round(height * target_w / width))
            fit = 'stretch'
        elif not target_w:
            target_w = max(1, round(width * target_h / height))
            fit = 'stretch'
        check_size(job, target_w, target_h, path)
        if fit == 'stretch':
            return image.resize((target_w, target_h), Image.Resampling.LANCZOS)
        if fit == 'cover':
            return ImageOps.fit(image, (target_w, target_h), Image.Resampling.LANCZOS)
        return ImageOps.contain(image, (target_w, target_h), Image.Resampling.LANCZOS)
    if op == 'thumbnail':
        edge = as_int(step.get('max_edge'), path + '.max_edge', 1, 100_000)
        result = image.copy()
        result.thumbnail((edge, edge), Image.Resampling.LANCZOS)
        return result
    if op == 'rotate':
        degrees = as_num(step.get('degrees'), path + '.degrees', -360, 360)
        expand = opt(step, 'expand', path, as_bool, default=True)
        if degrees % 90 == 0 and expand:
            turns = int(degrees // 90) % 4
            return image if turns == 0 else image.transpose(
                (None, Image.Transpose.ROTATE_90, Image.Transpose.ROTATE_180, Image.Transpose.ROTATE_270)[turns])
        fill = in_mode(color(step.get('fill'), path + '.fill', '#00000000' if 'A' in image.mode else 'white'),
                       image.mode)
        radians = math.radians(degrees)
        new_w = abs(width * math.cos(radians)) + abs(height * math.sin(radians))
        new_h = abs(width * math.sin(radians)) + abs(height * math.cos(radians))
        if expand:
            check_size(job, int(new_w) + 1, int(new_h) + 1, path)
        return image.rotate(degrees, resample=Image.Resampling.BICUBIC, expand=expand, fillcolor=fill)
    if op == 'flip':
        direction = as_str(step.get('direction'), path + '.direction', 16, ('horizontal', 'vertical'))
        return ImageOps.mirror(image) if direction == 'horizontal' else ImageOps.flip(image)
    if op == 'auto_orient':
        orientation = state.get('orientation') or 1
        state['orientation'] = 1
        method = {2: Image.Transpose.FLIP_LEFT_RIGHT, 3: Image.Transpose.ROTATE_180,
                  4: Image.Transpose.FLIP_TOP_BOTTOM, 5: Image.Transpose.TRANSPOSE, 6: Image.Transpose.ROTATE_270,
                  7: Image.Transpose.TRANSVERSE, 8: Image.Transpose.ROTATE_90}.get(orientation)
        return image.transpose(method) if method is not None else image
    if op == 'adjust':
        need(any(step.get(k) is not None for k in ('brightness', 'contrast', 'saturation', 'sharpness')),
             'invalid_spec', '%s needs brightness, contrast, saturation or sharpness' % path, path)
        result = image
        for key, enhancer in (('brightness', ImageEnhance.Brightness), ('contrast', ImageEnhance.Contrast),
                              ('saturation', ImageEnhance.Color), ('sharpness', ImageEnhance.Sharpness)):
            factor = opt(step, key, path, as_num, 0, 5)
            if factor is not None and factor != 1:
                if key == 'saturation' and result.mode in ('L', 'LA'):
                    continue
                result = with_alpha(result, lambda part, e=enhancer, f=factor: e(part).enhance(f))
        return result
    if op == 'grayscale':
        return image.convert('LA' if 'A' in image.mode else 'L')
    if op == 'blur':
        radius = as_num(step.get('radius', 2), path + '.radius', 0, 100)
        return with_alpha(image, lambda part: part.filter(ImageFilter.GaussianBlur(radius)))
    if op == 'sharpen':
        amount = as_num(step.get('amount', 1.0), path + '.amount', 0, 5)
        return with_alpha(image, lambda part: part.filter(ImageFilter.UnsharpMask(radius=2,
                                                                                   percent=int(amount * 100),
                                                                                   threshold=3)))
    if op == 'text':
        return step_text(job, image, step, path)
    if op == 'arrow':
        start, end = point(step.get('from'), path + '.from'), point(step.get('to'), path + '.to')
        stroke = in_mode(color(step.get('color'), path + '.color', 'red'), 'RGBA')
        line_width = opt(step, 'width', path, as_int, 1, 500, default=max(2, round(min(width, height) / 150)))
        length = math.hypot(end[0] - start[0], end[1] - start[1])
        need(length >= 1, 'invalid_spec', '%s: from and to must differ' % path, path)
        head = min(max(10.0, line_width * 4.0), length * 0.6)
        angle = math.atan2(end[1] - start[1], end[0] - start[0])
        base = (end[0] - head * math.cos(angle) * 0.85, end[1] - head * math.sin(angle) * 0.85)
        wing = math.radians(28)
        tip_left = (end[0] - head * math.cos(angle - wing), end[1] - head * math.sin(angle - wing))
        tip_right = (end[0] - head * math.cos(angle + wing), end[1] - head * math.sin(angle + wing))

        def paint(draw):
            draw.line([start, base], fill=stroke, width=line_width)
            draw.polygon([end, tip_left, tip_right], fill=stroke)
        return draw_layer(image, paint)
    if op in ('rectangle', 'ellipse'):
        shape = box(step.get('box'), path + '.box')
        stroke = in_mode(color(step.get('color'), path + '.color', 'red'), 'RGBA')
        fill = in_mode(color(step.get('fill'), path + '.fill'), 'RGBA')
        line_width = opt(step, 'width', path, as_int, 0, 500, default=4)

        def paint(draw):
            method = draw.rectangle if op == 'rectangle' else draw.ellipse
            method(shape, outline=stroke if line_width else None, fill=fill, width=line_width)
        return draw_layer(image, paint)
    if op in ('pad', 'border'):
        default = '#FFFFFF00' if 'A' in image.mode else 'white'
        fill = in_mode(color(step.get('color'), path + '.color', 'black' if op == 'border' else default), image.mode)
        if op == 'border':
            amount = as_int(step.get('width'), path + '.width', 1, 10_000)
            check_size(job, width + 2 * amount, height + 2 * amount, path)
            return ImageOps.expand(image, border=amount, fill=fill)
        if step.get('size') is not None:
            as_list(step['size'], path + '.size', 2, 2)
            target_w = as_int(step['size'][0], path + '.size[0]', 1, 100_000)
            target_h = as_int(step['size'][1], path + '.size[1]', 1, 100_000)
            need(target_w >= width and target_h >= height, 'invalid_spec', '%s.size must be at least the image size '
                 '(%dx%d); resize first' % (path, width, height), path + '.size')
            check_size(job, target_w, target_h, path)
            canvas = Image.new(image.mode, (target_w, target_h), fill)
            canvas.paste(image, ((target_w - width) // 2, (target_h - height) // 2))
            return canvas
        every = opt(step, 'all', path, as_int, 0, 10_000, default=0)
        sides = [opt(step, key, path, as_int, 0, 10_000, default=every) for key in ('left', 'top', 'right', 'bottom')]
        need(any(sides), 'invalid_spec', '%s needs all, left/top/right/bottom or size' % path, path)
        check_size(job, width + sides[0] + sides[2], height + sides[1] + sides[3], path)
        return ImageOps.expand(image, border=tuple(sides), fill=fill)
    if op == 'overlay':
        index = as_int(step.get('input'), path + '.input', 1)
        layer_image = working_mode(load(job, index, path + '.input')).convert('RGBA')
        scale = opt(step, 'scale', path, as_num, 0.01, 20, default=1.0)
        if scale != 1:
            size = (max(1, round(layer_image.width * scale)), max(1, round(layer_image.height * scale)))
            check_size(job, *size, path)
            layer_image = layer_image.resize(size, Image.Resampling.LANCZOS)
        opacity = opt(step, 'opacity', path, as_num, 0, 1, default=1.0)
        if opacity < 1:
            alpha = layer_image.getchannel('A').point(lambda v: int(v * opacity))
            layer_image.putalpha(alpha)
        x = opt(step, 'x', path, as_int, -100_000, 100_000, default=0)
        y = opt(step, 'y', path, as_int, -100_000, 100_000, default=0)
        layer = Image.new('RGBA', image.size, (0, 0, 0, 0))
        layer.paste(layer_image, (x, y))  # plain copy keeps the alpha; alpha_composite applies it once
        result = Image.alpha_composite(image.convert('RGBA'), layer)
        return result if image.mode == 'RGBA' else result.convert(image.mode if image.mode != 'L' else 'L')
    mode = as_str(step.get('mode'), path + '.mode', 8, ('RGB', 'RGBA', 'L', 'LA', 'P', '1', 'CMYK'))
    state['final_mode'] = mode
    if mode in ('P', '1', 'CMYK'):
        return image
    return image.convert(mode)


def step_text(job, image, step, path):
    from PIL import ImageFont
    text = as_str(step.get('text'), path + '.text', 2000).replace('\r\n', '\n')
    need(text.strip(), 'invalid_spec', '%s.text is empty' % path, path + '.text')
    if any(ord(char) > 0x24F and not 0x2000 <= ord(char) <= 0x20CF for char in text if not char.isspace()):
        job.warn('%s: the built-in font covers Latin text; other scripts may show as boxes' % path, 'text-font')
    size = opt(step, 'size', path, as_int, 4, 2000, default=max(12, round(min(image.size) / 20)))
    x = opt(step, 'x', path, as_num, -100_000, 100_000, default=10)
    y = opt(step, 'y', path, as_num, -100_000, 100_000, default=10)
    fill = color(step.get('color'), path + '.color', 'white')
    background = color(step.get('background'), path + '.background')
    anchor = opt(step, 'anchor', path, as_str, 16, tuple(ANCHORS), default='top_left')
    padding = opt(step, 'padding', path, as_int, 0, 1000, default=max(2, size // 4))
    font = ImageFont.load_default(size=size)

    def paint(draw):
        left, top, right, bottom = draw.multiline_textbbox((0, 0), text, font=font, anchor='la', spacing=size // 4)
        width, height = right - left, bottom - top
        horizontal, vertical = ANCHORS[anchor]
        origin_x = x - {'l': 0, 'm': width / 2, 'r': width}[horizontal] - left
        origin_y = y - {'t': 0, 'm': height / 2, 'b': height}[vertical] - top
        if background is not None:
            draw.rectangle((origin_x + left - padding, origin_y + top - padding, origin_x + right + padding,
                            origin_y + bottom + padding), fill=background)
        draw.multiline_text((origin_x, origin_y), text, font=font, fill=fill, anchor='la', spacing=size // 4)
    return draw_layer(image, paint)


# ---------------------------------------------------------------------------------------------
# Pipeline
# ---------------------------------------------------------------------------------------------

def encode(job, image, target, spec, state, exif):
    from PIL import Image
    quality = opt(spec, 'quality', 'spec', as_int, 1, 100, default=90)
    background = color(spec.get('background'), 'spec.background', 'white')
    final_mode = state.get('final_mode')
    if final_mode == 'CMYK' and target not in ('JPEG', 'TIFF'):
        job.warn('CMYK is only kept for JPEG and TIFF; the image is saved as RGB', 'cmyk')
        final_mode = None
    if target in ('JPEG', 'BMP') and 'A' in image.mode:
        flat = Image.new('RGB', image.size, background[:3])
        flat.paste(image.convert('RGBA'), mask=image.convert('RGBA').getchannel('A'))
        image = flat
        job.warn('%s cannot store transparency; it was flattened onto %s' % (target, spec.get('background') or
                                                                             'white'), 'alpha')
    if final_mode == 'CMYK':
        image = image.convert('RGB').convert('CMYK')
    elif final_mode == 'P' and target in ('PNG', 'GIF', 'BMP', 'TIFF'):
        image = image.convert('RGBA' if 'A' in image.mode else 'RGB').quantize(256)
    elif final_mode == '1':
        image = image.convert('L').convert('1')
    elif target == 'WEBP' and image.mode not in ('RGB', 'RGBA'):
        image = image.convert('RGBA' if 'A' in image.mode else 'RGB')
    elif target == 'JPEG' and image.mode not in ('RGB', 'L', 'CMYK'):
        image = image.convert('RGB')
    image.info = {}
    params = {}
    if state.get('icc_profile') and image.mode in ('RGB', 'RGBA', 'CMYK') and target in ('PNG', 'JPEG', 'WEBP',
                                                                                          'TIFF'):
        params['icc_profile'] = state['icc_profile']
    if exif is not None and target in ('PNG', 'JPEG', 'WEBP', 'TIFF'):
        params['exif'] = exif
    if target == 'JPEG':
        params.update(quality=quality, optimize=True)
    elif target == 'WEBP':
        params.update(quality=quality, method=4)
    elif target == 'PNG':
        params.update(optimize=True)
    elif target == 'TIFF':
        params.update(compression='tiff_deflate')
    buffer = io.BytesIO()
    image.save(buffer, target, **params)
    return buffer.getvalue(), image


def op_pipeline(job):
    spec = as_dict(job.spec, 'spec', {'steps', 'format', 'quality', 'keep_exif', 'background'}, ('steps',))
    steps = as_list(spec['steps'], 'spec.steps', MAX_STEPS, 1)
    need(job.inputs, 'invalid_spec', 'image_edit needs the image as inputs[0]', 'inputs')
    extension = ext_of(job.output)
    need(extension in OUTPUT_FORMATS, 'invalid_spec', 'output.name must end with .png, .jpg, .webp, .gif, .bmp or '
         '.tiff', 'output.name')
    target = OUTPUT_FORMATS[extension]
    if spec.get('format') is not None:
        requested = OUTPUT_FORMATS.get(as_str(spec['format'], 'spec.format', 8).lower())
        need(requested == target, 'invalid_spec', 'spec.format "%s" does not match the output extension .%s' % (
            spec['format'], extension), 'spec.format')
    real = job.resolve(job.output, ('out',), False)
    need(not os.path.lexists(real), 'invalid_spec', 'output "%s" already exists; choose a new name' % job.output,
         'output.name')
    source = load(job, 0, 'inputs[0]')
    keep_exif = opt(spec, 'keep_exif', 'spec', as_bool, default=False)
    try:
        exif = source.getexif()
    except Exception:
        exif = None
    state = {'orientation': int(exif.get(0x0112, 1)) if exif else 1,
             'icc_profile': source.info.get('icc_profile')}
    image = working_mode(source)
    for index, step in enumerate(steps):
        path = 'spec.steps[%d]' % index
        image = working_mode(apply_step(job, image, step, path, state))
        check_size(job, image.width, image.height, path)
    exif_bytes = None
    if keep_exif and exif:
        if state['orientation'] == 1 and exif.get(0x0112):
            exif[0x0112] = 1
        exif_bytes = exif.tobytes()
    data, saved = encode(job, image, target, spec, state, exif_bytes)
    need(len(data) <= job.limits['max_output_bytes'], 'limit', 'the edited image exceeds the output size limit')
    os.makedirs(os.path.dirname(real), exist_ok=True)
    with open(real, 'xb') as handle:
        handle.write(data)
    job.outputs.append({'name': job.rel(real), 'sha256': sha256(data), 'bytes': len(data),
                        'media_type': MEDIA_TYPES[target]})
    if job.preview:
        add_preview(job, saved)
    return {'width': saved.width, 'height': saved.height, 'format': target.lower(), 'mode': saved.mode,
            'bytes': len(data), 'steps_applied': len(steps), 'exif_kept': exif_bytes is not None}


def add_preview(job, image):
    from PIL import Image
    preview = image.convert('RGBA' if 'A' in image.mode or image.mode == 'P' else 'RGB')
    edge = job.limits['preview_max_edge']
    preview.thumbnail((edge, edge), Image.Resampling.LANCZOS)
    for _ in range(8):
        buffer = io.BytesIO()
        preview.save(buffer, 'PNG', optimize=True)
        data = buffer.getvalue()
        if len(data) <= job.limits['preview_max_bytes']:
            break
        preview = preview.resize((max(1, preview.width * 3 // 4), max(1, preview.height * 3 // 4)))
    else:
        job.warn('The preview stayed above preview_max_bytes and was skipped')
        return
    directory = os.path.join(job.stage, 'work', 'previews')
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, 'image.png')
    counter = 1
    while os.path.lexists(path):
        path = os.path.join(directory, 'image-%d.png' % counter)
        counter += 1
    with open(path, 'xb') as handle:
        handle.write(data)
    job.previews.append({'name': job.rel(path), 'sha256': sha256(data), 'width': preview.width,
                         'height': preview.height, 'page': 1})


# ---------------------------------------------------------------------------------------------
# Entry points
# ---------------------------------------------------------------------------------------------

def emit(stream, receipt):
    stream.write(json.dumps(receipt, ensure_ascii=True, separators=(',', ':'), allow_nan=False).encode('ascii')
                 + b'\n')
    stream.flush()


def probe():
    import importlib.metadata
    missing, modules, checks = [], {}, {}
    try:
        from PIL import features
        modules['PIL'] = importlib.metadata.version('pillow')
        checks['freetype'] = bool(features.check('freetype2'))
        if not checks['freetype']:
            missing.append('PIL.freetype')
    except Exception:
        missing.append('PIL')
    return {'format': PROBE_FORMAT, 'ok': not missing, 'engine_version': ENGINE_VERSION,
            'python': '%d.%d.%d' % sys.version_info[:3], 'modules': modules, 'missing': missing, 'checks': checks}


def load_request(stage):
    path = os.path.join(stage, 'request.json')
    need(os.path.isfile(path), 'invalid_spec', 'request.json is missing')
    need(os.path.getsize(path) <= MAX_REQUEST_BYTES, 'limit', 'request.json is too large')
    with open(path, 'rb') as handle:
        raw = handle.read()
    try:
        return json.loads(raw.decode('utf-8-sig'))
    except (UnicodeDecodeError, ValueError, RecursionError) as error:
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
        result = op_pipeline(job)
        return {'format': RECEIPT_FORMAT, 'ok': True, 'engine_version': ENGINE_VERSION, 'result': result,
                'outputs': job.outputs, 'previews': job.previews, 'diagnostics': job.diagnostics}, 0
    except EngineError as error:
        code, message, path = error.code, error.message, error.path
    except MemoryError:
        code, message, path = 'limit', 'the image needs more memory than allowed', None
    except PermissionError as error:
        code, message, path = 'engine', 'blocked: %s' % error, None
    except Exception as error:
        code, message, path = 'engine', 'internal engine error: %s: %s' % (type(error).__name__, error), None
    message = str(message)
    for variant in {stage, stage.replace('\\', '/')}:
        message = message.replace(variant, '<stage>')
    error = {'code': code, 'message': trunc(message, 500)}
    if path:
        error['path'] = path
    if job is not None:
        for output in job.outputs:
            try:
                os.remove(os.path.join(stage, *output['name'].split('/')))
            except OSError:
                pass
    return {'format': RECEIPT_FORMAT, 'ok': False, 'engine_version': ENGINE_VERSION, 'error': error,
            'diagnostics': job.diagnostics if job else []}, 2


def main(argv):
    sys.dont_write_bytecode = True
    stream = sys.stdout.buffer
    sys.stdout = sys.stderr
    if len(argv) == 2 and argv[1] == '--probe':
        sys.addaudithook(Sandbox(None))
        receipt = probe()
        emit(stream, receipt)
        return 0 if receipt['ok'] else 3
    if len(argv) != 2 or argv[1].startswith('-'):
        sys.stderr.write('usage: python -I image_engine.py <stage> | --probe\n')
        return 64
    receipt, code = run(argv[1])
    emit(stream, receipt)
    return code


if __name__ == '__main__':
    sys.exit(main(sys.argv))
