"""Explicit localhost-only cohosted daemon/runner setup; never remaps host grants.

The app and trusted runner share one exact Linux broker filesystem and OS locks.
Only the trusted runner mounts Docker's administrative socket. Guest processes
receive copied workspace bytes through the existing signed execution protocol.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid
from agent_sandbox import image, require

HERE = Path(__file__).resolve().parent
FORMAT = "cowork-local-container/1"
ROOT = "/var/lib/cowork"
_DOCKER_CONTEXT = None


def local_image(value):
    if isinstance(value, str) and re.fullmatch(r"sha256:[a-f0-9]{64}", value):
        return value
    return image(value)


def run(args, *, check=True):
    if args[0] == "docker" and _DOCKER_CONTEXT and args[1] != "--context":
        args = [args[0], "--context", _DOCKER_CONTEXT, *args[1:]]
    p = subprocess.run(args, text=True, capture_output=True, timeout=180)
    require(not check or p.returncode == 0, "Local Docker operation failed; private command output is withheld")
    return p


def engine():
    endpoint = json.loads(run(["docker", "context", "inspect", _DOCKER_CONTEXT, "--format", "{{json .Endpoints.docker.Host}}"]).stdout)
    require(endpoint.startswith(("unix://", "npipe://")), "Local profile requires a local Unix socket or Windows named-pipe Docker context")
    value = json.loads(run(["docker", "info", "--format", "{{json .}}"]).stdout)
    require(value.get("OSType") == "linux" and value.get("ID"), "Select the local Linux Docker Engine")
    return value["ID"]


def private_new(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
    if os.name == "posix":
        path.chmod(0o600)


def compose(root, c, *args):
    return run(["docker", "compose", "--project-name", c["project"], "--env-file", str(root / "compose.env"), "--file", str(HERE / "compose.local-runner.yml"), *args])


def load(root):
    global _DOCKER_CONTEXT
    require(root.is_absolute() and not root.is_symlink(), "Select an absolute owned local setup directory")
    c = json.loads((root / "deployment.json").read_text())
    _DOCKER_CONTEXT = c["docker_context"]
    require(c["format"] == FORMAT and str(root.resolve()) == c["root"], "Local setup root identity changed")
    require(c["engine_id"] == engine(), "Local Docker Engine changed; explicit reconciliation required")
    observed = json.loads(run(["docker", "volume", "inspect", c["volume"]["Name"]]).stdout)[0]
    require(all(observed.get(k) == c["volume"].get(k) for k in ("Name", "CreatedAt", "Mountpoint", "Driver", "Labels")), "Original workspace volume changed")
    return c


def initialize(a):
    global _DOCKER_CONTEXT
    _DOCKER_CONTEXT = a.docker_context or run(["docker", "context", "show"]).stdout.strip()
    root = a.root
    require(root.is_absolute() and not root.exists(), "Initialize a new absolute local setup directory")
    local_image(a.app_image); local_image(a.runner_image); local_image(a.guest_image)
    require(1024 <= a.app_port <= 65535 and 1024 <= a.runner_port <= 65535 and a.app_port != a.runner_port, "Use distinct unprivileged localhost ports")
    identifier = str(uuid.uuid4()); volume = "cowork-local-" + identifier
    root.mkdir(parents=True, mode=0o700)
    c = {"format": FORMAT, "id": identifier, "root": str(root.resolve()), "docker_context": _DOCKER_CONTEXT, "engine_id": engine(), "project": "cowork-local-" + identifier[:8], "app_image": a.app_image, "runner_image": a.runner_image, "guest_image": a.guest_image, "app_port": a.app_port, "runner_port": a.runner_port, "source_compose_sha256": hashlib.sha256((HERE / "compose.local-runner.yml").read_bytes()).hexdigest()}
    run(["docker", "volume", "create", "--label", "cowork.local/deployment-id=" + identifier, "--label", "cowork.local/purpose=single-user-local-broker", volume])
    c["volume"] = json.loads(run(["docker", "volume", "inspect", volume]).stdout)[0]
    # Persist original volume custody before further I/O. Failure is retained and
    # never silently reinitializes or removes an already created volume.
    private_new(root / "deployment.json", c)
    mount = "type=volume,source=" + volume + ",target=" + ROOT
    base = ["docker", "run", "--rm", "--network", "none", "--mount", mount]
    run([*base, "--user", "0:0", "--entrypoint", "/bin/sh", a.runner_image, "-ec", "mkdir -p /var/lib/cowork/data/file-broker/workspaces /var/lib/cowork/data/file-broker/locks /var/lib/cowork/tls; chown -R 1000:1000 /var/lib/cowork; chmod 700 /var/lib/cowork /var/lib/cowork/data /var/lib/cowork/data/file-broker /var/lib/cowork/data/file-broker/workspaces /var/lib/cowork/data/file-broker/locks /var/lib/cowork/tls"])
    run([*base, "--user", "1000:1000", "--entrypoint", "/usr/bin/openssl", a.runner_image, "req", "-x509", "-newkey", "ed25519", "-noenc", "-days", "365", "-keyout", ROOT + "/tls/server.key", "-out", ROOT + "/tls/server.crt", "-subj", "/CN=runner", "-addext", "subjectAltName=DNS:runner,DNS:localhost,IP:127.0.0.1"])
    run([*base, "--user", "1000:1000", "--entrypoint", "/bin/sh", a.runner_image, "-ec", "chmod 600 /var/lib/cowork/tls/server.key /var/lib/cowork/tls/server.crt"])
    run([*base, "--user", "1000:1000", a.runner_image, "init", "--data", ROOT + "/runner", "--workspace-state", ROOT + "/data/file-broker", "--tls-cert", ROOT + "/tls/server.crt", "--tls-key", ROOT + "/tls/server.key", "--probe", "/usr/local/bin/cowork-runner-probe", "--listen", "0.0.0.0:8443", "--public-origin", "https://runner:8443", "--docker", "/usr/local/bin/docker", "--local-container-profile", "true", "--local-engine-id", c["engine_id"]])
    # Query only the socket's numeric GID in a trusted temporary container.
    gid = run(["docker", "run", "--rm", "--network", "none", "--mount", "type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock,readonly", "--entrypoint", "/usr/bin/stat", a.runner_image, "-c", "%g", "/var/run/docker.sock"]).stdout.strip()
    require(re.fullmatch(r"[0-9]{1,10}", gid), "Docker socket group observation failed")
    entries = {"COWORK_LOCAL_PROJECT": c["project"], "COWORK_LOCAL_APP_IMAGE": a.app_image, "COWORK_LOCAL_RUNNER_IMAGE": a.runner_image, "COWORK_LOCAL_WORKSPACE_VOLUME": volume, "COWORK_LOCAL_DOCKER_GID": gid, "COWORK_LOCAL_APP_PORT": str(a.app_port), "COWORK_LOCAL_RUNNER_PORT": str(a.runner_port)}
    require(all(not any(ch in v for ch in "'\r\n") for v in entries.values()), "Invalid Compose setting")
    with (root / "compose.env").open("x", encoding="utf-8") as stream:
        stream.write("".join(k + "='" + v + "'\n" for k, v in entries.items()))
    if os.name == "posix":
        (root / "compose.env").chmod(0o600)
    private_new(root / "ready.json", {"format": FORMAT, "id": identifier, "engine_id": c["engine_id"], "native_folder_mapping": False, "storage_limit": "soft_monitored", "production_tenant_isolation": False})
    return {"root": str(root), "api": f"http://127.0.0.1:{a.app_port}", "started": False, "storage_limit": "soft_monitored"}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("command", choices=["init", "start", "stop", "inspect"])
    p.add_argument("--root", required=True, type=Path)
    for key in ("app-image", "runner-image", "guest-image"):
        p.add_argument("--" + key)
    p.add_argument("--app-port", type=int, default=43822)
    p.add_argument("--runner-port", type=int, default=9443)
    p.add_argument("--docker-context")
    a = p.parse_args()
    if a.command == "init":
        print(json.dumps(initialize(a)))
        return
    c = load(a.root)
    require((a.root / "ready.json").is_file() and c["source_compose_sha256"] == hashlib.sha256((HERE / "compose.local-runner.yml").read_bytes()).hexdigest(), "Incomplete setup or changed Compose source requires operator review")
    if a.command == "inspect":
        print(json.dumps({"id": c["id"], "engine_id": c["engine_id"], "volume": c["volume"]["Name"], "api": f"http://127.0.0.1:{c['app_port']}", "storage_limit": "soft_monitored", "native_folder_mapping": False}))
    else:
        compose(a.root, c, "up", "-d") if a.command == "start" else compose(a.root, c, "stop", "--timeout", "120")
        print(json.dumps({"operation": a.command, "workspace_retained": True}))


if __name__ == "__main__":
    main()
