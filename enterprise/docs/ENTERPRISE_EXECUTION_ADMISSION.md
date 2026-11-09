# Central execution admission candidate

This implementation is behind explicit feature flags. Management, provisioner and
web packages are installed in the local control plane at schema 49, but admission
and enrollment are **not enabled in the Docker Desktop installation or default
deployment templates**. This does not yet qualify fleet-wide production quotas.
Existing tenant instances retain their installed images.

## Implemented accounting

Migration `0044_execution_admission.sql` adds central reservations and a separate
instance-local completion outbox. Before a worker or external executor receives a
run lease, its trusted instance service requests one central slot. This covers
model-only runs as well as sandbox/device work; individual tool calls do not reserve
additional run slots. Waiting for input/approval keeps the run's slot occupied.

A PostgreSQL transaction serialises admission across management processes and
organisations. It checks the platform ceiling, organisation budget, per-user budget
within that organisation, and the effective instance policy. Active membership and
instance execution rights are checked under the organisation lock. A user with
memberships in several organisations has a separate budget in each organisation.
An owner or explicitly assigned manager/member can reserve; a viewer, auditor or
support-only administrator cannot acquire execution rights through this endpoint.

Reservations bind organisation, instance, execution epoch, run, user and target.
An identical retry returns the existing reservation without counting twice.
Changing the user/target conflicts; a released reservation cannot reopen. No
timeout, expired lease, stopped/deleted instance, revoked user or management restart
forgives an outstanding reservation. Administrative capacity reads expose metadata
only, return at most 100 outstanding records and retain the full outstanding count.
They do not create an organisation budget as a side effect.

An authenticated completion must match the exact reservation and confirm stopped
execution. Identical receipts are idempotent; altered receipts conflict. A matching
old completion can settle after membership revocation or an epoch change without
granting new work. The management API stores no prompts, outputs or file contents
in these records. Reservations, completions and budget edits join the organisation
audit chain.

For external executors, a valid leased completion/failure report persists the
outbox entry in the same transaction as the terminal run state. For server workers,
successful execution and desktop cleanup must both finish before the completion
receipt is persisted atomically with the run result. The relay retries unacknowledged
receipts after management outages and instance restarts. Uncertain errors,
cancellation, worker death, lease expiry and failed cleanup retain capacity.
The explicit Compose recovery below can settle selected server reservations after
verified instance shutdown; device/VM reservations still require executor evidence.

## Candidate configuration and interfaces

### Legacy enrollment preflight

`server/cowork-provisioner/execution_enrollment.py` provides an operator-only
observational check before an explicit enrollment operation. It requires
the existing operator lock and an explicit instance, organisation and backup UUID.
Run it in an operator maintenance session with the provisioner daemon inactive;
do not remove or bypass the operator lock:

```sh
python3 /opt/cowork-provisioner/execution_enrollment.py \
  --config /operator/config.json \
  --instance-id INSTANCE_UUID --organisation-id ORGANISATION_UUID --backup-id BACKUP_UUID
```

The operator configuration must explicitly allow the installed immutable server
image in `execution_admission.server_images`. API, worker and retained image pins
must agree. The isolated compatibility probe runs without a network, credentials
or host mounts; it does not create an epoch or start the instance application.

The check validates scoped ownership of current and retained resources, inspects
every retained database with server-enforced read-only transactions and bounded
statement/lock timeouts, and checks unfinished runs, executor counters, interactive
sessions, enabled schedules and unsettled admission receipts. All legacy assignment
history requires separate reconciliation, even completed/expired records: historical
state alone does not prove an offline executor has stopped. The report includes
counts only, with a separate count of explicitly external assignments.

It rejects unfinished local operation journals, unknown ownership, missing
resources, changed inventories and active/unknown container states. The selected
encrypted backup must belong to this instance and organisation, match the installed
image set and pass archive-size, ciphertext-digest, decryption and every member's
manifest/hash checks. Plaintext exists only in a private temporary directory which
is removed on success or failure. Runtime manifests, keys, data, containers,
management metadata and reservations are not changed by this check.

Exit zero and `checks_passed: true` mean only that these observations passed.
The report always contains `eligible_for_enrollment: false`; it is neither a signed
enrollment receipt nor accepted by the activation gate. It does not establish
backup freshness relative to current data, revoke device grants, remove retained
containers or perform a compatible release upgrade. The separate transaction
below repeats the checks and captures a fresh backup; the CLI report cannot
substitute for its receipt. This tool alone does not make it safe to turn on
admission for the live legacy fleet.

The dedicated real Docker acceptance is:

```sh
python scripts/test-enterprise-execution-docker.py --enrollment-preflight \
  --provisioner-image cowork-provisioner:execution-enrollment-preflight-candidate
```

