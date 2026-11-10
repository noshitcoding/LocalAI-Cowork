# Server implementation status — 2026-10-14

The current deliverable is the separate Enterprise source/Docker build bundle,
not an installed or production-qualified fleet. Current source includes the
original-bound VM/Kubernetes lifecycle, templates/environments, aggregate hard
disk placement, multi-destination workflows and device/environment transfers,
offline grants, additional VM controllers and encrypted snapshots, the pinned
OpenShell adapter, cross-instance content sharing, external audit anchors and
certificate renewal. These paths require the documented actual operator
enrollment, current authorization, physical identity observations and original
completion receipts; selecting a backend or changing a SQL state does not supply
those observations.

| Current source scope | Concrete path | Physical acceptance still required |
| --- | --- | --- |
| 66/67 managed VM and Kubernetes | Original host/backend enrollment, native Pod/PVC/CRI execution and desktop/terminal, UID-bound Helm lifecycle and physical stop recovery | Reviewed VM/cluster identities, images, storage, networking and installed lifecycle |
| 71 aggregate disk quota | Fully allocated guest/controller filesystems, dedicated PostgreSQL/MinIO, complete environment/snapshot reservations, current IO authorization and hard filesystem bounds | New qualified VM installation, actual topology/ENOSPC/teardown; shared legacy services are not adopted |
| 72 templates and environments | Immutable template revisions, current member/admin/session/MFA admission, original runtime identity, real lifecycle adapters and operator plans | Original VM/Kubernetes runtime, storage and capacity observations |
| 73 workflows, transfers and offline grants | Durable original DAG/Run bindings, current device grants, environment process/file authority callbacks, SHA/baseline CAS, original GET reconciliation and stop/held receipts | Installed device/environment synchronization and actual runtime completion/stop witnesses |
| 83 additional VM controllers and snapshots | Enrolled Libvirt/KubeVirt/Hyper-V/attached controllers; owned-QEMU offline encrypted snapshots with source/inode/hash CAS and original restart | Actual controller versions/permissions, stopped producers, archive custody and physical restore |
| 84 OpenShell | Pinned authenticated SDK/protobuf/mTLS gateway, physical parent VM, admitted Landlock/network/storage/resource limits and original create/stop/delete journals | Compatible installed SDK/gateway and actual hard-policy/runtime observations |
| 85 cross-instance content | Immutable export/import grants and receipts, current source/recipient Directory/session/MFA callbacks and retained original operations | Paired real instances, current keys and actual content export/import |
| 74/77 audit and certificates | Bound audit export/checkpoints/retention and independent witness; original certificate plans, renewal and actual selected restart/rollout adapters | Operator witness enrollment, DNS/TLS and installed renewal/rollout |

Current narrow compiler evidence is separate from the older qualifications below:
`.local/enterprise-current-checks-20261014/native-result2.json` records a successful
69.213-second Rust1.95 MSVC/default-stack/profile locked offline all-targets check
of Server, Runner, Device-Agent and Local-Daemon, with all 239 compiler inputs
unchanged. The current `ddl-result.json` records all 71 original migrations in a
fresh owned PostgreSQL16 single-user fixture, including the twelve70 workflow
tables and six enabled 70/71 original-write triggers, passed in 6.644 seconds.
The fixture was removed; no installed database or listener was used. Public
Python syntax receipts identify their actual interpreter and source hashes.

The source bundle producer requires those current receipts and a fresh successful
`build:web` with unchanged public source inputs before creating the archive. Its
public `evidence/frontend-build.json`, source manifest, SBOM and checksums bind the
actual web output. Public evidence is explicitly transformed from byte-preserved
private originals; machine paths become named roles and original byte hashes
remain recorded. Native Windows checking does not compile the Unix-only disk
quota `statvfs` branch. Linux Docker image compilation remains unperformed here
and is an explicit operator installation gate. No current VM/cluster/SDK probe,
image build, installation, publication or production acceptance is claimed.

## Historical qualifications through 2026-10-09

The following paragraphs and table describe their dated immutable snapshots.
Their test counts, runtime probes and image IDs do not qualify the current source
or the new69/70/71 paths. Later source/build additions are recorded separately
below; original evidence and the protected pilot restrictions remain unchanged.

The actual authenticated resume qualification (development artifact omitted from public sources)
passed all eight checks in 123.448 seconds on the immutable `2c4aa022` Python
snapshot, preserving all 177 qualified `dbf10bc3` runtime inputs. It used fresh
PostgreSQL, a native authenticated API and real worker, private HTTPS Runner and
an actual Bash job. A held unsafe checkpoint transaction was interrupted by the
owned worker's SIGKILL; real database lease expiry, signed stop authority,
explicit same-spec resume, immutable replay and stale-revision refusal were
verified. The resumed job completed with its marker once and no Bash replay.
All five cleanup gates passed, the owned harness child was reaped, the private
fixture was removed and the original 39 containers were preserved. All 298
frozen inputs and both native binaries were rechecked unchanged afterwards.
The model was synthetic; central fleet admission, production HA and installed
fleet migration remain unqualified.

The preceding zero-output diagnosis was traced to the qualification transport's
`NativeDockerRelay` handling of stdin EOF and output drain. The minimal Python
relay correction passed regression tests on Windows and Ubuntu. Direct pinned
Docker/Bash probes returned the expected output; no Rust-runtime correction or
test-trigger relaxation was made. Historical failed attempts and their retained
evidence remain unchanged.

The separate failed-VM retirements are physically verified: the exact owned
`de666` root and quarantine were removed with positive entry witnesses;
the later `5bb` CAS13 recovery finished at CAS14 with 24 entry witnesses,
matching tombstone/anchor and absent root/quarantine/WAL. The protected pilot
retained its full 4,500 CPU millis/4,608 MiB allocation. These dated receipts
do not imply current Docker uptime or a completed hosted lifecycle test.
The asset-complete `ec4263bb` snapshot adds the original AppArmor asset to
299 frozen inputs while preserving all 177 qualified runtime inputs; its
155 source tests passed. The actual hosted lifecycle is
blocked by the Docker dependency (development artifact omitted from public sources).
The signed targeted start failed on its stale AF_UNIX inference socket;
Windows rejected both the preserving exact rename and read-only metadata handles
with error 1920. The source object remained unchanged and no quarantine target
was created. The final read-only check confirms CAS14, only the fully charged
pilot and all 299/177 frozen inputs unchanged. No new test VM was created.
Current engine inventory/39-container preservation and uninterrupted uptime
cannot be asserted while the engine pipe is unavailable. No production HA or
installed migration is established by that attempt.

The [fleet plan](SERVER_FLEET_MIGRATION.md) and its separate 21-test calculator
review show that four new organisation guests and the protected pilot exceed
the unchanged parent plan. Even guest reserves plus QEMU overhead exceed it
before service/job pools. Smaller job profiles alone cannot resolve this.
Additional separately budgeted execution hardware or a newly qualified complete
capacity model is an explicit later decision; existing tasks and instances remain.


The requested implementation covers the original server architecture review.
The goal remains open until the remaining integration and rollout checks pass.
The existing deployment and a source candidate are distinct states. The evidence
in `SERVER_RUNTIME_QUALIFICATION.md` identifies the previously qualified image;
it does not qualify subsequent source changes automatically.

