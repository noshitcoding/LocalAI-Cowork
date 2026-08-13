# Setup and deployment selection

LocalAI Cowork supports three operating modes. Start with the smallest mode
that solves the problem; adding a server later does not invalidate local
projects or require uploading private files.

## Operating modes

| Mode | Keeps working without Internet | Continues after the laptop is off | Web/Android control | Office/desktop automation |
| --- | --- | --- | --- | --- |
| Standalone desktop | Yes | No | No | Local capabilities only |
| Desktop plus Linux server | Desktop work does | Server runs do | Yes | Chromium, Linux GUI, LibreOffice and OOXML |
| Server plus managed Windows pool | Desktop work does | Server and Windows runs do | Yes | Installed Microsoft Office and Windows UI |

The execution target is fixed when a Run is created. A Run never migrates from
the laptop to the server or between executor pools. Start a new Run when the
target must change.

## Standalone desktop

### Install a release

1. Download the Windows installer or Linux package from the
   [latest GitHub release](https://github.com/noshitcoding/LocalAI-Cowork/releases/latest).
2. Verify `SHA256SUMS` and, where available, the updater signature or package
   signature described by the release.
3. Start LocalAI Cowork. No server URL or LocalAI Cowork account is required.
4. Open **Settings → Models** and configure Ollama or an OpenAI-compatible
   provider. Provider credentials are stored through the native credential
   boundary, not in chat or Run input.
5. On Windows, open **Settings → AI Sandbox** and complete the one-time UAC
   setup before enabling shell or writable local file access.

For Ollama:

```powershell
ollama serve
ollama pull llama3.1:8b
```

The default endpoint is `http://localhost:11434`. See
[Ollama configuration](OLLAMA_CONFIGURATION.md) for vLLM/OpenAI-compatible
endpoint details and diagnostics.

### Run from source

```powershell
cd app
npm ci
npm run tauri dev
```

Use [Development setup](DEVELOPMENT.md) for exact toolchains, generated
artifacts and the complete test matrix.

## Optional durable local daemon

Packaged desktop builds install a hash-verified, versioned per-user daemon and
register it at user login. It owns durable local Runs, schedules and SQLite
state, so closing the UI does not cancel a local Run. It does not require an
administrator account.

Source developers can run it directly:

```powershell
cargo run -p cowork-local-daemon
```

The desktop and daemon communicate only through a per-user Named Pipe or Unix
Domain Socket protected by a 256-bit local token. See the
[daemon README](../agents/cowork-local-daemon/README.md) before overriding its
data directory, device ID or socket.

## Self-hosted Linux server

### Host prerequisites

- One supported Linux host with Docker Engine and Compose v2.24 or newer.
- A canonical HTTPS domain. Nginx Proxy Manager may terminate TLS and forward
  to the single Caddy upstream.
- Persistent storage sized for PostgreSQL, object data and retained artifacts.
- A tested backup destination outside the host.
- An external OpenAI-compatible model API for server-side model Runs. The
  Compose stack intentionally does not provide Ollama, vLLM or GPU hosting.

### Deployment sequence

```bash
git clone https://github.com/noshitcoding/LocalAI-Cowork.git
cd LocalAI-Cowork
cp deploy/.env.example deploy/.env
./deploy/init-secrets.sh
sudo ./deploy/security/install-apparmor.sh
docker compose --env-file deploy/.env -f deploy/docker-compose.yml config --quiet
docker compose --env-file deploy/.env -f deploy/docker-compose.yml up -d
curl --fail http://127.0.0.1:8080/readyz
```

Do not expose PostgreSQL, MinIO, the runner, VNC or any sandbox port. Caddy is
the only HTTP upstream. Before the first `up`, set the canonical origin,
WebAuthn RP ID, image version, model configuration and all secret-file paths.

Continue with [Server deployment](SERVER_DEPLOYMENT.md). It documents:

- bootstrap administration, invitations, TOTP, passkeys and optional OIDC;
- Nginx Proxy Manager headers for HTTP, SSE, WebSocket and large uploads;
- AppArmor/Seccomp and Docker runner isolation;
- external S3, FCM, WebPush, quotas, support grants and MCP bindings;
- backup, restore, upgrade, rollback and acceptance commands.

### Connect clients

- Desktop: configure the one canonical server origin and complete the native
  Authorization Code + PKCE login. Standalone mode remains available.
- Web: open the canonical origin served by the Compose gateway.
- Android: build or install the signed internal APK and log in to the same
  canonical origin. See [Android](../clients/android/README.md).
- Personal device: register an executor credential and run the outbound agent
  beside the local daemon.
- Managed Windows: create a pool, grant it to the required team/project and
  register each dedicated executor with an outbound credential.

## Private and team project behavior

- Private-project metadata can be synchronized, but files stay local until the
  user explicitly creates a Run snapshot for a remote target.
- Server snapshots are encrypted, versioned and retained according to the
  configured policy. A missing private snapshot leaves the Run visibly in
  `waiting_for_snapshot`.
- Team projects keep a server-side version history. Run results create a new
  version and diff; they are never silently applied to a desktop checkout.
- `.coworkignore` and project snapshot rules are the upload boundary. Secret
  detection warns but does not silently change the selected snapshot.

## Managed Windows and Microsoft Office

Real Word, Excel and PowerPoint automation requires a dedicated interactive
Windows VM or PC with appropriate Windows/RDS/Microsoft 365 licensing. It is not
implemented as an unattended Office server service. One interactive executor
handles at most one Run at a time; scale by adding executors.

Use the [device-agent README](../agents/cowork-device-agent/README.md) and the
[Windows section of the server guide](SERVER_DEPLOYMENT.md#7-managed-windows-executor).
Macros, add-ins and external data connections remain disabled by default.

## Verify before real data

At minimum:

```powershell
cd app
npm run test:ci
npm run build
```

```bash
cargo fmt --all -- --check
cargo test --workspace
docker compose --env-file deploy/.env -f deploy/docker-compose.yml config --quiet
```

Then run the environment-specific acceptance tests listed in
[Implementation status](DISTRIBUTED_IMPLEMENTATION_STATUS.md). Use disposable
projects and non-sensitive copies until backup/restore, executor cleanup and
your actual reverse proxy have passed.