It creates two new legacy-mode instances using the current candidate with admission
disabled. It checks actual encrypted backup verification, restored/retained database
generations, a retained connection, expired and ambiguous historical assignments,
database-enforced read-only inspection, corrupt/foreign backups, running containers,
operator-lock contention and the standalone CLI. It deletes its own instances and
test installation afterwards. This is not a cross-version migration qualification.
Evidence is stored in
`.internal/enterprise-local/execution-enrollment-preflight-docker-results.json`;
candidate and unit-test metadata are in
`.internal/enterprise-local/execution-enrollment-preflight-results.json`.

### Explicit Compose enrollment

Migration `0048_execution_enrollment.sql` and `instance_enrollment.py` implement
an opt-in `enroll_execution` operation for stopped, unbound Compose container
instances. The management/provisioner/web packages are deployed to the local
Docker Desktop control plane. Enrollment is still explicitly disabled there;
deploying these packages does not enroll or upgrade existing tenant instances.

Management must explicitly set
`COWORK_MANAGEMENT_EXECUTION_ENROLLMENT_ENABLED=true`. The provisioner must have
`execution_enrollment_enabled: true` and an operator-owned
`execution_admission.server_images` allowlist containing the installed, qualified
immutable image. The local entrypoint accepts these through
`COWORK_EXECUTION_ENROLLMENT_ENABLED=true` and
`COWORK_EXECUTION_ADMISSION_CONFIG_FILE`; both remain disabled in the default
deployment. Claims require the separate `execution_enrollment_version: 1` and
`execution_admission_version: 1` contracts. A legacy operator cannot claim this
work by advertising only the admission contract.

After separately qualifying any necessary source/target release upgrade, an
administrator stops the instance and selects **Enroll in central limits**. The
review requires `acknowledge_execution_enrollment: true`; no existing backup can
be selected instead of the fresh backup. Authority is checked again at dispatch.
The trusted operator verifies ownership and quiescence of every retained resource
generation, captures and fully verifies an encrypted backup bound to this
operation, and removes stopped service containers before changing runtime
configuration. Databases, files, workspaces and keys remain in place. Unknown
containers, active database connections and runnable/unresolved work block enrollment.

Historical local-worker assignments and enabled schedules can pass only with no
registered external executors, external assignment history or unresolved execution.
Stopped server-only instances may also retain `interrupted` runs with no lease,
and unstarted `waiting_for_snapshot` runs with neither a lease nor an assigned
executor. Every other nonterminal run still blocks enrollment, as do outstanding
reservations and interactive sessions. All current and retained resource generations
must meet these conditions. Enrollment neither changes task states nor queues
interrupted work. A later explicit user transition or validated snapshot upload
uses the normal claim path and its central reservation. The read-only CLI remains
conservative and reports these rows for review; its output never authorizes enrollment.
Even an idle or revoked registered device requires a separate, still-pending
executor migration procedure. Expired assignments do not prove an offline device
has stopped, and enrollment neither clears them nor releases reserved capacity.

A private, frozen journal binds configuration, installed images, the active
resource generation and the verified backup. Writing the immutable execution
epoch is the commit point. Afterwards, failures remain in reconciliation; they
cannot restore an unbound legacy runtime. An administrator can select **Review
interrupted operation** and explicitly authorize `continue_enrollment` for one
further attempt with the same journal, epoch and backup. Nothing is automatically
re-executed after a lost dispatch; an already verified receipt can be delivered
again without repeating infrastructure work. Before the commit point, only proof
that the original runtime configuration is unchanged permits an ordinary failed
operation and another later enrollment request.

The completion transaction atomically records the backup, bound runtime
attestation, immutable enrollment receipt, audit event and
`execution_required=true`. Missing/substituted evidence or an audit failure rolls
back the whole completion. Enrollment leaves the instance stopped. Start/resume
remain unavailable until every relevant instance is covered and the platform
activation gate succeeds; selecting enrollment does not activate the platform.

The dedicated acceptance creates two disposable legacy-mode instances using the
current candidate binary, including a retained restored generation:

```sh
python scripts/test-enterprise-execution-docker.py --enrollment-lifecycle \
  --server-image cowork-server:execution-enrollment-candidate \
  --provisioner-image cowork-provisioner:execution-enrollment-candidate
```

It covers real backup verification, unchanged rejection of an unreconciled
executor, interruption after the commit point, explicit recovery without replay,
durable activation, startup and one actual worker task through central admission
and its completion outbox. The model endpoint is controlled test infrastructure.
Success deletes only the generated instances and test installation. The report
is `.internal/enterprise-local/execution-enrollment-docker-results.json`; package
and test metadata are in `execution-enrollment-results.json` beside it.
This is **not cross-version qualification from the previous migration-43 release**,
device/VM enrollment, Kubernetes acceptance or the 40-task mixed workload.

