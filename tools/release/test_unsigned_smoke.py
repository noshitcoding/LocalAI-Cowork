"""Synthetic coordinator/Linux counterexamples; no installer/network/native launch."""
import argparse
import ast
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import urllib.error

import installed_lifetime_linux as linux
import unsigned_smoke as coordinator
import verify_review as verify


class CoordinatorTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).absolute()
        self.event = self.root / 'event.json'
        self.event.write_text(json.dumps({'repository': {'default_branch': 'main', 'full_name': 'owner/repo'}}))
        self.env = {'GITHUB_ACTIONS': 'true', 'GITHUB_EVENT_NAME': 'workflow_dispatch',
            'CI_RUNNER_ENVIRONMENT': 'github-hosted', 'GITHUB_EVENT_PATH': str(self.event),
            'GITHUB_REF': 'refs/heads/main', 'GITHUB_REPOSITORY': 'owner/repo',
            'GITHUB_SHA': 'a' * 40, 'RUNNER_OS': 'Linux', 'RUNNER_TEMP': str(self.root)}

    def test_host_selfhosted_pr_and_nondefault_harness_are_refused(self):
        coordinator.hosted_guard(self.env)
        for key, value in [('GITHUB_ACTIONS', 'false'), ('GITHUB_EVENT_NAME', 'pull_request'),
                ('CI_RUNNER_ENVIRONMENT', 'self-hosted'), ('GITHUB_REF', 'refs/heads/feature'),
                ('GITHUB_SHA', 'main'), ('GITHUB_REPOSITORY', 'fork/repo')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                coordinator.hosted_guard({**self.env, key: value})

    def test_native_child_environment_contains_no_inherited_credentials_or_overrides(self):
        selected = coordinator.clean_environment({**self.env, 'PATH': '/usr/bin', 'DISPLAY': ':99',
            'GH_TOKEN': 'secret-a', 'GITHUB_TOKEN': 'secret-b', 'ACTIONS_RUNTIME_TOKEN': 'secret-c',
            'MODEL_API_PASSWORD': 'secret-d', 'AWS_SECRET_ACCESS_KEY': 'secret-e',
            'COWORK_DATABASE_URL': 'foreign', 'HTTPS_PROXY': 'foreign',
            'PSModulePath': 'foreign-ps7', 'PSMODULEPATH': 'foreign-ps7',
            'psmodulepath': 'foreign-ps7'})
        self.assertEqual(selected['PATH'], '/usr/bin')
        self.assertEqual(selected['DISPLAY'], ':99')
        self.assertTrue(all(value not in selected.values() for value in ['secret-a', 'secret-b', 'secret-c', 'secret-d', 'secret-e', 'foreign']))
        self.assertEqual(selected['GITHUB_EVENT_NAME'], 'workflow_dispatch')
        for key in ('PSModulePath', 'PSMODULEPATH', 'psmodulepath'):
            with self.subTest(key=key):
                self.assertNotIn(key, selected)

    def test_exact_bundle_command_and_package_name_no_shell_fallback(self):
        for bad in ['', '-foreign', 'pkg; touch /tmp/x', '../pkg', 'pkg\nother']:
            with self.subTest(name=bad), self.assertRaises(ValueError):
                linux.package_commands('deb', Path('/original/package.deb'), bad)
        command = linux.package_commands('deb', Path('/original/exact package.deb'), 'localai-cowork-v2-desktop')
        self.assertEqual(command['install'][-1], str(Path('/original/exact package.deb')))
        self.assertEqual(command['remove'][-1], 'localai-cowork-v2-desktop')
        self.assertEqual(linux.package_commands('appimage', Path('/own/original.AppImage')),
                         {'launch': [str(Path('/own/original.AppImage'))]})
        arguments = [Path('/bound.json'), Path('/original'), Path('/own'), Path('/evidence')]
        for kind in ('nsis', 'deb', 'appimage'):
            generated = coordinator.native_command(kind, *arguments)
            self.assertNotIn('--fast', generated)
            self.assertNotIn('build', generated)
        with self.assertRaises(ValueError):
            coordinator.native_command('foreign', *arguments)

    def test_changed_binding_refused_before_metadata_or_installer(self):
        prepared = self.root / 'prepared'
        prepared.mkdir()
        (prepared / 'input-binding.json').write_text('{}')
        args = argparse.Namespace(kind='deb', prepared=prepared, binding_sha256='0' * 64)
        with patch.object(coordinator, 'hosted_guard', return_value=self.env), \
             patch.object(coordinator, 'github_json') as metadata, \
             patch.object(coordinator.subprocess, 'run') as native, self.assertRaises(ValueError):
            coordinator.execute(args)
        metadata.assert_not_called()
        native.assert_not_called()

    def test_download_redirect_drops_authorization_and_wrong_archive_fails(self):
        original = b'original synthetic ZIP bytes'
        api = Mock()
        api.open.side_effect = urllib.error.HTTPError('https://api.github.com/exact', 302, 'Found',
            {'Location': 'https://storage.example.test/signed?opaque=hidden'}, None)
        plain = Mock()
        plain.open.return_value = io.BytesIO(original)
        with patch.dict(os.environ, {'GH_TOKEN': 'never-forward-this'}), \
             patch.object(coordinator.urllib.request, 'build_opener', side_effect=[api, plain]):
            coordinator.download_archive('owner/repo', 88, self.root / 'original.zip', hashlib.sha256(original).hexdigest())
        forwarded = plain.open.call_args.args[0]
        self.assertIsNone(forwarded.get_header('Authorization'))
        plain.open.return_value = io.BytesIO(b'changed')
        with patch.dict(os.environ, {'GH_TOKEN': 'secret'}), \
             patch.object(coordinator.urllib.request, 'build_opener', side_effect=[api, plain]), self.assertRaises(ValueError):
            coordinator.download_archive('owner/repo', 88, self.root / 'changed.zip', hashlib.sha256(original).hexdigest())

    def test_nonhttps_artifact_redirect_refused_before_file_creation(self):
        api = Mock()
        api.open.side_effect = urllib.error.HTTPError('https://api.github.com/exact', 302, 'Found',
            {'Location': 'http://untrusted.example.test/file'}, None)
        with patch.dict(os.environ, {'GH_TOKEN': 'secret'}), \
             patch.object(coordinator.urllib.request, 'build_opener', return_value=api), self.assertRaises(ValueError):
            coordinator.download_archive('owner/repo', 88, self.root / 'not-created.zip', '0' * 64)
        self.assertFalse((self.root / 'not-created.zip').exists())

    def test_foreign_session_never_receives_a_pidfd_signal(self):
        with patch.object(linux.os, 'pidfd_open', return_value=9, create=True), \
             patch.object(linux.os, 'getuid', return_value=1000, create=True), \
             patch.object(linux.os, 'close') as close, \
             patch.object(linux, 'proc_info', return_value={'session': 77, 'uid': 1000}), \
             patch.object(linux.signal, 'pidfd_send_signal', create=True) as kill, self.assertRaises(ValueError):
            linux.capture(42, 88)
        close.assert_called_once_with(9)
        kill.assert_not_called()

    def test_timeout_has_no_fabricated_success_or_exit_code(self):
        report = {'commands': []}
        with patch.object(linux.subprocess, 'run', side_effect=subprocess.TimeoutExpired(['exact'], 1)), self.assertRaises(ValueError):
            linux.command(report, self.root, 'timed-out', ['exact'], timeout=1)
        self.assertIsNone(report['commands'][0]['exit_code'])
        self.assertTrue(report['commands'][0]['timed_out'])

    def test_linux_signals_only_retained_pidfds_and_has_no_build_or_shell_execution(self):
        tree = ast.parse(Path(linux.__file__).read_text())
        for node in ast.walk(tree):
            if isinstance(node, ast.Call):
                if isinstance(node.func, ast.Attribute):
                    self.assertNotIn(node.func.attr, ('kill', 'killpg', 'system', 'popen'))
                self.assertFalse(any(key.arg == 'shell' and isinstance(key.value, ast.Constant)
                                     and key.value.value is True for key in node.keywords))


if __name__ == '__main__':
    unittest.main(verbosity=2)
