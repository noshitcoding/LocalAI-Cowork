"""Create a GitHub draft from an explicitly staged preview plan.

This is the only release mutation in the preview workflow. It always remains a
prerelease draft and never updates Pages, updater feeds or the stable latest.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess

from release_checks import checked_asset, ordinary, preview_version, read
import preview_trust


def draft_command(folder: Path) -> list[str]:
    folder = ordinary(folder, directory=True)
    plan = read(folder / "release-plan.json")
    if (plan.get("format") != "localai-cowork-preview-draft-plan/1" or plan.get("draft") is not True
            or plan.get("prerelease") is not True or plan.get("make_latest") is not False
            or plan.get("published") is not False or plan.get("stable_feed_modified") is not False):
        raise ValueError("Only an unmodified preview draft plan is allowed")
    version = preview_version(plan["version"])
    repository = plan["repository"]
    if (not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository)
            or plan.get("tag") != "v" + version or not re.fullmatch(r"[0-9a-f]{40}", plan.get("commit", ""))):
        raise ValueError("The exact repository, prerelease tag and commit are required")
    required = {"source-provenance.json", "release-provenance.json", "preview.json", "RELEASE-NOTES.md", "SHA256SUMS"}
    if not required <= set(plan["assets"]):
        raise ValueError("The complete checked public release set is required")
    paths = [checked_asset(folder, dict(record, filename=name)) for name, record in sorted(plan["assets"].items())]
    return ["gh", "release", "create", plan["tag"], *map(str, paths), "--repo", repository,
            "--target", plan["commit"], "--verify-tag", "--draft", "--prerelease", "--latest=false",
            "--title", "LocalAI Cowork V2 Preview " + version,
            "--notes-file", str(folder / "RELEASE-NOTES.md")]


def create_draft(folder: Path, policy_path: Path) -> None:
    command = draft_command(folder)
    plan = read(folder / "release-plan.json")
    trusted = preview_trust.policy(policy_path, plan["repository"])
    provenance = read(folder / "release-provenance.json")
    from release_checks import pin
    if provenance.get("trusted_policy_sha256") != pin(policy_path)["sha256"]:
        raise ValueError("The independent release policy changed after staging")
    producer = provenance["producer"]
    preview_trust.live_producer(folder, trusted, plan["commit"], producer["run_id"], producer["artifact_id"])
    remote = json.loads(subprocess.check_output(["gh", "api",
                        f"repos/{plan['repository']}/commits/{plan['tag']}"], text=True))
    if remote.get("sha") != plan["commit"]:
        raise ValueError("The remote preview tag moved after the checked staging")
    subprocess.run(command, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--staged", type=Path, required=True)
    parser.add_argument("--create-draft", action="store_true", help="Explicitly create the remote draft; default prints its checked command")
    parser.add_argument("--trusted-policy", type=Path, help="Independent current policy; required when creating the draft")
    args = parser.parse_args()
    command = draft_command(args.staged)
    if args.create_draft:
        if args.trusted_policy is None:
            raise ValueError("Select the independently reviewed current release policy")
        create_draft(args.staged, args.trusted_policy)
    else:
        print(json.dumps({"command": command, "executed": False}, indent=2))


if __name__ == "__main__":
    main()
