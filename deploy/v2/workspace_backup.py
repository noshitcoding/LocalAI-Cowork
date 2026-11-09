"""Bounded team-workspace snapshots and exact, journal-bound Docker quiescence.

Never adopts a foreign marker, removes a container, edits a run receipt, or
purges an external directory. Run on the selected Linux Docker host, or inside
an explicitly node-bound Kubernetes maintenance pod.
"""
from __future__ import annotations
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import tarfile
import uuid

FORMAT = 'localai-cowork-team-workspace/1'
OWNER = '.cowork-workspace-owner.json'
MAX_ENTRIES = 100000
MAX_BYTES = 16 * 1024 ** 3
MAX_FILE = 2 * 1024 ** 3
MAX_MANIFEST = 32 * 1024 ** 2


def require(value, message):
    if not value:
        raise ValueError(message)


def canonical_uuid(value):
    require(isinstance(value, str) and str(uuid.UUID(value)) == value, 'Invalid retained UUID')
    return value


def metadata(path):
    item = path.lstat()
    require(not stat.S_ISLNK(item.st_mode) and not getattr(item, 'st_file_attributes', 0) & 0x400,
            'Workspace namespace contains an alias')
    return item


def directory(path):
    require(path.is_absolute() and '..' not in path.parts, 'Select an absolute canonical workspace directory')
    for parent in [*reversed(path.parents), path]:
        require(stat.S_ISDIR(metadata(parent).st_mode), 'Workspace namespace is not a directory')
    require(path.resolve(strict=True) == path, 'Workspace namespace is not canonical')
    return metadata(path)


def identity(item):
    # Windows stat/fstat expose different change-time semantics. The stable
    # creation time plus volume/file identity and write time are comparable.
    changed = getattr(item,'st_birthtime_ns',item.st_ctime_ns) if os.name=='nt' else item.st_ctime_ns
    return item.st_dev, item.st_ino, item.st_mode, item.st_size, item.st_mtime_ns, changed


def open_beneath(root, relative, flags):
    """Linux openat chain: no pathname component can redirect to an alias."""
    if os.name!='posix':
        directory((root/relative).parent)
        return os.open(root/relative,flags|getattr(os,'O_BINARY',0))
    descriptor=os.open('/',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try:
        for component in [*root.parts[1:],*PurePosixPath(relative).parts[:-1]]:
            next_descriptor=os.open(component,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=descriptor)
            os.close(descriptor);descriptor=next_descriptor
        return os.open(PurePosixPath(relative).name,flags|os.O_NOFOLLOW,dir_fd=descriptor)
    finally:
        os.close(descriptor)


def read_regular(path, limit):
    before = metadata(path)
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_size <= limit,
            'Expected a bounded unaliased regular file')
    descriptor = open_beneath(path.parent,path.name,os.O_RDONLY)
    try:
        require(identity(os.fstat(descriptor)) == identity(before), 'File identity changed before reading')
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            raw = stream.read(limit + 1)
        require(len(raw) <= limit and identity(os.fstat(descriptor)) == identity(before)
                and identity(metadata(path)) == identity(before), 'File changed during reading')
        return raw
    finally:
        os.close(descriptor)


def write_new(path, raw):
    with path.open('xb') as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    if os.name == 'posix':
        os.chmod(path, 0o600)


def sync_directory(path):
    if os.name == 'posix':
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def owned(root, deployment, claim=False):
    info = directory(root)
    require(re.fullmatch(r'[0-9a-f]{32}', deployment), 'Invalid deployment ownership')
    if os.name == 'posix':
        require(info.st_uid == 1000 and info.st_mode & 0o077 == 0,
                'Team workspace root must be private and owned by UID1000')
    path = root / OWNER
    if claim and not path.exists():
        value = {'format': FORMAT + '/owner', 'deployment_id': deployment,
                 'host_root': str(root), 'workspace_id': str(uuid.uuid4())}
        try:
            write_new(path, json.dumps(value, sort_keys=True).encode())
            sync_directory(root)
        except FileExistsError:
            pass
    value = json.loads(read_regular(path, 16384))
    require(value.get('format') == FORMAT + '/owner' and value.get('deployment_id') == deployment
            and value.get('host_root') == str(root), 'Workspace belongs to another deployment or host pathname')
    canonical_uuid(value['workspace_id'])
    return value