### Upgrade from the installed release

The separate cross-version acceptance starts the exact previously installed migration-43
server **and provisioner images** in a new installation. They create two instances,
complete real model tasks, retain their project/chat/message/run records, and
create encrypted instance backups. The stopped management database is also
encrypted, restored into a separate database and started with the original server
image. Existing authentication and the restored instance list must work before
the original test database is migrated by the candidate management service.

After checking unchanged organisation, operation and audit data, the candidate
operator performs a real canary against a cloned instance and verifies a return
to the unchanged source. Only then does it promote both instances, preserving
their application records and storage keys. Both enroll with their original
operation journals and retained migration-43 database generations. Following
activation, replacement workers complete new tasks through admission and the
completion outbox, without replaying the completed source tasks. The model
provider is a controlled private HTTP endpoint. This supplements, rather than
replaces, the interruption and isolation tests above.

```sh
python scripts/test-enterprise-execution-docker.py --enrollment-lifecycle \
  --cross-version-source-image cowork-server:local-review \
  --source-provisioner-image cowork-provisioner:local-review \
  --server-image cowork-server:execution-enrollment-candidate \
  --provisioner-image cowork-provisioner:execution-enrollment-candidate
```

Tags are resolved to immutable digests before test resources are created, and
the source and target server binaries must differ. Qualification applies only
to the recorded pair, not whichever images acquire those tags later. The tested
source server is `sha256:d31e9bfefd1345a33d7739b68edfa03c28ed19af39a2a80f7b7ae9369a0daf32`;
the target is `sha256:3350567c2e7c55c794cce8ac3eb619e94ac1e264e6735199e2efbb5997283aba`.
Evidence, operator digests and test boundaries are recorded in
`.internal/enterprise-local/execution-cross-version-docker-results.json` and
`execution-cross-version-results.json`. Test sources are frozen in a local
temporary directory to avoid Docker Desktop/Nextcloud directory-read failures.

This qualifies the image transition with generated representative data. It has
not migrated the seven live instances or inspected their complete executor history.
The management backup test covers database restore using the fixture's retained
configuration and keys; it does not claim a complete fresh-host disaster recovery.
Registered devices, running/offline work, VM/Kubernetes backends and arbitrary
source versions remain outside this acceptance.

The read-only local inventory on 2026-09-27 found that management is at migration
43 while the seven tenant instances are pinned to migrations 32, 33 and 38 across
four server image digests. Their five exact source-image combinations have since
passed the tenant qualification below. Across 36 current/retained databases, 55 run records are
not terminal; this includes historical copies, not 55 currently executing tasks.
The active generations contain four `interrupted` records and one
`waiting_for_snapshot` record. No registered executors or external assignments
were found. These observations are not quiescence proof or enrollment permission;
no task states were changed. Private counts and image mappings are recorded in
`.internal/enterprise-local/execution-live-inventory-observations.json`.

### Tenant image combinations and dormant work

The tenant variant separates the installed management release from the exact six
images pinned by an older instance. Supply a JSON object containing `server`,
`runner`, `sandbox_core`, `sandbox_gui`, `sandbox_crew` and `egress_proxy`, each
with its immutable repository digest. `--source-instance-migration` checks the
actual database schema before any upgrade:

```sh
python scripts/test-enterprise-execution-docker.py --enrollment-lifecycle \
  --cross-version-source-image cowork-server:local-review \
  --source-provisioner-image cowork-provisioner:local-review \
  --source-instance-images /path/to/source-images.json --source-instance-migration 32 \
  --server-image cowork-server:execution-enrollment-candidate \
  --provisioner-image cowork-provisioner:execution-legacy-upgrade-candidate
```

The target uses the candidate server and the explicitly resolved target runner/
sandbox/proxy image set. Results are named
`execution-tenant-<source-image-set-prefix>-docker-results.json` in the private
evidence directory. Image-set identity includes all six images; a common schema
number or server binary does not qualify a different runner combination.

This variant adds a real source shell task and persistent workspace file, an
encrypted attachment created through the source API, and new centrally admitted
shell work after migration. It checks the old file through the restored workspace
and decrypts the attachment through the upgraded authenticated API. Historical
interrupted and snapshot-wait states are deliberately seeded as SQL test data;
they are not represented as real crash/offline events. The canary leaves the source
untouched; promotion preserves their payloads while the existing restore fence
marks unfinished candidate work interrupted. Enrollment leaves those tasks
unchanged. A deliberately queued fixture must still reject enrollment without
rewriting the runtime. Only the newly requested model/shell tasks may execute.
All retained source generations and journals participate in the enrollment check.