| Requirement | Current state | Remaining acceptance |
| --- | --- | --- |
| Independent scheduler, bounded dispatch and maintenance | Schema59 candidate passed native lifecycle and ownership regressions | Installed rollout |
| Database-time leases, shutdown, fencing and unknown remote outcomes | Schema59 database-time ownership and cancellation fences passed Rust and PostgreSQL checks; eleven isolated physical Runner checks passed | Full manager/worker integration and installed rollout |
| Limits for infrastructure and retained instance allocations | Schema55 immutable host allocations and fresh exact-inventory attestation implemented; combined schema59 PostgreSQL qualification passed | Physical manager-to-guest client qualification and installed-fleet resource rollout |
| Global platform, organisation, user and instance admission | Real concurrent PostgreSQL waves and host CPU/RAM allocation, shrink/disable/delete-fencing checks passed | Installed rollout; operator attestation reports observed inventory, not independent manager measurement |
| Fair queue and understandable waiting | Queue rotation, access-controlled diagnostics and lease-owned runner waiting implemented | Rust/PostgreSQL and seven authenticated native HTTP waiting checks passed; installed rollout |
| Separate execution trust domains | Real KVM/mTLS/daemon pilot qualified; host registry, scoped operator claims and dynamic ports implemented | Real manager-to-VM tenant execution and lifecycle; migration of installed tenants |
| Connection ownership and multiple API replicas | Default-off private mTLS forwarding, physical-job binding and durable ownership passed schema59 PostgreSQL checks | Sixteen schema59 three-API protocol checks passed with a fake executor; the full physical 23-check Linux-runner HA matrix passed on its frozen sources; production HA flag remains false |
| Event hints plus durable replay and current authorisation | Expanded candidate passed durable replay and current-authorisation process regressions | Installed rollout |
| Smaller storage and sync modules | Source split into domain modules with retained contracts | Compile, Clippy with denied warnings and regression qualification passed for the frozen schema59 sources |
| Uniform execution state and shared runtime integration | Existing V1 runtime/contracts retained; trusted server-agent checkpoints, bounded resume and physical-stop receipt enforcement implemented; authenticated actual resume/workspace reuse passed all eight checks | Installed rollout; no V2 bridge is assumed by this server review |
| Persistent workspace lifecycle | Local backup/restore/canary/promotion/rollback/deletion passed with genuinely different schema52 and schema59 executables and Runner59 legacy compatibility | Full hosted schema54-to-59 lifecycle and actual guest qualification |
| Release/capability status reflects actual behaviour | Process health, actual loop ticks and release gates implemented | Host-scoped inventory/canaries and expanded release evidence |

No installed tenant, feature gate, global Docker context or unrelated container
is changed by source work or private qualification fixtures. A later deployment
must use exact qualified image IDs, preserve rollback material and report its
actual installed state. The VM pilot alone does not certify hostile tenants,
remote backup/update, disk quotas, certificate rotation or distributed HA.

The handoff from “App-Struktur bewerten” was explicitly coordinated in the app;
the continuing parent chat owns this V2/server completion scope. Server work uses
the Open_Cowork server/deploy/docs scope, and V2 uses its separate Development
checkout. This document does not claim an adapter agreement or production bridge.

Expansion evidence is kept separately under
`.internal/server-expansion-2026-10-09/`. Its candidate input manifest records
schema 54 and exact frozen source hashes. Failed builds remain evidence, and a
new source revision receives a new snapshot; only actually executed passing
checks establish qualification. No canonical knowledge vault is written.

The frozen schema-54 runtime passed 131 normal Rust tests, 16 separately
executed PostgreSQL tests and 15 native process checks. Its protocol waiting
fixture passed seven additional checks using a private authenticated HTTP runner;
that fixture executes no Docker workload. A later Python-only snapshot preserved
all Rust, SQL and Cargo input bytes and passed 178 tests on each platform
(174 passed / four skipped on Windows; 177 passed / one skipped on Ubuntu).

The separately built images are recorded with their immutable IDs and source
snapshots in `container-builds.json`. Server and runner use snapshot `5bf424b5`;
the guest-profile provisioner uses snapshot `883f43ef`. Current working source
also contains later host-admission and API-cluster work, which is not covered
by the schema-54 runtime reports. The distinct schema-52-to-54 binary lifecycle passed 22 observations
covering 18 distinct checks and 22 actual management operations. The
manager-to-VM execution acceptance remains unqualified.

The combined schema56 candidate is recorded separately under
`.internal/server-cluster-2026-10-09/`. Snapshot `ebc72bcc` passed 136 normal
Rust tests, Clippy for all targets with warnings denied, both native builds,
22 real PostgreSQL test groups and 15 native process checks. Its native server
SHA256 is `0e71642eaa1e8660514c19ea69ab77c7fc87e5dd8dcc5e084816b66d1e10abbb`.
Later Python-only snapshots preserve those runtime inputs. The full
three-API matrix passed 16 observations in 89.521 seconds, including private
mTLS identity, one-use callback tickets, durable SSE replay, current user and
credential revocation, executor reconnect, unknown outcome refusal, private
partition, bounded backpressure, exact ownership-lock loss, SIGKILL/reconnect
and SIGTERM/held-stream drain. Snapshot `6cd1f2fa` records the actual executed
harness hash. Its fake executor only exchanges protocol messages and bytes;
this result does not qualify physical Linux Runner/desktop adoption or enable
the production HA flag. The earlier failed fixture report is preserved.

Current working-source additions are separately staged before testing. A
success for schema54 does not qualify schema56, the newer operator client,
the canonical parent-VM capacity ledger, or physical Runner cancellation.
Temporary VM test failures are recorded outside private state; only exact
owned fixture processes/resources are stopped or removed. The last completed
inventory and fixture cleanup checks preserved the original 39 container
identities and image IDs.

## Schema59 execution core

Current qualification is recorded under
`.internal/server-execution-core-2026-10-09/`. The frozen runtime snapshot
`7c5c4adc` passed 186 normal Rust tests, Clippy with warnings denied, both native
builds, 30 actual PostgreSQL groups and 15 native process checks. The native
server SHA256 is `8ea2c75b7367a924fe0fffa9a986d2e04dbae1730707ccc6d31a082d2bfab6ff`;
the native Runner SHA256 is
`55691d838e62683d55de8e26a3e50debc5d26d290b64ee46d6b2c82bacaba60a`.
A separate existing V1 local-daemon/device-agent snapshot passed 60 Linux adapter
tests. It verifies compatible execution; it does not claim adapter resume wiring.

Resume uses the latest immutable, server-originated safe checkpoint, binds the
original run specification and cumulative budgets, checks current creator access,
and requires exact prior-attempt physical stop before requeue. Unknown model/tool
outcomes cannot fall back to an older safe checkpoint. Private keys, model API
keys and raw model endpoints are excluded from checkpoint contents.

The Python-only `42a0d9e8` revision preserved all 177 Rust, SQL and Cargo inputs.
Four full Python suites passed on both platforms: Windows 311 tests
(269 passed, 42 POSIX skips); Ubuntu 311 tests (310 passed, one Helm skip).
Later Python-only Runner/HA harness revisions preserve those runtime and image
inputs. The actual Runner result below records its executed `d6864f9a` helper
snapshot; the local lifecycle records `ad23e6fb`.
The latest helper qualification (development artifact omitted from public sources)
records snapshot `8ed2fe40` with all 177 runtime inputs byte-identical to the
qualified runtime and image snapshots. Runner, Linux desktop HA and API-cluster
helper suites ran 92 tests per platform: Ubuntu passed all 92; Windows passed
88 with four platform skips. These pure helper tests do not execute a physical
HA or hosted lifecycle fixture.

Local image builds from `42a0d9e8` succeeded and retained the original 39 running
container identities and images. Exact candidates are:

| Role | Local immutable image ID |
| --- | --- |
| Server | `sha256:5feb76cecd0efce3116cb731fdbead7200053ba48a824e637b40c3ebd04705f3` |
| Runner | `sha256:2d0e98cfadc1fdc708d85151627b97a881d56f0bc1e07e14685224b90eeb5656` |
| Provisioner | `sha256:3224cd511d636f33649ccb055eba4feb2265da8013b84881ab7533ca6d8970f0` |

These images are not installed or verified as published to a registry. The
production HA capability remains false, and no live-fleet migration is claimed.
Earlier failed Runner fixtures and their guarded cleanup evidence remain
preserved; they were not replaced by a blanket success claim.

