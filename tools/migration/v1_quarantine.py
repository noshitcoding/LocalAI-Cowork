"""Stage 3: complete, inert historical SQLite quarantine and read-only preview.

This format is deliberately not a Store database. Neither imported values nor
mapping files can create a Principal, token, ACL, approval or executable run.
"""
from __future__ import annotations
import argparse
import contextlib
import hashlib
import os
from pathlib import Path
import sqlite3
import sys
import uuid
import v1_sqlite_export as v1
import schema_profiles as source_profiles
import v2_snapshot_profile as target_profile
from migration_io import (Refused, FileFence, Stage, parse, fields, sha, integer,
                          natural, decode_cell, budget, receipt_set, cold, MAX_LINE, MAX_BUNDLE, MAX_QUARANTINE)

FORMAT = 'localai-cowork-inactive-quarantine/1'
MAPPING_FORMAT = 'localai-cowork-historical-mapping/1'
PREVIEW_FORMAT = 'localai-cowork-quarantine-preview/1'
DDL = '''
CREATE TABLE quarantine_meta (
 id INTEGER PRIMARY KEY CHECK(id=1), format TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state='inactive'), export_sha256 TEXT NOT NULL,
 export_manifest BLOB NOT NULL, execution_policy BLOB NOT NULL
) STRICT;
CREATE TABLE source_tables (
 name TEXT PRIMARY KEY, columns_json BLOB NOT NULL,
 row_count INTEGER NOT NULL CHECK(row_count>=0), content_sha256 TEXT NOT NULL
) STRICT;
CREATE TABLE history_rows (
 table_name TEXT NOT NULL REFERENCES source_tables(name), source_rowid INTEGER NOT NULL,
 position INTEGER NOT NULL CHECK(position>0), canonical_row BLOB NOT NULL,
 row_sha256 TEXT NOT NULL, PRIMARY KEY(table_name,source_rowid), UNIQUE(table_name,position)
) STRICT;
CREATE TABLE history_projects (
 source_id TEXT PRIMARY KEY, source_table TEXT, source_rowid INTEGER,
 FOREIGN KEY(source_table,source_rowid) REFERENCES history_rows(table_name,source_rowid),
 CHECK((source_table IS NULL)=(source_rowid IS NULL))
) STRICT;
CREATE TABLE history_actors (source_id TEXT PRIMARY KEY) STRICT;
CREATE TABLE history_entities (
 kind TEXT NOT NULL, source_id TEXT NOT NULL,
 source_table TEXT, source_rowid INTEGER,
 project_source_id TEXT REFERENCES history_projects(source_id),
 actor_source_id TEXT REFERENCES history_actors(source_id),
 PRIMARY KEY(kind,source_id),
 FOREIGN KEY(source_table,source_rowid) REFERENCES history_rows(table_name,source_rowid),
 CHECK((source_table IS NULL)=(source_rowid IS NULL))
) STRICT;
CREATE TABLE history_relations (
 kind TEXT NOT NULL, source_kind TEXT NOT NULL, source_id TEXT NOT NULL,
 target_kind TEXT NOT NULL, target_id TEXT NOT NULL,
 PRIMARY KEY(kind,source_kind,source_id,target_kind,target_id),
 FOREIGN KEY(source_kind,source_id) REFERENCES history_entities(kind,source_id),
 FOREIGN KEY(target_kind,target_id) REFERENCES history_entities(kind,source_id)
) STRICT;
'''
ENTITY_TABLES = {'projects': 'project', 'chat_threads': 'thread',
                 'engine_runs': 'engine_run', 'crew_runs': 'crew_run',
                 'artifact_versions': 'artifact_version', 'engine_run_artifacts': 'engine_artifact',
                 'agent_personalities': 'agent_persona', 'daemon_runs': 'daemon_run'}
MAPPABLE = set(ENTITY_TABLES.values()) | {'historical_actor'}
RUN_KINDS = {'engine_run', 'crew_run', 'daemon_run'}
ARTIFACT_KINDS = {'artifact_version', 'engine_artifact'}


def identifier(value):
    if not isinstance(value, str) or not value or len(value.encode('utf-8')) > 256 or any(ord(c)<32 or ord(c)==127 for c in value):
        raise Refused('An exact nonempty historical ID without control characters is required')
    return value


def uuid_id(value):
    identifier(value)
    try:
        parsed = uuid.UUID(value)
    except ValueError as error:
        raise Refused('Canonical non-nil V2 UUID required') from error
    if parsed.int == 0 or str(parsed) != value:
        raise Refused('Canonical non-nil V2 UUID required')
    return value


