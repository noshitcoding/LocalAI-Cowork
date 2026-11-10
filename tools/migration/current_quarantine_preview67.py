"""Inert schema67 preview; original schema20/64/66 producers are unchanged."""
from __future__ import annotations
import argparse
import sqlite3
import sys
import v1_sqlite_export as v1
import v1_quarantine as historical
import current_quarantine_preview as previous
import current_target_profiles67 as targets
from migration_io import Refused, FileFence, Stage, parse, fields, sha, budget, receipt_set, cold, MAX_QUARANTINE


def preview(quarantine_path, quarantine_sha256, target_path, target_sha256,
            mapping_path, mapping_sha256, output, target_profile, profile_source_root=None):
    if target_profile in targets.previous.ALLOWED_PROFILES:
        return previous.preview(quarantine_path, quarantine_sha256, target_path, target_sha256,
                                mapping_path, mapping_sha256, output, target_profile, profile_source_root)
    profile, profile_path, profile_sha = targets.load(target_profile)
    quarantine_path, target_path, mapping_path = [v1.bounded_path(p) for p in (quarantine_path, target_path, mapping_path)]
    database_path = v1.bounded_path(quarantine_path.parent / 'quarantine.sqlite3')
    refs = targets.source_paths(profile, profile_source_root)
    producers = targets.producer_paths(profile)
    paths = [quarantine_path, target_path, mapping_path, database_path, profile_path, *refs, *producers]
    for expected in (quarantine_sha256, target_sha256, mapping_sha256):
        sha(expected)
    op = budget()
    maximum = MAX_QUARANTINE + v1.MAX_SOURCE + 2 * 256 * 1024 * 1024 + targets.MAX_REFERENCES
    with Stage(output, paths) as stage, FileFence(paths, op, maximum) as fence:
        original = receipt_set(fence)
        if original[str(target_path)]['bytes'] > v1.MAX_SOURCE or original[str(database_path)]['bytes'] > MAX_QUARANTINE:
            raise Refused('Cold target/quarantine physical byte limit exceeded')
        if sum(original[str(path)]['bytes'] for path in (*refs, *producers, profile_path)) > targets.MAX_REFERENCES:
            raise Refused('Schema67 fixed reference closure byte limit exceeded')
        cold(target_path)
        cold(database_path)
        historical.exact_bundle_files(quarantine_path.parent, ['quarantine.json', 'quarantine.sqlite3'])
        expected = [(quarantine_path, quarantine_sha256), (target_path, target_sha256),
                    (mapping_path, mapping_sha256), (profile_path, profile_sha), *refs.items(), *producers.items()]
        for path, digest in expected:
            if original[str(path)]['sha256'] != digest:
                raise Refused('Pinned schema67 preview input/source bytes differ')
        manifest = parse(fence.read(quarantine_path, v1.MAX_MANIFEST))
        fields(manifest['database'], ['file', 'bytes', 'sha256'])
        if manifest['database'] != {'file': 'quarantine.sqlite3', 'bytes': original[str(database_path)]['bytes'],
                                    'sha256': original[str(database_path)]['sha256']}:
            raise Refused('Pinned original quarantine database differs')
        mapping = parse(fence.read(mapping_path, 256 * 1024 * 1024), 256 * 1024 * 1024)
        historical.mapping_header(mapping, manifest['export_sha256'], target_sha256)
        qcopy, tcopy = stage.directory / 'quarantine-read.sqlite3', stage.directory / 'target-read.sqlite3'
        fence.copy(database_path, qcopy)
        fence.copy(target_path, tcopy)
        with v1.database(qcopy) as source, v1.database(tcopy) as target:
            source.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, 256 * 1024 * 1024)
            source.set_progress_handler(op.sql_progress, 1000)
            target.set_progress_handler(op.sql_progress, 1000)
            export = historical.validate_quarantine(source, manifest)
            historical.audit_quarantine(source, export, stage.directory, op)
            target_receipt = targets.inspect(target, profile, profile_sha)
            target_id_rows = 0
            for table, metadata in profile['schema']['tables'].items():
                if any(column[1] == 'id' for column in metadata['columns']):
                    target_id_rows += target.execute('SELECT count(*) FROM ' + v1.quote(table)).fetchone()[0]
                    if target_id_rows > v1.MAX_ROWS:
                        raise Refused('Complete cold target opaque ID count limit')
            diff = historical.mapping_preview(source, target, mapping)
        qcopy.unlink()
        tcopy.unlink()
        result = v1.bundle_manifest({
            'format': targets.PREVIEW_FORMAT, 'purpose': 'historical_preview', 'state': 'inactive',
            'execution_policy': v1.POLICY, 'quarantine_sha256': quarantine_sha256,
            'export_sha256': manifest['export_sha256'], 'target_snapshot_sha256': target_sha256,
            'mapping_sha256': mapping_sha256, 'target_profile': target_receipt, 'diff': diff,
            'target_sessions_queried_for_authority': False, 'target_credentials_exported': False,
            'target_database_writes': 0, 'grants_created': 0, 'activation_allowed': False})
        result_sha = v1.write_json(stage.directory / 'preview.json', result)
        cold(target_path)
        cold(database_path)
        historical.exact_bundle_files(quarantine_path.parent, ['quarantine.json', 'quarantine.sqlite3'])
        if original != receipt_set(fence):
            raise Refused('Schema67 preview inputs changed under original fence')
        op.check()
        stage.publish(op)
        return {'preview': str(stage.output / 'preview.json'), 'preview_sha256': result_sha,
                'state': 'inactive', 'target_profile': target_profile, 'profile_manifest_sha256': profile_sha,
                'mapped': len(diff['entries']), 'unmapped': len(diff['unmapped']), 'activation_allowed': False}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target-profile', required=True,
                        choices=['v2-storage-20-identity-1-tenant-1-money-ddl-1', *targets.ALLOWED_PROFILES])
    for name in ('quarantine', 'quarantine-sha256', 'target-snapshot', 'target-snapshot-sha256',
                 'mapping', 'mapping-sha256', 'output'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--profile-source-root')
    args = parser.parse_args(argv)
    try:
        parameters = (args.quarantine, args.quarantine_sha256, args.target_snapshot,
                      args.target_snapshot_sha256, args.mapping, args.mapping_sha256, args.output)
        if args.target_profile == 'v2-storage-20-identity-1-tenant-1-money-ddl-1':
            result = historical.preview(*parameters, args.profile_source_root)
        else:
            result = preview(*parameters, args.target_profile, args.profile_source_root)
        print(v1.canonical(result).decode('ascii'))
        return 0
    except (Refused, sqlite3.Error, OSError, ValueError, TypeError, KeyError, UnicodeError) as error:
        print(v1.canonical({'state': 'refused', 'error': str(error), 'activation_allowed': False}).decode('ascii'), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
