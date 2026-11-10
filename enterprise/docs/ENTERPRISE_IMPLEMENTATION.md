# Enterprise implementation status

The agreed enterprise plan is **not yet fully implemented or production qualified**.
This change adds a tested management foundation and an initial Compose provisioner.
It does not constitute the five-stage enterprise release or its acceptance.

## Implemented

- Persistent chat-workspace candidate (schema 50): instance servers assign a
  project/chat-bound storage identity to new Linux runs when explicitly enabled.
  Follow-up tasks reuse it; other chats remain separate, and public sandbox input
  cannot choose a foreign workspace. Database bindings are immutable; dispatch
  serializes a chat across workers. Tools, Crew, desktop, terminal and artifact
  adapters use explicit versioned runner routes without legacy fallback. Existing
  runs retain their original volumes. This candidate is not deployed to the
  schema-49 management service or live tenants. Fleet lifecycle qualification,
  extension/environment management and other backends remain outstanding.
  Ninety-one Rust tests and twelve actual Docker checks pass, including two
  workers, restarts and authenticated terminal/desktop sessions. All generated
  test resources were removed; 39 preexisting running containers are unchanged.
  See `ENTERPRISE_CHAT_WORKSPACES.md` for configuration and test scope.

- Initial release selection (schema 49): new Compose/container instances
  can select a published, immutable six-image release. The instance and operation
  retain that selection; database triggers reject changed bindings and dispatch
  without the versioned operator contract. The operator pins the selected images
  before creating credentials/resources, checks installed images, and delivers
  bound completion receipts without replay. Withdrawn/stale catalog entries cannot
  authorize new provisioning. Explicit stopping recovery and delivery of an already
  completed receipt remain possible after withdrawal. Existing instances can still
  restart their installed version. This grants no update/canary qualification.
  The creation form requires a choice when the feature is enabled, clears it on
  organisation changes, and disables creation if the catalog cannot be loaded.
  Legacy clients/operators retain default-image provisioning when selection is
  omitted. It is installed and enabled on the local schema-49 control plane after
  a fresh encrypted backup and migration rehearsal preserving all 80 source tables.
  All seven tenants retained their images; default provisioning images are unchanged.
  Startup preserves the paired feature switches and rejects conflicting caller
  settings. Both switches still default to false in general deployment templates.
  Kubernetes/VM selection is not qualified. Deployment evidence:
  `.internal/enterprise-local/provisioning-release-control-cutover-results.json`.
  Validation and operation instructions: `ENTERPRISE_PROVISIONING_RELEASES.md`.

- Central execution-admission candidate with durable platform, organisation,
  per-user and instance run-count limits across separate databases, including
  model-only work. Instance workers reserve before granting a lease and persist
  successful completion receipts atomically with the terminal result; an outbox
  reconciles management outages and restarts. Reservations do not expire on
  disconnect. Immutable operator ceiling revisions reject conflicting/stale
  replicas; organisation policy changes also wait for outstanding executions.
  Its management/provisioner/web support remains installed locally at schema 49;
  central admission and enrollment remain **disabled**. The seven tenant instances
  retain their previous images and are not yet enrolled.
  Frozen lifecycle epoch transitions now cover restore, canary, promotion,
  rollback and explicit stopping recovery in management and provisioner code.
  Both service manifests carry the active epoch, required admission and scoped
  credentials; invalid evidence is rejected. Private callback networking and
  operator image allowlists include an isolated binary compatibility probe.
  Explicit administrator recovery stops a whole Compose instance, verifies every
  retained generation and releases only selected server reservations. Offline
  device/VM reservations remain held; unresolved reservations block deletion.
  The candidate UI provides limits, reservations and a stop acknowledgement.
  A persisted activation gate now rejects un-enrolled existing instances, fences
  old dispatch writes in PostgreSQL and requires a journal-bound runtime attestation
  before readiness. Disabled/stale management replicas leave readiness; disabling
  admission on restart is rejected. An operator-only legacy preflight now checks
  actual encrypted backup integrity, all retained database generations and pending
  work under a read-only database session, without enrolling or changing an instance.
  The opt-in enrollment transaction/UI now captures a fresh verified backup,
  checks all retained generations, installs the bound runtime and records an
  immutable receipt atomically with audit and backup metadata. Interrupted work
  requires an explicit recovery attempt; enrollment leaves the instance stopped
  until platform activation. Completed local history, lease-free interrupted
  server tasks and unstarted snapshot waits can be retained without queuing them;
  runnable work, active sessions and outstanding reservations still block enrollment.
  Any registered external executor or external assignment history still requires
  separate reconciliation, even after expiry/completion. Actual Docker tests cover
  enrollment, interruption/recovery, activation and subsequent worker admission
  with the current candidate initially in legacy mode. Separate cross-version
  acceptance covers the exact previous migration-43 server/provisioner pair:
  real source chats, encrypted management backup and return to the old server,
  management migration, canary/source recovery, instance promotions, enrollment
  with retained old generations and new centrally admitted tasks. The five actual
  tenant image combinations at schemas 32, 33 and 38 now also pass separate Docker
  upgrades to schema 48, including workspace files, encrypted attachments, dormant
  history and real shell work. The five cases total 56 checks and 80 lifecycle
  operations; these totals do not represent simultaneous workload acceptance.
  Evidence is in `.internal/enterprise-local/execution-legacy-upgrade-results.json`.
  These tenant tests use generated fixture data and do not migrate the live fleet.
  A separate read-only capture/rehearsal restores an encrypted copy of the actual
  management database with the old server, then migrates it with the candidate;
  all original columns/contents of its 75 tables remain identical. This was repeated
  with the control API/operator/web stopped for the subsequent live cutover. The
  verified backup was restored to a separate management database and that copy
  migrated to schema 48; the original schema-43 database remains untouched. All
  35 other running containers, including the seven tenants, kept their IDs/images.
  The operator now uses an independent namespace and authenticated private TLS.
  Startup preserves selected database URLs and four immutable control image pins;
  invalid image manifests are rejected before secret initialization.
  Evidence: `.internal/enterprise-local/control-plane-cutover-results.json`.
  The matching six-image tenant update is now published for exactly the five
  qualified source combinations. The live catalog has no canary qualification
  for it yet; installation remains fenced until a local test/restore operation
  succeeds. Publishing does not upgrade tenants or change provisioning defaults.
  Evidence: `.internal/enterprise-local/enterprise-execution-release-results.json`.
  Per-instance rollout readiness,
  device/VM migration and additional backend coverage are still required before
  live activation. A separate Docker acceptance harness covers actual
  provision, worker/outbox recovery, restore, release transitions and recovery of
  a killed worker, including refusal while an old database generation is active.
  See [execution admission status and tests](ENTERPRISE_EXECUTION_ADMISSION.md).

