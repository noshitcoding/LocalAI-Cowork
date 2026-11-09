# Agent Sandbox operator workflow

These commands operate the new V2 topology. Existing Compose/Docker deployments
and their backup/rollback workflows remain independent. No cluster, image,
Kata runtime, certificate or Secret is installed by editing these files.
The new environment/process/lifecycle path serves OwnerRun execution. Existing
organisation TeamService/LiteLlmExecutor/TeamToolbox flows are not automatically
migrated or sandbox-qualified. Treat the tenant namespace template as an
operator deployment boundary, not as evidence of complete team integration,
multi-user isolation or cross-node application HA.

## Build and pin

Agent Sandbox is pinned to v1.0.7, source commit
`807cc37107bd7c012115afac9fa9c3b77b8365c2`. The upstream controller multiarch
manifest digest was resolved from its release tag on 17 October 2026:
`sha256:f254384c24605436ce35d7077427559e24bda8f5691cff5a54a4c2e030d81083`.
The renderer requires that exact reference. Version upgrades require a reviewed
source/config change and fresh compatibility checks.

Build the actual workload and trusted runner with
`deploy/v2/build_agent_sandbox_images.py`. Supply digest-pinned Go, Rust, Node
and Debian base images to `--go-image`, `--rust-image`, `--node-image` and
`--runner-base-image`; `--target workload|runner|preview|local-app` selects the output. The script
passes a Git BuildKit context with the resolved v1.0.7 commit and its exact
checksum. The annotated tag object is `b3832ed88cb8acc57cc5e81072f551d52f14b32f`;
it resolves to the commit above. Selecting the commit avoids treating the tag
object ID as a source-commit checksum.
The workload contains the real `sandboxd`, tools and immutable
`/usr/local/bin/cowork-runner-probe` from this source. The trusted runner contains
`cowork-process-runner` and the same probe. Build and publish through the
operator's normal registry workflow, record the resulting digests and SBOMs,
and use those digests in deployment/environment enrollment. No image is pushed
by the helper. A failed build is not an installed environment.
Cargo registry data is shared, while each target cache serializes its writers
and refreshes source/include mtimes before compiling a copied source context.
Use `--target-cache-id NAME` to build an independent image group concurrently
with a long daemon build; record that cache ID with the source manifest.
The corresponding Docker build argument is `CARGO_TARGET_CACHE`. This changes
build scheduling, not the runtime or admitted environment.

## Prerequisites and bootstrap

Use Kubernetes with v1 admission policies enabled, an enforcing CNI, a qualified
Kata RuntimeClass on hardware-virtualization-capable execution nodes, CSI
workspace storage and qualified shared RWX custody storage. Measure actual PID,
CPU, memory, disk and network limits using the runner probe. Default hostPath
storage in a development cluster does not supply a bounded filesystem.
Required broker Pod affinity keeps API/worker/runner consumers on one node;
this is necessary but does not prove that a CSI driver exposes the same kernel
superblock. Before enabling admission, verify a fresh harmless broker directory
has identical actual device/inode IDs in every consumer and prove that a held
exclusive lock in one Pod blocks acquisition in the others. Include restart
and remount behavior. An identity/lock mismatch refuses qualification; do not
substitute path strings or weaken the resource ID check. API2/worker2 replicas
do not establish cross-node application HA. Kata guests scale separately.

Start with a private copy of `deploy/v2/agent-sandbox.example.json`. Set a fresh
installation UUID and tenant UUID; exact namespaces, explicit kubectl context,
real image digests, storage/runtime classes, API-server and dependency CIDRs,
database hostname, HTTPS origin, ingress identity and existing Secret names.
Set `egress_probe_address` to an actual reachable control-plane IPv4 TCP canary
permitted from the trusted runner. A failed sandbox connection to a nonexistent
endpoint does not prove network isolation.
The runner label is the **first 32 hex characters** of
`canonical_hash(binding)`, exactly matching the runner configuration. The
example intentionally contains invalid placeholders until these are supplied.

```sh
python deploy/v2/agent_sandbox.py bootstrap-info
python deploy/v2/agent_sandbox.py render --config /private/settings.json --output /private/rendered-initial
```

