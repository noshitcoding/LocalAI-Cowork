# Open Cowork single-host deployment

This directory contains the optional self-hosted Linux control plane. It
publishes one configurable HTTP port through Caddy; PostgreSQL, MinIO, workers,
the Docker runner and GUI services remain internal.

> Start with the complete [server deployment guide](../docs/SERVER_DEPLOYMENT.md).
> The commands below are an operator checklist, not a replacement for the
> security, backup and reverse-proxy sections in that guide.

## Components

| Compose service | Purpose | Host port |
| --- | --- | --- |
| `gateway` | Caddy entry point for Web, REST, SSE and WebSocket | One configured port |
| `web` | Static shared React Web client | None |
| `api` | Axum control plane and authentication | None |
| `worker` | PostgreSQL-backed scheduler and durable Run worker | None |
| `runner` | Validates signed job specs and owns Docker access | None |
| `sandbox-core` | Pinned shell/file/Git/Web/MCP image | None |
| `sandbox-crew` | Pinned CrewAI image | None |
| `sandbox-gui` | Chromium, virtual display and LibreOffice image | None |
| `postgres` | Metadata, queues, auth, events and revisions | None |
| `minio` | Default encrypted object storage | None |
| `egress-proxy` | Filtered public Internet route for opted-in sandboxes | None |

No job container receives the Docker socket, host project paths or persistent
provider/storage credentials.

## First deployment

```bash
cp deploy/.env.example deploy/.env
./deploy/init-secrets.sh
sudo ./deploy/security/install-apparmor.sh
docker compose --env-file deploy/.env -f deploy/docker-compose.yml config --quiet
docker compose --env-file deploy/.env -f deploy/docker-compose.yml pull
docker compose --env-file deploy/.env -f deploy/docker-compose.yml up -d
docker compose --env-file deploy/.env -f deploy/docker-compose.yml ps
curl --fail http://127.0.0.1:8080/readyz
```

Before `up`, review every value in `deploy/.env` and back up the secret files.
In particular:

- `COWORK_PUBLIC_ORIGIN` is the one canonical HTTPS origin.
- `COWORK_WEBAUTHN_RP_ID` is permanent for existing passkeys.
- `COWORK_VERSION` selects one compatible image release.
- `COWORK_STORAGE_MASTER_KEY_PATH` points to a separately backed-up 32-byte
  base64 key. Losing it makes encrypted snapshots unrecoverable.
- The server model endpoint is external; this stack intentionally has no GPU,
  Ollama or vLLM service.

## Reverse proxy

Keep `COWORK_BIND_ADDRESS=127.0.0.1` when Nginx Proxy Manager reaches Caddy via
the host. If both are containers, attach only the gateway to an explicit shared
proxy network and do not publish additional service ports.

The proxy must preserve:

- normal HTTP requests and large resumable uploads;
- SSE without response buffering;
- WebSocket upgrades and long-lived timeouts;
- the original HTTPS scheme and host.

Redirect alternate subdomains to the canonical domain. Do not serve independent
authenticated origins backed by the same deployment.

## Optional overlays

- `docker-compose.oidc.yml`: OpenID Connect confidential client
- `docker-compose.fcm.yml`: server-side Firebase Cloud Messaging credentials
- `docker-compose.external-s3.yml`: remove bundled MinIO and use external S3
- `docker-compose.external-s3-session.yml`: temporary S3 session token
- `docker-compose.e2e.yml`: disposable acceptance environment only

Pass the same ordered `-f` list to `config`, `pull`, `up`, `down` and upgrade
commands. Compose v2.24 or newer is required for fail-closed override tags.

## Lifecycle

```bash
./deploy/backup.sh
./deploy/upgrade.sh
./deploy/restore.sh
```

Read each script and the deployment guide first. A complete recovery set needs
PostgreSQL, object data, deployment configuration and secrets, especially the
storage master key. External S3 objects are backed up with the provider's own
versioning/replication process.

## Health and troubleshooting

```bash
docker compose --env-file deploy/.env -f deploy/docker-compose.yml ps
docker compose --env-file deploy/.env -f deploy/docker-compose.yml logs --tail=200 gateway api worker runner
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
```

The runner intentionally stays unhealthy when Docker Seccomp, the named
AppArmor profile or a real hardened probe container is unavailable. Do not fix
that condition by selecting `unconfined`.

## Production decision

The repository has executable tests for isolation, upgrades, large snapshots,
storage faults, quotas, authentication and reference load. Physical Android,
Office, identity-provider, Nginx Proxy Manager and prolonged soak acceptance
must still be executed in the operator's actual environment. See the
[implementation status](../docs/DISTRIBUTED_IMPLEMENTATION_STATUS.md) before
exposing the stack to untrusted tenants.