def docker(*arguments):
    child = subprocess.run(['docker', '--host=unix:///var/run/docker.sock', *arguments], stdin=subprocess.DEVNULL, capture_output=True,
                           timeout=90, check=False, env={key:value for key,value in os.environ.items() if not key.startswith('DOCKER_')})
    require(child.returncode == 0 and len(child.stdout) <= 16 * 1024 ** 2,
            'Selected Docker host did not confirm the operation')
    return child.stdout


def inspections():
    identifiers = docker('ps', '-a', '-q', '--no-trunc').decode().splitlines()
    require(len(identifiers) <= 4096 and all(re.fullmatch(r'[0-9a-f]{64}', item) for item in identifiers),
            'Docker inventory exceeds the bounded exact-ID contract')
    result = []
    template = ('{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Config.Image}},'
                '"labels":{{json .Config.Labels}},"mounts":{{json .Mounts}},"state":{{json .State}}}')
    for identifier in identifiers:
        value = json.loads(docker('inspect', '--format', template, identifier))
        require(value['id'] == identifier, 'Docker immutable identity changed')
        result.append(value)
    return result


def records(data):
    root = data / 'team-containers'
    if not root.exists():
        return {}
    directory(root)
    result = {}
    for path in sorted(root.iterdir()):
        if not path.name.startswith('ct-'):
            continue
        require(len(result) < 2048, 'Retained container record budget exceeded')
        item = json.loads(read_regular(path, 1024 * 1024))
        for field in ['nonce', 'team', 'order', 'run']:
            canonical_uuid(item[field])
        require(item['format'] in {1,2} and path.name == 'ct-' + item['nonce'] + '.json'
                and item['name'] == 'cowork-team-' + item['nonce'], 'Invalid retained container ownership')
        require((item['format']==2)==bool(item.get('host')),'Container record has ambiguous host binding')
        if item.get('host'):validate_host(item['host'])
        if item.get('container_id') is not None:
            require(re.fullmatch(r'[0-9a-f]{64}', item['container_id']), 'Invalid retained full container ID')
        require(item['nonce'] not in result, 'Duplicate retained container ownership')
        result[item['nonce']] = item
    return result