The actual Runner report (development artifact omitted from public sources)
passed all eleven checks using the exact qualified native Runner binary and a
fresh scoped Docker fixture. It exercised physical creation/binding, refusal
of create replay and foreign/tampered proofs, signed RFB traffic, lost creation
and stop responses, restart/reconciliation fences, delayed POST after Stop57,
and a long-running raw job whose cancellation stopped future workspace writes.
The owned resources were removed and the original 39 containers preserved.
This fixture has no manager and does not qualify physical cross-API adoption or
authenticated server resume.

The schema59 three-API report (development artifact omitted from public sources)
passed all sixteen checks in 78.118 seconds with three native API processes,
fresh PostgreSQL and private mTLS. Its fake executor exchanges protocol messages
and bytes; it executes no tool or GUI. This confirms the tested replay,
authorisation, forwarding, reconnect and ownership-failure paths, while the
physical 23-check Linux Runner HA matrix was qualified later on the frozen current candidate (see below).

The local schema52-to-59 lifecycle report (development artifact omitted from public sources)
records 22 observations, 18 distinct checks and 22 management operations on a
fresh tenant. It covers real legacy/chat jobs, encrypted backup, tamper refusal,
fresh restore, candidate canary, promotion, rollback and exact deletion.
The actual source and transition executable SHA256 values are respectively
`75db4fa80100be86d17af2e139aa83bbc2139345d2eeb72c253f795ebbcd56a7`
and `c4f5d72d90c5d28f3135fdaaa868d138276153f125cef47f1773e8e1629160d1`;
this is a true binary/schema transition, not label-only repackaging. Runner59
remained throughout and successfully accepted Server52's retained legacy job
envelope. Canary probes are directly signed provisioner jobs, so this lifecycle
does not independently qualify Worker59 lease-stop or Resume59 authority.
The original Root wrapper lost the harness exit code. Recovery preserved the
original log and recovered report after a path-validation fix; the source PASS
and cleanup line, distinct ELF hashes, tenant deletion and exact original39
preservation were verified. An independently retained exit0 is not claimed.

The installed fleet inventory (development artifact omitted from public sources)
captures public metadata for only the original 39 IDs on `desktop-linux`.
Compose labels establish seven API, seven worker, seven Runner and seven egress
services plus the listed control/infrastructure roles. The subsequent read-only management-DB metadata check establishes seven instances
owned by four organisations; the legacy instance table has no host column. Twenty-five containers have no configured CPU
ceiling, twenty-five have no memory ceiling and all 39 have no PIDs ceiling.
The finite container ceilings sum to 35,000 CPU-millis and 60,928 MiB; these are
neither reserved host capacity nor measured usage, and the unbounded containers
prevent treating that sum as a fleet maximum. This records rollout work still
required; it does not certify or migrate the installed fleet.

The full hosted schema54-to-59 VM lifecycle remains outstanding. Parent VM
capacity planning, read-only metadata adoption of the known stopped pilot and
retained charges must be qualified together with manager-to-guest execution.
The earlier 6 GiB QEMU address-space bound produced an observed thread-creation
abort. At 8 GiB QEMU stayed alive during diagnosis, but guest readiness did not
pass; this diagnostic result does not qualify tenant execution.

Docker Desktop was temporarily unable to start after a WSL `CommandTimedOut`
during unmount. The targeted recovery receipt (development artifact omitted from public sources)
records successful `docker desktop restart --timeout 60`, the unchanged Engine
ID and all original 39 IDs/images running afterwards. No global WSL shutdown or
data reset occurred. Continuous uptime is not claimed. The hosted lifecycle,
physical HA matrix and installed rollout remain open after this recovery.

## Current candidate and remaining physical acceptance

The frozen `dbf10bc3` runtime supersedes the earlier native binaries for new
qualification. It passed 192 Rust tests, Clippy for all targets with warnings
denied, both native builds, all 30 PostgreSQL groups and 15 native process checks.
Its native server SHA256 is
`4b862779e3219633d80386d62d246e2a2494d77ba7fe729f6ab64741f2e3ab72`;
the native Runner remains byte-identical to `55691d83â€¦`. The old exact executables
and qualification reports are preserved separately. The six added production
module tests cover a cancellation-safe reader that retains partial private frame
headers and bodies between `select!` iterations. This independently reproduced
bugfix does not establish the cause of the outstanding real RFB timeout.

The new native API protocol matrix (development artifact omitted from public sources)
passed all 16 checks on this exact server in 77.071 seconds. It uses the same
protocol-only fake executor limitation as the earlier matrix.

All eight Python domains were independently checked on the frozen runtime stage:
Windows passed 364 tests with 100 platform/dependency skips; Ubuntu passed 463
with one missing-Helm skip. Windows User-Site was disabled, causing two additional
PyYAML dependency skips. These isolated tests may open their own Python HTTP/TLS
listeners and QMP stubs; they start no actual manager, Runner, VM, Docker workload
or PostgreSQL. The subsequent Python-only `62dc3d16` revision preserved all 177
Rust/SQL/Cargo inputs and passed eight Runner/HA/API/resume suites. It fixes an
exclusive marker creation collision and retains the original file safety checks.

| Role | Current local immutable image ID |
| --- | --- |
| Server | `sha256:34b900de76238d66c2942056557d768d3ebcaa5d87a2736ceba68958a967bb73` |
| Runner | `sha256:8d09f1c091d23abb7ce363c16800cfd2a7ace61cb4ba8a659054c98c65e47a7f` |
| Provisioner | `sha256:19d9cf8f98d2fb86b0c5b1b28675aebcd51f1f8ca4971a0a602b00398e529b54` |

These are local builds from the qualified runtime inputs. The
current local schema52-to-59 lifecycle (development artifact omitted from public sources)
passed 22 observations, 18 distinct checks and 22 management operations with
these exact three image IDs and an independently retained harness exit code 0.
It verified real jobs, encrypted backup and tamper refusal, fresh restore with
workspace/artifact/chat data, credential rotation and explicit schedule rearm,
canary update, promotion, rollback and exact tenant resource deletion. The source
ELF SHA256 was `75db4fa80100be86d17af2e139aa83bbc2139345d2eeb72c253f795ebbcd56a7`;
the target was `c9432810ffbd12246b3fe56af36dc8820cf3de990deed6954579167d10b6c9da`.
All owned Docker resources were removed; private evidence was retained. All
original 39 IDs, images, running states, start times and restart counts matched
before and after. This qualifies the tested fresh local lifecycle, not installed
fleet migration, central admission, remote lifecycle, HA or authenticated resume.

The earlier physical HA attempt on the old native server passed twelve checks,
including actual framebuffer pixels, normal SIGTERM drain and the bounded slow
reader. It then failed in an isolated fault injection missing the workspace
contract acknowledgement. That fixture-only acknowledgement is now corrected.
The new physical attempt (development artifact omitted from public sources)
passed three checks but timed out before the first RFB byte on the healed third
stream. Its exact Docker identity and Worker lease were live, and independent
in-container checks obtained genuine banners from both configured RFB ports.
This narrows the failure to the tested streaming path; it does not prove a
particular product or fixture cause. All five cleanup proofs passed, owned
resources were removed and private failure evidence remains retained.

The subsequent four-check owner-partition diagnostic (development artifact omitted from public sources)
passed with the current native binaries and Python-only snapshot `18982e80`.
Three actual upstream 101 upgrades forwarded genuine RFB traffic in both
directions. All five cleanup proofs passed, the owned root was removed and the
original 39 container identities, images, start times and restart counts were
preserved. This diagnostic does not qualify the full 23-case matrix or establish
the cause of the earlier intermittent streaming failure. The same snapshot
preserves all 177 runtime inputs and passed four helper suites totaling 150
tests per platform, with explicitly recorded platform skips.

