"""Recover an exact lost local image from retained BuildKit blobs, never rebuild it.

Every blob must match its original digest. Intended for operator repair after a
mutable development tag displaced a fleet release; fails if cache was pruned.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--build-ref", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    match = re.fullmatch(r"(cowork-[a-z0-9-]+)@(sha256:[0-9a-f]{64})", args.image)
    if not match or not re.fullmatch(r"[a-z0-9/._-]+", args.build_ref):
        parser.error("Expected a local Cowork image digest and build record reference")
    repository, root_digest = match.groups()
    tag = "retained-" + root_digest.replace(":", "-")
    blobs = {}

    def fetch(digest):
        if digest in blobs:
            return
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
            raise ValueError("Invalid build blob digest")
        raw = subprocess.check_output(["docker", "buildx", "history", "inspect", "attachment", args.build_ref, digest], timeout=60)
        candidates = [raw, raw.rstrip(b"\n")]
        try:
            value = json.loads(raw)
            # CLI pretty-prints some manifests. Recover the original encoding,
            # but accept it only if its cryptographic digest is exactly right.
            candidates += [json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode(), json.dumps(value, indent=2, ensure_ascii=False).encode()]
        except (ValueError, UnicodeError):
            value = None
        blob = next((data for data in candidates if "sha256:" + hashlib.sha256(data).hexdigest() == digest), None)
        if blob is None:
            raise ValueError("Build blob digest mismatch: " + digest)
        blobs[digest] = blob
        if isinstance(value, dict) and value.get("mediaType") in ["application/vnd.oci.image.index.v1+json", "application/vnd.oci.image.manifest.v1+json"]:
            for child in [*value.get("manifests", []), *value.get("layers", []), *([value["config"]] if "config" in value else [])]:
                if "platform" in child and child["platform"] != {"architecture": "amd64", "os": "linux"}:
                    continue  # Recover the qualified runtime platform; attestations may no longer be cached.
                fetch(child["digest"])
                if len(blobs[child["digest"]]) != child["size"]:
                    raise ValueError("Build blob size mismatch")

    fetch(root_digest)
    media_type = json.loads(blobs[root_digest])["mediaType"]
    index = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": [{"mediaType": media_type, "digest": root_digest, "size": len(blobs[root_digest]), "annotations": {
        "containerd.io/distribution.source.docker.io": "library/" + repository,
        "io.containerd.image.name": "docker.io/library/" + repository + ":" + tag,
        "org.opencontainers.image.ref.name": tag}}]}
    with tarfile.open(args.output, "w") as archive:
        entries = {"oci-layout": b'{"imageLayoutVersion":"1.0.0"}', "index.json": json.dumps(index).encode(), **{"blobs/sha256/" + digest.split(":")[1]: blob for digest, blob in blobs.items()}}
        for name, data in entries.items():
            info = tarfile.TarInfo(name); info.size = len(data); info.mode = 0o600
            archive.addfile(info, io.BytesIO(data))
    subprocess.run(["docker", "image", "load", "--platform", "linux/amd64", "--input", str(args.output)], check=True)
    subprocess.run(["docker", "image", "inspect", args.image, "--format", "{{.Id}}"], check=True)
    print("exact_build_image_recovered=" + args.image)


if __name__ == "__main__":
    main()
