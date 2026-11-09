"""Release-time assembler for an OWN frozen schema64, 66, 67 or 71 source closure.

This is not the runtime preview producer. It never opens a source/target user
database, evaluates archived code, imports archived Python, or changes live
storage. It emits reviewed SQL and a complete schema from private in-memory
SQLite. Root independently inspects and pins the artifact before registering it.
"""
from __future__ import annotations
import argparse
import contextlib
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import sys
import v1_sqlite_export as v1
from migration_io import Refused, FileFence, Stage, fields, parse, sha, budget, receipt_set

FORMAT = 'localai-cowork-reviewed-cold-target-profile/1'
ROLE = 'cold provenance only; no current authority or execution adoption'
MAX_CLOSURE = 32 * 1024 * 1024
MAX_PROFILE = 2 * 1024 * 1024
AUDIT_SQL = ('audit_schema.sql', 'audit_insights_schema.sql', 'audit_recovery_schema.sql',
             'audit_checkpoint_schema.sql', 'audit_trust_schema.sql', 'audit_origin_schema.sql')
PRODUCERS = ('schema_profiles.py', 'v1_sqlite_export.py', 'v1_quarantine.py',
             'v2_snapshot_profile.py', 'migration_io.py', 'linux_source_fence.py',
             'current_target_profiles.py', 'current_quarantine_preview.py')
# The release-time assembler and separate literal registration file are bound
# by the delivery receipt, not each artifact's runtime producer pins. A later
# schema66 recipe review must not invalidate the immutable schema64 producer.
# This complete integer table policy is reviewed independently of input DBs.
# New current schemas must add explicit rules and receive their own artifact.
INTEGER_IDS = {
    'identity_installation': False, 'audit_legacy_boundary': True,
    'productivity_guard': True, 'android_compat_guard': True,
    'team_schedule_guard': True, 'offline_guard': True, 'offline_local_device': False,
    'android_device_guard': True, 'personal_provider_guard': True,
    'team_sync_clock': True, 'android_wire_sync_clock': True,
    'model_registry_guard': True, 'server_key_guard': True, 'server_key_registry': False,
    'actor_model_registry_guard': True, 'android_auth_security_policy': True,
    'audit_origin_guard': True, 'audit_origin_epoch': False,
}
GUARDS = ('identity_schema_guard', 'tenant_project_schema_guard', 'team_schema_guard',
          'team_integration_schema_guard', 'native_authority_schema_guard', 'model_accounting_schema_guard')
STRINGS = re.compile(r'"(?:\\.|[^"\\])*"')


def _strings(text):
    # The reviewed migration regions use ordinary Rust string literals. A raw
    # string/change in this schema recipe is refused, never evaluated as code.
    if re.search(r'\br#*"', text):
        raise Refused('Reviewed schema recipe changed to a Rust raw string')
    result = []
    for match in STRINGS.finditer(text):
        try:
            result.append(json.loads(match.group()))
        except (ValueError, TypeError) as error:
            raise Refused('Unsupported reviewed Rust SQL literal') from error
    return result


def _region(text, anchor, opening, closing):
    positions = list(re.finditer(anchor, text))
    if len(positions) != 1:
        raise Refused('Ambiguous or absent reviewed schema recipe region')
    start = text.find(opening, positions[0].end())
    if start < 0:
        raise Refused('Reviewed schema recipe opening delimiter')
    depth, at = 1, start + 1
    while at < len(text):
        if text.startswith('//', at):
            end = text.find('\n', at)
            at = len(text) if end < 0 else end + 1
            continue
        if text.startswith('/*', at):
            end = text.find('*/', at + 2)
            if end < 0:
                raise Refused('Unterminated reviewed schema comment')
            at = end + 2
            continue
        if text[at] == '"':
            literal = STRINGS.match(text, at)
            if literal is None:
                raise Refused('Unterminated reviewed schema SQL literal')
            at = literal.end()
            continue
        if text[at] == opening:
            depth += 1
        elif text[at] == closing:
            depth -= 1
            if depth == 0:
                return text[start + 1:at]
        at += 1
    raise Refused('Unterminated reviewed schema recipe region')


def _ddl(strings):
    result = []
    for statement in strings:
        if statement.lstrip().upper().startswith(('CREATE TABLE ', 'CREATE INDEX ', 'CREATE UNIQUE INDEX ', 'ALTER TABLE ')):
            result.append(statement)
    if not result:
        raise Refused('Reviewed schema recipe has no DDL')
    return result


def _split(sql):
    # Match the actual fixed product migration runner's statement boundary.
    return [part.strip() for part in sql.split(';') if part.strip()]


