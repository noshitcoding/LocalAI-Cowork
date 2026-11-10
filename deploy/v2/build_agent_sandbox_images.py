"""Explicit Docker BuildKit builds; never installs or deploys a cluster."""
import argparse
from pathlib import Path
import subprocess
import re
from agent_sandbox import image, require

# The v1.0.7 annotated tag resolves to this commit. BuildKit checks an annotated
# tag's object ID rather than peeling it, so select and verify the commit itself.
UPSTREAM = "https://github.com/kubernetes-sigs/agent-sandbox.git?ref=807cc37107bd7c012115afac9fa9c3b77b8365c2&checksum=807cc37107bd7c012115afac9fa9c3b77b8365c2"


def arguments(a):
    for value in (a.go_image, a.rust_image, a.node_image, a.runner_base_image):
        image(value)
    require(a.tag and a.tag.count(":") >= 1 and "\n" not in a.tag, "Explicit local image tag required")
    require(re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}", a.target_cache_id), "Invalid build target cache identifier")
    return ["docker", "buildx", "build", "--load", "--target", a.target, "--tag", a.tag,
        "--build-arg", "CARGO_TARGET_CACHE=" + a.target_cache_id,
        "--build-context", "agent_sandbox=" + UPSTREAM,
        "--build-arg", "GO_IMAGE=" + a.go_image, "--build-arg", "RUST_IMAGE=" + a.rust_image,
        "--build-arg", "NODE_IMAGE=" + a.node_image, "--build-arg", "RUNNER_BASE_IMAGE=" + a.runner_base_image,
        "--file", str(Path(__file__).with_name("Dockerfile.agent-sandbox")), str(Path(__file__).resolve().parents[2])]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--target", choices=["workload", "runner", "preview", "local-app"], required=True)
    for option in ("tag", "go-image", "rust-image", "node-image", "runner-base-image"):
        p.add_argument("--" + option, required=True)
    p.add_argument("--print-command", action="store_true")
    p.add_argument("--target-cache-id", default="cowork-agent-sandbox-cargo-target", help="Separate serialized target caches for independent build groups")
    a = p.parse_args()
    command = arguments(a)
    if a.print_command:
        import json
        print(json.dumps(command))
    else:
        subprocess.run(command, check=True, timeout=3600)


if __name__ == "__main__":
    main()
