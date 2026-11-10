# Enterprise management review environment

> This guide belongs to the independent Enterprise source line. Unless a command
> specifies a private operator directory, run it from the repository's `enterprise/`
> directory. See the [public build-context guide](../../../docs/PUBLIC-SELF-HOSTING.md).
> Existing standalone bundle/evidence references describe separate Enterprise
> work; this repository integration publishes none. Existing acceptance
> descriptions do not qualify V2 desktop release packages.

This is an implementation preview. Read the
[capability and acceptance status](../../docs/ENTERPRISE_IMPLEMENTATION.md)
before treating it as the agreed enterprise release.

## Standalone source and Docker build bundle

The current Enterprise delivery is a separate `open-cowork-enterprise-source.zip`.
It contains the public repository build contexts under `source/`, the actual
recorded standalone web build under `web-dist/`, source manifests, lockfile
dependency SBOM, public native/DDL/web evidence and `SHA256SUMS`. Public evidence
is explicitly derived from unchanged private originals: machine paths are
replaced with named roles, and each derivative records its original byte SHA256
and exact transformation scope. It is not represented as an original byte copy.
Private operator configuration, credentials, certificates, local caches and
desktop runtime bundles are excluded. The SBOM inventories the locked source
dependencies; it does not attest unbuilt OCI base images or a deployed runtime.

Verify the delivered archive SHA256 and the extracted `SHA256SUMS`, then read
`INSTALL.txt` and this README. From the extracted `source/` directory, build your
own server, runner, provisioner and web images using their included Dockerfiles.
Supply their actual immutable image digests to the gateway preparation and
original backend enrollment paths below. Supply your own DNS/TLS, private
database/S3 credentials, host profiles and capacity observations before serving.
The bundled web output can also be served with `deploy/web/nginx.conf`.

`scripts/build-enterprise-source-package.py` creates a new immutable bundle from
passing current native and fresh DDL receipts, runs the actual `build:web` command,
checks unchanged source inputs and validates each Docker COPY input. It never
starts Docker or publishes, installs or qualifies a server. Source compilation,
fresh single-user DDL and the web build remain separate from physical deployment
acceptance.

## Prepared production instance gateway

`docker-compose.production.yml` and `production-nginx.conf.template` provide the
actual HTTPS gateway for Compose instances on the reviewed routing network. Each
instance retains its own origin, `https://<32 lowercase UUID hex>.instances.example.com`,
and serves `/api/v1` there. The existing management launch response and
`loginFleet`/`/api/v1/auth/fleet` exchange use this exact origin; launch tickets still
expire after sixty seconds and remain bound to their original organisation,
instance, user, key generation and nonce. Every subsequent instance content
request retains the existing fresh central access checks. Management cookies and
instance cookies are separated by hostname. A shared management-origin
`/instances/{id}/api/v1` proxy is deliberately not enabled by this profile.

Copy `gateway.example.json` to a private absolute operator path and supply the
real management origin, instance domain, current server/web image digests,
existing external service/routing networks, reviewed policy, management secret
directory and the existing private provisioner configuration file. Its declared
server image and networks must match this selected release. Choose the explicit
`instance_passkeys_enabled` boolean; when enabled, every instance receives its
own exact hostname as WebAuthn RP, never the management or parent-domain RP.
Supply an existing PEM certificate chain with DNS SANs for
the management host and `*.instances.example.com`, and its matching private key.
The preparation tool requires Python 3 and the local `openssl` command. On Linux,
private key and management secret files must be operator-owned with no group or
other access. It checks matching key/certificate bytes, exact wildcard SAN,
hostname coverage, validity and at least twenty-four hours remaining. It copies
the TLS pair into a new private output directory; existing output is never
overwritten. The TLS key is mounted only by the web gateway.

```sh
python3 scripts/prepare-enterprise-gateway.py \
  --config /srv/cowork-operator/gateway.json \
  --output /srv/cowork-operator/gateway-release-1
docker compose --env-file /srv/cowork-operator/gateway-release-1/gateway.env \
  -f /srv/cowork-operator/gateway-release-1/docker-compose.yml config
```

Preparation writes the complete Compose configuration, paired launch environment,
rendered Nginx configuration, copied policy/TLS pair, a complete paired
`executor_configuration.json` and a
`PREPARATION-RECEIPT.json` with source/configuration hashes. It does not start or
modify any service, database, DNS entry, certificate account or installed fleet.
Keep this directory outside Git and back it up as private operator material.
After independent operator review, the same prepared Compose command with `up -d`
installs that selected gateway. The operator must first supply DNS for the exact
management host and wildcard instance domain pointing at this gateway; network
names must match the trusted provisioner's `services_network` and
`routing_network`. Only the API service's existing fixed
`cowork-<32hex>-api` routing alias is selected. No request can supply an upstream
URL, host, port or management/provisioner credential.

The public listener rejects unrecognised hosts and mismatching TLS SNI/Host,
redirects recognised HTTP origins to HTTPS, preserves SSE/WebSocket traffic and
overwrites proxy-origin headers. Instance origins deny management routes; the
management origin denies the private runner/provisioner listener. Browser-native
bearer sessions and the existing instance launch exchange retain their original
authority. The production domain variable is checked against the exact launch
template before management database startup.

Use the prepared `executor_configuration.json` as the provisioner's reviewed
operator configuration. Its typed `instance_gateway` profile is consumed by the
actual API Compose-manifest builder, so fresh provision, restore and update all
receive the exact instance `COWORK_PUBLIC_ORIGIN`, optional exact RP and explicit
own-origin/native Desktop CORS list. Guest endpoint adaptation retains these
fields. Starts and recovery reject a mismatching saved runtime manifest;
cleanup/stop remains available. The existing refresh cookie remains
`__Host-cowork_refresh` with `Path=/`, `Secure`, `HttpOnly`, `SameSite=Strict` and
no Domain attribute; the gateway does not rewrite its origin or cookie domain.

This is source/configuration delivery, not a claim of installed production
acceptance. Actual DNS/TLS trust, current image contents, authenticated launches,
revocation, SSE/WebSocket reconnects, upgrades and the physical routing network
still require deployment qualification. Hosted/remote executor transport needs
its separately qualified bridge; this profile does not infer a central Docker
alias for a remote engine. Certificate renewal remains an explicit operator
replacement followed by a newly prepared release and gateway recreation.

Central cross-instance execution quotas currently have a separately tested,
disabled [admission candidate](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md).
The local control plane now includes migrations 0044–0049. Initial instance
version selection is enabled in the local installation. Admission/enrollment
feature flags remain disabled; tenant images have not been upgraded or enrolled.
The candidate includes a budget/reservation UI and explicit whole-instance stop
verification for selected Compose server reservations. Device/VM holds are preserved.
Activation is persisted and rejects un-enrolled existing instances. Runtime proofs
and database dispatch barriers protect required instances from obsolete management
replicas. The operator must complete legacy enrollment and the remaining
backend qualifications before activation; existing sandbox limits do not imply that
organisation/user run budgets are already enforced throughout the fleet.
The admission candidate has a separate Docker Desktop acceptance launcher,
`python scripts/test-enterprise-execution-docker.py`. It creates private test
databases/storage and new instance fixtures; it does not upgrade the running local
stack. Its update fixture packages the same server binary under distinct immutable
digests, so this test does not establish cross-version migration compatibility.

## Docker Desktop test installation (Windows)

The updated source profile uses an independent provisioner network namespace and
a private TLS control gateway (`operator-control`, no published port). The start
script requires an operator image declaring this network contract, so `-SkipBuild`
cannot silently reuse an incompatible old image. The current local installation
now uses this profile after the verified schema-48 control-plane cutover.

The installed control images are pinned in the private
`.internal/enterprise-local/control-plane-images.json` Compose override. The start
script accepts only image entries for management, management web, the private
gateway and provisioner; it preserves existing database URL secrets. Building
development tags does not replace those selected control images. Use
`Start-EnterpriseLocal.ps1 -SkipBuild` to restart the installed version. A direct
Compose `up` must include the image override with a second `-f` argument; the base
file alone is not the selected control release. Normal instance images remain
independently pinned in the operator state.

`Initialize-EnterpriseControlTls.ps1` creates the private local certificate/key in
the ACL-protected test directory and validates hostname, expiry and matching keys
on reuse. The provisioner receives only the trusted certificate; only the private
gateway mounts the key. Existing identities are never silently replaced. Local
certificates last 397 days. The supplied Linux operator lifecycle below now
renews the pinned private identity and recreates its gateway/provisioner consumers,
with original plans, CAS and actual serving-TLS readback. Generic provisioner deployments can use `management_ca_file` for
their own CA; HTTPS hostname and certificate verification remain mandatory.

## Operator audit export, independent witness and retention

Migration `0065_management_audit_lifecycle.sql` adds frozen organisation-scoped
exports and original retained-prefix checkpoints. Administrators/auditors can
prepare and download an authenticated export. Applying retention requires current
administrator authority, the original export hash, unchanged checkpoint CAS and
retention policy, and a valid independently signed witness receipt. The server
rechecks every exported original record under the ordinary audit writer lock,
appends an authenticated retention event, removes only the plan's expired prefix,
and retains its original last removed digest. Ordinary appends continue the same
chain. The audit reader and future exports validate that signed base. Export
bodies in management, including abandoned plans, are compacted after their retention
window on later successful completions; checkpoint/signature/final operation receipts
survive. An expired uncompleted plan requires a fresh export request.
The separate witness archive keeps the original export bytes under its own policy.

The actual external role is `enterprise-audit-lifecycle.py witness`: it archives
the original export, enforces chain continuity against previously witnessed
heads, signs its hash and checkpoint with its private RSA key, and returns the
original receipt on retry. Install it on a separate host/storage/administrator
trust domain. Management receives only the public key. A witness beside the
management database under the same administrator is a local archive, without an
independent protection claim. The first archive establishes the external baseline;
it does not attest unwitnessed historical deletion. Export remains available
without a witness, while destructive retention fails closed until one is set up.

On the independent witness host (adapt user, paths and the HTTPS DNS identity):

```sh
install -d -m 0700 /srv/cowork-audit-witness/archive
umask 077
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 \
  -out /srv/cowork-audit-witness/signing-key.pem
openssl pkey -in /srv/cowork-audit-witness/signing-key.pem -pubout \
  -out /srv/cowork-audit-witness/witness-public.pem
openssl rand -hex 32 > /srv/cowork-audit-witness/witness-token
```

Run the supplied `cowork-audit-witness.service` as the separate
`cowork-audit-witness` account, with that account owning the private directory,
key and token. Put `audit-witness-nginx.example.conf` behind the witness's actual
trusted HTTPS certificate; its API binds only to `127.0.0.1:18095`. Set the
management process's `COWORK_AUDIT_WITNESS_ID=external-operator-1` and
`COWORK_AUDIT_WITNESS_PUBLIC_KEY_FILE` to the independent public PEM key. For the
prepared production Compose profile, add `docker-compose.audit-witness.yml` as a
reviewed overlay and supply those two environment values. Mount the public key;
never mount the witness's signing key or archive into management.

On the operator host, use a private directory for each original operation and a
current administrator bearer token in a mode-0600 file. The token is not printed
or sent across redirects. For exports alone an auditor token is sufficient.

