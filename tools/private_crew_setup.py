"""Build offline wheel pins and a new private, non-secret Crew operator profile.

No package installation, provider call, image activation or existing file mutation.
"""
import argparse
import email.parser
import hashlib
import json
import os
from pathlib import Path
import re
import zipfile


def write_new(path, raw, private=False):
    path = Path(path).absolute()
    if not path.parent.is_dir() or path.is_symlink():
        raise ValueError('select an existing output directory and a new regular file')
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600 if private else 0o644)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        # Preserve a failed exclusive write for operator inspection, never
        # overwrite an existing configuration or claim an installed profile.
        raise
    return path


def lock(args):
    root = Path(args.wheels).resolve(strict=True)
    if not root.is_dir():
        raise ValueError('wheel directory required')
    packages = {}
    files = sorted(root.iterdir())
    if not files or len(files) > 1000:
        raise ValueError('a bounded, complete offline wheel closure is required')
    total = 0
    for path in files:
        if path.is_symlink() or not path.is_file() or path.suffix != '.whl':
            raise ValueError('wheel directory must contain regular .whl files only')
        total += path.stat().st_size
        if total > 4 * 1024**3:
            raise ValueError('offline wheel closure exceeds 4 GiB')
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            while data := stream.read(1024**2):
                digest.update(data)
        with zipfile.ZipFile(path) as archive:
            metadata = [i for i in archive.infolist() if i.filename.endswith('.dist-info/METADATA')]
            if len(metadata) != 1 or metadata[0].file_size > 1024**2:
                raise ValueError('wheel identity metadata is ambiguous or oversized')
            identity = email.parser.BytesParser().parsebytes(archive.read(metadata[0]))
        name, version = identity['Name'], identity['Version']
        if not name or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', name) or not version or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.!+_-]*', version):
            raise ValueError('invalid wheel package identity')
        name = re.sub(r'[-_.]+', '-', name).lower()
        if name in packages:
            raise ValueError('select one version/wheel per package for the actual target Python/platform')
        packages[name] = (version, digest.hexdigest())
    if packages.get('crewai', (None,))[0] != args.crewai_version:
        raise ValueError('the selected explicit CrewAI version is not in this original closure')
    raw = ''.join(f'{name}=={version} --hash=sha256:{digest}\n' for name, (version, digest) in sorted(packages.items())).encode('ascii')
    path = write_new(args.output, raw)
    print(json.dumps({'path': str(path), 'sha256': hashlib.sha256(raw).hexdigest(), 'packages': len(packages), 'installed': False}))


def configure(args):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9./_:-]*@sha256:[a-f0-9]{64}', args.image):
        raise ValueError('select the exact observed immutable Crew image digest')
    path = Path(args.inventory).resolve(strict=True)
    if not path.is_file() or path.stat().st_size > 4096:
        raise ValueError('actual bounded Crew inventory output required')
    inventory = json.loads(path.read_bytes())
    if set(inventory) != {'version', 'source_sha256'} or not isinstance(inventory['version'], str) or not 1 <= len(inventory['version']) <= 100 or not re.fullmatch(r'[a-f0-9]{64}', inventory['source_sha256']):
        raise ValueError('invalid original installed distribution inventory')
    bridge = Path(__file__).resolve().parent.parent / 'deploy/v2/android-private-crew.py'
    result = {'format': 1, 'image': args.image, 'crewai_version': inventory['version'], 'crewai_source_sha256': inventory['source_sha256'], 'bridge_sha256': hashlib.sha256(bridge.read_bytes()).hexdigest()}
    raw = (json.dumps(result, indent=2) + '\n').encode('utf-8')
    output = write_new(args.output, raw, private=True)
    print(json.dumps({'path': str(output), 'sha256': hashlib.sha256(raw).hexdigest(), 'activated': False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    pin = commands.add_parser('lock')
    pin.add_argument('--wheels', required=True)
    pin.add_argument('--crewai-version', required=True)
    pin.add_argument('--output', required=True)
    profile = commands.add_parser('configure')
    profile.add_argument('--inventory', required=True)
    profile.add_argument('--image', required=True)
    profile.add_argument('--output', required=True)
    args = parser.parse_args()
    (lock if args.command == 'lock' else configure)(args)


if __name__ == '__main__':
    main()