Long-running tests renew their own management session through the ordinary
refresh endpoint. The restored-backup server has a separate refresh-token chain;
renewing it cannot rotate the source database's session. The upgraded server must
also accept the original session's refresh token and preserve its user/session
identity. No access-token lifetime or second-factor verification window is
extended by the harness. Fixture SQL inspection connections are allowed to close
before lifecycle operations; the provisioner's quiescence check is not bypassed.

All five image combinations observed in the seven live instances passed this
acceptance with two freshly generated instances per combination:

| Source image-set prefix | Source schema | Server digest prefix | Runner digest prefix |
| --- | --- | --- | --- |
| `0e28c11491dd` | 32 | `97838d49d594` | `316499a24b5e` |
| `761c6711fab2` | 33 | `3d4622db9dd0` | `e5bd547ea53c` |
| `dced4302afe0` | 33 | `3d4622db9dd0` | `6ca94347b8d6` |
| `6fbb704208b0` | 33 | `514ac1583049` | `6ca94347b8d6` |
| `0c720cef95ef` | 38 | `c7dddf4aca20` | `e5bd547ea53c` |

The exact six-image maps, frozen harness hashes and individual reports are
indexed by `.internal/enterprise-local/execution-legacy-upgrade-results.json`.
These runs total 56 checks, 80 lifecycle operations, 20 controlled model requests
and 20 real shell tasks, accumulated across separate cases. Every successful
fixture and both discarded diagnostic fixtures were removed. The last case also
checks refresh of the original management identity after migration; the earlier
four retain their own frozen harness versions and did not run that added check.

All cases target server
`sha256:3350567c2e7c55c794cce8ac3eb619e94ac1e264e6735199e2efbb5997283aba`
and provisioner
`sha256:f2739694a6d36f0c1dea869a84c26d73fe1fd3cdc94bb34cb19ed45e4db8ac9d`.
The separately validated web package is
`sha256:29c261f6908c49849dfe49f36534c76b9fe2784374d121e17bad6a9a8c62ae91`;
its browser tests use mocked API responses. These packages were subsequently
installed in the local control plane using the maintenance procedure below.
Tenant image qualification with generated data does not replace a fresh live
instance backup, per-instance review or the activation gate.

The read-only local inventory also confirms that the 55 historical nonterminal
records consist of 50 interrupted and five unstarted snapshot waits, all matching
the preservation predicate. This observation does not prove the live infrastructure
is stopped, validate a fresh backup, or perform enrollment. Real device/offline
reconciliation, arbitrary image combinations and the simultaneous 40-task workload
remain separate requirements.

### Rehearsal with the existing control-plane database

`scripts/prepare-enterprise-control-upgrade.py` additionally captures the local
management database and rehearses the installed-to-candidate transition on that
actual data. It requires explicit `--capture-local` and immutable image references:

```sh
python scripts/prepare-enterprise-control-upgrade.py --capture-local \
  --source-server-image cowork-server@sha256:d31e9bfefd1345a33d7739b68edfa03c28ed19af39a2a80f7b7ae9369a0daf32 \
  --target-server-image cowork-server@sha256:3350567c2e7c55c794cce8ac3eb619e94ac1e264e6735199e2efbb5997283aba \
  --operator-image cowork-provisioner@sha256:f2739694a6d36f0c1dea869a84c26d73fe1fd3cdc94bb34cb19ed45e4db8ac9d
```

Source database access is read-only. The encrypted archive includes the
transactionally consistent management dump, selected control/operator secrets,
actual container inspections and a supplemental operator-state archive. It is
encrypted to the existing backup identity; that decryption key stays separate.
The rehearsal restores the **decrypted archive member**, starts the installed
server against it, then starts the candidate in the same isolated test network.
It compares row counts and SHA-256 fingerprints of every original table's original
columns, including the previous migration records. New admission activation must
remain absent. The test APIs have no published ports, fleet networks or Docker
socket. Generated test containers/network and temporary plaintext are removed.

The recorded run preserved all 75 original tables through schema 43 to 48 and
left the live service container IDs, images and schema unchanged. Its encrypted
backup and four-check report are in the private
`cowork-control-rehearsal-3de5c5e5a732` evidence directory, referenced by
`execution-legacy-upgrade-results.json`.

This is a point-in-time rehearsal while source services remain running. The
operator-state copy is not a quiesced fleet snapshot; tenant databases, objects,
workspaces and VM disks are not backed up by this command. Current Compose source
may differ from the installed release, so the archived runtime inspections are
separate evidence, not an automatic backout manifest. Live cutover still requires
a fresh consistent maintenance backup and a deployment/backout procedure. Login,
MFA recovery and full fresh-host restoration are outside this rehearsal.

### First local control-plane update (schema 43 to 48)

