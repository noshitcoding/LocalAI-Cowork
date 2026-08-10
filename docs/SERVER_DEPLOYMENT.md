# Single-host Open Cowork server deployment

The Compose stack exposes exactly one HTTP upstream through Caddy. PostgreSQL,
MinIO, API, worker, runner and web containers have no host ports.

> Security status: the bootstrap bearer token is accepted only while no user
> exists. The first call creates an Argon2id-protected platform admin and returns
> short-lived access plus rotating hashed refresh tokens. TOTP, one-time recovery
> codes, WebAuthn passkeys, OIDC, native Authorization Code + PKCE, revocable
> executor credentials, time-limited support grants, user/team quotas, and
> team/project/pool RBAC are implemented and covered by E2E tests. Runner
> isolation, N-1 upgrade/rollback, a logical 20-GiB snapshot, and the reference
> 100-user/25-run load have executable acceptance tests. Physical Office,
> Android, identity-provider, physical NPM releases, kernel-policy and prolonged
> soak matrices remain release gates for an untrusted public deployment. The
> repository does exercise the recommended Nginx/NPM-compatible TLS directives
> through Caddy for web, SSE, binary WebSockets and a large upload.

Storage failure behavior is executable with `npm --prefix app run
test:storage-chaos`. A disposable TCP fault boundary in front of MinIO proves
that failed uploads leave no partial database metadata, uploads resume after
connectivity returns, active upload reservations block collection, abandoned
reservations are released, and failed object deletion returns to a retryable
state. The test never stops the shared MinIO container.

Run `npm --prefix app run test:storage-pressure` for the sustained storage
gate. The default run holds eight concurrent clients for 90 seconds while the
worker performs GC. It mixes shared and unique encrypted chunks, verifies every
download digest, deletes every manifest, waits for zero remaining chunk rows,
rejects negative refcounts and enforces a one-GiB peak-process envelope. The
reference run completed 1,804 roundtrips with a 635-ms p95 and sub-48-MiB API
peak working set.

## 1. Prepare secrets

Copy `deploy/.env.example` to `deploy/.env`. In `deploy/secrets/`, create:

- `bootstrap_token.txt` — random, at least 32 characters
- `postgres_password.txt`
- `database_url.txt` — `postgres://cowork:PASSWORD@postgres:5432/cowork`
- `minio_root_user.txt`
- `minio_root_password.txt`
- `runner_signing_key.txt` — independent random key, at least 32 characters
- `storage_master_key.txt` — exactly 32 random bytes encoded as base64 (for
  example `openssl rand -base64 32`); back this up separately from the object
  store because snapshots cannot be decrypted without it

Restrict those files to the deployment administrator. If the PostgreSQL
password contains URI-reserved characters, percent-encode it in
`database_url.txt`.

Configure `COWORK_MODEL_BASE_URL`, `COWORK_MODEL_NAME` and optionally
`COWORK_MODEL_API_KEY` for server model runs. This key is injected only into the
worker/control-plane environment, never into a sandbox or browser client.

Set `COWORK_PUBLIC_ORIGIN` to the one canonical HTTPS origin, without a path,
and `COWORK_WEBAUTHN_RP_ID` to its stable relying-party domain. For example:

```dotenv
COWORK_PUBLIC_ORIGIN=https://cowork.example.com
COWORK_WEBAUTHN_RP_ID=cowork.example.com
```

Changing the RP ID makes existing passkeys unusable. Redirect alternate
subdomains to the canonical origin; do not serve the same web UI from several
origins and expect one passkey ceremony to validate on all of them.

### Optional OpenID Connect

Register this exact confidential-client redirect URI at the provider:

```text
https://cowork.example.com/api/v1/auth/oidc/callback
```

Create `deploy/secrets/oidc_client_secret.txt`, set `COWORK_OIDC_ISSUER` and
`COWORK_OIDC_CLIENT_ID`, then add the optional Compose overlay to every Compose
command:

