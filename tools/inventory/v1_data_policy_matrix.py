"""Source-backed M0 table matrix; fixed in-memory schemas, no user database opens.

Disposition rules describe the V2 historical-import policy, not observed V1 grants.
Original source archive/manifest pins and the qualified schema/fence code are required.
"""
from __future__ import annotations
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/migration"))
import v1_sqlite_export as frozen

FORMAT = "localai-cowork-v1-source-data-policy-matrix/1"
FENCE_SHA = "618049c79925d9871cd7ead15fff2d87feb46a40cd400ed4dbded4504f979f92"
PROFILES_SHA = "995a51bc7101ac1b8f4ef84e04cc589b0c344259f19cebeca2ad20f15f34843f"
MAX_SOURCE_TEXT = 4 * 1024**2

# These reviewed names never select a source path or issue a runtime permission.
REVIEW_GROUPS = {
    "source_schema_metadata": {"schema_version", "sqlite_sequence"},
    "permission_reissue": {"allowed_folders", "policy_deny_rules", "policy_flags", "policy_settings", "policy_tool_states"},
    "credential_recreation": {"api_profiles", "codex_auth_profiles", "daemon_provider_bindings", "daemon_mcp_bindings", "daemon_run_model_configs"},
    "explicit_new_schedule": {"scheduled_tasks", "scheduled_runs", "daemon_schedules"},
    "external_resource_review": {"artifact_exports", "artifact_versions", "project_resources", "terminal_backends", "tool_gateway_entries", "worker_sandboxes", "managed_processes"},
    "device_reenrollment": {"daemon_sync_peers", "daemon_sync_changes", "daemon_sync_conflicts"},
    "inactive_history": {
        "agent_personalities", "app_backend_defaults", "audit_events", "chat_messages", "chat_threads",
        "codex_thread_bindings", "crew_approvals", "crew_definition_versions", "crew_definitions", "crew_role_bindings",
        "crew_run_events", "crew_run_logs", "crew_runs", "engine_run_artifacts", "engine_run_checkpoints", "engine_run_events",
        "engine_runs", "insights_events", "learning_outcomes", "memory_entries", "memory_providers", "project_threads", "projects",
        "provider_fallback_approvals", "rpc_pipelines", "runtime_instructions", "sessions", "skill_improvements", "skills",
        "task_steps", "tasks", "user_profile", "work_tasks", "daemon_approval_requests", "daemon_entities", "daemon_input_requests",
        "daemon_project_bindings", "daemon_run_checkpoints", "daemon_run_events", "daemon_run_workspaces", "daemon_runs",
    },
}

def source_reference(fences, manifest, archive, manifest_pin, archive_pin):
    for path, pin in [(manifest, manifest_pin), (archive, archive_pin)]:
        if re.fullmatch(r"[0-9a-f]{64}", pin) is None or fences.receipt(path)["sha256"] != pin:
            raise frozen.Refused("Original source capture pin mismatch")
    metadata, _ = frozen.read_json(fences, manifest)
    if metadata.get("schema") != 2 or metadata.get("archive_sha256") != archive_pin:
        raise frozen.Refused("Unknown or unbound source capture")
    selected = {}
    for item in metadata.get("files", []):
        if not isinstance(item, dict):
            raise frozen.Refused("Invalid source capture entry")
        name = item.get("path")
        if name in frozen.profiles.SOURCE_BINDINGS:
            if name in selected:
                raise frozen.Refused("Duplicate selected source capture entry")
            selected[name] = item
    if set(selected) != set(frozen.profiles.SOURCE_BINDINGS):
        raise frozen.Refused("Original schema source missing")
    stream = fences.streams[archive]
    stream.seek(0)
    result = {}
    with zipfile.ZipFile(stream) as zipped:
        entries = zipped.infolist()
        if len(entries) > 20000:
            raise frozen.Refused("Source archive entry limit")
        for name, expected in selected.items():
            matches = [entry for entry in entries if entry.filename == name]
            if len(matches) != 1:
                raise frozen.Refused("Ambiguous selected source archive member")
            entry = matches[0]
            if entry.is_dir() or entry.flag_bits & 1 or not 0 < entry.file_size <= MAX_SOURCE_TEXT or entry.file_size != expected.get("size"):
                raise frozen.Refused("Invalid selected source archive member")
            raw = zipped.read(entry)
            digest = hashlib.sha256(raw).hexdigest()
            if digest != expected.get("sha256") or digest != frozen.profiles.SOURCE_BINDINGS[name]:
                raise frozen.Refused("Original schema source binding differs")
            result[name] = raw.decode("utf-8")
    return result

def fixed_profiles():
    result = []
    def append(conn, family, version):
        schema = frozen.schema(conn)
        signature = frozen.signature(schema)
        profile = frozen.REFERENCE_PROFILES.get((family, version, signature))
        if profile is None:
            raise frozen.Refused("Fixed source schema binding differs")
        result.append({"profile": profile, "family": family, "version": version, "schema_sha256": signature, "schema": schema})
    with contextlib.closing(sqlite3.connect(":memory:")) as conn:
        append(conn, "desktop", 0)
        conn.executescript("PRAGMA foreign_keys=OFF;PRAGMA legacy_alter_table=ON;CREATE TABLE schema_version(version INTEGER NOT NULL);INSERT INTO schema_version VALUES(0);")
        append(conn, "desktop", 0)
        for version, events in enumerate(frozen.profiles.DESKTOP_STAGES, 1):
            frozen.apply_events(conn, events)
            append(conn, "desktop", version)
            if version in (27, 28):
                with contextlib.closing(sqlite3.connect(":memory:")) as variant:
                    conn.commit()
                    conn.backup(variant)
                    variant.executescript(frozen.profiles.DESKTOP_BACKPATCH)
                    append(variant, "desktop", version)
    with contextlib.closing(sqlite3.connect(":memory:")) as conn:
        conn.executescript(frozen.profiles.DAEMON_BASE)
        append(conn, "daemon", None)
        conn.executescript(frozen.profiles.DAEMON_SYNC)
        append(conn, "daemon", None)
    if len(result) != 34 or len({item["profile"] for item in result}) != 34:
        raise frozen.Refused("Fixed source profile coverage differs")
    return result

