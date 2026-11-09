# Current architecture

The complete M0–M10 scope remains open; the [workplan](COMPLETION-WORKPLAN.md)
records the required deliveries and acceptance. Historical reports below apply only
to their archived sources. The [qualified human-resume slice](WORKFLOW-HUMAN-RESUME.md)
uses SQL 20/private Clock 1/Progress 4/event 8/export 13 and 163 generated contracts.
Explicit continuation preserves original claims, deadlines, budgets and clock origin;
unknown effects block activation and answers require a separate explicit commit.
New identity/accounting/runner services and their integration need fresh qualification.

**Historical scoped workflow questions qualification passed (2026-10-09):** The single-owner input slice is source-bound in the immutable input evidence (private development evidence; omitted from public sources): seven reports, 263 inputs; Windows 364/Ubuntu 347 Rust tests with format/Clippy, 11 PostgreSQL contracts, ten daemon process suites per platform, 145 frontend tests on 71 sources/six Dist files and 20 browser checks on 74 sources. Then-qualified versions: database 19, event schema 7, export 12, 161 generated contracts. input_resume_available was false in that historical qualification. Historical 144-contract, 137-/102-test and 18/7/11 proofs keep their original scope. Native GUI, WCAG/NVDA, enterprise, complete M5/M0-M10 and release acceptance remain false. The 1440px conversation layout still needs improvement; see [WORKFLOW-INPUTS.md](WORKFLOW-INPUTS.md).
**Historical snapshot-copy qualification (2026-10-09):** Historical operation snapshots now support explicit inspection and restoration as independent draft copies. The separate frontend proof (private development evidence; omitted from public sources) passes 102 product tests on 66 inputs; the browser proof (private development evidence; omitted from public sources) passes 14 scoped checks on 69 inputs and six Dist files, with its own daemon stopped. The backend was unchanged at that historical snapshot revision; the current input slice is qualified separately in the input evidence above. Journal evidence 2a6e14a… is preserved; its 86-/18-test UI/Dist proofs are historical relative to this extension. Browser file saving, native dialogs, NVDA/WCAG AA, enterprise and complete M5 remain unqualified. The historical snapshot evidence (private development evidence; omitted from public sources) is passed: seven reports, 249 inputs, SHA256 bcbd57e6e93937b100447ce135d831c9fd20758b2fd76437ff0e4e3f09f5d425.

The implementation is an early, single-owner V2 runtime. It is not the enterprise server.

- cowork-contracts owns the existing Rust request/response types; cowork-workflow-contracts
  adds the domain-derived public workflow facade, durable draft/operation contracts and shared
  RunRequest/RunPreflight unions and owner-scoped answer draft/intent contracts. The current generator emits 163 definitions.
  The generator derives JSON Schema,
  OpenAPI 3.1, TypeScript and precompiled browser validators from those types.
  Runtime JavaScript compilation is prohibited by the web CSP.
- cowork-domain owns transitions, checkpoints, lease checks and canonical approval hashes.
  Its private environment records bind immutable definitions, qualification and runner
  observations. Storage migration 10 commits environment transitions with their idempotency
  receipts. Optional RunSpec bindings now freeze version and qualification hashes; product
  selection and the public management API remain open. See ENVIRONMENTS.md and PROCESS-JOURNAL.md.