```bash
docker compose -f docker-compose.yml -f docker-compose.oidc.yml config --quiet
docker compose -f docker-compose.yml -f docker-compose.oidc.yml up -d
```

The server performs Discovery and validates the exact issuer, JWKS signature,
audience, expiry, nonce, optional access-token hash, and S256 PKCE. Provider
tokens are not returned to clients. `COWORK_OIDC_AUTO_PROVISION=false` is the
safe default. Enabling it trusts the configured provider's verified `email`
claim to link an existing account or create an OIDC-only account, so use it
only with an identity provider and tenant controlled by the operator.

## 2. Validate and start

On the Linux host, install and enforce the version-controlled sandbox AppArmor
profile before starting Compose:

```bash
sudo ./deploy/security/install-apparmor.sh
sudo aa-status | grep open-cowork-sandbox
```

The production Compose configuration also forces Docker's current built-in
Seccomp allowlist for every job, terminal, artifact helper and GUI sandbox. The
runner stays unhealthy when Docker lacks Seccomp, AppArmor is unavailable, the
named profile was not loaded, or a real probe container cannot start with both
profiles. `unconfined` is rejected for either setting. Operators targeting a
different Linux security module must provide and test an equivalent deployment
overlay; silently disabling these controls is not a supported production mode.

```bash
cd deploy
docker compose config --quiet
docker compose pull
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:8080/readyz
```

Tagged releases publish server, runner, web, egress and sandbox images to GHCR
with SBOM and provenance attestations. `SERVER-IMAGES.txt` in the matching
GitHub release records immutable digests. For a reviewed source checkout, set
`COWORK_UPGRADE_BUILD_FROM_SOURCE=1` and run `docker compose build --pull`
instead. The runner starts only after both pinned sandbox images exist.

The bundled object store uses path-style MinIO by default. An external
S3-compatible bucket can be selected with `COWORK_S3_ENDPOINT`,
`COWORK_S3_REGION`, `COWORK_S3_BUCKET` and
`COWORK_S3_ADDRESSING_STYLE=path|virtual_hosted`; both API and worker must use
identical values. Put the external access key and secret key into the existing
Docker-secret source files (or override their mounts). Temporary AWS-style
credentials are supported through `COWORK_S3_SESSION_TOKEN` or
`COWORK_S3_SESSION_TOKEN_FILE`; use a Compose secret in an operator overlay,
never a committed `.env` value. Endpoints may not contain embedded credentials,
query strings, fragments or path prefixes. Virtual-hosted mode requires a
DNS-compatible bucket and DNS endpoint.

Use the external-storage overlay so bundled MinIO and its initializer are not
started and the API/worker dependency graph no longer waits for them:

```bash
docker compose \
  -f docker-compose.yml \
  -f docker-compose.external-s3.yml \
  config --quiet
docker compose \
  -f docker-compose.yml \
  -f docker-compose.external-s3.yml \
  up -d
```

Set `COWORK_S3_ACCESS_KEY_PATH` and `COWORK_S3_SECRET_KEY_PATH` to restricted
host files. Compose v2.24 or newer is required for the fail-closed `!override`
merge tags. When external storage is selected, `backup.sh`/`restore.sh` cover
PostgreSQL and deployment secrets only; use the provider's bucket versioning,
replication and restore procedure for objects.

For temporary credentials, additionally set `COWORK_S3_SESSION_TOKEN_PATH` and
append `-f docker-compose.external-s3-session.yml` to both Compose commands.
The token is mounted as a Docker secret and included in the SigV4 signed-header
set; it is not exposed to the web or Android clients.

Provider acceptance uses a dedicated, pre-created test bucket and never creates
or deletes the bucket itself:

```powershell
$env:COWORK_S3_TEST_ENDPOINT = 'https://s3.eu-central-1.amazonaws.com'
$env:COWORK_S3_TEST_REGION = 'eu-central-1'
$env:COWORK_S3_TEST_BUCKET = 'open-cowork-compat-unique'
$env:COWORK_S3_TEST_ADDRESSING_STYLE = 'virtual_hosted'
$env:COWORK_S3_TEST_ACCESS_KEY = '<temporary access key>'
$env:COWORK_S3_TEST_SECRET_KEY = '<temporary secret key>'
$env:COWORK_S3_TEST_SESSION_TOKEN = '<temporary session token>'
cargo test -p cowork-server storage::tests::external_s3_provider_acceptance -- --ignored --exact
```

The test writes one random encrypted object, verifies the envelope-encryption
roundtrip and deletes that object. Scope credentials to only the dedicated
bucket and expire them after the matrix run.

`.github/workflows/storage-compat.yml` runs the same acceptance weekly against
the digest-pinned 2025-04-22 container line and the 2024-06-26 N-1 baseline.
Both rows have also passed locally. A manual dispatch can add AWS S3,
Cloudflare R2 or another compatible provider using protected
`COWORK_S3_COMPAT_*` environment secrets and the `storage-compatibility` GitHub
environment. The workflow fails closed when any required protected value is
missing and never prints credentials.

## 3. Put Nginx Proxy Manager in front

Create one Proxy Host for the canonical domain. Use `http`, the server/private
address and port `8080`; enable WebSocket support and request a TLS certificate.
Do not create separate public hosts for PostgreSQL, MinIO, the runner or GUI
streaming.

Recommended Advanced directives:

```nginx
client_max_body_size 0;
proxy_request_buffering off;
proxy_buffering off;
proxy_read_timeout 86400s;
proxy_send_timeout 86400s;
send_timeout 86400s;
```

With `COWORK_BIND_ADDRESS=127.0.0.1`, NPM must reach the host loopback through a
host-network deployment or a host-gateway name. Otherwise bind Caddy to an
explicit private host address. If NPM and Open Cowork share an operator-managed
Docker network, add the gateway container to that network and proxy to
`gateway:8080`; no host port is then required. Never bind Caddy broadly merely
to make container DNS easier.

SSE and WebSocket upgrades stay on the same domain. Caddy forwards them
automatically; the long timeouts prevent terminal/desktop/event streams from
being closed by NPM.

Caddy applies the repository-owned browser policy after proxying: a
default-deny Content Security Policy, frame/object denial, one-year HSTS,
content-type protection, a restricted Permissions Policy and same-origin
opener/resource policies. Keep these headers at Caddy so an NPM configuration
change cannot silently remove them. If an operator adds an external asset or
connection, review and narrow the CSP explicitly instead of disabling it.

Run `npm --prefix app run test:gateway` to reproduce the isolated TLS proxy
acceptance. It creates a disposable Nginx/NPM-compatible edge and Caddy stack,
publishes exactly one loopback TLS port, and verifies web content, SSE, binary
GUI WebSocket frames, the complete browser security-header set and an
unbuffered 20-MiB upload. It does not replace the
release gate against the exact Nginx Proxy Manager version used in production.

## 4. Create the first administrator and smoke-test

```bash
TOKEN="$(cat deploy/secrets/bootstrap_token.txt)"
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  --data '{"email":"admin@example.com","display_name":"Admin","password":"replace-with-a-long-password","device_id":"00000000-0000-0000-0000-000000000001"}' \
  https://cowork.example.com/api/v1/auth/bootstrap
```

Store the returned refresh token in the platform credential store and use the
short-lived access token for API calls:

```bash
ACCESS_TOKEN="returned-access-token"
curl -H "Authorization: Bearer $ACCESS_TOKEN" \
  https://cowork.example.com/api/v1/version
curl -H "Authorization: Bearer $ACCESS_TOKEN" \
  https://cowork.example.com/api/v1/capabilities
```

The generated, unauthenticated protocol documents are served from the same
canonical origin at `/api/v1/openapi.json` and
`/api/v1/schemas/contracts.json`. The matching release also contains
`openapi-v2.json` and `contracts-v2.schema.json`. CI regenerates both from the
Axum route graph and Zod runtime contracts, rejects drift or unresolved
references, and compares every protocol-v2 contract against the retained v1 N-1
baseline for removed fields, narrowed enums, changed types, or new required
properties.