The full physical Linux desktop HA matrix (development artifact omitted from public sources)
passed all 23 distinct checks on frozen Python snapshot `18982e80` and native
runtime `dbf10bc3` in 610.791 seconds. The separately retained supervision record
confirms exit 0, no timeout and a reaped harness child. All five cleanup gates
passed; no owned containers, networks, volumes or pending creates remained,
and the original 39 container identities, images, start times and restart counts
were preserved. The report SHA256 is
`28869596b7f3bbc23a49363e9a1037182d71090fb0af95c25056304df1aa29e1`.
This covers real HTTPS Runner jobs, RFB traffic, API ownership takeover,
authorisation revocation, cancellation and loss of responses in the documented
private fixture. The model is a synthetic provider and public WSS uses private
TLS front proxies. Production HA capability remains false. Working-tree changes
after these frozen sources require their own qualification.

The actual resume attempt (development artifact omitted from public sources)
started authenticated API, Worker and real HTTPS Runner, then executed one
actual Bash job. The intended next-unsafe-checkpoint fault barrier was not
observed. The resume authority, same-workspace reuse and no-replay acceptance
remain unqualified. All five cleanup proofs passed. Earlier prestart failures
remain evidence and are not counted as runtime acceptance.

The private-copy checkpoint diagnosis (development artifact omitted from public sources)
passed a fixed read-only numeric query after recovery of a separate private
PostgreSQL copy. The copy was stopped and all owned children reaped. The
original 1,753 files remained byte-identical and its database was never started.
The exact Run, Worker, lease and tool-call bindings match; the expected completed
tool-message predicate is false, so the fault trigger never blocks. The
remaining subpredicate is under investigation. This diagnostic does not qualify
authenticated resume, workspace reuse or tool no-replay behavior.

The failed hosted fixture had booted its real Ubuntu guest with AppArmor
enforcement, but manager-to-guest tenant creation did not pass. Its own parent
resources were removed. Its stopped VM was subsequently
retired through the exact identity-bound controller (development artifact omitted from public sources):
24 positive entry deletion witnesses plus a root witness, fresh read-only root
process audits, tombstone and ledger CAS all passed. That retirement reached
pool revision 6 with only the unchanged adopted pilot charged at 4,500
CPU-millis and 4,608 MiB; the later fresh attempt below added its own allocation.
All original 39 container identities, images, start times and restart counts
were preserved. This releases only the physically deleted test VM's allocation;
it does not increase the parent budget or move an installed tenant.
The historical process audit covered TGIDs. A later source revision adds
all-UID per-thread FD, mapping, cwd and executable checks, including unshared
thread FD tables; 30 isolated Ubuntu tests passed. Its actual use for
the later test VM is recorded in the separately verified retirement below.

The
fresh hosted schema54-to-59 attempt (development artifact omitted from public sources)
passed four bootstrap, parent-capacity and guest-kernel/AppArmor checks. The
current Server image was imported through the private registry and its immutable
configuration/rootfs identity verified. The next Runner-image Docker command
failed; the old generic failure code cannot distinguish pull from inspect and
does not establish a registry, TLS or network cause. No manager-to-guest tenant
lifecycle acceptance is claimed. The owned QEMU process exited 0 and the private
VM disk and evidence remain stopped and retained. Parent control cleanup
removed only five stopped containers, one network and seven volumes (development artifact omitted from public sources).
It preserved the original 39 containers, the pilot, the VM metadata and the
capacity ledger. Pool revision remains 7, holding 9,000 CPU-millis and 9,216 MiB
for the pilot plus this stopped test VM. Guest persistent-data absence, VM
retirement and capacity release are not claimed by parent control cleanup.

The later stopped `de666e06-90f1-437d-8e4d-0f216c5ef3f0` VM was
retired with the frozen per-thread auditor (development artifact omitted from public sources).
Frozen Python snapshot `f5ee1fc8` preserved all 177 qualified runtime inputs
and passed 155 relevant isolated tests. Fresh root audits ran at prepare,
before deletion and final phases. The controller moved the parent pool from
revision 7 through 8 and 9 to 10; 28 positive entry deletion witnesses and the
physical root witness confirmed removal before releasing its allocation.
Root and quarantine are absent, the anchor matches and no WAL remains.
Revision 10 retains only the unchanged pilot at 4,500 CPU-millis/4,608 MiB.
Original 39 containers, pilot metadata, prior retirement and retained private
fixture evidence were unchanged. This is physical deletion of exactly the owned
failed VM, not proof of its guest's persistent-data inventory or installed-fleet
migration. A subsequent new qualification VM receives a fresh retained charge;
revision 10 is the verified retirement result rather than a perpetual pool value.

The installed instance scope check (development artifact omitted from public sources)
establishes seven instances in four organisations. The current host model binds
one organisation to each trust domain and requires distinct daemon identities.
Schema53 keeps instance host assignment immutable, including `NULL â†’ UUID`;
the existing encrypted restore contract binds the same instance and organisation.
A real fleet move therefore requires a separately implemented and qualified
same-organisation import/cutover contract, finite whole-host resource planning,
current authorisation, physical source fencing and a bounded rollback policy.
No blind host rebinding, capacity increase or installed-fleet migration occurred.

Full hosted lifecycle and installed rollout remain open. Authenticated resume passed in the bounded private scope above.
The full physical HA matrix passed within the frozen private scope above. Production HA capability remains false.

The [fleet sizing and import contract](SERVER_FLEET_MIGRATION.md) records the
seven-instance/four-organisation plan and its capacity inequalities. Four new
organisation domains cannot fit the unchanged parent budget even after exact
retirement of only the owned failed VM. Migration implementation and physical
cutover remain open; no host binding, installed data or budget is changed.

The new same-organisation instance-import source candidate implements durable
source/target locks, original backup provenance, fresh target credentials and
execution generation, blocked replay of imported old runs, and an explicit
revisioned source-to-target route. Its
preserved candidate report (development artifact omitted from public sources)
records six Rust contract checks and 80 passing Python checks, with four
platform skips. The Windows server check remains failed at the Git Perl
dependency; that historical report is unchanged.

A subsequent actual Ubuntu/WSL `cargo check --locked --offline -p cowork-server`
passed with exit 0 and a completed outer launcher. The
separate Linux typecheck receipt (development artifact omitted from public sources)
has SHA256 `3cbeac1051cc077d3d781fed8760f50372fcc5e3f8cc481a29b6512b1086b06c`.
All 21 candidate sources still match their previously sealed manifest and are
copied with the receipt. This is a typecheck result, not a new full runtime
qualification. At that historical candidate revision the private PostgreSQL import contract, current runtime
regressions, original encrypted backup access and actual complete data import
between two owned engines, management UI/route client, and physical cutover
remain open. No installed instance, live database, host assignment or capacity
budget was changed. The frozen earlier runtime and HA receipts continue to
apply exclusively to their original snapshots.

The subsequent historical Manager import acceptance (development artifact omitted from public sources)
seals 200 then-current source inputs without drift. SHA256:
`496af97a8b2a5e12eb07829015cac67e608ed5ef707efa18bf80021957076cf7`.
Two actual private PostgreSQL contracts pass, including 55 checks through the
real authenticated HTTP router. The four Server/Runtime/Runner/Contracts
packages pass 199 ordinary Rust tests (33 ignored), all-target Clippy with
warnings denied and the explicitly limited import formatting gate.

Two actual HTTP regressions were reproduced and corrected. Prepare replay now
returns its original immutable operation receipt. Prepare, Cutover and Abort
revalidate the exact session, bearer token, user and device within the existing
transaction after lock waits, after audit serialization and before commit.
The 55 checks include 18 real advisory-lock authority changes, six natural
expiry rollbacks after tentative writes behind the final audit lock, and three
real HTTP logout/session-row wait order cases. Rejected requests leave import,
route, allocation, operations and audit effects unchanged.

