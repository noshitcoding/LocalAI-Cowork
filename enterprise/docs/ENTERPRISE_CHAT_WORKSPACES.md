# Persistent chat workspaces

Schema 50 adds an opt-in instance-server implementation of a persistent workspace
per chat for Linux server execution. It is a candidate, not installed on the live
schema-49 control plane or existing tenant instances. It is one part of the agreed
persistent-environment scope; catalog approvals, isolated package builds, complete
environment lifecycle and other execution backends remain outstanding.

## Storage and authorization

With `COWORK_CHAT_WORKSPACES_ENABLED=true`, newly created Linux server runs receive
a server-assigned workspace identity. Runs in the same chat use the same identity;
another chat, including a fork, receives a separate workspace. Public sandbox
input cannot select that identity. Existing project/run authorization still applies.
The database binds each identity to its project and chat, and freezes the binding
on each run. Updates cannot move a run to another chat's workspace or remove the
binding. No shared writable cross-chat mode is introduced.

Two workers serialize execution within a bound chat, including when request
transactions commit in a different order from their creation timestamps. A run
with an unresolved interrupted execution continues to block later work in that
chat. This does not promise conflict detection between an interactive terminal
and an agent explicitly working in the same chat.

Files and tools installed **inside `/workspace`** survive subsequent runs and
API/runner restarts. The container root remains read-only. Packages installed
elsewhere, machine-wide installation, reusable environment templates and the
complete extension-installation workflow are not provided by this change.

## Compatibility and lifecycle

The feature defaults to false. Old runs keep their original per-run volumes and
are not automatically merged into a new chat workspace. When first enabled for
an existing chat, only newly created runs receive its new persistent workspace.
Disabling allocation later affects new chats; a chat with an existing binding
continues using it. Rollbacks must account for the new data and bindings.

Matching server and runner packages are required. The runner advertises
`chat_workspace_version: 1` and requires a non-nil `workspace_id` on the private
`/v2/jobs`, `/v2/desktop-sessions`, `/v2/runs/{run}/file` and
`/v2/runs/{run}/terminal` routes. All requests retain their existing HMAC signature,
nonce and timestamp checks. The `/v1` routes reject an explicit workspace, and
the `/v2` routes reject a missing binding. An older runner returns an error;
the server does not retry using a per-run workspace. This is an internal runner
API revision, not an implementation or advertisement of device protocol v3.

Raw sandbox runs, agent tools, Crew execution, desktop sessions, terminal streams
and artifact exports resolve the same immutable database binding. PostgreSQL
dispatch/session fences reject unaware worker/API writes for bound runs.

Volume names retain the existing instance/generation prefix and `run-<uuid>`
inventory format used by the Compose operator. For new bindings that UUID names
the workspace rather than an individual run. Instance/generation scoping still
applies. Full backup/restore, canary/promotion, deletion and the production runner
packaging must be qualified with these new bindings before fleet activation;
matching filename patterns alone do not establish that qualification. Existing
control-plane and tenant image selections have not been changed.

## Verification

`cargo test -p cowork-contracts -p cowork-server -p cowork-runner` covers the Rust
contracts and the runner's explicit workspace/route pairing. The real Docker
acceptance harness is:

```powershell
node app/scripts/test-chat-workspaces.mjs
```

It requires the separately built `cowork-server:chat-workspaces-candidate` and
`cowork-runner:chat-workspaces-candidate`, plus local core/GUI sandbox images.
Image overrides use `COWORK_WORKSPACE_SERVER_IMAGE`,
`COWORK_WORKSPACE_RUNNER_IMAGE`, `COWORK_WORKSPACE_CORE_IMAGE` and
`COWORK_WORKSPACE_GUI_IMAGE`. The generated fixture uses PostgreSQL and a real
API/worker/runner. It exercises persistent files/tools, separate chats, forged
bindings, mismatched project/chat contexts, concurrent workers, immutable database
bindings, legacy mode, signed artifact export, terminal/desktop access and restarts.
It does not use a live management administrator or enroll a human second factor.

Two dedicated subnets default to `10.254.247.0/28` and `10.254.247.16/28`;
`COWORK_WORKSPACE_TEST_CONTROL_CIDR` and `COWORK_WORKSPACE_TEST_EGRESS_CIDR`
override them. The trusted test services publish HTTP only on host loopback.
Core jobs use network `none`; GUI jobs use an internal bridge with an unreachable
fixture proxy. This is not a public-web browsing or hostile-extension test.

On Windows, stage the harness in a local temporary directory when the synchronized
project root cannot be loaded reliably, and set `COWORK_WORKSPACE_TEST_ROOT` to
the repository to select its private result directory. Successful runs record
`.internal/enterprise-local/chat-workspaces-docker-results.json`. Read its actual
checks and image identities; a harness existing on disk is not passing evidence.
No VM, Kubernetes, offline-device, browser UI, model/Crew end-to-end or 40-task
acceptance is implied by this fixture.

Local qualification on 2026-09-28 passed 91 Rust tests (one external-S3 test
requires separate credentials and was ignored) and twelve real Docker checks.
The Docker checks include the authenticated server API's terminal ticket/stream
and desktop-session paths, in addition to direct signed runner requests. All
generated fixture resources were removed; all 39 previously running containers
retained their IDs/images. The live management service remains healthy at schema
49 with seven ready instances and no active lifecycle operations. Build source
hashes, tested image identities and limitations are in
`.internal/enterprise-local/chat-workspaces-results.json`. No fleet activation
is implied by this candidate qualification.
