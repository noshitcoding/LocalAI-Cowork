#!/usr/bin/env python3
"""Real OIDC HTTP/JWKS/RS256 + disposable PostgreSQL. No external IdP qualification.

The provider is a test double; signature, nonce, issuer, audience, PKCE, database
transactions, migration and server authorization checks run without mocking them.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener
import uuid


def b64(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode()


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def totp(secret):
    key = base64.b32decode(secret + "=" * ((8 - len(secret) % 8) % 8))
    digest = hmac.new(key, (int(time.time()) // 30).to_bytes(8, "big"), hashlib.sha1).digest()
    offset = digest[-1] & 15
    return f"{(int.from_bytes(digest[offset:offset+4], 'big') & 0x7fffffff) % 1000000:06d}"


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class Provider:
    def __init__(self, root):
        self.key = root / "provider.pem"
        subprocess.run(["openssl", "genrsa", "-out", str(self.key), "2048"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        modulus = subprocess.check_output(["openssl", "rsa", "-in", str(self.key), "-modulus", "-noout"], text=True).strip().split("=", 1)[1]
        self.jwk = {"kty": "RSA", "n": b64(bytes.fromhex(modulus)), "e": "AQAB", "kid": "test-key", "use": "sig", "alg": "RS256"}
        self.client = "enterprise-oidc-test"
        self.secret = secrets.token_hex(32)
        self.cases = {}
        self.codes = {}
        self.lock = threading.Lock()
        provider = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def send(self, status, value=None, location=None):
                body = json.dumps(value).encode() if value is not None else b""
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Cache-Control", "no-store")
                if location:
                    self.send_header("Location", location)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                url = urlsplit(self.path)
                if url.path == "/.well-known/openid-configuration":
                    return self.send(200, {"issuer": provider.issuer, "authorization_endpoint": provider.issuer + "/authorize", "token_endpoint": provider.issuer + "/token", "jwks_uri": provider.issuer + "/jwks", "response_types_supported": ["code"], "subject_types_supported": ["public"], "id_token_signing_alg_values_supported": ["RS256"], "token_endpoint_auth_methods_supported": ["client_secret_basic"], "code_challenge_methods_supported": ["S256"]})
                if url.path == "/jwks":
                    return self.send(200, {"keys": [provider.jwk]})
                if url.path != "/authorize":
                    return self.send(404, {})
                query = {key: value[0] for key, value in parse_qs(url.query).items()}
                with provider.lock:
                    case = provider.cases.pop(query.get("state"), None)
                    if not case or query.get("client_id") != provider.client or query.get("code_challenge_method") != "S256" or "openid" not in query.get("scope", "").split():
                        return self.send(400, {"error": "invalid_request"})
                    code = secrets.token_urlsafe(32)
                    provider.codes[code] = (query, case)
                return self.send(302, location=query["redirect_uri"] + "?" + urlencode({"state": query["state"], "code": code}))

            def do_POST(self):
                if self.path != "/token":
                    return self.send(404, {})
                query = {key: value[0] for key, value in parse_qs(self.rfile.read(int(self.headers["Content-Length"])).decode()).items()}
                with provider.lock:
                    entry = provider.codes.pop(query.get("code"), None)
                if not entry:
                    return self.send(400, {"error": "invalid_grant"})
                original, case = entry
                basic = "Basic " + base64.b64encode((provider.client + ":" + provider.secret).encode()).decode()
                if self.headers.get("Authorization") != basic or query.get("redirect_uri") != original["redirect_uri"] or query.get("grant_type") != "authorization_code" or b64(hashlib.sha256(query.get("code_verifier", "").encode()).digest()) != original["code_challenge"]:
                    return self.send(400, {"error": "invalid_grant"})
                if case.get("token_error"):
                    return self.send(400, {"error": "invalid_grant", "error_description": "DO-NOT-EXPOSE-PROVIDER-SECRET-" + provider.secret})
                access = secrets.token_urlsafe(32)
                claims = {"iss": provider.issuer, "sub": case["sub"], "aud": provider.client, "iat": int(time.time()), "exp": int(time.time()) + 300, "nonce": original["nonce"], "at_hash": b64(hashlib.sha256(access.encode()).digest()[:16]), "email": case.get("email", case["sub"] + "@example.test"), "email_verified": True}
                claims.update(case.get("claims", {}))
                header = {"alg": "none" if case.get("unsigned") else "RS256", "kid": "test-key", "typ": "JWT"}
                signing = b64(json.dumps(header).encode()) + "." + b64(json.dumps(claims).encode())
                signature = subprocess.check_output(["openssl", "dgst", "-sha256", "-sign", str(provider.key)], input=signing.encode())
                if case.get("bad_signature"):
                    signature = bytes([signature[0] ^ 1]) + signature[1:]
                return self.send(200, {"access_token": access, "token_type": "Bearer", "expires_in": 300, "id_token": signing + "." + ("" if case.get("unsigned") else b64(signature))})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.issuer = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", required=True)
    parser.add_argument("--previous-server", help="Pre-0041 server binary for real upgrade acceptance")
    parser.add_argument("--postgres-bin", default="/usr/lib/postgresql/16/bin")
    parser.add_argument("--organisation-sso", action="store_true")
    args = parser.parse_args()
    pg = Path(args.postgres_bin)
    with tempfile.TemporaryDirectory(prefix="cowork-oidc-") as temp:
        root = Path(temp)
        db_port, api_port = port(), port()
        while db_port == api_port:
            api_port = port()
        origin = f"http://127.0.0.1:{api_port}"
        provider = Provider(root)
        data = root / "pg"
        subprocess.run([str(pg / "initdb"), "-D", str(data), "-A", "trust", "-U", "postgres", "--no-sync"], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([str(pg / "pg_ctl"), "-D", str(data), "-l", str(root / "postgres.log"), "-o", f"-h 127.0.0.1 -p {db_port} -k {temp}", "start", "-w"], check=True, stdout=subprocess.DEVNULL)
        process = None
        log = (root / "server.log").open("wb")
        bootstrap = secrets.token_hex(32)
        (root / "audit-key").write_bytes(secrets.token_bytes(32))
        (root / "broker-key").write_text(secrets.token_hex(32))
        env = {key: value for key, value in os.environ.items() if not key.startswith("COWORK_") and key != "DATABASE_URL"}
        env.update({"DATABASE_URL": f"postgres://postgres@127.0.0.1:{db_port}/postgres", "COWORK_MODE": "management" if args.organisation_sso else "api", "COWORK_AUTH_MASTER_KEY": b64(secrets.token_bytes(32)), "COWORK_LISTEN_ADDR": f"127.0.0.1:{api_port}", "COWORK_BOOTSTRAP_TOKEN": bootstrap, "COWORK_MANAGEMENT_AUDIT_KEY_FILE": str(root / "audit-key"), "COWORK_PROVISIONER_TOKEN_FILE": str(root / "broker-key"), "COWORK_WEB_PUSH_ENABLED": "false", "COWORK_PUBLIC_ORIGIN": origin, "COWORK_OIDC_ISSUER": provider.issuer, "COWORK_OIDC_CLIENT_ID": provider.client, "COWORK_OIDC_CLIENT_SECRET": provider.secret, "COWORK_OIDC_AUTO_PROVISION": "true"})
        if args.organisation_sso:
            # Even a usable operator-configured global provider and assurance
            # mapping must not bypass administrator-owned organisation settings.
            policy = root / "global-assurance.json"
            policy.write_text(json.dumps(dict(version=1, issuer=provider.issuer, client_id=provider.client,
                                              max_age_seconds=1800, mappings=[dict(id="test-mfa", acr="strong", amr=["mfa", "otp"])])))
            env["COWORK_OIDC_ASSURANCE_POLICY_FILE"] = str(policy)
        opener = build_opener(NoRedirect)

        def call(path, payload=None, token=None, expected=200, method=None, extra_headers=None):
            headers = {"Content-Type": "application/json", **(extra_headers or {})}
            if token:
                headers["Authorization"] = "Bearer " + token
            req = Request(path if path.startswith("http") else origin + path, None if payload is None else json.dumps(payload).encode(), headers, method=method)
            try:
                with opener.open(req, timeout=30) as response:
                    status, body, headers = response.status, response.read(), response.headers
            except HTTPError as error:
                status, body, headers = error.code, error.read(), error.headers
            assert status == expected, f"{req.get_method()} {urlsplit(req.full_url).path}: expected {expected}, got {status} (response redacted)"
            if 300 <= status < 400:
                return headers["Location"]
            if not body: return None
            try: return json.loads(body)
            except json.JSONDecodeError: return body.decode()

        def sql(statement):
            return subprocess.check_output([str(pg / "psql"), "-h", "127.0.0.1", "-p", str(db_port), "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-Atc", statement], text=True).strip()

        def start(binary):
            nonlocal process
            process = subprocess.Popen([str(Path(binary).resolve())], env=env, stdout=log, stderr=log)
            for _ in range(150):
                if process.poll() is not None:
                    raise AssertionError("server exited during startup")
                try:
                    call("/readyz")
                    return
                except OSError:
                    time.sleep(.1)
            raise AssertionError("server readiness timed out")

        def stop():
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

        def flow(case, token=None, native=False, org=None, identity_provider=None):
            selected_provider = identity_provider or provider
            verifier = secrets.token_urlsafe(48)
            request = {"device_id": str(uuid.uuid4()), "code_challenge": b64(hashlib.sha256(verifier.encode()).digest()), "code_challenge_method": "S256", "client_state": secrets.token_urlsafe(32), "redirect_uri": "open-cowork://auth/callback" if native else origin + "/auth/callback"}
            authorization = call((f"/api/v1/auth/organisations/{org}/oidc" if org else "/api/v1/auth/oidc") + ("/link" if token else "") + "/start", request, token)
            provider_state = parse_qs(urlsplit(authorization["authorization_url"]).query)["state"][0]
            with selected_provider.lock:
                selected_provider.cases[provider_state] = case
            callback = call(authorization["authorization_url"], expected=302)
            return callback, request, verifier

        def finish(pending, expected=303):
            callback, request, verifier = pending
            result = call(callback, expected=expected)
            if expected != 303:
                return result
            redirect = urlsplit(result)
            assert result.split("?", 1)[0] == request["redirect_uri"]
            query = parse_qs(redirect.query)
            assert query["state"][0] == request["client_state"]
            return call("/api/v1/auth/native/token", {"code": query["code"][0], "code_verifier": verifier, "device_id": request["device_id"]})

        def global_disabled(token=None):
            assert call("/api/v1/auth/oidc/config")["enabled"] is False
            request = {"device_id": str(uuid.uuid4()), "code_challenge": b64(secrets.token_bytes(32)),
                       "code_challenge_method": "S256", "client_state": secrets.token_urlsafe(32),
                       "redirect_uri": origin + "/auth/callback"}
            call("/api/v1/auth/oidc/start", request, expected=404)
            if token:
                call("/api/v1/auth/oidc/link/start", request, token, expected=404)

        try:
            start(args.previous_server or args.server)
            if args.organisation_sso:
                global_disabled()
                assert sql("SELECT count(*) FROM oidc_authorizations") == "0"
                assert sql("SELECT count(*) FROM users") == "0"
                # Model a still-live global authorization from an older replica.
                # Its callback must be consumed without token exchange or JIT.
                state = secrets.token_urlsafe(32)
                digest = hashlib.sha256(state.encode()).hexdigest()
                sql(f"INSERT INTO oidc_authorizations(id,state_hash,nonce,provider_pkce_verifier,device_id,client_code_challenge,client_state,client_redirect_uri,expires_at) VALUES('{uuid.uuid4()}',decode('{digest}','hex'),'old-nonce','old-verifier','{uuid.uuid4()}','old-challenge','old-client-state','{origin}/auth/callback',now()+interval '5 minutes')")
                stale = "/api/v1/auth/oidc/callback?" + urlencode(dict(state=state, code="old-provider-code"))
                call(stale, expected=404)
                call(stale, expected=401)
                assert sql("SELECT count(*) FROM users") == "0"
                assert sql("SELECT count(*) FROM oidc_identities") == "0"
                assert call("/api/v1/auth/bootstrap/status")["bootstrap_available"]
                print("management_global_oidc_disabled_before_bootstrap_and_stale_callbacks_consumed=ok")
            else:
                assert call("/api/v1/auth/oidc/config")["enabled"] is True
            password = "Oidc-Local-Admin-Password-42!"
            credentials = {"email": "admin@example.test", "password": password, "device_id": str(uuid.uuid4())}
            admin = call("/api/v1/auth/bootstrap", {**credentials, "display_name": "Operator"}, bootstrap, 201)
            if args.organisation_sso:
                global_disabled()
                call("/management/v1/organisations", {}, admin["access_token"], expected=403)
            secret = call("/api/v1/auth/totp/setup", {}, admin["access_token"])["secret"]
            recovery = call("/api/v1/auth/totp/enable", {"code": totp(secret)}, admin["access_token"])["recovery_codes"]
            if args.organisation_sso:
                global_disabled(admin["access_token"])
                assurance = call("/api/v1/auth/assurance", token=admin["access_token"])
                assert assurance["session_verified"] and not assurance["oidc_reauthentication_available"]
                print("management_global_oidc_stays_disabled_after_local_admin_mfa=ok")
                from enterprise_sso_acceptance import run
                run(root, env, start, stop, call, sql, flow, finish, admin, provider, Provider, args.server, recovery.pop())
                return
            if args.previous_server:
                # Exercise actual old auto-link, JIT and explicit-link paths.
                old_unsafe = finish(flow({"sub": "old-email-link", "email": credentials["email"]}))
                assert old_unsafe["user_id"] == admin["user_id"]
                old_jit = finish(flow({"sub": "old-jit"}))
                old_link = finish(flow({"sub": "old-explicit"}, admin["access_token"]))
                pending_link = flow({"sub": "old-pending"}, admin["access_token"])
                pending_signin = flow({"sub": "old-jit"})
                old_verifier = secrets.token_urlsafe(48)
                old_code = call("/api/v1/auth/native/authorize", {**credentials, "second_factor": recovery.pop(), "code_challenge": b64(hashlib.sha256(old_verifier.encode()).digest()), "code_challenge_method": "S256"})
                stop()
                start(args.server)
                assert sql("SELECT binding_method FROM oidc_identities WHERE subject='old-email-link'") == "legacy_unverified"
                assert sql("SELECT binding_method FROM oidc_identities WHERE subject='old-jit'") == "provisioned"
                assert sql("SELECT binding_method FROM oidc_identities WHERE subject='old-explicit'") == "authenticated_session"
                for session in [admin, old_unsafe, old_link]:
                    call("/api/v1/auth/assurance", token=session["access_token"], expected=401)
                    call("/api/v1/auth/refresh", {"refresh_token": session["refresh_token"]}, expected=401)
                call("/api/v1/auth/native/token", {"code": old_code["code"], "code_verifier": old_verifier, "device_id": credentials["device_id"]}, expected=401)
                call("/api/v1/auth/assurance", token=old_jit["access_token"])
                assert finish(pending_signin)["user_id"] == old_jit["user_id"]
                finish(flow({"sub": "old-email-link", "email": credentials["email"]}), 409)
                assert finish(flow({"sub": "old-jit"}))["user_id"] == old_jit["user_id"]
                assert finish(flow({"sub": "old-explicit"}))["user_id"] == admin["user_id"]
                finish(pending_link, 401)
                admin = call("/api/v1/auth/login", {**credentials, "second_factor": recovery.pop()})
                repaired = finish(flow({"sub": "old-email-link", "email": credentials["email"]}, admin["access_token"]))
                assert repaired["user_id"] == admin["user_id"]
                assert sql("SELECT binding_method FROM oidc_identities WHERE subject='old-email-link'") == "authenticated_session"
                print("previous_release_migration_preserves_proven_links_and_revokes_unsafe_sessions=ok")

            finish(flow({"sub": "email-takeover", "email": credentials["email"]}), 409)
            assert sql("SELECT count(*) FROM oidc_identities WHERE subject='email-takeover'") == "0"
            assert sql(f"SELECT platform_admin FROM users WHERE id='{admin['user_id']}'") == "t"
            jit = finish(flow({"sub": "new-jit"}, native=True))
            again = finish(flow({"sub": "new-jit", "email": credentials["email"], "claims": {"email_verified": False}}))
            assert jit["user_id"] == again["user_id"] != admin["user_id"]
            assert sql(f"SELECT email FROM users WHERE id='{jit['user_id']}'") == "new-jit@example.test"
            assert not call("/api/v1/auth/assurance", token=jit["access_token"])["required"]
            call("/management/v1/organisations", token=jit["access_token"], expected=404)
            finish(flow({"sub": "different-subject", "email": "new-jit@example.test"}), 409)
            print("issuer_subject_binding_no_email_takeover_and_no_implicit_organisation_access=ok")

            cases = [{"claims": {"iss": "https://wrong.example"}}, {"claims": {"aud": "wrong-client"}}, {"claims": {"nonce": "wrong-nonce"}}, {"claims": {"exp": int(time.time()) - 60}}, {"claims": {"at_hash": "wrong-hash"}}, {"bad_signature": True}, {"unsigned": True}, {"claims": {"email_verified": False}}, {"claims": {"email": None}}, {"claims": {"sub": ""}}, {"claims": {"sub": "s" * 256}}]
            for index, case in enumerate(cases):
                finish(flow({"sub": f"invalid-{index}", **case}), 401)
            failure = finish(flow({"sub": "provider-error", "token_error": True}), 401)
            assert provider.secret not in json.dumps(failure) and "DO-NOT-EXPOSE" not in json.dumps(failure)
            for claims in [{"aud": [provider.client, "untrusted-client"]}, {"azp": "other-client"}]:
                finish(flow({"sub": "wrong-authorized-party", "claims": claims}), 401)
            finish(flow({"sub": "valid-authorized-party", "claims": {"azp": provider.client}}))
            pending = flow({"sub": "provider-denied"})
            query = parse_qs(urlsplit(pending[0]).query)
            call("/api/v1/auth/oidc/callback?" + urlencode({"state": query["state"][0], "error": "access_denied"}), expected=401)
            finish(pending, 401)
            pending = flow({"sub": "expired-state"})
            sql("UPDATE oidc_authorizations SET expires_at=now()-interval '1 second' WHERE consumed_at IS NULL")
            finish(pending, 401)
            replay = flow({"sub": "one-time-state"})
            finish(replay)
            finish(replay, 401)
            print("signed_token_validation_provider_error_redaction_and_state_consumption=ok")

            linked = finish(flow({"sub": "explicit-admin", "claims": {"amr": ["mfa", "otp"], "acr": "strong"}}, admin["access_token"]))
            assert linked["user_id"] == admin["user_id"]
            assert not call("/api/v1/auth/assurance", token=linked["access_token"])["session_verified"]
            call("/management/v1/organisations", token=linked["access_token"], expected=404)
            finish(flow({"sub": "new-jit"}, admin["access_token"]), 409)
            separate = call("/api/v1/auth/login", {**credentials, "second_factor": recovery.pop()})
            pending = flow({"sub": "revoked-link"}, separate["access_token"])
            call("/api/v1/auth/logout", {}, separate["access_token"], 204)
            finish(pending, 401)
            pending = flow({"sub": "expired-assurance"}, admin["access_token"])
            sql(f"UPDATE auth_sessions SET expires_at=now()-interval '1 second' WHERE id='{admin['session_id']}'")
            finish(pending, 401)
            assert sql("SELECT count(*) FROM oidc_identities WHERE subject IN ('revoked-link','expired-assurance')") == "0"
            print("explicit_link_current_session_and_no_untrusted_idp_assurance_inference=ok")

            pending = [flow({"sub": "concurrent-new"}) for _ in range(2)]
            with ThreadPoolExecutor(max_workers=2) as executor:
                sessions = list(executor.map(finish, pending))
            assert sessions[0]["user_id"] == sessions[1]["user_id"]
            pending = [flow({"sub": f"email-race-{index}", "email": "shared-race@example.test"}) for index in range(2)]
            def competing(pending):
                try:
                    return finish(pending)["user_id"]
                except AssertionError as error:
                    assert "got 409" in str(error)
                    return None
            with ThreadPoolExecutor(max_workers=2) as executor:
                sessions = list(executor.map(competing, pending))
            assert sum(value is not None for value in sessions) == 1
            assert sql("SELECT count(*) FROM oidc_identities WHERE subject LIKE 'email-race-%'") == "1"
            assert provider.secret not in (root / "server.log").read_text()
            print("concurrent_identity_creation_and_same_email_collision=ok")

            # Different accounts cannot concurrently claim the same external identity.
            another = finish(flow({"sub": "another-account"}))
            pending = [flow({"sub": "link-race"}, session["access_token"]) for session in [jit, another]]
            with ThreadPoolExecutor(max_workers=2) as executor:
                owners = list(executor.map(competing, pending))
            assert sum(value is not None for value in owners) == 1
            assert sql("SELECT count(*) FROM oidc_identities WHERE subject='link-race'") == "1"
            assert sql("SELECT user_id FROM oidc_identities WHERE subject='link-race'") in [jit["user_id"], another["user_id"]]

            pending = flow({"sub": "deleted-link-owner"}, another["access_token"])
            sql(f"UPDATE users SET deleted_at=now() WHERE id='{another['user_id']}'")
            finish(pending, 401)
            finish(flow({"sub": "another-account"}), 401)
            print("concurrent_account_link_ownership_and_deleted_account_denial=ok")

            # Audit availability is part of the transaction that grants identity access.
            sql("CREATE FUNCTION fail_oidc_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action IN ('auth.oidc.link','auth.oidc.login') THEN RAISE EXCEPTION 'acceptance audit unavailable'; END IF; RETURN NEW; END $$; CREATE TRIGGER acceptance_oidc_audit BEFORE INSERT ON audit_events FOR EACH ROW EXECUTE FUNCTION fail_oidc_audit();")
            finish(flow({"sub": "audit-failed-provision"}), 500)
            finish(flow({"sub": "audit-failed-link"}, jit["access_token"]), 500)
            assert sql("SELECT count(*) FROM oidc_identities WHERE subject LIKE 'audit-failed-%'") == "0"
            assert sql("SELECT count(*) FROM users WHERE email='audit-failed-provision@example.test'") == "0"
            sql("DROP TRIGGER acceptance_oidc_audit ON audit_events; DROP FUNCTION fail_oidc_audit();")
            assert finish(flow({"sub": "audit-failed-link"}, jit["access_token"]))["user_id"] == jit["user_id"]
            print("audit_failure_rolls_back_account_and_identity_binding=ok")
            print("enterprise_oidc_database_acceptance=passed")
        finally:
            stop()
            log.close()
            provider.close()
            subprocess.run([str(pg / "pg_ctl"), "-D", str(data), "stop", "-m", "fast", "-w"], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
