"""Build-time software inventory for the delivered Debian container image.

Run only while constructing the image, before keys/volumes/user data are added.
Reads /usr software bytes and original dpkg status/list metadata. Does not read
/etc secrets, home directories, application data, or execute inventoried tools.
"""
from __future__ import annotations

import argparse
from email.parser import BytesParser
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import stat
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('cowork_container_sbom', HERE / 'sbom.py')
sbom = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sbom)


def software_tree(root):
    """Record aliases without following them; actual files stay inside /usr."""
    root = sbom.runtime.ordinary(root, directory=True)
    records, total = {}, 0
    def visit(folder):
        nonlocal total
        for path in sorted(folder.iterdir()):
            name = sbom.runtime.relative(path.relative_to(root).as_posix())
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode):
                target = os.readlink(path)
                raw = target.encode()
                records[name] = {'link': target, 'sha256': hashlib.sha256(raw).hexdigest(), 'size': len(raw)}
            elif stat.S_ISDIR(info.st_mode):
                visit(path)
            elif stat.S_ISREG(info.st_mode):
                record = sbom.runtime.pin(path)
                total += record['size']
                if total > sbom.runtime.MAX_BYTES:
                    raise ValueError('Container software bytes exceed inventory bound')
                native = sbom.runtime.native_imports(path)
                if native:
                    record['native'] = native
                records[name] = record
            else:
                raise ValueError('Container software tree contains a special file')
            if len(records) > sbom.runtime.MAX_FILES:
                raise ValueError('Container software count exceeds inventory bound')
    visit(root)
    return records


def dpkg_components(status_path, info_root, software_root, records):
    original = sbom.runtime.pin(status_path)
    if original['size'] > 32 * 1024**2:
        raise ValueError('dpkg status metadata exceeds bound')
    result = []
    for paragraph in Path(status_path).read_bytes().split(b'\n\n'):
        if not paragraph.strip():
            continue
        metadata = BytesParser().parsebytes(paragraph)
        if metadata.get('Status') != 'install ok installed':
            continue
        name, version, architecture = (metadata.get(key) for key in ('Package', 'Version', 'Architecture'))
        if not name or not version or not architecture or any(c in name + architecture for c in '/\\\0\r\n'):
            raise ValueError('Invalid installed dpkg identity')
        variants = [Path(info_root) / (name + ':' + architecture + '.list'), Path(info_root) / (name + '.list')]
        present = [path for path in variants if path.is_file()]
        if len(present) != 1:
            raise ValueError('Installed dpkg file ownership metadata missing or ambiguous')
        listed = sbom.runtime.pin(present[0])
        if listed['size'] > sbom.runtime.MAX_HEADER:
            raise ValueError('dpkg file ownership exceeds bound')
        owned = []
        for line in present[0].read_text(encoding='utf-8').splitlines():
            path = PurePosixPath(line)
            if not path.is_absolute() or '\0' in line:
                raise ValueError('Invalid dpkg file ownership path')
            # Lexical /usr namespace only. /etc and aliases outside it select no
            # reads and cannot cause secret files to enter this inventory.
            selected=None
            if path.is_relative_to(PurePosixPath('/usr')):selected=software_root/Path(*path.relative_to(PurePosixPath('/usr')).parts)
            else:
                # Debian merged-/usr lists retain historic /bin,/lib names.
                # Only these fixed software aliases are mapped; /etc,/home,
                # /var and arbitrary metadata paths never select reads.
                for alias in ('bin','sbin','lib','lib64'):
                    anchor=PurePosixPath('/')/alias
                    if path.is_relative_to(anchor):
                        selected=software_root/alias/Path(*path.relative_to(anchor).parts)
                        break
            if selected is not None:
                relative=selected.relative_to(software_root).as_posix()
                if relative not in records:
                    resolved=selected.resolve(strict=False)
                    if resolved.is_relative_to(software_root):relative=resolved.relative_to(software_root).as_posix()
                if relative in records:owned.append(relative)
        if sbom.runtime.pin(present[0]) != listed:
            raise ValueError('dpkg ownership metadata changed during inventory')
        requires = []
        for field in ('Depends', 'Pre-Depends'):
            for group in (metadata.get(field) or '').replace('\n', ' ').split(','):
                alternatives = [part.strip().split(' ', 1)[0].split(':', 1)[0] for part in group.split('|') if part.strip()]
                if alternatives:
                    # Alternatives are retained as original source information;
                    # no arbitrary choice is turned into a dependency edge.
                    requires.append(' | '.join(alternatives))
        result.append({'name': name, 'version': version, 'ecosystem': 'deb', 'license': 'NOASSERTION',
                       'purl': 'pkg:deb/debian/' + quote(name, safe='') + '@' + quote(version, safe='') + '?arch=' + quote(architecture, safe=''),
                       'files': sorted(set(owned)),
                       'source': 'Actual installed dpkg status; ownership list SHA256 ' + listed['sha256'] + '; declared alternatives: ' + ', '.join(requires)})
    if sbom.runtime.pin(status_path) != original:
        raise ValueError('Installed dpkg status changed during inventory')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--software-root', type=Path, default=Path('/usr'))
    args = parser.parse_args()
    if os.name != 'posix' or os.geteuid() != 0 or os.environ.get('COWORK_CONTAINER_BUILD_INVENTORY') != '1':
        raise ValueError('Explicit fresh container build inventory is required')
    root = sbom.runtime.ordinary(args.software_root, directory=True)
    if root != Path('/usr') or not args.output.is_absolute() or args.output.exists():
        raise ValueError('Use /usr software and a new absolute container SBOM output')
    before = software_tree(root)
    components = dpkg_components(Path('/var/lib/dpkg/status'), Path('/var/lib/dpkg/info'), root, before)
    # Python distribution metadata and RECORDs are also captured from the
    # explicitly copied private venv; no executing Python import is necessary.
    venv = Path('/opt/cowork/python')
    runtime_inventories = []
    if venv.is_dir():
        runtime_inventories.append(sbom.runtime.capture(venv, name='cowork-private-python-venv'))
    if software_tree(root) != before:
        raise ValueError('Container software changed during inventory')
    inventory = {'format': sbom.runtime.FORMAT, 'name': 'Debian container software', 'version': None,
                 'files': before, 'components': components, 'namespace_stable': True, 'executed_payload': False}
    runtime_inventories.insert(0, inventory)
    report = sbom.generate(args.source_root, [], payloads=[Path('/usr/local/bin/cowork-daemon')],
                           include_desktop=False, runtime_inventories=runtime_inventories)
    with args.output.open('x', encoding='utf-8', newline='\n') as stream:
        json.dump(report, stream, sort_keys=True, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    print(json.dumps({'sbom': args.output.name, 'packages': len(report['packages']), 'files': len(report['files'])}))


if __name__ == '__main__':
    main()