All own PostgreSQL, API and Windows WSL launcher processes have positive exit
or join witnesses; UUID schemas and own temporary clusters were removed.
The test uses synthetic operator host/allocation/release prerequisites and
seeded completed import phases. It does not prove original age-artifact access,
physical transfer between two engines, workspace parity or installed migration.
At that historical revision the global workspace format gate failed separately;
limited import formatting did not replace it. Later native and UI qualification
is recorded below. Full physical/system acceptance remains open.
The earlier candidate manifest changed and remains historical. No installed
tenant, protected pilot, host binding, live route or capacity budget was changed.

The sealed native import-interface snapshots pass 199 Rust tests (33 ignored)
on Windows and Ubuntu, global formatting and all-target Clippy with warnings
denied. The native Windows build uses a private hash-verified Perl toolchain.
The new Windows receipt (development artifact omitted from public sources)
and Linux receipt (development artifact omitted from public sources)
bind identical 175 core inputs and 244 effective global inputs; their positive
process waits are recorded separately. Earlier failed native attempts remain
unchanged and do not count as passes.

The import frontend qualification (development artifact omitted from public sources)
binds 535 inputs with no drift and passes 707 tests, TypeScript, style checks,
the 79-schema/160-operation/N-1 contract check and web build. The previous
152-operation generator failure is preserved. Only current generated OpenAPI
was repaired; the original V1 archive and its separate 159-operation private
candidate remain unchanged. The interface persists the exact request/key before
POST, retains lost acknowledgements through reload, requires explicit exact
retry and separate cutover consent, binds receipts to server/user/organisation,
and never automatically starts the target or resumes schedules. Older servers
hide the import tab unless their organisation response advertises support.
These frontend tests use fixtures; they do not establish joint runtime acceptance.

The target-readback acceptance after the final-readback repair (development artifact omitted from public sources)
binds 254 common Rust/Python/Dockerfile/OpenAPI inputs without drift.
SHA256: `13182deb7b50e539dfe0002b61cd8dd40e736bf5311343359541c75dd39bebf7`.
It passes 55 actual authenticated HTTP checks, the Server SQL contract and an
actual private PostgreSQL late-mutation test, 12 real-age cases and
18 target-readback cases. The complete helper suites pass 333 Linux tests
(two skips) and 288 native Windows tests (47 skips). Every own PG/API/WSL process
has positive wait/join evidence and the private PG fixtures were removed.

Import reads the original bound cipher/manifest, actual complete object and
workspace contents, resource generation and private credential comparisons
before active.json and again after qualification. Imported original Run IDs and
epoch are checked with a negative activation probe. Legacy backups lacking the
strong original import provenance are rejected before target writes; ordinary
same-instance restore compatibility is retained. Docker COPY/module graphs were
checked, without claiming an image build. Physical two-engine transfer and
service-side rejection of original Source DB/S3 credentials remain open.

The final readback repeats the stopped-state, imported-history and negative
activation checks after object/workspace reads. Reproduced late changes and
previous-target credential reuse are rejected. Provisioner writes the private
Fleet key file used by that comparison. This receipt covers the readback repair;
the subsequent generation-bound Fleet implementation has its own qualification
below. Earlier receipts remain tied to their own bytes.

The owned Chromium component exercise (development artifact omitted from public sources)
passes six groups covering acknowledgement loss, reload without POST, exact
explicit retry, separate route-switch consent, stopped target and explicit
read-only status. The compact 390px view has no overflow or scoped automated
axe violations. Its native owning-job receipt (development artifact omitted from public sources)
records natural Node/Chromium exit and ten total processes with zero active
processes before job close. This exercise uses a synthetic HTTP manager; it is
not joint Rust/PostgreSQL acceptance, physical import, full WCAG or NVDA proof.
The actual-router Chromium exercise (development artifact omitted from public sources)
passes the same six groups through the real ManagementClient, authenticated Rust
router and private PostgreSQL. SHA256:
`417c02c1f980382cd75076f05708943a549a8a52cfe344e2ce2457a67b3d20df`.
After the server commits one import and one operation, the proxy deliberately
truncates that response. Reload sends no POST; explicit retry sends identical
raw request bytes and returns the original receipt after later phase changes.
Separate consent commits revision 6 / route revision 2, with source retained
and target stopped. Qualification phases are explicitly private synthetic
operator state; this still does not prove physical import or a full V2/Server
system. The native owner records 45 total processes and zero active processes,
natural Node/Chromium exit and no forced cleanup. The separate backend receipt
records API join, native WSL/Rust/PG exit and private schema removal.

The combined immutable interface bundle (development artifact omitted from public sources)
contains 689 verified union source inputs and 372 witness files, including the
failed earlier browser attempts. SHA256:
`175b09e5d2138b373c617dc29ff77dce99be5f72bb0a4079423c3d53232389e7`.
An earlier access token naturally expired after 15 minutes; its 401 attempts
remain failed. A fresh fixture used normal session creation without extending
policy lifetimes. Subsequent Fleet generation changes require new qualification
and do not inherit this bundle's passes.

The subsequent Fleet epoch acceptance (development artifact omitted from public sources)
binds 255 unchanged inputs, with archive CRC and every entry hash verified.
SHA256: `7327173e2d3f517a5c362c300003ce381cf5e7e8ecfbc4794432acaac8946616`.
HMAC v2 binds organisation, instance and execution epoch. Current authority is
read under a PostgreSQL row lock retained through the caller's transaction.
Candidate operator envelopes bind the exact operation, lease, host and epoch.
Migration 0061 freezes each reservation's original key version: an old exact
v1 completion survives enrollment, and an old exact v2 completion survives
promotion/deletion. Neither key grants a new reservation in the new generation.

Four actual private PostgreSQL cases, including 14 reserve/release HTTP checks,
pass. They include a confirmed lock wait followed by a changed epoch, immutable
receipt replay and required stopped confirmation. Ordinary Rust regression
passes 200 tests (35 ignored); current import SQL, 55 import HTTP checks and the
Python target-PG race pass separately. Python passes 335 Linux tests (two skips)
and 290 Windows tests (47 skips), plus 12 actual-age cases. Every own PG/API/WSL
process has positive exit/join evidence and private fixtures were removed.
Enrollment/capacity prerequisites are synthetic. This seal does not claim
current Launch/Content HTTP, a rerun UI browser, physical stop or installed
migration; those remain separate required checks.

The subsequent Launch/Content HTTP acceptance (development artifact omitted from public sources)
keeps those same 255 product inputs unchanged and passes 39 actual HTTP checks
with five observed product PostgreSQL row-lock waits. SHA256:
`e8c4f94aed025996cb3b3f0a514978e2b11b5ad85e0158a4f937b0676f21d314`.
Normal Management sessions exercise member/viewer/support tickets, content and
current-access callbacks. A separately owned tenant router accepts `/auth/fleet`
through the actual Management context callback. Old/foreign tickets and keys,
including nonce replay, are denied. A waiting launch signs with the post-wait
epoch; an old runtime ticket fails against the updated post-wait context.
All own APIs, the retained tenant child, Cargo/WSL/native launcher and PG ended
positively; private schema and cluster were removed. Five failed fixture attempts
remain archived. Readiness/enrollment are synthetic and the two routers share
one private database; no physical runtime or installed migration is claimed.
The current combined Fleet/interface qualification (development artifact omitted from public sources)
binds 755 unique unchanged source inputs and 299 witness files. SHA256:
`3445e6d66714b0d24e290cf5f0c533a2b16e0771cec830b02356ce5aba0a85d3`.
Independent readback verified all sealed file hashes, all 755 archive entries,
their bytes and CRCs, and the current source files. Windows and Ubuntu each pass
200 Rust tests (35 explicit external-test ignores), format and Clippy; the
Windows native build passes. The frontend passes 707 tests, TypeScript,
contracts, styles and web build. The exact Fleet and 39-HTTP receipts above
remain bound without a product source change.

