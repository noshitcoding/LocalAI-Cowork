"""Bounded remote volume transport used by the private lifecycle fixture opt-in.

Production lifecycle gates remain closed. Qualification must prove the complete
DB/S3/archive, generation, admission and recovery matrix on actual guest images.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import selectors
import subprocess
import tarfile
import time
import uuid

import execution_generation
import executor_host
from operation_journal import operator_pass_fds
import resource_budget


class StreamError(Exception):
    pass


def check(value, code):
    if not value:
        raise StreamError(code)


def relative_name(value):
    check(isinstance(value, str) and value and value != '.' and len(value.encode('utf-8')) <= 2048 and '\\' not in value,
          'unsafe_volume_archive_entry')
    path = PurePosixPath(value)
    check(not path.is_absolute() and str(path) == value and not any(part in ['.', '..'] for part in path.parts),
          'unsafe_volume_archive_entry')
    return path


def settings(config):
    value = config.get('remote_volume_streaming_preparation')
    check(isinstance(value, dict) and set(value) == {'version', 'max_archive_bytes', 'max_file_bytes', 'max_files', 'timeout_seconds'}
          and type(value['version']) is int and value['version'] == 1, 'remote_volume_streaming_preparation_opt_in_required')
    for key, minimum, maximum in [('max_archive_bytes', 1048576, 8 * 1024**3), ('max_file_bytes', 1, 8 * 1024**3),
                                  ('max_files', 1, 100000), ('timeout_seconds', 10, 300)]:
        check(type(value[key]) is int and minimum <= value[key] <= maximum, 'invalid_volume_streaming_bounds')
    check(value['max_file_bytes'] <= value['max_archive_bytes'], 'invalid_volume_streaming_bounds')
    budget = resource_budget.host_budget(config)
    check(executor_host.manager_binding(config) is not None and budget is not None
          and budget['reserved_cpu_millis'] >= 500 and budget['reserved_memory_mib'] >= 256,
          'volume_streaming_reserved_capacity_required')
    return dict(value)


def verified_archive(path, expected, limits):
    check(isinstance(expected, dict) and set(expected) == {'bytes', 'sha256'} and type(expected['bytes']) is int
          and 0 < expected['bytes'] <= limits['max_archive_bytes'] and isinstance(expected['sha256'], str)
          and re.fullmatch('[a-f0-9]{64}', expected['sha256']), 'invalid_volume_archive_evidence')
    check(path.is_file() and not path.is_symlink() and path.stat().st_nlink == 1
          and path.stat().st_size == expected['bytes'], 'volume_archive_identity_mismatch')
    with path.open('rb') as stream:
        check(hashlib.file_digest(stream, 'sha256').hexdigest() == expected['sha256'], 'volume_archive_identity_mismatch')
    seen, types, total = set(), {}, 0
    try:
        with tarfile.open(path, 'r:') as archive:
            for entry in archive:
                relative_name(entry.name)
                check(entry.name not in seen and len(seen) < limits['max_files'] and (entry.isfile() or entry.isdir())
                      and 0 <= entry.size <= limits['max_file_bytes'], 'unsafe_volume_archive_entry')
                seen.add(entry.name); total += entry.size
                check(total <= limits['max_archive_bytes'], 'volume_archive_exceeds_bound')
                for parent in PurePosixPath(entry.name).parents:
                    if str(parent) != '.':
                        check(not any(prior == str(parent) and not is_directory for prior, is_directory in types.items()),
                              'unsafe_volume_archive_entry')
                types[entry.name] = entry.isdir()
    except (tarfile.TarError, OSError, ValueError):
        raise StreamError('invalid_volume_archive') from None
    return dict(expected)


# Fixed trusted helper source. It sees one named volume and no socket, state,
# keys, network or operator staging mount. Ownership is restored only as the
# sandbox UID; special files, hardlinks, symlinks and path escapes are refused.
HELPER_SOURCE = r'''
import hashlib,json,os,pathlib,stat,sys,tarfile
mode,location,uid,gid,maximum,per_file,count=sys.argv[1:]
root=pathlib.Path(location);uid=int(uid);gid=int(gid);maximum=int(maximum);per_file=int(per_file);count=int(count)
def valid(name):
 p=pathlib.PurePosixPath(name)
 assert name and name!='.' and len(name.encode())<=2048 and '\\' not in name and not p.is_absolute() and str(p)==name and all(x not in ['.','..'] for x in p.parts)
 return p
if mode=='capture':
 paths=sorted(root.rglob('*'));assert len(paths)<=count
 total=0
 with tarfile.open(fileobj=sys.stdout.buffer,mode='w|',format=tarfile.PAX_FORMAT) as out:
  for path in paths:
   name=path.relative_to(root).as_posix();valid(name);info=path.lstat()
   assert stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)
   assert not stat.S_ISREG(info.st_mode) or info.st_nlink==1
   assert info.st_size<=per_file or stat.S_ISDIR(info.st_mode)
   total+=info.st_size if stat.S_ISREG(info.st_mode) else 0;assert total<=maximum
   out.add(path,arcname=name,recursive=False)
elif mode=='restore':
 assert not any(root.iterdir())
 seen=set();total=0
 with tarfile.open(fileobj=sys.stdin.buffer,mode='r|') as source:
  for entry in source:
   relative=valid(entry.name);assert entry.name not in seen and len(seen)<count
   assert entry.isfile() or entry.isdir();assert 0<=entry.size<=per_file
   seen.add(entry.name);total+=entry.size;assert total<=maximum
   target=root.joinpath(*relative.parts)
   assert all(not p.is_symlink() for p in [target,*target.parents])
   target.parent.mkdir(parents=True,exist_ok=True)
   if entry.isdir():target.mkdir(exist_ok=True)
   else:
    with target.open('xb') as output:
     data=source.extractfile(entry);remaining=entry.size
     while remaining:
      block=data.read(min(65536,remaining));assert block;output.write(block);remaining-=len(block)
   os.chmod(target,entry.mode & 0o777);os.chown(target,uid,gid)
 for path in sorted(root.rglob('*'),reverse=True):
  if path.is_dir():os.chown(path,uid,gid)
 os.chown(root,uid,gid)
 sys.stdout.write(json.dumps({'restored_entries':len(seen)}))
else:raise ValueError('unsupported mode')
'''


class VolumeStreamer:
    def __init__(self, infrastructure, operation):
        if operation.get('action') in ['import_restore', 'import_qualify']:
            import instance_imports
            instance_imports.binding(operation)
        self.infra, self.operation = infrastructure, operation
        self.limits = settings(infrastructure.config)
        self.host_binding = executor_host.operation_binding(operation)
        check(self.host_binding == executor_host.manager_binding(infrastructure.config)
              and operation['action'] in ['backup', 'restore', 'upgrade', 'rollback','enroll_execution','import_restore','import_qualify'], 'volume_streaming_operation_scope_mismatch')
        self.execution = execution_generation.binding(operation)
        check(self.execution is not None, 'volume_streaming_execution_binding_required')
        self.instance = operation['instance']
        self.directory = infrastructure.root / uuid.UUID(self.instance['id']).hex
        self.image = infrastructure.config['images']['sandbox_core']
        check(isinstance(self.image, str) and re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[a-f0-9]{64}', self.image),
              'volume_streaming_pinned_helper_required')
        self.backup_root = Path(infrastructure.config['backup_directory'])
        check(self.backup_root.is_absolute() and not self.backup_root.resolve().is_relative_to(infrastructure.root)
              and not infrastructure.root.is_relative_to(self.backup_root.resolve()), 'volume_streaming_private_staging_required')
        check(self.directory.is_dir() and not self.directory.is_symlink(), 'volume_streaming_instance_state_required')
        self.infra.host.bind(); self.infra.host.bind(self.directory); self.infra.host.verify()

    def private_path(self, path, *, existing):
        path = Path(path)
        check(path.is_absolute() and path.resolve().is_relative_to(self.backup_root.resolve())
              and path.parent.is_dir() and all(not parent.is_symlink() for parent in [path, *path.parents])
              and re.fullmatch(r'[a-f0-9]{32}\.tar', path.name), 'unsafe_volume_streaming_staging_path')
        if os.name == 'posix':
            check(path.parent.stat().st_uid == os.geteuid() and not path.parent.stat().st_mode & 0o077,
                  'volume_streaming_private_staging_required')
        check(path.exists() == existing, 'volume_streaming_staging_exists_mismatch')
        return path

    def scope(self, directory, suffix, *, restore):
        check(isinstance(suffix, str) and re.fullmatch('[a-f0-9]{32}', suffix), 'invalid_workspace_storage_suffix')
        check(directory.is_dir() and not directory.is_symlink(), 'unsafe_volume_streaming_generation')
        if restore:
            check(self.operation['action'] in ['restore', 'upgrade', 'rollback','import_restore']
                  and directory == self.directory / 'generations' / uuid.UUID(self.operation['id']).hex,
                  'volume_streaming_candidate_generation_required')
            generation = execution_generation.document(directory / 'generation.json')
            check(generation.get('generation') == self.operation['id'], 'volume_streaming_candidate_generation_required')
            epoch = self.execution['candidate_epoch']
        else:
            check(self.operation['action'] != 'import_qualify', 'volume_streaming_import_target_scope_required')
            check(directory == self.infra.active_directory(self.directory), 'volume_streaming_source_generation_changed')
            epoch = self.execution['source_epoch']
        check(execution_generation.read(self.instance, directory) == epoch, 'volume_streaming_epoch_changed')
        scope = self.infra.resource_names(self.instance, directory)
        return scope['volume_prefix'] + suffix

    def quiescent(self):
        prefix = 'cowork-' + uuid.UUID(self.instance['id']).hex + '-'
        check(not self.infra.run(['docker', 'ps', '--filter', 'name=^/' + prefix, '--quiet']).strip(),
              'volume_streaming_requires_quiescent_instance')
        if self.infra.config.get('remote_lifecycle_qualification') is not None:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.quiescent(self.infra, self.instance)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise StreamError(str(error)) from None

    def inspect_volume(self, name):
        values = json.loads(self.infra.run(['docker', 'volume', 'inspect', name], timeout=10))
        check(isinstance(values, list) and len(values) == 1 and values[0].get('Name') == name
              and values[0].get('Driver') == 'local' and values[0].get('Scope') == 'local'
              and not values[0].get('Options'), 'volume_streaming_local_volume_required')
        return values[0]

    def transfer(self, mode, volume, archive):
        label = {'cowork.volume-stream.operation': self.operation['id'], 'cowork.volume-stream.instance': self.instance['id']}
        name = 'cowork-' + uuid.UUID(self.instance['id']).hex + '-g-' + uuid.UUID(self.operation['id']).hex + '-snapshot-' + uuid.uuid4().hex
        args = ['docker', 'create', '--name', name, '--network', 'none', '--read-only', '--user', '0:0',
                '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--pids-limit', '64',
                '--cpus', '0.5', '--memory', '256m', '--memory-swap', '256m']
        for key, value in label.items(): args += ['--label', key + '=' + value]
        for capability in ['DAC_OVERRIDE', *(['CHOWN', 'FOWNER'] if mode == 'restore' else [])]:
            args += ['--cap-add', capability]
        if mode == 'restore': args += ['--interactive']
        args += ['--volume', volume + ':/workspace:' + ('rw' if mode == 'restore' else 'ro'),
                 '--entrypoint', 'python3', self.image, '-c', HELPER_SOURCE, mode, '/workspace', '10001', '10001',
                 str(self.limits['max_archive_bytes']), str(self.limits['max_file_bytes']), str(self.limits['max_files'])]
        helper = None
        try:
            helper = self.infra.run(args, timeout=30).decode().strip()
            check(re.fullmatch('[a-f0-9]{64}', helper), 'volume_streaming_helper_identity_unproven')
            self.verify_helper(helper, name, label, volume, mode)
            cli = self.infra.host.arguments(['docker', 'start', '--attach', *(['--interactive'] if mode == 'restore' else []), helper])
            process = subprocess.Popen(cli, stdin=subprocess.PIPE if mode == 'restore' else subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=self.infra.host.environment(None),
                **({'pass_fds': operator_pass_fds()} if os.name == 'posix' else {}))
            try:
                result = pump(process, archive, mode, self.limits)
            finally:
                if process.poll() is None:
                    process.kill(); process.wait(timeout=10)
                for pipe in [process.stdin, process.stdout]:
                    if pipe is not None: pipe.close()
            return result
        except (OSError, subprocess.SubprocessError, ValueError, executor_host.HostError):
            raise StreamError('remote_volume_streaming_failed') from None
        finally:
            if helper is not None:
                self.verify_helper(helper, name, label, volume, mode)
                self.infra.run(['docker', 'rm', '--force', helper], timeout=30)

    def verify_helper(self, helper, name, label, volume, mode):
        check(isinstance(helper, str) and re.fullmatch('[a-f0-9]{64}', helper), 'volume_streaming_helper_identity_unproven')
        value = json.loads(self.infra.run(['docker', 'inspect', helper], timeout=10))[0]
        check(value.get('Id') == helper and value.get('Name') == '/' + name
              and all((value.get('Config', {}).get('Labels') or {}).get(key) == expected for key, expected in label.items())
              and value.get('Config', {}).get('Image') == self.image, 'volume_streaming_helper_scope_changed')
        caps = value.get('HostConfig') or {}
        check(caps.get('NanoCpus') == 500000000 and caps.get('Memory') == 256 * 1024 * 1024
              and caps.get('MemorySwap') == caps['Memory'] and caps.get('PidsLimit') == 64
              and caps.get('NetworkMode') == 'none' and caps.get('ReadonlyRootfs') is True and not caps.get('Privileged')
              and {key.upper() for key in caps.get('CapDrop') or []} == {'ALL'}
              and {key.upper() for key in caps.get('CapAdd') or []} == {'DAC_OVERRIDE', *(['CHOWN', 'FOWNER'] if mode == 'restore' else [])}
              and 'no-new-privileges:true' in (caps.get('SecurityOpt') or []), 'volume_streaming_helper_limits_changed')
        mounts = value.get('Mounts') or []
        check(len(mounts) == 1 and mounts[0].get('Type') == 'volume' and mounts[0].get('Name') == volume
              and mounts[0].get('Destination') == '/workspace' and mounts[0].get('RW') is (mode == 'restore'),
              'volume_streaming_helper_mount_changed')

    def capture(self, directory, suffix, destination):
        destination = self.private_path(destination, existing=False)
        volume = self.scope(directory, suffix, restore=False)
        self.quiescent(); self.inspect_volume(volume)  # A missing source never creates an empty volume.
        descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(descriptor, 'wb') as output:
                evidence = self.transfer('capture', volume, output); output.flush(); os.fsync(output.fileno())
            verified_archive(destination, evidence, self.limits)
            return evidence
        except BaseException:
            destination.unlink(missing_ok=True)  # Only this newly created private file.
            raise

    def capture_import_target(self, directory, suffix, destination):
        """Read only the bound imported candidate; never widen ordinary source capture."""
        import instance_imports
        value = instance_imports.binding(self.operation)
        check(self.operation['action'] in ['import_restore', 'import_qualify']
              and isinstance(suffix, str) and re.fullmatch('[a-f0-9]{32}', suffix)
              and directory.parent == self.directory / 'generations' and directory.is_dir()
              and not directory.is_symlink(), 'volume_streaming_import_target_scope_mismatch')
        marker = execution_generation.document(directory / 'generation.json')
        restored = execution_generation.document(directory / 'restore.json')
        check(uuid.UUID(marker['generation']).hex == directory.name
              and execution_generation.read(self.instance, directory) == value['candidate_epoch']
              and restored.get('import_binding_sha256') == instance_imports.canonical(value)
              and restored.get('restore_operation_id') == marker['generation'], 'volume_streaming_import_target_scope_mismatch')
        if self.operation['action'] == 'import_restore':
            check(marker['generation'] == self.operation['id'], 'volume_streaming_import_target_scope_mismatch')
        else:
            check(directory == self.infra.active_directory(self.directory), 'volume_streaming_import_target_scope_mismatch')
        destination = self.private_path(destination, existing=False)
        volume = self.infra.resource_names(self.instance, directory)['volume_prefix'] + suffix
        self.quiescent()
        actual = self.inspect_volume(volume)
        labels = actual.get('Labels') or {}
        check(labels.get('cowork.volume-stream.instance') == self.instance['id']
              and labels.get('cowork.volume-stream.operation') == restored['restore_operation_id'],
              'volume_streaming_import_target_owner_mismatch')
        descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(descriptor, 'wb') as output:
                evidence = self.transfer('capture', volume, output)
                output.flush(); os.fsync(output.fileno())
            verified_archive(destination, evidence, self.limits)
            return evidence
        except BaseException:
            destination.unlink(missing_ok=True)
            raise

    def restore(self, directory, suffix, archive, expected):
        archive = self.private_path(archive, existing=True)
        verified_archive(archive, expected, self.limits)  # Before Docker create or any volume write.
        volume = self.scope(directory, suffix, restore=True)
        self.quiescent()
        names = self.infra.run(['docker', 'volume', 'ls', '--format', '{{.Name}}']).decode().splitlines()
        check(volume not in names, 'volume_streaming_target_already_exists')
        labels = {'cowork.volume-stream.operation': self.operation['id'], 'cowork.volume-stream.instance': self.instance['id']}
        args = ['docker', 'volume', 'create', '--driver', 'local']
        for key, value in labels.items(): args += ['--label', key + '=' + value]
        check(self.infra.run([*args, volume]).decode().strip() == volume, 'volume_streaming_target_identity_unproven')
        actual = self.inspect_volume(volume)
        check(all((actual.get('Labels') or {}).get(key) == value for key, value in labels.items()), 'volume_streaming_target_scope_changed')
        with archive.open('rb') as source:
            result = self.transfer('restore', volume, source)
        check(result.pop('stream_bytes') == expected['bytes'] and result.pop('stream_sha256') == expected['sha256'],
              'volume_archive_changed_during_stream')
        # Failed or interrupted target volumes are retained for explicit recovery;
        # this method never replaces an active pointer or deletes source volumes.
        return {'volume': volume, 'archive': dict(expected), **result}


def pump(process, archive, mode, limits):
    check(os.name == 'posix', 'volume_streaming_requires_posix')
    check(mode in ['capture', 'restore'], 'invalid_volume_streaming_mode')
    deadline = time.monotonic() + limits['timeout_seconds']
    total, checksum, reply, pending = 0, hashlib.sha256(), bytearray(), b''
    with selectors.DefaultSelector() as selector:
        os.set_blocking(process.stdout.fileno(), False)
        selector.register(process.stdout, selectors.EVENT_READ, 'output')
        if mode == 'restore':
            os.set_blocking(process.stdin.fileno(), False); selector.register(process.stdin, selectors.EVENT_WRITE, 'input')
        while selector.get_map():
            check(time.monotonic() < deadline, 'volume_streaming_timeout')
            for key, _ in selector.select(min(1, max(0, deadline - time.monotonic()))):
                if key.data == 'output':
                    block = os.read(key.fileobj.fileno(), 65536)
                    if not block:
                        selector.unregister(key.fileobj); continue
                    if mode == 'capture':
                        total += len(block); check(total <= limits['max_archive_bytes'], 'volume_archive_exceeds_bound')
                        archive.write(block); checksum.update(block)
                    else:
                        reply.extend(block); check(len(reply) <= 4096, 'volume_streaming_reply_exceeds_bound')
                else:
                    if not pending: pending = archive.read(65536)
                    if not pending:
                        selector.unregister(key.fileobj); key.fileobj.close(); continue
                    try:
                        written = os.write(key.fileobj.fileno(), pending)
                        checksum.update(pending[:written]); pending = pending[written:]
                        total += written; check(total <= limits['max_archive_bytes'], 'volume_archive_exceeds_bound')
                    except BlockingIOError:
                        pass
        check(process.wait(timeout=max(.1, deadline - time.monotonic())) == 0, 'volume_streaming_helper_failed')
    if mode == 'capture':
        return {'bytes': total, 'sha256': checksum.hexdigest()}
    try:
        result = json.loads(reply)
        check(isinstance(result, dict) and set(result) == {'restored_entries'} and type(result['restored_entries']) is int
              and 0 <= result['restored_entries'] <= limits['max_files'], 'invalid_volume_streaming_reply')
        return {**result, 'stream_bytes': total, 'stream_sha256': checksum.hexdigest()}
    except ValueError:
        raise StreamError('invalid_volume_streaming_reply') from None
