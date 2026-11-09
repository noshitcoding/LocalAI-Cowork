import copy
import importlib.util
import json
import ipaddress
from pathlib import Path
import tempfile
import subprocess
import traceback
import unittest
import uuid
from unittest.mock import patch, Mock

module_spec = importlib.util.spec_from_file_location("provisioner", Path(__file__).with_name("provisioner.py"))
p = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(p)


class ProvisionerSecurityTests(unittest.TestCase):
    def setUp(self):
        self.instance = {"id": str(uuid.uuid4()), "organisation_id": str(uuid.uuid4()), "backend": "compose", "policy": {
            "minimum_isolation": "container", "limits": {"cpu_millis": 2000, "memory_mib": 4096, "storage_gib": 50, "concurrent_runs": 2}}}
        image = "registry.example/cowork@sha256:" + "a" * 64
        self.config = {"images": {name: image for name in ["server", "runner", "sandbox_core", "sandbox_gui", "sandbox_crew", "egress_proxy"]},
                       "s3_endpoint": "https://storage.example", "services_network": "cowork-services", "routing_network": "cowork-routing"}

    def test_identifiers_cannot_be_paths_or_flags(self):
        for value in ["../etc", "--privileged", str(uuid.uuid4()).upper(), "abc"]:
            with self.assertRaises((ValueError, p.ProvisioningError)):
                p.identifier(value)

    def test_infrastructure_failures_never_log_credentials_in_exception_chains(self):
        secret = "private-test-credential"
        arguments = ["mc", "alias", "set", secret]
        for failure in [subprocess.CalledProcessError(1, arguments, output=secret, stderr=secret),
                        subprocess.TimeoutExpired(arguments, 1, output=secret, stderr=secret),
                        OSError(secret)]:
            with self.subTest(failure=type(failure).__name__), patch.object(p.subprocess, "run", side_effect=failure):
                try:
                    p.run(arguments)
                except p.ProvisioningError:
                    rendered = traceback.format_exc()
                    self.assertIn("infrastructure_command_failed:mc", rendered)
                    self.assertNotIn(secret, rendered)
                    self.assertNotIn("CalledProcessError", rendered)
                else:
                    self.fail("infrastructure failure was ignored")

    def test_mutable_or_injected_images_rejected(self):
        for image in ["registry/image:latest", "--privileged", "x@sha256:" + "a" * 63, "x;id@sha256:" + "a" * 64]:
            with self.assertRaises(p.ProvisioningError):
                p.immutable_image(image)

    def test_instances_have_distinct_networks_buckets_and_secret_roots(self):
        one = p.compose_manifest(self.config, self.instance, Path("/private/one"))
        other = copy.deepcopy(self.instance)
        other["id"] = str(uuid.uuid4())
        two = p.compose_manifest(self.config, other, Path("/private/two"))
        self.assertNotEqual(one["networks"]["sandbox"]["name"], two["networks"]["sandbox"]["name"])
        self.assertNotEqual(one["services"]["api"]["environment"]["COWORK_S3_BUCKET"], two["services"]["api"]["environment"]["COWORK_S3_BUCKET"])
        self.assertNotEqual(one["secrets"], two["secrets"])

    def test_application_services_have_no_host_mounts_or_published_ports(self):
        manifest = p.compose_manifest(self.config, self.instance, Path("/private/one"))
        for name in ["api", "worker", "egress-proxy"]:
            service = manifest["services"][name]
            self.assertNotIn("ports", service)
            self.assertNotIn("volumes", service)
            self.assertTrue(service["read_only"])
            self.assertEqual(service["cap_drop"], ["ALL"])
        self.assertTrue(manifest["networks"]["sandbox"]["internal"])

    def test_resource_limits_reject_bool_and_negative(self):
        for value in [-1, 0, True, "4000"]:
            instance = copy.deepcopy(self.instance)
            instance["policy"]["limits"]["cpu_millis"] = value
            with self.assertRaises(p.ProvisioningError):
                p.resource_limits(instance)

    def test_runner_enforces_policy_with_private_persistent_admission_state(self):
        manifest = p.compose_manifest(self.config, self.instance, Path("/private/one"))
        runner = manifest["services"]["runner"]
        self.assertEqual(runner["environment"]["COWORK_RUNNER_CPU_MILLIS"], "2000")
        self.assertEqual(runner["environment"]["COWORK_RUNNER_MEMORY_MIB"], "4096")
        self.assertEqual(runner["environment"]["COWORK_RUNNER_CONCURRENT_RUNS"], "2")
        self.assertIn("runner-admission:/var/lib/cowork/admission", runner["volumes"])
        self.assertEqual(manifest["volumes"]["runner-admission"]["name"], "cowork-" + p.identifier(self.instance["id"]) + "-runner-admission")
        for name in ["api", "worker", "egress-proxy"]:
            self.assertNotIn("volumes", manifest["services"][name])

    def test_network_pool_supports_forty_instances_without_overlap(self):
        occupied = ["172.17.0.0/16", "10.240.0.0/24"]
        for _ in range(40):
            networks = p.allocate_subnets("10.240.0.0/16", {"sandbox": 48, "control": 8, "internet": 8}, occupied)
            for subnet in networks.values():
                self.assertFalse(any(ipaddress.ip_network(subnet).overlaps(ipaddress.ip_network(old)) for old in occupied))
                occupied.append(subnet)
            self.assertGreaterEqual(ipaddress.ip_network(networks["sandbox"]).num_addresses - 2, 48)
        with self.assertRaisesRegex(p.ProvisioningError, "exhausted"):
            p.allocate_subnets("10.240.0.0/28", {"sandbox": 48}, [])
        with self.assertRaisesRegex(p.ProvisioningError, "private_ipv4"):
            p.allocate_subnets("8.8.0.0/16", {"sandbox": 48}, [])

    def test_stopping_removes_created_workloads_and_preserves_compose_services(self):
        with tempfile.TemporaryDirectory() as temp:
            infrastructure = p.Infrastructure({**self.config, "state_directory": temp})
            prefix = "cowork-" + p.identifier(self.instance["id"]) + "-"
            job = prefix + "job-" + uuid.uuid4().hex
            desktop = prefix + "desktop-" + uuid.uuid4().hex
            with patch.object(infrastructure, "compose") as compose, patch.object(p, "run", side_effect=[(desktop + "\n").encode(), b"", (job + "\n" + prefix + "runner-1\n").encode(), b"", b""]) as run:
                infrastructure.stop_compose(self.instance, Path(temp))
                compose.assert_called_once_with(self.instance, Path(temp), "stop")
                self.assertIn("--all", run.call_args_list[2].args[0])
                self.assertEqual(run.call_args_list[1].args[0], ["docker", "stop", "--time", "20", desktop])
                self.assertEqual(run.call_args_list[3].args[0], ["docker", "rm", "--force", job])

    def test_stopping_refuses_unknown_instance_containers(self):
        with tempfile.TemporaryDirectory() as temp:
            infrastructure = p.Infrastructure({**self.config, "state_directory": temp})
            name = "cowork-" + p.identifier(self.instance["id"]) + "-unrecognised\n"
            with patch.object(infrastructure, "compose"), patch.object(p, "run", return_value=name.encode()) as run:
                with self.assertRaisesRegex(p.ProvisioningError, "unexpected_instance_runtime_container"):
                    infrastructure.stop_compose(self.instance, Path(temp))
                self.assertEqual(run.call_count, 1)

    def test_egress_defaults_to_deny_and_only_allows_exact_https_hosts(self):
        self.assertNotIn("http_access allow", p.egress_config(self.instance))
        self.instance["policy"]["allowed_network_origins"] = ["https://example.com"]
        config = p.egress_config(self.instance)
        self.assertIn("dstdomain example.com\n", config)
        self.assertLess(config.index("http_access deny blocked_dst"), config.index("http_access allow permitted_domain"))
        self.assertIn("access_log none", config)
        for origin in ["https://*.example.com", "https://example.com/path", "https://x\nhttp_access allow all", "http://example.com", "https://example.com:8443"]:
            self.instance["policy"]["allowed_network_origins"] = [origin]
            with self.assertRaises((p.ProvisioningError, ValueError)):
                p.egress_config(self.instance)

    def test_secrets_are_stable_across_retries_and_differ_per_instance(self):
        with tempfile.TemporaryDirectory() as temp:
            config = {**self.config, "state_directory": temp, "database_endpoint": "postgres:5432"}
            infrastructure = p.Infrastructure(config)
            directory = infrastructure.directory(self.instance["id"])
            first = infrastructure.ensure_secrets(self.instance, directory)
            self.assertEqual(first, infrastructure.ensure_secrets(self.instance, directory))
            other = {**self.instance, "id": str(uuid.uuid4())}
            second = infrastructure.ensure_secrets(other, infrastructure.directory(other["id"]))
            self.assertNotEqual(first["database_password"], second["database_password"])
            self.assertNotEqual(first["storage_master_key"], second["storage_master_key"])

    def test_vm_requirement_never_falls_back_to_container(self):
        with tempfile.TemporaryDirectory() as temp:
            infrastructure = p.Infrastructure({**self.config, "state_directory": temp})
            self.instance["policy"]["minimum_isolation"] = "virtual_machine"
            with self.assertRaisesRegex(p.ProvisioningError, "vm_instance_runner_not_qualified"):
                infrastructure.execute({"id": str(uuid.uuid4()), "action": "provision", "instance": self.instance})

    def test_normal_restart_does_not_silently_upgrade_images(self):
        with tempfile.TemporaryDirectory() as temp:
            infrastructure = p.Infrastructure({**self.config, "state_directory": temp})
            directory = infrastructure.directory(self.instance["id"])
            infrastructure.write_compose(self.instance, directory)
            expected = infrastructure.deployed_images(directory)
            infrastructure.config["images"] = {name: image.replace("a" * 64, "b" * 64) for name, image in expected.items()}
            # Simulate an existing deployment from before the release marker.
            (directory / "images.json").unlink()
            infrastructure.write_compose(self.instance, directory)
            self.assertEqual(infrastructure.deployed_images(directory), expected)
            self.assertEqual(json.loads((directory / "images.json").read_text()), expected)

    def test_restore_failure_only_allows_restart_when_original_activation_is_unchanged(self):
        with tempfile.TemporaryDirectory() as temp:
            infrastructure = p.Infrastructure({**self.config, "state_directory": temp})
            directory = infrastructure.directory(self.instance["id"])
            manager = Mock(quiesced=True)
            operation = {"id": str(uuid.uuid4()), "action": "restore", "instance": {**self.instance, "state": "stopped"}}
            manager.execute.side_effect = p.BackupError("invalid_archive")
            with patch.object(p, "InstanceBackups", return_value=manager):
                with self.assertRaises(p.ProvisioningError) as raised:
                    infrastructure.execute(operation)
                self.assertEqual(raised.exception.instance_state, "stopped")
                manager.quiescent.assert_called_once()
                manager.quiescent.reset_mock()
                def ambiguous_activation(*_):
                    (directory / "active.json").write_text('{}')
                    raise p.ProvisioningError("activation_failed")
                manager.execute.side_effect = ambiguous_activation
                with self.assertRaises(p.ProvisioningError) as raised:
                    infrastructure.execute(operation)
                self.assertIsNone(raised.exception.instance_state)
                manager.quiescent.assert_not_called()

    def test_desktop_seccomp_is_explicit_and_host_checked(self):
        self.assertEqual(p.security_environment(self.config)["COWORK_SANDBOX_REQUIRE_APPARMOR"], "true")
        desktop = {**self.config, "sandbox_security_profile": "docker_desktop_seccomp"}
        self.assertEqual(p.security_environment(desktop), {"COWORK_SANDBOX_REQUIRE_APPARMOR": "false"})
        with patch.object(p, "run", return_value=b'{"OperatingSystem":"Docker Desktop","SecurityOptions":["name=seccomp,profile=builtin"]}'):
            p.verify_security_host(desktop)
        for info in [b'{"OperatingSystem":"Debian","SecurityOptions":["seccomp"]}', b'{"OperatingSystem":"Docker Desktop","SecurityOptions":[]}']:
            with patch.object(p, "run", return_value=info), self.assertRaises(p.ProvisioningError):
                p.verify_security_host(desktop)

    def test_federation_key_is_instance_scoped_and_never_contains_operator_key(self):
        with tempfile.TemporaryDirectory() as temp:
            token = Path(temp) / "token"
            token.write_text("test-operator-key" * 4)
            config = {**self.config, "state_directory": temp, "database_endpoint": "postgres:5432", "fleet_management_url": "https://management.test", "provisioner_token_file": str(token)}
            infra = p.Infrastructure(config)
            one = infra.ensure_secrets(self.instance, infra.directory(self.instance["id"]))
            other = {**self.instance, "id": str(uuid.uuid4())}
            two = infra.ensure_secrets(other, infra.directory(other["id"]))
            self.assertNotEqual(one["fleet_key"], two["fleet_key"])
            self.assertNotEqual(one["fleet_key"], token.read_text())
            manifest = p.compose_manifest(config, self.instance, infra.directory(self.instance["id"]))
            self.assertIn("fleet_key", manifest["services"]["api"]["secrets"])
            self.assertNotIn("fleet_key", manifest["services"]["worker"]["secrets"])
            self.assertNotIn("provisioner_token", str(manifest))


if __name__ == "__main__":
    unittest.main()