- Explicit permanent deletion for qualified Compose container instances, enabled
  in the local Docker Desktop profile. Exact-name confirmation and acknowledgement
  of managed-backup removal are required. The trusted provisioner freezes and
  validates a scoped inventory, stops producers, removes every retained database/
  storage generation and identity, workspace volume, private network, managed
  archive and per-instance secret, then verifies absence before acknowledging.
  Central audit/operation metadata and private deletion receipts remain. A partial
  deletion stays locked in `deleting`; each recovery attempt needs a fresh explicit
  acknowledgement. Unknown ownership or missing evidence blocks removal. This is
  logical resource removal, not media sanitization, external-export erasure or an
  automatic retention policy. Kubernetes/VM deletion remains unqualified.

- Organisation-scoped OIDC providers from an operator-owned allowlisted catalog,
  signed ID-token group mapping, optional account/membership admission, explicit
  existing-account linking, revision-bound callbacks and audited suspension on
  policy/provider changes. Manual roles are retained; broker access checks reject
  suspended memberships. Shared browser/desktop SSO settings and scoped sign-in
  are implemented with disposable two-provider acceptance. See ENTERPRISE_OIDC.md
  for setup and limits. The opt-in migration0063 SCIM/LDAPS source slice now adds
  continuous source reconciliation, exact connector/contributor provenance,
  separately confirmed legacy containment and current-origin withdrawal, with
  actual operator/API/UI paths and narrow native/DDL checks. Installed Directory,
  mixed-replica races and real external IdP/SCIM/LDAP qualification remain open.

- Separate `COWORK_MODE=management` process with its own PostgreSQL database.
  It exposes authentication and management metadata, excluding the application
  project/run/agent API. Existing standalone API/worker modes remain available.
- Organisation membership, platform/organisation administration, auditor access,
  instance ownership and explicit manager/member/viewer assignments. Composite
  foreign keys prevent cross-organisation resource associations.
- Organisation administrators can issue, inspect and revoke single-use invitations
  for a named email address and member/admin/auditor role. Only token hashes are
  persisted; the private link is returned once and uses a URL fragment. Invitations
  expire within 30 days (seven days by default), and acceptance checks the inviter's
  current authority under the organisation lock. Registration atomically creates
  a non-platform account, membership, session and acceptance audit. Existing users
  must authenticate as the invited identity; their existing membership and content
  permissions are preserved. Invitations grant no instance/project assignment or
  administrative content permission. Browser/desktop management provides the form
  and a `/join` flow. Password, TOTP, passkey and the existing SSO sign-in can precede
  acceptance; tab-local, server-bound resume data expires after fifteen minutes.
  Links are copied for manual delivery; no email transport is implemented.
- `/management/v1` endpoints for organisations, policies, members, instances,
  operations, template registration, environment records and audit events.
- Instance-count quotas serialised against concurrent creation. The Docker
  runner candidate also enforces aggregate sandbox CPU, memory, distinct active
  run IDs and container count within one instance. Jobs, persistent desktops,
  terminals and file-transfer helpers share that budget. Docker memory/swap caps
  match the reservation; rejected jobs do not create workspace volumes.
  A private persistent ledger and interprocess file lock precede Docker creation.
  The acknowledged immutable container ID is persisted before execution; a failed
  create response or runner restart never forgives an unproven launch. Existing
  scoped containers are counted, including containers awaiting start. Corrupt or
  inconsistent evidence blocks further admission. Known workload containers are
  removed during an explicit instance stop, retaining workspace volumes.
  The candidate has separate Docker acceptance and a managed canary/update path;
  a dedicated update fixture now runs it. Other instances retain their pinned
  release until their exact source/target transition is qualified and selected.
  Central organisation/user run admission and model-only slots have a separately
  tested candidate, still disabled pending legacy enrollment and the remaining backend qualifications.
  Storage quotas and fair scheduling remain open. These sandbox caps exclude API/worker/proxy overhead;
  they are not an aggregate cap on all host resources used by an instance.
- Operator-owned policy ceiling, revision conflicts and policy intersection.
  Organisation policy changes require stopped instances and completed operations.
- Durable operation queue with one active operation per instance, request-bound
  idempotency keys, private provisioner authentication and lease-bound receipts.
  Authority is rechecked before dispatch. Expired leases become `reconciling`;
  they are never assigned again automatically. A matching delayed receipt may
  complete the original operation. Private journals persist both successful and
  failed completion envelopes. After process loss, a locked operator reports a
  verified receipt without re-execution. Unproven operations remain blocked; an
  authorised Compose recovery request may stop and verify the instance before a
  separate explicit start. Corrupt/missing resource evidence still requires review.
  A Linux file lock includes surviving infrastructure subprocesses and excludes
  overlapping operators on the same state directory; this is not Kubernetes HA.
- Transactional per-organisation HMAC audit chains with concurrent-writer
  serialisation and integrity checks on reads. Events contain metadata, not conversation contents. External
  checkpoints, verification UI and retention enforcement are still open.
