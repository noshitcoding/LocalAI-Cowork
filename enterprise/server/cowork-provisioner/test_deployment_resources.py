import copy
import importlib.util
from pathlib import Path
import shutil
import subprocess
import unittest

path = Path(__file__).resolve().parents[2] / 'deploy' / 'validate_resources.py'
spec = importlib.util.spec_from_file_location('deployment_resources', path)
validator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


class DeploymentResourceTests(unittest.TestCase):
    def setUp(self):
        self.service = {'cpus': 1, 'mem_limit': '512m', 'pids_limit': 128}

    def test_missing_or_unlimited_resources_and_application_socket_rejected(self):
        for key in ['cpus', 'mem_limit', 'pids_limit']:
            value = copy.deepcopy(self.service)
            del value[key]
            with self.subTest(key=key), self.assertRaises(validator.ConfigurationError):
                validator.validate({'services': {'management': value}})
        for override in [{'cpus': 0}, {'pids_limit': -1}, {'mem_limit': '0'}, {'privileged': True},
                         {'volumes': ['/var/run/docker.sock:/var/run/docker.sock']}, {'cpus': True}]:
            with self.subTest(override=override), self.assertRaises(validator.ConfigurationError):
                validator.validate({'services': {'api': {**self.service, **override}}})

    def test_replica_resources_and_sandbox_budgets_are_accounted_separately(self):
        value = validator.validate({'services': {'management': {**self.service, 'deploy': {'replicas': 2}},
                'runner': {**self.service, 'environment': {'COWORK_RUNNER_CPU_MILLIS': '4000',
                    'COWORK_RUNNER_MEMORY_MIB': '8192', 'COWORK_RUNNER_CONCURRENT_RUNS': '2'}}}})
        self.assertEqual(value['service_maximum'], {'cpu_millis': 3000, 'memory_mib': 1536, 'pids': 384})
        self.assertEqual(value['sandbox_budgets'][0]['memory_mib'], '8192')

    def test_checked_in_profiles_have_resource_bounds(self):
        try:
            import yaml
        except ImportError:
            self.skipTest('Install PyYAML to validate source Compose profiles; rendered JSON validator uses only stdlib')
        deploy = path.parent
        for relative in ['docker-compose.yml', 'enterprise/docker-compose.yml', 'enterprise/docker-compose.local.yml',
                         'enterprise/docker-compose.executor.yml']:
            with self.subTest(profile=relative):
                value = yaml.safe_load((deploy / relative).read_text())
                result = validator.validate(value)
                self.assertEqual(result['services'], len(value['services']))

    @unittest.skipUnless(shutil.which('helm'), 'Helm CLI is needed to render the instance chart')
    def test_instance_helm_enforces_single_api_and_separate_caps(self):
        try:
            import yaml
        except ImportError:
            self.skipTest('PyYAML is needed to inspect rendered Helm documents')
        arguments = ['helm', 'template', 'fixture', str(path.parent / 'enterprise' / 'instance')]
        values = {'image': 'registry.test/server@sha256:' + 'a' * 64,
                  'instanceId': '11111111-1111-4111-8111-111111111111',
                  'organisationId': '22222222-2222-4222-8222-222222222222',
                  'runnerUrl': 'http://qualified-runner:8090', 's3Endpoint': 'https://storage.test', 's3Bucket': 'fixture'}
        for name in ['bootstrap_token', 'database_url', 'storage_master_key', 's3_access_key', 's3_secret_key', 'runner_signing_key']:
            values['secrets.' + name] = 'test-only-placeholder'
        for key, value in values.items():
            arguments.extend(['--set-string', key + '=' + value])
        rendered = subprocess.run(arguments, capture_output=True, text=True, timeout=30)
        self.assertEqual(rendered.returncode, 0, rendered.stderr)
        documents = list(yaml.safe_load_all(rendered.stdout))
        deployments = {item['metadata']['name']: item for item in documents if item.get('kind') == 'Deployment'}
        self.assertEqual(deployments['api']['spec']['replicas'], 1)
        self.assertEqual(deployments['api']['spec']['template']['spec']['containers'][0]['resources']['limits'],
                         {'cpu': '1000m', 'memory': '512Mi'})
        self.assertEqual(deployments['worker']['spec']['template']['spec']['containers'][0]['resources']['limits'],
                         {'cpu': '500m', 'memory': '512Mi'})
        quota = next(item for item in documents if item.get('kind') == 'ResourceQuota')
        self.assertEqual(quota['spec']['hard'], {'limits.cpu': '1500m', 'limits.memory': '1024Mi'})
        rejected = subprocess.run([*arguments, '--set', 'replicas=2'], capture_output=True, text=True, timeout=30)
        self.assertNotEqual(rejected.returncode, 0)
        invalid = subprocess.run([*arguments, '--set', 'serviceResources.runner.cpu_millis=1000'], capture_output=True,
                                 text=True, timeout=30)
        self.assertNotEqual(invalid.returncode, 0)


if __name__ == '__main__':
    unittest.main()