def known_source(details):
    fields(details, ['family','version','profile','schema','schema_sha256','row_counts','sqlite_metadata'])
    fields(details['schema'],['objects','tables'])
    family, version = details['family'], details['version']
    conn = sqlite3.connect(':memory:')
    try:
        conn.execute('PRAGMA foreign_keys=OFF')
        conn.execute('PRAGMA legacy_alter_table=ON')
        if family == 'desktop':
            if type(version) is not int or not 0 <= version <= 28:
                raise Refused('Unknown desktop export version')
            if details['profile'] != 'desktop-0-uninitialized':
                conn.executescript('CREATE TABLE schema_version(version INTEGER NOT NULL);INSERT INTO schema_version VALUES(0);')
                v1.apply_events(conn, [event for stage in source_profiles.DESKTOP_STAGES[:version] for event in stage])
                if details['profile'].endswith('-backend-selection'):
                    conn.executescript(source_profiles.DESKTOP_BACKPATCH)
        elif family == 'daemon' and version is None:
            conn.executescript(source_profiles.DAEMON_BASE)
            if details['profile'] == 'daemon-sync-15':
                conn.executescript(source_profiles.DAEMON_SYNC)
        else:
            raise Refused('Unknown source family/version')
        actual = v1.schema(conn)
        fingerprint = v1.signature(actual)
        match = v1.REFERENCE_PROFILES.get((family, version, fingerprint))
        if match != details['profile'] or fingerprint != details['schema_sha256'] or v1.signature(details['schema']) != fingerprint:
            raise Refused('Export has unknown or forged complete source schema')
        if set(details['row_counts']) != set(actual['tables']):
            raise Refused('Missing source table count')
        if sum(natural(n) for n in details['row_counts'].values()) > v1.MAX_ROWS:
            raise Refused('Complete export row limit exceeded')
        fields(details['sqlite_metadata'], ['user_version','application_id','schema_version','page_size'])
        if any(type(value) is not int for value in details['sqlite_metadata'].values()):
            raise Refused('Invalid source SQLite metadata storage types')
        if details['sqlite_metadata']['user_version'] != 0 or details['sqlite_metadata']['application_id'] != 0:
            raise Refused('Unknown source SQLite metadata version')
        conn.commit()
        return conn
    except BaseException:
        conn.close()
        raise


def export_header(value):
    fields(value, ['format','stage','execution_policy','inventory_sha256','authority_sha256',
                   'inventory_manifest_sha256','source_files','snapshot','source_bindings',
                   'details','content','content_sha256','canonical_cells','manifest_sha256'])
    v1.verify_manifest(value)
    if v1.canonical(value['execution_policy']) != v1.canonical(v1.POLICY):
        raise Refused('Historical execution policy has unknown JSON types')
    if value['stage'] != 'readonly_export' or value['source_bindings'] != source_profiles.SOURCE_BINDINGS:
        raise Refused('Pinned complete historical export required')
    for name in ('inventory_sha256','authority_sha256','inventory_manifest_sha256','content_sha256'):
        sha(value[name])
    if value['canonical_cells'] != 'integer-decimal/null/text-utf8-bytes-base64/blob-base64/finite-real-ieee754-binary64-be':
        raise Refused('Unknown typed row encoding')
    if v1.digest(value['content']) != value['content_sha256']:
        raise Refused('Content manifest seal differs')
    if not isinstance(value['source_files'], list) or not 1 <= len(value['source_files']) <= 4:
        raise Refused('Incomplete source byte provenance')
    original_paths = set(); original_bytes = 0
    for original in value['source_files']:
        fields(original, ['bytes','volume','file_id','sha256','path'])
        natural(original['bytes'], v1.MAX_SOURCE)
        sha(original['sha256'])
        integer(original['file_id'],0,(1<<64)-1)
        natural(original['volume'],(1<<32)-1)
        if not isinstance(original['path'], str):
            raise Refused('Invalid source byte provenance')
        original_bytes += original['bytes']
        if original['path'] in original_paths or original_bytes > v1.MAX_SOURCE:
            raise Refused('Duplicate or oversized original byte provenance')
        original_paths.add(original['path'])
    fields(value['snapshot'], ['file','bytes','sha256'])
    if value['snapshot']['file'] != 'snapshot.sqlite3':
        raise Refused('Unknown original snapshot name')
    natural(value['snapshot']['bytes'], v1.MAX_SOURCE)
    sha(value['snapshot']['sha256'])
    with contextlib.closing(known_source(value['details'])) as conn:
        tables = v1.schema(conn)['tables']
    if set(value['content']) != set(tables):
        raise Refused('Export is missing or adds source tables')
    size = 0
    for ordinal, table in enumerate(sorted(tables)):
        item = value['content'][table]
        fields(item, ['file','rows','bytes','sha256','columns'])
        if item['file'] != f'table-{ordinal:03d}.jsonl' or item['columns'] != [c[1] for c in tables[table]['columns']]:
            raise Refused('Table path/column mapping differs from complete source profile')
        if natural(item['rows']) != value['details']['row_counts'][table]:
            raise Refused('Table row count differs')
        size += natural(item['bytes'], MAX_BUNDLE)
        sha(item['sha256'])
    if size > MAX_BUNDLE:
        raise Refused('Complete export byte limit exceeded')


def exact_bundle_files(directory, expected):
    actual = {entry.name for entry in directory.iterdir()}
    if actual != set(expected):
        raise Refused('Complete bundle file set differs')


def source_text(conn, query, parameters=()):
    # Source bytes stay unchanged; identity-bearing values require explicit UTF-8.
    try:
        return conn.execute(query, parameters).fetchall()
    except (sqlite3.Error, UnicodeError) as error:
        raise Refused('Unsupported invalid historical identity/association text') from error


