"""Export the common V2 execution contracts; never invent Enterprise runtime types.

This is a source artifact, not a runtime attestation or permission to execute.
An explicit Enterprise pin selects immutable bytes for its next local build.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
ROOTS = ("ExecutionPlan", "PreparedExecutionPlan", "CommitExecutionPlan", "RunRequest", "RetiredExecutionRequest", "RunRecord", "ApiError",
         "PrepareOwnerExecutionPlanV2", "ExecutionPlanV2", "PreparedExecutionPlanV2",
         "PrepareExecutionInput", "PreparedExecutionInput", "RetireExecutionInput", "PrepareRestoreExecutionPlanV2", "PrepareImportExecutionPlanV2", "WorkflowInputSnapshot")
ROUTES = ("/execution-plans", "/execution-plans/retire-request", "/orders/{order}/execution-plans/{plan}",
          "/orders/{order}/execution-plans/{plan}/commit", "/orders/{order}/execution-plans/{plan}/retire",
          "/execution-plans/v2", "/execution-plans/v2/retire-request", "/orders/{order}/execution-plans/v2/{plan}",
          "/orders/{order}/execution-plans/v2/{plan}/commit", "/orders/{order}/execution-plans/v2/{plan}/retire",
          "/orders/{order}/execution-inputs", "/orders/{order}/execution-inputs/{snapshot}",
          "/orders/{order}/execution-inputs/{snapshot}/retire",
          "/orders/{order}/runs/{run}/working-copy/files", "/orders/{order}/runs/{run}/working-copy/text",
          "/execution-plans/v2/file-restorations", "/execution-plans/v2/file-restorations/retire-request",
          "/execution-plans/v2/file-restorations/retire-intent",
          "/execution-plans/v2/file-imports", "/execution-plans/v2/file-imports/retire-request", "/execution-plans/v2/file-imports/retire-intent",
          "/runs/{run}/workflow/input-snapshot")
FILES = ("execution.schema.json", "openapi.json")


def canonical(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def sha(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def references(value: object, prefix: str) -> set[str]:
    result: set[str] = set()
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "$ref":
                if not isinstance(child, str) or not child.startswith(prefix):
                    raise ValueError("External or unsupported contract reference")
                name = child[len(prefix):]
                if not name or "/" in name or "~" in name:
                    raise ValueError("Unsupported contract reference")
                result.add(name)
            else:
                result.update(references(child, prefix))
    elif isinstance(value, list):
        for child in value:
            result.update(references(child, prefix))
    return result


def rewrite_references(value: object, old: str, new: str) -> object:
    if isinstance(value, dict):
        return {key: (new + child[len(old):] if key == "$ref" and isinstance(child, str) and child.startswith(old)
                      else rewrite_references(child, old, new)) for key, child in value.items()}
    if isinstance(value, list):
        return [rewrite_references(child, old, new) for child in value]
    return value


def closure(definitions: dict, seeds: set[str]) -> dict:
    pending = set(seeds)
    selected: dict = {}
    while pending:
        name = pending.pop()
        if name in selected:
            continue
        if name not in definitions or len(selected) >= 1000:
            raise ValueError("Missing or excessive transitive contract definitions")
        selected[name] = definitions[name]
        pending.update(references(definitions[name], "#/$defs/") - selected.keys())
    return dict(sorted(selected.items()))


def build(root: Path = ROOT) -> tuple[str, dict[str, bytes], dict]:
    root = root.resolve(strict=True)
    source_names = {"package.json", "contracts/api.schema.json", "contracts/openapi.json"}
    for directory in ("crates/contracts/src", "crates/workflow-contracts/src", "tools/contracts/src"):
        source_names.update(path.relative_to(root).as_posix() for path in (root / directory).rglob("*.rs"))
    sources: dict[str, bytes] = {}
    for name in sorted(source_names):
        path = root / name
        if path.is_symlink() or not path.resolve(strict=True).is_relative_to(root) or not path.is_file():
            raise ValueError("Source is not a regular project file")
        if path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("Contract source exceeds its bound")
        sources[name] = path.read_bytes()
    package = json.loads(sources["package.json"])
    if not re.fullmatch(r"[0-9A-Za-z.+-]{1,80}", package["version"]):
        raise ValueError("Invalid producer version")
    schema = json.loads(sources["contracts/api.schema.json"])
    api = json.loads(sources["contracts/openapi.json"])
    if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema" or api.get("openapi") != "3.1.0":
        raise ValueError("Unsupported generated contract dialect")
    paths = {name: api["paths"][name] for name in ROUTES}
    seeds = set(ROOTS) | references(paths, "#/components/schemas/")
    definitions = closure(schema["$defs"], seeds)
    api_definitions = rewrite_references(api.get("components", {}).get("schemas"), "#/components/schemas/", "#/$defs/")
    if not isinstance(api_definitions, dict) or any(api_definitions.get(name) != value for name, value in definitions.items()):
        raise ValueError("Generated schema and OpenAPI disagree; qualify generation first")
    subset = {"$schema": schema["$schema"], "$id": "https://localai-cowork.invalid/contracts/enterprise-execution/1", "$defs": definitions}
    exported_api = {"openapi": api["openapi"], "info": api["info"], "servers": api["servers"],
                    "security": api["security"], "paths": paths,
                    "components": {"securitySchemes": api["components"]["securitySchemes"],
                                   "schemas": rewrite_references(definitions, "#/$defs/", "#/components/schemas/")}}
    content = dict(zip(FILES, (canonical(subset), canonical(exported_api))))
    manifest = {"format_version": 1, "kind": "localai-cowork-v2-execution-contracts", "producer_version": package["version"],
                "exporter_sha256": sha(Path(__file__).read_bytes()),
                "roots": list(ROOTS), "definition_count": len(definitions),
                "files": {name: {"sha256": sha(value), "bytes": len(value)} for name, value in content.items()},
                "sources": {name: sha(value) for name, value in sources.items()},
                "qualification": "source_contract_only; no actor, snapshot, installed-runtime or execution attestation"}
    for name, value in sources.items():
        if (root / name).read_bytes() != value:
            raise ValueError("Producer changed during export; retry after generation completes")
    bundle_id = sha(canonical(manifest))
    content["manifest.json"] = canonical(manifest)
    return bundle_id, content, manifest


def write_bundle(parent: Path, bundle_id: str, content: dict[str, bytes]) -> Path:
    if not re.fullmatch(r"[a-f0-9]{64}", bundle_id) or set(content) != {*FILES, "manifest.json"}:
        raise ValueError("Invalid artifact layout")
    manifest = json.loads(content["manifest.json"])
    if sha(content["manifest.json"]) != bundle_id or manifest.get("format_version") != 1 or manifest.get("kind") != "localai-cowork-v2-execution-contracts":
        raise ValueError("Invalid artifact manifest")
    if set(manifest.get("files", {})) != set(FILES) or any(manifest["files"][name] != {"sha256": sha(content[name]), "bytes": len(content[name])} for name in FILES):
        raise ValueError("Artifact content does not match its manifest")
    parent = parent.resolve()
    parent.mkdir(parents=True, exist_ok=True)
    target = parent / bundle_id
    if target.is_symlink() or target.resolve().parent != parent:
        raise ValueError("Artifact target is not a direct child of its destination")
    if target.exists():
        if target.is_symlink() or not target.is_dir() or {p.name for p in target.iterdir()} != set(content):
            raise ValueError("Existing immutable artifact differs")
        if any((target / name).is_symlink() or (target / name).read_bytes() != value for name, value in content.items()):
            raise ValueError("Existing immutable artifact differs")
        return target
    with tempfile.TemporaryDirectory(prefix=".v2-contract-stage-", dir=parent) as temporary:
        stage = Path(temporary) / bundle_id
        stage.mkdir()
        for name, value in content.items():
            (stage / name).write_bytes(value)
        if stage.resolve().parent != Path(temporary).resolve() or target.parent.resolve() != parent:
            raise ValueError("Artifact staging path changed")
        # Windows indexers/sync clients may briefly hold newly written files.
        # Keep the complete directory unpublished until the atomic rename works.
        for attempt in range(7):
            try:
                os.rename(stage, target)
                break
            except PermissionError:
                if target.exists():
                    return write_bundle(parent, bundle_id, content)
                if attempt == 6:
                    raise
                time.sleep(min(0.1 * 2**attempt, 1.6))
    return target


def pin(enterprise: Path, bundle_id: str, content: dict[str, bytes]) -> Path:
    enterprise = enterprise.resolve(strict=True)
    cargo = (enterprise / "server/cowork-server/Cargo.toml").read_text(encoding="utf-8")
    if not re.search(r'(?m)^name\s*=\s*"cowork-server"\s*$', cargo):
        raise ValueError("Not the Enterprise server checkout")
    if not (enterprise / "contracts/v2-runtime").resolve().is_relative_to(enterprise):
        raise ValueError("Enterprise artifact destination is outside its source tree")
    write_bundle(enterprise / "contracts/v2-runtime", bundle_id, content)
    target = enterprise / "server/cowork-server/src/v2_runtime_contract_pin.rs"
    if not target.resolve().is_relative_to(enterprise) or target.is_symlink():
        raise ValueError("Enterprise pin is outside its source tree")
    if target.exists() and not target.read_bytes().startswith(b'// Generated by V2 tools/v2_rebuild/export_enterprise_contracts.py;'):
        raise ValueError("Refusing to overwrite an unrelated Enterprise source file")
    source = '// Generated by V2 tools/v2_rebuild/export_enterprise_contracts.py; source contract, no runtime qualification.\n'
    source += f'pub(super) const BUNDLE_ID: &str = "{bundle_id}";\n'
    for constant, filename in (("MANIFEST", "manifest.json"), ("SCHEMA", "execution.schema.json"), ("OPENAPI", "openapi.json")):
        source += f'pub(super) const {constant}: &[u8] = include_bytes!("../../../contracts/v2-runtime/{bundle_id}/{filename}");\n'
    encoded = source.encode("utf-8")
    if not target.exists() or target.read_bytes() != encoded:
        with tempfile.NamedTemporaryFile(prefix=".v2-contract-pin-", suffix=".tmp", dir=target.parent, delete=False) as temporary:
            staged = Path(temporary.name)
            temporary.write(encoded)
        try:
            if staged.resolve().parent != target.parent.resolve():
                raise ValueError("Pin staging path changed")
            os.replace(staged, target)
        finally:
            if staged.exists():
                staged.unlink()
    return target


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Parent of the immutable bundle directory")
    parser.add_argument("--enterprise-root", type=Path, help="Explicitly pin these source contracts for the next Enterprise build")
    args = parser.parse_args()
    if not args.output and not args.enterprise_root:
        parser.error("choose --output or --enterprise-root")
    bundle_id, content, manifest = build()
    if args.output:
        write_bundle(args.output, bundle_id, content)
    if args.enterprise_root:
        pin(args.enterprise_root, bundle_id, content)
    print(json.dumps({"bundle_id": bundle_id, "producer_version": manifest["producer_version"],
                      "definitions": manifest["definition_count"], "runtime_qualified": False}))


if __name__ == "__main__":
    main()