- Browser/desktop `/management` route with organisation selection, instance
  creation/actions, policy editing, member assignment and audit/history tables.
  The existing sign-in/session flows are reused.
- Docker Desktop provisioning is connected and enabled. Browser and desktop
  open a ready instance through a single-use, 60-second signed ticket. Tickets
  and per-instance keys bind organisation, instance and user; passwords remain
  in management. Every authenticated instance API request rechecks central
  membership and fails closed on management outage. Administrative reading is
  separately authorised below. Upgraded instances also accept purpose-bound
  viewer tickets, gated by a fresh version-2 content-access capability heartbeat;
  the legacy callback never authorises a viewer or support-only administrator.
- **Instance members** in the shared browser/desktop management UI lists the
  owner and assigned manager/member/viewer roles. Owners, assigned managers and
  infrastructure administrators can select active non-auditor organisation
  members, change assignments and revoke them. Ownership cannot be changed or
  removed through an assignment. Mutation authority is checked under the same
  organisation lock as membership changes; assignments/removals are audited.
  Readers cannot assign themselves a stronger role or enumerate the candidate
  directory. UI requests remain scoped to the selected organisation/instance.
- Instance viewers can read only objects allowed by their originating instance's
  project/team memberships; they do not acquire all-instance content access.
  Their central read-only ceiling overrides retained local owner/editor roles.
  Removing an assignment denies existing sessions and future launches. An
  administrator may still qualify for the separately governed support access.
  Personal-device HTTP requests and agent WebSockets recheck the owner's central
  execution authority. Interactive desktop/terminal ticket exchange also checks
  current instance and object rights; active relays recheck every two seconds
  and close on denial or unavailable authority. This is online revocation, not
  implementation of the planned offline execution/budget reconciliation engine.
- Administrative content access is integrated between management and upgraded
  instance APIs. Project editors can approve an organisation administrator for one
  project or one chat, read-only, for at most 24 hours; approvals are stored in the
  originating instance and can be revoked by their grantor, authorised editors or
  recipient. Platform administration alone does not qualify a content reader.
  An explicit `content_admin` membership flag permits reading without individual
  approval only when the platform, organisation and instance policies all allow it.
  Each administrative object read is recorded in the central HMAC audit chain,
  without prompts, files or grant reasons. Existing local grant-access audit records
  are retained. Sessions recheck central authority; event streams also recheck the
  session and object approval before each batch. Approval expiry, revocation,
  administrator demotion and management outage stop further reads. Support-only
  sessions cannot create content, run commands, pair devices or open interactive
  desktop/terminal channels. The shared UI displays the effective rule, offers
  eligible organisation administrators as recipients and identifies read-only
  support sessions. An authenticated capability heartbeat exposes support launch
  only for upgraded instances; legacy access checks never grant administrative
  authority. A separate read-only content browser opens authorised projects/chats
  without loading team-administration APIs or offering execution/reply controls.
  Chat-only approvals expose parent project navigation identifiers/name, but
  suppress its description and execution configuration. Broader fleet rollout and
  cross-instance organisation content sharing remain open.
- Local instance gateway uses separate `*.localhost` origins and host-only
  cookies. The shipped production gateway now prepares exact HTTPS instance
  origins, domain/TLS configuration and paired launch templates through
  `scripts/prepare-enterprise-gateway.py` and
  `deploy/enterprise/docker-compose.production.yml`. It retains host isolation
  instead of adopting management-origin credentials through a path proxy.
  Installed DNS/TLS, current images and authenticated routing remain open
  production acceptance. The local profile explicitly uses Docker Desktop seccomp;
  the normal Linux profile retains its AppArmor requirement.
- Operator-owned Python provisioner: separate Compose API/worker/runner/proxy
  services, PostgreSQL roles/databases, private S3 buckets and credentials,
  encryption keys and network names per instance. Implements provision, start,
  stop, suspend and resume with durable local receipts.
- Gated backups/restores for stopped Compose container instances. Archives include
  DB, encrypted objects, keys/configuration and runtime workspace volumes inside
  an age envelope, with scoped manifests and hash validation. Restoration stages
  fresh database/bucket/workspace generations before activation and preserves the
  previous resources. Historical sessions/stream tickets/executor credentials are
  revoked, unfinished runs interrupted and schedules disabled. Restore is currently
  restricted to the instance's exact installed image set. Normal starts keep each
  instance's deployed image set instead of silently upgrading it.
- Fleet release digests are persisted independently from management build tags.
  Retained image tags prevent local rebuilds from displacing a deployed release.
  A repair utility recovers the exact Linux/amd64 runtime blobs from retained
  BuildKit history, verifies every imported digest, and refuses substituted images.
  Production release registry retention and cross-release qualification remain open.
- Compose update engine with immutable operator-owned release manifests, exact
  supported source image sets, encrypted pre-update backups and fresh database/
  object/workspace generations. Candidate API/runner/worker startup and a real
  sandbox probe run without connecting the API to the instance gateway. A canary
  returns to the original generation and checks it before recording qualification.
  Qualification must match a completed durable marker; corrupt or interrupted
  evidence cannot authorize a rollout. Failed candidates preserve the source;
  unverified rollback does not report a startable instance. Successful promotions
  remain stopped for an explicit start. The management API and shared desktop/web
  UI now select operator-published releases, default to canary testing and gate
  installation on a completed canary. Catalog withdrawal/source compatibility
  and request authority are rechecked at dispatch and by the local operator.
  Pre-update backups enter the instance's backup catalog, including verified
  backups from failed updates. Incomplete updates retain their lease and block
  other actions; explicit recovery returns to the recorded source without replay.
  A stopped instance can roll back its latest installed update through a scoped
  promotion/backup selector. The current state is backed up first, then the exact
  pre-update image set and data are restored into fresh resources and probed off
  the gateway. Sessions/credentials rotate, schedules stay disabled and changes
  since the selected backup cease to be active; the UI requires acknowledgement.
  A later restore or promotion invalidates older rollback points. An interrupted
  rollback remains blocked until explicit recovery restores the newer source;
  it never repeats staging. Broader compatibility acceptance remains open;
  catalog publication is an operator CLI, not a public image editor.