```sh
install -d -m 0700 /srv/cowork-operator/audit-operation-1
python3 scripts/enterprise-audit-lifecycle.py operator plan \
  --management https://management.example.com --organisation ORGANISATION_UUID \
  --witness https://audit-witness.example.com \
  --state /srv/cowork-operator/audit-operation-1 \
  --token-file /srv/cowork-operator/management-access-token
python3 scripts/enterprise-audit-lifecycle.py operator apply \
  --management https://management.example.com --organisation ORGANISATION_UUID \
  --witness https://audit-witness.example.com \
  --state /srv/cowork-operator/audit-operation-1 \
  --token-file /srv/cowork-operator/management-access-token \
  --witness-token-file /srv/cowork-operator/witness-access-token
```

Optional `--management-ca` and `--witness-ca` provide explicit trust bundles.
The first command creates an original request and verified export; the second
archives/anchors those exact bytes before completing the matching management
plan. A lost response uses the same request, witness receipt and final receipt.
The frozen plan bounds a batch to `--max-records` (default 1000; maximum 10000).
Run later batches with fresh operation directories until the expired prefix is
gone; keep successful receipts and independently archived originals. API routes
are `/management/v1/organisations/{org}/audit/checkpoint`, `/audit/exports`,
`/audit/exports/{export}` and `/audit/exports/{export}/complete` under that same
organisation prefix. The regular audit view continues to return 500 records.

## Automatic private Control/VM certificate lifecycle

`enterprise-certificate-lifecycle.py` delivers Linux `plan`, `apply`, `reconcile`,
`maintain` and `status` commands. Copy `certificate-control.example.json` or
`certificate-vm.example.json` into private operator configuration and adapt the
exact existing Compose project, files, secrets/state paths and service names.
All selected existing consumer images must already be digest-pinned. The
operator must be able to resolve/reach the exact control HTTPS probe hostname;
the example's private `operator-control:8443` must therefore be reachable on the
operator host through its own reviewed private DNS/listener setup. The lifecycle
does not invent a port publication or silently skip the serving-TLS check.

Planning copies original/candidate TLS bytes to a new private operation, binds
the original configuration, immutable image/container identities, secret mount
paths, revision and VM identity/daemon/inventory, and performs real OpenSSL
key-pair, hostname, validity and EKU checks. Control renewal issues a new private
397-day self-signed identity for the exact configured hostname. VM renewal
replaces the local CA and both 90-day server/client identities together; the CA
lasts 397 days. This supports the existing local operator PKI. Public ACME or an
external enterprise issuer needs its own issuance integration and is not claimed.

```sh
install -d -m 0700 /srv/cowork-operator/certificate-state
python3 scripts/enterprise-certificate-lifecycle.py plan \
  --config /srv/cowork-operator/certificate-control.json \
  --state /srv/cowork-operator/certificate-state --expected-revision 0 --force
python3 scripts/enterprise-certificate-lifecycle.py apply \
  --config /srv/cowork-operator/certificate-control.json \
  --state /srv/cowork-operator/certificate-state \
  --plan /srv/cowork-operator/certificate-state/ORIGINAL_OPERATION_UUID/ORIGINAL-PLAN.json \
  --plan-sha256 ORIGINAL_PLAN_SHA256
```

`apply` stops only the positively bound selected consumer containers, acquires
the same `operator.lock` used by `provisioner.py`, atomically installs the exact
candidate, and runs real `compose up --no-deps --force-recreate --pull never
--no-build` for those services. For a VM it first requires all existing guest
containers to be stopped through normal management maintenance, checks the
original daemon/trust domain and retained inventory over pinned-host-key SSH,
installs the exact guest TLS pair/CA, and actually restarts only that VM's
`docker.service`. It verifies the rotated mTLS daemon and identical guest
container identities, images and stopped states. It then recreates the selected
provisioner consumer so its bound client secrets reload. These are maintenance
rollouts, not uninterrupted renewal promises; a busy guest is refused and no VM
capacity, installed tenant image, database or foreign deployment is changed.

Final receipts are issued only after all candidate file hashes, recreated
consumer identities/images/health, and actual serving HTTPS certificate/readiness
or real guest mTLS/daemon/secret readback pass. The original plan and private key
backups remain in operator-owned, mode-0700 storage for operator recovery.
Commands have durable pending stages. A lost command response requires the same
original `reconcile` command; physical readback completes a previously successful
rollout without repeating a destructive unknown-outcome command. A verified
completed guest phase may need a second identical reconciliation to continue its
consumer rollout. An actually incomplete restart/recreate remains pending for
operator repair and is never converted into a success receipt.

For automatic operation install `cowork-certificate-lifecycle.service` and
`cowork-certificate-lifecycle.timer` on the selected Linux operator host, adapting
the unit's user and allowed private paths. Set mode-0600
`/etc/cowork/certificate-lifecycle.env` to
`COWORK_CERTIFICATE_CONFIG=/srv/cowork-operator/certificate-control.json` and
`COWORK_CERTIFICATE_STATE=/srv/cowork-operator/certificate-state`. The daily timer
runs `maintain`: it renews only when any required certificate is within the
configured window (14 days by default) and reconciles an original pending plan.
Install separate instantiated/copy units and state directories for separate
Control/VM identities. Operator monitoring must alert on unit failure; a VM that
still has active guest workloads waits for maintenance instead of interrupting
them. Retained private backups are not automatically deleted.

The delivered server source compiles natively and the six focused offline
OpenSSL/archive checks pass via `scripts/test-enterprise-operator-lifecycles.py`.
These checks exercise real certificate generation, EKU/hostname refusal, external
RSA signatures, original-receipt retry, witness fork/rollback refusal and safe
non-root secret read-mode preservation. They do
not execute an installed Compose, guest restart, public witness or production
rollout. Migration/API race acceptance, real witness deployment and actual
Control/VM maintenance qualification remain deployment gates.

## Original VM/Kubernetes backend activation

Migration0066 adds immutable backend plans, current backend profile revisions and
an original profile snapshot on each management operation. The dedicated
executor-host registration/credential API remains the trust boundary. Create a
new pending instance on its exact registered host through
`/organisations/{org}/executor-hosts/{host}/instances`; a VM instance uses
`backend=compose` and `minimum_isolation=virtual_machine`, while Kubernetes uses
`backend=kubernetes`. Neither backend silently uses the local daemon or an
unbound global provisioner token. These setup steps are for the operator's own
server and do not adopt or migrate this repository's protected pilot or current
Docker Desktop installation.

Copy `backend-vm.example.json` or `backend-kubernetes.example.json` into an
owner-only operator file and replace all sample IDs/endpoints with the original
manager, organisation, instance, operator state and physical host identities.
Public origins must be the exact origin derived from management's configured
instance URL template. Supply the actual TLS chain/key and trusted public CA,
the selected frontend release, and a reachable original API forward or namespace
service. Use a new operation directory and current administrator access token.

The operator producer's `prepare-routing` writes a complete exact-host Nginx API
proxy, static frontend route, streaming/WebSocket forwarding and a separate
read-only route marker containing the main configuration hash. The VM variant
also writes a real pinned-host-key SSH API-forward service. Install/reload those
reviewed assets on the operator's server, including
`backend-gateway-deny-default.conf` once at the shared Nginx HTTP level. Keep the
VM source VM/root, SSH key/known-host file, port and service selection original.
The original `state_directory` must be a new private directory below that VM's
existing `shared/` read-only guest mount (normally `shared/instances`). The
actual shared-state nonce remains checked before Compose mounts are used.
Paired product configuration rejects a fixture-only lifecycle profile; copy
only the reviewed transport/image/service configuration into a new operator
state, without adopting a protected qualification fixture or its receipts.
The API-forward port must match the guest-runtime-endpoints port; no QEMU pilot
port or existing VM metadata is silently altered. Record that service's actual
`systemctl show --property=MainPID --value` in the configured owner-only
`tunnel_pid_file`. For Kubernetes, the operator first creates the exact fresh
namespace with `cowork.instance-id`, `cowork.organisation-id` and
`cowork.executor-host-id` labels, installs its scoped native runner and supplies
the original namespace, cluster, TLS and node identities. The Kubernetes adapter
uses those original objects; it does not adopt an arbitrary existing namespace.

```sh
python3 scripts/enterprise-backend-profile.py prepare-routing \
  --config /srv/cowork-operator/backend-vm.json \
  --output /srv/cowork-operator/routes/instance-1
# Operator installs the original route and VM forward on their own server here.
install -d -m 0700 /srv/cowork-operator/backend-operation-1
python3 scripts/enterprise-backend-profile.py plan \
  --config /srv/cowork-operator/backend-vm.json \
  --state /srv/cowork-operator/backend-operation-1 \
  --expected-instance-revision ORIGINAL_INSTANCE_REVISION \
  --expected-profile-revision 0 --management https://management.example.com \
  --management-token-file /srv/cowork-operator/management-access-token
python3 scripts/enterprise-backend-profile.py activate \
  --config /srv/cowork-operator/backend-vm.json \
  --state /srv/cowork-operator/backend-operation-1 \
  --control https://operator-control.example.internal \
  --control-ca /srv/cowork-operator/control-ca.pem \
  --operator-token-file /srv/cowork-operator/scoped-host-operator-token
python3 scripts/enterprise-backend-profile.py pair-provisioner \
  --config /srv/cowork-operator/backend-vm.json \
  --state /srv/cowork-operator/backend-operation-1 \
  --provisioner-config /srv/cowork-operator/original-executor-configuration.json \
  --output /srv/cowork-operator/paired-executor-configuration.json
```

`plan` binds the original actor/request, current instance/profile revisions,
assignment, specification hash, a ten-minute challenge and exact origin. It
returns the original plan on retry; `activate` persists the original observation
before sending it and returns the matching original activation receipt on a lost
response. `pair-provisioner` creates a new complete operator configuration with
that receipt, public specification and a source-pinned physical probe command.
It preserves the original image, endpoint, credential and secret configuration,
and rejects a mismatching host or runtime launch origin. `inspect` is the
read-only producer used before actual lifecycle work; it accepts the original
`--challenge` and `--specification-sha256`, and `--runtime-ready` additionally
requires the actual public HTTPS API readiness endpoint.

VM inspection measures the actual owned QEMU process and QMP KVM/running state,
the guest hardware UUID over pinned-host-key SSH, the retained capacity ledger
and exact inventory, real daemon mTLS/ID/trust domain, the original live SSH
forward process/port, and the actual HTTPS origin/configuration marker. It
therefore requires a real running, registered operator-owned VM. The supplied
VM controller currently uses its bounded four-CPU/4096-MiB guest profile; select
service/job reserves that actually fit it or supply separately budgeted hardware
and a reviewed profile implementation. An isolation string alone never opens
the VM lifecycle gates. The original container adapter runs inside that exact
VM, retaining its actual shell/GUI/runner behavior; successful activation opens
provision/start/stop/resume, backup/restore, update/rollback, explicit enrollment
and verified instance deletion through the existing receipt-driven handlers.
The parent VM itself keeps its retained allocation and is not silently retired
when an instance is deleted.