A cluster administrator installs the pinned **core CRD only** from the URL
reported by `bootstrap-info`, after reviewing its original bytes and checksum.
Do not apply the full upstream install manifest to an existing installation:
this topology supplies its own scoped controller RBAC. Extensions, warm pools,
routers and CRD removal are outside this bootstrap. Deleting the CRD would
delete custom resources across the cluster and is not an application teardown.

Create the new namespaces explicitly using `kubectl --context YOUR_CONTEXT
create -f /private/rendered-initial/namespaces.json`. A create conflict is not
permission to adopt an existing namespace. Then capture their actual UIDs:

```sh
python deploy/v2/agent_sandbox.py enroll-namespaces --config /private/settings.json --output /private/enrolled.json
python deploy/v2/agent_sandbox.py render --config /private/enrolled.json --output /private/rendered-enrolled
```

The cluster administrator reviews and applies `cluster-policy.json` explicitly.
That artifact contains admission enforcement and the controller's read-only
cluster cache/runner observer permissions. Normal lifecycle `apply` deliberately
cannot install it. The controller has no cluster-wide mutation grant.

## Private configuration and startup

Use `cowork-process-runner init` in a private operator staging directory to
generate its actual signing binding, transport credential and initial config.
Use its real `--help`/usage and the operator's certificate pair. The server
certificate must cover `cowork-process-runner.APP_NAMESPACE.svc.cluster.local`
(used by the generated preview gateway), and every separately selected client
origin. Transfer the generated keys using the existing private
Secret workflow. Do not commit these files or paste their contents into logs.

The runner Secret contains `config.json`, its PKCS8 signing key, transport token,
TLS certificate and key. Its final config uses `listen: 0.0.0.0:8443`,
`data_directory: /var/lib/cowork-runner/runner`,
`workspace_state_directory: /var/lib/cowork/file-broker`,
`probe_executable: /usr/local/bin/cowork-runner-probe`, and all private file
paths under `/run/private/keys`. Set `docker_executable` to null. Merge the
rendered `adapter-settings.json` nested `agent_sandbox` object, whose namespace
UID and worker binding must agree with enrollment. The projected rotating
ServiceAccount token/CA use `/var/run/cowork-kubernetes/token` and `ca.crt`.

The app Secret contains `postgres_password`, `postgres_ca.pem`,
`runner-client.json` and its exact transport/control-key/CA files.
`runner-client.json` is the existing `COWORK_PROCESS_RUNNER_CONFIG` format with
HTTPS runner origin, binding, control key and transport token paths. API and
worker share the same actual binding and credential revision. Secrets are
copied into private regular files in memory; no secret projection symlink is
passed as native key custody. Configure external HA PostgreSQL/S3 and model
credentials through their existing deployment workflow; no database credentials
are embedded in rendered manifests.

The runner and app additionally share the actual FileBroker subdirectory from
`cowork-data` at `/var/lib/cowork/file-broker`, including coherent original file
identities and locks. `runner-paths.json` contains the exact operator path
settings to merge alongside `adapter-settings.json`. Do not point the runner
at an unrelated empty workspace registry. A bounded root init owns only these
PVC root directories as UID1000/mode0700, then the normal services run as
UID1000 without capabilities. Its app/control namespace uses baseline Pod
Security for that init; controller and untrusted workload namespaces remain
restricted. CSI root-squash or incoherent locks must fail qualification rather
than weaken native custody checks.
Control Pods deliberately omit `fsGroup`: its recursive volume permission
changes could widen retained mode0600 credentials on another replica's mount.

```sh
python deploy/v2/agent_sandbox.py apply --config /private/enrolled.json --target tenant
python deploy/v2/agent_sandbox.py apply --config /private/enrolled.json --target controller
python deploy/v2/agent_sandbox.py apply --config /private/enrolled.json --target application
python deploy/v2/agent_sandbox.py inspect --config /private/enrolled.json
```