def validate_host(item):
    require(set(item)=={'format','host_id','engine_id','node_name','node_uid'} and item['format']==1,'Unknown Dockerhost binding')
    canonical_uuid(item['host_id'])
    require(item['host_id']!=str(uuid.UUID(int=0)) and re.fullmatch(r'[A-Za-z0-9_.:-]{1,128}',item['engine_id']),'Invalid Dockerhost identity')
    require((item['node_name'] is None)==(item['node_uid'] is None),'Ambiguous Docker node binding')
    if item['node_name'] is not None:
        require(re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',item['node_name']),'Invalid Docker node name')
        canonical_uuid(item['node_uid'])
        require(item['node_uid']!=str(uuid.UUID(int=0)),'Invalid immutable Docker node UID')
    return item


def host_inventory(data):
    root=data/'team-containers'/'hosts'
    result={}
    if root.exists():
        directory(root)
        for path in sorted(root.iterdir()):
            require(len(result)<64 and re.fullmatch(r'host-[0-9a-f-]{36}\.json',path.name),'Unknown retained Dockerhost inventory file')
            item=validate_host(json.loads(read_regular(path,16384)))
            require(path.name=='host-'+item['host_id']+'.json' and item['host_id'] not in result,'Dockerhost inventory alias')
            result[item['host_id']]=item
    retained=records(data)
    needed=set()
    unbound=0
    for record in retained.values():
        if record.get('host') is None:unbound+=1;continue
        host=record['host'];require(result.get(host['host_id'])==host,'Retained guest has no exact durable host inventory')
        needed.add(host['host_id'])
    return {'hosts':list(result.values()),'needed_host_ids':sorted(needed),'unbound_records':unbound}


def overlaps(left, right):
    if not left or not right or not PurePosixPath(left).is_absolute() or not PurePosixPath(right).is_absolute():
        return False
    a, b = PurePosixPath(left), PurePosixPath(right)
    return a == b or a in b.parents or b in a.parents


def match_container(item, record, host_root):
    labels = item.get('labels') or {}
    expected = {'cowork.team-tool.' + key: record[key] for key in ['nonce', 'team', 'order', 'run', 'image']}
    if record.get('github'):
        expected.update({'cowork.team-tool.github-revision': str(record['github'][0]),
                         'cowork.team-tool.github-host': record['github'][1]})
    if record.get('host'):
        host=record['host'];expected.update({'cowork.team-tool.host-id':host['host_id'],'cowork.team-tool.engine-id':host['engine_id']})
        if host['node_name'] is not None:expected.update({'cowork.team-tool.node-name':host['node_name'],'cowork.team-tool.node-uid':host['node_uid']})
    require(item['name'] == '/' + record['name'] and item['image'] == record['image']
            and (record.get('container_id') is None or record['container_id'] == item['id'])
            and all(labels.get(key) == value for key, value in expected.items()),
            'Container ownership does not match its retained record')
    expected_source = str(PurePosixPath(str(host_root)) / record['order'])
    bound = [mount for mount in item['mounts'] if overlaps(mount.get('Source', ''), str(host_root))]
    require(len(bound) == 1 and bound[0].get('Type') == 'bind'
            and bound[0].get('Source') == expected_source and bound[0].get('Destination') == '/workspace',
            'Container workspace mount does not match its exact owned order')


def quiesce(root, data, deployment, host_root=None, claim=False, host_id=None, node_name=None, node_uid=None):
    owner = owned(root, deployment, claim)
    directory(data)
    bound_root = host_root or root
    retained = records(data)
    inventory=host_inventory(data)
    require(inventory['unbound_records']==0,'Legacy guest records lack a durable Dockerhost binding; retain them for explicit recovery')
    require(host_id is not None,'Select the persistent operator Dockerhost UUID')
    canonical_uuid(host_id)
    engine=json.loads(docker('info','--format','{{json .ID}}'))
    local=validate_host({'format':1,'host_id':host_id,'engine_id':engine,'node_name':node_name,'node_uid':node_uid})
    known={item['host_id']:item for item in inventory['hosts']}
    require(host_id not in known or known[host_id]==local,'The selected Docker engine or immutable node identity changed')
    if node_name is None:
        require(all(identifier==host_id for identifier in inventory['needed_host_ids']),'Another Dockerhost still owns retained guests; local quiescence cannot prove its exit')
    stopped = []
    for item in inspections():
        if not any(overlaps(mount.get('Source', ''), str(bound_root)) and mount.get('RW')
                   for mount in item['mounts']):
            continue
        state = item['state']
        if state.get('Running') is False and state.get('Pid') == 0 and not state.get('Restarting') and not state.get('Paused'):
            continue
        nonce = (item.get('labels') or {}).get('cowork.team-tool.nonce')
        require(nonce in retained, 'An unowned or unrecorded container can write the workspace; retaining all data')
        require(retained[nonce].get('host')==local,'Guest belongs to another Dockerhost; refusing local control')
        match_container(item, retained[nonce], bound_root)
        if item['state'].get('Running') or item['state'].get('Pid', 0):
            docker('stop', '--time', '45', item['id'])
            stopped.append({'id': item['id'], 'nonce': nonce, 'run': retained[nonce]['run']})
    # A failed stop, paused/restarting guest, or newly appearing writer is not a witness.
    for item in inspections():
        if any(overlaps(mount.get('Source', ''), str(bound_root)) and mount.get('RW') for mount in item['mounts']):
            state = item['state']
            if state.get('Running') is False and state.get('Pid') == 0 and not state.get('Restarting') and not state.get('Paused'):
                continue
            nonce = (item.get('labels') or {}).get('cowork.team-tool.nonce')
            require(nonce in retained, 'Workspace writer appeared during quiescence')
            require(retained[nonce].get('host')==local,'Workspace writer belongs to another Dockerhost')
            match_container(item, retained[nonce], bound_root)
            require(state.get('Running') is False and state.get('Pid') == 0
                    and not state.get('Restarting') and not state.get('Paused'), 'Owned guest has not positively stopped')
    return {'format': FORMAT + '/quiescence', 'workspace_id': owner['workspace_id'], 'stopped_guests': stopped,
            'retained_records': len(retained), 'records_replayed': False,'host':local,
            'needed_host_ids':inventory['needed_host_ids']}


def safe_name(name):
    require(isinstance(name, str) and 0 < len(name) <= 4096 and '\\' not in name and ':' not in name
            and not any(ord(char) < 32 for char in name), 'Unsafe workspace archive name')
    path = PurePosixPath(name)
    require(not path.is_absolute() and '..' not in path.parts and '.' not in name.split('/')
            and str(path) == name and path.parts[0] != OWNER, 'Noncanonical workspace archive name')
    return name


def inventory(root):
    directory(root)
    result, directories, total = [], {}, 0
    def visit(base):
        nonlocal total
        require(len(base.relative_to(root).parts)<=64,'Workspace nesting budget exceeded')
        before = metadata(base)
        directories[str(base)] = identity(before)
        for path in sorted(base.iterdir()):
            if path == root / OWNER:
                continue
            info = metadata(path)
            name = safe_name(path.relative_to(root).as_posix())
            require(len(result) < MAX_ENTRIES, 'Workspace entry budget exceeded')
            if stat.S_ISDIR(info.st_mode):
                result.append({'path': name, 'kind': 'directory', 'bytes': 0, 'mode': info.st_mode & 0o777})
                visit(path)
            else:
                require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= MAX_FILE,
                        'Workspace snapshot refuses links, special files and oversized files')
                total += info.st_size
                require(total <= MAX_BYTES, 'Workspace byte budget exceeded')
                result.append({'path': name, 'kind': 'file', 'bytes': info.st_size, 'mode': info.st_mode & 0o777, '_identity': identity(info)})
        require(identity(metadata(base)) == identity(before), 'Workspace directory changed during enumeration')
    visit(root)
    return result, directories