def entity_index(source, target, family):
    tables = set(v1.schema(source)['tables'])
    for table in sorted(tables):
        names = [row[1] for row in source.execute('PRAGMA table_xinfo('+v1.quote(table)+')')]
        # Nullable TEXT primary keys accepted by SQLite are not safe migration IDs.
        if 'id' in names:
            seen = set()
            for value, kind in source_text(source, 'SELECT id,typeof(id) FROM '+v1.quote(table)):
                identity = identifier(value) if kind == 'text' else str(value) if kind == 'integer' else None
                if identity is None or identity in seen:
                    raise Refused('Duplicate, null or ambiguously typed historical ID')
                seen.add(identity)
    if 'projects' in tables:
        for rowid, source_id in source_text(source, 'SELECT _rowid_,id FROM projects'):
            target.execute('INSERT INTO history_projects VALUES(?,?,?)', (identifier(source_id),'projects',rowid))
    if family == 'daemon':
        # The actual RunRecord keeps project and human creator in record_json.spec.
        for rowid, run_id, encoded in source_text(source, 'SELECT _rowid_,id,record_json FROM daemon_runs'):
            record = parse(encoded.encode('utf-8'), MAX_LINE)
            spec = record.get('spec')
            if not isinstance(spec, dict) or record.get('id') != run_id:
                raise Refused('Daemon RunRecord ID/spec differs from original row')
            project, actor = uuid_id(spec.get('project_id')), uuid_id(spec.get('creator_user_id'))
            if 'project' in spec and (not isinstance(spec['project'],dict) or spec['project'].get('id') != project):
                raise Refused('Daemon project pin conflicts with project_id')
            target.execute('INSERT OR IGNORE INTO history_projects VALUES(?,NULL,NULL)', (project,))
            target.execute('INSERT OR IGNORE INTO history_actors VALUES(?)', (actor,))
            target.execute('INSERT OR IGNORE INTO history_entities VALUES(?,?,NULL,NULL,NULL,NULL)', ('historical_actor',actor))
            target.execute('INSERT INTO history_entities VALUES(?,?,?,?,?,?)', ('daemon_run',identifier(run_id),'daemon_runs',rowid,project,actor))
        if 'daemon_project_bindings' in tables:
            for project, in source_text(source, 'SELECT project_id FROM daemon_project_bindings'):
                target.execute('INSERT OR IGNORE INTO history_projects VALUES(?,NULL,NULL)', (uuid_id(project),))
        with contextlib.closing(target.execute('SELECT source_id FROM history_projects')) as cursor:
            for project, in cursor:
                target.execute('INSERT INTO history_entities VALUES(?,?,NULL,NULL,?,NULL)', ('project',project,project))
        return
    threads = {}
    if 'project_threads' in tables:
        for project, thread in source_text(source, 'SELECT project_id,thread_id FROM project_threads'):
            identifier(project); identifier(thread)
            if thread in threads and threads[thread] != project:
                raise Refused('Historical thread has multiple project scopes')
            threads[thread] = project
    for table, kind in ENTITY_TABLES.items():
        if table not in tables or kind == 'daemon_run':
            continue
        columns = [r[1] for r in source.execute('PRAGMA table_xinfo('+v1.quote(table)+')')]
        selected = ['id']+(['thread_id'] if 'thread_id' in columns else [])
        for row in source_text(source, 'SELECT _rowid_,'+','.join(v1.quote(name) for name in selected)+' FROM '+v1.quote(table)):
            values = dict(zip(selected,row[1:])); identity = identifier(values['id'])
            project = identity if kind == 'project' else threads.get(identity if kind == 'thread' else values.get('thread_id'))
            target.execute('INSERT INTO history_entities VALUES(?,?,?,?,?,NULL)', (kind,identity,table,row[0],project))
    for thread, project in threads.items():
        if target.execute('SELECT 1 FROM history_entities WHERE kind=? AND source_id=?',('thread',thread)).fetchone() is None:
            raise Refused('Project binding references a missing original thread')
        target.execute('INSERT INTO history_relations VALUES(?,?,?,?,?)', ('project_thread','project',project,'thread',thread))
    if 'engine_runs' in tables:
        for run, parent in source_text(source, 'SELECT id,parent_run_id FROM engine_runs WHERE parent_run_id IS NOT NULL'):
            a = target.execute('SELECT project_source_id FROM history_entities WHERE kind=? AND source_id=?',('engine_run',run)).fetchone()
            b = target.execute('SELECT project_source_id FROM history_entities WHERE kind=? AND source_id=?',('engine_run',identifier(parent))).fetchone()
            if b is None or a[0] != b[0]:
                raise Refused('Parent run is missing or crosses a historical project scope')
            target.execute('INSERT INTO history_relations VALUES(?,?,?,?,?)', ('parent_run','engine_run',run,'engine_run',parent))
    for table, kind in (('artifact_versions','artifact_version'),('engine_run_artifacts','engine_artifact')):
        if table not in tables:
            continue
        for identity, run in source_text(source,'SELECT id,run_id FROM '+table):
            candidates = target.execute('SELECT kind,project_source_id FROM history_entities WHERE kind IN (?,?,?) AND source_id=?',(*sorted(RUN_KINDS),run)).fetchall()
            if len(candidates) != 1:
                raise Refused('Artifact run binding is absent or ambiguous')
            run_kind, project = candidates[0]
            target.execute('UPDATE history_entities SET project_source_id=? WHERE kind=? AND source_id=?',(project,kind,identity))
            target.execute('INSERT INTO history_relations VALUES(?,?,?,?,?)', ('run_artifact',run_kind,run,kind,identity))