After successful bootstrap, the bootstrap token is rejected by every endpoint.
Password login is `POST /api/v1/auth/login`; rotation is
`POST /api/v1/auth/refresh`; authenticated logout revokes the whole session.
Desktop and Android keep the rotating refresh token only in their protected
credential store. The web build is locked to its own canonical origin and uses
`POST /api/v1/auth/browser/refresh`: the server removes the refresh token from
the JSON response and rotates a `Secure`, `HttpOnly`, `SameSite=Strict`,
host-only cookie. Logout and rejected replay clear that cookie; presenting an
old rotated token revokes the complete session family. Reproduce the real HTTPS
login/reload/replay/logout acceptance with `npm --prefix app run
test:browser-session` (it uses a disposable PostgreSQL database).
Users add passkeys in the web app on the canonical server origin. Desktop and
Android use the system browser for the WebAuthn ceremony, then return through
the registered `open-cowork://auth/callback` scheme. The callback carries only
a short-lived code and state; token exchange additionally requires the
Keystore/Credential-Store protected PKCE verifier and the original device ID.
OIDC uses the same protected final exchange, but its provider flow has a
separate state, nonce, and PKCE verifier. Web clients return to
`/auth/callback`; native clients return to the fixed app scheme.

A run body needs stable project/thread IDs, an idempotency key and an explicit
target. A private project without a snapshot intentionally remains
`waiting_for_snapshot`.

## 5. Personal daemon

The official Windows and Linux desktop packages contain the matching daemon.
On first desktop start the app verifies its packaged SHA-256 manifest, copies a
versioned executable into the private per-user daemon directory and starts it
detached. Windows registers the installed path in the current user's `Run` key.
Linux registers a user systemd unit, with XDG autostart as the fallback when a
user systemd manager is unavailable. No administrator rights are used.

The daemon creates a 256-bit local IPC token and persistent device UUID on first
start. Defaults are `%LOCALAPPDATA%\OpenCowork\daemon` on Windows and
`$XDG_STATE_HOME/open-cowork/daemon` (or `~/.local/state/open-cowork/daemon`)
on Linux. Token/identity creation is atomic, Unix modes are `0700`/`0600`, and
a cross-process file lock rejects duplicate daemon workers. The desktop bridge
uses the same authenticated Named Pipe or Unix Domain Socket.

For development builds, prepare the packaged daemon with:

```powershell
npm --prefix app run prepare:local-daemon
npm --prefix app run test:local-daemon
```

`COWORK_DAEMON_*` and `COWORK_MODEL_*` environment variables remain available
as development/managed overrides. Normal desktop installations need no manual
token, device-ID, service or Task Scheduler configuration. The Windows
uninstaller removes the login registration and versioned executable while
preserving the SQLite run data and identity for a non-destructive reinstall.

The daemon marks unfinished work `interrupted` after restart and does not repeat
unsafe work automatically.

Linux `SIGTERM`, Windows console close/logoff/shutdown events and an invisible
top-level `WM_QUERYENDSESSION` window all enter the same graceful path. That path
stops the worker first, persists active safe runs as
`interrupted/daemon_shutdown`, preserves unsafe checkpoints for manual review
and only then exits. `daemon.shutdown` offers the same token-authenticated path
for upgrades. `npm --prefix app run test:local-daemon` proves this with a
deliberately stalled model request, a real Windows session-end message, restart
and post-restart state inspection.

## 6. Personal device executor and remote desktop policy