- The private domain workflow module compiles explicit frozen role/provider graphs and reduces
  revisioned decisions. Parallel/delegated admissions share global and ancestor budgets.
  Storage migration 16 adds a project-scoped immutable registry and replay-validated
  run checkpoints. The shared run transaction freezes the plan and commits checkpoint,
  budget accounting, parent lease/revision and events together before returning a new
  permit. A private per-agent model adapter additionally commits exact scoped
  conversations, requests, complete answers and response-bound accounting receipts,
  uses native provider streaming and checks live leases/monotonic deadlines.
  A private coordinator now owns one queued parent, schedules static graphs within
  shared context capacity and commits terminal root results with the parent
  message/state/events. Managed results use the persisted snapshot phase; only a
  terminal graph with no pending actions may explicitly retry finalization.
  The ordinary supervisor now uses the shared trusted queue claim and passes the
  already committed parent lease to that coordinator. Single-agent-only callers
  retain the old claim path. Exact start revocation closes only an untouched queued
  or workspace-waiting graph, then permits later work to proceed; corrupt workflow
  bindings cannot enter the single-agent path. The private database-only workflow
  artifact broker now uses the shared policy with each node's frozen scoped RunSpec.
  Run/node/turn/raw-call hashes separate approvals and receipts while provider
  messages retain raw IDs. Artifact effects, one atomic ToolResult decision,
  checkpoint/step accounting, receipt and events commit together. Exact approval
  gates leave the parent Running so sibling models may settle; resolution and
  effect recheck current revocation, lease and parent/node/ancestor deadlines:
  before first/existing approval, after the head lock and before final commit.
  A stop-policy sibling failure causes a known workflow_stopped node failure.
  Its guard runs before the operation and after the head lock, blocks new sibling
  approvals and preserves already admitted model evidence; final commit checks
  the original lease and deadline.
  Continuations require every tool receipt and wait only for recoverable sibling
  reservations.
  Workflow format 3 adds replay-bound human input checkpoints; the current reader
  retains formats 1 and 2. That archived workflow format used event schema 7; new continuations use schema 8.
  The public project library, immutable version/revoke routes, shared preflight/start,
  safe WorkflowRunView and editor/composer now expose sequential/parallel models and
  the three database-artifact tools plus request_input. Migration 17 adds a derived durable attention
  index so agent approvals enter the shared inbox while the parent remains Running.
  It does not grant rights or own transitions. The public view excludes private
  leases/plans/permits and returns only current actionable exact approvals.
  External file/MCP/process tools, hierarchical model delegation
  and general uncertainty reconciliation remain open. Migration 18 separately stores incomplete
  editor documents, immutable snapshots and exact operation intentions. Draft CAS and
  historical save/conflict receipts commit atomically. Prepare stores the complete
  create/publish/revoke/start request; commit uses only its ID, while accepted start
  receipts preserve the initial RunSpec. Pending provider/storage uncertainty remains
  recoverable; discard cannot undo an accepted run. New actions still enforce live policy.
  Migration 19 adds single_owner_preview answer heads, immutable CAS/conflict
  snapshots and frozen answer intentions. A confirmed question binds parent/node/role/turn,
  raw call and frozen capability fingerprint. The parent remains Running with its lease
  while independent siblings continue. Only a confirmed draft may be prepared; answer
  Commit is bodyless and uses the saved ID. Graph/tool receipt, human receipt, actual
  event and Accepted intent commit in the same transaction. A known committed cancellation
  stays authoritative after conservative interruption; genuine uncertain interruption
  retains Pending. Reads and accepted replays preserve original evidence.
  input_resume_available is now derived from verified whole-graph continuation eligibility.
  Resume intent, queued parent and Accepted journal receipt commit atomically; activation
  consumes exactly that prepared generation with a fresh lease. The archived input
  evidence below predates this extension. The new scoped human-resume evidence passes
  Windows/Ubuntu/PostgreSQL/process/frontend and bounded browser checks; this is not
  native-GUI, WCAG, enterprise or full M5 acceptance.
  Older artifact-broker/product/desktop checks retain their historical scope.
  The historical journal evidence (private development evidence; omitted from public sources)
  separately binds core, PostgreSQL, processes, frontend and actual browser recovery.
  It is not full M5 or enterprise acceptance.
  See WORKFLOWS.md, WORKFLOW-PRODUCT.md, WORKFLOW-JOURNAL.md and WORKFLOW-INPUTS.md.
