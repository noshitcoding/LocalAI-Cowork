# Private desktop job proofs

The legacy signed runner protocol remains the default. Set
`COWORK_RUNNER_DESKTOP_JOB_PROOFS=true` only with native runner TLS:

- `COWORK_RUNNER_TLS_CERTIFICATE_FILE`: absolute path to a PEM certificate chain.
- `COWORK_RUNNER_TLS_PRIVATE_KEY_FILE`: absolute path to one PEM private key.
- `COWORK_RUNNER_TLS_CLIENT_CA_FILE`: optional PEM client trust roots. This enables
  mutual TLS and requires clients to provide an enrolled certificate. The server's
  initial desktop proof client uses server authentication only; leave this unset
  for that client.

Files must be regular, without symlink or hardlink aliases, and at most 1 MiB.
On Linux the private key must have no group/other permission bits. The certificate
must cover the hostname in the enrolled HTTPS runner URL. TLS carries the original
request path, query and body into the existing HMAC verifier; it does not replace
request signatures. Handshakes have a five second deadline and at most 64 pending
connections. Partial TLS configuration fails startup.

Keep `COWORK_RUNNER_ADMISSION_DIRECTORY` persistent and writable by the runner.
Its `runner-identity` freezes a generated journal UUID, the genuine Docker Engine
ID and the resource scope. Private desktop intents and confirmed bindings remain
in `desktop-identities`, including after stop. An engine change, missing identity
with retained private desktop inventory, or an inconsistent binding fails closed.
Legacy desktop jobs have no receipt and cannot be adopted as private proof jobs.

Private creation uses the existing public desktop spec and result, with signed
query `?lease_token=<UUID>&attempt_id=<UUID>`. The runner persists the attempt and
intent before reserving capacity or creating a container. The response includes
base64url `x-cowork-desktop-binding` (a `LiveJob` JSON object) and request-bound
`x-cowork-desktop-proof`. The binding contains the journal/Engine identity, exact
container ID, Created/StartedAt, scope, run/lease/attempt/session/workspace and
admission reservation. No receipt is issued for an unknown or non-running start;
the same attempt is never replayed.

`POST /v1/desktop-session-probe` receives `{version:1,challenge,binding}` and returns
`{version:1,challenge,job}` after read-only exact-ID inspection. A fresh challenge
is bound to the response MAC. RFB and DELETE requests require signed
`binding_sha256=<SHA256 of serialized JobBinding>`; RFB also accepts `control`.
The field order of `JobBinding` is version, runner_id, engine_id, scope_prefix,
reservation_id, attempt_id, container_id, container_name, created_at, started_at,
run_id, session_id, lease_token, workspace_id, workspace_volume.

Creation, probe and desktop stop use separate response-signing domains. They do
not share the run-wide execution-stop or no-start response domains. RFB rechecks
the binding immediately before relaying to the immutable inspected Docker ID.
A desktop restart (changed StartedAt), replacement, pause, or pending stop is
rejected.

DELETE persists stop uncertainty before Docker I/O and stops the exact ID. Only
daemon-wide confirmed absence returns HTTP 200 with
`{version:1,binding,execution_stopped:true}` and a desktop-stop proof. A repeated
request after uncertain stop performs read-only inspection: a remaining job is
held for operator reconciliation, and confirmed absence returns the same binding
without another physical stop. Unknown starts/stops retain their reservation and
workspace. Timeouts and runner restarts do not release capacity.

## Native Kubernetes runner

`COWORK_RUNNER_BACKEND=kubernetes` selects a separate native backend. It requires
HTTPS and the original instance/organisation/executor-host, cluster/namespace
UIDs, storage generation, exact Node UIDs, RuntimeClass, StorageClass and private
admission directory supplied by the enterprise instance chart. It uses the
in-cluster ServiceAccount; every kubectl invocation removes ambient KUBECONFIG
and uses its own writable private HOME/cache. A journal containing `.kube` is
rejected. No Docker inspection or Docker socket is used by this backend.

Core/Crew jobs journal the original attempt, lease and body before PVC/Pod
creation and execute once in the acknowledged Pod UID. Transfer Pods serve
artifact reads and terminals from original PVC receipts. The authenticated
scope API provides durable quiesce/activation; a stopped instance retains its
control Pod while denying new workload launches. Read-only `--kubernetes-journal-inspect`
emits actual capacity reservations without constructing a backend or changing
the ledger.

`COWORK_RUNNER_KUBERNETES_DESKTOP_PROOFS=true` requires a separate original GUI
node profile and is mutually exclusive with Docker desktop proofs. Native
creation/probe/stop use `v2:` proof headers and the domains
`open-cowork/runner-kubernetes-desktop-{create,probe,stop}/v2\0`. The original
JobBinding field order above is preserved with a final `kubernetes` identity;
engine/container IDs are empty. This identity carries original cluster,
namespace, instance, organisation, generation, Node/Pod/PVC UIDs, original
creation resourceVersion, actual CRI identity and measured kubelet PID limit.
Fresh probes return observed_resource_version separately. Replacement, restart,
changed generation or missing original creation acknowledgement blocks reuse.

All native Pods retain the physical-stop finalizer until actual kubelet terminal
container evidence is captured and UID/resourceVersion CAS removes the finalizer.
Force-deleted API objects cannot mint stopped receipts. Unknown launches retain
their original reservation. Deployment instructions and operator resource
enrollment are in [the enterprise README](../../deploy/enterprise/README.md#kubernetes-manifests).
Source compilation and manifest checks do not establish live CNI/CSI, GUI or
production qualification.
