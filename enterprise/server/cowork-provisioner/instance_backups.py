"""Stopped-instance backups; restore into a fresh generation before activation.

Only operator-owned code calls this module. No workload commands, host paths or
object-store administration credentials are accepted from the management API.
"""
from __future__ import annotations

from datetime import datetime, timezone
from dataclasses import dataclass
import hashlib
import execution_generation
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tarfile
import tempfile
from urllib.parse import unquote, urlsplit
import uuid


class BackupError(Exception):
    pass


RESTORE_FENCE_SQL = b"""
BEGIN;
UPDATE auth_sessions SET revoked_at=now(),revoke_reason='instance_restored' WHERE revoked_at IS NULL;
UPDATE executor_credentials SET revoked_at=now() WHERE revoked_at IS NULL;
UPDATE reauthentication_grants SET used_at=now() WHERE used_at IS NULL;
UPDATE terminal_stream_tickets SET used_at=now() WHERE used_at IS NULL;
UPDATE desktop_stream_tickets SET used_at=now() WHERE used_at IS NULL;
UPDATE terminal_sessions SET state='ended',ended_at=now(),failure='instance_restored_requires_review' WHERE state NOT IN ('ended','failed');
UPDATE desktop_sessions SET state='ended',ended_at=now() WHERE state NOT IN ('ended','failed');
UPDATE runs SET state='interrupted',lease_expires_at=NULL,lease_token=NULL,lease_owner=NULL,updated_at=now(),revision=revision+1
 WHERE state NOT IN ('completed','failed','canceled','expired');
UPDATE schedules SET enabled=FALSE,next_run_at=NULL,blocked_reason='instance_restored_requires_review',revision=revision+1,
 etag='W/"' || id::text || ':' || (revision+1)::text || '"',updated_at=now() WHERE enabled;
WITH changed AS (
 UPDATE sync_entities e SET payload=e.payload || jsonb_build_object('enabled',FALSE,'next_run_at',NULL,
   'blocked_reason',s.blocked_reason,'canonical_revision',s.revision,'updated_at',s.updated_at),
   revision=e.revision+1,etag='W/"' || e.entity_id::text || ':' || (e.revision+1)::text || '"',updated_at=now()
 FROM schedules s WHERE e.entity_type='schedule' AND e.entity_id=s.id AND NOT e.tombstone
   AND s.blocked_reason='instance_restored_requires_review'
 RETURNING e.user_id,e.entity_type,e.entity_id,e.revision,e.payload
)
INSERT INTO sync_changes(user_id,entity_type,entity_id,revision,operation,payload)
 SELECT user_id,entity_type,entity_id,revision,'upsert',payload FROM changed;
COMMIT;
"""


def compact(value: str) -> str:
    try:
        parsed = uuid.UUID(value)
    except (ValueError, TypeError, AttributeError) as error:
        raise BackupError("invalid_backup_identifier") from error
    if str(parsed) != value:
        raise BackupError("noncanonical_backup_identifier")
    return parsed.hex


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def pg_environment(url: str, config=None) -> dict:
    value = urlsplit(url)
    environment = {**os.environ, "PGHOST": value.hostname or "", "PGPORT": str(value.port or 5432),
            "PGUSER": unquote(value.username or ""), "PGPASSWORD": unquote(value.password or ""),
            "PGDATABASE": unquote(value.path.lstrip("/"))}
    if config is not None:
        import guest_endpoints
        try:
            return guest_endpoints.controller_pg_environment(config, url, environment)
        except guest_endpoints.GuestEndpointError as error:
            raise BackupError(str(error)) from None
    return environment


