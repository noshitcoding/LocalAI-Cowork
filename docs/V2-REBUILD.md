# Approved full V2 rebuild

This delivery follows the user's approved A–I implementation plan. The scope is
the entire M0–M10 roadmap, the existing Android client and the separate Enterprise
management plane. Private projects remain local; shared projects belong to their
team server. Unknown code and documents remain untrusted even within trusted teams.

## Required architecture

All execution paths must converge on an immutable, explicitly confirmed execution
plan. The common V2 runtime owns policy, admission, model dispatch, effect journals
and result promotion. Enterprise management remains in its existing repository
and must consume versioned V2 artifacts. Runtime installation is not qualification.

The local runtime is an app-owned WSL2 distribution on Windows and the same pinned
containerd/gVisor/tool packages on Linux. It has no second project database. It
receives verified input snapshots and returns reviewed output versions. Host files,
secrets, Docker/containerd sockets and Windows interop are not workload capabilities.
Original file identities remain local to the native FileBroker.

The safe default is an isolated working copy. Native programs require explicit user
rights. Preapproved task rights are bounded and revocable; delegated rights only
narrow. Model/connector transmission is separate from process network access.
Team token limits remain authoritative in LiteLLM.

## Requirement inventory

`v2-rebuild-inventory.json` maps every current Feature Matrix row to an acceptance
package. Row 78 (weighted global fairness) remains explicitly outside the approved
scope. Every included row starts as requiring current verification, regardless of
its historical source declaration. An empty evidence list never means acceptance.
Workflows/schedules have package F as a cross-cutting dependency of D and G.

Regenerate the deterministic source inventory with:

```powershell
python tools/v2_rebuild/inventory.py --output docs/v2-rebuild-inventory.json
```

Pass `--server-root` explicitly to include Enterprise source bindings. The inventory
does not inspect credentials or treat source hashes as installed-package evidence.

## Current implementation status

The current Owner follow-up integrates fresh post-stop result contexts and the
Store pool-close drain fix. A separately observed gVisor guest is actually stopped
through Store.cancel, its original signed receipt reconciled, and its output
explicitly received as a local Ready version while original files and the confirmed
input copy remain unchanged. The current Main build has its own checks; see
[behavior and bounded evidence](MANAGED-RESULT-CONTEXT.md). The [explicit delivery and original runner-volume release](MANAGED-RESULT-DELIVERY.md)
are now integrated in Main. Fresh Main checks pass89 Runtime/88 PostgreSQL,
27 TLS, seven migration, two Owner-route and704 frontend cases. A separately
bound actual two-job gVisor fixture verifies original ACK/retirement receipts,
empty native capacity and unchanged retained results, originals and confirmed
copies after each GC. Installed App acceptance, result adoption, local result
deletion, general crash-time custody recovery, final runtime packages and the
earlier Missing409 root cause remain open. Earlier milestones below retain their original source/test scope.

- B/C, registered Owner runner input authority: the explicit PlanV2 destination
  binds the current registry revision, qualified environment and actual confirmed
  copy to the existing process attempt and worker. Fresh per-read checks, immutable
  retained blocks and one-use pretransport nonstart settlement pass nine identical
  SQLite/PostgreSQL cases. The visible exact-environment entry preserves original
  journals and confirmations; 590 frontend cases, 26 process and 23 snapshot
  regressions pass. An actual Store upgrade on a retained copy preserves all
  398 existing tables and receipts. The owned local development profile now runs
  the frozen daemon/web artifacts on schema113 with all old tables preserved;
  connection, existing projects and environment settings passed a browser smoke check.
  Native recipient broker/expiry/restart findings are closed by a separate static
  review. Installed transport, actual managed task
  execution, per-alias rights and result/capacity release remain open. See
  [the current scope](MANAGED-OWNER-INPUTS.md) and
  source-bound checks (development artifact omitted from public sources).
  The controller HTTPS client separately passes nine focused cases, including
  real loopback TLS with a synthetic signed peer, cancellation and lost-reply
  handling without a second start. Native service integration remains pending.
  Original signed completion history is now retained before process settlement,
  without creating fresh content or worker authority. A separate schema114
  checkpoint passes16 identical SQLite/PostgreSQL cases,26 process and6 migration
  regressions. Its actual retained-copy upgrade preserves399 existing tables;
  the live development app remains on113 pending later service integration.
  The separate fresh result-content reader now passes27 SQLite and27 isolated
  PostgreSQL cases using one frozen runtime test executable, plus14 HTTPS-client
  cases with actual loopback TLS. Current plan/target/environment/native-copy and
  source rights are checked before and after reads without reviving a worker.
  A retention-expiry race was fixed and independently reviewed. Separate bounded
  output versions, review/adoption and native service/App integration remain open;
  see [result reads and current evidence](MANAGED-RESULT-READS.md).
  A separate bounded native receiver now retains partial and finished result
  packs across restart;17 Windows and18 WSL/Linux filesystem cases pass, including
  eight new stage cases per platform. Rejected requests no longer leak lockfiles.
  The subsequent private SQL115 version journal, explicit resumable downloader,
  full native verification and atomic publication pass40 SQLite and40 isolated
  PostgreSQL cases on one captured executable. Its extended native checkpoint
  passes19 Windows and20 WSL/Linux cases, including ten stage cases each.
  Six migration regressions and a retained schema114-to115 copy preserve400
  existing table definitions/rows and old receipts. The live profile is unchanged.
  Public Owner result routes and UI are now integrated; 628 frontend cases,
  typecheck, web build and 735 generated contracts pass. Fresh integrated Runtime
  SQL qualification, comparison/adoption, retirement and actual Controller to
  native HTTPS/App acceptance remain separate; see
  [Owner results](MANAGED-OWNER-RESULTS.md) and
  [private result versions](MANAGED-RESULT-VERSIONS.md).