def archive_workspace(root, output, deployment, receipt):
    owner = owned(root, deployment)
    entries, directories = inventory(root)
    require(output.is_absolute() and not output.exists() and not output.is_relative_to(root),
            'Workspace archive must be a new file outside the source')
    with tarfile.open(output, 'x') as archive:
        for entry in entries:
            path = root / entry['path']
            member = tarfile.TarInfo('content/' + entry['path'])
            member.mode = entry['mode']
            member.uid = member.gid = 1000
            member.size = entry['bytes']
            if entry['kind'] == 'directory':
                member.type = tarfile.DIRTYPE
                archive.addfile(member)
                continue
            expected = tuple(entry.pop('_identity'))
            require(identity(metadata(path)) == expected, 'Workspace file changed before archive read')
            descriptor = open_beneath(root,entry['path'],os.O_RDONLY)
            try:
                require(identity(os.fstat(descriptor)) == expected, 'Workspace file identity changed')
                digest = hashlib.sha256()
                with os.fdopen(descriptor, 'rb', closefd=False) as stream:
                    class Reader:
                        def read(self, size):
                            raw = stream.read(size)
                            digest.update(raw)
                            return raw
                    archive.addfile(member, Reader())
                require(identity(os.fstat(descriptor)) == expected and identity(metadata(path)) == expected,
                        'Workspace file changed during snapshot')
                entry['sha256'] = digest.hexdigest()
            finally:
                os.close(descriptor)
        for name, expected in directories.items():
            require(identity(metadata(Path(name))) == expected, 'Workspace namespace changed during snapshot')
        require(owned(root, deployment) == owner, 'Workspace ownership changed during snapshot')
        manifest = {'format': FORMAT + '/archive', 'deployment_id': deployment,
                    'workspace_id': owner['workspace_id'], 'quiescence': receipt, 'entries': entries}
        raw = json.dumps(manifest, sort_keys=True).encode()
        require(len(raw) <= MAX_MANIFEST, 'Workspace manifest budget exceeded')
        member = tarfile.TarInfo('manifest.json'); member.mode = 0o600; member.size = len(raw)
        archive.addfile(member, io.BytesIO(raw))
    with output.open('r+b') as stream:
        os.fsync(stream.fileno())
    return manifest