def validate_manifest(manifest: dict, instance: dict, backup_id: str, images: dict) -> None:
    if (not isinstance(manifest, dict) or type(manifest.get("schema_version")) is not int
            or manifest.get("schema_version") != 1 or manifest.get("backup_id") != backup_id
            or manifest.get("instance_id") != instance["id"] or manifest.get("organisation_id") != instance["organisation_id"]):
        raise BackupError("backup_scope_or_format_mismatch")
    # An image change is an upgrade and requires a separately qualified migration.
    if manifest.get("images") != images:
        raise BackupError("backup_release_not_qualified")
    files = manifest.get("files")
    if not isinstance(files, dict) or not {"database.dump", "instance.json", "secrets/storage_master_key", "secrets/bootstrap_token", "secrets/runner_signing_key"} <= files.keys():
        raise BackupError("backup_manifest_incomplete")
    for name, value in files.items():
        if not isinstance(name, str):
            raise BackupError("invalid_backup_manifest_entry")
        path = PurePosixPath(name)
        if (path.is_absolute() or ".." in path.parts or "\\" in name or str(path) != name
                or not name or len(name) > 2048 or name == "manifest.json"
                or not isinstance(value, dict) or type(value.get("bytes")) is not int or value["bytes"] < 0
                or not isinstance(value.get("sha256"), str) or not re.fullmatch("[0-9a-f]{64}", value["sha256"])):
            raise BackupError("invalid_backup_manifest_entry")
    runs = manifest.get("workspace_runs")
    if not isinstance(runs, list) or any(not isinstance(run, str) for run in runs) or len(set(runs)) != len(runs):
        raise BackupError("invalid_backup_workspace")
    for run in runs:
        if not re.fullmatch("[0-9a-f]{32}", run) or f"workspaces/{run}.tar" not in files:
            raise BackupError("invalid_backup_workspace")
    if {name for name in files if name.startswith("workspaces/")} != {
            f"workspaces/{run}.tar" for run in runs}:
        raise BackupError("backup_workspace_inventory_mismatch")


def _verified_tar_tail(source, path):
    # tarfile stops at the first end marker. Reject hidden entries after it and
    # incomplete record/footer bytes, rather than accepting an unread suffix.
    size = path.stat().st_size
    if size % tarfile.BLOCKSIZE or size - source.offset < 2 * tarfile.BLOCKSIZE:
        raise BackupError("backup_tar_truncated")
    with path.open("rb") as stream:
        stream.seek(source.offset)
        while chunk := stream.read(1024 * 1024):
            if any(chunk):
                raise BackupError("backup_tar_trailing_content")


def _workspace_inventory(path):
    """Read every inner file before any DB, bucket, generation or volume write."""
    entries, types = [], {}
    try:
        with tarfile.open(path, "r:") as source:
            for entry in source:
                name = entry.name
                # GNU tar -C workspace . captures a directory root and ./names.
                # Keep that real capture format, but detect canonical aliases.
                if name.startswith("./"):
                    name = name[2:]
                if name == "." or not name:
                    if entry.name not in (".", "./") or not entry.isdir() or entry.size != 0 or "." in types:
                        raise BackupError("unsafe_backup_workspace_entry")
                    types["."] = True
                    continue
                relative = PurePosixPath(name)
                if (relative.is_absolute() or str(relative) != name or ".." in relative.parts
                        or "\\" in name or len(name.encode("utf-8")) > 2048 or name in types
                        or entry.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE)
                        or entry.size < 0 or (entry.isdir() and entry.size != 0)):
                    raise BackupError("unsafe_backup_workspace_entry")
                if any(str(parent) in types and not types[str(parent)] for parent in relative.parents):
                    raise BackupError("unsafe_backup_workspace_entry")
                if not entry.isdir() and any(prior.startswith(name + "/") for prior in types):
                    raise BackupError("unsafe_backup_workspace_entry")
                types[name] = entry.isdir()
                value = {"path": name, "directory": entry.isdir(), "bytes": entry.size, "mode": entry.mode & 0o777}
                if entry.isfile():
                    checksum = hashlib.sha256()
                    remaining = entry.size
                    with source.extractfile(entry) as content:
                        while remaining:
                            chunk = content.read(min(1024 * 1024, remaining))
                            if not chunk:
                                raise BackupError("backup_workspace_truncated")
                            checksum.update(chunk)
                            remaining -= len(chunk)
                    value["sha256"] = checksum.hexdigest()
                entries.append(value)
            _verified_tar_tail(source, path)
    except (tarfile.TarError, OSError, ValueError) as error:
        raise BackupError("invalid_backup_workspace_archive") from error
    return sorted(entries, key=lambda entry: entry["path"])