- B/C, confirmed owner working copies: `ExecutionPlanV2` binds the typed owner,
  actual local BrokerController identity/generation, immutable input manifest,
  models, rights and budgets. Confirmation materializes independent copies before
  the atomic queue/original-receipt transaction. Model and file effects recheck
  the controller. Workflows keep the same copy binding; edited copies survive a
  restart and become a retained result version. Missing or replaced private
  directories never fall back to original folders. See [the contract and limits](EXECUTION-INPUTS.md).
  This explicit process-free adapter does not run programs, native tools or MCP;
  those require the forthcoming managed-runtime/connector adapters. The complete
  B local/server program flow and all-path D convergence remain open.
- C, local input custody: selected native folders now freeze into a portable
  manifest with private local provenance, original idempotent preparation receipt
  and durable retirement. Owner HTTP and SDK expose preparation/metadata only;
  preparation alone exports no input bytes and admits no workload. The confirmed
  PlanV2 broker adapter sets `input_snapshot_bound=true`; V1 and other execution
  adapters retain their own explicit boundary. Remote transfer grants and hard
  reserved runtime volumes are still required.
- The current snapshot development report (development artifact omitted from public sources)
  binds the stable source window, unsigned daemon, web frontend, test executables
  and review-source archive. It passes 10 Windows broker, 12 runtime/HTTP, 26
  WSL/Linux filesystem and 500 frontend cases, contract/type/build checks and a
  separate current PostgreSQL16 concurrency contract. The earlier browser proof
  retains its exact independent bytes. Two exploratory local model runs timed
  out and remain failed evidence, not provider qualification.
- A separate real local Qwen3:4b UI path subsequently completed three model
  attempts with provider-reported usage, explicit copy-write approval, unchanged
  originals and an actual verified retained-result download. Its first two
  requests took 119 and 206 seconds under the frozen 1800-second app budget;
  the earlier 60-second test failures are not rewritten. Exact scope/bytes are
  retained in `.local/remediation-20261024/plan2-ui-result-qualification.json`.
  This does not qualify other providers, tokenizers or the new workload runtime.
- C, private receiver core: durable out-of-order chunk acceptance, deterministic
  acknowledgement replay, local native identity checks and independent complete
  file verification now precede private working-copy materialization. Nine Windows
  and ten WSL/Linux filesystem tests pass. A separate 16 MiB ext4 fixture rejects
  a 32 MiB payload before intent/acknowledgement and is positively removed.
  The scoped receiver report (development artifact omitted from public sources) binds current
  source inputs, logs and unsigned development test artifacts. Confirmed input
  selection, receiver authority, aggregate capacity, atomic version publication
  and the queue/runtime adapter are still open; no runtime profile is released.
- C, retained-result transfer: the common portable manifest and source-scoped
  4 MiB chunk readers are integrated through Contracts/Domain/Broker/Runtime,
  authenticated HTTP and the TypeScript client. Current targeted checks cover
  changed originals, broker reopen, corrupt retained bytes, offsets, aliases,
  auth, order/run scope, transfer concurrency and complete runtime snapshotting.
  See [the transfer contract and remaining input/runtime work](WORKSPACE-TRANSFER.md).
  Retained-result reads do not grant execution authority.
