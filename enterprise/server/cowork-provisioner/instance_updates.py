"""Backup-first Compose updates; candidate generations never overwrite source data."""
import hashlib
import hmac
import json
import re
from pathlib import Path
import time
import uuid
import execution_generation

from fleet_images import retain_fleet_images
from instance_backups import InstanceBackups, compact, pg_environment
from release_catalog import ReleaseCatalog, ReleaseError, image_set_id


PROMOTE_SCHEDULES_SQL = b"""
BEGIN;
UPDATE schedules s SET enabled=TRUE,next_run_at=p.next_run_at,blocked_reason=NULL,
 revision=s.revision+1,etag='W/"' || s.id::text || ':' || (s.revision+1)::text || '"',updated_at=now()
 FROM cowork_upgrade_schedules p WHERE s.id=p.id;
WITH changed AS (
 UPDATE sync_entities e SET payload=e.payload || jsonb_build_object('enabled',TRUE,'next_run_at',s.next_run_at,
  'blocked_reason',s.blocked_reason,'canonical_revision',s.revision,'updated_at',s.updated_at),
  revision=e.revision+1,etag='W/"' || e.entity_id::text || ':' || (e.revision+1)::text || '"',updated_at=now()
 FROM schedules s JOIN cowork_upgrade_schedules p ON p.id=s.id
 WHERE e.entity_type='schedule' AND e.entity_id=s.id AND NOT e.tombstone
 RETURNING e.user_id,e.entity_type,e.entity_id,e.revision,e.payload
)
INSERT INTO sync_changes(user_id,entity_type,entity_id,revision,operation,payload)
 SELECT user_id,entity_type,entity_id,revision,'upsert',payload FROM changed;
DROP TABLE cowork_upgrade_schedules;
COMMIT;
"""