def create_quarantine(export_path, export_sha256, output):
    export_path = v1.bounded_path(export_path)
    sha(export_sha256)
    op = budget()
    # Preliminary manifest is untrusted; all fields/files are revalidated while fenced.
    with FileFence([export_path], op) as first:
        header = parse(first.read(export_path,v1.MAX_MANIFEST))
        if first.receipt(export_path)['sha256'] != export_sha256:
            raise Refused('Pinned export byte hash mismatch')
        export_header(header)
    paths = [export_path,*[v1.bounded_path(export_path.parent/item['file']) for item in header['content'].values()]]
    with Stage(output, paths) as stage, FileFence(paths, op) as fence:
        original = receipt_set(fence)
        exact_bundle_files(export_path.parent, [p.name for p in paths])
        raw = fence.read(export_path,v1.MAX_MANIFEST)
        if hashlib.sha256(raw).hexdigest() != export_sha256:
            raise Refused('Pinned export changed before complete freeze')
        manifest = parse(raw); export_header(manifest)
        validation = stage.directory/'source-validation.sqlite3'
        quarantine = stage.directory/'quarantine.sqlite3'
        with contextlib.closing(known_source(manifest['details'])) as schema:
            with contextlib.closing(sqlite3.connect(validation)) as source:
                schema.backup(source)
        with contextlib.closing(sqlite3.connect(validation)) as source, contextlib.closing(sqlite3.connect(quarantine)) as target:
            source.execute('PRAGMA foreign_keys=OFF')
            target.execute('PRAGMA foreign_keys=ON'); target.execute('PRAGMA trusted_schema=OFF')
            target.execute('PRAGMA synchronous=FULL'); target.execute('PRAGMA journal_mode=DELETE')
            target.setlimit(sqlite3.SQLITE_LIMIT_LENGTH,MAX_LINE)
            target.executescript(DDL)
            source.execute('BEGIN IMMEDIATE'); target.execute('BEGIN IMMEDIATE')
            try:
                target.execute('INSERT INTO quarantine_meta VALUES(1,?,?,?,?,?)', (FORMAT,'inactive',export_sha256,raw,v1.canonical(v1.POLICY)))
                for table in manifest['content']:
                    source.execute('DELETE FROM '+v1.quote(table))
                for table, item in sorted(manifest['content'].items()):
                    op.check()
                    path = export_path.parent/item['file']; witness = original[str(path)]
                    if witness['bytes'] != item['bytes'] or witness['sha256'] != item['sha256']:
                        raise Refused('Pinned table bytes/hash mismatch')
                    target.execute('INSERT INTO source_tables VALUES(?,?,?,?)',(table,v1.canonical(item['columns']),item['rows'],item['sha256']))
                    if table == 'sqlite_sequence': source.execute('DELETE FROM sqlite_sequence')
                    stream = fence.streams[path]; stream.seek(0)
                    position = 0
                    while line := stream.readline(MAX_LINE+1):
                        op.row(); position += 1
                        row = parse(line,MAX_LINE)
                        fields(row,['rowid','cells'])
                        if not line.endswith(b'\n') or v1.canonical(row)+b'\n' != line:
                            raise Refused('Row bytes are not the canonical original encoding')
                        rowid = integer(row['rowid'])
                        if not isinstance(row['cells'],list) or len(row['cells']) != len(item['columns']):
                            raise Refused('Original typed column count differs')
                        decoded = [decode_cell(cell) for cell in row['cells']]
                        placeholders = ['CAST(? AS TEXT)' if kind == 'text' else '?' for _,kind in decoded]
                        query = 'INSERT INTO '+v1.quote(table)+'(_rowid_,'+','.join(v1.quote(c) for c in item['columns'])+') VALUES(?,'+','.join(placeholders)+')'
                        source.execute(query,[rowid,*[value for value,_ in decoded]])
                        target.execute('INSERT INTO history_rows VALUES(?,?,?,?,?)',(table,rowid,position,line,hashlib.sha256(line).hexdigest()))
                    if position != item['rows']:
                        raise Refused('Original complete row count differs')
                source.commit()
                if source.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)] or source.execute('PRAGMA foreign_key_check').fetchone() is not None:
                    raise Refused('Reconstructed historical integrity/FK check failed')
                # Re-read every typed cell and ordering from the known private schema.
                check_budget = v1.Budget(op.deadline)
                actual = v1.content_receipts(source,manifest['details'],check_budget)
                if actual != manifest['content']:
                    raise Refused('Reconstructed SQLite types/ordering differ from original rows')
                entity_index(source,target,manifest['details']['family'])
                if target.execute('PRAGMA foreign_key_check').fetchone() is not None or target.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)]:
                    raise Refused('Inactive quarantine integrity/FK check failed')
                op.check()
                target.commit()
            except BaseException:
                source.rollback(); target.rollback()
                raise
            rows = target.execute('SELECT count(*) FROM history_rows').fetchone()[0]
            entities = dict(target.execute('SELECT kind,count(*) FROM history_entities GROUP BY kind'))
        validation.unlink()
        cold(quarantine)
        if quarantine.stat().st_size > MAX_QUARANTINE:
            raise Refused('Inactive quarantine physical byte limit exceeded')
        value = v1.bundle_manifest({'format':FORMAT,'stage':'inactive_quarantine','state':'inactive',
                                   'execution_policy':v1.POLICY,'export_sha256':export_sha256,
                                   'source_profile':manifest['details']['profile'],
                                   'database':{'file':'quarantine.sqlite3','bytes':quarantine.stat().st_size,'sha256':v1.hash_file(quarantine,op)},
                                   'rows':rows,'entities':entities,'schema_sha256':quarantine_schema_sha()})
        result_sha = v1.write_json(stage.directory/'quarantine.json',value)
        exact_bundle_files(export_path.parent,[p.name for p in paths])
        if original != receipt_set(fence): raise Refused('Original export changed under fence')
        op.check(); stage.publish(op)
        return {'quarantine':str(stage.output/'quarantine.json'),'quarantine_sha256':result_sha,'state':'inactive','rows':rows,'entities':entities}