Run the VM operator as the owning non-root Linux host account so QMP, `/proc`,
the canonical capacity pool and original SSH/daemon identities can be measured.
`cowork-bound-provisioner.service` is supplied for this mode. Install the actual
Docker CLI, Python with PyYAML, PostgreSQL client, age and MinIO client required by the
existing lifecycle, plus kubectl/Helm for Kubernetes. Set mode-0600
`/etc/cowork/bound-provisioner.env` to
`COWORK_BOUND_OPERATOR_CONFIG=/srv/cowork-operator/paired-executor-configuration.json`
and adapt the unit's user and allowed private paths. The VM physical profile is
not enabled by running a root Docker provisioner with an inaccessible host `/proc`
or QMP socket. Each paired operator state remains single-host/single-instance.

Management checks current administrator and host credential authority at
activation, requires the original instance/host assignment and CAS, freezes the
active profile on each lifecycle operation, rejects obsolete operator profile
contracts, and verifies profile-revision/hash bindings in final receipts.
Initial release selection uses the actual scoped host catalog. VM update canaries
retain their own `vm_compose`/`virtual_machine` evidence and exact original
profile, revision, images, backup and rollback; Kubernetes has its own native
canary adapter. A release canary from an older profile cannot qualify the current
profile. Batch rollouts freeze that current profile on every actual operation;
reserved or busy instances cannot acquire a replacement backend plan.
Native host capacity admission and current organisation MFA/admission fences remain
mandatory. Disabled or changed profiles/hosts prevent new work. Historical final
receipts retain their original operation; successful retries do not invent a new
generation or repeat an unknown command outcome. Actual VM/Kubernetes installation,
network/TLS trust, external services, replica races and physical lifecycle
qualification remain operator deployment gates; no such action is executed by
source delivery.

From the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/Start-EnterpriseLocal.ps1
powershell -ExecutionPolicy Bypass -File scripts/Start-EnterpriseDesktop.ps1
```

The first command builds and starts the separate `cowork-enterprise-local` Compose
project and opens management at
`http://localhost:18082/management`. The workspace login is at
`http://127.0.0.1:18083/server`. Distinct loopback hostnames avoid sharing browser
refresh cookies (cookies are not isolated by port). Both listeners are local only.
Databases use separate volumes, credentials and Docker networks.

Databases and object storage restart with Docker unless explicitly stopped.
Management/workspace gateways re-resolve their Docker service names after an
address change. `node app/scripts/test-enterprise-gateway-reconnect.mjs` verifies
this with disposable backends and deliberately different IPs, without restarting
the gateways. Its default test subnet is `10.254.202.0/28`; override it with
`COWORK_GATEWAY_TEST_SUBNET` if that conflicts with your Docker/LAN/VPN network.

Private generated sign-in details are in `.internal/enterprise-local/TESTZUGANG.md`.
They are excluded from Git and Docker build contexts. Subsequent starts support
`-SkipBuild`; they reuse existing data and credentials. Stop with
`docker compose -f deploy/enterprise/docker-compose.local.yml stop`.

### Administrator verification and upgrades

Platform administrators must verify their management session with an authenticator
or a passkey with user verification. On first sign-in choose **Set up authenticator**,
add the displayed secret to your authenticator, enter its six-digit code and save
the one-use recovery codes. Enrollment must begin within ten minutes of sign-in.
After upgrading, old sessions and native authorization codes have no verification
evidence: sign out and sign in again for initial enrollment, or verify your existing
factor. A refresh does not extend the twelve-hour verification period. An enrolled
factor alone never verifies other sessions. The last factor cannot be removed.
Five invalid second-factor attempts within five minutes block verification across
that account's sessions for five minutes. Organisation members are not subjected
to this platform-administrator requirement. Organisation-scoped SSO is documented
in [ENTERPRISE_OIDC.md](../../docs/ENTERPRISE_OIDC.md). Explicit operator-owned
provider assurance mapping is optional and has its own current-policy/expiry
gates and local CAS administration. Ordinary OIDC grants no automatic second
factor. Broad organisation MFA policy and external provider acceptance remain open.

The management service now requires `COWORK_AUTH_MASTER_KEY` or its `_FILE` variant:
32 random bytes encoded as base64 or 64 hexadecimal characters. Compose and Helm
mount the `auth_master_key` secret. The local launcher generates it once and keeps
it in the private secrets directory; it does not enroll a factor for you. Back up
this key together with the management database and retain it on restore. Existing
installations with encrypted TOTP records must use their previous storage master
key as the initial authentication key; changing the key without re-encryption
would make those records unreadable. API/worker deployments without the new setting
retain their previous object-store-key fallback. Authentication no longer needs S3.

The local launcher creates Alpha/Beta demonstration organisations only when it has
a verified administrator session. After enrollment it can use an operator-supplied
`COWORK_TEST_ADMIN_SECOND_FACTOR` for that invocation. Never persist factors or
recovery codes in tracked files. Existing live acceptance scripts that log into
management likewise need a fresh, operator-supplied code for each sign-in. Use
`COWORK_TEST_ADMIN_SECOND_FACTORS` as a JSON array for tests with multiple sign-ins
(API, browser and optional desktop); the helper consumes each value once and
never reads the authenticator seed. The
disposable `scripts/test-enterprise-mfa.py` harness instead creates and removes its
own isolated test identity. Historical browser acceptance does not qualify a real
hardware passkey or external identity provider for this new assurance requirement.

For SSO account linking and migration 0041, read
[OIDC identity binding](../../docs/ENTERPRISE_OIDC.md). Existing accounts must link
SSO from an authenticated **Security** session; matching email addresses no longer
merge accounts. Unproven legacy links require explicit verification after upgrade.

To invite team members, select an organisation, open **Members**, and create an
invitation for an email address and role. Copy the private link when it is shown
and deliver it yourself; no email is sent by the server. The recipient creates
an account or signs into the existing one at `/join`. The link expires and can be
revoked before acceptance. Existing organisation roles are retained. Afterwards,
assign the person to an instance through **Instance members** and share the needed
projects there. Joining an organisation alone grants neither instance content
nor platform administration. The desktop login also links to the invitation flow.

The desktop launcher uses the existing development shell and current frontend;
it does not replace an installed release. Build the shell with `cd app; npm run
tauri dev` first if it is missing. Open **Server** (Ctrl+8), enter a server URL and
sign in. Addresses are saved automatically; switching servers requires signing
in again. Management and workspace servers open their corresponding views.
Connection alone grants no laptop permissions. Credentials stay in the native
vault; saved profiles contain only names, addresses and email addresses.

The local profile provisions real Compose instances. Creation queues provisioning;
existing pending records have a **Provision** action. Status refreshes automatically.
Once ready, **Open instance** signs in through a single-use, 60-second ticket,
in either the browser or the development desktop app. No second password is needed.
Each instance has separate API/worker/runner services, database credentials,
storage encryption keys and an S3 bucket. Instance browser origins use
`http://<instance UUID without hyphens>.localhost:18084`; host-only refresh cookies
are isolated between instances. The gateway listener binds only to loopback.
Return to management to sign in again after ending an instance session.

