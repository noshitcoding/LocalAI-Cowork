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
            password="Testing-Admin-Assurance-42!"
            identity={"email":"admin@enterprise.test","password":password,"device_id":str(uuid.uuid4())}
            admin=request("/api/v1/auth/bootstrap",{**identity,"display_name":"Operator"},bootstrap,expected=201)
            token=admin["access_token"]
            assurance="/api/v1/auth/assurance"
            verify=assurance+"/verify"
            organisations="/management/v1/organisations"
            first=request(assurance,token=token)
            assert first=={"required":True,"totp_enabled":False,"passkey_registered":False,"session_verified":False,"enrollment_allowed":True,"reauthentication_required":False}
            request(organisations,token=token,expected=403)
            request(organisations,{"name":"Denied"},token,expected=403)
            request("/api/v1/auth/invitations",{"email":"denied@example.test"},token,expected=403)
            request("/api/v1/auth/passkeys/register/start",{},token,expected=403)
            request(verify,{"code":"111111","session_verified":True},token,expected=422)
            request(verify,{"code":"111111"},token,expected=409)
            weak=[request("/api/v1/auth/login",identity) for _ in range(3)]
            refreshed=request("/api/v1/auth/refresh",{"refresh_token":weak[0]["refresh_token"]})
            request(organisations,token=refreshed["access_token"],expected=403)
            verifier="v"*64
            challenge=base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
            old_code=request("/api/v1/auth/native/authorize",{**identity,"code_challenge":challenge,"code_challenge_method":"S256"})
            sql(f"UPDATE auth_sessions SET created_at=now()-interval '11 minutes' WHERE id='{weak[2]['session_id']}'")
            old=request(assurance,token=weak[2]["access_token"])
            assert old["reauthentication_required"] and not old["enrollment_allowed"]
            request("/api/v1/auth/totp/setup",{},weak[2]["access_token"],expected=403)
            print("restricted_bootstrap_legacy_session_and_refresh=ok")

            setup=request("/api/v1/auth/totp/setup",{},token)
            sql(f"UPDATE user_totp SET pending_expires_at=now()-interval '1 second' WHERE user_id='{admin['user_id']}'")
            request("/api/v1/auth/totp/enable",{"code":totp(setup["secret"])},token,expected=409)
            setup=request("/api/v1/auth/totp/setup",{},token)
            code=totp(setup["secret"])
            codes=request("/api/v1/auth/totp/enable",{"code":code},token)["recovery_codes"]
            assert len(codes)>=8 and len(set(codes))==len(codes)
            status=request(assurance,token=token)
            assert status["required"] and status["totp_enabled"] and status["session_verified"]
            org=request(organisations,{"name":"Protected organisation"},token)
            for session in weak:
                assert not request(assurance,token=session["access_token"])["session_verified"]
                request(organisations,token=session["access_token"],expected=403)
                request("/api/v1/auth/totp/setup",{},session["access_token"],expected=403)
            exchanged=request("/api/v1/auth/native/token",{"code":old_code["code"],"code_verifier":verifier,"device_id":identity["device_id"]})
            request(organisations,token=exchanged["access_token"],expected=403)
            assert base64.b32decode(setup["secret"]+'='*((8-len(setup["secret"])%8)%8)).hex() not in sql(f"SELECT encode(ciphertext,'hex') FROM user_totp WHERE user_id='{admin['user_id']}'")
            print("enrollment_without_object_store_and_session_bound_assurance=ok")

            request(verify,{"code":code},weak[0]["access_token"],expected=401)
            request("/api/v1/auth/login",identity,expected=401)
            verified=request(verify,{"code":codes[0]},weak[0]["access_token"])
            assert verified["session_verified"]
            request(verify,{"code":codes[0]},weak[1]["access_token"],expected=401)
            request(organisations,token=weak[0]["access_token"])
            login=request("/api/v1/auth/login",{**identity,"second_factor":codes[1]})
            assert request(assurance,token=login["access_token"])["session_verified"]
            rotated=request("/api/v1/auth/refresh",{"refresh_token":login["refresh_token"]})
            assert request(assurance,token=rotated["access_token"])["session_verified"]
            native=request("/api/v1/auth/native/authorize",{**identity,"second_factor":codes[2],"code_challenge":challenge,"code_challenge_method":"S256"})
            native_session=request("/api/v1/auth/native/token",{"code":native["code"],"code_verifier":verifier,"device_id":identity["device_id"]})
            assert request(assurance,token=native_session["access_token"])["session_verified"]
            sql(f"UPDATE auth_sessions SET second_factor_verified_at=now()-interval '13 hours' WHERE id='{native_session['session_id']}'")
            rotated=request("/api/v1/auth/refresh",{"refresh_token":native_session["refresh_token"]})
            assert not request(assurance,token=rotated["access_token"])["session_verified"]
            request(organisations,token=rotated["access_token"],expected=403)
            assert request(verify,{"code":codes[3]},rotated["access_token"])["session_verified"]
            print("password_pkce_recovery_and_twelve_hour_reverification=ok")

            def race(session):
                try:
                    request(verify,{"code":codes[4]},session["access_token"])
                    return "verified"
                except AssertionError as error:
                    assert "got 401" in str(error)
                    return "rejected"
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                assert sorted(executor.map(race,weak[1:]))==["rejected","verified"]
            assert sum(request(assurance,token=s["access_token"])["session_verified"] for s in weak[1:])==1
            request("/api/v1/auth/totp/disable",{"password":password,"second_factor":codes[5]},token,expected=422)
            assert request(assurance,token=token)["totp_enabled"]
            print("concurrent_recovery_and_last_factor_protection=ok")

            # Invalid factors are limited per account, including across distinct sessions.
            sql(f"UPDATE user_totp SET failed_attempts=0,failure_window_started_at=NULL,blocked_until=NULL WHERE user_id='{admin['user_id']}'")
            for _ in range(5): request(verify,{"code":"invalid-recovery-code"},token,expected=401)
            request(verify,{"code":codes[6]},weak[0]["access_token"],expected=429)
            sql(f"UPDATE user_totp SET blocked_until=now()-interval '1 second',failure_window_started_at=now()-interval '6 minutes' WHERE user_id='{admin['user_id']}'")
            assert request(verify,{"code":codes[6]},token)["session_verified"]
            assert sql(f"SELECT failed_attempts FROM user_totp WHERE user_id='{admin['user_id']}'")=="0"
            print("persistent_second_factor_attempt_limit=ok")

            headers={}
            browser_session=request("/api/v1/auth/login",{**identity,"second_factor":codes[7]},browser=True,response_headers=headers)
            assert "refresh_token" not in browser_session or browser_session["refresh_token"] is None
            headers={key.lower():value for key,value in headers.items()}
            cookie=headers.get("set-cookie", "")
            assert "HttpOnly" in cookie and "Secure" in cookie and "SameSite=Strict" in cookie
            assert "no-store" in headers.get("cache-control", "")
            assert request(assurance,token=browser_session["access_token"])["session_verified"]
            print("browser_session_cookie_and_assurance=ok")

            # Metadata fixtures test deletion locking, not a WebAuthn ceremony.
            passkey=str(uuid.uuid4())
            sql(f"INSERT INTO passkeys(id,user_id,credential_id,label,credential) VALUES('{passkey}','{admin['user_id']}',decode('abcd','hex'),'Deletion-lock fixture','{{}}')")
            def remove_factor(kind):
                path="/api/v1/auth/totp/disable" if kind==0 else f"/api/v1/auth/passkeys/{passkey}"
                try:
                    request(path,{"password":password,"second_factor":codes[8+kind]},token,method="POST" if kind==0 else "DELETE",expected=204)
                    return "removed"
                except AssertionError as error:
                    assert "got 422" in str(error)
                    return "retained"
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                assert sorted(executor.map(remove_factor,range(2)))==["removed","retained"]
            assert sql(f"SELECT (SELECT count(*) FROM passkeys WHERE user_id='{admin['user_id']}')+(SELECT count(*) FROM user_totp WHERE user_id='{admin['user_id']}' AND enabled)")=="1"
            print("concurrent_distinct_factor_deletion_retains_one=ok")

            invite=request(f"{organisations}/{org['id']}/invitations",{"email":"member@example.test","role":"member"},token,expected=201)
            member=request("/api/v1/auth/organisation-invitations/register",{"token":invite["token"],"display_name":"Member","password":password,"device_id":str(uuid.uuid4())},expected=201)
            assert not request(assurance,token=member["access_token"])["required"]
            request(organisations,token=member["access_token"])
            sql(f"UPDATE users SET platform_admin=TRUE WHERE id='{member['user_id']}'")
            request(organisations,token=member["access_token"],expected=403)
            assert request(assurance,token=member["access_token"])["required"]
            request("/api/v1/auth/logout",{},member["access_token"],expected=204)
            request(assurance,token=member["access_token"],expected=401)
            print("role_promotion_and_restricted_logout=ok")
            assert sql("SELECT count(*) FROM audit_events WHERE action='auth.session.second_factor_verified'")!="0"
            print("enterprise_admin_assurance_database_acceptance=passed")
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
