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
            log = (root / "server.log").open("wb")
            process = subprocess.Popen([str(Path(args.server).resolve())], env=env, stdout=log, stderr=log)
            origin = f"http://127.0.0.1:{api_port}"

            def request(path, payload=None, token=None, method=None, expected=200, broker=False, browser=False, response_headers=None):
                headers = {"Content-Type": "application/json"}
                if token:
                    headers["Authorization"] = "Bearer " + token
                if broker:
                    headers["x-cowork-provisioner-token"] = broker_key
                if browser:
                    headers["x-cowork-session-mode"] = "browser-cookie"
                body = None if payload is None else json.dumps(payload).encode()
                req = Request(origin + path, body, headers, method=method or ("GET" if body is None else "POST"))
                try:
                    with urlopen(req, timeout=30) as response:
                        status, content = response.status, response.read()
                        if response_headers is not None:
                            response_headers.update(response.headers)
                except HTTPError as error:
                    status, content = error.code, error.read()
                if status != expected:
                    # Authentication responses contain credentials even when only the status is unexpected.
                    detail = "<response redacted>"
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
            def sql(statement):
                return subprocess.check_output([str(pg / "psql"), "-h", "127.0.0.1", "-p", str(db_port), "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-Atc", statement], text=True).strip()

            password = "Test-Password-42-Long"
            device = str(uuid.uuid4())
            admin = request("/api/v1/auth/bootstrap", {"email": "admin@enterprise.test", "display_name": "Administrator", "password": password, "device_id": device}, bootstrap, expected=201)
            token = admin["access_token"]
            setup=request("/api/v1/auth/totp/setup", {}, token)
            request("/api/v1/auth/totp/enable", {"code":totp(setup["secret"])}, token)
            a = request("/management/v1/organisations", {"name": "Organisation A"}, token)["id"]
            b = request("/management/v1/organisations", {"name": "Organisation B"}, token)["id"]
            def path(org): return f"/management/v1/organisations/{org}/invitations"
            def invite(org, email, role="member", auth=token):
                headers = {}
                value = request(path(org), {"email": email, "role": role}, auth, expected=201, response_headers=headers)
                assert headers["cache-control"] == "no-store"
                return value
            inspect = "/api/v1/auth/organisation-invitations/inspect"
            register = "/api/v1/auth/organisation-invitations/register"
            accept = "/management/v1/invitations/accept"
            def inspect_token(value, **kwargs): return request(inspect, {"token": value["token"]}, **kwargs)
            def signup(value, **kwargs):
                return request(register, {"token": value["token"], "display_name": "Invited user", "password": password, "device_id": device}, **kwargs)

            # The scoped org administrator may invite without platform-wide account authority.
            seed = invite(a, "org-admin@enterprise.test", "admin")
            org_admin = signup(seed, expected=201)
            oa = org_admin["access_token"]
            assert sql(f"SELECT platform_admin FROM users WHERE id='{org_admin['user_id']}'") == "f"
            assert sql(f"SELECT content_admin FROM management_members WHERE user_id='{org_admin['user_id']}'") == "f"
            request("/api/v1/auth/invitations", {"email": "forbidden@enterprise.test"}, oa, expected=401)
            request(path(b), token=oa, expected=404)
            request(path(b), {"email": "foreign@enterprise.test", "role": "member"}, oa, expected=404)
            request(path(a), {"email": "invalid@enterprise.test", "role": "platform_admin"}, oa, expected=422)
            request(path(a), {"email": "invalid@enterprise.test", "role": "admin", "content_admin": True}, oa, expected=422)
            request(path(a), {"email": "invalid@enterprise.test", "role": "member", "expires_at": "2099-01-01T00:00:00Z"}, oa, expected=422)
            for invalid_email in ["a@b", "a..b@example.test", ".a@example.test", "a@bad_domain.test", "a@example.1", "a'@example.test", "a@example.test@other.test"]:
                request(path(a), {"email": invalid_email, "role": "member"}, oa, expected=422)
            inv = invite(a, "  New-Member@enterprise.test  ", auth=oa)
            headers = {}
            preview = inspect_token(inv, response_headers=headers)
            assert headers["cache-control"] == "no-store"
            assert not preview["account_exists"] and preview["email"] == "new-member@enterprise.test" and preview["organisation_id"] == a
            request(register, {"token": inv["token"], "display_name": "Injected", "password": password, "device_id": device, "role": "admin"}, expected=422)
            headers = {}
            new = signup(inv, expected=201, browser=True, response_headers=headers)
            assert "refresh_token" not in new and headers["cache-control"] == "no-store"
            assert "HttpOnly" in headers["set-cookie"] and "Secure" in headers["set-cookie"] and "SameSite=Strict" in headers["set-cookie"]
            assert sql(f"SELECT role || ':' || content_admin::text FROM management_members WHERE user_id='{new['user_id']}'") == "member:false"
            assert sql(f"SELECT platform_admin FROM users WHERE id='{new['user_id']}'") == "f"
            request(path(a), token=new["access_token"], expected=404)
            request(path(a), {"email": "denied@enterprise.test", "role": "member"}, new["access_token"], expected=404)
            request(path(b), token=new["access_token"], expected=404)
            assert request(f"/management/v1/organisations/{a}/instances", token=new["access_token"]) == []
            inspect_token(inv, expected=401)
            signup(inv, expected=401)
            request(accept, {"token": inv["token"]}, new["access_token"], expected=401)
            request(path(a) + "/" + inv["id"], token=oa, method="DELETE", expected=409)
            print("organisation_invitation_registration_and_boundaries=ok")

            # Existing identities authenticate; invitation does not reset password or role.
            existing = invite(b, "new-member@enterprise.test", "auditor")
            assert inspect_token(existing)["account_exists"]
            signup(existing, expected=409)
            request(accept, {"token": existing["token"]}, oa, expected=401)
            request(accept, {"token": existing["token"]}, expected=401)
            new_login = request("/api/v1/auth/login", {"email": "new-member@enterprise.test", "password": password, "device_id": device})
            joined = request(accept, {"token": existing["token"]}, new_login["access_token"])
            assert joined == {"organisation_id": b, "user_id": new["user_id"], "role": "auditor", "membership_created": True}
            existing_role = invite(a, "org-admin@enterprise.test", "member")
            assert request(accept, {"token": existing_role["token"]}, oa)["role"] == "admin"
            higher_role = invite(a, "new-member@enterprise.test", "admin")
            kept = request(accept, {"token": higher_role["token"]}, new_login["access_token"])
            assert kept["role"] == "member" and not kept["membership_created"]
            print("existing_identity_authentication_and_role_preservation=ok")

            revoked = invite(a, "revoked@enterprise.test", auth=oa)
            request(path(b) + "/" + revoked["id"], token=token, method="DELETE", expected=404)
            request(path(a) + "/" + revoked["id"], token=new["access_token"], method="DELETE", expected=404)
            request(path(a) + "/" + revoked["id"], token=oa, method="DELETE")
            request(path(a) + "/" + revoked["id"], token=oa, method="DELETE")
            inspect_token(revoked, expected=401)
            signup(revoked, expected=401)
            expired = invite(a, "expired@enterprise.test", auth=oa)
            sql(f"UPDATE management_invitations SET created_at=now()-interval '2 days',expires_at=now()-interval '1 day' WHERE id='{expired['id']}'")
            inspect_token(expired, expected=401)
            signup(expired, expected=401)
            authority = invite(a, "authority@enterprise.test", auth=oa)
            request(f"/management/v1/organisations/{a}/members", {"email": "org-admin@enterprise.test", "role": "member"}, token)
            inspect_token(authority, expected=401)
            signup(authority, expected=401)
            state = next(row["state"] for row in request(path(a), token=token) if row["id"] == authority["id"])
            assert state == "authority_revoked"
            print("revocation_expiry_and_current_inviter_authority=ok")

            # Two simultaneous accepts can never consume a token twice.
            race_invite = invite(a, "concurrent@enterprise.test")
            def concurrent_signup(_):
                try:
                    signup(race_invite, expected=201)
                    return "created"
                except AssertionError as error:
                    assert "got 401" in str(error)
                    return "rejected"
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
                result = sorted(workers.map(concurrent_signup, range(2)))
            assert result == ["created", "rejected"]
            assert sql("SELECT count(*) FROM users WHERE email='concurrent@enterprise.test'") == "1"
            print("one_use_concurrent_consumption=ok")

            rows = request(path(a), token=token)
            assert all("token" not in row and "token_hash" not in row for row in rows)
            assert sql(f"SELECT encode(token_hash,'hex') FROM management_invitations WHERE id='{inv['id']}'") == hashlib.sha256(inv["token"].encode()).hexdigest()
            events = request(f"/management/v1/organisations/{a}/audit", token=token)
            text = json.dumps(events)
            assert all(value["token"] not in text for value in [inv, revoked, expired, authority, race_invite])
            assert "invitation.create" in text and "invitation.accept" in text and "invitation.revoke" in text
            inspect_token({"token": "x" * 43}, expected=401)
            print("hashed_invitation_storage_and_audit=ok")
            print("enterprise_invitation_database_acceptance=passed")
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
