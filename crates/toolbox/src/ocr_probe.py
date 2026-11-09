"""Describe the selected optional OCR binding; never search host model directories."""
import base64
import csv
import hashlib
import importlib.metadata
import importlib.util
import io
import json
import os
from pathlib import Path
import sys

sys.dont_write_bytecode = True
sys.path[:] = [p for p in sys.path if p]
spec = importlib.util.spec_from_file_location('pinned_document_engine', Path(__file__).with_name('document_engine.py'))
engine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)
sys.addaudithook(engine.Sandbox(None))


def identity():
    distribution = importlib.metadata.distribution('tesserocr')
    if distribution.version != '2.12.0':
        raise ValueError('unexpected_binding_version')
    records = {row[0]: row for row in csv.reader(io.StringIO(distribution.read_text('RECORD') or ''))}
    prefix = Path(sys.prefix).resolve()
    files, total = [], 0
    for entry in sorted(distribution.files or [], key=str):
        name = str(entry)
        is_record = name.endswith('.dist-info/RECORD')
        if not (name.endswith(('.pyd', '.dll', '.so', '.py', '.pth')) or '.so.' in name or is_record):
            continue
        path = Path(os.path.abspath(distribution.locate_file(entry)))
        if not path.is_relative_to(prefix) or path.resolve(strict=True) != path:
            raise ValueError('native_library_outside_runtime')
        metadata = path.stat()
        if metadata.st_nlink != 1 or any(p.is_symlink() or getattr(p.lstat(), 'st_file_attributes', 0) & 0x400
                                        for p in [path, *path.parents] if p == prefix or prefix in p.parents):
            raise ValueError('native_library_alias')
        total += metadata.st_size
        if not 0 <= metadata.st_size <= 64 * 1024 * 1024 or total > 192 * 1024 * 1024 or len(files) >= 64:
            raise ValueError('native_library_bounds')
        row = records.get(name)
        if not is_record and (row is None or not row[1].startswith('sha256=') or int(row[2]) != metadata.st_size):
            raise ValueError('native_library_record_missing')
        with path.open('rb') as stream:
            data = stream.read(metadata.st_size + 1)
        digest = hashlib.sha256(data).digest()
        if len(data) != metadata.st_size or (not is_record and base64.urlsafe_b64encode(digest).rstrip(b'=').decode() != row[1][7:]):
            raise ValueError('native_library_record_changed')
        files.append({'path': str(path), 'bytes': len(data), 'sha256': digest.hex()})
    if not files:
        raise ValueError('native_libraries_missing')
    # Validate installed native bytes before importing the extension.
    import tesserocr
    version = tesserocr.tesseract_version().splitlines()[0]
    if version != 'tesseract 5.5.3':
        raise ValueError('unexpected_native_version')
    return {'binding_version': distribution.version, 'engine_version': version, 'files': files}


try:
    receipt = {'format': 'cowork-document-ocr-probe/1', 'ok': True, 'native': identity()}
    if len(json.dumps(receipt)) > 48 * 1024:
        raise ValueError('native_identity_bounds')
except Exception as error:
    receipt = {'format': 'cowork-document-ocr-probe/1', 'ok': False,
               'reason': str(error) if isinstance(error, ValueError) else type(error).__name__}
print(json.dumps(receipt, ensure_ascii=True, separators=(',', ':')))
raise SystemExit(0 if receipt['ok'] else 3)