Each mutation rechecks the original namespace UID and installation label and
refuses a foreign existing resource. There is no implicit context switch,
force-conflicts, name adoption, prune or PVC deletion. Enroll a concrete
environment and require its real qualification before enabling product runs.
Do not treat `profiles.json` resource candidates as observed hard limits.

## Optional web proxy and Windows

For web-enabled profiles add an explicit `web_proxy` setting:

```json
{"namespace":"cowork-egress","image":"YOUR_REGISTRY/proxy@sha256:REAL_DIGEST","policy_id":"public-web","allowed_domains":["example.com",".github.com"],"probe_url":"http://example.com/","direct_probe_address":"93.184.215.14:80"}
```

Build `Dockerfile.egress-proxy` from an explicitly pinned Debian base, create and
enroll the separate namespace, rerender, then apply `--target egress-proxy`.
The Squid config denies private/loopback/link-local destinations and all domains
outside the allowlist. Only public IPv4 TCP 80/443 and cluster DNS leave that
namespace. IPv6 is denied by this profile. Replace the explicit probe endpoint
when necessary; qualification must show proxy success and direct-IP denial.
Merge the generated `web_proxy_policies` binding into the runner Secret before
selecting the format-2 network policy. Ordinary profiles stay network denied.

For web previews add a `preview` setting with a separate namespace, the built
gateway image digest, `domain` (for example `previews.example.net` while Cowork
uses `cowork.example.com`), wildcard-TLS `tls_secret`, private `secret`, original
`runner_id`, `runner_generation`, `runner_public_key` and `allowed_ports` such
as `[8081]`. Create/enroll that namespace and rerender. The gateway Secret has
only `preview-validator-bearer` and `runner-ca.pem`. The runner Secret separately
contains the same validator-only credential; its generated
`preview_validator_token_file` points at that private file. Do not give the
gateway the runner transport token or any signing key. The public config is
copied to an owner-only regular file before startup, matching native custody
validation. Apply `--target preview` after application/runner configuration.
Configure DNS and a valid wildcard certificate for `*.previews.example.net`.

`/_cowork/bootstrap` accepts the signed preview grant through an explicit POST;
the backend validator route is `/v2/sandbox-previews/validate`. Every proxy
request checks the pinned original binding and short expiry. The main app
origin never hosts arbitrary sandbox HTML. Sandboxd and runner-control ports
cannot be selected as application previews. The adapter must be restarted with
the generated `preview_gateway` selector and allowed ports so its exact
per-attempt ingress policy admits the gateway. A static overlapping tenant
allow policy would be rejected by the adapter's isolation check.
Idle previews use the approved static workspace server on 8081. Custom
development servers require an active admitted attempt/port, and normal
completion terminates its Pod. PTY wire support is not a newly exposed
persistent terminal or full interactive IDE interface.

For Windows add `windows` with a separate `namespace`, `vm_name`, existing
`boot_pvc`, CPU/memory ceilings and `agent_config_secret`. Install and qualify
KubeVirt separately, prepare the licensed Windows boot disk, and place the
existing V2 device-agent package inside it. The rendered VM is halted and has
no owned data-volume template, preserving boot-disk retention. Pair the guest's
actual device identity and current remote-control policy through the existing
device protocol; native GUI/Office requires its supported interactive session.
Set Windows namespace network policy, TLS route, guest firewall, encrypted
configuration custody and power/backup authority before starting that VM.
Kata workload admission rules deliberately do not apply to KubeVirt launchers.

## Local Docker Desktop workspace

`compose.local-runner.yml` is a separate single-user execution location for
Docker Desktop's Linux Engine or an explicitly chosen local Linux VM Engine.
The same V2 daemon and trusted runner share one owned named volume at the same
Linux path. This preserves the FileBroker's original dev/inode identities and
OS locks. A Windows sidecar keeps its native user-rights/folder execution path;
it does not hand Windows file identities to this Linux runner.

