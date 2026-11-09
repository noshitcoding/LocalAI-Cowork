"""Restore the verified pre-update backup, never run old code on a newer DB."""
import json
import execution_generation

from fleet_images import retain_fleet_images
from instance_backups import InstanceBackups, compact
from instance_updates import InstanceUpdates
from release_catalog import ReleaseError, image_set_id


class InstanceRollbacks(InstanceUpdates):
    def promotion(self, operation, directory):
        instance, arguments = operation["instance"], operation["arguments"]
        promotion_id = arguments.get("rollback_operation_id")
        state = self.catalog.read(directory / ("upgrade-" + compact(promotion_id) + ".json"))
        receipt = state.get("receipt", {}) if isinstance(state, dict) else {}
        expected = {"operation_id": promotion_id, "instance_id": instance["id"],
                    "organisation_id": instance["organisation_id"], "phase": "completed",
                    "qualification_only": False, "source_release": arguments.get("release"),
                    "release": arguments.get("source_release")}
        if operation.get('backend_profile') is not None:expected['backend_profile']=operation['backend_profile']
        if (not isinstance(state, dict) or type(state.get("version")) is not int or state["version"] != 1 or any(type(state.get(k)) is not type(v) or state[k] != v for k, v in expected.items())
                or receipt.get("generation") != promotion_id or receipt.get("receipt_id") != promotion_id
                or receipt.get("backup_id") != promotion_id or receipt.get("qualification_only") is not False
                or receipt.get("release") != state["release"] or receipt.get("source_release") != state["source_release"]
                or receipt.get("instance_id") != instance["id"] or receipt.get("instance_state") != "stopped"):
            raise ReleaseError("rollback_promotion_evidence_invalid")
        active = self.infra.active_directory(directory)
        if (active != directory / "generations" / compact(promotion_id)
                or self.catalog.read(active / "generation.json").get("generation") != promotion_id
                or image_set_id(self.infra.deployed_images(directory)) != state["release"]):
            raise ReleaseError("rollback_source_generation_changed")
        source = self.recorded_source(instance, directory, state)
        images = self.catalog.read(source / "images.json")
        if not self.catalog.enabled(state["source_release"]):
            raise ReleaseError("rollback_release_withdrawn")
        backup = operation.get("backup", {})
        if (backup.get("id") != promotion_id or backup.get("instance_id") != instance["id"]
                or backup.get("organisation_id") != instance["organisation_id"]
                or backup.get("sha256") != receipt.get("backup_sha256")):
            raise ReleaseError("rollback_backup_evidence_invalid")
        return images

    def execute(self, operation, directory):
        self.require_remote_operation(operation)
        instance, arguments = operation["instance"], operation["arguments"]
        import backend_profile
        if instance["backend"] != "compose" or not (instance["policy"]["minimum_isolation"] == "container" or backend_profile.vm_instance(self.infra.config,instance)) or instance["state"] != "stopped":
            raise ReleaseError("rollback_requires_stopped_compose_container_instance")
        operation_id = operation["id"]
        execution_generation.prepare(self.infra,operation,directory)
        marker = directory / ("rollback-" + compact(operation_id) + ".json")
        if marker.exists() or marker.is_symlink():
            raise ReleaseError("rollback_started_without_completion")
        images = self.promotion(operation, directory)
        from execution_runtime import probe
        probe(self.infra, instance, self.infra.active_directory(directory), images)
        current_images = self.infra.deployed_images(directory)
        for image_set in [current_images, images]:
            retain_fleet_images(image_set, self.run, lambda value: value)
        active = self.infra.active_directory(directory)
        backups = InstanceBackups(self.infra)
        backups.config = {**backups.config, "images": current_images}
        backups.quiescent(instance, active)
        previous_pointer = (directory / "active.json").read_bytes()
        previous_compose = (directory / "compose.json").read_bytes()
        state = {"version": 1, "operation_id": operation_id, "instance_id": instance["id"],
                 "organisation_id": instance["organisation_id"], "source_release": arguments["source_release"],
                 "release": arguments["release"], "rollback_operation_id": arguments["rollback_operation_id"],
                 "qualification_only": False, "previous_pointer": previous_pointer.decode(),
                 "previous_compose": json.loads(previous_compose)}
        if operation.get('execution_binding') is not None:
            state['execution_binding'] = operation['execution_binding']
        for key in ['executor_host_binding', 'host_allocation','backend_profile']:
            if operation.get(key) is not None:
                state[key] = operation[key]
        def checkpoint(phase):
            state["phase"] = phase
            self.write(marker, json.dumps(state, sort_keys=True).encode())
        checkpoint("started")
        receipt = {"receipt_id": operation_id, "instance_id": instance["id"], "source_release": arguments["source_release"],
                   "release": arguments["release"], "rollback_operation_id": arguments["rollback_operation_id"],
                   "restored_backup_id": operation["backup"]["id"], "instance_state": "stopped"}
        try:
            receipt.update(backups.backup(operation, directory, active))
            state["backup"] = {k: receipt[k] for k in ["backup_id", "backup_sha256", "backup_bytes", "manifest_version"]}
            checkpoint("backed_up")
            # Restore accepts only the exact images captured by the original
            # backup. Schema, files and keys travel together into fresh resources.
            backups.config = {**backups.config, "images": images}
            backups.restore({**operation, "arguments": {"backup_id": operation["backup"]["id"]}}, directory, active, activate=False)
            candidate = directory / "generations" / compact(operation_id)
            checkpoint("staged")
            self.write(directory / "active.json", json.dumps({"generation": operation_id}).encode())
            self.infra.write_compose(instance, directory)
            checkpoint("probing")
            self.probe(instance, directory, candidate, worker=True)
            self.infra.stop_compose(instance, directory)
            backups.quiescent(instance, candidate)
            # Historical sessions are revoked and schedules remain disabled by
            # restore's fence. Rollback never replays work done since the update.
            receipt["generation"] = operation_id
            self.write(candidate / "release.json", json.dumps(receipt, sort_keys=True).encode())
            state["receipt"] = receipt
            checkpoint("completed")
            return receipt
        except Exception as error:
            try:
                self.restore_previous(instance, directory, active, previous_pointer, previous_compose, backups)
                checkpoint("rolled_back")
            except Exception as recovery_error:
                checkpoint("reconciliation_required")
                raise ReleaseError("rollback_recovery_requires_review", result=state.get("backup", {})) from recovery_error
            raise ReleaseError("rollback_failed_source_preserved", instance_state="stopped", result=state.get("backup", {})) from error
