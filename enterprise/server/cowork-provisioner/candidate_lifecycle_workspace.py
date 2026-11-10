"""Workspace/content fixtures for the isolated candidate lifecycle harness.

All API, SQL and volume operations go through the owning harness context. This
module does not choose a Docker project, start services, or read live settings.
The returned state contains temporary authentication secrets; retain it only in
the private fixture process, never in the public qualification report.

Required context methods: api, request, sql, run_shell (Python source),
set_chat_enabled, login, active, current, read_workspace, and check. The harness
owns lifecycle actions and cleanup. read_workspace must inspect an existing
volume before mounting it read-only, so inspection cannot create missing data.
"""
from __future__ import annotations

import copy
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import uuid


TARGET = {"kind": "server_linux", "pool_id": None}
MARKER_FILE = "lifecycle-marker.txt"
SECRET_NAMES = (
    "storage_master_key", "bootstrap_token", "runner_signing_key",
    "database_password", "s3_access_key", "s3_secret_key",
)


def _uuid(value):
    parsed = uuid.UUID(value)
    assert str(parsed) == value, "fixture identifier is not canonical"
    return parsed


def _ids(values):
    return ",".join("'" + str(_uuid(value)) + "'" for value in values)


def _json_sql(ctx, statement):
    value = ctx.sql(statement)
    if isinstance(value, bytes):
        value = value.decode("utf-8")
    return json.loads(value)


def _thread(ctx, project_id, title):
    return ctx.api("/threads", {"project_id": project_id, "title": title}, expected=201)["id"]


def _completed(ctx, thread, script):
    run = ctx.run_shell(thread, script)
    assert run["state"] == "completed", "fixture shell task did not complete"
    _uuid(run["spec"]["id"])
    sandbox = run["result"]["sandbox"]
    assert sandbox["exit_code"] == 0, "fixture shell task failed"
    assert isinstance(sandbox["workspace_volume"], str), "runner omitted workspace volume"
    return run


def _binding(ctx, run):
    ident = str(_uuid(run["spec"]["id"]))
    return ctx.sql(f"SELECT coalesce(sandbox_workspace_id::text,'legacy') FROM runs WHERE id='{ident}';").strip()


def _resources(ctx):
    return ctx.infra.resource_names(ctx.current(), ctx.active())


def _secret_hashes(ctx):
    root = Path(ctx.active()) / "secrets"
    return {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in SECRET_NAMES}


def _content(ctx, state):
    # Compare only fixture-owned records; process health and fresh login/audit
    # records are expected to change during a real restart or promotion.
    return {
        "projects": _json_sql(ctx, "SELECT coalesce(jsonb_agg(to_jsonb(p) ORDER BY id),'[]'::jsonb) FROM projects p WHERE id IN (" + _ids(state["project_ids"]) + ");"),
        "threads": _json_sql(ctx, "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM threads t WHERE id IN (" + _ids(state["thread_ids"]) + ");"),
        "runs": _json_sql(ctx, "SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY id),'[]'::jsonb) FROM runs r WHERE id IN (" + _ids(state["completed_run_ids"]) + ");"),
        "messages": _json_sql(ctx, "SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY id),'[]'::jsonb) FROM messages m WHERE run_id IN (" + _ids(state["completed_run_ids"]) + ");"),
    }


def _bindings(ctx, state):
    runs = _json_sql(ctx, "SELECT jsonb_object_agg(id::text,sandbox_workspace_id) FROM runs WHERE id IN (" + _ids(state["completed_run_ids"]) + ");")
    chats = _json_sql(ctx, "SELECT coalesce(jsonb_agg(to_jsonb(w) ORDER BY thread_id),'[]'::jsonb) FROM chat_workspaces w WHERE thread_id IN (" + _ids(state["thread_ids"]) + ");")
    return {"runs": runs, "chats": chats}


def _schedule(ctx, state):
    ident = str(_uuid(state["schedule_id"]))
    return _json_sql(ctx, f"SELECT to_jsonb(s) FROM schedules s WHERE id='{ident}';")