Build the trusted `runner` and workload images above and the actual V2 app image
from `deploy/v2/Dockerfile`. The additional `local-app` build target provides the
current API-only daemon for a desktop connected to this localhost execution
location; it does not contain the web frontend or native Codex/keyring payloads.
Select real immutable local image IDs (`sha256:` plus 64 hex characters) or
registry digest references. Tags alone are refused. Local bare IDs are scoped to
the pinned Engine; server Kata images still require registry manifest digests.
The runner image includes Docker's CLI and OpenSSL; Kubernetes Pods still mount
no Docker socket. Initialize a new private directory:

```sh
python deploy/v2/local_runner.py init --root /absolute/private/local-container --docker-context desktop-linux --app-image REGISTRY/app@sha256:REAL_DIGEST --runner-image REGISTRY/runner@sha256:REAL_DIGEST --guest-image REGISTRY/workload@sha256:REAL_DIGEST
python deploy/v2/local_runner.py inspect --root /absolute/private/local-container
python deploy/v2/local_runner.py start --root /absolute/private/local-container
```

On Windows pass a new absolute Windows path under the user's private application
data directory. The initializer creates one UUID-owned Docker volume, records
its original creation/mount/label receipt and Engine ID before further IO,
generates private local TLS, and calls the existing `cowork-process-runner init`
with `--local-container-profile true`. It supplies a private HTTPS control origin
and observed Docker socket group. No keys are written into the repository or
printed. Incomplete initialization is retained for review rather than retried
against existing resources. Only local Unix-socket/named-pipe Docker contexts
are accepted; remote TCP/SSH contexts are refused.

The API is published only at `http://127.0.0.1:43822` by default; runner TLS only
at `https://127.0.0.1:9443`. Both ports are configurable before initialization.
Choose that localhost server through the existing connection setting and use
its own private daemon token. The token remains in
`/var/lib/cowork/data/daemon.token` inside the owned volume; retrieve it through
the user's own local terminal/credential setup without sharing it in logs.
Enroll the selected guest image as `local_linux_image`, require the actual
probe and select that environment for the run. There is one existing V2 queue
and API in this execution location, without a separate ad hoc task broker.

Import selected files into this working copy explicitly. Download/Save As is the
explicit path back to an authorized native folder. No guest mounts the Docker
socket, native home, selected original folders or unselected host paths. The
trusted runner has Docker administrative authority; this profile is for one
trusted local user, not hostile server tenants. Its disk budget is monitored
and may be exceeded between samples; it is not a hard filesystem quota.

`local_runner.py stop` stops only its recorded Compose project and retains the
volume and all keys/workspaces. Engine/volume identity or Compose-source changes
refuse startup until reviewed. Backup uses a confirmed stopped app/runner and
an encrypted archive of the complete owned volume with its original receipts;
restore targets a fresh volume and explicitly reenrolls the new runner identity.
There is no automatic `down --volumes`, prune or native folder synchronization.

## Stop, backup and restore

`agent_sandbox.py stop` scales app writers to zero and requests suspension of
the exact runner-labeled Sandboxes using UID-tested patches. The runner remains
available to reconcile physical stop. Its response does not claim physical
termination, release quota or remove any workspace PVC.

For a consistent backup:

1. Close new run admission and record the app/worker revisions, image digests,
   namespace UIDs, runner binding/generation and database migration version.
2. Cancel/stop active work through the existing run/runner API. Wait for original
   Pod/container terminal receipts and application checkpoints; unknown work
   stays held. A missing node/Pod is insufficient. Preserve the runner journal.
3. With all writers confirmed stopped, take the consistent PostgreSQL backup,
   private custody-volume backup, runner-journal backup, object-storage version
   manifest and every retained workspace PVC snapshot/archive. Use the CSI
   provider's quiesced procedure and record claim/PV/VolumeSnapshot UIDs and
   source hashes. KubeVirt disks use their separate stopped-VM procedure.
4. Encrypt the bundle, retain recovery keys separately, and record all original
   source identities, image digests, file hashes and actual completion receipts.
   Verify an independent restore before calling the backup accepted.