class InstanceUpdates:
    def __init__(self, infrastructure):
        if infrastructure.config.get('executor_host') is not None:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.enabled(infrastructure.config)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise ReleaseError(str(error)) from None
        self.infra = infrastructure
        self.run = infrastructure.run
        self.write = infrastructure.private_write
        self.catalog = ReleaseCatalog(infrastructure.config, self.write)

    def require_remote_operation(self, operation):
        if self.infra.config.get('executor_host') is not None:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.operation(self.infra, operation)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise ReleaseError(str(error)) from None

    def execute(self, operation, directory):
        self.require_remote_operation(operation)
        instance = operation["instance"]
        arguments = operation["arguments"]
        import backend_profile
        if instance["backend"] != "compose" or not (instance["policy"]["minimum_isolation"] == "container" or backend_profile.vm_instance(self.infra.config,instance)) or instance["state"] != "stopped":
            raise ReleaseError("upgrade_requires_stopped_compose_container_instance")
        canary = arguments.get("qualification_only", False)
        if type(canary) is not bool:
            raise ReleaseError("invalid_release_qualification_mode")
        operation_id = operation["id"]
        compact(operation_id)
        execution_generation.prepare(self.infra,operation,directory)
        active = self.infra.active_directory(directory)
        source_images = self.infra.deployed_images(directory)
        source_id = image_set_id(source_images)
        if "source_release" in arguments and arguments["source_release"] != source_id:
            raise ReleaseError("installed_release_changed")
        release = self.catalog.release(arguments.get("release"), source_images)
        from execution_runtime import probe
        probe(self.infra, instance, active, release['images'])
        if release["id"] == source_id:
            raise ReleaseError("release_already_deployed")
        if not canary:
            self.catalog.require_qualification(release["id"], source_id,operation)
        # Imports/tags are restricted to immutable operator-approved images.
        # Do this before any backup, credentials or active pointer are changed.
        retain_fleet_images(source_images, self.run, lambda value: value)
        retain_fleet_images(release["images"], self.run, lambda value: value)
        backups = InstanceBackups(self.infra)
        # A qualified transition names the exact source image set, including a
        # legacy deployed release that is no longer the fleet's default.
        backups.config = {**backups.config, "images": source_images}
        backups.quiescent(instance, active)
        marker = directory / ("upgrade-" + compact(operation_id) + ".json")
        if marker.exists() or marker.is_symlink():
            raise ReleaseError("upgrade_started_without_completion")
        previous_pointer = (directory / "active.json").read_bytes() if (directory / "active.json").exists() else None
        previous_compose = (directory / "compose.json").read_bytes()
        state = {"version": 1, "operation_id": operation_id, "instance_id": instance["id"], "organisation_id": instance["organisation_id"],
                 "source_release": source_id, "release": release["id"], "qualification_only": canary,
                 "previous_pointer": previous_pointer.decode() if previous_pointer is not None else None,
                 "previous_compose": json.loads(previous_compose), "phase": "started"}
        if operation.get('execution_binding') is not None:
            state['execution_binding'] = operation['execution_binding']
        for key in ['executor_host_binding', 'host_allocation','backend_profile']:
            if operation.get(key) is not None:
                state[key] = operation[key]
        def checkpoint(phase):
            state["phase"] = phase
            self.write(marker, json.dumps(state, sort_keys=True).encode())
        checkpoint("started")
        receipt = {"receipt_id": operation_id, "instance_id": instance["id"], "source_release": source_id, "release": release["id"],
                   "qualification_only": canary, "instance_state": "stopped"}
        try:
            receipt.update(backups.backup(operation, directory, active))
            state["backup"] = {key: receipt[key] for key in ["backup_id", "backup_sha256", "backup_bytes", "manifest_version"]}
            checkpoint("backed_up")
            backup = {"id": receipt["backup_id"], "instance_id": instance["id"], "organisation_id": instance["organisation_id"],
                      "sha256": receipt["backup_sha256"], "size_bytes": receipt["backup_bytes"]}
            restoration = {**operation, "arguments": {"backup_id": receipt["backup_id"]}, "backup": backup}
            backups.restore(restoration, directory, active, activate=False, upgrade=True)
            candidate = directory / "generations" / compact(operation_id)
            self.write(candidate / "images.json", json.dumps(release["images"]).encode())
            checkpoint("staged")
            self.write(directory / "active.json", json.dumps({"generation": operation_id}).encode())
            self.infra.write_compose(instance, directory)
            checkpoint("probing")
            self.probe(instance, directory, candidate, worker=True)
            self.infra.stop_compose(instance, directory)
            backups.quiescent(instance, candidate)
            if canary:
                self.restore_previous(instance, directory, active, previous_pointer, previous_compose, backups)
                checkpoint("verifying_rollback")
                # Do not run the source worker: its schedules retain their live
                # settings, while the candidate's dispatch sources were fenced.
                self.probe(instance, directory, active, worker=False)
                self.infra.stop_compose(instance, directory)
                backups.quiescent(instance, active)
                receipt["rollback_verified"] = True
                self.catalog.qualify(receipt,operation)
            else:
                environment = pg_environment((candidate / "secrets" / "database_url").read_text().strip(), self.infra.config)
                self.run(["psql", "-X", "-v", "ON_ERROR_STOP=1"], env=environment, data=PROMOTE_SCHEDULES_SQL)
                self.write(candidate / "release.json", json.dumps(receipt, sort_keys=True).encode())
                receipt["generation"] = operation_id
            state["receipt"] = receipt
            checkpoint("completed")
            return receipt
        except Exception as error:
            state["failure_phase"] = state["phase"]
            # Persist only stable domain codes, never exception payloads, command
            # arguments or paths. This makes failed staging diagnosable safely.
            code = str(error)
            state["failure_code"] = code if type(error).__name__ in ["ReleaseError", "BackupError", "ProvisioningError"] and re.fullmatch(r"[a-z0-9_:.-]{1,120}", code) else type(error).__name__
            try:
                self.restore_previous(instance, directory, active, previous_pointer, previous_compose, backups)
                checkpoint("rolled_back")
            except Exception as recovery_error:
                checkpoint("reconciliation_required")
                raise ReleaseError("upgrade_rollback_requires_review", result={key: value for key, value in receipt.items() if key.startswith("backup_") or key == "manifest_version"}) from recovery_error
            raise ReleaseError("upgrade_failed_source_preserved", instance_state="stopped",
                               result={key: value for key, value in receipt.items() if key.startswith("backup_") or key == "manifest_version"}) from error

    def restore_previous(self, instance, directory, active, pointer, compose, backups):
        self.infra.stop_compose(instance, directory)
        # Verify the candidate is quiescent before switching any active resource.
        backups.quiescent(instance, self.infra.active_directory(directory))
        from instance_gateway import validate_manifest, GatewayError
        try:
            validate_manifest(self.infra.config, instance, json.loads(compose))
        except GatewayError as error:
            raise ReleaseError(str(error)) from None
        if pointer is None:
            (directory / "active.json").unlink(missing_ok=True)
        else:
            self.write(directory / "active.json", pointer)
        self.write(directory / "compose.json", compose)
        backups.quiescent(instance, active)

    def recover(self, operation):
        """Explicit reconciliation restores the recorded source; never rerun staging."""
        self.require_remote_operation(operation)
        instance = operation["instance"]
        directory = self.infra.directory(instance["id"])
        marker = directory / (operation["action"] + "-" + compact(operation["id"]) + ".json")
        if not marker.exists():
            # A preflight rejection can precede the first update checkpoint.
            if image_set_id(self.infra.deployed_images(directory)) != operation["arguments"].get("source_release"):
                raise ReleaseError("upgrade_rollback_requires_review")
            self.infra.stop_compose(instance, directory)
            InstanceBackups(self.infra).quiescent(instance, self.infra.active_directory(directory))
            return {}
        state = self.catalog.read(marker)
        expected = {"operation_id": operation["id"], "instance_id": instance["id"], "organisation_id": instance["organisation_id"],
                    "release": operation["arguments"]["release"], "source_release": operation["arguments"].get("source_release"),
                    "qualification_only": operation["arguments"].get("qualification_only", False)}
        if operation["action"] == "rollback":
            expected["rollback_operation_id"] = operation["arguments"].get("rollback_operation_id")
        expected['execution_binding'] = execution_generation.binding(operation)
        for key in ['executor_host_binding', 'host_allocation','backend_profile']:
            if operation.get(key) is not None:
                expected[key] = operation[key]
        if (not isinstance(state, dict) or type(state.get("version")) is not int or state["version"] != 1
                or any(state.get(key) != value for key, value in expected.items())
                or state.get("phase") not in ["started", "backed_up", "staged", "probing", "verifying_rollback", "completed", "rolled_back", "reconciliation_required", "recovered"]):
            raise ReleaseError("upgrade_rollback_requires_review")
        source = self.recorded_source(instance, directory, state)
        pointer, previous = state["previous_pointer"], state["previous_compose"]
        self.restore_previous(instance, directory, source, pointer.encode() if pointer is not None else None,
                              json.dumps(previous).encode(), InstanceBackups(self.infra))
        if image_set_id(self.infra.deployed_images(directory)) != state["source_release"]:
            raise ReleaseError("upgrade_rollback_requires_review")
        state["phase"] = "recovered"
        self.write(marker, json.dumps(state, sort_keys=True).encode())
        return state.get("backup", {})

    def recorded_source(self, instance, directory, state):
        pointer = state["previous_pointer"]
        source = directory
        if pointer is not None:
            parsed = json.loads(pointer)
            if not isinstance(parsed, dict) or set(parsed) != {"generation"}:
                raise ReleaseError("upgrade_rollback_requires_review")
            source = directory / "generations" / compact(parsed["generation"])
            if source.is_symlink() or self.catalog.read(source / "generation.json")["generation"] != parsed["generation"]:
                raise ReleaseError("upgrade_rollback_requires_review")
        if image_set_id(self.catalog.read(source / "images.json")) != state["source_release"]:
            raise ReleaseError("upgrade_rollback_requires_review")
        previous = state["previous_compose"]
        scope = previous["services"]["api"]["environment"]
        if scope["COWORK_INSTANCE_ID"] != instance["id"] or scope["COWORK_ORGANISATION_ID"] != instance["organisation_id"]:
            raise ReleaseError("upgrade_rollback_requires_review")
        if state.get('execution_binding') is not None:
            from execution_runtime import validate_manifest
            validate_manifest(self.infra.config, previous)
            expected_epoch = state['execution_binding']['source_epoch']
            if execution_generation.read(instance,source) != expected_epoch or any(
                    previous['services'][service]['environment'].get('COWORK_EXECUTION_EPOCH') != expected_epoch for service in ['api','worker']):
                raise ReleaseError('upgrade_execution_source_requires_review')
        return source

    def probe(self, instance, directory, active, *, worker):
        original = (directory / "compose.json").read_bytes()
        manifest = json.loads(original)
        # Neither the candidate nor the rollback verification may accept user
        # traffic while migrations and checks are in progress. Retain only the
        # private runner/database/storage networks during the probe.
        manifest["services"]["api"]["networks"].pop("routing", None)
        if self.infra.config.get('executor_host') is not None:
            manifest['services']['api'].pop('ports', None)
        self.write(directory / "compose.json", json.dumps(manifest).encode())
        try:
            return self._probe_services(instance, directory, active, worker=worker)
        finally:
            self.infra.stop_compose(instance, directory)
            InstanceBackups(self.infra).quiescent(instance, active)
            self.write(directory / "compose.json", original)

    def _probe_services(self, instance, directory, active, *, worker):
        services = ["api", "runner", "egress-proxy", *(["worker"] if worker else [])]
        self.infra.compose(instance, directory, "up", "--detach", "--wait", "--wait-timeout", "90", *services)
        # --wait covers configured API/runner/proxy health checks. Keep observing
        # the worker long enough to detect immediate configuration/startup exits.
        for _ in range(3):
            rows = self.infra.compose(instance, directory, "ps", "--format", "json").decode()
            entries = json.loads(rows) if rows.lstrip().startswith("[") else [json.loads(line) for line in rows.splitlines() if line.strip()]
            states = {entry["Service"]: entry for entry in entries}
            if any(states.get(service, {}).get("State") != "running" for service in services):
                raise ReleaseError("release_service_probe_failed")
            time.sleep(2)
        run_id = str(uuid.uuid4())
        marker = "cowork-release-probe-" + run_id
        spec = {"schema_version": 2, "run_id": run_id, "image": "core", "network": "none", "environment": {}, "stdin_base64": None,
                "argv": ["python3", "-c", "from pathlib import Path; p=Path('release-probe.txt'); p.write_text('" + marker + "'); print(p.read_text())"],
                "limits": {"cpu_nanos": 100000000, "memory_bytes": 128 * 1024 * 1024, "pids": 64, "timeout_seconds": 20, "tmpfs_bytes": 4 * 1024 * 1024, "output_bytes": 1024}}
        body = json.dumps(spec, separators=(",", ":")).encode()
        timestamp, nonce = str(int(time.time())), str(uuid.uuid4())
        key = (active / "secrets" / "runner_signing_key").read_bytes().strip()
        signature = hmac.new(key, timestamp.encode() + b"\n" + nonce.encode() + b"\nPOST\n/v1/jobs\n" + body, hashlib.sha256).hexdigest()
        try:
            api_id = self.infra.compose(instance, directory, "ps", "--quiet", "api").decode().strip()
            if not api_id or "\n" in api_id:
                raise ReleaseError("release_api_probe_unavailable")
            response = self.run(["docker", "exec", "--interactive", api_id, "curl", "--fail", "--silent", "--max-time", "30", "--request", "POST",
                                 "--header", "content-type: application/json", "--header", "x-cowork-timestamp: " + timestamp,
                                 "--header", "x-cowork-nonce: " + nonce, "--header", "x-cowork-signature: " + signature,
                                 "--data-binary", "@-", "http://runner:8090/v1/jobs"], data=body)
            result = json.loads(response)
            if result.get("run_id") != run_id or result.get("exit_code") != 0 or result.get("timed_out") is not False or result.get("stdout", "").strip() != marker:
                raise ReleaseError("release_sandbox_probe_failed")
            return {"run_id":run_id,"instance_id":instance['id'],"spec_sha256":hashlib.sha256(body).hexdigest(),
                "output_sha256":hashlib.sha256(result['stdout'].encode()).hexdigest(),"exit_code":0,"timed_out":False,
                "image":self.infra.config['images']['sandbox_core'],"limits":spec['limits']}
        finally:
            # Remove only this freshly generated probe's container and volume.
            name = "cowork-" + compact(instance["id"]) + "-job-" + compact(run_id)
            try:
                self.run(["docker", "rm", "--force", name])
            except Exception:
                pass
            volume = self.infra.resource_names(instance, active)["volume_prefix"] + compact(run_id)
            try:
                self.run(["docker", "volume", "rm", volume])
            except Exception:
                pass
