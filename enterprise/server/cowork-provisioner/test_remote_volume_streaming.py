import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
import uuid
from unittest.mock import Mock

from instance_backups import BackupError, InstanceBackups
import remote_volume_streaming as stream


class VolumeStreamingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.limits = {'version': 1, 'max_archive_bytes': 1048576, 'max_file_bytes': 524288,
                       'max_files': 100, 'timeout_seconds': 10}

    def archive(self, entries):
        target = self.root / (uuid.uuid4().hex + '.tar')
        with tarfile.open(target, 'w') as output:
            for entry, content in entries:
                output.addfile(entry, io.BytesIO(content) if content is not None else None)
        expected = {'bytes': target.stat().st_size, 'sha256': hashlib.sha256(target.read_bytes()).hexdigest()}
        return target, expected

    def test_verified_archive_accepts_bounded_regular_files(self):
        entry = tarfile.TarInfo('subdir/marker.txt'); entry.size = 6
        target, expected = self.archive([(entry, b'marker')])
        self.assertEqual(stream.verified_archive(target, expected, self.limits), expected)

    def test_links_special_files_duplicates_and_path_escape_refused(self):
        cases = []
        for name in ['../outside', '/absolute', 'bad\\name', '.', 'sub/./file']:
            entry = tarfile.TarInfo(name); entry.size = 1; cases.append([(entry, b'x')])
        for kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE]:
            entry = tarfile.TarInfo('unsafe'); entry.type = kind; entry.linkname = '../outside'; cases.append([(entry, None)])
        entry = tarfile.TarInfo('same'); entry.size = 1; cases.append([(entry, b'x'), (entry, b'x')])
        parent = tarfile.TarInfo('parent'); parent.size = 1
        child = tarfile.TarInfo('parent/file'); child.size = 1; cases.append([(parent, b'x'), (child, b'x')])
        for entries in cases:
            target, expected = self.archive(entries)
            with self.assertRaises(stream.StreamError): stream.verified_archive(target, expected, self.limits)

    def test_integrity_file_count_and_byte_bounds_checked_before_any_docker_action(self):
        entry = tarfile.TarInfo('marker'); entry.size = 2
        target, expected = self.archive([(entry, b'ok')])
        for changed, limits in [({**expected, 'sha256': 'a' * 64}, self.limits),
                                 (expected, {**self.limits, 'max_file_bytes': 1}),
                                 (expected, {**self.limits, 'max_files': 0})]:
            with self.assertRaises(stream.StreamError): stream.verified_archive(target, changed, limits)
        operator = object.__new__(stream.VolumeStreamer)
        operator.backup_root = self.root; operator.limits = self.limits; operator.infra = Mock()
        self.root.chmod(0o700)
        with self.assertRaises(stream.StreamError):
            operator.restore(self.root, uuid.uuid4().hex, target, {**expected, 'sha256': 'a' * 64})
        operator.infra.run.assert_not_called()

    def test_missing_source_volume_cannot_silently_create_empty_volume_or_archive(self):
        operator = object.__new__(stream.VolumeStreamer)
        operator.backup_root = self.root; operator.limits = self.limits
        operator.scope = Mock(return_value='owned-existing-volume'); operator.quiescent = Mock()
        operator.inspect_volume = Mock(side_effect=stream.StreamError('missing source'))
        operator.transfer = Mock(); self.root.chmod(0o700)
        target = self.root / (uuid.uuid4().hex + '.tar')
        with self.assertRaises(stream.StreamError): operator.capture(self.root, uuid.uuid4().hex, target)
        self.assertFalse(target.exists()); operator.transfer.assert_not_called()

    def test_preparation_option_cannot_open_existing_remote_lifecycle_gates(self):
        infrastructure = SimpleNamespace(config={'executor_host': {'id': str(uuid.uuid4())},
            'remote_volume_streaming_preparation': self.limits})
        with self.assertRaisesRegex(BackupError, 'remote_executor_lifecycle_not_qualified'):
            InstanceBackups(infrastructure)

    @unittest.skipUnless(os.name == 'posix', 'POSIX bounded pipe helper test')
    def test_actual_fixed_helper_capture_restore_through_bounded_pipes(self):
        source, restored = self.root / 'source', self.root / 'restored'
        source.mkdir(); restored.mkdir(); (source / 'subdir').mkdir()
        content = b'uuid-workspace-' + uuid.uuid4().hex.encode()
        marker = source / 'subdir/marker'; marker.write_bytes(content); marker.chmod(0o751)
        target = self.root / (uuid.uuid4().hex + '.tar')
        def helper(mode, directory):
            child = subprocess.Popen([sys.executable, '-c', stream.HELPER_SOURCE, mode, str(directory),
                str(os.geteuid()), str(os.getegid()), str(self.limits['max_archive_bytes']),
                str(self.limits['max_file_bytes']), str(self.limits['max_files'])],
                stdin=subprocess.PIPE if mode == 'restore' else subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            self.addCleanup(lambda: child.poll() is None and child.kill())
            return child
        child = helper('capture', source)
        try:
            with target.open('xb') as output:
                evidence = stream.pump(child, output, 'capture', self.limits)
        finally:
            child.stdout.close(); child.wait(timeout=5)
        stream.verified_archive(target, evidence, self.limits)
        child = helper('restore', restored)
        try:
            with target.open('rb') as archive:
                receipt = stream.pump(child, archive, 'restore', self.limits)
        finally:
            child.stdin.close(); child.stdout.close(); child.wait(timeout=5)
        self.assertEqual(receipt['stream_sha256'], evidence['sha256'])
        self.assertEqual(receipt['stream_bytes'], evidence['bytes'])
        self.assertEqual((restored / 'subdir/marker').read_bytes(), content)
        self.assertEqual((restored / 'subdir/marker').stat().st_mode & 0o777, 0o751)

    @unittest.skipUnless(os.name == 'posix', 'POSIX timeout/backpressure test')
    def test_stalled_helper_is_bounded_by_monotonic_deadline(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(30)'],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            with self.assertRaisesRegex(stream.StreamError, 'volume_streaming_timeout'):
                stream.pump(child, io.BytesIO(), 'capture', {**self.limits, 'timeout_seconds': .1})
        finally:
            child.kill(); child.wait(timeout=5); child.stdout.close()


if __name__ == '__main__':
    unittest.main()