Register a personal executor through the authenticated `/api/v1/executors`
endpoint, create its revocable credential, then run `cowork-device-agent` in the
interactive user session. Connect that outbound agent to the existing personal
daemon with `COWORK_LOCAL_DAEMON_IPC_ENDPOINT` and either
`COWORK_LOCAL_DAEMON_IPC_TOKEN` or `COWORK_LOCAL_DAEMON_IPC_TOKEN_FILE`. The
agent's `COWORK_EXECUTOR_ID` must equal the daemon's stable device ID. Typical
endpoints are `\\.\pipe\open-cowork-<USERNAME>` on Windows and
`$XDG_RUNTIME_DIR/open-cowork/daemon.sock` on Linux. The agent refuses to
register if the identities differ or the authenticated daemon handshake fails.

With this bridge enabled, server-originated personal runs are imported under
their exact server Run ID and execute in the same durable Rust runtime used by
offline desktop runs. For private projects, an existing local project binding
is used without uploading the project; an explicitly supplied server snapshot
is instead materialized into an isolated temporary workspace. Model/tool events,
safe and unsafe checkpoints, approvals, input requests and declared artifacts
are relayed back to the control plane. Before execution the bridge inventories
the explicit `.coworkignore` boundary; after a changed run it uploads the full
result manifest as resumable content-addressed chunks. Completion atomically
creates a reviewable project version, links it to the Run and leaves the
project’s current version untouched until an Editor applies or merges it. A
WebSocket disconnect leaves the durable daemon run alive. Reconnecting the same
executor recovers its unexpired lease and resumes relay/upload with stable local
event, checkpoint, intervention and artifact IDs; repeated messages and chunks
are idempotent. A run that finishes while offline therefore only replays missing
data and is never executed a second time. Model settings from the agent are
encrypted by the daemon for that run and removed at terminal state; when they
are omitted, the daemon uses its own local model configuration.

Advertise only capabilities that the device and its daemon can actually serve,
for example `model.ollama,files,shell,git,web,mcp,browser.headless`. Add
`desktop.windows` on an interactive Windows device or `desktop.linux` on an
interactive Linux device. In addition
to the canonical HTTPS server URL, executor ID, credential file, bridge settings
and optional local model settings, configure exactly one remote-control policy:

- `COWORK_PERSONAL_REMOTE_CONTROL=off` rejects screen viewing and input control.
- `COWORK_PERSONAL_REMOTE_CONTROL=confirm_each_session` is the default. Windows
  displays a native local confirmation and Linux uses Zenity, KDialog, or
  XMessage before the first view or control stream; escalating a view-only
  session to input control requires its own confirmation.
- `COWORK_PERSONAL_REMOTE_CONTROL=unattended` permits authenticated remote
  viewing and control without a local dialog.

The server also enforces the advertised mode and permits personal-device GUI
sessions only for the device owner. Every control stream still requires fresh
account reauthentication and is audited. The agent independently enforces its
local setting, so a server request cannot bypass `off` or the confirmation
dialog. Desktop, Web and Android expose the owner-controlled server ceiling in
the **Devices** panel and show the stricter mode currently advertised by the
local agent. Executor credentials may refresh that status but cannot relax the
server ceiling. Linux streaming requires an interactive X11 `DISPLAY`,
`xdotool`, ImageMagick `import`, and `xclip` or `xsel`. Wayland-only sessions
must expose a compatible XWayland desktop to the agent. Local agent tools
continue to work independently of the remote-view policy.

## 7. Managed Windows executor

Build `cowork-device-agent.exe` and install it on a dedicated Windows VM/PC and
dedicated Windows account. Configure:

- `COWORK_AGENT_KIND=managed_windows`
- canonical HTTPS `COWORK_SERVER_URL`
- an executor ID and admin-created pool ID
- an agent token
- `COWORK_AGENT_CAPABILITIES=office.microsoft,desktop.windows`
- a dedicated `COWORK_AGENT_WORKSPACE_ROOT`

Start its supervisor at machine startup and the interactive component at
dedicated-user logon, not as Office automation inside Session 0. Its Office
adapter performs Word/Excel/PowerPoint editing and export through COM and GUI
automation with visible applications, forced macro disablement, dialog
detection, live desktop streaming, single-run concurrency, cleanup, and a
post-run health gate.