Restore into fresh namespaces and fresh PVCs with new actual UIDs. Keep admission
closed. Restore the database, custody files, object versions and workspaces as a
single selected consistent recovery point. Preserve original historical receipts;
enroll the new storage/namespace/runner generation explicitly instead of editing
old receipts to match new IDs. Start only the control path, validate database and
artifact hashes, reconcile old held work and perform a fresh harmless probe.
Reopen admission only after that new generation is qualified. Never replay an
uncertain old launch to test restore. Existing Docker backups do not automatically
restore into this different storage topology.

Retention is an app-authorized operation after confirmed physical stop and its
retention deadline. Workspace removal, snapshot retention, object deletion and
cryptographic-key destruction are separate decisions. This helper deliberately
offers no recursive namespace teardown or automatic PVC/CRD deletion command.

## Source and live checks

```sh
python -m unittest discover -s deploy/v2 -p test_agent_sandbox.py -v
python -m unittest discover -s deploy/v2 -p test_local_runner.py -v
```

The focused suite checks namespace/foreign-resource rejection before mutation,
UID-tested stop, retained claims, no Docker socket/hostPath, tokenless APIs and
workers, scoped RBAC, quotas/default deny, proxy allowlists and the halted
separate Windows VM.
The current renderer suite has 14 passing regressions and the localhost
bootstrap/Compose suite has five. The final process-runner source also passed
the Linux control-plane tests (12), receiver regressions (six, including exact
retention units, overflow and actual diagnostic PID retention), real local
probe HTTP test (one), and a Windows process-runner compiler check. These are
source and development checks; production physical acceptance remains separate.

The final shared-code checks also passed: domain environment regressions (five),
signed protocol regressions (two), runtime process tests (17), SQLite lifecycle
ownership (one), storage environment/migration/process integration groups
(8/2/8, with five separately configured fixture cases skipped), and the real
PostgreSQL ownership/environment cases (one each). That PostgreSQL fixture
records its confirmed shutdown in
`.local/sandbox-postgres-9e9c7f5e60cb49019d3d0a48c490f222/result.json`.
Frontend tests passed 360 cases in 68 files; contract generation/checks,
TypeScript checking and the web build passed. Three focused browser proxy
regressions also passed, covering uppercase-variable precedence, Chromium CLI
configuration and refusal to retry directly after proxy failure.
These results do not qualify the
separate organisation executor or replace actual physical boundary probes.
The original shared-code commands, results and source hashes are recorded in
`.local/agent-sandbox-root-validation-20261017/result.json`.

`qualify_agent_sandbox.py` is a separate disposable **core lifecycle/storage**
harness. It requires a newly created context named
`cowork-sandbox-validation-*`, an explicit private kubeconfig and the pinned
controller. It creates one UUID-owned namespace and app-owned PVC, writes a
marker, suspends/resumes a Sandbox, checks a new Pod sees the same bytes,
rejects a wrong-UID patch and checks Sandbox deletion preserves the PVC. Cleanup
checks the namespace's original UID before deleting its disposable test data.
It never targets an installed app namespace. Its plain-runtime result cannot
qualify Kata, enforcing CNI, hard disk quotas, proxy, Windows, security attacks
or HA. The complete workload/tool integration needs the actual built images and
enrolled production-like runtime.

### Executed development evidence, 17 October 2026

An isolated profile `cowork-sandbox-validation-646b` was created with its own
kubeconfig, two CPUs, 3 GiB and no host data mounts. Kubernetes 1.34.0 ran the
real pinned v1.0.7 controller. The core harness passed in 90.95 seconds:
marker bytes survived suspend/resume onto a new Pod UID, an incorrect Sandbox
UID mutation was rejected, deleting the Sandbox preserved the original
app-owned PVC, and its disposable test namespace was cleaned up.

All six rendered artifacts (cluster policy, controller, tenant, application,
preview and egress proxy) passed a real Kubernetes server-side dry run, including
Service and EndpointSlice observation privileges. The subsequently added
same-node broker affinity and explicit preview-issuer egress are covered by
focused source regressions; the saved
schema report retains its original renderer hash and does not cover that later
scheduling addition.
A separately installed rendered admission policy accepted the valid dry-run
Pod and rejected an ambient ServiceAccount token, a different image and a
different ServiceAccount; CEL type checking reported no warnings. This schema
test used an explicitly named plain-runc RuntimeClass and did not execute a
Kata workload. The cluster's development bridge CNI was not qualified for
NetworkPolicy enforcement. No hard filesystem ceiling, proxy, preview gateway,
Windows or application failover was established by these checks.

