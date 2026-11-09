# Initial instance versions

Initial version selection is installed and enabled in the local Docker Desktop
control plane at schema 49. Existing tenants and default provisioning images are unchanged.
It qualifies Compose/container provisioning, not Kubernetes, VM provisioning or
the complete enterprise release.

## Configuration and public contract

After backing up and upgrading all management replicas and the operator together,
enable `COWORK_MANAGEMENT_RELEASE_SELECTION_ENABLED=true` in management and
`release_selection_enabled: true` in the operator's private JSON configuration.
The local entrypoint accepts `COWORK_RELEASE_SELECTION_ENABLED=true` for that
operator setting. Both environment switches default to false in local Compose.
Do not enable a new operator contract against an older management API.

The local installation persists both enabled switches in the private
`control-plane-features.env`. `Start-EnterpriseLocal.ps1` accepts only those two
keys, requires matching values and rejects conflicting caller environment values
before initializing secrets or starting Docker. Installed image pins, database
URLs and credentials are preserved across startup.

Publish immutable manifests through the existing operator catalog publisher and
keep catalog synchronization enabled. The authenticated organisation endpoint
`GET /management/v1/organisations/{org}/provisioning-releases` returns only approved
release IDs and names. Audit-only members cannot use this creation catalog. The
organisation listing advertises `release_selection_enabled` for compatible UIs.

`POST /management/v1/organisations/{org}/instances` accepts optional `release`, a
64-character lowercase SHA-256 image-set ID. This is fixed in the instance's
`provisioning_release` column. Creating an empty instance does not require the
source-specific canary needed to update existing data. It also does not create a
canary qualification. Source transitions and instance updates retain their gates.

When requesting provision, omit the release argument: management copies the frozen
initial selection into the operation. The operator must advertise
`provisioning_release_version: 1`. The database also requires this dispatch contract,
so an older API process cannot silently claim such a queued operation. Unknown,
withdrawn or stale (five-minute synchronization window) releases fail closed at
creation, queuing and dispatch. Neither the API nor the operator falls back to its
default image set for an explicit selection. An omitted selection preserves the
legacy default-image path.

## Persistence and recovery

Before creating database credentials, buckets or containers, the operator validates
the selected manifest and persists its exact six images in the active generation.
Conflicting existing pins or Compose manifests require review. Successful receipts
identify the operation, instance and selected image set. Management updates the
installed release and lifecycle state in the same transaction as completion/audit.
Changing operator defaults or restarting a process cannot replace the selection.

Catalog withdrawal prevents new provisioning; it does not invalidate proof of work
already completed, delete an installation, or prevent explicit stopping recovery.
Interrupted work retains its original lease and is never automatically provisioned
again. Recovery verifies that pins and deployed images still match before reporting
a startable stopped state. An installed instance can restart its recorded images
after withdrawal. To change installed versions, use the separate update workflow.

## Validation

- `test_provisioning_release.py`: durable pins, default changes, withdrawn/missing
  entries, binding/version errors, contradictory manifests, exact receipts and
  stopping recovery. The infrastructure boundary is mocked in these unit tests.
- `test-enterprise-management.py --provisioning-releases`: disposable PostgreSQL
  and real HTTP, including cross-organisation/auditor rejection, database trigger
  enforcement, disabled feature, freshness/withdrawal, old operator rejection,
  atomic completion, duplicate receipts and legacy compatibility. Infrastructure
  receipts in this suite are simulated.
- `test-enterprise-execution-docker.py --provisioning-releases --server-image ...
  --provisioner-image ...`: a dedicated Docker Desktop installation with two real
  tenant instances, six lifecycle operations and verified deletion. It checks exact
  selected/default images, re-delivery without recreated containers and restart
  after withdrawal/default changes. The two distinct server packages contain the
  same binary; this is image-selection acceptance, not cross-version migration.
  Reconstructed operator objects exercise durable receipt recovery, not an actual
  process kill or laptop disconnect.
- Management component/client tests and
  `app/scripts/test-enterprise-provisioning-release-ui.mjs`: desktop/mobile
  rendering, required selection, unavailable catalog, auditor access, old servers
  and emitted request bodies. Browser API responses are mocked.
- Rust tests, TypeScript checking, generated OpenAPI/schema checks, Rust-to-Zod
  wire validation and the web production build remain required.

Local evidence is recorded under `.internal/enterprise-local/provisioning-release-*`.
No 40-task load, Kubernetes failure, VM lifecycle or offline laptop acceptance is
implied by these checks.

## Installed local control plane

The schema-48 to schema-49 cutover completed on 2026-09-28 after stopping all four
control services and rehearsing a fresh encrypted management backup. It restored
that backup into `cowork_management_release_2f4a709064bb` and migrated the copy;
all original columns of all 80 source tables matched. The schema-48 database
`cowork_management_release_0336b889de45` remains an unchanged maintenance checkpoint,
not a current replica. Do not switch back after new writes without reconciliation.
The backup covers control metadata, operator state and configuration/secrets;
it does not include tenant databases, object storage or VM disks.

The four pinned control packages and all 44 served web files match the tested
build. All 35 other running containers retained their IDs/images, including the
seven ready tenant instances. Private operator callbacks use authenticated TLS;
public runner routes remain inaccessible and the creation catalog requires login.
Central admission and enrollment remain disabled. No live user instance was
created or upgraded as part of this deployment.

Deployment evidence is in
`.internal/enterprise-local/provisioning-release-control-cutover-results.json`,
with its fresh backup report and one-shot maintenance journal. The earlier
`provisioning-release-results.json` remains the historical pre-deployment test
report. `installed-control-plane.json` identifies the selected database, images
and persisted feature settings. Startup fixture tests also reject unknown,
duplicate, unpaired and conflicting feature settings.