def _recipe(files, central_version):
    def text(relative):
        try:
            return files[relative].decode('utf-8')
        except (KeyError, UnicodeError) as error:
            raise Refused('Complete ordinary UTF8 schema source required') from error
    def array(relative, constant):
        return _strings(_region(text(relative), r'\bconst\s+' + re.escape(constant) + r'\s*:[^=]+?=\s*&?', '[', ']'))
    def migration(relative):
        return _ddl(_strings(_region(text(relative), r'\b(?:async\s+)?fn\s+migrate\s*\(', '{', '}')))

    identity = 'crates/identity/src/'
    storage = 'crates/storage/src/'
    ddl = _ddl(array(identity + 'migrations.rs', 'STATEMENTS'))
    # Separate constants referenced by identity STATEMENTS; never source-user data.
    constants = [(identity + 'store/import_storage.rs', 'IMPORT_STORAGE_SCHEMA')]
    if central_version in (66, 67, 71, 75):
        constants.append((identity + 'store/import_storage.rs', 'IMPORT_STORAGE_RELEASE_SCHEMA'))
    constants.append((identity + 'store/session_scopes.rs', 'SESSION_SCOPE_SCHEMA'))
    for relative, constant in constants:
        source = text(relative)
        declaration = re.search(r'\bconst\s+' + constant + r'\s*:[^=]+=(\s*"(?:\\.|[^"\\])*")\s*;', source)
        if declaration is None:
            raise Refused('Missing reviewed identity schema constant')
        ddl += _ddl(_strings(declaration.group(1)))
    ddl.append('INSERT INTO identity_schema_guard(version) VALUES(1)')
    for name in AUDIT_SQL:
        ddl += _split(text(identity + name))
    # The complete include graph is fixed explicitly. Refund66 is the reviewed
    # literal above, not a dynamically interpreted extra include.
    includes = re.findall(r'include_str!\("([^"/]+\.sql)"\)', text(identity + 'migrations.rs'))
    extras = [name for name in includes if name not in AUDIT_SQL]
    if extras:
        raise Refused('Unknown additional fixed identity DDL include')
    ddl += migration(storage + 'tenant_projects.rs')
    ddl.append('INSERT INTO tenant_project_schema_guard(version) VALUES(1)')
    ddl += migration(storage + 'team_service.rs')
    ddl.append('INSERT INTO team_schema_guard(version) VALUES(1)')
    ddl += migration(storage + 'team_integrations.rs')
    ddl.append('INSERT INTO team_integration_schema_guard(version) VALUES(1)')

    registry = text(storage + 'migrations.rs')
    central = re.findall(r'\((\d+),\s*include_str!\("\.\./migrations/(\d{4}\.sql)"\)\)', registry)
    if [int(version) for version, _ in central] != list(range(1, central_version + 1)):
        raise Refused('Frozen central registration is not the exact requested profile')
    migrations = []
    for version, filename in central:
        raw = files['crates/storage/migrations/' + filename]
        digest = hashlib.sha256(raw).hexdigest()
        migrations.append([int(version), digest])
        ddl += _split(raw.decode('utf-8'))
        if int(version) == 2:
            ddl.append('CREATE UNIQUE INDEX messages_position ON messages(order_id,position)')
        ddl.append("INSERT INTO schema_migrations(version,checksum) VALUES(" + version + ",'" + digest + "')")
    ddl += migration('crates/model-accounting/src/persistence.rs')
    ddl += migration(storage + 'import_basis.rs')
    ddl += _ddl(array(storage + 'import_attachments.rs', 'SCHEMA'))
    ddl += _ddl(array(storage + 'import_catalog.rs', 'SCHEMA'))
    ddl += migration(storage + 'model_money.rs')
    return ddl, migrations


