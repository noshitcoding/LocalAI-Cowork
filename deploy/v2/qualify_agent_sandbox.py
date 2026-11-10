"""Disposable Core lifecycle/storage qualification, explicitly NOT Kata isolation.

Requires a NEW separately created validation cluster, pinned v1.0.7 controller,
and preloaded harmless BusyBox image. Every call uses the selected kubeconfig
and context. No existing application namespace, database or volume is adopted.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time
import uuid
from agent_sandbox import API, OWNER, CONTROLLER_IMAGE, require


class Fixture:
    def __init__(self, a):
        require(a.context.startswith("cowork-sandbox-validation-"), "Select an isolated validation cluster")
        self.a = a
        self.id = str(uuid.uuid4())
        self.ns = "qualification-" + self.id[:8]
        self.base = ["kubectl", "--kubeconfig", str(a.kubeconfig), "--context", a.context]
        self.objects = {}
        self.checks = []

    def call(self, *args, data=None):
        r = subprocess.run([*self.base, *args], input=json.dumps(data) if data else None, text=True, capture_output=True, timeout=120)
        require(r.returncode == 0, "Validation kubectl failed: " + r.stderr[:2000])
        return json.loads(r.stdout) if r.stdout.lstrip().startswith(("{", "[")) else r.stdout.strip()

    def create(self, item):
        result = self.call("create", "-f", "-", "-o", "json", data=item)
        self.objects[item["kind"]] = result["metadata"]["uid"]
        return result

    def get(self, kind, name):
        return self.call("get", kind, name, "-n", self.ns, "-o", "json")

    def wait_ready(self):
        self.call("wait", "sandbox/original", "-n", self.ns, "--for=condition=Ready", "--timeout=90s")
        pods = self.call("get", "pods", "-n", self.ns, "-o", "json")["items"]
        require(len(pods) == 1, "Expected exactly one original workload")
        self.checks.append({"check": "sandbox_ready", "sandbox_uid": self.objects["Sandbox"], "pod_uid": pods[0]["metadata"]["uid"]})
        return pods[0]

    def patch_mode(self, mode):
        self.call("patch", "sandbox", "original", "-n", self.ns, "--type=json", "-p", json.dumps([{"op": "test", "path": "/metadata/uid", "value": self.objects["Sandbox"]}, {"op": "replace", "path": "/spec/operatingMode", "value": mode}]))

    def execute(self):
        deployment = self.call("get", "deployment", "agent-sandbox-controller", "-n", "agent-sandbox-system", "-o", "json")
        require(deployment["spec"]["template"]["spec"]["containers"][0]["image"] == CONTROLLER_IMAGE, "Pinned v1.0.7 controller required")
        self.create({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": self.ns, "labels": {OWNER: self.id}}})
        self.create({"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": "original-data", "namespace": self.ns, "labels": {OWNER: self.id}}, "spec": {"accessModes": ["ReadWriteOnce"], "storageClassName": self.a.storage_class, "resources": {"requests": {"storage": "100Mi"}}}})
        sandbox = {"apiVersion": API, "kind": "Sandbox", "metadata": {"name": "original", "namespace": self.ns, "labels": {OWNER: self.id}}, "spec": {"service": False, "operatingMode": "Running", "podTemplate": {"metadata": {"labels": {OWNER: self.id}}, "spec": {"automountServiceAccountToken": False, "restartPolicy": "Never", "securityContext": {"runAsUser": 1000, "runAsGroup": 1000, "fsGroup": 1000}, "containers": [{"name": "test", "image": self.a.image, "imagePullPolicy": "Never", "command": ["/bin/sh", "-ec", "sleep 3600"], "volumeMounts": [{"name": "data", "mountPath": "/workspace"}], "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "32Mi"}}}], "volumes": [{"name": "data", "persistentVolumeClaim": {"claimName": "original-data"}}]}}}}
        self.create(sandbox)
        original_pod = self.wait_ready()
        marker = "original-" + self.id
        self.call("exec", "-n", self.ns, original_pod["metadata"]["name"], "--", "/bin/sh", "-ec", "printf '%s' '" + marker + "' > /workspace/marker; sync")
        self.patch_mode("Suspended")
        self.call("wait", "sandbox/original", "-n", self.ns, "--for=condition=Suspended", "--timeout=90s")
        require(self.get("pvc", "original-data")["metadata"]["uid"] == self.objects["PersistentVolumeClaim"], "PVC identity changed after suspension")
        self.patch_mode("Running")
        resumed = self.wait_ready()
        require(resumed["metadata"]["uid"] != original_pod["metadata"]["uid"], "Suspension did not replace the original Pod")
        found = self.call("exec", "-n", self.ns, resumed["metadata"]["name"], "--", "cat", "/workspace/marker")
        require(found == marker, "Workspace bytes did not survive suspend/resume")
        self.checks.append({"check": "workspace_survives_new_pod", "passed": True, "pvc_uid": self.objects["PersistentVolumeClaim"]})
        # Wrong identity must fail the UID test before any mode mutation.
        wrong = subprocess.run([*self.base, "patch", "sandbox", "original", "-n", self.ns, "--type=json", "-p", json.dumps([{"op": "test", "path": "/metadata/uid", "value": str(uuid.uuid4())}, {"op": "replace", "path": "/spec/operatingMode", "value": "Suspended"}])], text=True, capture_output=True, timeout=30)
        require(wrong.returncode != 0 and self.get("sandbox", "original")["spec"]["operatingMode"] == "Running", "Wrong-UID mutation was accepted")
        self.checks.append({"check": "wrong_uid_mutation_refused", "passed": True})
        self.patch_mode("Suspended")
        self.call("wait", "sandbox/original", "-n", self.ns, "--for=condition=Suspended", "--timeout=90s")
        self.call("delete", "sandbox", "original", "-n", self.ns, "--wait=true", "--timeout=60s")
        retained = self.get("pvc", "original-data")
        require(retained["metadata"]["uid"] == self.objects["PersistentVolumeClaim"] and not retained["metadata"].get("ownerReferences"), "Application-owned PVC was removed or adopted")
        self.checks.append({"check": "sandbox_delete_retains_app_owned_pvc", "passed": True})

    def cleanup(self):
        if "Namespace" not in self.objects:
            return
        namespace = self.call("get", "namespace", self.ns, "-o", "json")
        require(namespace["metadata"]["uid"] == self.objects["Namespace"] and namespace["metadata"]["labels"].get(OWNER) == self.id, "Refusing cleanup after namespace replacement")
        # The entire UUID-owned namespace is disposable test data, independent
        # from the production lifecycle which never deletes workspace PVCs.
        self.call("delete", "namespace", self.ns, "--wait=true", "--timeout=90s")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--context", required=True)
    p.add_argument("--kubeconfig", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--image", default="busybox:1.37.0")
    p.add_argument("--storage-class", default="standard")
    a = p.parse_args()
    require(a.output.is_absolute() and not a.output.exists(), "Write a new absolute result file")
    f = Fixture(a)
    result = {"upstream_version": "v1.0.7", "controller_image": CONTROLLER_IMAGE, "context": a.context, "namespace": f.ns, "scope": "core-lifecycle-storage-only", "kata_qualified": False, "cni_isolation_qualified": False, "production_qualified": False}
    started = time.monotonic()
    try:
        f.execute()
        result["passed"] = True
    except Exception as e:
        result.update(passed=False, error=str(e))
    finally:
        try:
            f.cleanup()
            result["cleanup_confirmed"] = True
        except Exception as e:
            result.update(cleanup_confirmed=False, cleanup_error=str(e))
        result.update(checks=f.checks, duration_seconds=round(time.monotonic()-started, 3), source_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
        a.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    require(result["passed"] and result["cleanup_confirmed"], "Scoped qualification failed; original report retained")


if __name__ == "__main__":
    main()
