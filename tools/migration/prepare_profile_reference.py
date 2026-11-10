"""Prepare the exact immutable historical V2 preview profile sources.

Only the 25 fixed source files named by v2_snapshot_profile are admitted.
No archive-supplied paths, code execution, live data or operational rights.
"""
from __future__ import annotations
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import sys
import zipfile
import v1_sqlite_export as v1
import v2_snapshot_profile as profile
from migration_io import FileFence, Stage, sha, budget

FORMAT = 'localai-cowork-historical-target-profile-sources/1'
MAX_ARCHIVE = 8 * 1024 * 1024
MAX_SOURCE = 2 * 1024 * 1024


def prepare(archive_path, archive_sha256, output):
    archive_path = v1.bounded_path(archive_path)
    sha(archive_sha256)
    op = budget()
    with FileFence([archive_path], op, MAX_ARCHIVE) as fence, Stage(output, [archive_path]) as stage:
        receipt = fence.receipt(archive_path)
        if receipt['sha256'] != archive_sha256:
            raise v1.Refused('Original historical profile source archive differs')
        raw = fence.read(archive_path, MAX_ARCHIVE)
        selected = {}
        total = 0
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            names = archive.namelist()
            if len(names) > 10000 or len(names) != len(set(names)):
                raise v1.Refused('Ambiguous or oversized historical source archive')
            for relative, expected in profile.SOURCE_BINDINGS.items():
                name = 'sources/' + relative
                parts = PurePosixPath(relative)
                if parts.is_absolute() or '..' in parts.parts or '\\' in relative or name not in names:
                    raise v1.Refused('Missing exact historical profile source')
                member = archive.getinfo(name)
                if member.is_dir() or member.file_size > MAX_SOURCE or (member.external_attr >> 16) & 0o170000 == 0o120000:
                    raise v1.Refused('Nonordinary or oversized historical profile source')
                total += member.file_size
                if total > MAX_SOURCE:
                    raise v1.Refused('Historical profile source closure exceeds bound')
                data = archive.read(member)  # actual CRC checked by ZipFile
                if hashlib.sha256(data).hexdigest() != expected:
                    raise v1.Refused('Historical source bytes do not match the fixed profile')
                selected[relative] = data
        for relative, data in selected.items():
            destination = stage.directory.joinpath(*PurePosixPath(relative).parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with destination.open('xb') as stream:
                stream.write(data)
                stream.flush()
                __import__('os').fsync(stream.fileno())
        manifest = v1.bundle_manifest({'format':FORMAT,'profile':profile.PROFILE,
                                       'archive_sha256':archive_sha256,'source_bindings':profile.SOURCE_BINDINGS,
                                       'historical_only':True,'activation_allowed':False})
        manifest_sha = v1.write_json(stage.directory/'reference.json', manifest)
        if receipt != fence.receipt(archive_path):
            raise v1.Refused('Historical profile archive changed')
        op.check()
        stage.publish(op)
        return {'format':FORMAT,'output':str(stage.output),'files':len(selected),
                'bytes':total,'reference_sha256':manifest_sha,'archive_sha256':archive_sha256,
                'historical_only':True,'activation_allowed':False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', required=True)
    parser.add_argument('--archive-sha256', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(prepare(args.archive,args.archive_sha256,args.output), sort_keys=True))
        return 0
    except (v1.Refused,OSError,ValueError,zipfile.BadZipFile) as error:
        print('REFUSED: '+str(error),file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