- B, original V1 owner slice: immutable five-minute execution plans, authenticated
  prepare/inspect/confirm/retire routes, fresh admission and atomic original run
  receipts. The old owner run endpoint only recovers accepted requests; a new
  legacy start receives `execution_plan_required`. Actor scope is still the
  installation owner, not a shared team/device identity. V1 receipt bytes remain
  historical; the separate public V2 contract does not rewrite them.
- The single-agent and workflow chat composers show target, model transmission,
  file rights, process networking, tools and budgets before confirmation. Original
  requests and confirmations survive reload. Server-side retirement also fences
  a preparation whose response was lost. Workflow recovery retains its existing
  journal. Retained file/directory restoration now uses the same confirmed new-copy
  path without a model; see [restoration](RESTORE-EXECUTION-PLANS.md). Task,
  schedule, crew and other document start paths still need
  the complete D convergence.
- Single-model answer regeneration now durably hands its exact original intent
  to the new chat's existing plan review. It no longer confirms a legacy plan
  automatically. Actual reload/retirement and a confirmed local Qwen run with
  original receipt replay pass; see [regeneration](ANSWER-REGENERATION.md).
  Portable local-copy workflows now retain the original immutable version and
  typed inputs while preparing a fresh child-scope plan. Actual review/reload,
  confirmed test-provider execution, receipt replay and retirement pass, including
  when a newer version exists. Native input references now preserve exact original
  JSON through review/reload/commit, including integers outside JS precision; see
  [the source contract](WORKFLOW-INPUT-SOURCES.md). Old order-bound folder rights
  still need explicit portable binding.
  File actions and unsupported workflows are not converted into model runs.
- Deterministic upload import now uses the common confirmed PlanV2 copy/receipt
  path, preserves the draft and earlier results and requires the observed hash
  for replacement inside its new copy. Actual UI reload/approval/download,
  explicit retirement and result reuse pass; see [import](IMPORT-EXECUTION-PLANS.md).
  Schema112 is unchanged. The common generator currently contains 722 types.
- Windows checks: 28 selected runtime regressions, three authenticated HTTP cases,
  four native ACL cases, current frontend tests/typecheck/build and public-contract
  consistency. Real daemon suites cover durability, workflows and model budgets
  with synthetic providers. A real Chromium case discards a successful commit
  response, reloads and explicitly recovers the same single run. Workflow review
  and a 390px viewport were also exercised. These are scoped development checks,
  not M10, real-provider, desktop or accessibility qualification.
- A private PostgreSQL16 cluster passed 16 simultaneous confirmations and twelve
  confirmation/retirement races, plus original receipts after cancellation and
  provider removal. The cluster was positively stopped. Reproduce with
  `tools/v2_rebuild/qualify_execution_plans_postgres.py`; its generated source
  manifest and report remain under `.local/execution-plans-postgres-*`.
- C, managed runtime components: the actual reserved ext4 volume core and the
  private containerd control library now have separate source-bound physical WSL
  proofs. The control library pins the newly spawned daemon/ELF/socket, isolates
  its own control mount namespace, and retains non-adoptable start fences.
  The existing Broker also supplies a non-cloneable owned dispatch handoff from
  its original transaction; ProcessRunner carries it into the independent worker.
  See [reserved volumes](MANAGED-RUNTIME-VOLUMES.md) and
  [control process, proof and limits](MANAGED-RUNTIME-CONTROL.md).
  Those earlier component reports started no workload tasks. The newer separate
  task proof below does not upgrade them. App-issued PlanV2/receiver grants,
  complete platform limit acceptance, result promotion and shipped packages remain
  open; no runtime profile is released.
- C, private input grants: a controller-signed short-lived grant now binds the
  confirmed plan, actual target/runner generation, complete input manifest and
  exact process intent/environment/fence. Only the existing verified receiver
  capability in the exact published reserved volume can reach owned dispatch
  validation. UID conversion affects the verified working copy. Six actual WSL
  component cases, 13 repeated volume cases and Windows/Linux grant/environment/
  broker regressions have source-bound development evidence. See
  [input grant implementation and limits](MANAGED-RUNTIME-INPUTS.md).
  The distinct private ManagedLinuxPackage format 3 cannot inherit an old Docker
  qualification. Actual app grant issuance, transfer endpoints, result promotion
  and published-volume release remain open; no new app start route is enabled.