def validate_archive(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= MAX_BYTES + MAX_MANIFEST + MAX_ENTRIES * 2048,
            'Workspace archive budget exceeded')
    with tarfile.open(path, 'r:') as archive:
        members, total = {}, 0
        for member in archive:
            require(len(members) < MAX_ENTRIES + 1 and member.name.rstrip('/') not in members and not member.issym()
                    and not member.islnk() and (member.isfile() or member.isdir()), 'Unsafe or duplicate workspace member')
            if member.name != 'manifest.json':
                require(member.name.startswith('content/'), 'Unknown workspace archive member')
                safe_name(member.name[len('content/'):].rstrip('/') if member.isdir() else member.name[len('content/'):])
            total += member.size
            require(member.size >= 0 and total <= MAX_BYTES + MAX_MANIFEST, 'Workspace archive byte budget exceeded')
            members[member.name.rstrip('/')] = member
        member = members.get('manifest.json')
        require(member is not None and member.isfile() and member.size <= MAX_MANIFEST, 'Missing bounded workspace manifest')
        manifest = json.load(archive.extractfile(member))
        require(manifest.get('format') == FORMAT + '/archive' and len(manifest['entries']) <= MAX_ENTRIES,
                'Unknown workspace archive format')
        expected = {'manifest.json'}
        types = {}
        for entry in manifest['entries']:
            name = safe_name(entry['path']); member_name = 'content/' + name
            require(member_name not in expected and entry['kind'] in {'file', 'directory'}, 'Duplicate manifest entry')
            require(isinstance(entry.get('mode'), int) and 0 <= entry['mode'] <= 0o777, 'Invalid workspace mode')
            expected.add(member_name); types[name] = entry['kind']
            member = members.get(member_name)
            require(member is not None and member.size == entry['bytes']
                    and (member.isdir() if entry['kind'] == 'directory' else member.isfile()), 'Workspace manifest/member mismatch')
            if entry['kind'] == 'file':
                require(member.size <= MAX_FILE and re.fullmatch(r'[0-9a-f]{64}', entry['sha256']), 'Invalid workspace file hash')
                digest = hashlib.sha256()
                with archive.extractfile(member) as stream:
                    while chunk := stream.read(1024 * 1024):
                        digest.update(chunk)
                require(digest.hexdigest() == entry['sha256'], 'Workspace archive content changed')
        require(set(members) == expected, 'Workspace archive has undeclared members')
        for name in types:
            require(all(types.get(str(parent)) == 'directory' for parent in PurePosixPath(name).parents
                        if str(parent) != '.'), 'Workspace archive has a missing or non-directory parent')
    return manifest