def quarantine_schema_sha():
    with contextlib.closing(sqlite3.connect(':memory:')) as conn:
        conn.executescript(DDL)
        return v1.signature(v1.schema(conn))


def validate_quarantine(conn, manifest):
    fields(manifest,['format','stage','state','execution_policy','export_sha256','source_profile','database','rows','entities','schema_sha256','manifest_sha256'])
    copy = dict(manifest); seal = copy.pop('manifest_sha256')
    if seal != v1.digest(copy) or manifest['format'] != FORMAT or manifest['stage'] != 'inactive_quarantine' or manifest['state'] != 'inactive' or v1.canonical(manifest['execution_policy']) != v1.canonical(v1.POLICY):
        raise Refused('Unknown quarantine format or active authority')
    natural(manifest['rows']);sha(manifest['export_sha256']);sha(manifest['schema_sha256'])
    if not isinstance(manifest['entities'],dict) or not set(manifest['entities']).issubset(MAPPABLE):
        raise Refused('Unknown quarantine entity counts')
    for count in manifest['entities'].values():natural(count)
    actual_schema = v1.signature(v1.schema(conn))
    if actual_schema != quarantine_schema_sha() or actual_schema != manifest['schema_sha256']:
        raise Refused('Unknown or changed quarantine schema')
    if conn.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)] or conn.execute('PRAGMA foreign_key_check').fetchone() is not None:
        raise Refused('Quarantine integrity/FK check failed')
    meta = conn.execute('SELECT * FROM quarantine_meta').fetchall()
    if len(meta) != 1 or meta[0][:4] != (1,FORMAT,'inactive',manifest['export_sha256']) or meta[0][5] != v1.canonical(v1.POLICY):
        raise Refused('Quarantine metadata has execution authority or different origin')
    export = parse(meta[0][4]); export_header(export)
    if hashlib.sha256(meta[0][4]).hexdigest() != manifest['export_sha256'] or export['details']['profile'] != manifest['source_profile']:
        raise Refused('Quarantine original export provenance differs')
    if conn.execute('SELECT count(*) FROM history_rows').fetchone()[0] != manifest['rows'] or dict(conn.execute('SELECT kind,count(*) FROM history_entities GROUP BY kind')) != manifest['entities']:
        raise Refused('Quarantine counts differ')
    return export


def audit_quarantine(conn, export, directory, op):
    """Derive IDs/associations again from all original typed rows, never trust projections."""
    validation, expected_path = directory/'history-audit.sqlite3', directory/'projection-audit.sqlite3'
    with contextlib.closing(known_source(export['details'])) as seed:
        with contextlib.closing(sqlite3.connect(validation)) as source:
            seed.backup(source)
    with contextlib.closing(sqlite3.connect(validation)) as source, contextlib.closing(sqlite3.connect(expected_path)) as expected:
        source.execute('PRAGMA foreign_keys=OFF')
        expected.execute('PRAGMA foreign_keys=OFF')
        expected.executescript(DDL)
        source.execute('BEGIN IMMEDIATE'); expected.execute('BEGIN IMMEDIATE')
        try:
            actual_tables = conn.execute('SELECT name,columns_json,row_count,content_sha256 FROM source_tables ORDER BY name').fetchall()
            expected_tables = [(name,v1.canonical(item['columns']),item['rows'],item['sha256']) for name,item in sorted(export['content'].items())]
            if actual_tables != expected_tables:
                raise Refused('Quarantine table metadata differs from the pinned original export')
            for table in export['content']:
                source.execute('DELETE FROM '+v1.quote(table))
            count = 0
            for table, item in sorted(export['content'].items()):
                if table == 'sqlite_sequence': source.execute('DELETE FROM sqlite_sequence')
                expected.execute('INSERT INTO source_tables VALUES(?,?,?,?)',(table,v1.canonical(item['columns']),item['rows'],item['sha256']))
                hasher = hashlib.sha256(); size = 0; position = 0
                with contextlib.closing(conn.execute('SELECT source_rowid,position,canonical_row,row_sha256 FROM history_rows WHERE table_name=? ORDER BY position',(table,))) as cursor:
                    for rowid,ordinal,raw,row_sha in cursor:
                        op.check(); count += 1; position += 1
                        if count > v1.MAX_ROWS or ordinal != position or hashlib.sha256(raw).hexdigest() != row_sha:
                            raise Refused('Quarantine row identity/order/hash differs')
                        row = parse(raw,MAX_LINE); fields(row,['rowid','cells'])
                        if integer(row['rowid']) != rowid or v1.canonical(row)+b'\n' != raw or not isinstance(row['cells'],list) or len(row['cells']) != len(item['columns']):
                            raise Refused('Quarantine original typed row differs')
                        decoded = [decode_cell(cell) for cell in row['cells']]
                        placeholders = ['CAST(? AS TEXT)' if kind == 'text' else '?' for _,kind in decoded]
                        query = 'INSERT INTO '+v1.quote(table)+'(_rowid_,'+','.join(v1.quote(c) for c in item['columns'])+') VALUES(?,'+','.join(placeholders)+')'
                        source.execute(query,[rowid,*[value for value,_ in decoded]])
                        expected.execute('INSERT INTO history_rows VALUES(?,?,?,?,?)',(table,rowid,position,raw,row_sha))
                        hasher.update(raw); size += len(raw)
                if position != item['rows'] or size != item['bytes'] or hasher.hexdigest() != item['sha256']:
                    raise Refused('Quarantine content is not the complete pinned original export')
            source.commit()
            if source.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)] or source.execute('PRAGMA foreign_key_check').fetchone() is not None:
                raise Refused('Quarantine original rows have an integrity/FK violation')
            if v1.content_receipts(source,export['details'],v1.Budget(op.deadline)) != export['content']:
                raise Refused('Quarantine SQLite types/order differ from original export')
            entity_index(source,expected,export['details']['family'])
            for table in ('history_projects','history_actors','history_entities','history_relations'):
                names = [r[1] for r in expected.execute('PRAGMA table_xinfo('+table+')')]
                query = 'SELECT * FROM '+table+' ORDER BY '+','.join(v1.quote(name) for name in names)
                with contextlib.closing(conn.execute(query)) as a, contextlib.closing(expected.execute(query)) as b:
                    sentinel = object()
                    while True:
                        op.check(); original,derived = next(a,sentinel),next(b,sentinel)
                        if original is sentinel and derived is sentinel: break
                        if original != derived: raise Refused('Quarantine ID/project/actor projection differs from original rows')
            expected.commit()
        except BaseException:
            source.rollback(); expected.rollback()
            raise
    validation.unlink(); expected_path.unlink()