- cowork-policy owns the built-in tool effect catalog, deny precedence and permission modes.
  External tool metadata is not a trust source.
- cowork-executor owns trusted adapter process lifecycle supervision: explicit argv/environment,
  bounded output, deadlines/cancellation and OS process cleanup. Windows admission atomically
  attaches a kill-on-close Job Object; Linux uses a process group and direct-child death signal.
  It is not yet connected to the tool/API path and is not a security sandbox. The native
  restrictions, gVisor/container adapters and final runner integration remain required;
  see PROCESS-SUPERVISOR.md.
- cowork-storage commits state, checkpoint, events, messages and artifacts in one transaction.
  SQLite uses WAL and synchronous FULL. PostgreSQL runs the same storage contract.
  Revision checks reject stale updates. New runs snapshot provider, policy, target and limits.
  Migration 11 adds a private durable process journal: exact admission, environment recheck,
  writer ownership, original worker fence and event commit together. Duplicates return no
  new launch authority. Uncertain execution requires reconciliation before explicit resume;
  see PROCESS-JOURNAL.md. No product process tool is enabled by this foundation.
- Native restoration and import runs carry a single frozen file action with no provider. They preserve
  drafts and share the runtime policy, approval, journal and snapshot paths with model runs.
  Migration 6 records execution kind; migration 7 adds scoped upload records. Runtime event
  schema 7 and order-export format 12 include model attempts, budget summaries, process/MCP attempts and hashed workflow snapshots with per-agent contexts, human questions/receipts and owner-scoped answer draft/snapshot/intent archives. Optional archival fields preserve old exports; import grants no active authority.
  See FILE-OPERATIONS.md and FILE-UPLOADS.md.
- Migration 8 guards compatibility with durable native revocation. Native folder registration
  and tombstones live in a locked, atomically published broker registry. Each IO authorization
  rereads it and checks physical identity; revocation shares the physical-resource IO lock.
  The native-only API additionally requires a separate key and rejects browser Origins.
  This registry is part of the required backup set; see NATIVE-FOLDERS.md.
- cowork-mcp is a private MCP 2025-11-25 HTTP client with bounded JSON/SSE parsing,
  offline schema validation and a hashed catalogue. Prepared calls are bound to one
  session/epoch; failures after dispatch invalidate that session without replay.
  It grants no policy rights. Migration 12 adds the project-scoped server register
  and durable MCP journal. Frozen bindings, exact shared approvals, one dispatch
  opportunity and protected reconciliation are available through a trusted runtime
  entry point. Migration 13 adds private encrypted session checkpoints, transactional
  busy/ready fencing and continuation after worker restart without reinitialization.
  Migration 14 adds private encrypted credentials with revision and terminal revocation.
  cowork-secrets owns native machine-local Windows keys and Linux Secret Service keys;
  the runtime resolves exact credential scopes and the original session key before IO.
  Migration 15 adds durable native key setup/rotation receipts and active-key confirmation.
  Only a newly committed reservation yields creation authority; uncertain completion
  requires explicit read-only reconciliation or abandonment. Old receipts cannot reselect
  an earlier key. The native-only setup API requires both authorities and rejects Origins.
  Desktop setup commands/UI now require native confirmation and use shared DE/EN messages.
  The owner API/UI now supports project catalogue import and explicit run selections.
  Preflight resolves references; start acceptance rechecks frozen bindings transactionally.
  The model loop uses the same durable dispatcher and does not settle results twice.
  Interactive native dialogs, key recovery, server KMS, discovery/credential setup,
  explicit reconnect and public reconciliation remain open; see MCP-PRODUCT.md.
  No remote schema or returned resource URL is fetched.