- Durable Compose batch updates through the management API and shared web/desktop
  **Batch updates** tab. Administrators select up to 40 instances in a fixed order,
  with groups of 1–10; auditors can inspect progress. Every source/target pair
  requires completed qualification. One transaction reserves all targets and
  rejects stale revisions or concurrent instance actions. Ready instances run
  stop/update/start; initially stopped instances remain stopped. Each installation
  retains its verified pre-update backup and rollback point. The next group waits
  for the previous group to finish. Metadata survives management restarts and
  retains the original operation IDs. Pause prevents new dispatch; running actions
  settle normally. Failed or uncertain actions pause the plan without replay.
  Cancellation releases instances only when their operations are settled; it does
  not undo installed versions or automatically restart stopped instances. Authority
  and catalog availability are rechecked before every step and dispatch. Policy
  changes cannot bypass reserved instances. Kubernetes rollout parity remains open.
- Instance-scoped runner container/volume names and recovery filters. Stopping
  an instance also stops surviving workload containers. The provisioner and
  runner form the trusted infrastructure boundary; tools get neither host
  credentials nor the Docker socket.
- Durable small-subnet allocation from an operator-selected private IPv4 pool,
  excluding existing Docker networks and retained instance allocations. Legacy
  instance networks are preserved. Unit coverage allocates 40 isolated network
  sets; this is not the forty-workload performance acceptance.
- Generated per-instance HTTPS proxy allowlists and blocked private addresses.
  An empty allowlist grants no Internet access. Internal destination profiles
  and IPv6 egress are not yet qualified.
- Compose management/web configuration and management/instance Helm charts.
  Kubernetes manifests include restricted pod permissions, node spreading,
  disruption budgets and network policies. The native Kubernetes runner is
  outstanding, so Kubernetes instance operations are not offered by the API.
- Enterprise v3 Rust/TypeScript contracts and generated OpenAPI/JSON schemas.
  The existing runtime/device protocol remains v2 with its v1 compatibility
  window; distributed protocol-v3 execution is not advertised.

Unavailable operations are rejected before queuing and omitted from the UI.
VM isolation never falls back to a container. Templates remain disabled pending
a real qualification/activation workflow. Workflow/grant structs are contracts,
**not a running distributed workflow engine**.

## Remaining agreed scope

| Capability | Remaining implementation |
| --- | --- |
| Instance fleet | Production acceptance of the shipped domain/TLS instance-origin gateway, organisation content sharing and broader rollout of the content-access/viewer release |
| Identity | Broad organisation MFA policy; production acceptance of the shipped optional provider assurance and SCIM/LDAPS Directory paths, mixed-replica containment qualification, external identity-provider and hardware-passkey qualification |
| Lifecycle | Kubernetes/VM and fresh-install backup/restore; broader cross-release qualification and retention-controlled deletion; recovery of corrupt or missing infrastructure evidence |
| Environments/extensions | Fleet lifecycle qualification and activation of the per-chat workspace candidate; persistent environment lifecycle, catalogue approvals, isolated installation/builds and template qualification |
| VMs | KVM/libvirt, KubeVirt, Hyper-V and attached Linux/Windows VM lifecycle and snapshots |
| OpenShell | Adapter, prerequisites, independent compatibility tests and release gate |
| Distributed chats | Durable coordinator connected to workers and cross-destination execution |
| Laptop access | Per-chat local grants, local Linux/VM sandbox modes, full-access activation and stop switch |
| Transfers/offline | Grant enforcement/conflicts, local event buffering, reconnect reconciliation and offline budget reservations |
| Governance | Production integration/activation of the central run-admission candidate; aggregate CPU/memory/storage quotas, fair capacity queues, admission-recovery UI and qualified fleet rollout; model budgets, key lifecycle, external audit anchoring, retention/export/deletion |
| Kubernetes parity/HA | Native execution backend, failover/recovery integration and live parity tests |
| Migration/acceptance | Verified legacy backup/import; 40 mixed tasks; hostile-extension tests; fresh restore and actual node/DB/worker failures |

Pod resource limits do not establish an aggregate sandbox quota. An HMAC chain
alone does not establish external tamper evidence. No certification or legal
auditability claim is made.

## Verification

The execution-generation extension passes real PostgreSQL/HTTP lifecycle tests
with simulated infrastructure receipts, including rollback of an epoch transition
on injected audit failure. The worker harness also keeps an actual old worker alive
after a simulated infrastructure promotion: it is fenced, and its replacement
executes the queued task once. Seventy-three provisioner tests cover marker/manifest
scope, durable journal binding, canary/source restoration and distinct rollback
epochs; the actual infrastructure calls in those unit tests are mocked. The
existing management/admission/deletion harnesses, 78 Rust tests (one external S3
test ignored), TypeScript and contract checks (55 schemas / 44 operations) pass.
Separate server/provisioner candidate images are built, with the tested server
binary hash verified inside its package and the provisioner imports checked.
At the time of that qualification, neither candidate nor migration 0045 had been
installed and the live database was at 0043 with seven ready instances. The later
schema-48 control-plane cutover is recorded above. Original evidence:
`.internal/enterprise-local/execution-generation-results.json`.