def unpack_verified(archive: Path, destination: Path, instance: dict, backup_id: str, images: dict) -> dict:
    """Never extract archive-provided links or paths with tarfile.extractall."""
    with tarfile.open(archive, "r:") as source:
        first = source.next()
        if first is None or first.name != "manifest.json" or not first.isfile() or first.size > 32 * 1024 * 1024:
            raise BackupError("backup_manifest_missing")
        try:
            manifest = json.load(source.extractfile(first))
        except (ValueError, UnicodeError) as error:
            raise BackupError("invalid_backup_manifest") from error
        validate_manifest(manifest, instance, backup_id, images)
        found = set()
        for entry in source:
            if entry is first:
                continue
            expected = manifest["files"].get(entry.name)
            if not entry.isfile() or expected is None or entry.name in found or entry.size != expected["bytes"]:
                raise BackupError("backup_archive_content_mismatch")
            target = destination / entry.name
            if not target.resolve().is_relative_to(destination.resolve()):
                raise BackupError("backup_archive_path_escape")
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with target.open("xb") as output:
                os.chmod(target, 0o600)
                shutil.copyfileobj(source.extractfile(entry), output, length=1024 * 1024)
            if digest(target) != expected["sha256"]:
                raise BackupError("backup_file_integrity_failed")
            found.add(entry.name)
        if found != manifest["files"].keys():
            raise BackupError("backup_archive_incomplete")
        _verified_tar_tail(source, archive)
        # The manifest lists files, so a new instance's empty object/workspace
        # trees have no archive entries. Restore still needs valid source dirs.
        for name in ["objects", "workspaces"]:
            path = destination / name
            if path.exists() and not path.is_dir():
                raise BackupError("backup_payload_directory_invalid")
            path.mkdir(mode=0o700, exist_ok=True)
        return manifest


@dataclass(frozen=True, repr=False)
class VerifiedBackup:
    manifest: dict
    payload: Path
    cipher_sha256: str
    cipher_bytes: int
    manifest_sha256: str
    workspace_inventory: dict


def _catalog(catalog, instance, backup_id):
    compact(backup_id)
    if (not isinstance(catalog, dict) or catalog.get("id") != backup_id
            or catalog.get("instance_id") != instance["id"]
            or catalog.get("organisation_id") != instance["organisation_id"]
            or type(catalog.get("size_bytes")) is not int or catalog["size_bytes"] <= 0
            or not isinstance(catalog.get("sha256"), str)
            or not re.fullmatch("[a-f0-9]{64}", catalog["sha256"])):
        raise BackupError("backup_catalog_integrity_failed")


def _cipher_stat(metadata):
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise BackupError("backup_catalog_integrity_failed")
    return (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns, metadata.st_ctime_ns)


def _bounded_cipher(source, size, destination=None):
    source.seek(0)
    checksum, remaining = hashlib.sha256(), size
    while remaining:
        chunk = source.read(min(1024 * 1024, remaining))
        if not chunk:
            raise BackupError("backup_catalog_integrity_failed")
        checksum.update(chunk)
        if destination is not None:
            destination.write(chunk)
        remaining -= len(chunk)
    if source.read(1):
        raise BackupError("backup_catalog_integrity_failed")
    return checksum.hexdigest()