def _assert_schedule(ctx, state, *, paused):
    row = _schedule(ctx, state)
    for field, expected in state["schedule_content"].items():
        assert row[field] == expected, "schedule content changed: " + field
    assert row["enabled"] is (not paused), "schedule enabled state changed"
    if paused:
        assert row["next_run_at"] is None, "paused schedule still has a dispatch time"
        assert row["blocked_reason"] == "instance_restored_requires_review", "schedule review fence missing"
    else:
        assert row["next_run_at"] == state["schedule_next_run_at"], "enabled schedule dispatch time changed"
        assert row["blocked_reason"] is None, "enabled schedule remains blocked"


def _assert_waiting(ctx, state):
    ident = str(_uuid(state["waiting_id"]))
    row = _json_sql(ctx, f"SELECT jsonb_build_object('state',state,'spec',spec,'clear_lease',lease_owner IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL) FROM runs WHERE id='{ident}';")
    assert row["spec"] == state["waiting_spec"], "unfinished task input changed"
    assert row["state"] == state["waiting_state"], "unfinished task was replayed or its fence changed"
    assert row["clear_lease"] is True, "unfinished historical task retained a lease"


def _assert_volumes(ctx, state):
    prefix = _resources(ctx)["volume_prefix"]
    for sample in state["volume_samples"]:
        volume = prefix + _uuid(sample["storage_id"]).hex
        assert ctx.read_workspace(volume, MARKER_FILE) == sample["marker"].encode(), "workspace marker changed or generation volume is missing"


