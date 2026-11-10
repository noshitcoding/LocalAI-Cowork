"""Real operator acceptance; only the dedicated, stopped backup test instance."""
import argparse
import json
from pathlib import Path
import uuid

import provisioner as p
from instance_backups import InstanceBackups, pg_environment
from instance_updates import InstanceUpdates
from release_catalog import ReleaseCatalog, ReleaseError, image_set_id


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--request", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    config = json.loads(Path(args.config).read_text())
    if config.get('executor_host') is not None:
        raise p.ProvisioningError('remote_executor_lifecycle_not_qualified')
    request = json.loads(Path(args.request).read_text())
    instance = request["instance"]
    if instance["id"] != "9fa8d4c9-cf1f-4e67-a8e8-d65156589ca3" or instance["name"] != "Backup-Abnahme 2026-09-25T06:17:23.242Z" or instance["state"] != "stopped":
        raise RuntimeError("only_the_dedicated_stopped_acceptance_instance_is_allowed")
    config["release_directory"] = str(Path(config["state_directory"]).parent / "releases")
    with p.operator_lock(Path(config["state_directory"])):
        infra = p.Infrastructure(config)
        directory = infra.directory(instance["id"])
        original_active = infra.active_directory(directory)
        original_pointer = (directory / "active.json").read_bytes() if (directory / "active.json").exists() else None
        original_compose = (directory / "compose.json").read_bytes()
        images = infra.deployed_images(directory)
        target = {**images, "runner": p.immutable_image(request["runner_image"])}
        catalog = ReleaseCatalog(config, p.private_write)
        release = catalog.publish("Admission runner acceptance", target, [image_set_id(images)])
        backups = InstanceBackups(infra)
        backups.quiescent(instance, original_active)
        def counts(active):
            env = pg_environment((active / "secrets" / "database_url").read_text().strip())
            return p.run(["psql", "-X", "-tAc", "SELECT json_build_object('projects',(SELECT count(*) FROM projects),'runs',(SELECT count(*) FROM runs),'enabled_schedules',(SELECT count(*) FROM schedules WHERE enabled));"], env=env).strip()
        baseline = counts(original_active)
        checks, receipts = [], []
        def passed(name):
            checks.append(name)
            print(name + "=ok", flush=True)
        def operation(canary):
            return {"id": str(uuid.uuid4()), "action": "upgrade", "instance": instance,
                    "arguments": {"release": release["id"], "qualification_only": canary}}
        engine = InstanceUpdates(infra)
        original_services = engine._probe_services
        def isolated_services(*values, **keywords):
            original_services(*values, **keywords)
            api_id = infra.compose(instance, directory, "ps", "--quiet", "api").decode().strip()
            networks = json.loads(infra.run(["docker", "inspect", api_id, "--format", "{{json .NetworkSettings.Networks}} "]))
            assert config["routing_network"] not in networks, "probe exposed through instance gateway"
        engine._probe_services = isolated_services
        try:
            original_probe = engine.probe
            def failing_probe(*values, **keywords):
                original_probe(*values, **keywords)
                raise ReleaseError("injected_post_probe_failure")
            engine.probe = failing_probe
            failed = operation(True)
            try:
                engine.execute(failed, directory)
                raise AssertionError("injected failure was accepted")
            except ReleaseError as error:
                assert str(error) == "upgrade_failed_source_preserved" and error.instance_state == "stopped"
                assert error.result["backup_id"] == failed["id"]
            assert (directory / "compose.json").read_bytes() == original_compose
            assert infra.active_directory(directory) == original_active
            assert counts(original_active) == baseline
            passed("failed-candidate-preserves-source-database-and-release")
            engine.probe = original_probe
            qualification = engine.execute(operation(True), directory)
            receipts.append(qualification)
            assert qualification["rollback_verified"] is True
            assert infra.active_directory(directory) == original_active
            assert counts(original_active) == baseline
            catalog.require_qualification(release["id"], image_set_id(images))
            passed("real-backup-clone-runner-worker-and-rollback-qualification")
            promoted = engine.execute(operation(False), directory)
            receipts.append(promoted)
            active = infra.active_directory(directory)
            assert active != original_active and infra.deployed_images(directory) == target
            assert counts(active) == baseline
            assert counts(original_active) == baseline
            assert (active / "secrets" / "database_url").read_bytes() != (original_active / "secrets" / "database_url").read_bytes()
            assert (active / "secrets" / "storage_master_key").read_bytes() == (original_active / "secrets" / "storage_master_key").read_bytes()
            passed("qualified-promotion-uses-separate-data-and-preserves-schedules")
        finally:
            # This test does not deploy a new release to any long-lived instance.
            engine.restore_previous(instance, directory, original_active, original_pointer, original_compose, backups)
        assert infra.active_directory(directory) == original_active
        assert infra.deployed_images(directory) == images
        assert counts(original_active) == baseline
        passed("acceptance-restores-original-fixture")
        p.private_write(Path(args.output), json.dumps({"instance_id": instance["id"], "release": release["id"], "checks": checks, "receipts": receipts}, indent=2).encode())


if __name__ == "__main__":
    main()