def restore_workspace(root, archive_path, deployment, source_deployment):
    owner = owned(root, deployment)
    manifest = validate_archive(archive_path)
    require(manifest['deployment_id'] == source_deployment, 'Workspace archive belongs to another source deployment')
    current, _ = inventory(root)  # Refuse all existing aliases before moving any file.
    retained = '.cowork-retained-before-restore-' + uuid.uuid4().hex
    require(retained not in {PurePosixPath(entry['path']).parts[0] for entry in manifest['entries']},
            'Retention namespace collides with archive')
    retained_path = root / retained
    retained_path.mkdir(mode=0o700)
    for path in sorted(root.iterdir()):
        if path.name not in {OWNER, retained}:
            path.rename(retained_path / path.name)
    sync_directory(retained_path); sync_directory(root)
    with tarfile.open(archive_path, 'r:') as archive:
        for entry in sorted(manifest['entries'], key=lambda item: (len(PurePosixPath(item['path']).parts), item['path'])):
            target = root / entry['path']
            directory(target.parent)
            if entry['kind'] == 'directory':
                target.mkdir(mode=0o700)
            else:
                member = archive.getmember('content/' + entry['path'])
                descriptor=open_beneath(root,entry['path'],os.O_WRONLY|os.O_CREAT|os.O_EXCL)
                with archive.extractfile(member) as source, os.fdopen(descriptor,'wb') as output:
                    while chunk := source.read(1024 * 1024):
                        output.write(chunk)
                    output.flush(); os.fsync(output.fileno())
                    if os.name == 'posix':os.fchmod(output.fileno(),entry['mode'])
            sync_directory(target.parent)
    if os.name == 'posix':
        for entry in sorted((entry for entry in manifest['entries'] if entry['kind']=='directory'),
                            key=lambda item:len(PurePosixPath(item['path']).parts),reverse=True):
            os.chmod(root/entry['path'],entry['mode'])
    require(owned(root, deployment) == owner, 'Target workspace ownership changed during restore')
    sync_directory(root)
    return {'state': 'restored', 'workspace_id': owner['workspace_id'], 'retained_directory': retained,
            'retained_entries': len(current), 'runs_replayed': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['claim', 'quiesce', 'archive', 'restore','inventory-hosts'])
    parser.add_argument('--root', type=Path)
    parser.add_argument('--deployment-id', required=True)
    parser.add_argument('--data', type=Path)
    parser.add_argument('--host-root', type=Path)
    parser.add_argument('--archive', type=Path)
    parser.add_argument('--source-deployment-id')
    parser.add_argument('--host-id')
    parser.add_argument('--node-name')
    parser.add_argument('--node-uid')
    args = parser.parse_args()
    if args.action=='inventory-hosts':
        result=host_inventory(args.data)
    elif args.action == 'claim':
        result = owned(args.root, args.deployment_id, True)
    elif args.action == 'quiesce':
        result = quiesce(args.root, args.data, args.deployment_id, args.host_root,host_id=args.host_id,node_name=args.node_name,node_uid=args.node_uid)
    elif args.action == 'archive':
        receipt = quiesce(args.root, args.data, args.deployment_id, args.host_root,host_id=args.host_id,node_name=args.node_name,node_uid=args.node_uid)
        result = archive_workspace(args.root, args.archive, args.deployment_id, receipt)
        quiesce(args.root, args.data, args.deployment_id, args.host_root,host_id=args.host_id,node_name=args.node_name,node_uid=args.node_uid)
        result = {'format':result['format'],'deployment_id':result['deployment_id'],'workspace_id':result['workspace_id'],
                  'entries':len(result['entries']),'bytes':sum(entry['bytes'] for entry in result['entries'])}
    else:
        quiesce(args.root, args.data, args.deployment_id, args.host_root,host_id=args.host_id,node_name=args.node_name,node_uid=args.node_uid)
        result = restore_workspace(args.root, args.archive, args.deployment_id, args.source_deployment_id)
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        raise SystemExit('Team workspace operation refused: ' + str(error))
