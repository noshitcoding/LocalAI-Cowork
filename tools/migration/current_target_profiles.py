"""Explicit reviewed cold-target profiles; never derive a profile from an input DB.

The registration table is populated only after release-time profile assembly.
Schema 20 remains exclusively owned by v2_snapshot_profile.py. Schema 64 and
the separately frozen schema 66 each have their own immutable artifact/pin.
"""
from __future__ import annotations
import contextlib
import hashlib
from pathlib import Path
import sqlite3
import v1_sqlite_export as v1
from migration_io import Refused, fields, parse, sha
from current_target_registration import REGISTERED_PROFILES

FORMAT = 'localai-cowork-reviewed-cold-target-profile/1'
PREVIEW_FORMAT = 'localai-cowork-quarantine-preview/2'
MAX_PROFILE = 2 * 1024 * 1024
MAX_REFERENCES = 32 * 1024 * 1024
ALLOWED_PROFILES = {
    'v2-storage-64-complete-store-connect/1': 64,
    'v2-storage-66-complete-store-connect/1': 66,
}
# Exact literal registrations live in a separately released file to avoid a
# manifest -> producer -> same manifest hash cycle. Missing entries fail closed.


def load(name):
    if name not in ALLOWED_PROFILES or name not in REGISTERED_PROFILES:
        raise Refused('Cold target profile has no immutable reviewed registration')
    registration = REGISTERED_PROFILES[name]
    fields(registration, ['manifest_file', 'manifest_sha256', 'reference_directory'])
    sha(registration['manifest_sha256'])
    expected_file = 'current-target-' + str(ALLOWED_PROFILES[name]) + '.json'
    expected_directory = 'reference_source' + str(ALLOWED_PROFILES[name])
    if registration['manifest_file'] != expected_file or registration['reference_directory'] != expected_directory:
        raise Refused('Unknown current target artifact location')
    path = v1.bounded_path(Path(__file__).resolve().parent / expected_file)
    raw = path.read_bytes()
    if len(raw) > MAX_PROFILE or hashlib.sha256(raw).hexdigest() != registration['manifest_sha256']:
        raise Refused('Registered current target artifact changed')
    value = parse(raw, MAX_PROFILE)
    fields(value, ['format', 'profile', 'central_version', 'schema', 'schema_sha256',
                   'migrations', 'source_bindings', 'source_closure_sha256',
                   'producer_source_pins', 'guards', 'integer_ids', 'ddl', 'role'])
    if (value['format'] != FORMAT or value['profile'] != name
            or value['central_version'] != ALLOWED_PROFILES[name]
            or value['role'] != 'cold provenance only; no current authority or execution adoption'
            or v1.signature(value['schema']) != value['schema_sha256']):
        raise Refused('Registered current target descriptor differs')
    sha(value['source_closure_sha256'])
    if len(value['migrations']) != value['central_version']:
        raise Refused('Incomplete current target migration registry')
    for index, entry in enumerate(value['migrations'], 1):
        if not isinstance(entry, list) or len(entry) != 2 or entry[0] != index:
            raise Refused('Noncontiguous current target migrations')
        sha(entry[1])
    for key in ('source_bindings', 'producer_source_pins'):
        if not isinstance(value[key], dict) or not value[key]:
            raise Refused('Complete reviewed current source bindings required')
        for relative, digest in value[key].items():
            parts = Path(relative)
            if parts.is_absolute() or '..' in parts.parts or '\\' in relative:
                raise Refused('Invalid fixed profile source path')
            sha(digest)
    if not isinstance(value['ddl'], list) or len(value['ddl']) > 4096:
        raise Refused('Bounded reviewed schema DDL required')
    return value, path, registration['manifest_sha256']


def source_paths(profile, root=None):
    if root is None:
        root = Path(__file__).resolve().parent / ('reference_source' + str(profile['central_version']))
    root = Path(root)
    if not root.is_absolute() or '..' in root.parts:
        raise Refused('Absolute fixed current target reference root required')
    return {v1.bounded_path(root / relative): digest for relative, digest in profile['source_bindings'].items()}