def prepare_workspace_checks(ctx):
    """Create real legacy/chat jobs, encrypted content and exact auth fixtures."""
    project_id = str(_uuid(ctx.project["id"]))
    ctx.set_chat_enabled(False)
    legacy_thread = _thread(ctx, project_id, "Lifecycle legacy workspace")
    legacy_marker = "legacy-" + uuid.uuid4().hex
    legacy = _completed(ctx, legacy_thread, f"from pathlib import Path; Path({MARKER_FILE!r}).write_text({legacy_marker!r}); print('legacy-ready')")
    legacy_next = _completed(ctx, legacy_thread, f"from pathlib import Path; assert not Path({MARKER_FILE!r}).exists(); print('legacy-isolated')")
    assert _binding(ctx, legacy) == _binding(ctx, legacy_next) == "legacy", "legacy runs acquired a chat binding"
    assert legacy["result"]["sandbox"]["workspace_volume"] != legacy_next["result"]["sandbox"]["workspace_volume"], "legacy jobs shared writable storage"
    ctx.check("lifecycle_legacy_jobs_keep_distinct_per_run_storage")

    ctx.set_chat_enabled(True)
    chat_thread = _thread(ctx, project_id, "Lifecycle persistent workspace")
    foreign_thread = _thread(ctx, project_id, "Lifecycle isolated workspace")
    chat_marker = "chat-" + uuid.uuid4().hex
    first = _completed(ctx, chat_thread, f"from pathlib import Path; Path({MARKER_FILE!r}).write_text({chat_marker!r}); Path('fixture-tool.py').write_text('print(42)'); print('chat-ready')")
    follow_script = f"from pathlib import Path; import subprocess; assert Path({MARKER_FILE!r}).read_text()=={chat_marker!r}; assert subprocess.check_output(['python3','fixture-tool.py']).strip()==b'42'; print('chat-preserved')"
    follow = _completed(ctx, chat_thread, follow_script)
    workspace = _binding(ctx, first)
    _uuid(workspace)
    assert _binding(ctx, follow) == workspace, "chat follow-up changed its database binding"
    assert first["result"]["sandbox"]["workspace_volume"] == follow["result"]["sandbox"]["workspace_volume"], "chat follow-up changed its volume"
    foreign = _completed(ctx, foreign_thread, f"from pathlib import Path; assert not Path({MARKER_FILE!r}).exists(); assert not Path('fixture-tool.py').exists(); print('chat-isolated')")
    assert _binding(ctx, foreign) != workspace, "separate chats shared a database binding"
    assert foreign["result"]["sandbox"]["workspace_volume"] != first["result"]["sandbox"]["workspace_volume"], "separate chats shared writable storage"
    ctx.check("lifecycle_chat_followup_keeps_files_tool_and_binding_while_other_chat_is_isolated")

    artifact = ctx.api(f"/runs/{first['spec']['id']}/attachments?name=lifecycle-marker.txt", chat_marker.encode(), expected=201)
    assert artifact["storage"] == "object_store_encrypted", "attachment did not use encrypted object storage"
    assert ctx.api(f"/runs/{first['spec']['id']}/artifacts/{artifact['id']}", raw=True) == chat_marker.encode(), "encrypted attachment round-trip failed"
    task = ctx.api("/tasks", {"project_id": project_id, "name": "Lifecycle schedule must pause", "instructions": "fixture-only", "required_capabilities": ["shell"], "default_target": TARGET, "release": True}, expected=201)
    schedule_thread = _thread(ctx, project_id, "Lifecycle paused schedule")
    schedule = ctx.api("/schedules", {"task_id": task["id"], "project_id": project_id, "thread_id": schedule_thread, "cron": "0 0 1 1 *", "timezone": "UTC", "executor_target": TARGET, "enabled": True}, expected=201)
    assert schedule["enabled"] is True and schedule["next_run_at"], "fixture schedule was not armed"

    private = ctx.api("/projects", {"name": "Lifecycle snapshot waiting", "privacy": "private_local"}, expected=201)
    waiting_thread = _thread(ctx, private["id"], "Lifecycle unfinished history")
    waiting = ctx.api("/runs", {"thread_id": waiting_thread, "project_id": private["id"], "project_revision": private["revision"], "project_privacy": "private_local", "executor_target": TARGET, "required_capabilities": ["shell"], "idempotency_key": str(uuid.uuid4()), "input": {"prompt": "Wait for a local snapshot; never replay on restore."}}, expected=201)
    assert ctx.api(f"/runs/{waiting['spec']['id']}")["state"] == "waiting_for_snapshot", "unfinished fixture unexpectedly executed"

    executor_id = str(uuid.uuid4())
    ctx.api("/executors", {"schema_version": 2, "executor_id": executor_id, "kind": "personal_device", "pool_id": None, "owner_user_id": None, "display_name": "Lifecycle credential fixture", "protocol_version": 2, "capabilities": [], "labels": {}, "max_concurrent_runs": 1}, expected=201)
    credential = ctx.api(f"/executors/{executor_id}/credentials", {"label": "Lifecycle restore fencing"}, expected=201)
    heartbeat = {"protocol_version": 2, "active_run_ids": [], "health": {"fixture": True}}
    ctx.request(ctx.origin, f"/api/v1/agent/executors/{executor_id}/heartbeat", token=credential["token"], body=heartbeat)
    auth_tokens = copy.deepcopy(ctx.session_tokens)
    assert auth_tokens["access_token"] == ctx.token and auth_tokens["refresh_token"], "harness did not retain the current fixture session"
    state = {
        "project_ids": [project_id, private["id"]],
        "thread_ids": [legacy_thread, chat_thread, foreign_thread, schedule_thread, waiting_thread],
        "completed_run_ids": [run["spec"]["id"] for run in [legacy, legacy_next, first, follow, foreign]],
        "chat_thread": chat_thread, "workspace_id": workspace, "follow_script": follow_script,
        "artifact_run_id": first["spec"]["id"], "artifact_id": artifact["id"], "artifact_marker": chat_marker,
        "volume_samples": [{"storage_id": legacy["spec"]["id"], "marker": legacy_marker}, {"storage_id": workspace, "marker": chat_marker}],
        "schedule_id": schedule["id"], "schedule_paused": False,
        "waiting_id": waiting["spec"]["id"], "waiting_spec": waiting["spec"], "waiting_state": "waiting_for_snapshot",
        "auth_tokens": auth_tokens, "executor_id": executor_id, "credential": credential, "heartbeat": heartbeat,
        "resources": _resources(ctx), "secret_hashes": _secret_hashes(ctx),
    }
    row = _schedule(ctx, state)
    state["schedule_content"] = {key: row[key] for key in ["task_id", "project_id", "thread_id", "cron_expression", "timezone", "executor_target", "input", "model_profile_id", "created_by"]}
    state["schedule_next_run_at"] = row["next_run_at"]
    for run in [legacy, legacy_next, first, follow, foreign]:
        binding = _binding(ctx, run)
        storage_id = run["spec"]["id"] if binding == "legacy" else binding
        assert run["result"]["sandbox"]["workspace_volume"] == state["resources"]["volume_prefix"] + _uuid(storage_id).hex, "runner volume disagrees with frozen binding"
    state["content"] = _content(ctx, state)
    assert state["content"]["messages"], "conversation preservation fixture has no stored messages"
    first_messages = [message for message in state["content"]["messages"] if message["run_id"] == first["spec"]["id"]]
    assert {"user", "assistant"} <= {message["role"] for message in first_messages if message["content"]}, "completed chat fixture lacks stored user and assistant content"
    state["bindings"] = _bindings(ctx, state)
    _assert_volumes(ctx, state)
    _assert_waiting(ctx, state)
    ctx.check("lifecycle_encrypted_artifact_enabled_schedule_unfinished_run_and_live_credentials_created")
    return state