- cowork-runtime owns provider streaming, the model/tool loop and the worker supervisor.
  Queued runs are leased by optimistic compare-and-swap; every worker write checks its lease.
  Shared worker claims route single-agent and trusted workflow parents without a second
  claim. Original and newly minted leases are checked again before commit after lock
  waits. A workflow parent occupies one supervisor slot while bounded child tasks use
  its own frozen pool; these bounds do not implement enterprise or global model quotas.
  Lost leases and uncertain workflow cancellation/shutdown interrupt rather than replay.
  Known cancellation of confirmed human waits can finish Canceled; unconfirmed model
  effects remain Interrupted with their reservation. Durable cancel provenance closes
  pending human answers without treating every interruption as cancellation.
  Explicit completion-only workflow resume uses the same queue without another model
  request; general interrupted-agent reconciliation remains unavailable. Internal artifact
  tools and explicit native folder capabilities are enabled in ordinary runs.
  Public workflows enable the three scoped database-artifact tools and request_input;
  external file/MCP/process dispatch is not connected to workflow execution. There is no
  host shell or MCP subprocess execution.
  Execution time starts on the first claim and includes approval/input waits. Active model
  requests use a monotonic deadline and durable wall-clock checks; expired waiting runs are
  swept on startup and every two seconds. A late approval cannot authorize another action.
  Model requests reserve one configured context window in the run transaction before HTTP.
  Complete usage settles the reservation; unknown or interrupted usage remains charged.
  Provider-reported overruns are recorded before returned tools can execute. Migration 9
  reconstructs older preview attempts from unchanged events; see MODEL-BUDGETS.md.
  Organization-wide admission, cost reservations, OS resource quotas and offline grants remain pending.
- cowork-daemon exposes authenticated loopback HTTP under /api/v2, polling and sequenced SSE.
  It can run an API process without workers or workers without an API. Those are process options,
  not yet an enterprise queue/deployment implementation.
- Authenticated binary downloads resolve either a managed workspace or a stored run snapshot.
  The broker verifies scope and retained hashes; the generated DownloadMetadata contract and
  client digest check guard the browser handoff. URLs never carry authentication tokens.
  See WORKSPACES.md for the 64 MiB bound and NATIVE-SAVE.md for native file delivery.
- Upload reservation and verified binary transfer precede any workspace mutation. An explicit
  native import run shares the policy, journal and reconciliation path. Uploads are scoped to
  their order; only that order's managed workspace can be an import destination. Per-process
  transfer admission bounds concurrent buffering/IO. Aggregate storage quotas remain pending.
- @cowork/ui is shared by the web entry and Tauri. TanStack Query holds confirmed API reads.
  Zustand holds only view preferences. Order drafts use a durable application command
  with revisions. Workflow editor documents use serialized CAS autosaves through the
  application service, including incomplete data. Recovery loads paginated draft heads
  and unacknowledged operation receipts before generating a new key. Frozen Pending
  requests retry the same ID; Accepted receipts open their original immutable version/run.
  Current client/project context owns callbacks, and stale callbacks cannot update a
  different editor. Unknown formats remain read-only/exportable; conflict snapshots can
  be explicitly opened as a new draft. Historical create/publish source recovery first reads
  the current operation and binds its exact scoped immutable snapshot/hash. Definition
  comparison normalizes typed budget defaults and set fields only, preserving vector order;
  the server verifies the full content hash. Original publish-version/origin references
  remain fixed. A user-requested copy saves a fresh ID/CAS0 with one frozen key; preview
  performs no save/acknowledge, and acknowledging the source does not erase the copy.
  Another publication base requires an explicit reviewed choice. Pending and unknown/bad
  bindings remain recovery-only or read-only. StrictMode and stale project reads are fenced. Export uses
  the shared verified browser/native download path; OS-dialog acceptance stays open.
  The working-file listing and text preview wait for confirmed workspace initialization
  and cached revalidation to finish. Explicit refresh awaits workspace confirmation before
  listing/preview. Broker workspace_busy is a temporary status, and other errors remain
  visible; no automatic mutation retry or weaker broker lock was introduced.