A fresh authenticated Chromium run against the actual Management router and
private PostgreSQL passes six groups. Exactly one import and one operation are
committed; the explicit identical raw-body replay retains the original receipt.
The separate route CAS reaches cutover revision 6 / route revision 2 while the
target remains stopped. The backend started after the native/frontend gates,
and the normally minted session kept its normal lifetime. The browser's own
Windows Job fell from 45 processes to zero naturally; browser/API/Cargo/WSL/PG
exit and cleanup witnesses are positive. Both recorded viewport images were
inspected. Existing UTF-16 BOM JSON and JSONC configs were retained unchanged;
three failed collector assumptions remain archived.

Operator readiness/enrollment/qualification prerequisites remain synthetic;
the browser does not execute the Python fixtures. This combined receipt does
not establish physical Engine import or stop, actual enrollment acceptance,
installed migration, full V2/server acceptance, NVDA/WCAG or a production release.

## Production instance gateway source delivery (2026-10-12)

The Compose production gateway is now supplied in
`deploy/enterprise/docker-compose.production.yml` with a strict renderer at
`scripts/prepare-enterprise-gateway.py`. It prepares the management HTTPS origin,
one separate HTTPS origin per original instance, existing-certificate/key mounts,
fixed UUID routing aliases, SSE/WebSocket forwarding and matching management
launch environment. The Rust launch-origin adapter verifies the paired domain
and template before management connects to the database. Launch-ticket,
organisation/member/viewer/support and subsequent content authority remain in
the existing server paths; the gateway does not adopt their credentials.

The paired preparation now includes a complete copied provisioner configuration.
Its typed `instance_gateway` profile propagates the same actual instance origin,
optional hostname-specific passkey RP and own-origin/Desktop CORS through API
manifest construction, guest adaptation, restore, update and rollback. Saved
manifests are checked before a start/recovery; cleanup retains its original
authority. Cookie domain is not widened and management/parent RPs are not reused.

The preparation receipt binds the rendered files and producer sources and marks
deployment/runtime/production qualification false. DNS, certificate issuance,
trust, installed images, actual authenticated routing, revocation and reconnects
are independent operator acceptance. This source delivery does not deploy the
current fleet or qualify remote/hosted engine transport.

Source checks for this delivery: Python AST, Compose YAML and the bounded
in-memory template contract passed; the changed Rust files parsed under rustfmt.
The Windows offline `cargo check -p cowork-server` did not reach the server check:
vendored OpenSSL first lacked Perl on PATH, and the installed Git Perl then
lacked `Locale::Maketext::Simple`. The complete private Perl subsequently passed
that prerequisite. Later isolated attempts retained native failures: MSVC PDB
errors, a generated-file C1083 on a long target path, and `install_dev` demanding
the PDB omitted by `/Z7` after a short-path C compile completed. The subsequent
Windows Rust 1.95/MSVC release-profile check passed against an immutable copied
235-input Gateway cohort (`cargo check -p cowork-server --offline --locked
--release`, jobs=1, default stack, no CFLAGS override). Its manifest SHA is
`7caaab32d1716de16dfaaeda169d3f90d4d01dfcb640b95e96af1856ecb33306`;
the terminal log SHA is
`f6ba1f517a20bcaa84435151a09645e7579c59c16416a8c6c07f00074f0bba77`.
That source cohort includes the Origin/generator/restore leaves but predates the
separate provider-assurance addition below. It is no live runtime acceptance.
No dependencies were installed or build
requirements relaxed. A real Nginx syntax/runtime check remains unclaimed because
no Nginx validator was available on the checked Windows/WSL paths. No production
configuration or certificate artifact was generated from live operator material.

## Optional provider-assurance source delivery (2026-10-12)

Migration 0062 and `oidc_assurance.rs` add optional explicit operator mappings of
already cryptographically verified typed OIDC `acr`/`amr` tuples. Provider identity
must match exactly; signed `auth_time`, issue time, token expiry and the bounded
operator age limit are checked. Unknown/missing tuples provide no elevation.
Native one-use codes and sessions carry separate original user/identity and policy
revision/epoch provenance. Current management and linking gates additionally
check scoped provider/SSO revisions and membership; local factor verification
retains its original enrollment and twelve-hour rules. Refresh does not renew
provider verification. Existing installations with no mapping preserve None and
legacy provider fingerprints.

Policy startup registration is initial-only. Local `oidc-assurance inspect`,
`disable` and `activate` commands require an explicit expected revision for changes,
retain immutable operation receipts, rotate epochs and commit audit atomically.
No tenant API sets assurance or operator configuration. Existing UI/API contracts
show provider verification/expiry and offer an approved fresh sign-in when
available; Compose/Helm source profiles and ENTERPRISE_OIDC.md cover setup.
This source addition has not run an external-provider, migration/live-session,
installed-image or production acceptance. Continuous directory sync was a
separate remaining product path at this receipt; its new source slice is below.
Broad organisation MFA policy remains separate work.

Narrow source checks passed: Rust syntax/rustfmt of the affected leaves, syntax
transpile of the three TS/TSX leaves, plain YAML/JSON parsing and default/explicit
global-SSO Helm rendering with dummy values. These checks are not a full UI
typecheck or an external-provider/migration/session acceptance. The separate
current Windows Rust 1.95/MSVC source check also passed (`cargo check -p
cowork-server --offline --locked --release`, jobs=1, default stack, no CFLAGS
override) against a separate immutable 248-input snapshot. Its manifest SHA is
`e98590c74f4857bd0aabb8705e713f3d778cbd81334ee5c89c41f42c999ad806`
and terminal log SHA is
`e1ecf640c6f3e23a57b767467bca6c86bb0f91eaa7f899b1a5043d6a60cf1c2c`.
All working source pins matched the compiled snapshot after terminal success;
this paragraph only records that outcome. No live schema or policy changed.

## Directory sync source delivery (2026-10-12)

Additive migration0063 and `management/directory` supply an opt-in, original
tenant/provider-bound SCIM provisioning service and continuous read-only LDAPS
worker. Actual management API/UI, operator catalog and inspect/activate/disable
CAS, private credential mounts, per-connector envelope domain, original encrypted
once responses/cursors, group contributor provenance, role projection and
provider-origin session/code withdrawal are connected. Fresh source records are
required after policy/SSO/settings/operator epoch changes. Preview does not create
accounts or adopt memberships. Local/manual/other-provider records are retained;
email never reconstructs an external binding. No platform-admin or content-reader
grant comes from a directory group.

Existing login origins remain NULL on upgrade. The separately confirmed exact
legacy inventory may sign out the listed provider-bound users' unclassified old
sessions/codes, including their local old sign-ins. Its original receipt describes
that user-wide containment scope. Classified local/foreign-provider sessions
remain. User-row synchronization and insertion guards apply only to explicitly
contained/new directory-owned users; mixed old replicas fail closed for them.
Directory-managed membership writes derive only current source contributions.

LDAP pages, frame/byte/depth bounds, leases, encrypted cursors, deadlines and
backoff are implemented; a complete scan additionally point-checks each missing
immutable subject before retirement. Failed or partial scans do not prove
deletion. Only exact operator LDAPS configuration is contacted; referrals and
remote mutation are rejected. `ENTERPRISE_OIDC.md` covers the supported bounded
SCIM subset, textual subject/canonical DN mapping and operator installation.