def mapping_header(value, export_sha, target_sha):
    fields(value,['format','purpose','export_sha256','target_snapshot_sha256','entries'])
    if value['format'] != MAPPING_FORMAT or value['purpose'] != 'historical_preview' or value['export_sha256'] != export_sha or value['target_snapshot_sha256'] != target_sha:
        raise Refused('Mapping binds another origin, snapshot or purpose')
    if not isinstance(value['entries'],list) or len(value['entries']) > v1.MAX_ROWS:
        raise Refused('Bounded mapping entries required')


def read_target_ids(conn):
    result = {}
    uuid_tables = {'projects','tenant_projects','runs','artifacts','orders','identity_resources','identity_subjects','identity_organizations'}
    for table,metadata in v1.schema(conn)['tables'].items():
        if 'id' not in [column[1] for column in metadata['columns']]: continue
        # Only opaque IDs are inspected. Tokens, password columns, payloads and
        # credentials are never selected or copied into the preview artifact.
        with contextlib.closing(conn.execute('SELECT id,typeof(id) FROM '+v1.quote(table))) as cursor:
            for identity,storage_class in cursor:
                if storage_class != 'text':
                    # Installation/guard tables have integer IDs and cannot collide
                    # with a canonical UUID. Their values confer no authority here.
                    if storage_class == 'integer': continue
                    raise Refused('Invalid target entity ID storage class')
                if table in uuid_tables: uuid_id(identity)
                else: identifier(identity)
                result.setdefault(identity,set()).add(table)
    return result


