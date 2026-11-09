"""Boundary and ownership regressions for the new topology; no live cluster."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("agent_sandbox_deploy", HERE / "agent_sandbox.py")
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


def config():
    c = json.loads((HERE / "agent-sandbox.example.json").read_text())
    for key in ("app_image", "runner_image", "init_image", "sandbox_image"):
        c[key] = "registry.example/" + key.replace("_", "-") + "@sha256:" + "a" * 64
    c["controller_image"] = deploy.CONTROLLER_IMAGE
    c["storage_class"], c["data_storage_class"] = "csi-block", "csi-shared"
    c["runner_label"] = "d" * 32
    c["namespace_uids"] = {c[k]: "bf426e15-38d3-4f03-a5e5-c4917aa919d5" for k in ("app_namespace", "controller_namespace", "tenant_namespace")}
    return c


class Fake:
    def __init__(self, c, foreign=False, uid_changed=False):
        self.c, self.foreign, self.uid_changed = c, foreign, uid_changed
        self.calls = []

    def __call__(self, argv, **kwargs):
        self.calls.append((argv, kwargs.get("input")))
        args = argv[3:]
        result = None
        if args[:2] == ["get", "namespace"]:
            result = {"metadata": {"uid": "f0a0fb17-b9e5-4a7e-8148-ce324461cf76" if self.uid_changed else self.c["namespace_uids"][args[2]], "labels": {deploy.OWNER: self.c["deployment_id"]}}}
        elif args[0] == "get":
            if args[1] == "sandboxes":
                result = {"items": [{"metadata": {"name": "original-sandbox", "uid": "original-sandbox-uid"}}]}
            elif self.foreign:
                result = {"metadata": {"labels": {deploy.OWNER: "foreign"}}}
            elif args[1] == "deployment":
                result = {"metadata": {"labels": {deploy.OWNER: self.c["deployment_id"]}}}
        return subprocess.CompletedProcess(argv, 0, "" if result is None else json.dumps(result), "")


class Topology(unittest.TestCase):
    def setUp(self):
        self.c = config()
        self.rendered = deploy.render(self.c)

    def test_no_socket_hostpaths_credentials_or_api_token_in_guests(self):
        all_resources = [x for value in self.rendered.values() if isinstance(value, dict) for x in value.get("items", [])]
        for item in all_resources:
            raw = json.dumps(item)
            self.assertNotIn("docker.sock", raw)
            self.assertNotIn('"hostPath"', raw)
        for item in self.rendered["application"]["items"]:
            if item["kind"] in {"Deployment", "StatefulSet"}:
                pod = item["spec"]["template"]["spec"]
                self.assertFalse(pod["automountServiceAccountToken"])
                api_projection = any(v.get("projected") for v in pod["volumes"])
                self.assertEqual(api_projection, item["kind"] == "StatefulSet")
                self.assertTrue(pod["securityContext"]["runAsNonRoot"])
                self.assertNotIn("fsGroup", pod["securityContext"], "A later shared-PVC mount must not widen private credential modes")
                self.assertTrue(all(not c["securityContext"].get("allowPrivilegeEscalation", True) for c in pod["containers"] + pod["initContainers"]))

    def test_quota_default_deny_and_cross_namespace_runner_selector(self):
        resources = self.rendered["tenant"]["items"]
        quota = next(x for x in resources if x["kind"] == "ResourceQuota")
        self.assertEqual(quota["spec"]["hard"]["count/sandboxes.agents.x-k8s.io"], "20")
        deny = next(x for x in resources if x["metadata"]["name"] == "default-deny")
        self.assertEqual(deny["spec"]["egress"], [])
        self.assertEqual(self.rendered["adapter-settings"]["agent_sandbox"]["approved_images"], [self.c["sandbox_image"]])
        # Only the adapter creates per-attempt allow policies. A shared ingress
        # policy would invalidate its overlap verifier and could expand access.
        self.assertEqual(len([x for x in resources if x["kind"] == "NetworkPolicy"]), 1)
        runner = next(x for x in self.rendered["application"]["items"] if x["metadata"]["name"] == "runner-worker")
        peer = runner["spec"]["egress"][-1]["to"][0]
        self.assertEqual(peer["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"], self.c["tenant_namespace"])
        self.assertEqual(peer["podSelector"]["matchLabels"]["cowork.local/runner"], self.c["runner_label"])

    def test_controller_two_replicas_elects_and_never_has_pvc_delete_authority(self):
        ctrl = next(x for x in self.rendered["controller"]["items"] if x["kind"] == "Deployment")
        self.assertEqual(ctrl["spec"]["replicas"], 2)
        self.assertIn("--leader-elect=true", ctrl["spec"]["template"]["spec"]["containers"][0]["args"])
        self.assertNotIn("ClusterRole", [x["kind"] for x in self.rendered["application"]["items"]])
        for x in self.rendered["tenant"]["items"]:
            if x["kind"] == "Role":
                for r in x["rules"]:
                    if "persistentvolumeclaims" in r["resources"]:
                        self.assertNotIn("delete", r["verbs"])
                    self.assertNotIn("secrets", r["resources"])
                    self.assertNotIn("*", r["resources"] + r["verbs"])

    def test_claims_are_app_owned_retained_and_runner_is_single_writer(self):
        for x in self.rendered["application"]["items"]:
            if x["kind"] == "PersistentVolumeClaim":
                self.assertNotIn("ownerReferences", x["metadata"])
                self.assertNotIn("finalizers", x["metadata"])
            if x["kind"] == "StatefulSet":
                self.assertEqual(x["spec"]["replicas"], 1)
        source = (HERE / "agent_sandbox.py").read_text()
        self.assertNotIn('"delete", "pvc"', source)
        self.assertNotIn('"delete", "namespace"', source)

    def test_runner_and_app_share_the_actual_broker_filesystem(self):
        items = self.rendered["application"]["items"]
        runner = next(x for x in items if x["kind"] == "StatefulSet")["spec"]["template"]["spec"]
        broker = next(x for x in runner["volumes"] if x["name"] == "broker")
        self.assertEqual(broker["persistentVolumeClaim"]["claimName"], "cowork-data")
        mount = next(x for x in runner["containers"][0]["volumeMounts"] if x["name"] == "broker")
        self.assertEqual(mount["mountPath"], "/var/lib/cowork/file-broker")
        self.assertEqual(mount["subPath"], "file-broker")
        self.assertEqual(self.rendered["runner-paths"]["workspace_state_directory"], mount["mountPath"])

    def test_broker_consumers_share_one_kernel_identity_domain(self):
        expected = {"cowork.local/broker-group": self.c["deployment_id"]}
        for item in self.rendered["application"]["items"]:
            if item["kind"] not in {"Deployment", "StatefulSet"}:
                continue
            template = item["spec"]["template"]
            self.assertEqual(template["metadata"]["labels"]["cowork.local/broker-group"], self.c["deployment_id"])
            terms = template["spec"]["affinity"]["podAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]
            self.assertEqual(terms, [{"labelSelector": {"matchLabels": expected}, "topologyKey": "kubernetes.io/hostname"}])

    def test_rejects_unpinned_images_broad_cidrs_and_wrong_namespaces(self):
        for change in ({"sandbox_image": "repo:latest"}, {"api_cidrs": ["0.0.0.0/0"]}, {"tenant_namespace": self.c["app_namespace"]}, {"runner_label": "a" * 64}, {"runtime_handler": "runc"}, {"unexpected": "field"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                deploy.render({**self.c, **change})

    def test_foreign_resource_refused_before_apply(self):
        fake = Fake(self.c, foreign=True)
        with self.assertRaisesRegex(ValueError, "different installation"):
            deploy.Cluster(self.c, fake).apply("application")
        self.assertFalse(any("apply" in call[0] for call in fake.calls))

    def test_changed_namespace_uid_refused_before_mutation(self):
        fake = Fake(self.c, uid_changed=True)
        with self.assertRaisesRegex(ValueError, "ownership changed"):
            deploy.Cluster(self.c, fake).stop()
        self.assertFalse(any("scale" in call[0] or "patch" in call[0] for call in fake.calls))

    def test_stop_pins_sandbox_uid_preserves_pvcs_and_keeps_uncertainty(self):
        fake = Fake(self.c)
        result = deploy.Cluster(self.c, fake).stop()
        self.assertFalse(result["physical_stop_confirmed"])
        self.assertFalse(result["pvc_deleted"])
        self.assertFalse(any("delete" in call[0] for call in fake.calls))
        patch = next(call[0] for call in fake.calls if "patch" in call[0])
        operation = json.loads(patch[patch.index("-p") + 1])
        self.assertEqual(operation[0], {"op": "test", "path": "/metadata/uid", "value": "original-sandbox-uid"})

    def test_cluster_policy_apply_requires_separate_administrator(self):
        fake = Fake(self.c)
        with self.assertRaisesRegex(ValueError, "separate cluster administrator"):
            deploy.Cluster(self.c, fake).apply("cluster-policy")
        self.assertEqual(fake.calls, [])

    def test_windows_vm_halted_in_separate_namespace_with_retained_boot_volume(self):
        c = copy.deepcopy(self.c)
        c["windows"] = {"namespace": "cowork-windows", "vm_name": "windows-original", "boot_pvc": "windows-original-boot", "cpu": 2, "memory_gib": 4, "agent_config_secret": "windows-agent"}
        output = deploy.render(c)
        vm = output["windows"]["items"][0]
        self.assertEqual(vm["metadata"]["namespace"], "cowork-windows")
        self.assertEqual(vm["spec"]["runStrategy"], "Halted")
        self.assertNotIn("dataVolumeTemplates", vm["spec"])
        c["windows"]["namespace"] = c["tenant_namespace"]
        with self.assertRaises(ValueError):
            deploy.render(c)

    def test_preview_different_site_no_sandboxd_access_and_exact_runner_binding(self):
        c = copy.deepcopy(self.c)
        c["preview"] = {"namespace": "cowork-previews", "image": "registry.example/preview@sha256:" + "e" * 64, "domain": "previews.example.net", "tls_secret": "preview-tls", "secret": "preview-custody", "runner_id": "runner-original", "runner_generation": 1, "runner_public_key": [1] * 32, "allowed_ports": [8081]}
        r = deploy.render(c)
        gateway = next(x for x in r["preview"]["items"] if x["kind"] == "Deployment")
        self.assertFalse(gateway["spec"]["template"]["spec"]["automountServiceAccountToken"])
        binding = r["adapter-settings"]["agent_sandbox"]["preview_gateway"]
        self.assertEqual(binding["namespace"], "cowork-previews")
        self.assertEqual(binding["ports"], [8081])
        runner_policy = next(x for x in r["application"]["items"] if x["metadata"]["name"] == "runner-worker")
        self.assertEqual([x["port"] for x in runner_policy["spec"]["egress"][-1]["ports"]], [8080, 9090, 8081], "Trusted issuer must reach the exact admitted preview before signing")
        c["preview"]["allowed_ports"] = [8080]
        with self.assertRaises(ValueError):
            deploy.render(c)
        c["preview"]["allowed_ports"] = [8081]
        c["preview"]["domain"] = "previews.example.com"
        with self.assertRaisesRegex(ValueError, "different site"):
            deploy.render(c)

    def test_proxy_has_bounded_domains_private_network_denial_and_no_guest_egress_by_default(self):
        c = copy.deepcopy(self.c)
        c["web_proxy"] = {"namespace": "cowork-egress", "image": "registry.example/proxy@sha256:" + "e" * 64, "policy_id": "public-web", "allowed_domains": ["example.com", ".github.com"], "probe_url": "http://example.com/", "direct_probe_address": "93.184.215.14:80"}
        r = deploy.render(c)
        proxy = next(x for x in r["egress-proxy"]["items"] if x["kind"] == "ConfigMap")
        self.assertIn("http_access deny private_dst", proxy["data"]["squid.conf"])
        self.assertIn("http_access deny all", proxy["data"]["squid.conf"])
        binding = r["adapter-settings"]["agent_sandbox"]["web_proxy_policies"]["public-web"]
        self.assertEqual(binding["port"], 3128)
        self.assertEqual(binding["namespace"], "cowork-egress")
        self.assertTrue(all(x["network"] == "denied" for x in r["profiles"]["profiles"].values()))
        c["web_proxy"]["allowed_domains"] = ["example.com\nhttp_access allow all"]
        with self.assertRaises(ValueError):
            deploy.render(c)


if __name__ == "__main__":
    unittest.main()
