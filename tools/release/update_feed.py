"""Produce an isolated V2 preview feed from original signed package receipts.

Writes a new local artifact. Publication and stable/V1 feeds are separate.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import urllib.parse

from release_checks import HOSTS, checked_package, https_url, preview_version


def generate(packages: list[Path], version: str, base_url: str, notes: str = "", *, trusted_policy: dict | None = None) -> dict:
    if trusted_policy is None:
        raise ValueError("An independently reviewed preview identity policy is required")
    preview_version(version)
    https_url(base_url)
    if urllib.parse.unquote(urllib.parse.urlsplit(base_url).path).rstrip("/").split("/")[-1] != "v" + version:
        raise ValueError("Preview artifacts must use their exact immutable version/tag directory")
    platforms = {}
    selected_profile = None
    for folder in packages:
        receipt, paths = checked_package(folder, version, trusted_policy=trusted_policy)
        profile = receipt["signing_profile"]
        profile_identity = (profile["updater_public_key"], profile["updater_endpoint"])
        if selected_profile is not None and selected_profile != profile_identity:
            raise ValueError("All preview platforms must share the same updater identity")
        selected_profile = profile_identity
        platform = HOSTS[receipt["host"]][0]
        if platform in platforms:
            raise ValueError("Duplicate update platform")
        kind = "nsis" if platform.startswith("windows") else "appimage"
        platforms[platform] = {"url": base_url.rstrip("/") + "/" + urllib.parse.quote(paths[kind].name),
                               "signature": paths[kind + "_signature"].read_text().strip()}
    if not platforms:
        raise ValueError("At least one original signed package is required")
    return {"version": version, "notes": notes, "pub_date": datetime.now(timezone.utc).isoformat(),
            "platforms": platforms}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, action="append", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--notes", default="")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--trusted-policy", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    args = parser.parse_args()
    if not args.output.is_absolute() or args.output.exists() or args.output.is_symlink():
        raise ValueError("Select a NEW absolute update-feed output")
    from preview_trust import policy
    result = generate(args.package, args.version, args.base_url, args.notes,
                      trusted_policy=policy(args.trusted_policy, args.repository))
    with args.output.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"feed": str(args.output), "platforms": list(result["platforms"]), "published": False}))


if __name__ == "__main__":
    main()