The execution-admission candidate passes the real PostgreSQL/HTTP broker harness
and the separate two-instance worker harness, including durable completion during
management loss, shared model-only/device run slots, no reassignment after worker
death, ceiling-revision fencing and policy changes with outstanding reservations.
Injected outbox failure rolls back the terminal run state and assistant message.
A standalone worker with admission disabled still finishes while management is
unreachable. Infrastructure readiness and device protocol peers are simulated;
the model is a controlled local endpoint. This is not a 40-workload or HA result.
The existing management regression suite, 78 Rust tests (one external S3 test
ignored), TypeScript checking and generated/wire contracts (53 schemas / 44
operations) also pass. That original qualification did not deploy the candidate;
management then remained at migration 0043. The later control-plane cutover
installs schema 48 while keeping admission disabled. Original evidence:
`.internal/enterprise-local/execution-admission-results.json`.

Organisation SSO acceptance now passes with two independent signed test providers:
catalog scope, exact signed group claims, role downgrade/removal, existing-session
and broker denial, manual membership preservation, invitation suspension, linking
after logout, transactional audit failure, stale queued member mutations, and
provider withdrawal/restoration. Configuration withdrawal is persisted before
management starts serving, preventing stale replicas from silently restoring it.
Migration 0042 is deployed to Docker Desktop after a validated backup. The live
API confirms an empty provider catalog is disabled and members cannot edit SSO.
The user's instance remains ready and restores its browser session. Browser login
retains its organisation and explains unavailable SSO. No real IdP is configured.
Evidence: `.internal/enterprise-local/organisation-sso-results.json` (2026-09-27).
This iteration passed 78 Rust tests (one external S3 test ignored), 47 selected
frontend tests, all six database/HTTP harness modes, web build, contract drift
checks (45 schemas / 40 operations), Helm lint/render and Compose override parsing.
The latest server/web image digests are in that evidence file; existing tenant
images remain pinned. Native desktop code shares these UI/runtime changes, but a
new installed Windows package has not been built in this iteration.

OIDC identity binding now uses issuer/subject and explicit binding provenance;
verified email alone cannot attach an external identity to an existing account.
An authenticated link rechecks its original session, account and mandatory admin
assurance at callback time. Account creation/linking, authorization-code issuance
and auth audit commit together. Migration 0041 preserves demonstrable old links,
blocks unverified email-derived links, revokes their sessions/codes and invalidates
old pending linking operations. See [OIDC upgrade behavior](ENTERPRISE_OIDC.md).
Provider error details are redacted; failed provider responses consume their state.
The application explicitly checks the authorized-party claim in addition to the
library's signature, issuer, audience, expiry, nonce and access-token-hash checks.
The disposable HTTP/PostgreSQL OIDC harness passes both fresh installation and a
real upgrade from the preceding MFA server binary. It verifies signature/claim
rejection, replay, concurrent ownership, session revocation, preservation/repair
of proven/unproven old bindings, and rollback when audit insertion fails. The
provider generates real RS256 signatures but is a local test double; no external
IdP is qualified by these global-provider tests. Organisation group mapping now has
a separate two-provider acceptance described in ENTERPRISE_OIDC.md.
The candidate is installed in the local management service after a validated
database backup. Migration 0041 and scoped organisation sign-in are verified live;
all seven tenant instances retain their images and ready state. The user's instance
also restores its existing browser session after reload. Evidence:
`.internal/enterprise-local/oidc-binding-results.json` (2026-09-27).

- Rust: 88 tests passed across contracts, runner and server; one existing external
  S3 test ignored because it needs a separate object-store environment.
- Disposable PostgreSQL and real HTTP server: organisation isolation, concurrent
  instance quota, platform ceiling, unknown-field rejection, idempotency, hidden
  leases, queued role/admin revocation, no reassignment after timeout, scoped
  recovery requests, recovery authority revocation before dispatch, conflicting
  late receipts, HMAC chain
  and rejection of an intentionally tampered audit event.
  Provisioner receipts in this test are simulated, not container/VM acceptance.
- Provisioner tests: safe identifiers/images, distinct scopes and keys,
  secret retry stability, host-mount restrictions, resource validation,
  default-deny egress, explicit Docker Desktop qualification, scoped federation
  keys and refusal to weaken VM isolation.
- 57 provisioner tests pass in Linux (55 pass and 2 Linux-only tests skip on
  Windows). They also cover archive scope, release compatibility,
  path traversal/links/duplicates, content corruption, quiescence, recovery-key
  replacement, separate restore generations, unchanged activation after failed
  staging, image pinning across restarts and forty distinct network allocations.
  Journal tests cover success/failure across processes, mismatched scope/arguments/
  leases, corrupt records, recovery without execution, stopped-state verification,
  exclusive Linux locks and lock inheritance by surviving child processes.
  Update tests cover immutable manifests, required completed canary evidence,
  rollback failure, no replay after interruption and detached probe routing.
  Infrastructure failure tracebacks suppress command arguments and captured
  output, including errors and timeouts that could otherwise expose credentials.
  Coverage also includes explicit interrupted-update source recovery, unchanged
  installed-source checks, catalog withdrawal and empty object/workspace restores.
  Post-update rollback coverage includes backup/scope tampering, changed generation,
  fresh restored resources, source preservation and recovery without replay.
- Runner admission: 11 Rust tests pass on both Windows and Linux. Disposable
  Docker Desktop tests use two runner processes sharing one instance ledger:
  eight simultaneous requests admit exactly two jobs and reject six without
  creating their volumes. Coverage includes stdin, artifact transfer, terminal
  disconnect, desktop recovery, timeout cleanup, abrupt runner death, corrupt
  and unproven reservations, preexisting containers, and an injected failed CLI
  response followed by delayed Docker creation. No tenant release is changed.
  This is a capacity/security acceptance, not the forty-task performance test.
- Real Docker Desktop fault tests: abrupt exit after completion, durable receipt
  delivery without restarting the instance container, denial of a second operator,
  abrupt exit after journalling but before execution, no automatic replay, and an
  authorised stopped-state recovery followed by an explicit new start. The same
  review/confirmation flow was exercised through the browser UI.