The control plane has now moved to the qualified schema-48 server, provisioner
and web images listed above. Before the cutover, management API/web and the
operator were stopped, with no active management operations or other clients on
the management database. The rehearsal was repeated with
`--stopped-control-plane`; it verified the new maintenance backup and left those
source services stopped. This flag itself never stops or restarts source services.

The decrypted backup was then restored into the separate database
`cowork_management_release_0336b889de45` on the existing management PostgreSQL
service. All 75 original tables matched before and after migration. The source
`cowork_management` database remains at schema 43 and its original fingerprints
remain unchanged. Only the management database URL and four pinned control images
were selected for the new runtime. Other running containers, including all seven
tenants, retained their container IDs/images throughout. Admission activation and
legacy enrollment remain disabled.

The operator's authenticated `/pending` request succeeds over the private TLS
gateway in its independent namespace. The management web was reopened only after
that check, schema/data checks, seven ready instances and zero active operations.
The new web package is served at the existing local URL. An existing browser
administrator identity reaches the second-factor gate; its factor is not enrolled
by the deployment tooling.

Deployment evidence and maintenance-backup references are in
`.internal/enterprise-local/control-plane-cutover-results.json`. The private
`control-cutover-0336b889de45` directory contains the frozen old Compose profile,
original runtime inspections and a one-shot phase journal. The source database
and old image digests remain available for investigation/recovery. They are a
maintenance checkpoint, **not a current replica**: after reopening management,
new users, factors, policies and operations may exist only in the new database.
Do not silently switch back after new writes or replay the completed cutover.

`Start-EnterpriseLocal.ps1` validates and consumes the private
`control-plane-images.json` override, fixes the local Compose project name and
preserves existing database URL secrets. The five isolated PowerShell startup
checks verify that preservation and rejection of extra services, privileged
attributes, foreign images and extra root properties before secret initialization.
This does not upgrade/enroll tenant runtimes or qualify another backend.

### Subsequent local control-plane update (schema 48 to 49)

The local management database now selects `cowork_management_release_2f4a709064bb`
at schema 49, with initial release selection enabled. The four control services
were stopped for a fresh encrypted backup and restore/migration rehearsal. All
80 source tables were preserved; the schema-48 database remains unchanged as a
maintenance checkpoint, not a current replica. The seven tenants and all 35 other
running containers retained their IDs/images. Admission and enrollment remain
disabled and tenant/default images are unchanged.

