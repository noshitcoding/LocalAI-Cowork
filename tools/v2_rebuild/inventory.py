"""Deterministic source inventory for the approved V2 rebuild, not qualification."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


PACKAGES = {
    "B": {24, 48},
    "C": {17, 18, 19, 25, 26},
    "D": {23, 62, 79, 80, 81, 82},
    "E": {21, 22, 27, 53, 54, 55, 63, 64, 87},
    "G": set(range(1, 17)) | {20, 28, 29, 30, 31, 32, 33, 34, 35, 36, 38, 56, 57, 65},
    "H": {37, 39, 40, 49, 50, 51, 52, 66, 67, 70, 71, 72, 73, 74, 75, 76, 77, 83, 84, 85, 86},
    "I": {41, 42, 43, 44, 45, 46, 47, 58, 59, 60, 61, 68, 69},
}
ACCEPTANCE = {
    "B": "Authenticated prepare/review/commit, atomic original receipt, immutable target and negative admission tests across every execution route.",
    "C": "Real Windows/WSL2 and native Linux isolation, bounded resources, verified input/output and runtime update/rollback.",
    "D": "Shared admission and effect journal; inherited rights only narrow; no replay of ambiguous external effects.",
    "E": "Actual supported models, services and tool bundles; verified outputs, explicit unsupported cases and qualified target capabilities.",
    "G": "Real identity/key/device/client/offline workflows, conflict and revocation tests; metadata never grants execution authority.",
    "H": "Actual configured infrastructure, positive stop/storage evidence, backup/restore and failure/recovery qualification.",
    "I": "Current final package bytes, complete migration/integrity, physical platform and accessibility acceptance, load and endurance evidence.",
}


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build_inventory(root: Path, server: Path | None = None) -> dict:
    feature_path = root / "docs/FEATURE-MATRIX.md"
    requirements = []
    seen: set[int] = set()
    for line in feature_path.read_text(encoding="utf-8-sig").splitlines():
        match = re.match(r"^\|\s*(\d+)\s*\|\s*([^|]+)\|\s*(.*)\|\s*$", line)
        if not match:
            continue
        number = int(match[1])
        if number in seen:
            raise ValueError(f"Duplicate requirement: {number}")
        seen.add(number)
        groups = [name for name, ids in PACKAGES.items() if number in ids]
        if number == 78:
            package, status, acceptance = "excluded", "excluded_by_approved_scope", "Weighted global fairness is a separate extension."
        else:
            if len(groups) != 1:
                raise ValueError(f"Requirement {number} needs exactly one explicit work package: {groups}")
            package, status = groups[0], "requires_current_verification"
            acceptance = ACCEPTANCE[package]
        requirements.append({
            "id": number, "title": match[2].strip(), "source_declaration": match[3].strip(),
            "work_package": package, "status": status, "acceptance": acceptance,
            "evidence": [], "delivery_artifact": None,
        })
    expected = {78} | set().union(*PACKAGES.values())
    if seen != expected:
        raise ValueError(f"Feature mapping drift: missing={sorted(expected-seen)}, new={sorted(seen-expected)}")
    package_json = json.loads((root / "package.json").read_text(encoding="utf-8-sig"))
    tauri = json.loads((root / "apps/desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8-sig"))
    cargo = (root / "Cargo.toml").read_text(encoding="utf-8-sig")
    version = re.search(r'(?m)^version\s*=\s*"([^"]+)"', cargo)
    versions = {"npm": package_json["version"], "tauri": tauri["version"], "cargo": version[1] if version else None}
    if len(set(versions.values())) != 1:
        raise ValueError(f"Application version mismatch: {versions}")
    inputs = ["AGENTS.md", "README.md", "package.json", "Cargo.toml", "Cargo.lock", "package-lock.json", "apps/desktop/src-tauri/tauri.conf.json", "docs/FEATURE-MATRIX.md", "docs/COMPLETION-WORKPLAN.md"]
    result = {
        "format": 1, "purpose": "source_inventory_not_runtime_or_release_qualification",
        "versions": versions, "source_sha256": {p: sha(root / p) for p in inputs},
        "roadmap": [f"M{i}" for i in range(11)],
        "work_packages": {"A": "inventory", "B": "execution_contract", "C": "managed_runtime", "D": "route_unification", "E": "models_and_tools", "F": "workflows_and_schedules", "G": "identity_data_devices", "H": "enterprise_operations", "I": "migration_ux_release"},
        "requirements": requirements,
        "production_accepted": False,
    }
    if server is not None:
        selected = ["AGENTS.md", "Cargo.toml", "Cargo.lock", "docs/DISTRIBUTED_ARCHITECTURE.md", "docs/ENTERPRISE_EXECUTION_ADMISSION.md"]
        result["enterprise_source_sha256"] = {p: sha(server / p) for p in selected}
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--server-root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = build_inventory(args.root.resolve(), args.server_root.resolve() if args.server_root else None)
    body = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(body, encoding="utf-8", newline="")
        print(f"Inventoried {len(result['requirements'])} requirements; no runtime qualification asserted.")
    else:
        print(body, end="")


if __name__ == "__main__":
    main()