- Real Docker Desktop backup acceptance passed through four successive restore
  generations. It covers DB rollback, encrypted artifact decryption, persistent
  workspace contents, fresh DB/S3/bootstrap/runner credentials, retention of the
  previous resources, an actually modified encrypted archive rejected before
  activation, foreign-scope/missing catalog rejection, revoked historical
  sessions, interrupted unfinished runs, disabled schedules and a new shell job
  using the restored runner's new volume prefix. Results are in the private
  `.internal/enterprise-local/backup-test-results.json`. Test instances are
  separate from the user's instance. Fresh-install/offline-client/VM recovery
  and cross-release migration acceptance remain outstanding.
- Real Docker Desktop update acceptance switches a dedicated stopped test
  instance to the admission-runner image in a cloned generation, verifies a
  deliberately failed candidate, performs a successful canary and promotion,
  then restores and starts the original fixture. It checks source data counts,
  separate database credentials, retained encryption key and gateway detachment
  during probes. The result is in the private
  `.internal/enterprise-local/update-acceptance-results.json`. This does not
  qualify other server/schema, GUI, VM or scheduled-workload upgrades.
- Public update API Docker acceptance creates its own `Update-Abnahme` instance,
  rejects installation before qualification, completes a canary, installs the
  admission runner, registers both automatic backups, starts the instance and
  verifies another stop/backup/restore/start on the updated release. The result is
  in `.internal/enterprise-local/update-api-results.json`. The first execution
  found an empty-store restore bug; the regression and the repeated Docker flow
  now pass. Original tenant instances are not advanced by this acceptance.
- Interrupted-update Docker acceptance terminates a dedicated operator after a
  real candidate probe, observes reconciliation with all other actions denied,
  explicitly requests recovery and verifies the original generation and image
  set without repeating the update. The verified automatic backup is retained;
  a separate start returns the fixture to ready. The result is in the private
  `.internal/enterprise-local/update-recovery-results.json` (2026-09-27).
- Post-promotion rollback Docker acceptance installs the admission runner on the
  guarded backup fixture, then changes project data. A real process exit after
  probing the restored version leaves the operation in reconciliation; explicit
  recovery preserves the newer generation and both changed/new projects without
  replay. A new rollback restores the original project, encrypted attachment and
  workspace file, retains a backup of the newer data, rotates runtime credentials,
  revokes historical sessions and disables schedules. A shell job succeeds after
  restart on the restored release. Results are in the private
  `.internal/enterprise-local/rollback-results.json` (2026-09-27).
- Batch-update Docker acceptance updates two dedicated instances in separate
  ordered groups. A pause during the first stop survives a management restart;
  continuation keeps the original operation and performs no duplicate installation.
  Both installations retain automatic backups and scoped rollback points. The first
  instance returns to ready with its project unchanged; the initially stopped
  second instance remains stopped. Reservations are released and the live browser
  reports both targets succeeded. Results are in the private
  `.internal/enterprise-local/rollout-results.json` (2026-09-27). This exercises the
  qualified runner-only transition, not arbitrary schema or Kubernetes upgrades.
- Docker restart recovery: management/workspace gateways re-resolve Docker DNS;
  databases and object storage have persistent restart policies. A disposable
  test replaces both backend IPs while their gateways keep the same container
  and process start time, preserving request paths/query strings and private
  route denial. Existing managed instance images remain unchanged.
- 31 targeted frontend/runtime tests: schema boundaries, navigation, stale
  organisation responses, auditor controls, backend availability, history and
  recovery confirmation. A reconnected event stream clears only its own error,
  including on idle completed runs; unrelated action errors remain visible.
  A live browser test stopped and restarted the instance gateway and verified
  that the transient connection warning disappeared without a page refresh.
- Browser inspection confirms the backup catalog, scoped restore selector and
  automatic operation-status updates. Management clears temporary connection
  errors on recovery and prevents submitting a dialog after its instance changes.
  The release selector offers the approved compatible version, defaults to a
  canary and enables installation only with qualification. The stopped fixture
  was started again after inspecting the dialog.
- 14 update/rollback-focused management UI/client tests pass, including exact
  source matching, stale organisation replies, explicit data-loss acknowledgement
  and promotion selection. Public PostgreSQL/HTTP tests reject forged receipts,
  caller-supplied rollback images/backups, foreign scope and obsolete points after
  a later restore, and preserve transaction/idempotency boundaries.
- Five additional batch-update UI tests cover selection order and frozen revisions,
  qualification for every source, stale instance changes, read-only auditors,
  organisation changes and cancellation acknowledgement. PostgreSQL/HTTP tests
  cover atomic reservations, group ordering, pause/restart/resume, uncertain actions,
  cancellation during execution, queued authority revocation and catalog withdrawal.
  These HTTP tests use simulated infrastructure receipts.
- Administrative access acceptance starts a real management API and two instance
  APIs with separate databases in a disposable Docker PostgreSQL environment. It
  verifies project/chat scope, importing eligible recipients, denied foreign-org
  recipients and keys, refusal of legacy administrative access, read-only methods,
  revocation, expiry of an already-open SSE connection, administrator demotion,
  loss of instance membership despite retained local project ownership,
  explicit content permission, stricter instance policy, chained audit and fail-
  closed management outage. Instance infrastructure readiness is seeded in this
  test; this is not VM or Compose provisioning acceptance. Twenty-seven targeted
  frontend/client tests pass, including effective-rule notices, organisation
  recipient selection, explicit reader assignment, read-only controls, stale
  replies, legacy endpoint compatibility and connection recovery.
- Live Docker administrative-access acceptance uses only the second guarded
  rollout fixture. The new API/worker image passes canary source restoration,
  backup and promotion. Its existing project remains readable by its owner; a
  separate organisation administrator is denied before approval, can read the
  approved project but cannot create content, and is denied again immediately
  after revocation. A central chained audit entry identifies the approval. Results
  are in `.internal/enterprise-local/content-access-results.json` (2026-09-27).
  Management and web are deployed; other tenant API images remain pinned. The
  browser confirms the effective rule and scoped administrator directory.
