import argparse
import base64
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import uuid
import xml.etree.ElementTree as ET

import configure
import windows_sandbox as sandbox


class WindowsSandboxTests(unittest.TestCase):
    def test_xml_has_only_explicit_shares_and_network_is_denied_by_default(self):
        grant = {'host': r'C:\Files & data', 'guest': r'C:\CoworkShares\folder-001', 'read_only': False}
        raw = sandbox.configuration_xml(r'C:\Private <bootstrap>', [grant])
        root = ET.fromstring(raw)
        self.assertEqual(root.findtext('Networking'), 'Disable')
        self.assertEqual(root.findtext('ClipboardRedirection'), 'Disable')
        self.assertEqual(root.findtext('AudioInput'), 'Disable')
        self.assertEqual(root.findtext('VideoInput'), 'Disable')
        self.assertEqual(root.findtext('vGPU'), 'Disable')
        mappings = root.findall('./MappedFolders/MappedFolder')
        self.assertEqual(len(mappings), 2)
        self.assertEqual(mappings[0].findtext('ReadOnly'), 'true')
        self.assertEqual(mappings[1].findtext('ReadOnly'), 'false')
        self.assertEqual(mappings[1].findtext('HostFolder'), grant['host'])
        self.assertNotIn(grant['host'], root.findtext('./LogonCommand/Command'))
        self.assertEqual(ET.fromstring(sandbox.configuration_xml('payload', [], True)).findtext('Networking'), 'Enable')

    def test_grants_reject_overlaps_credentials_variables_and_aliases(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            selected = root / 'selected'; selected.mkdir()
            child = selected / 'child'; child.mkdir()
            secret = selected / 'profile.json'; secret.write_text('{}')
            with self.assertRaises(configure.ConfigurationError):
                sandbox.grants([str(selected)], [], [secret])
            with self.assertRaises(configure.ConfigurationError):
                sandbox.grants([str(selected)], [str(child)], [])
            variable = root / '%USERPROFILE%'; variable.mkdir()
            with self.assertRaises(configure.ConfigurationError):
                sandbox.grants([str(variable)], [], [])
            grants = sandbox.grants([str(child)], [], [secret])
            self.assertTrue(grants[0]['read_only'])
            self.assertEqual(grants[0]['guest'], r'C:\CoworkShares\folder-001')

    @unittest.skipUnless(os.name == 'nt', 'Guest bundle creation uses private Windows DACLs')
    def test_prepare_uses_same_protocol_but_distinct_pairing_without_start(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            guest_id, host_id = str(uuid.uuid4()), str(uuid.uuid4())
            registration = {'schema_version': 2, 'protocol_version': 2, 'executor_id': guest_id,
                            'kind': 'personal_device', 'display_name': 'Disposable guest', 'max_concurrent_runs': 1,
                            'personal_device_remote_control': 'confirm_each_session',
                            'capabilities': [{'schema_version': 2, 'name': 'terminal', 'version': '1', 'attributes': {}}]}
            profile = root / 'profile.json'
            profile.write_text(json.dumps({'format': 1, 'protocol_version': 2, 'server_url': 'https://cowork.example',
                                           'registration': registration, 'token': 'private-test-token'}))
            binary = root / 'agent.exe'; binary.write_bytes(b'MZtest-fixture-not-executable')
            shared = root / 'shared'; shared.mkdir()
            args = argparse.Namespace(profile=str(profile), agent_binary=str(binary), output=str(root / 'private-vm'),
                    host_executor_id=host_id, read_only=[str(shared)], read_write=[], network=True,
                    allow_guest_tools=True, memory_mb=4096, model_url=None, model_name=None, model_key_file=None)
            with patch.object(sandbox.subprocess, 'Popen') as launch:
                output = sandbox.prepare(args)
                launch.assert_not_called()
            config = json.loads((output / 'agent.json').read_text())
            self.assertEqual(config['registration'], registration)
            self.assertEqual(config['workspace_root'], r'C:\Cowork\state')
            self.assertEqual(config['agent_token_file'], r'C:\Cowork\agent-token.txt')
            self.assertTrue(json.loads((output / 'toolbox.json').read_text())['native_owner_rights'])
            self.assertFalse(json.loads((output / 'launch-manifest.json').read_text())['runtime_qualified'])
            self.assertEqual((output / 'agent-token.txt').read_text(), 'private-test-token\n')
            # Invalid identity/network/tools are rejected before any output is created.
            for key, value in [('host_executor_id', guest_id), ('network', False), ('allow_guest_tools', False)]:
                invalid = argparse.Namespace(**vars(args)); setattr(invalid, key, value)
                invalid.output = str(root / ('invalid-' + key))
                with self.assertRaises(configure.ConfigurationError):
                    sandbox.prepare(invalid)
                self.assertFalse(pathlib.Path(invalid.output).exists())
            # Reconciliation refuses changed executable bytes instead of starting.
            with (output / 'agent.exe').open('ab') as file:
                file.write(b'changed')
            with patch.object(sandbox, 'discovery', return_value={'launcher_installed': True, 'executable': 'WindowsSandbox.exe'}), \
                 patch.object(sandbox.subprocess, 'Popen') as launch:
                with self.assertRaises(configure.ConfigurationError):
                    sandbox.launch(output)
                launch.assert_not_called()

    def test_guest_approval_console_verifies_original_actions(self):
        self.assertIn("$id + '.original.json'", sandbox.CONSENT_SCRIPT)
        self.assertIn('Get-FileHash', sandbox.CONSENT_SCRIPT)
        self.assertIn('consent --data', sandbox.CONSENT_SCRIPT)
        self.assertNotIn('unattended', sandbox.CONSENT_SCRIPT)
        self.assertIn('--check', sandbox.BOOTSTRAP_SCRIPT)

    @unittest.skipUnless(os.name == 'nt', 'Validate using the Windows PowerShell parser')
    def test_bootstrap_and_consent_are_valid_windows_powershell_without_execution(self):
        for script in [sandbox.BOOTSTRAP_SCRIPT, sandbox.CONSENT_SCRIPT]:
            encoded = base64.b64encode(script.encode('utf-8')).decode('ascii')
            check = ("$source=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('" + encoded + "'));"
                     "$tokens=$null;$errors=$null;[Management.Automation.Language.Parser]::ParseInput($source,[ref]$tokens,[ref]$errors)|Out-Null;"
                     "if($errors.Count){$errors|ForEach-Object{$_.Message};exit 1}")
            result = subprocess.run([str(sandbox.powershell_path()), '-NoProfile', '-NonInteractive', '-EncodedCommand',
                                     base64.b64encode(check.encode('utf-16-le')).decode('ascii')],
                                    capture_output=True, text=True, timeout=15, creationflags=subprocess.CREATE_NO_WINDOW)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