The trusted operator container owns the Docker socket, dedicated PostgreSQL/S3
administration credentials and a private named state volume. Application and
sandbox containers never receive these privileges. The explicitly selected
`docker_desktop_seccomp` profile requires Docker Desktop with seccomp; the default
Linux profile still requires AppArmor. Requested VM isolation is never downgraded.
Source-pinned official MinIO/mc builds replace unavailable upstream registry images.
See [MinIO's source distribution](https://github.com/minio/minio).
New instance networks use the operator's dedicated private IPv4 pool, locally
`10.240.0.0/16` (override `COWORK_FLEET_NETWORK_POOL` on the provisioner). Check for
LAN/VPN conflicts before choosing a different host's pool. Allocations exclude
existing Docker subnets and retained instance plans, and size the sandbox subnet
for its declared concurrency. Existing legacy networks are kept. This avoids
consuming Docker's large default subnets for every instance. Compose's private
CLI configuration lives in the provisioner's temporary filesystem.

Stop managed instances through management before stopping the management stack.
Instance Compose projects are separate from `cowork-enterprise-local` and retain
their data and runtime volumes across stop/start. Do not remove their volumes.

The separate workspace login service on port 18083 remains a connection-only
fixture, without execution or storage. Model credentials are not preconfigured.
Outbound access defaults to denied; operator and organisation policy must allow
the required model/network destinations. VM operations, Kubernetes HA and the
40-task acceptance suite remain outstanding.

After startup, run `cd app; node scripts/test-enterprise-local.mjs` for real browser
and HTTP checks. `--desktop` additionally uses an already-open development
WebView with its test-only CDP port 9223; the normal launcher does not enable CDP.
Reports and screenshots are stored in the private test directory.

`node scripts/test-enterprise-provisioning.mjs --lifecycle --desktop` additionally
tests actual instance provisioning, instance-bound/replay-resistant tickets,
central membership revocation, a real restricted shell job, encrypted file
upload/download, stop/start persistence and browser/native opening. This local
acceptance fixture uses `test c / instance 1`, `Testorganisation Alpha /
Docker-Testinstanz Alpha` and the local member from `test-enterprise-local.mjs`.
It creates clearly named test projects/runs; it does not claim VM or HA acceptance.

Server CORS permits the standard Tauri origins, without cross-origin cookie
credentials. Additional exact HTTPS origins (HTTP only on loopback) can be set
with `COWORK_DESKTOP_ORIGINS`; this profile adds `http://127.0.0.1:5173` for the
development shell. Wildcards, userinfo and path-bearing origins are rejected.

## Management

Use a **separate** management PostgreSQL database, not an existing application's
content database. Build images from this checkout using `server/Dockerfile` and
`app/Dockerfile.web`; use actual immutable registry digests.

Create these secret files in an operator-owned directory with mode `0700`:

| File | Value |
| --- | --- |
| `management_database_url` | Dedicated management PostgreSQL connection URL |
| `bootstrap_token` | Random initial administrator token, at least 32 characters |
| `audit_key` | Random HMAC key, at least 32 bytes; retain for verification |
| `provisioner_token` | Random provisioner credential, at least 32 characters |

Compose file secrets preserve source permissions. Permit container UID/GID 10001
to read the files while keeping their host parent directory private. Instance
runtime copies are created with these permissions by the provisioner.

Copy `platform-policy.example.json` to an operator-owned path. Its defaults are
catalogue-only extensions, support-grant content access, no laptop full access
and no public network destinations. Organisations cannot widen the ceiling.
Stop the fleet before tightening it, then restart management; the ceiling is
immutable within a process.

Configure these environment variables:

```text
COWORK_SERVER_IMAGE=<digest-pinned server image>
COWORK_WEB_IMAGE=<digest-pinned web image>
COWORK_MANAGEMENT_SECRETS=<absolute private directory>
COWORK_PLATFORM_POLICY_FILE=<absolute operator policy JSON path>
```

Create the external `cowork-services` and `cowork-routing` Docker networks as
operator infrastructure. Attach only trusted DB/S3 services to the first and
the management gateway/instance APIs to the second.

```sh
docker compose -f deploy/enterprise/docker-compose.yml config --quiet
docker compose -f deploy/enterprise/docker-compose.yml up -d
```

The web review URL is `http://127.0.0.1:8082/management`; the private management
listener is `http://127.0.0.1:8081`. Use a trusted TLS gateway for remote access
and overwrite forwarded headers there. Preserve the web gateway's denial of
`/management/runner/*`; never expose the provisioner API publicly.

Bootstrap an administrator with the existing sign-in form and create
organisations. Member assignment currently uses already-existing accounts;
create accounts through the existing platform invitation API. Per-organisation
invitations and the scoped central instance-login gateway remain outstanding.

## Provisioner

Requires Linux, Python 3.11+, Docker Compose, PostgreSQL client tools, `age`,
and an operator-configured MinIO `mc`. The private state directory must be
outside all user workspaces. Run only one provisioner against a state directory.
The manager never redelivers an expired lease. Stored completions are recovered;
unproven outcomes require the scoped review flow described below. The provisioner and sandbox runner are trusted
infrastructure services, never user-installable extensions.

Use an operator-owned JSON file outside the checkout. All image placeholders
below must be replaced with real `repository@sha256:...` digests.

```json
{
  "backend": "compose",
  "management_url": "http://127.0.0.1:8081",
  "provisioner_token_file": "/etc/cowork/secrets/provisioner_token",
  "state_directory": "/var/lib/cowork/fleet",
  "backup_directory": "/var/lib/cowork/backups",
  "backup_identity_file": "/etc/cowork/secrets/backup_identity",
  "database_endpoint": "postgres:5432",
  "database_admin_url_file": "/etc/cowork/secrets/postgres_admin_url",
  "s3_endpoint": "http://minio:9000",
  "s3_admin_alias": "cowork",
  "s3_admin_config_directory": "/etc/cowork/mc",
  "services_network": "cowork-services",
  "routing_network": "cowork-routing",
  "compose_network_pool": "10.240.0.0/16",
  "images": {
    "server": "<server digest>",
    "runner": "<runner digest>",
    "sandbox_core": "<core digest>",
    "sandbox_gui": "<GUI digest>",
    "sandbox_crew": "<crew digest>",
    "egress_proxy": "<Squid digest>"
  }
}
```

The admin DB URL must be reachable from the provisioner host; `database_endpoint`
and `s3_endpoint` from generated services. Remote management URLs require HTTPS.
Install the repository's AppArmor profile before provisioning: the adapter
requires it and fails if unavailable. Keep MinIO admin credentials in the private
`mc` configuration. Use an operator-only process/PID namespace because `mc admin
user add` accepts the generated secret as a process argument.

Use PostgreSQL infrastructure dedicated to the fleet, with PUBLIC access revoked
on management/default databases too. The provisioner revokes PUBLIC on databases
it creates; it does not reconfigure unrelated databases. Restrict DB/S3 network
access and use TLS across remote infrastructure boundaries.

```sh
python3 server/cowork-provisioner/provisioner.py --config /etc/cowork/provisioner.json
```

`--once` claims at most one operation. Receipts contain safe status/identifiers,
never subprocess output or secrets. Preserve private state in operator backups:
it contains database/object credentials, encryption keys and receipts. The standalone deployment's
backup script must not be used against this multi-instance deployment.

## Interrupted provisioning operations

Run one operator per private state directory. A Linux advisory lock is held by
the operator and its infrastructure subprocesses; a second operator fails closed.
Each claimed operation is journalled before execution. Success and failure
envelopes are persisted before delivery, bound to the original organisation,
instance, request and lease. Startup/poll recovery only delivers stored results;
it does not reclaim or repeat an interrupted operation.

An unproven outcome appears as `reconciling` in **Operations**. For a qualified
Compose container instance, choose **Review interrupted operation**, then
**Reconcile; stop if unproven**. A valid completion receipt takes precedence.
Otherwise this authorises stopping all work in that instance, verifying that
containers and database connections have stopped, and marking the original
operation interrupted. Starting again is a separate action. Instance managers
may request recovery; suspend/resume recovery requires organisation administration.
Permissions are rechecked before dispatch. Corrupt journals, missing manifests,
unexpected runtime containers or unprovable quiescence remain blocked for review.

`node app/scripts/test-enterprise-recovery.mjs` temporarily stops only the local
provisioner, injects process exits in a test-only wrapper, and exercises the
dedicated `Backup-Abnahme` instance. It requires the other provisioning queues
to be idle and always restarts the provisioner. `--prepare-review` leaves its
unproven operation for UI testing; after UI recovery and a separate successful
start, `--verify-review` checks the recorded outcome.

For database/API acceptance without WSL, build and run
`deploy/enterprise/management-tests.Dockerfile`. Its disposable PostgreSQL and
HTTP server run inside one container with `--network none`; it never connects to
the local installation's database.

## Local release retention

The operator's `image-set.json` is the fleet release, independent of current
management/provisioner build tags. Existing installations migrate this file from
their previous `config.json`. Changing development tags does not advance that
release. Instances also preserve their deployed image set. Keep all qualified
images available; backups record references and do not embed OCI image layers.

The setup script preserves existing local image digests before builds, and the
operator creates `retained-sha256-…` tags before starting Compose instances.
Do not prune these release images while instances or backups still require them.
For an already displaced local build, `scripts/recover-local-build-image.py`
can reconstruct its exact Linux/amd64 runtime from BuildKit history and load it
under a retained tag. It checks all imported blob digests, fails when runtime
blobs are missing, and does not claim to recover missing build attestations.
Production deployments need a durable release registry and its retention policy.

## Compose instance backups

Set `COWORK_MANAGEMENT_BACKUPS_ENABLED=true` only with a matching provisioner
that has `backup_directory`, an operator-owned `age-keygen` identity, `pg_dump`
and `pg_restore`. The local Docker Desktop profile prepares these automatically.
The backup directory and private state must be mounted at paths resolvable by
the Docker daemon, because bounded helper containers archive runtime volumes.

In management, stop an instance, select **Back up**, then inspect **Backups**.
The operator verifies that all instance containers and DB connections are stopped.
An encrypted `.age` archive contains the database dump, encrypted object data,
storage/runner/bootstrap keys, instance configuration and runtime workspace volumes.
The catalog contains only scope, timestamp, size and SHA-256. A read-after-write
decryption check qualifies the capture. Export the encrypted archives and catalog
metadata to separate durable storage; keeping them in the same Docker Desktop
volume is not disaster recovery. Keep the recovery identity separately and test
its recovery. For this local profile it is the protected, Git-ignored file
`.internal/enterprise-local/fleet_backup_identity`.

**Restore** offers only this instance's catalog entries. Restore requires a stopped
Compose container instance and exactly its currently installed image digests;
cross-release migrations, Kubernetes/VM disks and fresh-install imports remain
unavailable. Archive scope, digest, file list and individual hashes are checked
before any new resources are created. Restore creates fresh DB credentials,
database, private bucket, control/bootstrap credentials and workspace volumes.
Only the data decryption key is retained. The active generation changes
only after staging succeeds. Previous resources remain retained for operator
reconciliation; arbitrary generation switching and automatic retention remain
operator procedures. Permanent instance deletion now includes those retained
generations. Updates have the scoped rollback flow below.

After restore, start and reopen the instance through management. Historical
sessions, executor credentials and terminal/desktop tickets are revoked;
unfinished runs are interrupted and schedules are disabled for review. Launch
tickets issued before restore are rejected. Client resynchronisation and offline
device recovery still need their separate acceptance. Failed staging preserves
the original stopped generation when the operator can verify that it is unchanged.
A crash or ambiguous activation requires reconciliation instead of a blind retry.
Ordinary starts preserve the deployed images; rebuilding the operator does not
silently upgrade stopped instances.

`cd app; node scripts/test-enterprise-backup.mjs` exercises the dedicated
`Backup-Abnahme` fixture with a real database, encrypted file and sandbox volume.
It also tests tamper rejection, scope checks, fresh storage generations, revoked
sessions and suspended unfinished work. `--fresh` creates a separately named
instance when qualifying a new release; retained test instances still count
towards the organisation's quota.

## Compose updates and qualification

Enable `COWORK_MANAGEMENT_UPDATES_ENABLED=true` with backups enabled and an
operator using `release_catalog_sync: true`; the local profile enables these.

Initial version selection for new instances is installed locally at schema 49.
It requires the matching management/operator versions and explicitly enabled
selection switches; it does not qualify an update of existing data. Both switches
default to false in deployment templates. The local installation persists both
enabled values in the private `control-plane-features.env`; startup validates
the paired keys and rejects conflicting caller environment overrides. Existing
tenants and default provisioning images remain unchanged.
See [initial instance versions](../../docs/ENTERPRISE_PROVISIONING_RELEASES.md)
for the API, frozen image bindings, recovery behavior and isolated test evidence.
The operator has a release catalog and a backup-first, separate-generation update engine. Release
manifests contain an immutable six-image digest set and exact supported source
hashes; management requests cannot provide host commands or arbitrary images.
The private catalog snapshot advertises installed release hashes and approved
transitions. Stale or withdrawn catalogs cannot authorize dispatch. In management,
stop an instance, choose **Upgrade**, select a release and use **Test and restore
current version** first. After a successful test, **Install tested version**
becomes available. Both actions create a verified backup and leave the instance
stopped for an explicit start. Installation rotates runtime credentials; reopen
the instance through management and reconnect devices. Original generations are
retained. An updated instance can back up and restore its current release.

To undo the latest installed update, stop the instance and choose **Roll back
update**. Select the promotion and acknowledge that changes since its pre-update
backup will no longer be active. The operator first captures the current state,
then restores the previous images **and their matching data** into fresh database,
object and workspace resources. It never starts old code against the newer schema.
The restored version is probed without user routing and remains stopped until
explicitly started. Reopen the instance and reconnect devices; review and enable
schedules as needed. The newer generation and both backups remain retained.
A subsequent restore or another installed update invalidates the old rollback
point. Withdrawn target releases cannot be restored through this action.
An interrupted rollback uses **Review interrupted operation** to return to the
recorded newer source, without repeating staging or discarding newer data.

Canary qualification is valid only for the tested source/target pair and a
completed durable operation marker. The API is detached from the instance
gateway throughout candidate and rollback probes. A failed candidate returns to
the preserved source when quiescence can be proved; otherwise reconciliation is
required. Successful promotions remain stopped. Probe startup and one sandbox
job do not establish full application/schema/GUI/VM compatibility.

For ordered updates across multiple Compose instances, open **Batch updates**
in the organisation. Select up to 40 ready/stopped instances in the desired order,
choose a release qualified for every exact installed source, and set a group size
of 1–10. Acknowledge maintenance downtime before starting. All selected instances
are reserved together: other lifecycle operations and organisation policy changes
are rejected until the affected reservations are released. Initially running
instances are stopped, backed up, updated and restarted; initially stopped ones
remain stopped. The next group starts only after the previous group succeeds.
Users and devices must reconnect after credential rotation; review paused
schedules after installation. Group size bounds the active group, not the number
of parallel Docker operations the operator can perform.

The plan and its original operation IDs survive management/operator restarts.
**Pause** prevents new dispatch; already running operations may finish.
**Resume** continues a manually paused plan. Any failed or uncertain operation
pauses the rollout and requires review; uncertain actions are never automatically
repeated or reassigned. **Cancel rollout** cancels undispatched steps and releases
each target only after its running operation is settled. It does not roll back
completed installations or restart stopped instances. Reconcile interrupted
operations before making a new plan. Administrators control plans; auditors have
read-only progress access. This workflow currently supports Compose container
instances; Kubernetes/VM rollout acceptance remains outstanding.

Pause the operator before editing its catalog so its exclusive state lock is
available. Publish a reviewed manifest with `python3 publish_release.py --config
/operator/config.json --manifest /operator/release.json`, or withdraw its hash
with `--withdraw <release-hash>`. The manifest fields are `version: 1`, `id`,
`name`, `images` and `supported_sources`; `id` is the SHA-256 of the sorted compact
JSON image map. Publication validates all six immutable image references and
refuses rewriting a published manifest. Withdrawal is persistent and republishing
the same document does not reactivate it. Restart the operator to synchronize.
The container contains the CLI under `/opt/cowork-provisioner`. No tenant receives
the catalog, state volume or infrastructure credentials.

Build the operator candidate with
`powershell -NoProfile -File scripts/Build-ProvisionerCandidate.ps1`. This stages
only its source files outside the cloud-backed checkout. It never changes the
fleet image set or the running operator by itself.

`node app/scripts/test-enterprise-updates.mjs` is restricted to the dedicated
local backup fixture by both UUID and name. It requires idle provisioning queues,
stops that fixture and the normal operator, and tests injected failure, canary
rollback and promotion using the previously built admission-runner candidate.
It retains backup/generation evidence, restores the original fixture, restarts
the operator, then starts the original instance. Results are private under
`.internal/enterprise-local/update-acceptance-results.json`. It does not roll out
the candidate to user instances. If verification fails, inspect the retained
state before restarting a stopped fixture. Do not manually replay an interrupted
update. **Review interrupted operation** reports an existing receipt, or restores
the recorded source generation and leaves it stopped when explicitly requested.
Missing/corrupt evidence keeps the operation blocked for review.

`node app/scripts/test-enterprise-update-api.mjs` exercises the public queue,
canary, promotion, automatic backup catalog and a subsequent backup/restore/start.
It retains its own named instance, which consumes one quota slot; the fixture and
result are stored in the private test directory. A previously completed fixture
needs a new supported target to rerun the transition test.
`node app/scripts/test-enterprise-update-recovery.mjs` injects a process exit after
the candidate probe on the separate backup fixture, restarts the operator, and
verifies explicit reconciliation to the source without repeating staging.
`node app/scripts/test-enterprise-rollback.mjs` uses the same UUID/name-guarded
backup fixture and requires idle fleet queues. It installs the approved runner,
changes application data, interrupts a real rollback after probing, verifies
explicit recovery, then completes a new rollback. It checks DB/artifact/workspace
contents, fresh credentials, revoked sessions and paused schedules. Private
results are in `.internal/enterprise-local/rollback-results.json`.
`node app/scripts/test-enterprise-rollout.mjs` creates its own guarded organisation
and two instances, consuming their quota. It tests ordered groups, pause during a
running stop, management restart, continuation using original operations, per-update
backups/rollback points and preservation of initial running/stopped states. It
retains fixtures and private results in `.internal/enterprise-local/rollout-results.json`.
An unfinished run must be inspected before continuing; a completed fixture needs
a new supported, qualified target before repeating the transition test.
Public catalog editing and broader compatibility acceptance remain outstanding.

## Administrative content access

### Instance membership and read-only collaboration

Open **Instance members** on an instance card to assign an existing organisation
member as manager, member or reader. Managers operate the instance and manage
assignments; members use their project permissions; readers have a central
read-only ceiling. Assignment never shares all projects. An instance's owner is
shown separately and cannot be removed or demoted through this panel. Audit-only
organisation members cannot receive execution/content assignments. Previously
assigned users demoted to auditor remain listed for review and removal, but
cannot enter the instance. Assignment changes and removals enter the audit chain.

Reader sign-in requires an upgraded instance advertising content-access protocol
version 2. The separate `instance_viewer` ticket purpose is rejected by older
instance APIs; an unavailable reader profile never falls back to full access.
This capability version is separate from the runtime/device protocol, which
remains v2. Deploy the management update and shared frontend before qualifying
and installing the instance release. New viewer sessions use **Read content**;
the originating instance still checks project/team sharing permissions.

Revoking an assignment denies subsequent instance requests and launches.
Organisation administrators can still qualify for the independently controlled
support access described below. Personal-device credentials also require a
current owner with execution access, including on already-connected agent
WebSockets. Desktop/terminal exchanges recheck the user and object permission;
active relays recheck every two seconds. A failed central check closes the
connection. No statement about offline task cancellation or automatic replay is
implied by these online checks.

`node app/scripts/test-enterprise-members.mjs` exercises only the guarded second
rollout fixture after release qualification. It preserves private test-account
information under `.internal/enterprise-local/instance-members-fixture.json` and
writes verification results to `instance-members-results.json`. Do not commit
either file. It creates a test project, shares the pre-existing fixture project,
tests member-to-reader demotion and finally removes the instance assignment.

### Administrative approvals

Deploy the upgraded management service before its matching instance API. The
instance advertises content-access version 1 using its scoped key; management
shows **Open support access** for organisation administrators when that capability
is fresh. Existing instance members retain **Open instance**. A support launch is
a distinct signed ticket purpose that older instance APIs reject. Old instance
access callbacks continue to authorise only regular members, never administrators
merely because of their infrastructure role.

In an upgraded instance, **Governance → Temporary support access** lets project
editors select an administrator from the current organisation and approve a project
or one chat for at most 24 hours. The recipient need not have signed in before.
The recipient can read only the approved content and can relinquish the approval;
the grantor and authorised project editors can revoke it. The effective rule is
shown in the workspace. **Read content** opens authorised projects and chats without
editing or execution controls. A chat approval exposes its parent project name and
navigation IDs, but hides the project description, execution configuration and
unapproved chats. Approvals remain in the original instance database.

For reading without individual approval, all three conditions must hold: the
operator ceiling permits `organisation_admin`, the organisation and instance
policies permit it, and **Members** explicitly assigns that administrator the
content-reader permission. The flag defaults to false and is cleared when an older
membership request omits it or when the role is changed away from administrator.
Changing an organisation policy does not widen an existing, stricter instance
policy. Newly created instances can inherit the permitted organisation profile.
Platform administration without organisation membership does not grant content
access. Content permission always grants reading; it grants no execution, device
pairing, settings management or interactive desktop/terminal access.

Management records administrative read authorisations in its existing chained
audit log with actor, instance, object ID, approval ID where applicable and policy
revision. It does not record the content or support reason. Failure to record the
central audit denies the read. Expired/revoked approvals and revoked administrator
roles are rechecked on requests and during run-event streaming. Already delivered
data cannot be recalled. The independent audit witness and retention/export
source is documented in the operator lifecycle section above; that role needs
its separate external deployment and acceptance.

`scripts/test-enterprise-content.py` tests real HTTP APIs against isolated disposable
databases. `node app/scripts/test-enterprise-content.mjs` is the separate live
Docker acceptance on the UUID/name-guarded second rollout fixture. It tests a canary,
backed-up promotion and actual approval/read/revoke through the managed instance.
It retains a scoped test administrator and writes private credentials/fixtures and
results under `.internal/enterprise-local/`; it does not advance user instances.

## Kubernetes manifests

`management/` requires an existing Secret containing the five management secrets,
a pinned image, DB IP ranges and trusted gateway/provisioner namespaces.
`instance/` includes the native Kubernetes runner, durable runner journal,
namespace quota, scoped service-account RBAC and separate sandbox network
policies. API and worker Pods mount no service-account token or Docker socket.
Only the native runner receives its namespace-scoped API token plus read-only
access to the explicitly enrolled RuntimeClasses, Nodes and kubelet `configz`.
Management actions require an activated original backend profile, current host
capacity evidence and the operation's original instance/organisation/host binding.

The native runner supports Core/Crew jobs, persistent chat workspaces, artifact
reads, a shell terminal and physical cancellation. An explicit `desktop_profile`
adds GUI tool execution and control/view-only VNC with version-2 proofs carrying
actual Pod, PVC, Node, namespace and cluster UIDs and CRI identity. Docker proof
version 1 remains separate. The profile enables desktop capabilities only when
the corresponding dedicated GUI node pool is configured. Jobs with unknown
create/exec acknowledgements remain held for original-operation reconciliation.

Provision/start/resume, stop/suspend, encrypted DB/object/workspace backup,
verified fresh-generation restore, admission enrollment, update/canary/rollback
and permanent deletion dispatch through `kubernetes_lifecycle.py`. Helm uses a
private immutable chart copy and checks the original release Secret UID/revision
and existing resource UIDs before dispatch. Updates probe actual API/worker and
native sandbox processes behind a maintenance network policy, then leave the
instance stopped. Deletion binds every retained PVC and CSI volume, database,
object identity, backup and namespace to its original plan. A Kubernetes object
disappearing alone does not prove process termination: owned Pods retain a
physical-stop finalizer until kubelet terminal evidence is captured.

For installation, first create the reviewed dedicated node pools, RuntimeClasses,
CSI StorageClass (`Delete` reclaim policy), exact instance namespace, original
default ServiceAccount and native-runner TLS Secret. Record their real UIDs;
namespace labels must be `cowork.instance-id`, `cowork.organisation-id` and
`cowork.executor-host-id`. Render the chart with the intended instance values and
bootstrap only its native-runner dependencies and original runner; API/worker
remain stopped until profile activation. Enroll every pre-existing bootstrap
resource by `{kind,name,uid}` in `kubernetes_execution.bootstrap_resources`,
including the default ServiceAccount. If bootstrap used Helm, also record
`bootstrap_release:{secret_name,uid,revision}`. Empty resource lists do not permit
adoption of matching names or labels. An old external-runner installation without
original resource receipts requires a separately reviewed migration; it is not
automatically adopted by native enrollment.

The operator config requires explicit `kubeconfig_file`, `kubernetes_context` and
`kubernetes_execution` values: `version:1`, `runtime_class`, `storage_class`,
`node_selector`, exact `node_names`/`node_uids`, `system_pod_uids`,
`workspace_claim_gib`, `runner_tls:{secret_name,certificate_uid}`,
`runner_ca_file`, `runner_certificate_sha256`, HTTPS `runner_url`, `service_cidrs`,
`routing_namespace` and positive `max_workspace_archives`, `max_archive_bytes`,
`max_workspace_archive_bytes`. `desktop_profile` is either null or
`{version:1,runtime_class,node_selector,node_names,node_uids}` for a disjoint GUI
pool. Both selectors must contain `cowork.role: executor`,
`cowork.trust-domain: <organisation UUID>` and `cowork.host: <executor host UUID>`.
An additional pool label selects the GUI nodes. The operator and runner must be able to read real kubelet
`/api/v1/nodes/<name>/proxy/configz`: service/Core nodes require positive
`podPidsLimit <= 64`; GUI nodes require a positive measured value no greater than
the original desktop limit (64–4096). Labels do not establish PID enforcement.
The CNI must enforce all chart NetworkPolicies and support kubelet startup and
readiness probes. Configure original DB/S3 endpoints, backup keys and current
release/admission allowlists before activating the profile. The stopped instance
keeps one authenticated native control Pod with a durable closed execution
barrier; API/worker/egress and workload Pods are stopped.

These sources have syntax, manifest-render and native compile checks. No live
Kubernetes deployment, CNI/CSI fault test, GUI acceptance or production failure
qualification is claimed by those checks.

Management defaults to two replicas, node spreading and a disruption budget.
These settings do not establish working failover. Production still requires HA
PostgreSQL/object storage, a qualified CNI, native sandbox/VM backends, persistent
VM storage and live failure tests.


## Legacy admission enrollment preflight

An additional **candidate-only** operator preflight for migration to central
execution admission is available as `execution_enrollment.py`. It inspects
stopped legacy resources and an encrypted backup under the operator lock; it
does not enroll an instance or activate quotas. Keep the daemon inactive while
running the check, and never bypass its lock. See
[the preflight procedure and its limits](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md#legacy-enrollment-preflight).
The live Docker Desktop images are not replaced by the preflight test harness.

A separate **candidate-only** `enroll_execution` transaction and management UI
are now available with explicit management/provisioner opt-in. They capture a
fresh encrypted backup, verify all retained generations and install central
admission configuration while leaving the instance stopped. Interrupted work
requires explicit `continue_enrollment` recovery. See
[configuration, operation and qualification limits](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md#explicit-compose-enrollment).
The initial Docker acceptance covers the current candidate in legacy mode. A
separate [cross-version acceptance](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md#upgrade-from-the-installed-release)
now covers the recorded installed server/provisioner image pair, encrypted
management database recovery, canary/promotion and enrollment with retained old
generations. It uses disposable generated data and does not migrate the live fleet. Device histories,
VM/Kubernetes enrollment and full mixed-workload acceptance remain outstanding.

The tenant-image variant also accepts `--source-instance-images` with a JSON map
of all six immutable source images and `--source-instance-migration` with the
expected schema version. Management and tenant source versions are checked
separately. It verifies actual workspace files, encrypted attachments and new
shell work after promotion. Interrupted server history is retained without replay;
registered devices and ambiguous/runnable work continue to block enrollment.
See [tenant qualification and its limits](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md#tenant-image-combinations-and-dormant-work).
The candidate management/provisioner/web packages are installed in the local
control plane, but default deployments and the running fleet do not enable admission.

For a separate read-only backup/migration rehearsal on existing local management
data, see [the control-plane rehearsal](../../docs/ENTERPRISE_EXECUTION_ADMISSION.md#rehearsal-with-the-existing-control-plane-database).
Its explicit `--capture-local` command preserves an encrypted backup and uses only
isolated generated containers for restore/migration. Add `--stopped-control-plane`
only after separately stopping the management API, web, operator and (for schema
48) its private TLS gateway: the command verifies that condition and zero other
management-database clients, and leaves the source services stopped. Supported
transitions are 43 to 48 (the default) and 48 to 49 with explicit
`--source-migration 48 --target-migration 49`. Source schema and pinned server
images must match. Schema-48 captures include the installed image override,
installation pointer, TLS certificate/key and any persisted feature switches.
Admission must remain disabled for this maintenance procedure.

## Permanent Compose instance deletion

The Docker Desktop profile enables `COWORK_MANAGEMENT_DELETION_ENABLED=true` and
`COWORK_INSTANCE_DELETION_ENABLED=true`. Other installations default to disabled;
the provisioner configuration must explicitly set `deletion_enabled: true`.
The original Compose path and explicitly activated VM/native Kubernetes profiles
have separate bound deletion adapters. Remove retained environments before
deleting their instance; uncertain environment runtimes keep their reservations.

Choose **Delete**, enter the exact instance name, and acknowledge deletion of
**all managed backups**. This removes current and retained restored/update
resource generations, database credentials, private object-store identities and
buckets, encryption keys, workspace/admission volumes, private networks and
managed backup archives/staging. Shared infrastructure, images, the operator's
backup identity and exports copied outside the managed store are not removed.
Central audit/operation metadata and private frozen deletion inventories/receipts
remain, including names of revoked storage identities, but no instance secrets.
Removal is logical; it does not promise physical disk sanitization.

Once dispatched, the instance is inaccessible and remains `deleting` until a
verified completion arrives. An interruption does not release its quota or
make it startable. After investigating the reported cause, select **Review
interrupted operation**, re-enter the name and acknowledge backup removal to
permit one further deletion attempt. A lost/failed recovery dispatch needs another
explicit confirmation. Existing verified completion receipts can be delivered
without repeating destructive work. Unknown ownership, unexpected volumes,
unattributed legacy backup staging or corrupt evidence require operator review.
Do not remove the private inventory or set a partially deleted instance to ready.

`node app/scripts/test-enterprise-deletion.mjs` creates a new, exclusively disposable
fixture under the ordinary invitation-test administrator (requires the private
`organisation-invitations-fixture.json`). It never selects an existing instance
for deletion. The test provisions a real sandbox, writes a workspace file and an
encrypted object, backs up and restores into a second generation, then forces
Docker volume removal to fail using a container holding only the test volume.
It verifies no automatic retry, explicit recovery, resource/key/archive removal
and unchanged neighbouring tenant containers. Evidence is written privately to
`.internal/enterprise-local/deletion-results.json`.

### Templates and persistent environments (source item72)

Migration0067 adds original template/environment registration requests, immutable
qualification/action plans, retained leases, scoped qualifications and final
runtime receipts. Registration alone leaves a template disabled. The management
console's **Templates and environments** selector provides qualification and
create/start/stop/inspect/delete actions, original request retries, job inspection
and explicit acknowledged-runtime recovery. Permanent deletion requires the
current environment name. Current administrator/instance-manager authority and
the original enabled host/profile are checked again at dispatch and completion.

Enable `"environment_lifecycle":{"version":1}` in the paired native Kubernetes
operator configuration. The original runtime class, actual NodeUIDs, measured
kubelet PID limit at most64, NetworkPolicy-enforcing CNI and CSI Delete storage
are mandatory. Environment Pods use their own immutable UID/finalizer receipts,
deny ingress/egress, no API token, nonroot/read-only container roots and bounded
CPU/RAM. Persistent/shared PVCs remain bound to their original claim UID and
reference ledger. Actual mounted filesystem size is measured by the probe.

For an explicitly activated own VM, install Python3, coreutils, util-linux and
e2fsprogs **inside that VM**, then install the reviewed
`server/cowork-provisioner/environment_guest_storage.py` root-owned as
`/usr/local/libexec/cowork-environment-storage.py`. Add this source-pinned opt-in
to the paired operator file, replacing the SHA with the actual installed source:

```json
{"environment_lifecycle":{"version":1,
 "guest_storage_helper":"/usr/local/libexec/cowork-environment-storage.py",
 "guest_storage_helper_sha256":"<actual SHA256 of installed helper>"}}
```

The operator uses the original pinned guest SSH key, VM hardware UUID and daemon
identity. The root helper allocates full ext4 images, records their inode,
filesystem UUID, actual loop device/mount and Docker volume binding, and checks
them on every probe. Runtime containers have no network, privilege, Docker socket
or host path access, enforced AppArmor/no-new-privileges, cgroupv2 memory/PID caps
and digest-pinned images. Guest reboot/mount loss is held for review; this source
does not silently recreate or adopt missing mounts. The operator's normal locked
work loop consumes `/management/runner/v1/environments/{claim,pending}` alongside
its original instance operations.

Templates must contain `/bin/sh`, `sleep infinity`, `id`, `cat` and `stat`. The
bounded qualification runs the image, verifies a fresh challenge plus writable
workspace and actual resource/storage caps, records real runtime IDs, then stops
and removes the probe and its storage reference before enabling the template.
The qualification is scoped to that instance's original physical profile. API
clients can use these same concrete routes (all requests use normal authenticated
management HTTPS and current MFA requirements):

```text
POST /management/v1/organisations/ORG/templates
  {request_id,name,backend:"docker"|"kubernetes",isolation,image_reference:"registry/image@sha256:..."}
POST /management/v1/organisations/ORG/instances/INSTANCE/templates/TEMPLATE/qualify
  {request_id,expected_instance_revision,template_revision,action:"qualify"}
POST /management/v1/organisations/ORG/instances/INSTANCE/environments
  {request_id,instance_id,name,backend,isolation,template_id,template_revision,
   persistent,shared_workspace,limits:{cpu_millis,memory_mib,storage_gib,concurrent_runs:1,instances:1}}
POST /management/v1/organisations/ORG/instances/INSTANCE/environments/ENV/actions
  {request_id,expected_instance_revision,expected_environment_revision,
   action:"start"|"stop"|"inspect"|"delete",confirmation_name:"required exact name for delete"}
GET  /management/v1/organisations/ORG/instances/INSTANCE/environment-jobs
POST /management/v1/organisations/ORG/instances/INSTANCE/environment-jobs/JOB/recover
  {plan_sha256,confirmation:"STOP_ACKNOWLEDGED_ORIGINAL_ENVIRONMENT"}
```

All retained environments conservatively reserve CPU/RAM/storage and a slot in
the effective instance/organisation/platform pool. Active or uncertain
environments block ordinary task admission, and retained environments block
instance lifecycle/profile changes. Existing held executions must finish before
an environment action can start. Stopped persistent storage remains charged;
nonpersistent references are released after confirmed stop. Shared workspaces
must use equal storage caps. Host capacity collectors classify environments only
from actual original private create-ACK journals and measured runtime limits.
Lost create/start/exec acknowledgements are never replayed. Explicit recovery can
stop an acknowledged original runtime; a missing original ACK or lost physical
node remains held until trusted physical fencing, never freed by disappearance.

### Independent physical VM retirement witness

`scripts/enterprise-host-retirement-witness.py` has plan/retire/verify commands
for an independent parent/controller, using its separately enrolled RSA key.
The trusted consumer supplies its own original intent: format
`cowork-original-host-retirement-intent-v1`, original request/operation/org/
instance/host/VM/operator-state/execution-epoch UUIDs, original daemon ID,
`minimum_isolation:"virtual_machine"` and original runtime IDs. Keep any original
lease/assignment fields in that same intent; they are included in the signature.
Never derive the expected intent or trusted public key from the witness itself.

```sh
python3 scripts/enterprise-host-retirement-witness.py plan \
  --vm-root /absolute/own/cowork-executor-VMHEX --intent /private/original-intent.json \
  --backend-receipt /private/original-activation.json --output /private/retirement-plan.json
python3 scripts/enterprise-host-retirement-witness.py retire \
  --plan /private/retirement-plan.json --signing-key /private/controller-rsa.pem \
  --confirmation RETIRE_EXACT_ORIGINAL_STOPPED_VM --output /private/retirement-witness.json
python3 scripts/enterprise-host-retirement-witness.py verify \
  --witness /private/retirement-witness.json --intent /private/original-intent.json \
  --public-key /trusted/enrolled-controller-public.pem
```

The VM must already be physically stopped. The controller accepts only its
original version2 allocation and CAS revision, rejects adopted/protected VMs,
checks process/QMP/mount/file-descriptor evidence and records actual inode
deletion witnesses before releasing its parent capacity allocation. The RSA
payload contains that actual tombstone, original allocation/source hashes and
original intent; a durable signature is replayed unchanged after a lost output.
This adapter covers the owned QEMU VM. It cannot certify a Kubernetes node, an
arbitrary host, a replacement VM or copied disappearance evidence. No management
reservation is automatically freed merely by producing a file. Deployment,
key enrollment and installed runtime acceptance remain operator steps.

### Bound VM controllers, encrypted disk snapshots and OpenShell (source83/84)

Migration0068 adds original controller plans, current physical assignments,
snapshot jobs and environment execution journals. The environments console
selects only enrolled controllers and exposes their original VM power actions.
The independent VM must differ from the product API/control VM; OpenShell runs
on that original product VM. A backend name alone never grants VM access.
Operators prepare the independent VM with their native hypervisor tools and
install the source-pinned guest storage helper before enrollment. Nothing in
these commands provisions or adopts the protected pilot.

Run the producer on the Linux parent that owns the original executor-host
credential and VM/cluster access, with an owner-only controller JSON file.
Common fields are `format:1`, `backend`, actual UUIDs `organisation_id`,
`instance_id`, `host_id`, `operator_state_id`, `controller_id`, `vm_id`,
`guest_hardware_uuid`, actual `guest_daemon_id`, `controller_identity`, measured
`limits:{cpu_millis,memory_mib,storage_gib}`, explicit `protected_vm_ids`,
private `power_state_directory`, `credential_pins` (absolute filename to actual
SHA256) and `guest:{hostname,port,username,ssh_key,known_hosts}`. Pin every SSH
key/known-host file separately. In the guest, root installs mode0600
`/etc/cowork/environment-owner.json` containing exactly the original
organisation/instance/operator-state/controller/VM UUIDs under those same keys.

| Backend | Required actual parent facts/configuration |
| --- | --- |
| `libvirt` | Local `libvirt_uri:"qemu:///system"`, parent machine-id as controller identity, `disk_file`, `virsh` and `qemu-img`; domain UUID, owner XML metadata at `https://open-cowork.org/enterprise/v1`, fixed KiB memory/currentMemory, CPU quota/period, one standalone disk and no host-device passthrough. |
| `kubevirt` | `kubeconfig_file`, `kubernetes_context`, namespace/name and original namespace UID, cluster kube-system UID as controller identity, VM UID, disk claim/UID, pinned node name/UID; VM owner annotation `open-cowork.io/environment-owner`, fixed CPU/RAM limits equal requests and firmware hardware UUID. Active migrations are held. |
| `hyper_v` | Private `controller` SSH endpoint/key/known-host files, actual Windows computer name as controller identity, original disk ID/path, VM Notes containing the exact owner JSON, fixed memory, positive processor Maximum and one VHD without a parent. |
| `attached` | `vm_root`, actual private `qemu_controller_file` plus `qemu_controller_sha256`, original version2 capacity ledger/QMP/KVM allocation and parent machine-id. Adopted/protected allocations are refused. |

The manager credential JSON has exact fields `url` (the actual public HTTPS
origin ending `/management/v1`), `ca_file`,
`access_token_file` and `operator_token_file`. Public routes and private
`/management/runner/v1` must resolve to the same reviewed control origin.
Use the real current instance and controller revision (zero before enrollment):

```sh
python3 scripts/enterprise-environment-controller.py plan \
  --config /private/controller.json --manager-config /private/manager.json \
  --instance-revision CURRENT --controller-revision CURRENT_OR_ZERO \
  --request-id ORIGINAL_UUID --output /private/controller-plan.json
python3 scripts/enterprise-environment-controller.py inspect \
  --config /private/controller.json --plan /private/controller-plan.json \
  --output /private/controller-inspection.json
python3 scripts/enterprise-environment-controller.py activate \
  --config /private/controller.json --plan /private/controller-plan.json \
  --manager-config /private/manager.json --output /private/controller-activation.json
python3 scripts/enterprise-environment-controller.py pair-provisioner \
  --config /private/controller.json --plan /private/controller-plan.json \
  --activation-receipt /private/controller-activation.json \
  --provisioner-config /private/original-provisioner.json \
  --output /private/paired-provisioner.json
```

Activation saves its actual original observation before POST. Retry uses those
same bytes. Physical start/stop uses current manager authority before IO, native
UUID/RV checks and an original private journal. KubeVirt saves actual launcher
Pod/Node/CRI terminal evidence behind a physical-stop finalizer before cleanup.
A later start needs a prior positive stop receipt; missing VMI/node resources
cannot supply that receipt. Unknown power outcomes block environment dispatch.

Disk snapshots require the original owned QEMU parent, a physically stopped
instance with no retained environments/executions, `qemu-img`, `age`, SSH and
the reviewed controller installed on that parent. Configure `vm_snapshot` with
exact fields `version:1`, `controller_file`, `controller_sha256`, a dedicated
mounted ext4 `store_directory`, its actual `store_filesystem_uuid`,
`maximum_bytes`, `age_recipient` and private `recovery_identity_file`. The real
filesystem total must fit the bound; full scratch reservations must fit before
copying. The UI or POST to
`/management/v1/organisations/ORG/instances/INSTANCE/vm-snapshots` submits
`{request_id,expected_instance_revision,action:"snapshot"}`. Restore/deletion
also require original `snapshot_id`, `expected_snapshot_revision` and exact
`confirmation_name`. These paths stop the original VM, verify the source disk,
perform qemu-img comparison/check, age encrypt/decrypt verification, inode/SHA
CAS restore or actual archive unlink, restart the same VM and read back
QMP/KVM/SSH/mTLS plus stopped producers. They do not replace PostgreSQL/S3
instance backups. Lost IO ACKs remain held, with no automatic disk replay.

The `open_shell` controller additionally pins `grpcurl_file`,
`descriptor_file`, `gateway_ca_file`, `gateway_client_certificate`,
`gateway_client_key`, `gateway_token_file`, `parent_profile_producer_file` and
`parent_backend_configuration_file` in `credential_pins`. Set actual
`gateway_endpoint`, TLS certificate DER hash as `controller_identity`,
`gateway_version`, authenticated `gateway_subject`, private `workspace` and
`workspace_uid`, root-owned `gateway_systemd_unit`, `gateway_binary_sha256`,
`gateway_guest_port`, measured `gateway_limits`, `parent_disk_virtual_gib` and
immutable `trusted_auxiliary_image_ids`. The gateway/driver use the reviewed
OpenShell v1 protobuf descriptor and a private workspace with exactly its one
enrolled operator admin. Enable only reviewed Docker driver volume/tmpfs
configuration; the actual installed Landlock hard policy restricts UID10001 to
workspace/tmp writes, with no network policies or providers. The adapter
verifies TLS peer, current authenticated workspace/SDK version, actual parent
VM, gateway executable/listener/cgroups and real Docker resources. It bounds
all workload/supervisor CPU/RAM/PIDs together; a VM driver that merely accepts
portable limit flags is refused. Stop records each actual original container's
terminal fact before SDK cleanup, waits for the SDK stopped barrier and verifies
metadata/resource absence. Installed acceptance of the selected SDK release and
physical hypervisor remains an operator gate.

### Original environment tasks and file transfer legs

Private POST `/management/runner/v1/environment-execution` receives format1
with original request/workflow/step/org/instance/epoch/user/management-session,
environment ID/revision/profile SHA, frozen `task_input` and exact capabilities.
`process` inputs contain an argv vector, optional bounded stdin and timeout
1..300 seconds, with `['process.execute']`; templates need `timeout`, `base64`
and the actual requested executable. File operations need Linux `/usr/bin/python3` and
kernel file leases in the qualified template. Their input has `operation`
`file_read` or `file_write`, safe relative workspace path, original transfer
request/grant/revision and SHA baseline; writes add <=256KiB decoded content.
Capabilities are exactly `['file.read']` or `['file.write']`. Before file IO the
manager calls the current source instance's fixed private transfer-authority
route with its actual epoch key and current grant/session/MFA. Configure
`COWORK_INSTANCE_GATEWAY_CA_FILE` when using a private gateway CA. Process
dispatch also calls the original instance's workflow environment-authority route
on submit/claim and every new IO, checking the exact active original DAG step and
dispatch session. A stopped parent cannot authorize a delayed new task.

Existing-file CAS requires a positive kernel write lease on the original
single-link owned inode and SHA baseline. Missing-file creation uses O_EXCL;
baseline/lease contention produces a unique conflict copy. Every component is
opened through directory FDs with O_NOFOLLOW. Read/content/output bounds are
256KiB. Broken leases, changed path identities and unknown terminal command
ACKs remain held; sandbox-owned output files cannot forge a completion. The
operator alone persists the original result after the actual terminal RPC/CLI
ACK and readback, then redelivers those same bytes after a manager POST ACK loss.

GET `/management/runner/v1/environment-execution/REQUEST` uses current
`organisation_id,instance_id,user_id,management_session_id` query fields and
current fleet key. A fresh login of that same original user may inspect an old
receipt without replacing its dispatch authority. POST to that same path plus
`/stop` blocks future dispatch/authorization. A positively unclaimed queued job
halts; an already claimed task whose process identity/termination is unproven
stays held and charged. This source never reports a physical stop from a SQL
flag. File completion includes `file_result` with content/hash/size, original
destination before/after SHA, conflict alias and inode/device/kernel-lease
facts; the instance coordinator encrypts content and retains immutable original
receipts. No model task is inferred from a generic environment flag.

For an independently enrolled original-host retirement consumer, use
`enterprise-manager-host-identity.py` with the original backend config,
management HTTPS CA/host-scoped token and separately enrolled RSA3072+ key. It
exports the actual manager's `execution_epoch` plus exact original host/VM/
operator/daemon/profile revision/activation-receipt SHA and a detached RSA seal.
The durable original export journal prevents a lost output from minting a new
identity. The consumer enrolls these bytes/key outside the guest before taking
any retirement intent; witness files never supply their own trust root.


### Original total physical disk quota (71)

The explicit `cowork-total-disk-specification-v1` path covers a new owned Linux
VM and its original controller. It is separate from the old shared PostgreSQL/S3
placement. It reserves the complete, fully allocated ext4 disk maxima before IO;
PostgreSQL data and WAL, MinIO objects, Docker images/volumes/workspaces, retained
instance generations, scratch and backups are placed on those filesystems.
The entire configured snapshot-store maximum counts even when the store is
empty. A missing archive, lost host, expired lease or absent container never
releases a disk reservation. Generic cloud/CSI/PVC requests and unmeasured shared
services do not qualify through this adapter.

Use the delivered `scripts/enterprise-disk-quota.py` and
`server/cowork-provisioner/disk_quota_guest.py`. Install the same reviewed helper
on the original VM and controller as a root-owned, nonwritable executable:

```sh
sudo install -D -m 0755 disk_quota_guest.py /usr/local/lib/cowork/disk_quota_guest.py
sha256sum /usr/local/lib/cowork/disk_quota_guest.py
```

Prepare a **public, key-free** specification from the current real organisation,
instance, execution epoch, enrolled host binding and activated backend profile.
Use canonical nonnil UUIDs, fresh placement/filesystem UUIDs and real DMI machine
UUIDs. Its exact shape is:

```json
{
  "format": "cowork-total-disk-specification-v1",
  "organisation_id": "ACTUAL_ORGANISATION_UUID",
  "instance_id": "ACTUAL_INSTANCE_UUID",
  "execution_epoch": "ACTUAL_CURRENT_MANAGER_EPOCH_UUID",
  "executor_host_binding": {
    "version": 1,
    "host_id": "ACTUAL_HOST_UUID",
    "trust_domain": "ACTUAL_ORGANISATION_UUID",
    "daemon_id": "ACTUAL_DOCKER_INFO_ID",
    "operator_state_id": "ACTUAL_OPERATOR_STATE_UUID"
  },
  "backend_profile_revision": 1,
  "backend_specification_sha256": "ACTUAL_ORIGINAL_BACKEND_SPEC_SHA256",
  "guest_helper_sha256": "ACTUAL_INSTALLED_HELPER_SHA256",
  "controller_helper_sha256": "ACTUAL_INSTALLED_HELPER_SHA256",
  "environment_pool_bytes": 1073741824,
  "placements": [
    {
      "id": "NEW_GUEST_PLACEMENT_UUID",
      "kind": "guest",
      "instance_id": "ACTUAL_INSTANCE_UUID",
      "organisation_id": "ACTUAL_ORGANISATION_UUID",
      "machine_id": "ACTUAL_OWNED_VM_DMI_UUID",
      "daemon_id": "ACTUAL_DOCKER_INFO_ID",
      "filesystem_uuid": "NEW_GUEST_FILESYSTEM_UUID",
      "size_bytes": 68719476736
    },
    {
      "id": "NEW_CONTROLLER_PLACEMENT_UUID",
      "kind": "controller",
      "instance_id": "ACTUAL_INSTANCE_UUID",
      "organisation_id": "ACTUAL_ORGANISATION_UUID",
      "machine_id": "ACTUAL_CONTROLLER_DMI_UUID",
      "filesystem_uuid": "NEW_CONTROLLER_FILESYSTEM_UUID",
      "size_bytes": 17179869184
    }
  ],
  "topology": {
    "database": {"dedicated": true, "host": "postgres:5432"},
    "objects": {"dedicated": true, "endpoint": "http://minio:9000"},
    "workspace": {"docker_root_bounded": true},
    "controller_paths": {
      "state": "/var/lib/cowork-disk-quotas/CONTROLLER_UUID_WITHOUT_HYPHENS/mount/state",
      "backups": "/var/lib/cowork-disk-quotas/CONTROLLER_UUID_WITHOUT_HYPHENS/mount/backups",
      "scratch": "/var/lib/cowork-disk-quotas/CONTROLLER_UUID_WITHOUT_HYPHENS/mount/scratch",
      "snapshot_store": "/var/lib/cowork-disk-quotas/CONTROLLER_UUID_WITHOUT_HYPHENS/mount/unused-snapshots"
    }
  }
}
```

Placeholders are not usable identifiers or proofs. The VM is the actual newly
owned VM from the original backend/controller; protected/adopted baselines are
not migration targets. For VM snapshots add a separate `kind: "snapshot"`
placement, its original controller machine/filesystem UUID and the full
`vm_snapshot.maximum_bytes`. Set `controller_paths.snapshot_store` to that
placement's exact `/var/lib/cowork-disk-quotas/<compact-id>/mount`, matching the
snapshot configuration. Environment workspace backing images on the qualified
VM are bound to the guest disk; their loop/mount/volume/inode originals remain
those of the environment helper. External controllers whose storage is outside
these measured disks require their own physical adapter and remain unqualified
for this total-disk claim.

The sum of all placement maxima must fit the current instance, organisation and
platform storage policy. Four-service legacy capacity plans keep their original
bytes. The qualified plan reserves two additional real services, `postgres` and
`minio`; enroll the corresponding six-service CPU/RAM plan on the host. Both data
images must be digest pinned. Use the reviewed PostgreSQL image with UID/GID 999
and PGDATA `/var/lib/postgresql/data`, and MinIO with UID/GID 1000 and `/data`.
The dedicated internal network has no external route. Controller dump/restore
and administrative S3 use the original VM's pinned SSH/mTLS loopback transport;
configure the existing private `database_admin_url_file`, `s3_admin_alias` and
`guest_runtime_endpoints` to reach these same dedicated services, not the old
shared services. Keep these credentials out of the public specification.

The operator config used for initial bootstrap has `disk_quota_installation`
containing only `postgres_image`, `minio_image`,
`postgres_root_password_file`, `minio_root_user_file`,
`minio_root_password_file`. The credential files are private owner-only files;
bootstrap preserves their bytes and writes separate read-only runtime mounts in
the original private state directory. No key appears in an HTTP receipt.

With the current administrator session, original scoped provisioner credential,
private CA and reviewed config, run the explicit sequence (all outputs are new
private files; never overwrite a receipt):

```sh
python3 scripts/enterprise-disk-quota.py prepare --manager-url https://CONTROL_ORIGIN \
  --manager-ca /private/control-ca.pem --session-file /private/current-admin-access \
  --organisation-id ACTUAL_ORG --instance-id ACTUAL_INSTANCE --instance-revision ACTUAL_REV \
  --profile-revision 0 --request-id ORIGINAL_REQUEST_UUID --specification-file /private/disk-spec.json \
  --output /private/original-disk-plan.json

python3 scripts/enterprise-disk-quota.py install --manager-url https://CONTROL_ORIGIN \
  --manager-ca /private/control-ca.pem --operator-token-file /private/scoped-provisioner-token \
  --config /private/operator.json --plan-id ACTUAL_PLAN_ID --journal-directory /private/disk-install-journal \
  --guest-helper /usr/local/lib/cowork/disk_quota_guest.py \
  --controller-helper /usr/local/lib/cowork/disk_quota_guest.py --output /private/disk-installed.json

python3 scripts/enterprise-disk-quota.py bootstrap --manager-url https://CONTROL_ORIGIN \
  --manager-ca /private/control-ca.pem --operator-token-file /private/scoped-provisioner-token \
  --config /private/operator.json --plan-id ACTUAL_PLAN_ID --journal-directory /private/disk-install-journal \
  --guest-helper /usr/local/lib/cowork/disk_quota_guest.py \
  --controller-helper /usr/local/lib/cowork/disk_quota_guest.py \
  --installation-file /private/disk-installed.json --output /private/disk-bootstrap.json

python3 scripts/enterprise-disk-quota.py observe --manager-url https://CONTROL_ORIGIN \
  --manager-ca /private/control-ca.pem --operator-token-file /private/scoped-provisioner-token \
  --config /private/operator.json --plan-id ACTUAL_PLAN_ID \
  --guest-helper /usr/local/lib/cowork/disk_quota_guest.py \
  --controller-helper /usr/local/lib/cowork/disk_quota_guest.py \
  --installation-file /private/disk-installed.json --bootstrap-file /private/disk-bootstrap.json \
  --output /private/disk-observed.json

python3 scripts/enterprise-disk-quota.py commit --manager-url https://CONTROL_ORIGIN \
  --manager-ca /private/control-ca.pem --session-file /private/current-admin-access \
  --organisation-id ACTUAL_ORG --instance-id ACTUAL_INSTANCE --plan-id ACTUAL_PLAN_ID \
  --output /private/original-disk-commit.json
```

`install` refuses an existing Docker container, volume or image inventory. It
never copies an old live DB or Docker root. Fallocate/mkfs/mount and each data
Create have durable dispatch journals; unknown ACKs need original reconciliation
or retirement, not a new UUID, name lookup or resend. `observe` is read-only.
The observation must be freshly committed within 20 seconds. A lost POST is
recovered with `original --request-id <original>`; the delivered UI also performs
this GET before any same-body replay. A fresh read-only qualification plan can
use the unchanged installation/bootstrap originals after an old observation
expired; original allocation/operation/receipt bytes remain intact.

After commit, read the actual profile using `inspect`. Extract its `.profile`
unchanged into a new private profile file. Extract the controller requests from
`disk-installed.json` and the guest request from `disk-bootstrap.json`:

```sh
umask 077
jq '.requests | map(select(.placement.kind != "guest"))' /private/disk-installed.json > /private/controller-disk-requests.json
jq '[.guest_request]' /private/disk-bootstrap.json > /private/guest-disk-requests.json
```

Add the explicit operator setting (all filenames are operator-owned, never
supplied by a user operation) and change `state_directory`/`backup_directory` to
the exact bounded controller paths:

```json
{
  "instance_disk_quotas": {
    "version": 1,
    "profile_file": "/private/actual-committed-disk-profile.json",
    "guest_helper": "/usr/local/lib/cowork/disk_quota_guest.py",
    "controller_helper": "/usr/local/lib/cowork/disk_quota_guest.py",
    "controller_request_file": "/private/controller-disk-requests.json",
    "guest_request_file": "/private/guest-disk-requests.json",
    "postgres_image": "REVIEWED_POSTGRES_IMAGE@sha256:ACTUAL_DIGEST",
    "minio_image": "REVIEWED_MINIO_IMAGE@sha256:ACTUAL_DIGEST",
    "postgres_root_password_file": "/private/original-pg-root",
    "minio_root_user_file": "/private/original-s3-root-user",
    "minio_root_password_file": "/private/original-s3-root"
  }
}
```

Install persistent original disk/loop mounts and the environment-root bind mount
in the operator's boot configuration; Docker must depend on their real mount
units (`RequiresMountsFor=`). Do not start Docker with an unmounted fallback
folder. After a reboot or namespace/device change, perform original readback and
rerender the retained application manifest before restarting it: runtime device,
filesystem-ID and capacity pins deliberately fail closed on stale mounts.
The provisioner renews a fresh single-use manager observation challenge every
10 seconds; qualification expires after 30 seconds. API writes, worker work,
object PUT and desktop start also need current management quota authority and an
actual runtime statvfs check. Concurrent/background DB writes remain bounded by
the hard filesystem; ENOSPC is not a successful receipt or a release.

Normal application stop keeps its dedicated data services for verified backup,
restore and cleanup, as the old external services did. Complete VM snapshots
still stop **all** original producers and require genuine terminal/PID0 evidence;
only the retained exact data-service IDs may subsequently start. Permanent
instance deletion verifies its original DB/S3 objects gone before recording
positive data-service stop evidence and removing those exact containers. Lost
rm ACK recovery uses that stored original stop, never absence alone. It retains
the physical disk reservation. Disable the disk profile explicitly with a new
current administrator CAS plan; to free the disk, prepare purpose `release`, run
`retire` with the unchanged installation/bootstrap originals after all owned
resources and live filesystem handles are positively quiescent, then capture
and commit its original teardown observation. The helper performs actual
unmount, loop detach and original inode unlink; unknown teardown stays held.

**Deutsch:** In der Instanzverwaltung steht „Gesamte physische Diskquote“ bereit.
Die Oberfläche prüft die aktuelle Anmeldung und zeigt die tatsächlichen
Reservationen. Eine öffentliche Originalspezifikation wird ausdrücklich
hochgeladen; vorbereiten und übernehmen sind getrennte Schritte. Server-URL,
aktueller Benutzer, Organisation und Instanz binden den gespeicherten
Vorgangsbezug. Lokal werden nur UUIDs und Hashes gespeichert. Deaktivieren gibt
keinen Speicher frei; Freigeben benötigt den positiven Original-Abbaubeleg.
Altdaten, geteilte Dienste und nicht gemessene Cloud-/CSI-/externe Controller
werden nicht automatisch übernommen. Die installierbare Quelle und ihre engen
Compile-/DDL-Prüfungen sind kein Nachweis einer realen VM-/TLS-/ENOSPC-/Crash-
oder Wiederherstellungsabnahme auf dem späteren Betreiberhost.