def verify_backup(archive, catalog, provenance_instance, backup_id, images, identity, run, scratch, *, manifest_sha256=None):
    """Bind actual cipher bytes to the original catalog before age decryption.

    age authenticates its envelope; it is not an origin/manifest signature. The
    immutable managed catalog is the provenance authority for this verifier.
    All extraction happens in private scratch, never in the target generation.
    """
    _catalog(catalog, provenance_instance, backup_id)
    archive, scratch = Path(archive), Path(scratch)
    copied = scratch / "verified-cipher.age"
    plain = scratch / "verified-backup.tar"
    payload = scratch / "verified-payload"
    try:
        if archive.is_symlink() or scratch.is_symlink() or not scratch.is_dir():
            raise BackupError("backup_catalog_integrity_failed")
        original = _cipher_stat(archive.lstat())
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0)
        with os.fdopen(os.open(archive, flags), "rb") as source:
            if _cipher_stat(os.fstat(source.fileno())) != original or original[2] != catalog["size_bytes"]:
                raise BackupError("backup_catalog_integrity_failed")
            output_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
            with os.fdopen(os.open(copied, output_flags, 0o600), "wb") as output:
                measured = _bounded_cipher(source, catalog["size_bytes"], output)
                output.flush()
                os.fsync(output.fileno())
            if measured != catalog["sha256"] or _cipher_stat(os.fstat(source.fileno())) != original:
                raise BackupError("backup_catalog_integrity_failed")
            # Only the private O_EXCL copy with the verified catalog hash is ever
            # passed to age. A change-and-restore of the source during decrypt
            # cannot substitute other decrypted bytes.
            os.chmod(copied, 0o400)
            try:
                run(["age", "--decrypt", "-i", identity, "-o", str(plain), str(copied)], timeout=21600)
            except Exception:
                raise BackupError("backup_decryption_failed") from None
            payload.mkdir(mode=0o700)
            manifest = unpack_verified(plain, payload, provenance_instance, backup_id, images)
            original_manifest = hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
            if manifest_sha256 is not None and original_manifest != manifest_sha256:
                raise BackupError("backup_original_manifest_changed")
            instance_path = payload / "instance.json"
            if instance_path.stat().st_size > 1024 * 1024:
                raise BackupError("backup_instance_scope_mismatch")
            try:
                archived_instance = json.loads(instance_path.read_text(encoding="utf-8"))
            except (ValueError, UnicodeError):
                raise BackupError("backup_instance_scope_mismatch") from None
            if (not isinstance(archived_instance, dict) or archived_instance.get("id") != provenance_instance["id"]
                    or archived_instance.get("organisation_id") != provenance_instance["organisation_id"]):
                raise BackupError("backup_instance_scope_mismatch")
            workspaces = {run_id: _workspace_inventory(payload / "workspaces" / (run_id + ".tar"))
                          for run_id in manifest["workspace_runs"]}
            if (_cipher_stat(os.fstat(source.fileno())) != original or archive.is_symlink()
                    or _cipher_stat(archive.lstat()) != original
                    or _bounded_cipher(source, catalog["size_bytes"]) != measured):
                raise BackupError("backup_cipher_changed")
            return VerifiedBackup(manifest, payload, measured, catalog["size_bytes"], original_manifest, workspaces)
    except (tarfile.TarError, OSError, ValueError) as error:
        raise BackupError("backup_archive_invalid") from error