Original lifecycle/admission evidence is retained in
`.local/agent-sandbox-validation-20261017-646b/deployment-validation-result.json`;
the later schema run pins its own artifact and renderer hashes in
`schema-reviewed-result.json` in the same directory. The disposable profile was
deleted after validation; the original preexisting minikube profile was not
modified. Reports remain available. These records do not retroactively qualify
an image or establish enforcement on another cluster.

`smoke_agent_sandbox_image.py` additionally exercises the actual built sandboxd
health endpoint, authenticated file PUT/GET and one gRPC Start stream through
Init, stdout and successful Exit. It requires `grpcio`, an explicit local Docker
context and an immutable image ID/reference. It creates a labeled disposable
volume and nonroot read-only container, publishes both listeners on loopback,
then verifies ownership before removing only its own fixture. This smoke also
uses plain runc and establishes wire compatibility rather than Kata isolation.
The complete image checks record their own immutable image IDs and source
manifests under `.local/agent-sandbox-image-build-20261017-f4af/`.

The actual localhost Compose bootstrap also passed with the built API-only
daemon and trusted runner: health 200, authenticated project listing and
creation, wrong-bearer rejection (401), and the paginated environment listing.
The two containers observed the same workspace device/inode and a real
exclusive OS lock excluded the second holder. Only the runner had the Docker
socket, both published ports bound to `127.0.0.1`, and ordinary stop retained
the original volume. Its new synthetic project, containers, networks and volume
were removed after receipt checks; no provider or user data was used.
`bootstrap-smoke-final-result.json` records the exact final image IDs and scope. This
bootstrap/API check does not establish a successful signed command or qualify
Kata. The real Compose parser regression also protects the single quoted tmpfs
mount specification used by both services.

The built preview gateway passed nonroot/read-only startup, real health 204 and
unauthenticated proxy rejection (401), using only disposable synthetic private
configuration. Its final-image evidence is `preview-image-smoke-result-final.json`. This did not
exercise a signed preview or a live validator. The separately built proxy
passed nonroot/read-only startup and rejected an unallowlisted public host
(403); `proxy-smoke-result.json` records that application-layer check. Cluster
egress enforcement and a full signed preview remain unqualified.

The final immutable local runner/workload images also passed the real signed
execution fixture: measured environment qualification matched its signed report
hash, one Python command completed with its genuine supervised diagnostic PID
and exit code zero, and its files were promoted. Replaying the request returned
the same receipt and left the counter at one; no guest remained during the
additional two-second observation, and completed resources were released.
Only this final successful fixture establishes the signed local execution
result. Earlier failed candidate reports remain separate. This verifies the
explicit Docker Desktop local profile, including its soft disk budget; it
does not qualify server Kata, CNI policy or a hard filesystem quota.

The final workload image also ran the shipped Playwright adapter and real
Chromium against a synthetic HTTP fixture through an explicit uppercase proxy.
JavaScript changed the DOM marker, and the proxy recorded the expected absolute
target URL. The fixture denied unrelated browser requests and used an internal
network, UID 1000, read-only root, dropped capabilities, bounded resources and
tmpfs workspace/profile paths, with no host mounts or published ports.
`browser-image-smoke-result-final.json` records the exact adapter hash, versions
and confirmed cleanup. This is browser/proxy wire evidence, not Kubernetes CNI
enforcement.

The stable local evidence index is
`.local/agent-sandbox-image-build-20261017-f4af/final-validation-result.json`.
It pins the common frozen source manifest, all four final image IDs and hashes
of the selected public reports. The signed fixture driver has its own recorded
hash because its test-only reporting improved after the image source snapshot.
The index also preserves the distinction between local development evidence,
the earlier plain-runtime Kubernetes lifecycle/schema check, and outstanding
production acceptance. Private keys, bearer tokens and user files are absent.