def matrix(source_texts):
    profiles = fixed_profiles()
    groups = {}
    for group, tables in REVIEW_GROUPS.items():
        for table in tables:
            if table in groups:
                raise frozen.Refused("Ambiguous reviewed table policy")
            groups[table] = group
    tables = {}
    for profile in profiles:
        for table, metadata in profile["schema"]["tables"].items():
            key = (profile["family"], table)
            if table not in groups:
                raise frozen.Refused("Unreviewed source table")
            item = tables.setdefault(key, {"family": profile["family"], "table": table, "v2_review_group": groups[table],
                "v2_execution_policy": dict(frozen.POLICY), "profile_membership": [], "schema_variants": {}, "source_locations": []})
            item["profile_membership"].append(profile["profile"])
            digest = frozen.digest(metadata)
            item["schema_variants"].setdefault(digest, {"profiles": [], "columns": metadata["columns"],
                "foreign_keys": metadata["foreign_keys"], "indexes": metadata["indexes"]})["profiles"].append(profile["profile"])
    if {item["table"] for item in tables.values()} != set(groups):
        raise frozen.Refused("Reviewed policy table coverage differs")
    for item in tables.values():
        name = item["table"]
        pattern = re.compile(r"\b(?:CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?|ALTER\s+TABLE\s+)" + re.escape(name) + r"\b", re.I)
        for path, content in source_texts.items():
            for match in pattern.finditer(content):
                item["source_locations"].append({"path": path, "source_sha256": frozen.profiles.SOURCE_BINDINGS[path],
                    "line": content.count("\n", 0, match.start()) + 1})
        # SQLite's sequence table is implicit in reviewed AUTOINCREMENT DDL.
        if not item["source_locations"] and name not in ("sqlite_sequence", "schema_version"):
            raise frozen.Refused("Reviewed source DDL locator missing")
    return {"format": FORMAT, "scope": "Pinned original source schemas and proposed V2 inactive-history disposition; no installed user rows or V1 runtime grants verified",
        "profiles": [{key: value for key, value in item.items() if key != "schema"} for item in profiles],
        "tables": [tables[key] for key in sorted(tables)], "profile_count": len(profiles),
        "table_counts": {family: sum(item["family"] == family for item in tables.values()) for family in ("desktop", "daemon")},
        "source_bindings": frozen.profiles.SOURCE_BINDINGS, "execution_policy": dict(frozen.POLICY),
        "original_database_opens": 0, "original_application_launches": 0, "actual_data_migrated": False,
        "v1_runtime_policy_behavior_verified": False, "installed_schema_parity_proven": False, "full_m0_complete": False}

def run(manifest_value, archive_value, manifest_pin, archive_pin, output_value):
    if os.name != "nt":
        raise frozen.Refused("Qualified native source fences require Windows")
    if frozen.hash_file(Path(frozen.__file__)) != FENCE_SHA or frozen.hash_file(Path(frozen.profiles.__file__)) != PROFILES_SHA:
        raise frozen.Refused("Qualified source schema or native fence code changed")
    manifest, archive = frozen.bounded_path(manifest_value), frozen.bounded_path(archive_value)
    output = frozen.output_path(output_value, [manifest, archive])
    budget = frozen.Budget(time.monotonic() + 120)
    with frozen.FrozenDirectories([output.parent]), frozen.FrozenFiles([manifest, archive], budget) as fences:
        before = [fences.receipt(path) for path in (manifest, archive)]
        source = source_reference(fences, manifest, archive, manifest_pin, archive_pin)
        result = matrix(source)
        result["source_capture"] = {"manifest_sha256": manifest_pin, "archive_sha256": archive_pin}
        if before != [fences.receipt(path) for path in (manifest, archive)]:
            raise frozen.Refused("Source capture changed under native fence")
        stage = Path(tempfile.mkdtemp(prefix=".private-data-policy-matrix-", dir=output.parent))
        identity = frozen.stage_identity(stage)
        try:
            digest = frozen.write_json(stage / "matrix.json", result)
            if before != [fences.receipt(path) for path in (manifest, archive)]:
                raise frozen.Refused("Source capture changed before publication")
            frozen.publish(stage, output, identity)
        except BaseException:
            frozen.cleanup_stage(stage, identity)
            raise
    return {"output": str(output), "matrix_sha256": digest, "profiles": result["profile_count"], "table_counts": result["table_counts"]}

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source-manifest", "source-archive", "source-manifest-sha256", "source-archive-sha256", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args(argv)
    try:
        result = run(args.source_manifest, args.source_archive, args.source_manifest_sha256, args.source_archive_sha256, args.output)
    except (frozen.Refused, OSError, ValueError, zipfile.BadZipFile, UnicodeError):
        print("Source data policy matrix refused", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
