import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import Mock, patch

import executor_host
import guest_endpoints as guest
import provisioner as p
import resource_budget


class GuestEndpointTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.ca = self.root / 'public.pem'
        self.ca.write_text('-----BEGIN CERTIFICATE-----\nZml4dHVyZS1vbmx5\n-----END CERTIFICATE-----')
        self.ca.chmod(0o600)
        selection = {'id': str(uuid.uuid4()), 'trust_domain': str(uuid.uuid4()),
                     'operator_state_id': str(uuid.uuid4()), 'endpoint': 'tcp://localhost:22376',
                     'daemon_id': 'a' * 64, 'ca_file': str(self.ca), 'cert_file': str(self.ca), 'key_file': str(self.ca)}
        self.instance = {'id': str(uuid.uuid4()), 'organisation_id': selection['trust_domain'],
                         'backend': 'compose', 'executor_host_id': selection['id'],
                         'policy': {'minimum_isolation': 'container', 'limits': {'cpu_millis': 500,
                             'memory_mib': 512, 'storage_gib': 1, 'concurrent_runs': 1}}}
        self.config = {'backend': 'compose', 'state_directory': str(self.root / 'state'), 'executor_host': selection,
                       'sandbox_security_profile': 'linux_apparmor', 'fleet_host_budget': {'cpu_millis': 4000,
                           'memory_mib': 3584, 'reserved_cpu_millis': 1000, 'reserved_memory_mib': 1024},
                       'database_endpoint': '10.0.2.2:23456', 's3_endpoint': 'https://10.0.2.2:23457',
                       'fleet_management_url': 'https://10.0.2.2:23458', 'services_network': 'fixture-services',
                       'routing_network': 'fixture-routing', 'images': {name: 'registry.invalid/candidate@sha256:' + 'b' * 64
                           for name in ['server', 'runner', 'sandbox_core', 'sandbox_gui', 'sandbox_crew', 'egress_proxy']}}
        self.profile = {'version': 1, 'instance_id': self.instance['id'],
                        'executor_host_binding': executor_host.manager_binding(self.config),
                        'public_ca_file': str(self.ca), 'api_loopback_port': 38080,
                        **{name: self.config[name] for name in ['database_endpoint', 's3_endpoint', 'fleet_management_url']}}
        self.config['guest_runtime_endpoints'] = self.profile
        context = patch.object(guest.ssl, 'create_default_context', return_value=Mock())
        context.start(); self.addCleanup(context.stop)

    def test_profile_pins_host_instance_and_verified_runtime_endpoints(self):
        manifest = p.compose_manifest(self.config, self.instance, self.root)
        for name in ['api', 'worker']:
            service = manifest['services'][name]
            self.assertIn('control_public_ca', service['secrets'])
            self.assertEqual(service['environment']['COWORK_FLEET_CA_CERTIFICATE_FILE'], '/run/secrets/control_public_ca')
            self.assertEqual(service['environment']['COWORK_S3_CA_CERTIFICATE_FILE'], '/run/secrets/control_public_ca')
        self.assertEqual(manifest['services']['api']['ports'], [{'target': 8080, 'published': '38080', 'host_ip': '127.0.0.1', 'protocol': 'tcp'}])
        self.assertNotIn('ports', manifest['services']['worker'])
        self.assertNotIn('control_public_ca', manifest['services']['runner']['secrets'])
        budget = resource_budget.manifest_budget(manifest, self.instance['id'])
        self.assertEqual(budget['total'], {'cpu_millis': 2750, 'memory_mib': 1920})
        self.assertEqual(guest.database_url(self.config, 'postgres://private@10.0.2.2:23456/fixture'),
                         'postgres://private@10.0.2.2:23456/fixture?sslmode=verify-full&sslrootcert=/run/secrets/control_public_ca')

    def test_legacy_profile_is_unchanged(self):
        self.assertIsNone(guest.settings({}))
        value = {'original': True}
        self.assertIs(guest.apply_manifest({}, value, self.root), value)
        self.assertEqual(guest.database_url({}, 'original'), 'original')

    def test_transport_configuration_rejects_redirect_credentials_http_and_non_qemu_hosts(self):
        for endpoint in ['http://10.0.2.2:23458', 'https://user:private@10.0.2.2:23458',
                         'https://10.0.2.2:23458?admin=1', 'https://10.0.2.2:23458/management',
                         'https://localhost:23458', 'https://10.0.2.2:443', 'https://10.0.2.2:99999']:
            altered = copy.deepcopy(self.config)
            altered['fleet_management_url'] = altered['guest_runtime_endpoints']['fleet_management_url'] = endpoint
            with self.subTest(endpoint=endpoint), self.assertRaises(guest.GuestEndpointError):
                guest.settings(altered)

    def test_foreign_binding_second_instance_and_unenforced_apparmor_are_refused(self):
        for mutate in [lambda c: c['guest_runtime_endpoints']['executor_host_binding'].update({'host_id': str(uuid.uuid4())}),
                       lambda c: c.update({'sandbox_security_profile': 'docker_desktop_seccomp'}),
                       lambda c: c['guest_runtime_endpoints'].update({'unknown': True}),
                       lambda c: c['guest_runtime_endpoints'].update({'instance_id': str(uuid.uuid4())})]:
            altered = copy.deepcopy(self.config); mutate(altered)
            with self.assertRaises((guest.GuestEndpointError, p.ProvisioningError)):
                p.compose_manifest(altered, self.instance, self.root)

    def test_public_ca_rejects_private_key_and_real_invalid_pem(self):
        self.ca.write_text('-----BEGIN CERTIFICATE-----\nfixture\nPRIVATE KEY')
        with self.assertRaisesRegex(guest.GuestEndpointError, 'private_material'):
            guest.public_ca(self.ca)
        self.ca.write_text('-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----')
        with patch.object(guest.ssl, 'create_default_context', side_effect=guest.ssl.SSLError()):
            with self.assertRaisesRegex(guest.GuestEndpointError, 'invalid_guest_public_ca'):
                guest.public_ca(self.ca)

    def test_ca_copy_contains_only_public_certificate_and_is_readable_by_runtime(self):
        (self.root / 'runtime-secrets').mkdir()
        guest.copy_public_ca(self.config, self.root, p.private_write)
        target = self.root / 'runtime-secrets/control_public_ca'
        self.assertEqual(target.read_bytes(), self.ca.read_bytes())
        if os.name == 'posix':
            self.assertEqual(target.stat().st_mode & 0o777, 0o444)

    def test_saved_endpoint_profile_cannot_retarget_disable_or_enroll_existing_inventory(self):
        state = self.root / 'state'; state.mkdir()
        infrastructure = Mock(root=state, config=self.config, private_write=p.private_write)
        guest.bind(infrastructure); guest.bind(infrastructure)
        marker = json.loads((state / 'guest-runtime-endpoints.json').read_text())
        self.assertNotIn('public_ca_file', marker)
        changed = copy.deepcopy(self.config)
        changed['guest_runtime_endpoints']['api_loopback_port'] += 1
        infrastructure.config = changed
        with self.assertRaisesRegex(guest.GuestEndpointError, 'retarget'):
            guest.bind(infrastructure)
        infrastructure.config = {}
        with self.assertRaisesRegex(guest.GuestEndpointError, 'disable'):
            guest.bind(infrastructure)
        (state / 'guest-runtime-endpoints.json').unlink()
        (state / uuid.uuid4().hex).mkdir()
        infrastructure.config = self.config
        with self.assertRaisesRegex(guest.GuestEndpointError, 'existing_inventory'):
            guest.bind(infrastructure)


if __name__ == '__main__':
    unittest.main()
