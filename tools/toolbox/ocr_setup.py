"""Pinned local OCR language setup, used by the private runtime installer."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import re
import uuid

BINDING_VERSION = '2.12.0'
LICENSE_SHA256 = 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'
LICENSE_BYTES = 11358


def read_manifest():
    path = Path(__file__).with_name('ocr-languages.lock.json')
    raw = path.read_bytes()
    value = json.loads(raw)
    if value.get('format') != 'cowork-ocr-language-pack/1' or value.get('engine') != 'tesseract-lstm':
        raise RuntimeError('Invalid pinned OCR language manifest')
    commit = value.get('source_commit', '')
    if not re.fullmatch('[0-9a-f]{40}', commit):
        raise RuntimeError('Invalid pinned OCR source revision')
    models = value.get('models')
    if not isinstance(models, list) or len(models) != 2 or {m.get('language') for m in models} != {'deu', 'eng'}:
        raise RuntimeError('The pinned OCR pack must contain German and English')
    for model in models:
        language = model['language']
        expected_url = 'https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/%s/%s.traineddata' % (commit, language)
        if (model.get('path') != language + '.traineddata' or model.get('source') != expected_url
                or not isinstance(model.get('sha256'), str) or not re.fullmatch('[0-9a-f]{64}', model['sha256'])
                or type(model.get('bytes')) is not int or not 0 < model['bytes'] <= 8 * 1024 * 1024):
            raise RuntimeError('Invalid pinned OCR model identity')
    return value, hashlib.sha256(raw).hexdigest()


def matches(path, digest, size):
    if not path.is_file():
        return False
    metadata = path.stat()
    if metadata.st_size != size or metadata.st_nlink != 1:
        return False
    with path.open('rb') as stream:
        data = stream.read(size + 1)
    return len(data) == size and hashlib.sha256(data).hexdigest() == digest


def publish_verified(source, target, digest, size, ordinary):
    ordinary(source)
    ordinary(target)
    if source.stat().st_size != size:
        raise RuntimeError('The downloaded OCR data has an unexpected size')
    with source.open('rb') as stream:
        data = stream.read(size + 1)
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        raise RuntimeError('The downloaded OCR data failed its pinned SHA-256 check')
    temporary = target.with_name(target.name + '.' + uuid.uuid4().hex + '.new')
    try:
        with temporary.open('xb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        if os.name != 'nt':
            temporary.chmod(0o600)
        ordinary(target)
        os.replace(temporary, target)
    finally:
        if temporary.exists():
            temporary.unlink()
    if not matches(target, digest, size):
        raise RuntimeError('The installed OCR data did not retain its original bytes')


def install_models(runtime, download, ordinary, atomic):
    """Use the installer's existing download/path checks; no system tool or registry."""
    runtime = Path(runtime)
    if not runtime.is_absolute():
        raise RuntimeError('An absolute private OCR runtime directory is required')
    manifest, manifest_sha = read_manifest()
    directory = ordinary(runtime / 'ocr', True)
    models = ordinary(directory / 'tessdata', True)
    for model in manifest['models']:
        target = ordinary(models / model['path'])
        if not matches(target, model['sha256'], model['bytes']):
            source = download('ocr-' + model['sha256'] + '.traineddata',
                              (model['source'], model['sha256']), model['bytes'])
            publish_verified(source, target, model['sha256'], model['bytes'], ordinary)
    license_url = 'https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/%s/LICENSE' % manifest['source_commit']
    license_target = ordinary(directory / 'LICENSE-tessdata-fast.txt')
    if not matches(license_target, LICENSE_SHA256, LICENSE_BYTES):
        source = download('ocr-license-' + LICENSE_SHA256 + '.txt', (license_url, LICENSE_SHA256), LICENSE_BYTES)
        publish_verified(source, license_target, LICENSE_SHA256, LICENSE_BYTES, ordinary)
    receipt = {'format': 'cowork-ocr-installation/1', 'binding_version': BINDING_VERSION,
               'language_manifest_sha256': manifest_sha, 'source_commit': manifest['source_commit'],
               'models': [{'language': m['language'], 'sha256': m['sha256'], 'bytes': m['bytes']} for m in manifest['models']],
               'license': {'spdx': 'Apache-2.0', 'sha256': LICENSE_SHA256, 'source': license_url}}
    atomic(directory / 'receipt.json', receipt)
    return receipt


def verify_runtime(python, runtime, execute):
    code = '''import importlib.metadata,os,sys,tesserocr
from PIL import Image,ImageDraw,ImageFont
assert importlib.metadata.version("tesserocr")=="2.12.0"
assert tesserocr.tesseract_version().splitlines()[0]=="tesseract 5.5.3"
image=Image.new("RGB",(700,100),"white")
ImageDraw.Draw(image).text((15,20),"OCR CHECK 12345",fill="black",font=ImageFont.load_default(size=32))
for language in ("deu","eng"):
 with tesserocr.PyTessBaseAPI(path=sys.argv[1]+os.sep,lang=language,oem=tesserocr.OEM.LSTM_ONLY,psm=tesserocr.PSM.SINGLE_LINE) as api:
  api.SetImage(image)
  text=api.GetUTF8Text()
  assert "12345" in text and api.AllWordConfidences()
print("Local OCR binding and German/English recognition ready")
'''
    execute([str(python), '-I', '-X', 'utf8', '-c', code, str(Path(runtime) / 'ocr/tessdata')])