def assemble(closure_path, closure_sha, central_version, output):
    if central_version not in (64, 66, 67, 71, 75):
        raise Refused('Only separately reviewed schema64/schema66/schema67/schema71 closures are supported')
    closure_path = v1.bounded_path(closure_path)
    sha(closure_sha)
    raw = closure_path.read_bytes()
    if len(raw) > 2 * 1024 * 1024 or hashlib.sha256(raw).hexdigest() != closure_sha:
        raise Refused('Own frozen source closure differs')
    closure = parse(raw)
    fields(closure, ['format', 'profile', 'central_migrations', 'status', 'runtime_qualification', 'files'])
    name = 'v2-storage-' + str(central_version) + '-complete-store-connect/1'
    if (closure['format'] != 'localai-cowork-schema-source-closure/1' or closure['profile'] != name
            or closure['central_migrations'] != central_version or closure['status'] != 'immutable-source-capture'
            or closure['runtime_qualification'] is not False):
        raise Refused('Unknown own frozen source closure contract')
    if not isinstance(closure['files'], list) or not 1 <= len(closure['files']) <= 512:
        raise Refused('Own complete source closure count')
    root = closure_path.parent
    sources = {}
    for entry in closure['files']:
        fields(entry, ['path', 'bytes', 'sha256'])
        relative = entry['path']
        if not isinstance(relative, str) or not relative or '\\' in relative or ':' in relative:
            raise Refused('Nonordinary frozen source path')
        if any(part in ('', '.', '..') for part in relative.split('/')) or relative.startswith('/'):
            raise Refused('Frozen source path leaves owned closure')
        path = v1.bounded_path(root / relative)
        if path in sources or type(entry['bytes']) is not int or not 0 <= entry['bytes'] <= MAX_CLOSURE:
            raise Refused('Ambiguous frozen source entry')
        sha(entry['sha256'])
        sources[path] = entry
    producer_names = PRODUCERS
    if central_version in (67, 71, 75):
        producer_names += ('current_target_profiles67.py', 'current_quarantine_preview67.py')
    if central_version in (71, 75):
        producer_names += ('current_target_profiles71.py', 'current_quarantine_preview71.py')
    if central_version == 75:
        producer_names += ('current_target_profiles75.py', 'current_quarantine_preview75.py')
    producers = {v1.bounded_path(Path(__file__).resolve().parent / filename): filename for filename in producer_names}
    paths = [closure_path, *sources, *producers]
    op = budget()
    with FileFence(paths, op, MAX_CLOSURE + 4 * 1024 * 1024) as fence, Stage(output, paths) as stage:
        original = receipt_set(fence)
        if original[str(closure_path)]['sha256'] != closure_sha:
            raise Refused('Source closure changed before its native fence')
        files = {}
        for path, entry in sources.items():
            receipt = original[str(path)]
            if receipt['bytes'] != entry['bytes'] or receipt['sha256'] != entry['sha256']:
                raise Refused('Original frozen schema source bytes differ')
            files[entry['path']] = fence.read(path, MAX_CLOSURE)
        ddl, migrations = _recipe(files, central_version)
        with contextlib.closing(sqlite3.connect(':memory:')) as conn:
            conn.execute('PRAGMA foreign_keys=ON')
            conn.set_progress_handler(op.sql_progress, 1000)
            for statement in ddl:
                conn.execute(statement)
            conn.commit()
            schema = v1.schema(conn)
            integer_tables = {table for table, metadata in schema['tables'].items()
                              if any(column[1] == 'id' and column[2].upper() in ('BIGINT', 'INTEGER')
                                     for column in metadata['columns'])}
            if integer_tables != set(INTEGER_IDS):
                raise Refused('New integer schema table needs an explicit reviewed rule')
            for table in GUARDS:
                if conn.execute('SELECT version FROM ' + v1.quote(table)).fetchall() != [(1,)]:
                    raise Refused('Reviewed fixed guard seed differs')
            profile = {'format': FORMAT, 'profile': name, 'central_version': central_version,
                       'schema': schema, 'schema_sha256': v1.signature(schema), 'migrations': migrations,
                       'source_bindings': {entry['path']: entry['sha256'] for entry in sources.values()},
                       'source_closure_sha256': closure_sha,
                       'producer_source_pins': {filename: original[str(path)]['sha256'] for path, filename in producers.items()},
                       'guards': {table: 1 for table in GUARDS},
                       'integer_ids': {table: {'value': 1, 'required': required} for table, required in INTEGER_IDS.items()},
                       'ddl': ddl, 'role': ROLE}
        if len(v1.canonical(profile)) > MAX_PROFILE:
            raise Refused('Reviewed current profile artifact exceeds fixed bound')
        filename = 'current-target-' + str(central_version) + '.json'
        artifact_sha = v1.write_json(stage.directory / filename, profile)
        receipt = {'format': 'localai-cowork-current-target-assembly/1', 'profile': name,
                   'profile_manifest_sha256': artifact_sha, 'source_closure_sha256': closure_sha,
                   'schema_sha256': profile['schema_sha256'], 'source_files': len(sources),
                   'ddl_statements': len(ddl), 'private_memory_sqlite_only': True,
                   'live_database_opened': False, 'runtime_qualified': False, 'registered': False}
        v1.write_json(stage.directory / 'assembly-receipt.json', receipt)
        if original != receipt_set(fence):
            raise Refused('Owned profile assembly inputs changed under fence')
        op.check()
        stage.publish(op)
        return receipt


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--closure', required=True)
    parser.add_argument('--closure-sha256', required=True)
    parser.add_argument('--central-version', type=int, choices=(64, 66, 67, 71, 75), required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args(argv)
    try:
        print(v1.canonical(assemble(args.closure, args.closure_sha256, args.central_version, args.output)).decode('ascii'))
        return 0
    except (Refused, OSError, ValueError, TypeError, KeyError, UnicodeError, sqlite3.Error) as error:
        print(v1.canonical({'state': 'refused', 'error': str(error), 'registered': False}).decode('ascii'), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