def mapping_preview(source, target, mapping):
    known = read_target_ids(target)
    mapped = {}; proposed = {}; results = []
    for entry in mapping['entries']:
        fields(entry,['source_kind','source_id','target_kind','target_id','namespace','project_id','organization_id','creator_identity_id'])
        kind, identity = entry['source_kind'], identifier(entry['source_id'])
        if kind not in MAPPABLE:
            raise Refused('Unknown original mapping kind')
        key = (kind,identity)
        if key in mapped:
            raise Refused('Multivalued or duplicate original mapping')
        original = source.execute('SELECT project_source_id,actor_source_id FROM history_entities WHERE kind=? AND source_id=?',key).fetchone()
        if original is None:
            raise Refused('Mapping references an absent original ID')
        mapped[key] = entry
        dest = uuid_id(entry['target_id']); namespace = entry['namespace']
        expected_kind = 'project' if kind == 'project' else 'run' if kind in RUN_KINDS else 'artifact' if kind in ARTIFACT_KINDS else 'identity_subject' if kind == 'historical_actor' else kind
        if entry['target_kind'] != expected_kind:
            raise Refused('Human identities and agent personas cannot be conflated')
        if dest in proposed:
            raise Refused('Multiple original entities target the same V2 ID')
        proposed[dest] = key
        history = kind in ('historical_actor','agent_persona','thread','crew_run')
        if history:
            if namespace != 'history' or any(entry[x] is not None for x in ('project_id','organization_id','creator_identity_id')):
                raise Refused('Historical actor/persona/thread must remain nonauthoritative history')
        else:
            if namespace not in ('personal','tenant'):
                raise Refused('Explicit personal or tenant project namespace required')
            uuid_id(entry['project_id'])
            if kind == 'project' and entry['project_id'] != dest:
                raise Refused('Project mapping target and project pin differ')
            if namespace == 'personal':
                if entry['organization_id'] is not None or entry['creator_identity_id'] is not None:
                    raise Refused('Personal history cannot inherit tenant identity')
            else:
                org, actor = uuid_id(entry['organization_id']), uuid_id(entry['creator_identity_id'])
                if 'identity_organizations' not in known.get(org,set()) or 'identity_subjects' not in known.get(actor,set()):
                    raise Refused('Tenant mapping requires existing pinned organization/creator IDs')
        existing = known.get(dest,set())
        facts = None
        if kind == 'project':
            if namespace == 'personal':
                if existing and existing != {'projects'}:
                    raise Refused('Personal target ID is already in another namespace')
                row = target.execute('SELECT payload FROM projects WHERE id=?',(dest,)).fetchone()
                facts = None if row is None else {'namespace':'personal','payload_sha256':hashlib.sha256(row[0].encode()).hexdigest()}
            else:
                if existing and existing != {'tenant_projects','identity_resources'}:
                    raise Refused('Tenant project cannot adopt an unrelated existing resource')
                row = target.execute('SELECT p.organization_id,p.creator_identity_id,p.revision,r.organization_id,r.kind,r.revision,p.creation_key,p.request_sha256,p.creation_payload,p.payload,p.initial_receipt FROM tenant_projects p JOIN identity_resources r ON r.id=p.id WHERE p.id=?',(dest,)).fetchone()
                if row is not None:
                    if row[0] != entry['organization_id'] or row[1] != entry['creator_identity_id'] or row[3] != row[0] or row[4] != 'project':
                        raise Refused('Tenant resource organization/kind/creator pin differs')
                    creation, payload, initial = [parse(value.encode('utf-8')) for value in row[8:11]]
                    fields(creation,['name','idempotency_key'])
                    fields(payload,['id','organization_id','name','revision','created_at'])
                    fields(initial,['id','organization_id','name','revision','created_at'])
                    name, intent = creation['name'], creation['idempotency_key']
                    if not isinstance(name,str) or not name.strip() or len(name.encode('utf-8'))>200 or any(ord(c)<32 or ord(c)==127 for c in name):
                        raise Refused('Invalid original tenant project creation name')
                    if not isinstance(intent,str) or not intent.strip() or len(intent.encode('utf-8'))>128 or any(ord(c)<32 or ord(c)==127 for c in intent):
                        raise Refused('Invalid original tenant project creation key')
                    import json
                    request = json.dumps(['tenant_project_create/v1',row[0],row[1],{'name':name,'idempotency_key':intent}],ensure_ascii=False,separators=(',',':')).encode('utf-8')
                    if payload != initial or payload['id'] != dest or payload['organization_id'] != row[0] or type(payload['revision']) is not int or payload['revision'] != row[2] or row[2] != 1 or type(row[5]) is not int or row[5]<=0 or type(payload['created_at']) is not int or payload['created_at']<=0 or payload['name'] != name.strip() or intent != row[6] or hashlib.sha256(request).hexdigest() != row[7]:
                        raise Refused('Tenant project payload/creation receipt binding differs')
                    facts = {'namespace':'tenant','organization_id':row[0],'creator_identity_id':row[1],'project_revision':row[2],'resource_revision':row[5]}
        elif kind in RUN_KINDS and kind != 'crew_run':
            if existing and existing != {'runs'}:
                raise Refused('Run target ID already belongs to another entity namespace')
            row = target.execute('SELECT o.project_id,r.revision FROM runs r JOIN orders o ON o.id=r.order_id WHERE r.id=?',(dest,)).fetchone()
            if row is not None:
                if namespace != 'personal' or row[0] != entry['project_id'] or type(row[1]) is not int or row[1]<0:
                    raise Refused('Run target belongs to another project/namespace')
                facts = {'project_id':row[0],'revision':row[1]}
        elif kind in ARTIFACT_KINDS:
            if existing and existing != {'artifacts'}:
                raise Refused('Artifact target ID already belongs to another namespace')
            row = target.execute('SELECT o.project_id,a.run_id FROM artifacts a JOIN orders o ON o.id=a.order_id WHERE a.id=?',(dest,)).fetchone()
            if row is not None:
                if namespace != 'personal' or row[0] != entry['project_id']:
                    raise Refused('Artifact target belongs to another project/namespace')
                facts = {'project_id':row[0],'run_id':row[1]}
        elif kind == 'historical_actor':
            if existing != {'identity_subjects'}:
                raise Refused('Historical actor may only refer to one existing V2 human ID')
            row = target.execute('SELECT active,revision FROM identity_subjects WHERE id=?',(dest,)).fetchone()
            if type(row[0]) is not int or row[0] not in (0,1) or type(row[1]) is not int or row[1]<=0:
                raise Refused('Invalid existing target human metadata types')
            facts = {'active':row[0],'revision':row[1]}
        elif existing:
            raise Refused('Historical content/persona target ID collides with a V2 operational entity')
        results.append({'mapping':entry,'diff':'existing_target_reference' if facts is not None else 'proposed_new_history_id',
                        'target_facts':facts,'source_project_id':original[0],'source_actor_id':original[1],
                        'grants_created':0,'activation_allowed':False})
    for key, entry in mapped.items():
        original,actor = source.execute('SELECT project_source_id,actor_source_id FROM history_entities WHERE kind=? AND source_id=?',key).fetchone()
        if entry['namespace'] != 'history' and key[0] != 'project':
            project = mapped.get(('project',original))
            if original is None or project is None or any(entry[x] != project[x] for x in ('namespace','project_id','organization_id','creator_identity_id')):
                raise Refused('Content mapping is missing its original project or crosses project pins')
            if entry['namespace'] == 'tenant' and actor is not None:
                actor_map = mapped.get(('historical_actor',actor))
                if actor_map is None or actor_map['target_id'] != entry['creator_identity_id']:
                    raise Refused('Tenant content creator differs from its explicitly mapped original actor')
        if key[0] in ARTIFACT_KINDS:
            linked = source.execute('SELECT source_kind,source_id FROM history_relations WHERE kind=? AND target_kind=? AND target_id=?',('run_artifact',*key)).fetchall()
            if len(linked) != 1 or tuple(linked[0]) not in mapped:
                raise Refused('Artifact mapping is missing its exact original run mapping')
            mapped_run = mapped[tuple(linked[0])]
            if any(entry[x] != mapped_run[x] for x in ('namespace','project_id','organization_id','creator_identity_id')):
                raise Refused('Artifact mapping crosses original run/project pins')
            target_run = target.execute('SELECT run_id FROM artifacts WHERE id=?',(entry['target_id'],)).fetchone()
            if target_run is not None and target_run[0] != mapped_run['target_id']:
                raise Refused('Existing target artifact belongs to another mapped run')
    unmapped = source.execute('SELECT kind,source_id FROM history_entities ORDER BY kind,source_id').fetchall()
    return {'entries':results,'unmapped':[{'source_kind':kind,'source_id':identity} for kind,identity in unmapped if (kind,identity) not in mapped],
            'complete_mapping':len(unmapped)==len(mapped),'fresh_principal_required_for_activation':True}


