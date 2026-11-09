"""Explicit release signing profile. Private material is never returned in receipts."""
from __future__ import annotations
import json
import os
from pathlib import Path
import re
import struct
import urllib.parse


def private_file(path):
    path=Path(path)
    if not path.is_absolute() or path.is_symlink() or not path.is_file() or path.stat().st_size>1024*1024:
        raise ValueError('Select an absolute private signing file')
    for ancestor in path.parents:
        if ancestor.is_symlink() or getattr(ancestor.stat(),'st_file_attributes',0)&0x400:
            raise ValueError('Signing files cannot traverse linked directories')
    if os.name=='posix' and path.stat().st_mode&0o077:
        raise ValueError('Signing files must be private')
    return path


def profile(path):
    value=json.loads(private_file(path).read_bytes())
    allowed={'format','updater_public_key','updater_endpoint','signing_private_key_file','signing_password_file',
             'windows_certificate_thumbprint','timestamp_url'}
    if value.get('format')!=1 or set(value)-allowed:
        raise ValueError('Unsupported release signing profile')
    endpoint=urllib.parse.urlsplit(value['updater_endpoint'])
    if endpoint.scheme!='https' or not endpoint.hostname or endpoint.username or endpoint.password or endpoint.fragment:
        raise ValueError('Updater endpoint must be HTTPS without credentials')
    public=value['updater_public_key']
    if not isinstance(public,str) or not re.fullmatch(r'[A-Za-z0-9+/=]{40,4096}',public):
        raise ValueError('Provide the actual Tauri updater public key')
    environment={'TAURI_SIGNING_PRIVATE_KEY':str(private_file(value['signing_private_key_file']))}
    if value.get('signing_password_file'):
        environment['TAURI_SIGNING_PRIVATE_KEY_PASSWORD']=private_file(value['signing_password_file']).read_text().rstrip('\r\n')
    bundle={'createUpdaterArtifacts':True}
    thumbprint=value.get('windows_certificate_thumbprint')
    if thumbprint:
        if not re.fullmatch(r'[A-Fa-f0-9]{40,64}',thumbprint):raise ValueError('Invalid Windows certificate thumbprint')
        timestamp=urllib.parse.urlsplit(value.get('timestamp_url',''))
        if timestamp.scheme!='https' or not timestamp.hostname or timestamp.username or timestamp.password:
            raise ValueError('Provide an HTTPS Authenticode timestamp server')
        bundle['windows']={'certificateThumbprint':thumbprint,'digestAlgorithm':'sha256','timestampUrl':value['timestamp_url']}
    configuration={'bundle':bundle,'plugins':{'updater':{'pubkey':public,'endpoints':[value['updater_endpoint']]}}}
    receipt={'format':1,'updater_public_key':public,'updater_endpoint':value['updater_endpoint'],
             'windows_certificate_thumbprint':thumbprint,'timestamp_url':value.get('timestamp_url')}
    return configuration,environment,receipt


def normalized_pe(raw):
    if raw[:2]!=b'MZ' or len(raw)<64:raise ValueError('Signed payload is not a PE executable')
    pe=struct.unpack_from('<I',raw,60)[0]
    if raw[pe:pe+4]!=b'PE\0\0':raise ValueError('Invalid PE header')
    optional=pe+24;magic=struct.unpack_from('<H',raw,optional)[0]
    if magic not in (0x10b,0x20b):raise ValueError('Unknown PE optional header')
    security=optional+(112 if magic==0x20b else 96)+4*8
    address,size=struct.unpack_from('<II',raw,security)
    if bool(address)!=bool(size) or address and (address%8 or address+size!=len(raw)):
        raise ValueError('Unbounded PE Authenticode certificate table')
    result=bytearray(raw[:address] if address else raw)
    result[optional+64:optional+68]=b'\0'*4
    result[security:security+8]=b'\0'*8
    return bytes(result)