The installed pointer includes a private, paired feature file. Startup validates
it and rejects conflicting caller environment values before secret initialization.
Eleven isolated PowerShell cases cover URL/image/feature preservation and invalid
manifests/settings. The live preparation check also preserved existing credentials.
Deployment evidence and limitations are documented in
[initial instance versions](ENTERPRISE_PROVISIONING_RELEASES.md#installed-local-control-plane).

### Published instance update package

The operator catalog now advertises **Enterprise: zentrale Aufgabenverwaltung**,
release `4a5e5f5cab8d667e27a854127a7d17bfe673c1373b81dd9adc297639f143528d`.
Its six immutable images match the five completed Docker qualifications above;
its supported-source list contains exactly their five image-set hashes. Image
digests are retained locally. Publication used the existing operator-only CLI
under the exclusive operator lock, with the daemon stopped and no active
management operations. The daemon resumed in the same container and synchronized
the exact manifest into the management catalog. No infrastructure socket or
network was mounted into the catalog-publication helper.

The per-installation canary gate is separate from those package compatibility
tests. No test-fixture qualification receipt was copied or invented: management
has zero qualification records for this release, and the operator rejects
installation for each of its five sources until an actual local canary completes.
After administrator second-factor verification, stop a selected instance and use
**Test and restore current version** before **Install tested version**. Every
instance operation still requires its normal authority and backup checks.

Publication changed neither tenant images nor the independently pinned default
images used when creating an instance. Existing new-instance default selection
is still an operator configuration concern; catalog publication does not silently
select the latest image. The 38 other running containers retained their IDs and
images throughout publication. Evidence and the reviewed manifest are in the
private `enterprise-execution-release-results.json` and
`enterprise-execution-release.json` files. Admission/enrollment remain disabled.

### Admission settings

The disposable acceptance harnesses supply these settings themselves. Do not add
them to a mixed-version live fleet as a shortcut to enforcing quotas.

| Process | Setting | Meaning |
| --- | --- | --- |
| Management | `COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED=true` | Enable the central candidate API; default is false |
| Management | `COWORK_MANAGEMENT_EXECUTION_CEILING_FILE` | Optional operator-owned JSON ceiling; defaults below |
| Instance API and worker | `COWORK_FLEET_EXECUTION_ADMISSION=true` | Require central admission; default is false |
| Instance API and worker | `COWORK_EXECUTION_EPOCH` | Current management-issued UUID for this runtime generation |
| Instance API and worker | Existing `COWORK_FLEET_KEY_FILE`, `COWORK_FLEET_MANAGEMENT_URL`, `COWORK_ORGANISATION_ID`, `COWORK_INSTANCE_ID` | Bind the authenticated callback to one instance |

Default ceiling:

```json
{
  "revision": 1,
  "platform_concurrent_runs": 40,
  "organisation_concurrent_runs": 40,
  "user_concurrent_runs": 2
}
```

Ceiling revisions are immutable in the database. Starting a replica with conflicting
settings at the same revision or an older revision fails. A new revision cannot
reduce capacity below existing reservations. Once a newer revision is registered,
old replicas reject new admission and budget edits. Completion delivery remains
available. Replaying an already-held reservation does not consume another slot.
Changing the organisation policy requires both quiescent infrastructure and no
outstanding execution reservations, including reservations of deleted instances.

`GET /management/v1/organisations/{org}/execution-capacity` is available to
organisation administrators and auditors; `PUT` requires administration and an
`expected_revision`. Budget revision zero means the operator defaults, with no
persisted organisation override. Positive limits must remain inside the ceiling
and cannot be lowered below outstanding reservations.

The private endpoints `POST /management/runner/v1/execution-reserve` and
`POST /management/runner/v1/execution-release` use an organisation/instance-bound
`x-cowork-instance-key`, not a user's bearer token. Disabled or unavailable
admission does not silently fall back when the instance explicitly requires it.
Contracts are shared through `cowork-contracts::enterprise_execution`, TypeScript
validation and the generated enterprise OpenAPI/JSON schemas.

## Lifecycle generation binding

Migration `0045_execution_generation_binding.sql` freezes a typed source/candidate
epoch binding when management dispatches an operation with admission enabled.
Pending/reconciling operations retain that exact binding. Queued, running and
reconciling lifecycle operations block new reservations even while the instance
still has its previous ready status. Existing completion receipts remain accepted.

The provisioner stores a scoped `execution.json` in each resource generation and
puts its epoch into both API and worker manifests. Restoration creates a fresh
candidate marker instead of restoring a historical execution identity from the
backup. Update/rollback checkpoints retain the source binding; journals bind the
lease, operation and execution transition together. A completion verifies that the
active marker and both generated manifests agree. Missing, malformed, mismatched
or substituted evidence stays in reconciliation. A previously saved exact receipt
is replayed without re-running infrastructure work.

Management validates that receipt before atomically updating the instance epoch,
operation result and audit chain. A successful restore/promotion/rollback uses the
new epoch; a canary or failed update retains its source epoch. Rollback to an older
release still gets a fresh execution identity. Explicit stopping recovery of an
interrupted restore can attest either the source or the already-switched candidate
pointer. Recovery of an interrupted update/rollback restores its recorded source.
Ordinary lifecycle transitions do not release outstanding run reservations. Delayed completed
operation receipts cannot switch the epoch again after a later transition.

This is implemented candidate lifecycle integration, **not live qualification**.
Bound Compose generations now require an operator-owned `execution_admission`
configuration with a `server_images` allowlist of exact repository digests. The
provisioner renders admission enabled in both API and worker, mounts their scoped
instance key, and checks both configurations before acknowledging completion.
Runner, egress proxy and workload containers never receive that key. The private
management callback must resolve on the trusted services network; the local
Compose profile now declares its management alias on that network as well.
Update probes can therefore remove gateway routing without losing the callback.

Before provision/start/restore/update/rollback starts an admitted runtime, an
isolated image probe invokes `cowork-server --execution-admission-contract`.
Unknown images, old binaries, missing opt-in, disabled service configuration and
unbound operations with operator admission enabled are rejected. The declaration
checks compatibility; it is not independent proof of an arbitrary image's safety
or an independent measurement of every running process. The operator allowlist
still requires qualification.
`local_entrypoint.py` accepts this configuration through
`COWORK_EXECUTION_ADMISSION_CONFIG_FILE`; no live configuration enables it yet.

New provisioned test instances can initialize their epoch marker. Existing unbound
instances require the explicit Compose enrollment above after qualification of
their installed release. Ordinary lifecycle operations cannot silently enroll
them. The installed source/target image pair now has separate cross-version
acceptance above. Per-instance readiness and executor history of the seven live
instances still require review before their rollout.

## Persisted activation and runtime coverage

Migration `0047_execution_coverage.sql` makes activation durable. Enabling admission
rejects every existing, non-deleted instance without enrollment, including stopped
instances. Failure rolls back both activation and ceiling registration. A fresh
empty installation can activate; subsequent instances automatically require
admission even when inserted by an already-running older management process.
The database also prevents clearing that requirement or dispatching an unbound
lifecycle operation. This is a managed-lifecycle coverage gate, not legacy enrollment.

Claim/pending requests for required instances need `execution_admission_version: 1`.
The provisioner opts in when its execution-admission configuration is present.
Its journal binds the runtime requirement; old operators cannot silently complete
these operations with generation metadata alone. Successful non-deletion receipts
include an operator attestation of the immutable server image and the digest of
the checked API/worker manifest. Both services must require admission, use the
scoped key/epoch, and pass the image compatibility probe. Management stores the
attestation and an audit event atomically with lifecycle completion. A required
instance cannot commit readiness with absent evidence or the wrong active epoch.
Deletion instead requires its scoped resource-absence proof.

Starting a disabled replica after admission was configured is rejected. An
already-running disabled replica, or one using an obsolete ceiling revision,
returns 503 from `/readyz` after configuration changes. Database dispatch/readiness
barriers remain effective even for callers that ignore load-balancer readiness.
Ordinary configuration cannot turn admission back off; rollback must retain the
compatible admission contract. Existing unbound instances still require an explicit
backup, qualification and enrollment workflow before activation is possible.

The Docker Desktop source profile now gives the provisioner its own network
namespace. Its private control channel uses `https://operator-control:8443` with
an explicitly trusted certificate; the gateway exposes no host port. A management
restart therefore does not detach the operator. This profile and the management/
provisioner/web packages are deployed locally; tenant admission remains disabled.

## Explicit recovery of server reservations

Migration `0046_execution_recovery.sql` adds a frozen recovery binding and records
which verified stop operation settled a reservation. Organisation administrators
can select up to 100 outstanding server reservations for a Compose container
instance and request `stop` with `reconcile_reservation_ids` and
`acknowledge_execution_stop: true`. This stops the **entire instance**, including
unselected server tasks. An ordinary stop does not release slots. Recovery also
works when an instance was already stopped, but still requires the full proof.
Offline devices, managed VMs and other executor pools cannot be settled this way.
Deletion is rejected while any execution reservation remains held.

The provisioner validates ownership of every retained resource generation, stops
workloads, removes stopped service containers, verifies their absence and checks
for active database connections across all retained generations. Databases, files,
workspace volumes and keys remain available for a later start. A durable, bound
receipt attests the inventory and stop. An unknown resource, active connection or
missing proof leaves the operation in reconciliation and keeps the slots occupied.
Further verification after an interruption requires explicit administrator recovery;
an already stored exact receipt can be replayed without executing the operation again.

Management rechecks the administrator's authority, matches the exact generation,
run and reservation selection, and commits slot release with the operation result
and audit events in one transaction. A completion racing with recovery does not
release twice. A later valid executor completion acknowledges the settled slot
without replacing its stop evidence; a released run ID cannot start again.

The browser/desktop management console exposes a Task capacity tab only when the
server advertises this candidate. Administrators can edit revision-bound limits,
inspect outstanding reservations and acknowledge a whole-instance stop. Auditors
have read-only access. Changed instance revisions or reservations invalidate an
open recovery review. Budget conflicts preserve the edited values for review.

## Activation work still required

- Qualified images and execution coverage across every backend. The implemented
  activation/dispatch gate requires enrollment; it does not enroll or inspect
  arbitrary out-of-band legacy runtimes by itself.
- Per-instance rollout readiness, verified backups and executor reconciliation
  for the live fleet. The installed management pair and all five observed tenant
  image combinations have cross-version acceptance with generated data; arbitrary
  source releases remain unqualified. VM/Kubernetes
  enrollment and lifecycle qualification remain open.
- Executor-specific recovery for devices/VMs and automatic run-scoped cleanup for
  canceled/failed work. The implemented Compose stop recovery is an explicit,
  disruptive administrator action; there is no timeout-based release.
- Fair queues. Claims inspect at
  most 25 compatible candidates with admission enabled; this does not guarantee
  fairness. Standalone server workers retain their previous single-row claim.
- Aggregate CPU, memory and storage reservations across instances; monetary model
  budgets and unknown-cost accounting. Run counts do not implement these limits.
- Protocol-v3 offline completion/reconciliation, local permission grants, actual
  laptop/VM work, Kubernetes failover and mixed-workload performance acceptance.

## Reproducible verification

Run against the newly built server in a disposable environment with PostgreSQL 16
and Python; the harnesses create their own databases, identities and test factors:

```sh
python3 scripts/test-enterprise-management.py --server /path/to/cowork-server --execution-admission
python3 scripts/test-enterprise-management.py --server /path/to/cowork-server --execution-generation
python3 scripts/test-enterprise-management.py --server /path/to/cowork-server --execution-recovery
python3 scripts/test-enterprise-management.py --server /path/to/cowork-server --execution-coverage
python3 scripts/test-enterprise-management.py --server /path/to/cowork-server --execution-enrollment
python3 scripts/test-enterprise-execution.py --server /path/to/cowork-server
```

The first harness uses real PostgreSQL and HTTP but simulates executors and
infrastructure readiness. Forty concurrent reservation requests exercise atomic
limits, immutable retries, revocation, generation fencing, scoped completion,
policy changes with outstanding work and incompatible management replicas.

The generation harness uses real lifecycle HTTP/database transactions with
simulated infrastructure receipts. It checks frozen bindings, delayed receipts,
the admission fence while lifecycle work is queued, restore/canary/promotion/
rollback, scoped recovery and rollback of the entire transition when the epoch
audit insertion fails. Filesystem/provisioner tests additionally exercise actual
generation markers, both rendered service configurations, durable journals,
source restoration after injected interruption and fresh rollback identities;
infrastructure/database restoration in those unit tests is mocked.

The recovery HTTP harness also simulates stop receipts. It checks administrator
revocation at dispatch and recovery, foreign/device selections, substituted proofs,
atomic rollback on audit failure, late completions, races, already-stopped recovery
and the deletion gate. Provisioner tests cover retained-generation connections,
foreign/surviving containers, inventory changes, journal substitution and failure
to persist the stop receipt. Component tests cover explicit acknowledgement,
auditor access, stale review data and revision conflicts.

After `npm run build:web` in `app`, run
`node scripts/test-enterprise-capacity-ui.mjs` there for desktop/mobile rendering,
administrator submission and auditor read-only checks. This browser test uses
mocked API responses and does not claim full browser-to-provisioner acceptance.

The coverage harness tests failed activation with running/stopped legacy fixtures,
a still-running disabled replica, database rejection of obsolete dispatch writes,
missing/malformed runtime proofs, audit-write rollback, generation changes without
matching evidence and refusal to restart with admission disabled. Infrastructure
receipts in this harness are simulated. The Docker harness additionally checks
actual manifests/attestations, TLS trust/hostname rejection and a management restart
while the independent operator stays running.

The enrollment HTTP harness uses real PostgreSQL with simulated infrastructure
receipts. It checks explicit acknowledgement, administrator and operator contracts,
receipt substitution, atomic rollback on audit failure, one-attempt recovery and
the start fence before activation. Provisioner tests exercise journals, backup
verification and interruption boundaries with mocked infrastructure; the separate
Docker enrollment harness above exercises the actual infrastructure path. After
building the web app, `node scripts/test-enterprise-enrollment-ui.mjs` from `app`
checks desktop/mobile review and auditor restrictions with mocked API responses.

The worker harness runs two real instance workers with separate databases and a controlled
local model endpoint. It checks the shared user limit, model-only accounting,
management restart/outage, instance restart, durable receipt delivery, worker death
and no timeout reassignment. Simulated authenticated v2 protocol peers exercise
external executor claims and terminal receipts. A database trigger deliberately
rejects outbox writes to verify that run completion and the assistant message roll
back with the receipt. An independent standalone worker also completes with
admission disabled while management is unreachable. A lifecycle promotion with
simulated infrastructure receipts deliberately leaves the old real worker alive:
it cannot claim new work, while the replacement configured with the promoted epoch
executes once. Old unresolved reservations remain held throughout this test.
The controlled model is not a real provider, the protocol
peers are not laptop agents, and none of these tests are the agreed 40 simultaneous
shell/browser/VM workload or Kubernetes HA acceptance.

The Docker Desktop harness creates a separate management database, fleet database,
object store, private networks, operator volume and two newly provisioned instances:

```sh
python scripts/test-enterprise-execution-docker.py
```

It exercises the shared slot with actual API/worker containers, a paused management
container, worker restart and outbox delivery, a shell task through the real runner,
consistent backup/restore, an actual worker configured with a stale epoch, canary,
promotion, rollback, and deletion of all test resource generations. It additionally
kills an actual worker with an outstanding reservation and holds a connection to
an older retained database generation. Recovery must fail and retain the slot;
after the connection is closed, explicit recovery releases it and allows a fresh
task without replaying the interrupted task. Its controlled
model runs only on the private test network. The update fixture uses different
immutable package digests with the **same server binary**; it tests lifecycle and
generation transitions, not arbitrary cross-version migration compatibility.
Failures retain the dedicated installation for inspection; success removes it.
No existing management API, instance, database or storage is used for these tests.

Local candidate packages are built as `cowork-server:execution-coverage-candidate`
and `cowork-provisioner:execution-coverage-candidate`. These tags do not replace
the live `local-review` images. Build hashes and qualification boundaries are recorded
in `.internal/enterprise-local/execution-coverage-results.json`; the isolated Docker
report is `.internal/enterprise-local/execution-coverage-docker-results.json`.
