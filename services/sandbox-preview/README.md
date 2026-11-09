# Sandbox preview gateway

`cowork-sandbox-preview --config /run/private/keys/preview-config.json` serves
authenticated HTTP and WebSocket previews on port 8080 behind TLS ingress. It is
a separate service and browser origin. The preview base domain must belong to a
different site from the main app (for example `previews.example.net` versus
`cowork.example.com`), not a sibling beneath the app's base domain.

The daemon issues a short-lived runner-signed grant after resolving the actual
live sandbox, Pod UID and selected preview service. Its URL carries the grant in
the fragment. The gateway's fixed bootstrap page removes that fragment before
posting it to the same origin. A successful live validation sets a Secure,
HttpOnly, SameSite=Strict, host-only `__Host-cowork-preview` cookie. The ticket is
never placed in a query, app cookie or guest header. Each wildcard hostname must
match its exact signed grant.

Every HTTP request and WebSocket upgrade verifies the runner signature, pinned
runner generation, namespace, fixed service DNS suffix, allowed application port,
hostname and expiry. It then calls the runner's authenticated
`POST /v2/sandbox-previews/validate` to check the exact live sandbox/Pod/Service
identity. WebSockets revalidate every two seconds and close when revoked or the
ticket expires. This gateway has no Kubernetes credentials or signing key. The
validator bearer is dedicated to validation and must not grant the general
process-runner API.

Configuration fields:

```json
{
  "format": 1,
  "listen": "0.0.0.0:8080",
  "preview_domain": "previews.example.net",
  "app_origin": "https://cowork.example.com",
  "namespace": "cowork-tenant-example",
  "service_suffix": ".cowork-tenant-example.svc.cluster.local",
  "allowed_ports": [8081],
  "runner_id": "operator-provisioned-runner-id",
  "runner_generation": 1,
  "runner_public_key": ["replace with the exact 32-byte numeric public key"],
  "runner_url": "https://cowork-process-runner.cowork-system.svc.cluster.local",
  "runner_bearer_file": "/run/private/keys/preview-validator-bearer",
  "runner_ca_file": "/run/private/keys/runner-ca.pem",
  "max_ticket_ms": 60000
}
```

The public-key example is explanatory, not runnable configuration. The deployment
renderer supplies the exact pinned numeric key and copies configuration and
validator credentials to ordinary owner-only files. Direct symlink/ConfigMap
projections are refused by the private-file reader. The gateway can run as
nonroot with a read-only root filesystem; it writes no state or temporary files.

Only configured application ports are reachable; sandbox control ports 8080 and
9090 are rejected. Requests forward a small allowlist of content/range headers.
Browser authorization, cookies, proxy and forwarding headers are stripped, as
are guest Set-Cookie headers. Redirects may remain only on the same signed guest
origin and are rewritten to the external preview origin. Responses disable
caching, referrers, framing and workers. CSP permits scripts and connections only
within the preview origin; dependencies must be served by the preview app.
Configure development apps to serve on `0.0.0.0:8081`, accept the selected service
host and use the external HTTPS/WSS origin for HMR. Application message sizes are
bounded at 8 MiB; ordinary HTTP responses at 32 MiB. Ticket expiry also closes
active WebSockets. Create a fresh link in the chat when the ticket expires.

Kubernetes deployment, wildcard TLS ingress and namespace network policies are
rendered by the Agent Sandbox setup tooling. No production deployment is performed
by building or testing this service. `cargo test -p cowork-sandbox-preview` uses
real local HTTP and WebSocket fixtures for forwarding, credential stripping,
forged/expired/cross-sandbox tickets, redirects and live revocation.