The final immutable263-input cohort passed Windows1.95/MSVC native release
checking (`cargo check -p cowork-server --offline --locked --release`,jobs1,
default stack, no CFLAGS override). Manifest SHA:
`39c2ed33380095c040d54940d54dcf84ceb7457699a9fccb0e27c827df7a89a1`;
terminal log SHA:
`6a114b50e1989fae10dfc3897b56902993e9ef2b24de9426f42a425adc70d08f`.
All copied and actual working source pins matched at terminal completion. The
lock adds14 packages for exact ldap3=0.12.1/Rustls; existing package versions and
checksums are unchanged. Four frozen UI/API leaves passed TypeScript syntax
transpile (not a full typecheck), and operator JSON/Compose YAML parsed.
PostgreSQL16.15 actually loaded migrations0001–0063 in a private standalone
backend without a TCP listener and created all11 Directory tables. Final0063 SHA:
`4b905a140905db4dc8cb084a066955733d495626dc8ed9abaf1809ecc1dacdf2`;
DDL log SHA:
`59fd202041a924fce337033f4ff3927b135032d3e2c02577f8bd30ec523b2180`.
These are compile/syntax/DDL checks. No live Directory, installed migration/image, external SCIM/LDAP,
mixed-replica race acceptance or production qualification is claimed. Historical
Gateway/IdP snapshots and their receipts remain unchanged. Broad organisation
MFA policy was subsequently implemented in migration0064; its source delivery is
recorded below. This Directory receipt remains unchanged.

Organisation MFA source delivery (12 October2026): current organisation policy,
operator CAS and original operation receipts, current session/IdP assurance,
HTTP/SSE/download, device, worker, model, schedule and terminal fences are wired.
Fresh same-run continuation records preserve the original authentication session.
Existing organisation policies remain disabled until explicitly configured.
The final197-input source cohort passed Windows1.95/MSVC release checking with
the default thread stack, contract generation and canonical contract fixtures.
Diagnostic frontend TypeScript checking passed. PostgreSQL16.15 loaded all64
migrations in a new private single-user database and created the five MFA tables,
instance version column and both terminal session columns. The owned fixture was
removed after backend exit. The integrated receipt is
delivery.json (private development evidence; omitted from public sources),
SHA `25ba7d29e1d81cffa8a3c6b06f3ce36801ab666e5e96fea5235090ef3f7e911e`.
The explicit UI-label and schedule-revision source fixes are bound separately;
historical receipts and failed helper/check attempts remain intact. Real IdP/MFA,
mixed-replica races, installed images, upgrades and production operation are
still unqualified. This source delivery does not constitute release approval.

## Audit and certificate operator source delivery — 13 October 2026

The previously missing implementation paths for server items74 and77 are now
delivered as runnable operator/source functionality. Migration0065 is additive;
the original0064 MFA migration remains byte-identical at
`23cc60bb9d83d997c7ca3d62df164a97bfe43beaaf38e69ddd0455338c72a386`.
Historical MFA/Directory/runtime cohorts and their receipts remain unchanged.

Management now exposes authenticated organisation-scoped checkpoint, frozen
export, original operation inspection and retention completion routes. A plan
binds its original request, checkpoint CAS, retention policy, authenticated records
and export digest. Destructive completion requires current administrator authority
and an independent RSA witness receipt; every original row is rechecked under the
ordinary audit writer lock immediately before deletion. The last removed original
digest becomes the retained signed prefix checkpoint, and an authenticated
retention event preserves continuation even when the whole older prefix is
removed. Existing audit views and future exports verify that base. Retention uses
the authenticated event timestamps and configured `audit_retention_days`, with
bounded batches. Original final receipts remain recoverable on retry; older
export bodies, including abandoned plans, are compacted after their own retention
window during later successful completions. Original final receipts survive;
an expired uncompleted export must be prepared again with a fresh request.

`scripts/enterprise-audit-lifecycle.py` includes the real independent archive
witness and the operator plan/apply path. The witness durably archives the exact
export, signs its hash and prefix/head, rejects forks/rollback relative to its
previous head, and returns its original receipt on retry. Management receives
only its public key, with an optional Compose overlay. The external role requires
a separate host/storage/administrator trust domain; a collocated archive does not
establish independent protection. The initial witness baseline cannot establish
integrity for already deleted, previously unwitnessed history.

`scripts/enterprise-certificate-lifecycle.py` supplies Control/VM
plan/apply/reconcile/maintain/status plus daily systemd service/timer assets. It
generates actual matching, hostname/EKU-validated private certificates; binds
original configuration, secret bytes/modes, images, containers, revision and VM
identity; and retains original/candidate plans and exact completion receipts.
The Control adapter actually stops/recreates only selected digest-pinned TLS
consumers and verifies the new serving HTTPS certificate and readiness. The VM
adapter uses pinned-host-key SSH, original daemon/trust-domain and stopped guest
inventory, installs the exact new CA/server identity, actually restarts only that
guest's Docker service, verifies real rotated mTLS and identical retained guest
containers, and recreates its provisioner consumer with the new client identity.
The ordinary provisioner `operator.lock` prevents concurrent infrastructure
mutations. Read-only secret source modes are preserved for the non-root control
gateway under an owner-only parent. Unknown command outcomes remain pending for
physical reconciliation and never produce a fabricated success receipt.

The automatic `maintain` path renews within the configured window or reconciles
the original pending plan. A VM with running guest containers waits for normal
management maintenance; these are maintenance rollouts, not continuous-uptime
promises. The supplied issuance supports the existing private local Control/VM
PKI; public ACME and enterprise external-issuer integrations are outside this
implementation. Exact setup commands, config examples, external-witness services,
secret permissions, recovery and rollout gates are documented in the existing
[enterprise operator README](../deploy/enterprise/README.md).

Narrow verification passed: native Windows/MSVC `cargo check -p cowork-server
--locked --offline` with the already available private Perl toolchain; Python
syntax; focused Rust formatting; six Linux/OpenSSL archive/crypto/secret-mode checks; and
all65 migrations in a fresh owned PostgreSQL16 single-user backend without a
network listener. The DDL check created both audit tables and the checkpoint
default, then removed only its owned fixture after synchronous backend exit.
The source receipt (private development evidence; omitted from public sources)
records the final source pins and terminal checks. The
DDL receipt (private development evidence; omitted from public sources)
binds original migration hashes and actual transport/log hashes. These are
source/crypto/DDL checks. Real authenticated management retention races, external
witness deployment, Control/VM restart/renewal acceptance and installed production
rollout remain unqualified. No existing deployment, protected VM pilot, container,
Docker/WSL service, knowledge vault or Git publication was changed by this work.

## Bound VM and native Kubernetes operator source — 13 October 2026

Server items66 and67 now have concrete backend activation and execution paths.
Additive migration0066 stores immutable administrator plans, ten-minute original
challenges, current profile revisions and the exact backend snapshot on each
management operation. Activation rechecks current organisation administrator and
scoped host authority, original host assignment, instance/profile CAS and the
physical producer observation. Claims, recovery, final receipts and batch rollout
steps retain that original profile. Disabled/mismatching hosts or obsolete
operator contracts cannot acquire new work. Release qualification is bound to
the same current profile; a prior profile's canary cannot qualify its successor.

The actual Linux producer `scripts/enterprise-backend-profile.py` supplies
prepare-routing, plan, inspect, activate and pair-provisioner. Its VM inspection
reads the original owned QEMU/QMP KVM identity, hardware UUID over pinned-host-key
SSH, retained parent allocation/inventory, actual daemon mTLS/trust domain and
the serving HTTPS route. It generates a real own-host API-forward service and
exact-origin Nginx route. The paired operator uses that VM's existing read-only
shared state and source-pinned producer, and rejects a fixture-only profile.
Existing container adapters execute inside the actual bound VM for provision,
start/stop/resume, encrypted backup/restore, canary/promotion/rollback, enrollment
and verified deletion. The parent VM allocation is retained after instance
deletion. No protected pilot is adopted and no isolation string alone opens the
VM gates.

Kubernetes uses native Kubernetes resources and a dedicated Runner backend.
The operator binds real cluster/namespace/node UIDs, the original namespace
assignment, actual HTTPS Runner identity, immutable images and reviewed runtime
classes. Helm preflight freezes the actual chart source and rejects unbound
existing resources; original release/target UIDs and readbacks constrain updates.
The native Runner creates actual Pod/PVC resources and performs bounded exec,
workspace/artifact access, GUI/VNC and terminal operations. Desktop identities
use a separate v2 proof domain with original Pod/PVC/node/CRI IDs, create-ACK
resource version and fresh challenge readbacks. Docker v1 proof bytes remain
unchanged. API consumers additionally check their current organisation, instance,
cluster, namespace and storage generation, with explicit native proof opt-in.

