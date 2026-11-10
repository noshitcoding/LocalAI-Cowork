import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
import uuid

import hosted_qemu_process as qemu


class OwnedQemuTests(unittest.TestCase):
    def test_classification_cannot_infer_oom_from_signal_or_render_private_text(self):
        self.assertEqual(qemu.classification(b'private secret', -9), 'terminated_by_sigkill')
        self.assertEqual(qemu.classification(b'Could not allocate backing store: private', 1), 'memory_allocation_failure')
        self.assertEqual(qemu.classification(b'Permission denied: private-path', 1), 'permission_denied')
        self.assertEqual(qemu.classification(b'', None), 'running')

    @unittest.skipUnless(os.name == 'posix', 'POSIX child and socket ownership')
    def test_actual_child_cwd_owned_pid_reap_and_private_capped_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); root.chmod(0o700)
            (root / 'vm.json').write_text('{}'); (root / 'vm.json').chmod(0o600)
            program = """import os,pathlib,socket,sys,time
pathlib.Path('child-cwd.json').write_text(str(pathlib.Path.cwd()))
pathlib.Path('qemu.pid').write_text(str(os.getpid()))
s=socket.socket(socket.AF_UNIX);s.bind('qmp.sock');s.listen(1)
os.write(2,b'permission denied: private secret\\n'+b'x'*100000)
connection,_=s.accept();connection.sendall(b'{"QMP":{}}\\n');connection.close()
while not pathlib.Path('stop-child').exists():time.sleep(.02)
s.close()
"""
            real_popen = subprocess.Popen
            observed = []
            def fake_qemu(command, **kwargs):
                observed.append((command, kwargs))
                return real_popen([sys.executable, '-c', program], **kwargs)
            children = qemu.QemuChildren(); before = Path.cwd()
            argv = ['qemu-system-x86_64', '-m', '4096', '-uuid', str(uuid.uuid4()), '-daemonize',
                    '-pidfile', str(root / 'qemu.pid'), '-qmp', 'unix:qmp.sock,server=on,wait=off']
            with patch.object(qemu.subprocess, 'Popen', side_effect=fake_qemu):
                try:
                    self.assertEqual(children.launch(argv, cwd=root), b'')
                    child = children.children[root]
                    self.assertEqual(children.require_running(root), int((root / 'qemu.pid').read_text()))
                    self.assertEqual(Path((root / 'child-cwd.json').read_text()), root)
                    self.assertEqual(Path.cwd(), before)
                    command, kwargs = observed[0]
                    self.assertEqual(command[:4], ['prlimit', '--as=8589934592:8589934592', '--cpu=5400:5400', '--'])
                    self.assertNotIn('-daemonize', command)
                    self.assertEqual(kwargs['cwd'], root)
                    self.assertEqual(kwargs['stdout'], subprocess.DEVNULL)
                finally:
                    (root / 'stop-child').touch()
                    evidence = children.reap_after_stop(root)
            self.assertEqual(evidence['exit_code'], 0)
            self.assertEqual(evidence['stderr_retained_bytes'], qemu.STDERR_LIMIT)
            self.assertGreater(evidence['stderr_observed_bytes'], qemu.STDERR_LIMIT)
            self.assertTrue(evidence['stderr_truncated'])
            self.assertEqual(evidence['error_classification'], 'permission_denied')
            self.assertNotIn('private secret', json.dumps(evidence))
            diagnostic = root / 'qemu-stderr.private'
            self.assertEqual(diagnostic.stat().st_size, qemu.STDERR_LIMIT)
            self.assertEqual(diagnostic.stat().st_mode & 0o777, 0o600)
            with self.assertRaisesRegex(RuntimeError, 'owned_qemu_exited:permission_denied'):
                children.require_running(root)
            self.assertEqual(child.child.returncode, 0)

    @unittest.skipUnless(os.name == 'posix', 'POSIX child and socket ownership')
    def test_early_exit_is_classified_and_reaped_without_waiting_for_guest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); root.chmod(0o700)
            (root / 'vm.json').write_text('{}'); (root / 'vm.json').chmod(0o600)
            real_popen = subprocess.Popen
            def exiting(command, **kwargs):
                return real_popen([sys.executable, '-c', 'import os;os.write(2,b"Cannot allocate memory");raise SystemExit(3)'], **kwargs)
            children = qemu.QemuChildren(); start = time.monotonic()
            with patch.object(qemu.subprocess, 'Popen', side_effect=exiting):
                with self.assertRaisesRegex(RuntimeError, 'owned_qemu_exited:memory_allocation_failure'):
                    children.launch(['qemu-system-x86_64', '-pidfile', str(root / 'qemu.pid'),
                                     '-qmp', 'unix:qmp.sock,server=on,wait=off'], timeout=10)
            self.assertLess(time.monotonic() - start, 3)
            self.assertEqual(children.evidence(root)['exit_code'], 3)

    @unittest.skipUnless(os.name == 'posix', 'POSIX root ownership')
    def test_foreign_socket_cwd_and_existing_pid_refuse_before_start(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); root.chmod(0o700)
            (root / 'vm.json').write_text('{}'); (root / 'vm.json').chmod(0o600)
            argv = ['qemu-system-x86_64', '-pidfile', str(root / 'qemu.pid'), '-qmp',
                    'unix:' + str(root.parent / 'foreign.sock') + ',server=on,wait=off']
            with patch.object(qemu.subprocess, 'Popen') as spawn:
                with self.assertRaisesRegex(RuntimeError, 'foreign_qemu_socket_refused'):
                    qemu.QemuChildren().launch(argv, cwd=root)
                argv[-1] = 'unix:qmp.sock,server=on,wait=off'
                with self.assertRaisesRegex(RuntimeError, 'foreign_qemu_cwd_refused'):
                    qemu.QemuChildren().launch(argv, cwd=root.parent)
                (root / 'qemu.pid').write_text('999')
                with self.assertRaisesRegex(RuntimeError, 'owned_qemu_launch_requires_fresh_files'):
                    qemu.QemuChildren().launch(argv, cwd=root)
                spawn.assert_not_called()


if __name__ == '__main__':
    unittest.main()