def producer_paths(profile):
    root = Path(__file__).resolve().parent
    return {v1.bounded_path(root / relative): digest for relative, digest in profile['producer_source_pins'].items()}


def reference(profile):
    conn = sqlite3.connect(':memory:')
    try:
        conn.execute('PRAGMA foreign_keys=ON')
        # This SQL is reviewed release artifact data. Neither input DB SQL nor
        # live Rust source is extracted or executed by the preview producer.
        for statement in profile['ddl']:
            if not isinstance(statement, str) or not statement.strip() or len(statement.encode()) > 256 * 1024:
                raise Refused('Invalid fixed current schema statement')
            conn.execute(statement)
        conn.commit()
        if v1.signature(v1.schema(conn)) != profile['schema_sha256']:
            raise Refused('Reviewed current reference schema differs')
        return conn
    except BaseException:
        conn.close()
        raise


def descriptor(profile, manifest_sha):
    return {'profile': profile['profile'], 'schema_sha256': profile['schema_sha256'],
            'source_bindings': profile['source_bindings'], 'profile_manifest_sha256': manifest_sha,
            'source_closure_sha256': profile['source_closure_sha256'],
            'producer_source_pins': profile['producer_source_pins']}


def inspect(conn, profile, manifest_sha):
    # Recognize complete fixed SQL before integrity/PRAGMA traversal of input.
    count = conn.execute('SELECT count(*) FROM sqlite_schema').fetchone()[0]
    if count != len(profile['schema']['objects']):
        raise Refused('Additional or missing current target SQL objects')
    rows = conn.execute('SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE sql IS NULL OR length(CAST(sql AS BLOB))<=131072 ORDER BY type,name').fetchall()
    if len(rows) != count:
        raise Refused('Current target schema SQL byte limit')
    shell = {'objects': [{'type': kind, 'name': name, 'table': table, 'sql': sql}
                         for kind, name, table, sql in rows], 'tables': profile['schema']['tables']}
    if v1.signature(shell) != profile['schema_sha256']:
        raise Refused('Unknown or changed current target SQL objects')
    if v1.signature(v1.schema(conn)) != profile['schema_sha256']:
        raise Refused('Unknown or changed complete current target schema')
    migrations = conn.execute('SELECT version,checksum FROM schema_migrations ORDER BY version').fetchall()
    if migrations != [tuple(entry) for entry in profile['migrations']]:
        raise Refused('Current target exact migration sequence/checksums differ')
    for table, version in profile['guards'].items():
        rows = conn.execute('SELECT version,typeof(version) FROM ' + v1.quote(table)).fetchall()
        if rows != [(version, 'integer')]:
            raise Refused('Current target guarded version differs')
    integer_tables = {table for table, metadata in profile['schema']['tables'].items()
                      if any(column[1] == 'id' and column[2].upper() in ('BIGINT', 'INTEGER')
                             for column in metadata['columns'])}
    if integer_tables != set(profile['integer_ids']):
        raise Refused('Unreviewed integer target identity table')
    for table, rule in profile['integer_ids'].items():
        fields(rule, ['value', 'required'])
        if type(rule['value']) is not int or type(rule['required']) is not bool:
            raise Refused('Invalid fixed integer target identity rule')
        rows = conn.execute('SELECT id,typeof(id) FROM ' + v1.quote(table) + ' LIMIT 2').fetchall()
        if rows != [(rule['value'], 'integer')] and not (not rule['required'] and rows == []):
            raise Refused('Current target singleton identity differs')
    for name in ('user_version', 'application_id'):
        if conn.execute('PRAGMA ' + name).fetchone()[0] != 0:
            raise Refused('Unknown current target SQLite metadata version')
    if conn.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)]:
        raise Refused('Current target integrity check failed')
    if conn.execute('PRAGMA foreign_key_check').fetchone() is not None:
        raise Refused('Current target foreign key violation')
    return descriptor(profile, manifest_sha)