Stopped instances keep the HTTPS control Runner physically quiescent behind a
durable signed scope barrier; API/worker/egress producers and actual work Pods
are stopped. Start activates that same scope through the original management
operation. Archives use real PostgreSQL, S3 and original PVC volumes. Updates
probe the candidate and preserved rollback source under a private maintenance
network boundary. Permanent deletion freezes original data/resource identities,
archives actual Pod terminal evidence, removes only UID-bound objects and data
principals, verifies CSI Delete/PV absence, and requires namespace UID/RV-CAS plus
absence readback. Explicit partial deletion recovery reuses this original plan,
including after its control Runner is removed. Unknown create/Helm outcomes are
held rather than replayed. The default service account remains bound to its
original UID until its namespace is removed.

Operator setup, current feature flags, original bootstrap identities, native
runtime prerequisites and commands are in the existing
[enterprise README](../deploy/enterprise/README.md). VM setup needs separately
budgeted real KVM hardware and an original registered guest. Kubernetes needs
reviewed Linux runtime classes, CNI/network isolation, actual measured PID limits,
CSI storage with Delete semantics, pinned Runner TLS, and original bootstrap
UIDs. HA data services, live authentication/replica races, physical VM/Kubernetes
lifecycle, upgrades and installed production acceptance remain deployment gates.
The bounded delivery evidence records source/compile/syntax and owned single-user
DDL checks only; installed, runtime-qualified and production-qualified flags
remain false. The frozen74/77 source receipt and original64/MFA197 history are
preserved. No existing deployment, container, protected VM, Docker/WSL service,
vault or Git publication is changed by this delivery.

Server item72 now has a concrete template/environment management and operator
path in additive migration0067. The console registers immutable digest images,
qualifies them on the assigned physical profile and manages persistent/shared
environments. Registration/qualification/actions retain original request IDs,
current admin/member authority, instance/environment revisions, original host
profile and leases. Effective CPU/RAM/storage/slot admission is serialized with
ordinary execution and instance lifecycle; stopped/uncertain resources remain
charged until actual original stop/storage release receipts arrive.

The VM adapter uses the existing positive physical VM profile, original pinned
SSH/mTLS assignment and a source-pinned root guest helper with fully allocated
ext4 filesystem/loop/mount/volume receipts. Native Kubernetes uses original
Pod/PVC/NodeUIDs, actual CRI termination, physical-stop finalizers, digest and
workspace challenges, actual PID limits and measured filesystem size. Host
capacity collectors require original private create ACKs. Template activation
requires a completed real bounded probe followed by confirmed stop and reference
release. Unknown outcomes are held, with explicit acknowledged-runtime recovery;
no name/label adoption or create/task replay is offered.

The independent parent-controller VM retirement producer/consumer signs only an
actual original stopped owned-QEMU retirement and its inode deletion witnesses,
bound to the consumer's original intent and separately enrolled key. It does not
free management capacity from node absence or certify unrelated backends.
Operator commands and material prerequisites are in the existing enterprise
README. Source, native compile, frontend typecheck and owned single-user DDL
checks do not constitute installed VM/Kubernetes production acceptance. The
original66/67 and74/77 frozen receipts and migration64 checksum remain preserved;
installed, runtime-qualified, production-qualified and published remain false.

Server source83/84 and the environment task bridge for73 now add migration0068,
current management-session/MFA authority, original controller/VM assignments,
encrypted QEMU disk snapshot custody, qualified Libvirt/KubeVirt/Hyper-V/attached
VM adapters and an explicitly pinned OpenShell v1 SDK path. The console selects
actual enrolled controllers, requests original VM power operations and manages
encrypted disk snapshots. Native controller start/stop checks physical state and
original UUID/RV; KubeVirt captures original launcher CRI termination behind a
finalizer before cleanup and cannot restart from node/VMI absence.

Snapshots require the positive owned QEMU parent and all original instance
producers stopped. Actual qemu-img checks/comparison, age recovery verification,
source/inode/SHA CAS, full scratch reservations and original VM restart/readbacks
produce the completion; unknown IO stays held. External PostgreSQL/S3 and parent
control data require their existing instance-backup paths. The additional VM
controllers accept only explicitly enrolled original metadata and actual hard
hypervisor limits, never a backend enum as execution authority. Protected/adopted
VMs remain outside their lifecycle.

OpenShell uses its actual authenticated protobuf SDK, pinned descriptor/client/
TLS peer, private sole-owner workspace, original physical VM and installed
gateway executable/listener/cgroups. Real workload/supervisor Docker IDs,
immutable images, aggregate CPU/RAM/PID bounds and actual admitted hard Landlock
policy are verified. Original physical container terminal facts are saved before
the SDK stopped barrier and cleanup. Auxiliary resources retain their SDK names;
capacity classification uses immutable original create-ACK journals. Installed
SDK release compatibility and full physical parent storage accounting remain
deployment gates. This is a source path, not evidence that an SDK/hypervisor was
installed or operated here.

The environment bridge journals real deterministic process and file operations,
with original workflow/step/request bindings, current epoch fleet keys and
source-instance workflow/transfer authorization before new IO. File operations
are <=256KiB, use safe workspace directory FDs, isolated Python imports, actual
kernel file leases and original SHA/inode CAS, and preserve conflicts under a new
original alias. Sandbox-owned output files cannot supply terminal proof; only a
durable operator result after the actual terminal RPC/CLI ACK is redelivered.
Fresh same-user management sessions may read old receipts without inheriting
dispatch authority. Stop blocks future IO; positively undispatched queued jobs
halt, while unproven claimed process stops remain held and charged.

The independent original-host identity exporter obtains the authentic current
execution epoch from the actual manager's host-scoped registry, verifies the
original VM/SSH/mTLS tuple and signs the exact export with a separately enrolled
parent RSA key. A durable original bundle handles lost output without minting a
replacement epoch. The existing72 retirement witness and original64/MFA197,
74/77 and66/67 receipts remain immutable. A followup corrects current audit and
backend-plan Operator public URLs to the actual `/management/v1` mount.

Current narrow evidence is under `.local/server-vm-backends-20261014/`:
native-result4 records an unchanged successful Rust1.95 MSVC default stack/profile
`--locked --offline --all-targets -j2` check of Server, Runner, Device-Agent and
Local-Daemon. Separate bounded Python AST, frontend typecheck and fresh owned
PostgreSQL16 single-user DDL receipts describe their own source pins. No testsuite,
live VM/cluster/SDK probe, installation, container mutation, publication or vault
write was performed. Own frozen83/84/environment-bridge sources are retained in
that directory's source-result receipt; later71/73/85 integration and final
installed acceptance require their own current source/build receipts.

The current69/70/71 native integration has a separate unchanged-input receipt in
`.local/enterprise-current-checks-20261014/native-result2.json`: Rust1.95 MSVC,
default stack/profile, locked offline all-targets checks for Server, Runner,
Device-Agent and Local-Daemon passed. The fresh owned PostgreSQL16 single-user
`ddl-result.json` in the same directory passed all71 original migrations and
actual table/trigger readbacks, including the twelve70 workflow tables and six
enabled70/71 original-write triggers. No existing database or listener was used.
These Windows compiler checks do not compile the Unix-only disk-quota statvfs
branch. No warm compatible Server Linux target was available for a bounded
followup; Linux Docker image compilation remains an explicit installation gate.
The standalone Enterprise source/Docker build bundle is produced by
`scripts/build-enterprise-source-package.py`, with its own real current web build,
source hashes, explicitly derived public terminal evidence tied to private
original byte hashes, dependency SBOM and archive checksums. It
excludes private operator material and does not build images or install, publish
or physically qualify a deployment.
