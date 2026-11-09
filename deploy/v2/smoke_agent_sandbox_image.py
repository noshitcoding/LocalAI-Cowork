"""Opt-in sandboxd wire smoke in a fresh, disposable local Docker fixture.

Requires grpcio. This verifies the built runtime image, never Kata/CNI isolation.
Only a newly created named volume is mounted; both listeners bind to loopback.
"""
import argparse
import hashlib
import json
from pathlib import Path
import secrets
import subprocess
import time
import urllib.request
import uuid

import grpc
from local_runner import local_image


def varint(n):
    result = bytearray()
    while n > 127:
        result.append((n & 127) | 128)
        n >>= 7
    return bytes(result) + bytes([n])


def field(tag, value):
    value = value.encode() if isinstance(value, str) else value
    return varint(tag * 8 + 2) + varint(len(value)) + value


def events(data):
    def number(offset):
        n = shift = 0
        while True:
            value = data[offset]; offset += 1
            n |= (value & 127) << shift
            if value < 128:
                return n, offset
            shift += 7
            assert shift < 64
    offset = 0
    while offset < len(data):
        tag, offset = number(offset)
        assert tag & 7 == 2, "Unexpected sandboxd wire field"
        size, offset = number(offset)
        assert size <= len(data) - offset
        yield tag >> 3, data[offset:offset + size]
        offset += size


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--image", required=True)
    p.add_argument("--report", type=Path, required=True)
    p.add_argument("--docker-context", required=True)
    a = p.parse_args()
    local_image(a.image)
    identifier = uuid.uuid4().hex
    name, volume = "cowork-sandboxd-smoke-" + identifier, "cowork-sandboxd-smoke-" + identifier
    label = "cowork.local/validation-id=" + identifier

    def docker(*argv):
        r = subprocess.run(["docker", "--context", a.docker_context, *argv], capture_output=True, text=True, timeout=120)
        if r.returncode:
            raise RuntimeError("Disposable Docker smoke operation failed: " + " ".join(argv[:2]))
        return r.stdout.strip()

    endpoint = json.loads(docker("context", "inspect", a.docker_context, "--format", "{{json .Endpoints.docker.Host}}"))
    assert endpoint.startswith(("npipe://", "unix://")), "Local Docker context required"
    observed = json.loads(docker("image", "inspect", a.image))[0]
    image_id = observed["Id"]
    token = secrets.token_hex(32)
    token_hash = hashlib.sha256(token.encode()).hexdigest()
    created_volume = created_container = False
    report = {"scope": "local-sandboxd-wire-only", "image_id": image_id, "kata_qualified": False, "checks": {}}
    try:
        docker("volume", "create", "--label", label, volume); created_volume = True
        docker("run", "--rm", "--network", "none", "--user", "0:0", "--mount", f"type=volume,source={volume},target=/workspace", "--entrypoint", "/bin/sh", image_id, "-ec", "chown 1000:1000 /workspace; chmod 700 /workspace")
        docker("run", "-d", "--name", name, "--label", label, "--runtime", "runc", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--memory", "512m", "--cpus", "1", "--pids-limit", "64", "--mount", f"type=volume,source={volume},target=/workspace", "--tmpfs", "/tmp:rw,nosuid,nodev,size=64m", "-p", "127.0.0.1::8080", "-p", "127.0.0.1::9090", image_id, "--root-dir=/workspace", "--auth-token-sha256=" + token_hash)
        created_container = True
        state = json.loads(docker("inspect", name))[0]
        assert state["HostConfig"]["Binds"] is None
        ports = state["NetworkSettings"]["Ports"]
        rest = "http://127.0.0.1:" + ports["8080/tcp"][0]["HostPort"]
        rpc = "127.0.0.1:" + ports["9090/tcp"][0]["HostPort"]
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + 30
        while True:
            try:
                with opener.open(rest + "/v1/health", timeout=2) as response:
                    assert response.status == 200
                break
            except OSError:
                assert time.monotonic() < deadline, "sandboxd health timeout"
                time.sleep(.2)
        headers = {"Authorization": "Bearer " + token, "Content-Type": "application/octet-stream"}
        marker = b"sandboxd-image-smoke\n"
        encoded_path = "nested/space%20%C3%A4.txt"
        req = urllib.request.Request(rest + "/v1/files/" + encoded_path + "?mode=0644", data=marker, headers=headers, method="PUT")
        with opener.open(req, timeout=5) as response:
            assert response.status in (200, 201, 204)
        req = urllib.request.Request(rest + "/v1/files/" + encoded_path, headers=headers)
        with opener.open(req, timeout=5) as response:
            assert response.read(100) == marker
        request = field(1, b"".join(field(1, v) for v in ["/bin/sh", "-c", "cat 'nested/space ä.txt'; printf start-ok"]) + field(3, "/workspace"))
        output, event_tags = bytearray(), []
        with grpc.insecure_channel(rpc) as channel:
            start = channel.unary_stream("/process.v1.ProcessService/Start")
            for raw in start(request, metadata=[("authorization", "Bearer " + token)], timeout=15):
                for tag, payload in events(raw):
                    event_tags.append(tag)
                    if tag == 2:
                        output.extend(payload)
                    if tag == 4:
                        assert payload in (b"", b"\x08\x00"), "sandboxd process failed"
        assert event_tags[0] == 1 and event_tags[-1] == 4
        assert bytes(output) == marker + b"start-ok"
        report["checks"] = {"health": True, "file_put_get": True, "grpc_start_init_stdout_exit": True, "nonroot_readonly_guest": True, "no_host_mounts": True}
        report["passed"] = True
    finally:
        if created_container:
            item = json.loads(docker("inspect", name))[0]
            assert item["Config"]["Labels"]["cowork.local/validation-id"] == identifier
            docker("rm", "-f", name)
        if created_volume:
            item = json.loads(docker("volume", "inspect", volume))[0]
            assert item["Labels"]["cowork.local/validation-id"] == identifier
            docker("volume", "rm", volume)
        report["cleanup_completed"] = True
        a.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": True, "image_id": image_id, "report": str(a.report)}))


if __name__ == "__main__":
    main()