def preview(quarantine_path, quarantine_sha256, target_path, target_sha256, mapping_path, mapping_sha256, output, profile_source_root=None):
    paths = [v1.bounded_path(p) for p in (quarantine_path,target_path,mapping_path)]
    quarantine_path,target_path,mapping_path = paths
    database_path = v1.bounded_path(quarantine_path.parent/'quarantine.sqlite3')
    refs = target_profile.source_paths(profile_source_root)
    paths += [database_path,*[v1.bounded_path(p) for p in refs]]
    for expected in (quarantine_sha256,target_sha256,mapping_sha256): sha(expected)
    op = budget()
    with Stage(output,paths) as stage, FileFence(paths,op,MAX_QUARANTINE+v1.MAX_SOURCE+8*v1.MAX_MANIFEST) as fence:
        original = receipt_set(fence)
        if original[str(target_path)]['bytes'] > v1.MAX_SOURCE or original[str(database_path)]['bytes'] > MAX_QUARANTINE:
            raise Refused('Target snapshot/quarantine physical byte limit exceeded')
        cold(target_path); cold(database_path)
        exact_bundle_files(quarantine_path.parent,['quarantine.json','quarantine.sqlite3'])
        for path,expected in ((quarantine_path,quarantine_sha256),(target_path,target_sha256),(mapping_path,mapping_sha256),*refs.items()):
            if original[str(path)]['sha256'] != expected:
                raise Refused('Pinned quarantine/target/mapping/reference source byte hash mismatch')
        manifest = parse(fence.read(quarantine_path,v1.MAX_MANIFEST))
        fields(manifest['database'],['file','bytes','sha256'])
        if manifest['database'] != {'file':'quarantine.sqlite3','bytes':original[str(database_path)]['bytes'],'sha256':original[str(database_path)]['sha256']}:
            raise Refused('Pinned inactive quarantine database differs')
        mapping = parse(fence.read(mapping_path,v1.MAX_MANIFEST))
        mapping_header(mapping,manifest['export_sha256'],target_sha256)
        qcopy,tcopy = stage.directory/'quarantine-read.sqlite3',stage.directory/'target-read.sqlite3'
        fence.copy(database_path,qcopy); fence.copy(target_path,tcopy)
        with v1.database(qcopy) as historical, v1.database(tcopy) as target:
            historical.setlimit(sqlite3.SQLITE_LIMIT_LENGTH,MAX_LINE)
            historical.set_progress_handler(op.sql_progress,1000)
            target.set_progress_handler(op.sql_progress,1000)
            original_export = validate_quarantine(historical,manifest)
            audit_quarantine(historical,original_export,stage.directory,op)
            target_receipt = target_profile.inspect(target)
            diff = mapping_preview(historical,target,mapping)
        qcopy.unlink(); tcopy.unlink()
        result = v1.bundle_manifest({'format':PREVIEW_FORMAT,'purpose':'historical_preview','state':'inactive',
                                    'execution_policy':v1.POLICY,'quarantine_sha256':quarantine_sha256,
                                    'export_sha256':manifest['export_sha256'],'target_snapshot_sha256':target_sha256,
                                    'mapping_sha256':mapping_sha256,'target_profile':target_receipt,'diff':diff,
                                    'target_sessions_queried_for_authority':False,'target_credentials_exported':False,
                                    'target_database_writes':0,'grants_created':0,'activation_allowed':False})
        result_sha = v1.write_json(stage.directory/'preview.json',result)
        cold(target_path); cold(database_path)
        exact_bundle_files(quarantine_path.parent,['quarantine.json','quarantine.sqlite3'])
        if original != receipt_set(fence): raise Refused('Pinned preview inputs changed under fence')
        op.check(); stage.publish(op)
        return {'preview':str(stage.output/'preview.json'),'preview_sha256':result_sha,'state':'inactive',
                'mapped':len(diff['entries']),'unmapped':len(diff['unmapped']),'activation_allowed':False}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command',required=True)
    create = commands.add_parser('quarantine')
    create.add_argument('--export',required=True); create.add_argument('--export-sha256',required=True)
    create.add_argument('--output',required=True)
    show = commands.add_parser('preview')
    for name in ('quarantine','quarantine-sha256','target-snapshot','target-snapshot-sha256','mapping','mapping-sha256','output'):
        show.add_argument('--'+name,required=True)
    show.add_argument('--profile-source-root',help='Optional original source root; every fixed source digest must match')
    args = parser.parse_args(argv)
    try:
        if args.command == 'quarantine': result = create_quarantine(args.export,args.export_sha256,args.output)
        else: result = preview(args.quarantine,args.quarantine_sha256,args.target_snapshot,args.target_snapshot_sha256,args.mapping,args.mapping_sha256,args.output,args.profile_source_root)
        print(v1.canonical(result).decode('ascii'))
        return 0
    except (Refused,sqlite3.Error,OSError,ValueError,TypeError,KeyError,UnicodeError) as error:
        print(v1.canonical({'state':'refused','error':str(error),'activation_allowed':False}).decode('ascii'),file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