- Membership API acceptance extends the disposable PostgreSQL/two-instance test:
  scoped candidate discovery, owner protection, foreign-organisation/auditor
  rejection, older capability denial, viewer-specific tickets, retained local
  ownership/editor permissions capped at read-only, denied self-escalation,
  idempotent removal, SSE closure and personal-agent HTTP/WebSocket revocation.
  A real reverse desktop relay exchanges bytes between browser and agent sockets,
  then closes after demotion and denies an unused ticket. Desktop metadata is
  seeded; this is not VM or graphical workload qualification. Thirty targeted
  UI/client tests pass, including assignment defaults, reader-only controls and
  stale instance responses. OpenAPI now describes the member directory and
  removal endpoints, with 28 shared schemas and 27 operations.
- Live Docker membership acceptance qualifies and installs the release on the
  second rollout fixture with source restoration and pre-update backup. A
  separate member creates a project, receives a shared project and becomes a
  reader: both objects remain readable while creation, device access and role
  escalation are denied. Removing the assignment denies both the active session
  and a new launch. The chained membership audit is verified. Results are in
  `.internal/enterprise-local/instance-members-results.json` (2026-09-27).
  Browser inspection additionally saves and removes the same test account's
  reader assignment through **Instance members**. Other tenant images retain
  their installed release; the new viewer protocol is qualified only on the
  dedicated fixture so far.
- TypeScript, targeted ESLint, style contract, web production build, legacy
  compatibility, enterprise schema drift and Rust-to-TypeScript wire fixtures.
- Organisation invitations: real disposable PostgreSQL/HTTP tests cover scoped
  administration, role/field injection, new registration, existing identity and
  role preservation, expiry, revocation, inviter demotion, simultaneous token
  consumption, hash-only storage, browser cookies and secret-free audit records.
  Forty-five targeted UI/runtime tests pass, including server changes, stale
  invitation responses, single-display links, redirect resumption and a second invitation opened in the same tab. The shared
  OpenAPI now has 36 schemas and 33 operations. Live Docker acceptance additionally
  exercises two dedicated organisations, a non-platform inviting administrator,
  new registration, joining an existing identity to a second organisation and
  revocation. Evidence: `.internal/enterprise-local/organisation-invitations-results.json`
  (2026-09-27). Only management and web images were updated; tenant releases remain
  pinned. This does not qualify an external identity provider or SSO group mapping.
  Browser verification additionally creates/revokes a test invitation, inspects
  the new-account form, accepts an existing account without changing its role,
  checks the destination organisation and switches between links in one tab.
- Compose configuration validation and Helm lint for both charts; these validate
  rendering, not container or cluster operation.

Platform-administrator sessions now require a verified second factor in management.
Migration 0040 deliberately leaves pre-existing sessions/codes unverified. Fresh
initial sign-ins can enroll TOTP; password/PKCE, one-use recovery and user-verified
passkeys preserve actual proof rather than inferring it from enrollment. Refresh
does not renew the twelve-hour proof lifetime. IdP claims do not grant proof without
a qualified mapping. Encrypted authentication secrets have a separate required
management key, independent of S3. The UI gates management and invitation acceptance,
guides enrollment, requires acknowledgement of recovery codes and fails closed on
an unavailable assurance check. The local startup script leaves enrollment to the
operator. Existing instance sessions are not retroactively requalified by this change.

The disposable PostgreSQL/HTTP MFA harness passes initial/old-session restrictions,
session-bound enrollment, encrypted storage without S3, password/PKCE/refresh,
expired proof, replay and concurrent recovery rejection, browser-cookie boundaries,
account-wide attempt limits, role promotion, and simultaneous TOTP/passkey removal
retaining a factor. Passkey deletion uses metadata fixtures, not hardware ceremony
qualification. Management, invitation and two-instance content regression harnesses
also pass. There are 78 passing Rust tests (one external-S3 test remains excluded),
33 targeted UI/client tests, and 38 shared schemas/35 OpenAPI operations with passing
Rust/TypeScript wire checks. Authentication events use the existing authentication
audit table; external audit anchoring remains outstanding.

The MFA management/web candidate is installed in Docker Desktop after a validated
management-database backup (2026-09-27). Live checks deny the unverified platform
administrator and retain scoped access for the ordinary organisation administrator;
all seven instances remain ready with unchanged tenant images. Browser inspection
opens the user's existing instance successfully and shows the new management
verification gate. The operator's authenticator is intentionally not enrolled by
the test. Evidence: `.internal/enterprise-local/management-mfa-results.json`.

Docker Desktop was repaired and the isolated `cowork-enterprise-local` review
stack was started on 2026-09-25. Real browser and native Windows WebView checks
pass for login, management/workspace discovery, saved server addresses, server
switching, secure session restoration, organisation selection and audit access.
HTTP checks reject cross-organisation access, member administration and credentials
from other instances. The local profile now provisions two actual instances with
healthy API, worker, runner and egress services. Live checks pass for single-use
instance tickets, central membership revocation, a shell job running as UID 10001
with seccomp/no-new-privileges/no capabilities/no host mounts/no Internet,
encrypted file upload/download, and stop/start preserving database contents and
sessions. The development desktop opens the managed instance using the current
frontend without replacing the installed release. Test details are recorded in
`.internal/enterprise-local/provisioning-test-results.json` (private, not committed).

There is no connected qualification cluster or VM infrastructure. VM lifecycle,
full hostile-extension penetration tests, forty-workload and Kubernetes
failover acceptance have **not** run. The workspace login service in the local
review stack has no worker, model, object store or sandbox configured.
No production deployment, migration, commit or push was performed.

