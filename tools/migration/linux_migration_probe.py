"""One bounded native Linux migration probe, exclusively on its own /tmp fixtures.

No installed database, credential, writer, sysctl, service or mount is touched.
Run with system python3 -B; stdout is the nonsecret narrow evidence receipt.
"""
from __future__ import annotations
import contextlib
from datetime import datetime, timedelta, timezone
import hashlib
import json
import mmap
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch
import v1_sqlite_export as v1
import v1_quarantine as quarantine
import v2_snapshot_profile as target_profile
from migration_io import FileFence
from test_v1_sqlite_export import desktop, insert_required


def require(value, label):
    if not value:
        raise AssertionError(label)


def refused(action):
    try:
        action()
    except (v1.Refused, OSError):
        return
    raise AssertionError('Expected native boundary refusal')


def main():
    require(sys.platform == 'linux', 'Native Linux required')
    started = time.monotonic()
    checks = []
    initial_mask = signal.pthread_sigmask(signal.SIG_BLOCK, set())
    initial_handler = signal.getsignal(signal.SIGIO)
    with tempfile.TemporaryDirectory(prefix='cowork-linux-m9-', dir='/tmp') as directory:
        root = Path(directory)
        require(root.parent == Path('/tmp') and not root.is_symlink(), 'Exact owned fixture root')
        sources = root/'sources'; sources.mkdir()
        outputs = root/'outputs'; outputs.mkdir()
        approvals = root/'approvals'; approvals.mkdir()
        source = sources/'source.sqlite3'
        origin = root/'original-fixture.sqlite3'
        desktop(origin, 28)
        with contextlib.closing(sqlite3.connect(origin)) as conn:
            conn.execute('PRAGMA journal_mode=WAL')
            conn.execute('PRAGMA wal_autocheckpoint=0')
            insert_required(conn, 'audit_events', {'id':'probe-wal-row', 'event_type':'fixture', 'details_json':b'\x00\xff'})
            conn.commit()
            for suffix in ('', '-wal', '-shm'):
                Path(str(source)+suffix).write_bytes(Path(str(origin)+suffix).read_bytes())
        before = {p.name:v1.hash_file(p) for p in sources.iterdir()}
        real_connect = sqlite3.connect
        connections = []
        original_database_paths = set()
        def private_connect(database, *args, **kwargs):
            value = str(database)
            require(str(sources) not in value and sources.as_uri() not in value, 'Original source SQLite open refused')
            require(value.split('?')[0] not in original_database_paths, 'Original target/quarantine SQLite open refused')
            connections.append(value)
            return real_connect(database, *args, **kwargs)
        with patch.object(sqlite3, 'connect', private_connect):
            inventory = v1.inventory(source, 'desktop', outputs/'inventory')
            manifest = json.loads(Path(inventory['inventory']).read_text())
            require(manifest['freeze'] == v1.linux_boundary().FREEZE, 'Actual Linux fence receipt')
            require(manifest['content']['audit_events']['rows'] == 1, 'Committed WAL row retained')
            authority = approvals/'approval.json'
            v1.write_json(authority, {'format':v1.FORMAT, 'state':'approved', 'purpose':'historical_export',
                                     'inventory_sha256':inventory['approved_inventory_sha256'],
                                     'expires_at':(datetime.now(timezone.utc)+timedelta(minutes=5)).isoformat()})
            export = v1.export(Path(inventory['inventory']), inventory['approved_inventory_sha256'],
                               authority, v1.hash_file(authority), outputs/'export')
            bundle = quarantine.create_quarantine(Path(export['export']), export['export_sha256'], outputs/'quarantine')
            require((outputs/'quarantine'/'quarantine.sqlite3').is_file(), 'Actual inert quarantine')
            require(v1.POLICY['historical_only'] and all(not v for k,v in v1.POLICY.items() if k != 'historical_only'), 'No execution authority')
            # Copy only already hash-pinned immutable code references to our own
            # ext4 fixture. DrvFs itself is never declared lease-qualified.
            references = root/'profile-reference'; references.mkdir()
            for p, expected in target_profile.source_paths().items():
                relative = p.relative_to(Path(target_profile.__file__).parent/'reference_source')
                data = p.read_bytes()
                require(hashlib.sha256(data).hexdigest() == expected, 'Original immutable profile source pin')
                destination = references/relative
                destination.parent.mkdir(parents=True,exist_ok=True)
                destination.write_bytes(data)
            targets = root/'target-input'; targets.mkdir()
            target = targets/'cold-target.sqlite3'
            with contextlib.closing(target_profile.reference()) as ref, contextlib.closing(real_connect(target)) as conn:
                ref.backup(conn)
                conn.execute('INSERT INTO identity_subjects VALUES(?,?,?,?,?,?,?,?)',
                             ('50000000-0000-0000-0000-000000000001','fixture-human','Fixture',1,0,1,
                              'DO_NOT_EXPORT_PASSWORD_SALT','DO_NOT_EXPORT_PASSWORD_HASH'))
                conn.commit()
            original_database_paths.update((str(target),target.as_uri(),
                str(outputs/'quarantine'/'quarantine.sqlite3'),(outputs/'quarantine'/'quarantine.sqlite3').as_uri()))
            target_before = v1.hash_file(target)
            mapping = approvals/'mapping.json'
            v1.write_json(mapping, {'format':quarantine.MAPPING_FORMAT,'purpose':'historical_preview',
                                   'export_sha256':export['export_sha256'],'target_snapshot_sha256':target_before,'entries':[]})
            preview = quarantine.preview(Path(bundle['quarantine']),bundle['quarantine_sha256'],
                                         target,target_before,mapping,v1.hash_file(mapping),outputs/'preview',references)
            preview_bytes = Path(preview['preview']).read_bytes()
            require(preview['activation_allowed'] is False and preview['state'] == 'inactive' and
                    b'DO_NOT_EXPORT_PASSWORD' not in preview_bytes and v1.hash_file(target) == target_before,
                    'Original cold target unchanged, no credential or activation export')
        checks.append('actual_committed_WAL_inventory_approval_export_quarantine_no_original_SQLite_open')
        checks.append('actual_native_readonly_preview_25_original_profile_pins_no_target_SQLite_open_or_credentials')
        require(before == {p.name:v1.hash_file(p) for p in sources.iterdir()}, 'Original byte provenance unchanged')

        with open(source, 'r+b'):
            refused(lambda: v1.FrozenFiles([source]).__enter__())
        checks.append('preexisting_native_writer_refused')
        with open(source, 'r+b') as writer:
            mapped = mmap.mmap(writer.fileno(), 0, access=mmap.ACCESS_WRITE)
        try:
            refused(lambda: v1.FrozenFiles([source]).__enter__())
        finally:
            mapped.close()
        checks.append('preexisting_writable_mmap_after_fd_close_refused')

        child_receipt = None
        with v1.FrozenFiles([source]) as fence:
            child = subprocess.Popen([sys.executable, '-B', '-c',
                'import os,sys,errno\ntry:\n fd=os.open(sys.argv[1],os.O_WRONLY|os.O_NONBLOCK);os.close(fd);sys.exit(3)\n'
                'except OSError as e:\n sys.exit(0 if e.errno in (errno.EAGAIN,errno.EWOULDBLOCK) else 4)', str(source)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            out, err = child.communicate(timeout=5)
            require(child.returncode == 0 and not out and not err, 'Actual new write open blocked and original child reaped')
            child_receipt = {'exit_code':child.returncode,'positive_wait':True,'stdout_bytes':len(out),'stderr_bytes':len(err)}
            refused(fence.tick)
        checks.append('new_native_write_open_causes_permanent_lease_break_refusal')

        with v1.FrozenFiles([source]) as fence:
            sidecar = Path(str(source)+'-journal')
            sidecar.touch(); sidecar.unlink()
            refused(fence.tick)
        checks.append('transient_absent_sidecar_create_delete_detected')

        with v1.FrozenFiles([source]) as fence:
            moved = root/'moved-sources'
            sources.rename(moved); moved.rename(sources)
            refused(fence.tick)
        checks.append('original_ancestor_rename_and_restore_detected')

        hardlink = root/'hardlink.sqlite3'
        os.link(source, hardlink)
        try:
            refused(lambda:v1.bounded_path(source))
        finally:
            hardlink.unlink()
        symlink = root/'symlink.sqlite3'; symlink.symlink_to(source)
        try:
            refused(lambda:v1.bounded_path(symlink))
        finally:
            symlink.unlink()
        checks.append('hardlink_symlink_inputs_refused')

        stage = outputs/'.own-no-replace-stage'; stage.mkdir()
        (stage/'own-file').write_bytes(b'original-stage')
        destination = outputs/'existing'; destination.mkdir()
        destination_identity = v1.stage_identity(destination)
        stage_identity = v1.stage_identity(stage)
        refused(lambda:v1.publish(stage,destination,stage_identity))
        require(v1.stage_identity(destination) == destination_identity and not list(destination.iterdir()), 'Existing destination unchanged')
        v1.cleanup_stage(stage, stage_identity)
        checks.append('actual_exclusive_directory_no_replace_and_scoped_stage_cleanup')

        stage = outputs/'.own-post-rename-stage'; stage.mkdir()
        (stage/'own-file').write_bytes(b'inert-unconfirmed')
        stage_identity = v1.stage_identity(stage)
        target = outputs/'unconfirmed'
        linux = v1.linux_boundary()
        original_check = linux.check_active
        def intervening_change():
            if target.is_dir() and not hasattr(intervening_change,'changed'):
                intervening_change.changed = True
                sidecar = Path(str(source)+'-journal')
                sidecar.touch(); sidecar.unlink()
            original_check()
        with v1.FrozenFiles([source]):
            with patch.object(linux, 'check_active', intervening_change):
                refused(lambda:v1.publish(stage,target,stage_identity))
        require(target.is_dir() and linux.outside_pending(target).is_file(), 'Unconfirmed publication retained under independent marker')
        (target/'own-file').write_bytes(b'unconfirmed-fixture-input')
        refused(lambda:v1.bounded_path(target/'own-file'))
        refused(lambda:FileFence([target/'own-file']).__enter__())
        v1.cleanup_stage(target, v1.stage_identity(target))
        linux.outside_pending(target).unlink()
        checks.append('real_post_create_source_change_retains_independent_inert_marker_and_input_refusal')

        stage = outputs/'.own-name-race-stage'; stage.mkdir()
        (stage/'own-file').write_bytes(b'original-owned-stage')
        stage_identity = v1.stage_identity(stage)
        saved = outputs/'.own-original-stage-saved'
        foreign = outputs/'.foreign-name-race-fixture'; foreign.mkdir()
        (foreign/'foreign-file').write_bytes(b'foreign-preserved')
        foreign_identity = v1.stage_identity(foreign)
        target = outputs/'name-race-output'
        def replace_stage_name():
            if linux.outside_pending(target).exists() and not hasattr(replace_stage_name,'changed'):
                replace_stage_name.changed = True
                stage.rename(saved); foreign.rename(stage)
            original_check()
        with patch.object(linux,'check_active',replace_stage_name):
            refused(lambda:v1.publish(stage,target,stage_identity))
        require(not target.exists() and v1.stage_identity(stage)==foreign_identity and
                (stage/'foreign-file').read_bytes()==b'foreign-preserved' and linux.outside_pending(target).is_file(),
                'Foreign replacement never moved/adopted; independent output marker retained')
        stage.rename(foreign); saved.rename(stage)
        v1.cleanup_stage(stage,stage_identity)
        linux.outside_pending(target).unlink()
        checks.append('actual_stage_name_swap_refused_without_moving_foreign_directory')

        stage = outputs/'.own-deadline-stage'; stage.mkdir()
        (stage/'own-file').write_bytes(b'owned-deadline-fixture')
        stage_identity = v1.stage_identity(stage)
        target = outputs/'deadline-output'
        deadline = v1.Budget(time.monotonic()+120)
        original_copy = linux._copy_tree
        def expire_during_copy(*args,**kwargs):
            deadline.deadline = time.monotonic()-1
            # Actual slow-copy expiry, without sleeping or extending the budget.
            args = list(args); args[7] = deadline.deadline
            return original_copy(*args,**kwargs)
        with patch.object(linux,'_copy_tree',expire_during_copy):
            refused(lambda:v1.publish(stage,target,stage_identity,deadline))
        require(target.is_dir() and linux.outside_pending(target).exists(), 'Original deadline preserves inert publication')
        v1.cleanup_stage(target,v1.stage_identity(target))
        v1.cleanup_stage(stage,stage_identity)
        linux.outside_pending(target).unlink()
        checks.append('original_operation_deadline_enforced_during_publication_copy')

        require(before == {p.name:v1.hash_file(p) for p in sources.iterdir()}, 'All original source bytes still unchanged')
        for p in sources.iterdir():
            with open(p,'r+b'):
                pass
        require(signal.pthread_sigmask(signal.SIG_BLOCK,set()) == initial_mask and signal.getsignal(signal.SIGIO) == initial_handler, 'Original signal ownership restored')
        require(not linux._files and not linux._namespaces and linux._signals is None, 'All original handles and lease signal descriptor closed')
        checks.append('original_sources_write_reopen_signal_restore_and_no_held_handles')
        return {'status':'passed','checks':checks,'check_count':len(checks),'seconds':round(time.monotonic()-started,3),
                'kernel':os.uname().release,'sqlite':sqlite3.sqlite_version,'original_source_opened_by_SQLite':False,
                'source_bytes_unchanged':True,'native_child':child_receipt,'private_SQLite_connections':len(connections),
                'physical_V1_installation_qualified':False,'migration_activation':False,'m9_complete':False,
                'quarantine_receipt':bundle,
                'preview_receipt':preview,
                'profile_reference_sources':len(target_profile.SOURCE_BINDINGS),
                'profile_reference_sha256':target_profile.SOURCE_BINDINGS,
                'source_sha256':{name:hashlib.sha256((Path(__file__).parent/name).read_bytes()).hexdigest()
                    for name in ('linux_source_fence.py','v1_sqlite_export.py','migration_io.py','linux_migration_probe.py',
                                 'v1_quarantine.py','schema_profiles.py','v2_snapshot_profile.py','prepare_profile_reference.py')}}


if __name__ == '__main__':
    print(json.dumps(main(), sort_keys=True))