def assert_current_workspace_checks(ctx, state):
    """Read-only source/promotion/rollback checks, before submitting new jobs."""
    assert _content(ctx, state) == state["content"], "fixture project/chat/completed history changed"
    assert _bindings(ctx, state) == state["bindings"], "frozen workspace identity changed"
    assert _secret_hashes(ctx)["storage_master_key"] == state["secret_hashes"]["storage_master_key"], "data decryption key changed"
    _assert_volumes(ctx, state)
    _assert_waiting(ctx, state)
    _assert_schedule(ctx, state, paused=state["schedule_paused"])
    assert ctx.api(f"/runs/{state['artifact_run_id']}/artifacts/{state['artifact_id']}", raw=True) == state["artifact_marker"].encode(), "restored encrypted attachment did not decrypt"
    ctx.check("lifecycle_current_generation_preserves_legacy_chat_files_bindings_content_and_schedule_state")


def assert_restored_workspace_checks(ctx, state, restore_receipt):
    """Verify a fresh fenced generation, then execute one real chat follow-up."""
    resources = _resources(ctx)
    assert resources["generation"] == restore_receipt["id"], "restore activated a different generation"
    for name in ["database", "bucket", "volume_prefix"]:
        assert resources[name] != state["resources"][name], "restore reused source resource: " + name
    hashes = _secret_hashes(ctx)
    assert hashes["storage_master_key"] == state["secret_hashes"]["storage_master_key"], "restore changed the data decryption key"
    for name in SECRET_NAMES[1:]:
        assert hashes[name] != state["secret_hashes"][name], "restore did not rotate runtime credential: " + name
    tokens = state["auth_tokens"]
    ctx.request(ctx.origin, "/api/v1/projects", token=tokens["access_token"], expected=401)
    ctx.request(ctx.origin, "/api/v1/auth/refresh", body={"refresh_token": tokens["refresh_token"]}, expected=401)
    ctx.request(ctx.origin, f"/api/v1/agent/executors/{state['executor_id']}/heartbeat", token=state["credential"]["token"], body=state["heartbeat"], expected=401)
    session_id = str(_uuid(tokens["session_id"]))
    credential_id = str(_uuid(state["credential"]["credential_id"]))
    assert ctx.sql(f"SELECT revoked_at IS NOT NULL AND revoke_reason='instance_restored' FROM auth_sessions WHERE id='{session_id}';").strip() == "t", "source session lacks the restore fence"
    assert ctx.sql(f"SELECT revoked_at IS NOT NULL FROM executor_credentials WHERE id='{credential_id}' AND executor_id='{state['executor_id']}';").strip() == "t", "source executor credential survived restore"
    ctx.login()
    ctx.api("/projects")
    assert ctx.session_tokens["session_id"] != session_id, "restore login reused the revoked session"
    state["waiting_state"] = "interrupted"
    state["schedule_paused"] = True
    assert_current_workspace_checks(ctx, state)
    ctx.check("lifecycle_restore_rotates_resources_and_credentials_revokes_old_access_refresh_and_executor_and_pauses_work")
    follow = _completed(ctx, state["chat_thread"], state["follow_script"])
    assert _binding(ctx, follow) == state["workspace_id"], "post-restore follow-up changed frozen workspace identity"
    volume = resources["volume_prefix"] + _uuid(state["workspace_id"]).hex
    assert follow["result"]["sandbox"]["workspace_volume"] == volume, "post-restore runner used an old generation volume"
    assert ctx.read_workspace(volume, MARKER_FILE) == state["artifact_marker"].encode(), "post-restore runner lost chat files"
    state["completed_run_ids"].append(follow["spec"]["id"])
    # This new baseline includes only the deliberate, verified follow-up above.
    state["content"] = _content(ctx, state)
    state["bindings"] = _bindings(ctx, state)
    ctx.check("lifecycle_post_restore_real_runner_followup_uses_new_volume_with_unchanged_chat_binding")


