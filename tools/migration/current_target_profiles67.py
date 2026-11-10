"""Explicit schema67 cold target; older reviewed producer bytes stay immutable.

Only an exact separately released artifact may select this profile. Schema20,
64 and 66 retain their original producers and registrations. These profiles
describe cold provenance and never grant execution or identity authority.
"""
from __future__ import annotations
import hashlib
from pathlib import Path
import current_target_profiles as previous
import v1_sqlite_export as v1
from current_target_registration import REGISTERED_PROFILES
from migration_io import Refused, fields, parse, sha

FORMAT = previous.FORMAT
PREVIEW_FORMAT = previous.PREVIEW_FORMAT
MAX_PROFILE = previous.MAX_PROFILE
MAX_REFERENCES = previous.MAX_REFERENCES
PROFILE = 'v2-storage-67-complete-store-connect/1'
ALLOWED_PROFILES = {**previous.ALLOWED_PROFILES, PROFILE: 67}


def load(name):
    if name in previous.ALLOWED_PROFILES:
        return previous.load(name)
    if name != PROFILE or name not in REGISTERED_PROFILES:
        raise Refused('Cold target profile has no immutable reviewed registration')
    registration = REGISTERED_PROFILES[name]
    fields(registration, ['manifest_file', 'manifest_sha256', 'reference_directory'])
    sha(registration['manifest_sha256'])
    if (registration['manifest_file'] != 'current-target-67.json'
            or registration['reference_directory'] != 'reference_source67'):
        raise Refused('Unknown schema67 target artifact location')
    path = v1.bounded_path(Path(__file__).resolve().parent / 'current-target-67.json')
    raw = path.read_bytes()
    if len(raw) > MAX_PROFILE or hashlib.sha256(raw).hexdigest() != registration['manifest_sha256']:
        raise Refused('Registered schema67 target artifact changed')
    value = parse(raw, MAX_PROFILE)
    fields(value, ['format', 'profile', 'central_version', 'schema', 'schema_sha256',
                   'migrations', 'source_bindings', 'source_closure_sha256',
                   'producer_source_pins', 'guards', 'integer_ids', 'ddl', 'role'])
    if (value['format'] != FORMAT or value['profile'] != PROFILE
            or value['central_version'] != 67
            or value['role'] != 'cold provenance only; no current authority or execution adoption'
            or v1.signature(value['schema']) != value['schema_sha256']):
        raise Refused('Registered schema67 descriptor differs')
    sha(value['source_closure_sha256'])
    if not isinstance(value['migrations'], list) or len(value['migrations']) != 67:
        raise Refused('Incomplete schema67 target migration registry')
    for index, entry in enumerate(value['migrations'], 1):
        if not isinstance(entry, list) or len(entry) != 2 or entry[0] != index:
            raise Refused('Noncontiguous schema67 target migrations')
        sha(entry[1])
    for key in ('source_bindings', 'producer_source_pins'):
        if not isinstance(value[key], dict) or not value[key]:
            raise Refused('Complete reviewed schema67 source bindings required')
        for relative, digest in value[key].items():
            if not isinstance(relative, str) or not relative or ':' in relative or '\\' in relative:
                raise Refused('Invalid fixed schema67 source path')
            parts = relative.split('/')
            if any(part in ('', '.', '..') for part in parts) or Path(relative).is_absolute():
                raise Refused('Invalid fixed schema67 source path')
            sha(digest)
    required_producers = {'current_target_profiles67.py', 'current_quarantine_preview67.py'}
    if not required_producers.issubset(value['producer_source_pins']):
        raise Refused('Schema67 producer closure is incomplete')
    if not isinstance(value['ddl'], list) or not 1 <= len(value['ddl']) <= 4096:
        raise Refused('Bounded reviewed schema67 DDL required')
    return value, path, registration['manifest_sha256']


# The frozen predecessor helpers accept an already verified fixed descriptor;
# none discovers profiles, evaluates source code or opens a live data store.
source_paths = previous.source_paths
producer_paths = previous.producer_paths
reference = previous.reference
descriptor = previous.descriptor
inspect = previous.inspect