```sh
cargo test -p cowork-contracts -p cowork-server -p cowork-runner
cargo build -p cowork-server
python3 scripts/test-enterprise-management.py --server target/debug/cowork-server
python3 scripts/test-enterprise-content.py --server target/debug/cowork-server
python3 -m unittest discover -s server/cowork-provisioner -p 'test_*.py' -v
cd app
node scripts/generate-enterprise-contracts.mjs --check
node scripts/check-enterprise-rust-contracts.mjs
npm run contracts:check
npm exec vitest -- run src/components/EnterpriseManagement.test.tsx src/components/EnterpriseRollouts.test.tsx src/components/RemoteServerView.test.tsx src/runtime/managementClient.test.ts
npm run build:web
```

The isolated runner candidate can be built on Windows with
`powershell -NoProfile -File scripts/Build-RunnerCandidate.ps1`, then exercised
with `node app/scripts/test-runner-admission.mjs`. The build script copies only
Rust build inputs to a temporary local directory, avoiding Docker metadata
lookups against missing files in cloud-backed folders. The test uses the local
sandbox images and a disposable private subnet (override with
`COWORK_ADMISSION_TEST_SUBNET`), and removes only its own random Docker scope.
Its result is stored in `.internal/enterprise-local/admission-test-results.json`.
The candidate defaults to 4 CPU, 8192 MiB, 2 distinct active run IDs and 64
containers; the provisioner supplies the instance policy limits and a dedicated
`runner-admission` volume. Standalone Compose supplies the same persistent volume.
Operators must retain this ledger across restarts. Unknown launch outcomes remain
reserved for investigation; there is no timeout-based release or automatic replay.

The HTTP acceptance script requires Linux and PostgreSQL 16 binaries (override
with `--postgres-bin`). It creates/stops a disposable cluster and removes its
temporary files. Server builds can run under WSL on Windows.

See [operator setup](../deploy/enterprise/README.md) for the review environment.


Permanent-deletion acceptance (2026-09-27): the disposable HTTP/PostgreSQL harness
checks exact-name/backup acknowledgement, cross-organisation denial, immutable
backup selection, inaccessible deleting instances, invalid/failed receipt rejection,
role revocation and one-shot recovery dispatch, backup purge metadata and audit.
Infrastructure receipts in that harness are explicitly simulated. The separate
Docker Desktop test creates a new instance with an encrypted artifact and workspace,
backs up/restores two generations, forces a real partial failure with an occupied
fixture volume, proves no automatic retry, then completes explicit recovery.
Both generations' resources, keys and archive are removed; 28 existing tenant
containers retain their IDs and images. The user's existing instance still loads
in the browser with its previous runs/artifact. Management was backed up before
migration 0043; tenant image releases were not changed. Evidence:
`.internal/enterprise-local/deletion-results.json`. This does not qualify VM,
Kubernetes, automatic retention or physical erasure.

The separate headless Edge browser acceptance also signs in as the ordinary test
organisation administrator, creates and automatically provisions a fresh instance, exercises wrong/
missing name and backup acknowledgement, confirms deletion, and verifies clean
removal without stale access errors. Evidence: `deletion-ui-results.json` and
`deletion-confirmation.png` in the same private directory. Final checks: 78 Rust
server/contracts tests, 66 provisioner tests on Linux, 19 targeted UI/client tests,
real HTTP management/deletion harnesses, Rust/TypeScript wire compatibility,
45 generated schemas/40 OpenAPI operations, and the web production build pass.
Seven existing instances remain ready; both newly created deletion fixtures are
removed, and no operation remains queued, running or reconciling.


## Additional source paths, 14 October 2026

The device agent now requires separate native original consent for enterprise device grants. Interactive `enterprise-grant-approve` retains the exact policy, real native aliases and any actually qualified local Docker image/context in the daemon's encrypted store. `enterprise-grant-resume` repeats the same original confirmation; `enterprise-grant-stop` requests local cancellation and retains evidence. Selected mode limits filesystem tools to the approved aliases. Sandbox uses a dedicated private workspace, immutable local OCI image, no network, a read-only root filesystem, dropped capabilities and bounded process resources. Full mode requires explicit local approval. A server checkbox cannot supply this native decision. Current server authorization and confirmation credentials are checked independently of transfer traffic; a 15-second freshness bound may be extended only by the same original bounded offline permit. Original leases, event batches and receipts remain encrypted. The actual complete request is retained only after result-snapshot publication; reconnect repeats that immutable original body, including its snapshot reference. Cancellation, timeout and restart do not prove external effects were undone or physical resources released. No live Docker/native qualification was performed for these source additions.

Device file transfer source uses the actual selected alias, source digest/size and original destination baseline. Files are bounded to 16 MiB; IPC moves 1 MiB chunks. Conflicts retain original incoming bytes and destination backups. Recovery accepts the original staged filesystem identity rather than treating a current matching digest as an original write witness. Native encrypted transfer custody is bounded to 1,000 originals / 256 MiB of archive ciphertext and 128 MiB of downloaded chunk ciphertext; reaching a bound holds the operation and preserves prior originals. Arbitrary host paths are not accepted through Sandbox grants.

Content federation source (schema 71) stores only instance/grant IDs, epochs, digests and original receipts in management. The recipient explicitly accepts an organisation-scoped source grant. The source instance checks the actual original session, owner or current administrative content grant, real project/version/snapshot FKs and current fleet epoch before exporting immutable document files. The target validates the live source through its configured instance gateway and copies the package through real scoped object-storage helpers into a new private project, snapshot and immutable version. Imported projects stay hidden until the original management receipt and current authority checks succeed. Sessions, provider profiles, runnable definitions, executor permissions and project policies are not imported. Transfer IDs recover the exact original bytes and receipts. Source/target journals remain encrypted under each instance's actual user scope. Bounds are 64 exact files, 16 MiB plaintext, 24 MiB package and a 32 MiB import request. These are source implementation statements, not deployed or physically qualified instance-sharing evidence.