Microsoft does not recommend unattended Office server automation. Operate this
as best-effort interactive RPA on licensed Windows/Microsoft 365 installations
and run compatibility tests for every supported Office channel.

Executor agents use independently revocable, hashed executor credentials from
files or the platform credential store; they never depend on a user's
15-minute access token and connect only outbound.

## Quotas and temporary support

The Governance panel shows live logical snapshot storage, nonterminal runs,
monthly tokens, and configured model costs. Platform administrators set user
limits; team owners/admins set team limits. A configured cost limit always
requires a token fallback. Set both model price environment variables to record
costs; without known prices the token ceiling remains the hard guard.

Platform administrators have no implicit project-content access. A project
editor can grant one administrator project- or thread-scoped Viewer access for
at most 24 hours. Creation, use, and revocation are audited; Viewer access does
not grant terminal execution or project-version application.

## Backups and restore

Open Cowork intentionally has no in-app backup scheduler. Back up these as one
consistent set:

1. PostgreSQL (`pg_dump --format=custom` or storage snapshot)
2. MinIO/S3 bucket with object versions when enabled
3. Compose secret files, especially the envelope-encryption master key
4. Compose file and exact release version

Restore secrets first, then PostgreSQL and object storage, and start the exact
application version that produced the backup before upgrading. A PostgreSQL
dump without object blobs, or encrypted blobs without their master key, is not a
restorable backup.

The supplied operator workflow takes a maintenance-consistent PostgreSQL dump,
stops object-store writes before copying MinIO data, captures configuration and
secrets, and hashes every file:

```bash
chmod 700 backup.sh restore.sh upgrade.sh
./backup.sh /srv/open-cowork-backups
./restore.sh /srv/open-cowork-backups/20260808T120000Z --confirm
./upgrade.sh 0.3.1 /srv/open-cowork-backups
```

`restore.sh` refuses broad paths, verifies all SHA-256 checksums, requires an
explicit confirmation flag and enforces the exact backed-up application
version. Add `--restore-config` only when the backed-up Compose configuration
and secrets must replace the active deployment. `upgrade.sh` first creates a
consistent backup and automatically restores it together with the prior image
version when pulling, migration, startup or readiness fails. Practice restore
on a separate host before relying on a backup.

## Operations and privacy

- `/healthz` is process liveness; `/readyz` includes PostgreSQL readiness.
- Logs are structured JSON and remain local to Docker logging.
- No component contains external telemetry.
- MinIO console and PostgreSQL are intentionally not public.
- Operators must not call broad Docker volume-prune commands. Lifecycle cleanup
  removes only database-confirmed expired snapshots and workspaces.
- The worker removes event rows only for terminal runs whose events and finish
  time are older than 90 days. Per-run event cursors are stored independently,
  so SSE sequence numbers remain monotonic after retention. Waiting approvals
  and input requests expire after seven days; private snapshots default to and
  cannot exceed 30 days. Accepted project versions remain until project/user
  deletion policy removes them.
- Platform administrators can read aggregate local metrics at
  `/api/v1/operations/metrics` and download an audited JSON support bundle at
  `/api/v1/operations/support-bundle`. Both endpoints exclude prompts, files,
  names, email addresses, object keys, tokens and secrets; the web Governance
  panel exposes the same workflow.
- `scripts/test-server-load.ps1` is the reference 100-user/25-parallel-run
  acceptance test. On the recorded development host its latest p95 for 100
  simultaneous authorized project-list requests was 538 ms; operators must
  establish their own baseline and repeat the test on production-equivalent
  storage and networking.
- `scripts/test-run-chaos.ps1` kills a worker after a sandbox dispatch, proves
  that the unsafe action is not submitted twice, verifies safe/unsafe lease
  expiry classification, rejects late executor writes, and exercises executor
  reconnect plus capacity cleanup against an isolated PostgreSQL database.
  It also backdates a completed run, proves the 90-day event purge, and verifies
  that a later event cannot reuse a removed sequence number.