def assert_candidate_schedule_paused(ctx, state):
    """Call inside the owning update harness's candidate probe, before promote."""
    _assert_schedule(ctx, state, paused=True)
    if not state["schedule_paused"]:
        ident = str(_uuid(state["schedule_id"]))
        assert ctx.sql(f"SELECT count(*) FROM cowork_upgrade_schedules WHERE id='{ident}';").strip() == "1", "upgrade lost the original enabled schedule set"
    ctx.check("lifecycle_candidate_probe_cannot_dispatch_source_schedule")


def arm_schedule_for_upgrade_checks(ctx, state):
    """Explicitly re-enable this one reviewed fixture before upgrade checks."""
    row = _schedule(ctx, state)
    future = datetime.now(timezone.utc) + timedelta(days=30)
    cron = f"0 0 {future.day} {future.month} *"
    schedule = ctx.api(f"/schedules/{state['schedule_id']}", {
        "expected_revision": row["revision"], "cron": cron, "timezone": "UTC",
        "executor_target": row["executor_target"], "input": row["input"],
        "model_profile_id": row["model_profile_id"], "enabled": True,
    }, method="PUT")
    assert schedule["enabled"] is True and schedule["blocked_reason"] is None, "reviewed schedule did not re-enable"
    canonical_cron = "0 " + cron  # The server prepends seconds to five-field input.
    assert schedule["cron"] == canonical_cron and schedule["timezone"] == "UTC", "schedule API changed the requested cron or timezone"
    next_run = datetime.fromisoformat(schedule["next_run_at"].replace("Z", "+00:00"))
    intended_run = datetime(future.year, future.month, future.day, tzinfo=timezone.utc)
    assert next_run.utcoffset() == timedelta(0) and next_run == intended_run, "schedule dispatch date or UTC midnight changed"
    assert next_run > datetime.now(timezone.utc) + timedelta(days=1), "fixture schedule is too close to dispatch"
    stored = _schedule(ctx, state)
    assert stored["cron_expression"] == schedule["cron"] and stored["timezone"] == schedule["timezone"], "stored canonical schedule differs from API response"
    stored_run = datetime.fromisoformat(stored["next_run_at"].replace("Z", "+00:00"))
    assert stored_run == next_run, "stored dispatch time differs from API response"
    state["schedule_content"]["cron_expression"] = stored["cron_expression"]
    state["schedule_content"]["timezone"] = stored["timezone"]
    state["schedule_paused"] = False
    state["schedule_next_run_at"] = stored["next_run_at"]
    _assert_schedule(ctx, state, paused=False)
    ctx.check("lifecycle_reviewed_fixture_schedule_rearmed_for_enabled_source_upgrade_pause_check")


def capture_activation_checks(ctx, state):
    """Capture the exact active source before a deliberately rejected tamper."""
    directory = Path(ctx.directory)
    files = {}
    for name in ["active.json", "compose.json"]:
        path = directory / name
        files[name] = hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
    session_id = str(_uuid(state["auth_tokens"]["session_id"]))
    credential_id = str(_uuid(state["credential"]["credential_id"]))
    waiting_id = str(_uuid(state["waiting_id"]))
    return {
        "files": files, "resources": _resources(ctx), "content": _content(ctx, state),
        "bindings": _bindings(ctx, state), "secrets": _secret_hashes(ctx),
        "schedule": _schedule(ctx, state),
        "waiting": _json_sql(ctx, f"SELECT to_jsonb(r) FROM runs r WHERE id='{waiting_id}';"),
        "session_fence": _json_sql(ctx, f"SELECT jsonb_build_object('revoked_at',revoked_at,'revoke_reason',revoke_reason,'expires_at',expires_at) FROM auth_sessions WHERE id='{session_id}';"),
        "credential_fence": _json_sql(ctx, f"SELECT jsonb_build_object('revoked_at',revoked_at,'revoke_reason',revoke_reason,'expires_at',expires_at) FROM executor_credentials WHERE id='{credential_id}';"),
    }


def assert_activation_unchanged(ctx, state, before):
    """A rejected archive must leave its source pointer/data/credentials intact."""
    assert capture_activation_checks(ctx, state) == before, "rejected tamper changed the active source before validation completed"
    _assert_volumes(ctx, state)
    ctx.check("lifecycle_tampered_archive_rejected_before_source_activation_or_data_change")