- The Tauri shell exposes connection, user-mediated save, folder-management and native key-setup commands,
  and starts/attaches to a separate daemon. File delivery accepts bytes and integrity metadata;
  only the native picker supplies the host destination. No agent tool exposes this writer.
  Folder selection likewise accepts no renderer-supplied path. Native confirmation fixes the
  project, canonical path, physical identity and rights before privileged registration.
  Dropping the window does not own or kill the daemon process. The historical
  Windows lifecycle proof (private development evidence; omitted from public sources)
  confirms a two-role workflow completes after desktop termination, with exact HTTP
  approvals and two hash-verified artifacts; reopening uses the same daemon without
  new model calls. It uses an isolated build snapshot with only a documented empty
  Cargo workspace marker. That old source/Dist snapshot does not qualify the new journal
  frontend. GUI clicks, native dialogs and installers remain unqualified.
  Development shells use the staged target-specific sidecar. Build preparation reads Cargo's
  reported executable path, so an external build directory works on Windows and Linux.
- cowork-broker owns native directory capabilities, path validation, file locks, hash-checked
  text writes and retained versions. The runtime journals an effect before calling the broker;
  the storage transaction confirms the receipt and tool completion together. Uncertain effects
  require hash-bound reconciliation. See FILE-BROKER.md for the supported scope and limitations.

## Current invariants and limits

The product id is dev.localai.cowork.v2.preview. Default API port is 43821; Vite uses 5175.
No V1 path or database is opened by the new runtime.

A daemon token authenticates one owner. It is not multi-user authorization, OIDC, RBAC or
tenant isolation. Binding remains loopback-only, including when PostgreSQL is selected.
The capability response describes the actually enabled preview features.

A tool approval hashes run, exact call id/name/JSON arguments, executor and policy revision,
plus the immutable environment binding when present. Absent bindings retain old fingerprints.
Deny rules and read-only mode still win over approvals. Workflow artifact calls use
a distinct run/node/turn/raw-call digest with the same scoped policy fingerprint;
the parent stays Running while node-level approvals wait. A resolved approval does
not bypass the final lease, revocation or deadline checks. Tool results and content
cannot grant rights.

Text artifacts are stored transactionally in the database. The private workflow
artifact read capability covers immutable results of the authorized order,
including earlier runs and its verified inherited branch snapshot. It checks
metadata index, source-run/parent-order binding and content size/hash; explicit
branch inheritance grants no arbitrary foreign-order or host-file access. New
workflow tool effects and receipts share one transaction. Native text-file versions
reside
in private content-addressed storage. Managed per-order directories and immutable file snapshots
use the same broker; completion checkpoints and snapshot transactions are described in
WORKSPACES.md. S3 and untrusted process execution remain pending in M4/M6.
Conversation snapshots, search indexing, activity and consistent order exports are described
in HISTORY.md. Database migrations verify every applied checksum and reject unknown versions.

Provider streaming supports Ollama NDJSON and OpenAI-compatible SSE. A missing completion
marker fails the step. There is no implicit provider switch. Costs remain null without a
qualified versioned price source. Context uses a conservative byte-based ceiling including tool
definitions, shared context assembly and an execution-time recheck; a tokenizer
and context compaction are still required before full runtime acceptance.

## Trust and operations boundaries still to implement

Organization/project/device grant intersections must be wired to live policy versions before
team execution. The existing policy evaluator accepts intersection layers, but this preview
has only one local owner. Offline grants, quota reservation and synchronization are not enabled.

The full 100-session/40-run workload, migration activation, Kubernetes/Compose lifecycle,
Windows native sandbox and Linux gVisor execution are release blockers. Deployment templates
alone must never be used to claim these capabilities.

Native preparation follows [Tauri's sidecar naming contract](https://v2.tauri.app/develop/sidecar/)
and the [Cargo build-script target environment](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts).
