import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
spec = importlib.util.spec_from_file_location("local_runner_deploy", HERE / "local_runner.py")
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


class LocalRunner(unittest.TestCase):
    def test_local_immutable_image_id_allowed_tags_refused(self):
        self.assertEqual(local.local_image("sha256:" + "a" * 64), "sha256:" + "a" * 64)
        with self.assertRaises(ValueError):
            local.local_image("repo:latest")

    def test_compose_only_runner_has_socket_and_ports_are_loopback(self):
        if not shutil.which("docker"):
            self.skipTest("Docker Compose parser unavailable")
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / "compose.env"
            env.write_text("\n".join(["COWORK_LOCAL_PROJECT=test-local", "COWORK_LOCAL_APP_IMAGE=registry.example/app@sha256:" + "a" * 64, "COWORK_LOCAL_RUNNER_IMAGE=registry.example/runner@sha256:" + "b" * 64, "COWORK_LOCAL_WORKSPACE_VOLUME=original-volume", "COWORK_LOCAL_DOCKER_GID=0"]))
            result = subprocess.run(["docker", "compose", "--env-file", str(env), "--file", str(HERE / "compose.local-runner.yml"), "config", "--format", "json"], text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            c = json.loads(result.stdout)
            for name, service in c["services"].items():
                self.assertTrue(all(p["host_ip"] == "127.0.0.1" for p in service["ports"]))
                sockets = [v for v in service["volumes"] if v.get("target") == "/var/run/docker.sock"]
                self.assertEqual(bool(sockets), name == "runner")
                self.assertFalse(service.get("privileged", False))
                self.assertEqual(service["tmpfs"], ["/tmp:size=256m,mode=1777"])
                broker = next(v for v in service["volumes"] if v["target"] == "/var/lib/cowork")
                self.assertEqual(broker["source"], "workspace")

    def test_init_uses_single_owned_volume_local_flag_and_no_guest_host_mount(self):
        calls = []
        def fake(args, **_):
            calls.append(args)
            output = ""
            if args[:3] == ["docker", "volume", "inspect"]:
                output = json.dumps([{"Name": args[3], "CreatedAt": "original", "Mountpoint": "/volumes/original", "Driver": "local", "Labels": {"owned": "true"}}])
            if "/usr/bin/stat" in args:
                output = "0\n"
            return subprocess.CompletedProcess(args, 0, output, "")
        with tempfile.TemporaryDirectory() as tmp, patch.object(local, "run", fake), patch.object(local, "engine", return_value="original-engine"):
            a = SimpleNamespace(root=Path(tmp) / "new", docker_context="desktop-linux", app_image="registry.example/app@sha256:" + "a"*64, runner_image="registry.example/runner@sha256:" + "b"*64, guest_image="registry.example/guest@sha256:" + "c"*64, app_port=43822, runner_port=9443)
            result = local.initialize(a)
            self.assertFalse(result["started"])
            init = next(x for x in calls if "--local-container-profile" in x)
            self.assertEqual(init[init.index("--local-container-profile") + 1], "true")
            self.assertIn("/var/lib/cowork/data/file-broker", init)
            self.assertIn("original-engine", init)
            for call in calls:
                for value in call:
                    if value.startswith("type=bind"):
                        self.assertIn("docker.sock", value)
            self.assertTrue((a.root / "ready.json").is_file())

    def test_changed_engine_or_volume_refused_before_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            data = {"format": local.FORMAT, "root": str(root.resolve()), "docker_context": "desktop-linux", "engine_id": "original", "volume": {"Name": "original", "CreatedAt": "original", "Mountpoint": "/original", "Driver": "local", "Labels": {"owner": "original"}}}
            (root / "deployment.json").write_text(json.dumps(data))
            with patch.object(local, "engine", return_value="replacement"):
                with self.assertRaisesRegex(ValueError, "Engine changed"):
                    local.load(root)
            replacement = subprocess.CompletedProcess([], 0, json.dumps([{**data["volume"], "CreatedAt": "replacement"}]), "")
            with patch.object(local, "engine", return_value="original"), patch.object(local, "run", return_value=replacement):
                with self.assertRaisesRegex(ValueError, "volume changed"):
                    local.load(root)

    def test_remote_context_refused(self):
        with patch.object(local, "run", return_value=subprocess.CompletedProcess([], 0, '"tcp://remote.example:2376"', "")):
            with self.assertRaisesRegex(ValueError, "local Unix socket"):
                local.engine()


if __name__ == "__main__":
    unittest.main()