- C, actual managed tasks: a scoped development snapshot now completes six real
  containerd/gVisor jobs and probes original-owner SIGKILL and initial lease expiry
  while that owner is SIGSTOPped. Held sandbox and workload statuses are separate;
  the exact original controller is reaped, both task deletions are acknowledged
  and FIFO EOF precedes durable output completion. Workspaces and containerd state
  use separate fixed ext4 reservations, with an independent native deadline fence.
  The eight-job snapshot binds 178 stable source inputs and unsigned development
  ELF artifacts; see [implementation, evidence and limits](MANAGED-RUNTIME-TASKS.md).
  Lease renewal, partial-output crash recovery, app/remote integration, resource
  release, full stress/platform qualification and shipped packages remain open.
- C, experimental Windows mechanism proof: a separate, subsequently removed WSL2
  distribution ran containerd 2.2.1 and gVisor release-20261026.0 with Windows
  automount/interop disabled. Physical probes blocked host paths/sockets and
  direct public/metadata networking, exhausted a reserved 64 MiB ext4 volume and
  its 2,048 inodes, observed CPU throttling, bounded forks, forced a cgroup OOM and
  confirmed empty process groups after stop. Own loop devices, cgroups, the
  distribution and its VHD were positively retired. This is not a shipped runtime.
- A remains open for installed-artifact/platform inventory and complete acceptance
  ownership. B now has an explicit private runner registry, exact live target
  gates and retained native plan references; see [execution targets](EXECUTION-TARGETS.md).
  Its complete local/server program-and-snapshot flow and common
  Team/device dispatcher remain open. C–I remain open. No historical report is upgraded.
- The first V1 plan implementation describes live folder grants honestly. Its
  `input_snapshot_bound` remains false until immutable snapshot staging is wired.
  It does not establish cross-node execution or managed-runtime isolation.

## Runtime findings that must shape C

The [reserved-volume component](MANAGED-RUNTIME-VOLUMES.md) now enforces real
ext4 allocation, inode limits and private native loop/mount identity. Its fixed
journal slots survive a full backing filesystem; a separate publish fence
prevents downgrade after a torn record. Thirteen physical WSL/Linux cases and
positive own-fixture cleanup are bound in its report (development artifact omitted from public sources).
This remains a resource component, not a released runtime or shared process adapter.

The [managed-runtime package contract](MANAGED-RUNTIME-PACKAGES.md) now has a
shared native verifier and an offline producer that never reads signing keys.
It binds exact signed manifest bytes, platform and every payload hash. It is
not yet an installer, active runtime service or physical qualification.

The mechanism proof is preserved under `.local/v2-rebuild-validation/` with its
exact scripts, upstream signature verification, binary hashes, runtime logs,
resource observations and positive cleanup. `tools/v2_rebuild/runtime_platform_probe.sh`
provides the reusable development probe for a disposable Linux host; its cleanup
now also removes its empty cgroup. No product qualification accepts this script.

- WSL still exposes integration mounts such as `/mnt/wsl` even with Windows drive
  automount disabled. The workload boundary must therefore be gVisor with a closed
  mount set, rather than the existence of a dedicated distribution.
- The containerd CLI's inherited FIFO output path produced broken pipes in this
  setup. The successful probe used bounded-volume log files and null CLI IO. The
  product service must own durable output capture and loss recovery independently
  of a client connection; this fixture does not implement that service.
- Filling the workspace can prevent the error log itself from being written.
  Protected, separately bounded runtime logs and caches are required. The proof
  reads actual filesystem capacity and process state rather than inferring them
  from an absent error message.
- Virtual process limits and host runtime thread limits need separate enforcement.
  The proof combines an in-guest process rlimit with host pids/memory/CPU cgroups.
- A current gVisor package includes supporting binaries. Offline packaging must
  retain the entire pinned package, plus the signed Cowork manifest and trust chain.

Still missing: native Linux hardware evidence, managed runtime service and app
integration, immutable transfers, complete offline tool bundles, hard runtime
cache/log limits, signed installation/update/rollback artifacts and final-package
qualification. No weaker backend is selected when this profile is unavailable.

## Completion and rollout gates

Every included requirement needs an integrated path, a current source-bound report
and the exact delivered artifact. Runtime profiles remain unavailable until their
physical negative isolation checks pass. The final gates include 100 sessions,
40 concurrent runs, 60 minutes mixed load, 24 hours endurance, physical desktop/
Android/accessibility checks and signed final-package installation/rollback.

Migrations use verified copies; historical payloads and uncertain operations stay
inert. Schedules start paused and credentials/devices are rebound explicitly.
Production cutover, publication, Git commit and push remain separate operations.
No public SaaS, macOS or new Android redesign is included.