class InstanceBackups:
    def __init__(self, infrastructure):
        if infrastructure.config.get('executor_host') is not None:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.enabled(infrastructure.config)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise BackupError(str(error)) from None
        self.infra = infrastructure
        self.quiesced = False
        self.config = infrastructure.config
        self.run = infrastructure.run
        self.write = infrastructure.private_write
        self.root = Path(self.config.get("backup_directory", ""))
        if not self.root.is_absolute() or self.root.is_symlink() or self.root.resolve() == Path(self.root.anchor):
            raise BackupError("backup_directory_not_configured")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.identity = self.config.get("backup_identity_file")
        if not self.identity:
            raise BackupError("backup_identity_not_configured")
        self.recipient = self.run(["age-keygen", "-y", self.identity]).decode().strip()
        if not re.fullmatch("age1[a-z0-9]+", self.recipient):
            raise BackupError("unsupported_backup_identity")
        recipient_file = self.root / "recipient.txt"
        if recipient_file.exists():
            if recipient_file.is_symlink() or recipient_file.read_text().strip() != self.recipient:
                raise BackupError("backup_recovery_identity_mismatch")
        else:
            if any(self.root.glob("*.age")):
                raise BackupError("backup_recipient_missing_requires_reconciliation")
            self.write(recipient_file, (self.recipient + "\n").encode())

    def quiescent(self, instance: dict, active: Path) -> None:
        import backend_profile
        if instance["backend"] != "compose" or not (instance["policy"]["minimum_isolation"] == "container" or backend_profile.vm_instance(self.config,instance)):
            raise BackupError("backup_backend_not_qualified")
        if instance["state"] != "stopped":
            raise BackupError("backup_restore_requires_stopped_instance")
        prefix = "cowork-" + compact(instance["id"]) + "-"
        running=self.run(["docker", "ps", "--filter", "name=^/" + prefix, "--format", "{{.Names}}"] ).decode().splitlines()
        dedicated=self.config.get('instance_disk_quotas') is not None
        if any(not dedicated or not re.fullmatch(re.escape(prefix)+r'(?:postgres|minio)-1',n) for n in running):
            raise BackupError("instance_workloads_not_quiescent")
        if self.config.get('executor_host') is not None:
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.quiescent(self.infra, instance)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise BackupError(str(error)) from None
        resources = self.infra.resource_names(instance, active)
        env = pg_environment(Path(self.config["database_admin_url_file"]).read_text().strip(), self.config)
        connections = self.run(["psql", "-X", "-tAc", f"SELECT count(*) FROM pg_stat_activity WHERE datname='{resources['database']}';"], env=env)
        if connections.strip() != b"0":
            raise BackupError("instance_database_not_quiescent")

    def storage(self, *arguments: str) -> bytes:
        import guest_endpoints
        try:
            environment = guest_endpoints.controller_storage_environment(self.config)
        except guest_endpoints.GuestEndpointError as error:
            raise BackupError(str(error)) from None
        return self.run(["mc", "--config-dir", self.config["s3_admin_config_directory"], *arguments], timeout=21600,
                        **({'env': environment} if environment is not None else {}))

    def read_object(self, name, destination, bound):
        from instance_import_target import read_object
        return read_object(self, name, destination, bound)

    def capture_import_volume(self, operation, generation, run_id, destination):
        from remote_volume_streaming import VolumeStreamer
        return VolumeStreamer(self.infra, operation).capture_import_target(generation, run_id, destination)

    def require_remote_operation(self, operation):
        if self.config.get('executor_host') is not None:
            import remote_lifecycle_qualification
            try:
                return remote_lifecycle_qualification.operation(self.infra, operation)
            except remote_lifecycle_qualification.LifecycleError as error:
                raise BackupError(str(error)) from None

    def volume_archive(self, volume: str, temp: Path, name: str, restore=False, *, operation=None, directory=None, expected=None) -> None:
        if self.config.get('executor_host') is not None:
            import remote_volume_streaming
            import remote_lifecycle_qualification
            try:
                remote_lifecycle_qualification.operation(self.infra, operation)
                streamer = remote_volume_streaming.VolumeStreamer(self.infra, operation)
                suffix = name.removesuffix('.tar')
                if name != suffix + '.tar' or streamer.scope(directory, suffix, restore=restore) != volume:
                    raise BackupError('remote_workspace_scope_mismatch')
                if restore:
                    return streamer.restore(directory, suffix, temp / name, expected)
                return streamer.capture(directory, suffix, temp / name)
            except (remote_volume_streaming.StreamError, remote_lifecycle_qualification.LifecycleError) as error:
                raise BackupError(str(error)) from None
        # This trusted tar helper only sees one instance volume and the staging
        # directory. It does not receive a socket, network, secrets or host root.
        helper = volume.split("-run-")[0] + "-snapshot-" + uuid.uuid4().hex
        command = ["docker", "run", "--rm", "--name", helper, "--network", "none", "--read-only", "--user", "0:0",
                   "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--pids-limit", "64",
                   "--memory", "256m", "--cpus", "1", "--cap-add", "DAC_OVERRIDE"]
        if restore:
            command += ["--cap-add", "CHOWN", "--cap-add", "FOWNER", "--cap-add", "FSETID"]
        command += ["--mount", f"type=volume,src={volume},dst=/workspace" + ("" if restore else ",readonly"),
                    "--mount", f"type=bind,src={temp},dst=/backup" + (",readonly" if restore else ""),
                    self.config["images"]["sandbox_core"], "tar", "--numeric-owner"]
        command += (["-xpf", "/backup/" + name, "-C", "/workspace"] if restore else ["-cpf", "/backup/" + name, "-C", "/workspace", "."])
        try:
            self.run(command, timeout=21600)
        finally:
            # A timed-out Docker client can leave its container alive.
            try:
                self.run(["docker", "rm", "--force", helper])
            except Exception:
                # A subsequent quiescence check rejects surviving helpers.
                pass

    def execute(self, operation: dict, directory: Path) -> dict:
        instance = operation["instance"]
        compact(instance["organisation_id"])
        active = self.infra.active_directory(directory)
        # A qualified update can advance this instance independently of the
        # fleet default. Restore still requires exactly its installed image set.
        self.config = {**self.config, "images": self.infra.deployed_images(directory)}
        self.quiescent(instance, active)
        self.quiesced = True
        if operation["action"] == "backup":
            return self.backup(operation, directory, active)
        return self.restore(operation, directory, active)

    def backup(self, operation: dict, directory: Path, active: Path) -> dict:
        self.require_remote_operation(operation)
        instance, backup_id = operation["instance"], operation["id"]
        archive = self.root / (compact(backup_id) + ".age")
        metadata = archive.with_suffix(".json")
        if metadata.exists() or archive.exists():
            raise BackupError("backup_already_exists_reconcile_receipt")
        resources = self.infra.resource_names(instance, active)
        deployed_images = self.infra.deployed_images(directory)
        with tempfile.TemporaryDirectory(prefix="capture-", dir=self.root) as temporary:
            temp = Path(temporary)
            self.write(temp / "owner.json", json.dumps({"instance_id":instance["id"],"organisation_id":instance["organisation_id"],"operation_id":operation["id"]}).encode())
            payload = temp / "payload"
            payload.mkdir(mode=0o700)
            (payload / "objects").mkdir(mode=0o700)
            (payload / "workspaces").mkdir(mode=0o700)
            (payload / "secrets").mkdir(mode=0o700)
            # Snapshot credentials are inside the age envelope, never the catalog.
            for secret in ["bootstrap_token", "runner_signing_key", "storage_master_key"]:
                value = (active / "secrets" / secret).read_bytes()
                self.write(payload / "secrets" / secret, value)
            self.write(payload / "instance.json", json.dumps(instance, sort_keys=True).encode())
            env = pg_environment((active / "secrets" / "database_url").read_text().strip(), self.config)
            self.run(["pg_dump", "--format=custom", "--no-owner", "--no-privileges", "--file", str(payload / "database.dump")], env=env, timeout=21600)
            self.storage("mirror", self.config["s3_admin_alias"] + "/" + resources["bucket"], str(payload / "objects"))
            names = self.run(["docker", "volume", "ls", "--filter", "name=^" + resources["volume_prefix"], "--format", "{{.Name}}"] ).decode().splitlines()
            workspace_runs = []
            if self.config.get('executor_host') is not None:
                import remote_lifecycle_qualification
                limits = remote_lifecycle_qualification.enabled(self.config)
                if len(names) > limits['max_workspace_archives'] or len(names) != len(set(names)):
                    raise BackupError('remote_workspace_inventory_exceeds_bound')
            workspace_bytes = 0
            for volume in names:
                suffix = volume.removeprefix(resources["volume_prefix"])
                if not re.fullmatch("[0-9a-f]{32}", suffix):
                    raise BackupError("unrecognised_workspace_volume")
                if self.config.get('executor_host') is not None:
                    self.volume_archive(volume, payload / "workspaces", suffix + ".tar", operation=operation, directory=active)
                    workspace_bytes += (payload / 'workspaces' / (suffix + '.tar')).stat().st_size
                    if workspace_bytes > limits['max_workspace_archive_bytes']:
                        raise BackupError('remote_workspace_archive_total_exceeds_bound')
                else:
                    self.volume_archive(volume, payload / "workspaces", suffix + ".tar")
                workspace_runs.append(suffix)
            files = {}
            for path in sorted(payload.rglob("*")):
                if path.is_symlink():
                    raise BackupError("unexpected_backup_symlink")
                if path.is_file():
                    files[path.relative_to(payload).as_posix()] = {"bytes": path.stat().st_size, "sha256": digest(path)}
            manifest = {"schema_version": 1, "backup_id": backup_id, "organisation_id": instance["organisation_id"],
                        "instance_id": instance["id"], "created_at": datetime.now(timezone.utc).isoformat(),
                        "images": deployed_images, "workspace_runs": workspace_runs, "files": files}
            # New captures preserve original epoch and the complete quiescent
            # source history. Legacy manifests remain valid for normal restore.
            if instance.get('execution_epoch') is not None:
                origin_epoch = execution_generation.identifier(instance['execution_epoch'])
                run_ids = json.loads(self.run(['psql', '-X', '-tAc',
                    "SELECT COALESCE(json_agg(id ORDER BY id),'[]'::json) FROM runs;"], env=env))
                if not isinstance(run_ids, list) or len(run_ids) > 100000 or run_ids != sorted(set(run_ids)):
                    raise BackupError('backup_original_run_inventory_invalid')
                for run_id in run_ids:
                    execution_generation.identifier(run_id)
                manifest['import_provenance'] = {'version': 1, 'origin_epoch': origin_epoch, 'run_ids': run_ids}
            if self.config.get('executor_host') is not None:
                manifest['executor_host_binding'] = operation['executor_host_binding']
                manifest['remote_fixture_id'] = limits['fixture_id']
            self.write(payload / "manifest.json", json.dumps(manifest, sort_keys=True).encode())
            plain = temp / "backup.tar"
            with tarfile.open(plain, "w") as output:
                output.add(payload / "manifest.json", arcname="manifest.json", recursive=False)
                for name in files:
                    output.add(payload / name, arcname=name, recursive=False)
            encrypted = temp / "backup.age"
            self.run(["age", "--encrypt", "-r", self.recipient, "-o", str(encrypted), str(plain)], timeout=21600)
            os.chmod(encrypted, 0o600)
            # The same catalog, source and inner-workspace checks used by import
            # and restore qualify the actually encrypted artifact before publish.
            verification = temp / "verification"
            verification.mkdir(mode=0o700)
            catalog = {"id": backup_id, "instance_id": instance["id"], "organisation_id": instance["organisation_id"],
                       "sha256": digest(encrypted), "size_bytes": encrypted.stat().st_size}
            verified = verify_backup(encrypted, catalog, instance, backup_id, deployed_images, self.identity, self.run, verification)
            if digest(verification / "verified-backup.tar") != digest(plain):
                raise BackupError("backup_encryption_verification_failed")
            receipt = {"backup_id": backup_id, "backup_sha256": verified.cipher_sha256, "backup_bytes": verified.cipher_bytes, "manifest_version": 1}
            with encrypted.open("rb") as stream:
                os.fsync(stream.fileno())
            os.replace(encrypted, archive)
            self.write(metadata, json.dumps({**receipt, "organisation_id": instance["organisation_id"], "instance_id": instance["id"]}).encode())
            return receipt

    def restore(self, operation: dict, directory: Path, active: Path, *, activate=True, upgrade=False, source_import=None) -> dict:
        self.require_remote_operation(operation)
        instance = operation["instance"]
        provenance_instance = instance
        if source_import is not None:
            import instance_imports
            frozen = instance_imports.binding(operation)
            if operation['action'] != 'import_restore' or source_import != frozen:
                raise BackupError('invalid_separate_import_binding')
            provenance_instance = frozen['source']
        backup_id = operation["arguments"]["backup_id"]
        archive = self.root / (compact(backup_id) + ".age")
        expected = operation.get("backup", {})
        _catalog(expected, provenance_instance, backup_id)
        generation = operation["id"]
        stage = directory / "generations" / compact(generation)
        if stage.exists():
            raise BackupError("restore_generation_exists_reconcile_receipt")
        with tempfile.TemporaryDirectory(prefix="restore-", dir=self.root) as temporary:
            temp = Path(temporary)
            self.write(temp / "owner.json", json.dumps({"instance_id":instance["id"],"organisation_id":instance["organisation_id"],"operation_id":operation["id"]}).encode())
            verified_images = self.config['images'] if source_import is None else operation['arguments']['import_export']['images']
            expected_manifest = None if source_import is None else operation['arguments']['import_export']['manifest_sha256']
            verified = verify_backup(archive, expected, provenance_instance, backup_id, verified_images,
                                     self.identity, self.run, temp, manifest_sha256=expected_manifest)
            manifest, payload = verified.manifest, verified.payload
            if source_import is not None:
                import instance_imports
                instance_imports.verify_export(operation, manifest)
                from instance_import_target import original_provenance
                original_provenance(verified)  # Unsupported import metadata fails before any target write.
            if self.config.get('executor_host') is not None:
                import remote_lifecycle_qualification
                import remote_volume_streaming
                try:
                    remote_lifecycle_qualification.validate_workspaces(self.infra, operation, payload, manifest)
                except (remote_lifecycle_qualification.LifecycleError, remote_volume_streaming.StreamError) as error:
                    raise BackupError(str(error)) from None
            from execution_runtime import probe
            restore_images = manifest['images'] if source_import is None else self.config['images']
            probe(self.infra, instance, active, restore_images)
            # Everything above is read-only toward the instance. New resources
            # remain unreachable until the final atomic generation pointer write.
            stage.mkdir(mode=0o700, parents=True)
            execution_generation.stage(self.infra,operation,stage)
            self.write(stage / "generation.json", json.dumps({"generation": generation}).encode())
            (stage / "secrets").mkdir(mode=0o700)
            # Retain only the data decryption key. Runtime/bootstrap credentials
            # are freshly generated, so old signed control requests cannot replay.
            self.write(stage / "secrets" / "storage_master_key", (payload / "secrets" / "storage_master_key").read_bytes())
            values = self.infra.ensure_secrets(instance, stage)
            # Records the qualified release before switching the active resources.
            self.write(stage / "images.json", json.dumps(restore_images).encode())
            self.infra.ensure_database(instance, values, stage)
            env = pg_environment(values["database_url"], self.config)
            self.run(["pg_restore", "--exit-on-error", "--no-owner", "--no-privileges", "--dbname", env["PGDATABASE"], str(payload / "database.dump")], env=env, timeout=21600)
            # Restoring history must not replay external actions or revive old
            # credentials. Schedules and unfinished runs require an explicit review.
            fence = RESTORE_FENCE_SQL
            if upgrade:
                # The clone remains unable to dispatch schedules while its new
                # API/worker are probed. Promotion restores the enabled set.
                fence = fence.replace(b"UPDATE schedules SET", b"CREATE TABLE cowork_upgrade_schedules AS SELECT id,next_run_at FROM schedules WHERE enabled;\nUPDATE schedules SET", 1)
            self.run(["psql", "-X", "-v", "ON_ERROR_STOP=1"], env=env, data=fence)
            if source_import is not None:
                import instance_imports
                self.run(['psql','-X','-v','ON_ERROR_STOP=1'], env=env, data=instance_imports.origin_fence(operation))
            self.infra.ensure_bucket(instance, values, stage)
            resources = self.infra.resource_names(instance, stage)
            self.storage("mirror", str(payload / "objects"), self.config["s3_admin_alias"] + "/" + resources["bucket"])
            for run_id in manifest["workspace_runs"]:
                if self.config.get('executor_host') is not None:
                    self.volume_archive(resources['volume_prefix'] + run_id, payload / 'workspaces', run_id + '.tar', restore=True,
                                        operation=operation, directory=stage, expected=manifest['files']['workspaces/' + run_id + '.tar'])
                else:
                    self.volume_archive(resources["volume_prefix"] + run_id, payload / "workspaces", run_id + ".tar", restore=True)
            restored = {"backup_id": backup_id, "previous_generation": self.infra.resource_names(instance, active)["generation"]}
            if source_import is not None:
                restored.update(import_binding_sha256=instance_imports.canonical(source_import),
                    original_manifest_sha256=verified.manifest_sha256, origin_epoch=original_provenance(verified)['origin_epoch'],
                    restore_operation_id=operation['id'])
            self.write(stage / "restore.json", json.dumps(restored).encode())
            self.write(stage / "generation.json", json.dumps({"generation": generation, "launch_not_before": int(datetime.now(timezone.utc).timestamp())}).encode())
            target = None
            if source_import is not None:
                from instance_import_target import verify_target
                readback = temp / 'target-readback'
                readback.mkdir(mode=0o700)
                target = verify_target(self, operation, directory, stage, verified, readback, before_activation=True)
            # Policies remain at their current (possibly stricter) revision.
            if activate:
                self.write(directory / "active.json", json.dumps({"generation": generation}).encode())
                self.infra.write_compose(instance, directory)
            return {"backup_id": backup_id, **({'import_target': target} if target is not None else {})}
