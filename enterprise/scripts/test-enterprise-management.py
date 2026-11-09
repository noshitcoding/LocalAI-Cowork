#!/usr/bin/env python3
"""Disposable PostgreSQL + real HTTP management acceptance (no infrastructure mocks claimed as VM acceptance)."""
import argparse
import base64
import concurrent.futures
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def totp(secret):
    key=base64.b32decode(secret + '=' * ((8-len(secret)%8)%8))
    digest=hmac.new(key,(int(time.time())//30).to_bytes(8,'big'),hashlib.sha1).digest()
    index=digest[-1]&15
    return f"{(int.from_bytes(digest[index:index+4],'big')&0x7fffffff)%1000000:06d}"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", required=True)
    parser.add_argument("--provisioning-releases", action="store_true")
    parser.add_argument("--deletion", action="store_true")
    parser.add_argument("--execution-admission", action="store_true")
    parser.add_argument("--execution-generation", action="store_true")
    parser.add_argument("--execution-recovery", action="store_true")
    parser.add_argument("--execution-coverage", action="store_true")
    parser.add_argument("--execution-enrollment", action="store_true")
    parser.add_argument("--postgres-bin", default="/usr/lib/postgresql/16/bin")
    args = parser.parse_args()
    pg = Path(args.postgres_bin)
    with tempfile.TemporaryDirectory(prefix="cowork-enterprise-test-") as temp:
        root = Path(temp)
        data = root / "pg"
        db_port, api_port = free_port(), free_port()
        while db_port == api_port:
            api_port = free_port()
        audit_key = secrets.token_bytes(32)
        (root / "audit-key").write_bytes(audit_key)
        broker_key, bootstrap = secrets.token_hex(32), secrets.token_hex(32)
        (root / "broker-key").write_text(broker_key)
        subprocess.run([str(pg / "initdb"), "-D", str(data), "-A", "trust", "-U", "postgres", "--no-sync"], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([str(pg / "pg_ctl"), "-D", str(data), "-l", str(root / "postgres.log"), "-o", f"-h 127.0.0.1 -p {db_port} -k {temp}", "start", "-w"], check=True, stdout=subprocess.DEVNULL)
        process = None
        try:
            env = {key: value for key, value in os.environ.items() if not key.startswith("COWORK_") and key != "DATABASE_URL"}
            env.update({"DATABASE_URL": f"postgres://postgres@127.0.0.1:{db_port}/postgres", "COWORK_MODE": "management", "COWORK_AUTH_MASTER_KEY": base64.b64encode(secrets.token_bytes(32)).decode(),
                        "COWORK_LISTEN_ADDR": f"127.0.0.1:{api_port}", "COWORK_BOOTSTRAP_TOKEN": bootstrap,
                        "COWORK_MANAGEMENT_AUDIT_KEY_FILE": str(root / "audit-key"), "COWORK_PROVISIONER_TOKEN_FILE": str(root / "broker-key"),
                        "COWORK_WEB_PUSH_ENABLED": "false", "COWORK_MANAGEMENT_BACKUPS_ENABLED": "true", "COWORK_MANAGEMENT_UPDATES_ENABLED": "true"})
            if args.provisioning_releases: env["COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED"]="true"
            if args.deletion or args.execution_recovery: env["COWORK_MANAGEMENT_DELETION_ENABLED"] = "true"
            if args.execution_enrollment: env['COWORK_MANAGEMENT_EXECUTION_ENROLLMENT_ENABLED']='true'
            if args.execution_admission or args.execution_generation or args.execution_recovery:
                (root / 'execution-ceiling').write_text(json.dumps({'platform_concurrent_runs': 4, 'organisation_concurrent_runs': 3, 'user_concurrent_runs': 2}))
                env['COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED'] = 'true'
                env['COWORK_MANAGEMENT_EXECUTION_CEILING_FILE'] = str(root / 'execution-ceiling')
            log = (root / "server.log").open("wb")
            process = subprocess.Popen([str(Path(args.server).resolve())], env=env, stdout=log, stderr=log)
            origin = f"http://127.0.0.1:{api_port}"

            def request(path, payload=None, token=None, method=None, expected=200, broker=False, instance_key=None, origin_override=None):
                headers = {"Content-Type": "application/json"}
                if token:
                    headers["Authorization"] = "Bearer " + token
                if broker:
                    headers["x-cowork-provisioner-token"] = broker_key
                if instance_key:
                    headers["x-cowork-instance-key"] = instance_key
                body = None if payload is None else json.dumps(payload).encode()
                req = Request((origin_override or origin) + path, body, headers, method=method or ("GET" if body is None else "POST"))
                try:
                    with urlopen(req, timeout=30) as response:
                        status, content = response.status, response.read()
                except HTTPError as error:
                    status, content = error.code, error.read()
                if status != expected:
                    # Authentication responses contain credentials even when only the status is unexpected.
                    detail = "<redacted authentication response>" if "/auth/" in path else repr(content[:300])
                    raise AssertionError(f"{req.method} {path}: expected {expected}, got {status}: {detail}")
                if not content:
                    return None
                if status >= 400:
                    return content.decode(errors="replace")
                return json.loads(content)

            for attempt in range(100):
                if process.poll() is not None:
                    raise RuntimeError((root / "server.log").read_text()[-8000:])
                try:
                    request("/readyz")
                    break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError("server readiness timeout")
            admin = request("/api/v1/auth/bootstrap", {"email": "admin@enterprise.test", "display_name": "Administrator", "password": "Test-Password-42-Long", "device_id": str(uuid.uuid4())}, bootstrap, expected=201)
            token = admin["access_token"]
            setup=request("/api/v1/auth/totp/setup", {}, token)
            request("/api/v1/auth/totp/enable", {"code":totp(setup["secret"])}, token)
            def user(email):
                invitation = request("/api/v1/auth/invitations", {"email": email}, token, expected=201)
                return request("/api/v1/auth/invitations/accept", {"token": invitation["token"], "display_name": email, "password": "Test-Password-42-Long", "device_id": str(uuid.uuid4())}, expected=201)

            if args.execution_admission or args.execution_generation or args.execution_recovery or args.execution_coverage or args.execution_enrollment or args.provisioning_releases:
                if args.provisioning_releases:
                    from enterprise_provisioning_release_acceptance import run
                elif args.execution_enrollment:
                    from enterprise_execution_enrollment_http import run
                elif args.execution_coverage:
                    from enterprise_execution_coverage_acceptance import run
                elif args.execution_recovery:
                    from enterprise_execution_recovery_acceptance import run
                elif args.execution_generation:
                    from enterprise_execution_generation_acceptance import run
                else:
                    from enterprise_execution_admission_acceptance import run
                def sql(query):
                    return subprocess.check_output([str(pg / 'psql'), '-h', '127.0.0.1', '-p', str(db_port), '-U', 'postgres', '-X', '-At', '-v', 'ON_ERROR_STOP=1', '-c', query], text=True)
                replicas = []
                def replica(ceiling, error=None, admission=None, release_selection=None):
                    replica_env = dict(env)
                    if release_selection is not None: replica_env["COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED"] = str(release_selection).lower()
                    if admission is not None: replica_env['COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED'] = str(admission).lower()
                    replica_port = free_port()
                    ceiling_path = root / ('ceiling-' + str(uuid.uuid4()))
                    ceiling_path.write_text(json.dumps(ceiling))
                    replica_env['COWORK_MANAGEMENT_EXECUTION_CEILING_FILE'] = str(ceiling_path)
                    replica_env['COWORK_LISTEN_ADDR'] = f'127.0.0.1:{replica_port}'
                    replica_log = root / ('replica-' + str(replica_port) + '.log')
                    child = subprocess.Popen([str(Path(args.server).resolve())],env=replica_env,stdout=replica_log.open('wb'),stderr=subprocess.STDOUT)
                    replicas.append(child)
                    if error:
                        assert child.wait(timeout=10) != 0
                        assert error in replica_log.read_text(), 'wrong configuration failure'
                    else:
                        for _ in range(100):
                            assert child.poll() is None, replica_log.read_text()[-1000:]
                            try:
                                with urlopen(f'http://127.0.0.1:{replica_port}/readyz',timeout=1): return f'http://127.0.0.1:{replica_port}'
                            except OSError: time.sleep(.1)
                        raise AssertionError('replica readiness timeout')
                try:
                    run(request, token, user, sql, broker_key, replica)
                finally:
                    for child in replicas:
                        if child.poll() is None:
                            child.terminate(); child.wait(timeout=10)
                return

            if args.deletion:
                from enterprise_deletion_acceptance import run
                def sql(query):
                    return subprocess.check_output([str(pg / 'psql'), '-h', '127.0.0.1', '-p', str(db_port), '-U', 'postgres', '-X', '-Atc', query], text=True)
                run(request, token, user, sql)
                return

            alice, bob = user("alice@enterprise.test"), user("bob@enterprise.test")
            a = request("/management/v1/organisations", {"name": "Organisation A"}, token)["id"]
            b = request("/management/v1/organisations", {"name": "Organisation B"}, token)["id"]
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "member"}, token)
            request(f"/management/v1/organisations/{b}/members", {"email": "bob@enterprise.test", "role": "member"}, token)
            request(f"/management/v1/organisations/{b}/instances", token=alice["access_token"], expected=404)
            request(f"/management/v1/organisations/{a}/audit", token=bob["access_token"], expected=404)
            request("/management/v1/organisations", {"name": "Forbidden"}, alice["access_token"], expected=404)
            request("/management/v1/organisations", token=bootstrap, expected=401)
            request("/api/v1/projects", token=token, expected=404)
            print("organisation_auth_boundaries=ok")

            orgs = request("/management/v1/organisations", token=token)["items"]
            policy = next(org["policy"] for org in orgs if org["id"] == a)
            policy["revision"] += 1
            policy["limits"]["instances"] = 2
            request(f"/management/v1/organisations/{a}/policy", policy, token, "PUT")
            request(f"/management/v1/organisations/{a}/policy", policy, token, "PUT", expected=409)
            request(f"/management/v1/organisations/{a}/policy", {**policy, "allow_host_mounts": True}, token, "PUT", expected=422)
            request(f"/management/v1/organisations/{a}/policy", {**policy, "revision": policy["revision"] + 1, "allow_laptop_full_access": True}, token, "PUT", expected=422)
            request(f"/management/v1/organisations/{a}/instances", {"name": "Injected host", "backend": "compose", "host_mounts": ["/"]}, alice["access_token"], expected=422)

            def create(index):
                try:
                    return request(f"/management/v1/organisations/{a}/instances", {"name": f"Alice {index}", "backend": "compose"}, alice["access_token"])
                except AssertionError as error:
                    if "got 429" not in str(error):
                        raise
                    return None
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                created = [result for result in pool.map(create, range(8)) if result]
            assert len(created) == 2, created
            print("concurrent_instance_quota=ok")
            instance = created[0]["id"]
            visible = request(f"/management/v1/organisations/{a}/instances", token=alice["access_token"])
            assert all(item["can_manage"] and item["available_actions"] == ["provision"] for item in visible)
            request(f"/management/v1/organisations/{b}/instances/{instance}/operations", token=bob["access_token"], expected=404)
            operation_path = f"/management/v1/organisations/{a}/instances/{instance}/operations"
            action = {"action": "provision", "expected_revision": 1, "idempotency_key": str(uuid.uuid4()), "backup_id": None, "release": None}
            first = request(operation_path, action, alice["access_token"])
            repeat = request(operation_path, action, alice["access_token"])
            assert first["id"] == repeat["id"]
            request(operation_path, {**action, "action": "delete"}, alice["access_token"], expected=409)
            request("/management/runner/v1/claim", {"backend": "compose"}, token=token, expected=404)
            claimed = request("/management/runner/v1/claim", {"backend": "compose"}, broker=True)
            assert claimed["id"] == first["id"]
            assert set(request(operation_path, action, alice["access_token"])) == {"id", "state"}
            assert request("/management/runner/v1/claim", {"backend": "compose"}, broker=True) is None
            completion = {"lease_token": claimed["lease_token"], "succeeded": True, "result": {"receipt_id": first["id"]}}
            completion_path = f"/management/runner/v1/operations/{first['id']}/complete"
            request(completion_path, {**completion, "lease_token": str(uuid.uuid4())}, broker=True, expected=404)
            request(completion_path, completion, broker=True)
            request(completion_path, completion, broker=True)
            request(completion_path, {**completion, "succeeded": False}, broker=True, expected=409)
            print("operation_authority_idempotency_and_completion=ok (simulated provisioner receipt)")

            # A running fleet cannot acknowledge tighter rules before enforcement.
            request(f"/management/v1/organisations/{a}/policy", {**policy, "revision": policy["revision"] + 1}, token, "PUT", expected=409)
            request(f"/management/v1/organisations/{a}/members", {"email": "admin@enterprise.test", "role": "member"}, token, expected=422)
            second = created[1]["id"]
            second_path = f"/management/v1/organisations/{a}/instances/{second}/operations"
            revoked = request(second_path, {**action, "idempotency_key": str(uuid.uuid4())}, alice["access_token"])
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "auditor"}, token)
            request(second_path, {**action, "idempotency_key": str(uuid.uuid4())}, alice["access_token"], expected=404)
            assert request("/management/runner/v1/claim", {"backend": "compose"}, broker=True) is None
            revoked_result = request(second_path, token=token)[0]
            assert revoked_result["id"] == revoked["id"] and revoked_result["state"] == "failed"
            assert revoked_result["result"]["error_code"] == "request_authority_revoked"
            request(f"/management/v1/organisations/{a}/audit", token=alice["access_token"])
            request(f"/management/v1/organisations/{a}/members", token=alice["access_token"], expected=404)
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "member"}, token)
            current = next(item for item in request(f"/management/v1/organisations/{a}/instances", token=token) if item["id"] == second)
            expired = request(second_path, {**action, "expected_revision": current["revision"], "idempotency_key": str(uuid.uuid4())}, alice["access_token"])
            lease = request("/management/runner/v1/claim", {"backend": "compose"}, broker=True)
            assert lease["id"] == expired["id"]
            subprocess.run([str(pg / "psql"), "-h", "127.0.0.1", "-p", str(db_port), "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
                            f"UPDATE management_operations SET lease_expires_at=now()-interval '1 minute' WHERE id='{expired['id']}';"], check=True, stdout=subprocess.DEVNULL)
            assert request("/management/runner/v1/claim", {"backend": "compose"}, broker=True) is None
            assert request(second_path, token=token)[0]["state"] == "reconciling"
            pending_path = "/management/runner/v1/pending"
            request(pending_path, {"backend": "compose"}, token=token, expected=404)
            pending = request(pending_path, {"backend": "compose"}, broker=True)
            assert len(pending) == 1 and pending[0]["lease_token"] == lease["lease_token"]
            assert pending[0]["recovery_authorized"] is False
            recover = second_path + "/" + expired["id"] + "/recover"
            completion_path = f"/management/runner/v1/operations/{expired['id']}/complete"
            stopped = {"lease_token": lease["lease_token"], "succeeded": False, "result": {"error_code": "operation_interrupted_resources_stopped", "instance_state": "stopped"}}
            request(completion_path, stopped, broker=True, expected=422)
            request(recover, {"mode": "replay"}, alice["access_token"], expected=422)
            request(recover.replace(a, b), {"mode": "stop_if_unproven"}, token, expected=404)
            request(recover, {"mode": "stop_if_unproven"}, bob["access_token"], expected=404)
            request(recover, {"mode": "stop_if_unproven"}, alice["access_token"])
            request(recover, {"mode": "stop_if_unproven"}, alice["access_token"])
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "auditor"}, token)
            request(recover, {"mode": "stop_if_unproven"}, alice["access_token"], expected=404)
            assert request(pending_path, {"backend": "compose"}, broker=True)[0]["recovery_authorized"] is False
            request(completion_path, stopped, broker=True, expected=422)
            request(recover, {"mode": "stop_if_unproven"}, token)
            pending = request(pending_path, {"backend": "compose"}, broker=True)
            assert pending[0]["recovery_authorized"] is True
            assert pending[0]["lease_token"] == lease["lease_token"]
            request(completion_path, stopped, broker=True)
            request(completion_path, stopped, broker=True)
            request(completion_path, {"lease_token": lease["lease_token"], "succeeded": True, "result": {"receipt_id": expired["id"]}}, broker=True, expected=409)
            assert request(pending_path, {"backend": "compose"}, broker=True) == []
            current = next(item for item in request(f"/management/v1/organisations/{a}/instances", token=token) if item["id"] == second)
            assert current["state"] == "stopped"
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "member"}, token)
            print("queued_role_revocation_no_reassignment_and_authorized_recovery=ok")

            # An owner demoted from organisation admin retains normal management
            # rights, but cannot dispatch a previously queued administrative lock.
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "admin"}, token)
            current = next(item for item in request(f"/management/v1/organisations/{a}/instances", token=token) if item["id"] == instance)
            lock = request(operation_path, {**action, "action": "suspend", "expected_revision": current["revision"], "idempotency_key": str(uuid.uuid4())}, alice["access_token"])
            request(f"/management/v1/organisations/{a}/members", {"email": "alice@enterprise.test", "role": "member"}, token)
            assert request("/management/runner/v1/claim", {"backend": "compose"}, broker=True) is None
            rejected_lock = request(operation_path, token=token)[0]
            assert rejected_lock["id"] == lock["id"] and rejected_lock["state"] == "failed"
            print("queued_admin_demotion_rejected=ok")

            # Real DB/HTTP acceptance of release authority; infrastructure receipts
            # here are deliberately simulated, with Docker acceptance kept separate.
            def image_set(letter):
                return {key: f"registry.test/{key}@sha256:" + letter * 64 for key in ["server", "runner", "sandbox_core", "sandbox_gui", "sandbox_crew", "egress_proxy"]}
            def release_hash(images):
                return hashlib.sha256(json.dumps(images, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            source = release_hash(image_set("a"))
            target_images = image_set("b")
            target = release_hash(target_images)
            manifest = {"version": 1, "id": target, "name": "Approved candidate", "images": target_images, "supported_sources": [source]}
            inventory = {"instance_id": second, "organisation_id": a, "release": source}
            catalog = {"releases": [manifest], "instances": [inventory]}
            sync = "/management/runner/v1/releases/sync"
            choices = f"/management/v1/organisations/{a}/instances/{second}/releases"
            request(sync, catalog, token=token, expected=404)
            request(sync, {**catalog, "releases": [{**manifest, "id": "a" * 64}]}, broker=True, expected=422)
            request(sync, {**catalog, "instances": [{**inventory, "organisation_id": b}]}, broker=True, expected=404)
            request(sync, catalog, broker=True)
            request(sync, {**catalog, "releases": [{**manifest, "name": "Rewrite"}]}, broker=True, expected=409)
            request(choices, token=bob["access_token"], expected=404)
            listed = request(choices, token=alice["access_token"])
            assert len(listed) == 1 and listed[0]["qualified"] is False
            def second_now():
                return next(row for row in request(f"/management/v1/organisations/{a}/instances", token=token) if row["id"] == second)
            def update(canary, expected=200):
                return request(second_path, {"action": "upgrade", "expected_revision": second_now()["revision"], "idempotency_key": str(uuid.uuid4()), "release": target, "qualification_only": canary}, alice["access_token"], expected=expected)
            update(False, 422)
            canary = update(True)
            assert second_now()["available_actions"] == []
            request(sync, {**catalog, "instances": [{**inventory, "release": target}]}, broker=True)
            assert second_now()["current_release"] == source
            lease = request("/management/runner/v1/claim", {"backend": "compose"}, broker=True)
            assert lease["arguments"]["source_release"] == source
            complete_path = f"/management/runner/v1/operations/{canary['id']}/complete"
            result = {"receipt_id": canary["id"], "instance_id": second, "source_release": source, "release": target, "qualification_only": True, "rollback_verified": True, "instance_state": "stopped",
                      "backup_id": canary["id"], "backup_sha256": "c" * 64, "backup_bytes": 1234, "manifest_version": 1}
            completion = {"lease_token": lease["lease_token"], "succeeded": True, "result": result}
            for invalid_result in [{**result, "source_release": target}, {**result, "rollback_verified": False}, {**result, "instance_id": instance}, {**result, "backup_id": str(uuid.uuid4())}]:
                request(complete_path, {**completion, "result": invalid_result}, broker=True, expected=422)
            request(complete_path, completion, broker=True)
            request(complete_path, completion, broker=True)
            assert second_now()["current_release"] == source and request(choices, token=token)[0]["qualified"] is True
            assert len(request(choices.replace('/releases', '/backups'), token=token)) == 1
            revoked_update = update(False)
            request(sync, {"releases": [], "instances": [inventory]}, broker=True)
            assert request("/management/runner/v1/claim", {"backend": "compose"}, broker=True) is None
            assert request(second_path, token=token)[0]["id"] == revoked_update["id"]
            assert request(second_path, token=token)[0]["result"]["error_code"] == "release_authorization_revoked"
            request(sync, catalog, broker=True)
            promoted = update(False)
            lease = request("/management/runner/v1/claim", {"backend": "compose"}, broker=True)
            installed = {key: value for key, value in result.items() if key != "rollback_verified"}
            installed.update({"receipt_id": promoted["id"], "backup_id": promoted["id"], "qualification_only": False, "generation": promoted["id"]})
            request(f"/management/runner/v1/operations/{promoted['id']}/complete", {"lease_token": lease["lease_token"], "succeeded": True, "result": installed}, broker=True)
            assert second_now()["current_release"] == target and second_now()["state"] == "stopped"
            assert len(request(choices.replace('/releases', '/backups'), token=token)) == 2
            print("release_catalog_scope_canary_revocation_and_promotion=ok (simulated infrastructure receipts)")
            rollback_path = choices.replace('/releases', '/rollbacks')
            points = request(rollback_path, token=alice["access_token"])
            assert len(points) == 1 and points[0]["operation_id"] == promoted["id"] and points[0]["release"] == source
            request(rollback_path.replace(a, b), token=alice["access_token"], expected=404)
            def rollback_body(**extra):
                return {"action": "rollback", "expected_revision": second_now()["revision"], "idempotency_key": str(uuid.uuid4()), "rollback_operation_id": promoted["id"], **extra}
            request(second_path, rollback_body(release=source), alice["access_token"], expected=422)
            request(second_path, rollback_body(backup_id=promoted["id"]), alice["access_token"], expected=422)
            request(second_path, rollback_body(rollback_operation_id=canary["id"]), alice["access_token"], expected=422)
            rolled = request(second_path, rollback_body(), alice["access_token"])
            lease = request("/management/runner/v1/claim", {"backend":"compose"}, broker=True)
            assert lease["id"] == rolled["id"] and lease["backup"]["id"] == promoted["id"]
            assert lease["arguments"]["source_release"] == target and lease["arguments"]["release"] == source
            rollback_receipt = {"receipt_id":rolled["id"], "instance_id":second, "source_release":target, "release":source,
                "rollback_operation_id":promoted["id"], "restored_backup_id":promoted["id"], "generation":rolled["id"],
                "instance_state":"stopped", "backup_id":rolled["id"], "backup_sha256":"e"*64, "backup_bytes":2048, "manifest_version":1}
            completion_path = f"/management/runner/v1/operations/{rolled['id']}/complete"
            for key, bad in [("generation",promoted["id"]), ("source_release",source), ("release",target), ("rollback_operation_id",canary["id"]), ("restored_backup_id",canary["id"]), ("backup_id",promoted["id"])]:
                request(completion_path, {"lease_token":lease["lease_token"], "succeeded":True, "result":{**rollback_receipt,key:bad}}, broker=True, expected=422)
                assert second_now()["current_release"] == target
            request(completion_path, {"lease_token":lease["lease_token"], "succeeded":False, "result":{"error_code":"unproven"}}, broker=True, expected=422)
            completion = {"lease_token":lease["lease_token"], "succeeded":True, "result":rollback_receipt}
            request(completion_path, completion, broker=True)
            request(completion_path, completion, broker=True)  # Lost response is idempotent.
            assert second_now()["current_release"] == source and second_now()["state"] == "stopped"
            assert request(rollback_path, token=token) == []
            assert len(request(choices.replace('/releases', '/backups'), token=token)) == 3
            request(second_path, rollback_body(), alice["access_token"], expected=422)

            # A later data restore invalidates the previous promotion even when
            # its image set is unchanged. Old points must not follow new data.
            again = update(False)
            lease = request("/management/runner/v1/claim", {"backend":"compose"}, broker=True)
            again_receipt = {**installed,"receipt_id":again["id"],"backup_id":again["id"],"generation":again["id"]}
            request(f"/management/runner/v1/operations/{again['id']}/complete", {"lease_token":lease["lease_token"],"succeeded":True,"result":again_receipt}, broker=True)
            assert request(rollback_path, token=token)[0]["operation_id"] == again["id"]
            saved = request(second_path, {"action":"backup","expected_revision":second_now()["revision"],"idempotency_key":str(uuid.uuid4())}, alice["access_token"])
            lease = request("/management/runner/v1/claim", {"backend":"compose"}, broker=True)
            request(f"/management/runner/v1/operations/{saved['id']}/complete", {"lease_token":lease["lease_token"],"succeeded":True,"result":{"receipt_id":saved["id"],"backup_id":saved["id"],"backup_sha256":"f"*64,"backup_bytes":1234,"manifest_version":1}}, broker=True)
            restored = request(second_path, {"action":"restore","backup_id":saved["id"],"expected_revision":second_now()["revision"],"idempotency_key":str(uuid.uuid4())}, alice["access_token"])
            lease = request("/management/runner/v1/claim", {"backend":"compose"}, broker=True)
            request(f"/management/runner/v1/operations/{restored['id']}/complete", {"lease_token":lease["lease_token"],"succeeded":True,"result":{"receipt_id":restored["id"],"backup_id":saved["id"]}}, broker=True)
            assert second_now()["current_release"] == target
            assert request(rollback_path, token=token) == [] and "rollback" not in second_now()["available_actions"]
            request(second_path, rollback_body(rollback_operation_id=again["id"]), alice["access_token"], expected=422)
            print("rollback_scoped_point_frozen_transition_receipt_and_replay_denial=ok (simulated infrastructure receipts)")

            # Durable orchestration: real PostgreSQL/HTTP, simulated infrastructure.
            fleet_path=f"/management/v1/organisations/{a}/instances"
            rollout_path=f"/management/v1/organisations/{a}/rollouts"
            claim=lambda: request("/management/runner/v1/claim", {"backend":"compose"}, broker=True)
            def fleet_row(i):
                return next(row for row in request(fleet_path, token=token) if row["id"]==i)
            def complete_step(op, succeeded=True, extra=None):
                result={"receipt_id":op["id"]}
                if op["action"]=="upgrade":
                    result.update({"instance_id":op["instance_id"],"source_release":op["arguments"]["source_release"],"release":op["arguments"]["release"],"qualification_only":False,"generation":op["id"],"instance_state":"stopped","backup_id":op["id"],"backup_sha256":"b"*64,"backup_bytes":1234,"manifest_version":1})
                if extra is not None: result=extra
                request(f"/management/runner/v1/operations/{op['id']}/complete", {"lease_token":op["lease_token"],"succeeded":succeeded,"result":result}, broker=True)
            fleet=[]
            for name in ["Running batch fixture","Stopped batch fixture"]:
                i=request(fleet_path,{"name":name,"backend":"compose"},token)["id"]
                request(f"{fleet_path}/{i}/operations",{"action":"provision","expected_revision":fleet_row(i)["revision"],"idempotency_key":str(uuid.uuid4())},token)
                complete_step(claim()); fleet.append(i)
            request(f"{fleet_path}/{fleet[1]}/operations",{"action":"stop","expected_revision":fleet_row(fleet[1])["revision"],"idempotency_key":str(uuid.uuid4())},token)
            complete_step(claim())
            def inventory_source():
                request(sync,{"releases":[manifest],"instances":[{"instance_id":i,"organisation_id":a,"release":source} for i in fleet]},broker=True)
            inventory_source()
            def plan_body(batch=1):
                return {"name":"Grouped update","release":target,"batch_size":batch,"targets":[{"instance_id":i,"expected_revision":fleet_row(i)["revision"]} for i in fleet],"idempotency_key":str(uuid.uuid4())}
            def rollout_row(i):
                return next(row for row in request(rollout_path,token=token) if row["id"]==i)
            def control(i, action, expected=200):
                return request(f"{rollout_path}/{i}/control",{"action":action,"expected_revision":rollout_row(i)["revision"]},token,expected=expected)
            request(rollout_path,token=alice["access_token"],expected=404)
            request(rollout_path,plan_body(),alice["access_token"],expected=404)
            body=plan_body()
            request(rollout_path,{**body,"release":"f"*64},token,expected=422)
            request(rollout_path,{**body,"targets":[body["targets"][0]]*2},token,expected=422)
            request(rollout_path,{**body,"targets":[body["targets"][0],{"instance_id":str(uuid.uuid4()),"expected_revision":1}]},token,expected=404)
            assert fleet_row(fleet[0])["rollout_id"] is None
            request(rollout_path,{**body,"targets":[{**body["targets"][0],"expected_revision":0}]},token,expected=409)
            plan=request(rollout_path,body,token)
            assert request(rollout_path,body,token)["id"]==plan["id"]
            request(rollout_path,{**body,"batch_size":2},token,expected=409)
            assert all(fleet_row(i)["available_actions"]==[] and fleet_row(i)["rollout_id"]==plan["id"] for i in fleet)
            request(f"{fleet_path}/{fleet[0]}/operations",{"action":"stop","expected_revision":fleet_row(fleet[0])["revision"],"idempotency_key":str(uuid.uuid4())},token,expected=409)
            first_step=claim()
            assert first_step["instance_id"]==fleet[0] and first_step["action"]=="stop"
            assert claim() is None
            control(plan["id"],"pause")
            complete_step(first_step)
            assert claim() is None
            assert rollout_row(plan["id"])["targets"][0]["next_action"]=="upgrade"
            control(plan["id"],"resume")
            upgrade=claim(); assert upgrade["action"]=="upgrade" and upgrade["instance_id"]==fleet[0]
            complete_step(upgrade)
            # Restart management after the receipt is committed, before the next
            # tick. Its durable rows must advance to start, not repeat upgrade.
            process.terminate(); process.wait(timeout=10)
            process=subprocess.Popen([str(Path(args.server).resolve())],env=env,stdout=log,stderr=log)
            for attempt in range(100):
                try: request("/readyz"); break
                except OSError: time.sleep(.1)
            start=claim(); assert start["action"]=="start" and start["instance_id"]==fleet[0]
            complete_step(start)
            stopped_update=claim(); assert stopped_update["action"]=="upgrade" and stopped_update["instance_id"]==fleet[1]
            complete_step(stopped_update); assert claim() is None
            assert rollout_row(plan["id"])["state"]=="succeeded"
            assert [fleet_row(i)["state"] for i in fleet]==["ready","stopped"]
            assert all(fleet_row(i)["rollout_id"] is None and fleet_row(i)["current_release"]==target for i in fleet)
            print("rollout_scoped_atomic_reservation_batch_order_pause_resume_restart_and_initial_state=ok")

            inventory_source()
            uncertain=request(rollout_path,plan_body(),token)
            step=claim(); assert step["action"]=="stop"
            request(f"/management/runner/v1/operations/{step['id']}/reconciliation",{"lease_token":step["lease_token"],"reason":"operation_completion_unproven"},broker=True)
            assert claim() is None
            assert rollout_row(uncertain["id"])["state"]=="paused"
            control(uncertain["id"],"resume",422)
            control(uncertain["id"],"cancel"); assert claim() is None
            assert rollout_row(uncertain["id"])["state"]=="canceling"
            assert fleet_row(fleet[0])["rollout_id"]==uncertain["id"] and fleet_row(fleet[1])["rollout_id"] is None
            request(f"{fleet_path}/{fleet[0]}/operations/{step['id']}/recover",{"mode":"stop_if_unproven"},token)
            pending=request("/management/runner/v1/pending",{"backend":"compose"},broker=True)
            assert next(row for row in pending if row["id"]==step["id"])["recovery_authorized"] is True
            complete_step(step,False,{"error_code":"operation_interrupted_resources_stopped","instance_state":"stopped"})
            assert claim() is None and rollout_row(uncertain["id"])["state"]=="canceled"
            assert all(fleet_row(i)["rollout_id"] is None for i in fleet)
            print("rollout_uncertain_operation_never_replayed_cancel_waits_for_reconciliation=ok")

            withdrawn=request(rollout_path,plan_body(2),token)
            request(sync,{"releases":[],"instances":[]},broker=True)
            assert claim() is None and rollout_row(withdrawn["id"])["state"]=="paused"
            assert all(row["operation_id"] is None for row in rollout_row(withdrawn["id"])["targets"])
            control(withdrawn["id"],"cancel"); assert claim() is None
            inventory_source()
            request(f"/management/v1/organisations/{a}/members",{"email":"alice@enterprise.test","role":"admin"},token)
            demoted=request(rollout_path,plan_body(),alice["access_token"])
            request(f"/management/v1/organisations/{a}/members",{"email":"alice@enterprise.test","role":"auditor"},token)
            assert claim() is None and rollout_row(demoted["id"])["state"]=="paused"
            assert request(rollout_path,token=alice["access_token"])
            request(f"{rollout_path}/{demoted['id']}/control",{"action":"cancel","expected_revision":rollout_row(demoted["id"])["revision"]},alice["access_token"],expected=404)
            control(demoted["id"],"cancel"); assert claim() is None
            request(f"/management/v1/organisations/{a}/members",{"email":"alice@enterprise.test","role":"member"},token)
            print("rollout_catalog_and_authority_revocation_auditor_read_only=ok")
            inventory_source()
            canceled=request(rollout_path,plan_body(2),token)
            running=claim(); assert running["action"]=="upgrade"
            before=rollout_row(canceled["id"])
            assert all(t["operation_id"] for t in before["targets"])
            control(canceled["id"],"cancel"); assert claim() is None
            waiting=[t for t in rollout_row(canceled["id"])["targets"] if t["instance_id"]!=running["instance_id"]][0]
            assert waiting["state"]=="canceled" and fleet_row(waiting["instance_id"])["current_release"]==source
            assert rollout_row(canceled["id"])["state"]=="canceling"
            complete_step(running); assert claim() is None
            assert rollout_row(canceled["id"])["state"]=="canceled"
            assert fleet_row(running["instance_id"])["current_release"]==target
            assert all(fleet_row(i)["rollout_id"] is None for i in fleet)
            print("rollout_cancellation_settles_inflight_work_and_cancels_queued_work=ok")

            inventory_source()
            request(f"/management/v1/organisations/{a}/members",{"email":"alice@enterprise.test","role":"admin"},token)
            demoted_queued=request(rollout_path,plan_body(2),alice["access_token"])
            active=claim()
            request(f"/management/v1/organisations/{a}/members",{"email":"alice@enterprise.test","role":"member"},token)
            complete_step(active)
            assert claim() is None
            assert claim() is None and rollout_row(demoted_queued["id"])["state"]=="paused"
            control(demoted_queued["id"],"cancel"); assert claim() is None
            assert all(fleet_row(i)["rollout_id"] is None for i in fleet)
            print("rollout_dispatch_rechecks_authority_for_already_queued_steps=ok")




            events = request(f"/management/v1/organisations/{a}/audit", token=token)
            previous = "0" * 64
            for event in reversed(events):
                assert event["previous_digest"] == previous
                encoded = json.dumps(event["payload"], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
                digest = hmac.new(audit_key, previous.encode() + encoded, hashlib.sha256).hexdigest()
                assert hmac.compare_digest(digest, event["digest"])
                previous = digest
            print(f"transactional_audit_chain=ok events={len(events)}")
            subprocess.run([str(pg / "psql"), "-h", "127.0.0.1", "-p", str(db_port), "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
                            f"UPDATE management_audit SET payload=jsonb_set(payload,'{{action}}','\"forged\"') WHERE sequence={events[0]['sequence']};"], check=True, stdout=subprocess.DEVNULL)
            request(f"/management/v1/organisations/{a}/audit", token=token, expected=409)
            print("audit_tamper_rejected=ok")
            print("enterprise_management_database_acceptance=passed")
        finally:
            if process:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            subprocess.run([str(pg / "pg_ctl"), "-D", str(data), "stop", "-m", "fast", "-w"], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
