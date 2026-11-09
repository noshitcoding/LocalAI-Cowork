"""Copy the bounded original server build context into a new ordinary directory.

No dependency installation, Docker build, service start or credential discovery.
The context includes only explicit source trees and the fixed historical compiler
fixtures already admitted by the release manifest. Operator state is excluded.
"""
from __future__ import annotations
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('source_bundle_release', ROOT / 'tools/desktop/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

SOURCE_TREES = ('apps/web', 'packages/client', 'packages/ui', 'tools/server', 'tools/toolbox', 'tools/browser', 'extensions/browser', 'docs')
EXCLUDED = {'node_modules', 'dist', 'target', '.git', '__pycache__', '.local', 'binaries', 'gen'}
PRIVATE_NAMES = {'daemon.token', 'native.token', 'toolbox.json'}
PRIVATE_SUFFIXES = {'.key', '.pem', '.pfx', '.p12', '.token', '.db', '.sqlite', '.sqlite3'}


def manifest() -> dict:
    records = release.source_manifest(ROOT)
    paths = {ROOT / name for name in ('README.md', 'LICENSE', 'package.json', 'package-lock.json',
                                    'tsconfig.json', 'tools/release/source_bundle.py',
                                    'tools/desktop/frontend.py', 'tools/server/package.py')}
    for tree in SOURCE_TREES:
        for parent, directories, files in os.walk(ROOT / tree, followlinks=False):
            directories[:] = sorted(name for name in directories if name not in EXCLUDED)
            for name in directories:
                path = Path(parent) / name
                info = path.lstat()
                if path.is_symlink() or getattr(info, 'st_file_attributes', 0) & 0x400:
                    raise release.Refused('aliased source directories are not a build context')
            for name in files:
                path = Path(parent) / name
                if (name in PRIVATE_NAMES or path.suffix.lower() in PRIVATE_SUFFIXES
                        or name == '.env' or name.startswith('.env.') and not name.endswith('.example')):
                    continue
                paths.add(path)
    for path in sorted(paths):
        release.regular(path)
        name = path.relative_to(ROOT).as_posix()
        records[name] = {'size': path.stat().st_size, 'sha256': release.sha256(path)}
    if len(records) > 8192 or sum(row['size'] for row in records.values()) > 256 * 1024 * 1024:
        raise release.Refused('the original build source closure exceeds its bound')
    return dict(sorted(records.items()))


def prepare(destination: Path, expected: dict) -> dict:
    destination = destination.absolute()
    if destination.exists() or destination.is_symlink() or not destination.parent.is_dir():
        raise release.Refused('a new source directory with an existing parent is required')
    if manifest() != expected:
        raise release.Refused('the original source changed before its build-context copy')
    destination.mkdir(mode=0o700)
    for name, original in expected.items():
        source = release.regular(ROOT / name)
        if source.stat().st_size != original['size'] or release.sha256(source) != original['sha256']:
            raise release.Refused('the original source changed before its file copy')
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        if target.stat().st_size != original['size'] or release.sha256(target) != original['sha256']:
            raise release.Refused('the copied build source differs')
    if manifest() != expected:
        raise release.Refused('the original source changed during its build-context copy')
    for name, original in expected.items():
        target = release.regular(destination / name)
        if target.stat().st_size != original['size'] or release.sha256(target) != original['sha256']:
            raise release.Refused('the complete copied build source changed before its receipt')
    result = {'format': 'localai-cowork-server-build-source/1', 'files': expected,
              'source_before_after_equal': True, 'dependencies_installed': False,
              'container_image_built': False, 'runtime_qualified': False, 'release_qualified': False}
    raw = (json.dumps(result, sort_keys=True, indent=2) + '\n').encode('utf-8')
    receipt = destination / 'BUILD-SOURCE-RECEIPT.json'
    with receipt.open('xb') as file:
        file.write(raw)
    return {'files': len(expected), 'bytes': sum(row['size'] for row in expected.values()),
            'manifest_sha256': hashlib.sha256(raw).hexdigest(),
            'filename': 'BUILD-SOURCE-RECEIPT.json', 'source_before_after_equal': True}
