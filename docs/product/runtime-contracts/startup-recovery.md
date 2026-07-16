---
title: Startup and Crash Recovery Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-10
last_verified: 2026-07-10
endpoint: Database recover_after_unclean_shutdown and begin_scheduled_run; Tauri IPC startup_recovery_status
purpose: Reconcile persisted executions after an unclean shutdown before any scheduler or runtime worker can start new work.
userStory: As a user I can restart Open Cowork after a crash and see honest terminal states instead of stale running work or silently missing runs.
visibleText: recovered startup state count in System gateway diagnostics
sizeToken: none
states: active, interrupted, failed, disconnected, retained, recovered
interactions: start app, inspect recovery count, inspect interrupted run, retry or resume explicitly
dataSource: SQLite runtime state reconciled in one immediate transaction during Tauri setup
accessibility: recovery count uses a labeled read-only input in the diagnostics panel
tests: db.rs persistent reopen/idempotence and scheduler claim tests, process_manager.rs exit/stop tests, SettingsView.test.tsx startup recovery test
source_files:
  - app/src-tauri/src/db.rs
  - app/src-tauri/src/lib.rs
  - app/src-tauri/src/process_manager.rs
  - app/src-tauri/src/process_control.rs
  - app/src-tauri/src/support_bundle.rs
  - app/src/components/SettingsView.tsx
canonical_for:
  - unclean shutdown reconciliation
  - scheduler run claiming
  - crew run start persistence
  - managed process lifetime and exit monitoring
  - startup recovery diagnostics
rationale: In-memory execution cannot continue after process loss. Persisted state must distinguish an interrupted execution from work that is still running, deliberately waiting, or safely resumable.
---

# Startup and Crash Recovery Contract

Tauri opens and verifies SQLite, runs startup recovery, registers every managed runtime state, and only then starts the scheduler worker. Recovery uses one `BEGIN IMMEDIATE` transaction. If any statement fails, no partial state transition or recovery event is committed and app setup fails rather than starting workers against ambiguous state. Repeating recovery is idempotent and returns zero changes after the first successful pass.

Engine runs with status `running` become `interrupted`, phase `interrupted`, receive a fixed content-free error when none exists, and receive an ordered `run_interrupted` event. Legacy tasks and work tasks that were `running` become `failed`; running legacy task steps become `failed`. Scheduled and crew runs that were `running` become `interrupted` with a completion timestamp and fixed error, and crew runs receive a `run_interrupted` event. Active worker sandboxes become `interrupted` but retain their workspace and protected environment reference so an explicit inspection, resume, or destroy action remains possible.

Managed processes in `starting` or `running` become `interrupted`, their stale PID is cleared, and a stop timestamp is recorded. Terminal backends in `connecting` or `connected` become `disconnected` because native sessions are process-local. Recovery never attempts to kill a persisted PID: an older build may have left an unrelated process at a reused PID, so automatic PID-based termination after restart is unsafe.

New Crew runs are inserted with status `running` before the Python runtime is invoked. Completion updates the same row and therefore cannot leave an invisible execution after a crash. New scheduled runs are claimed in an immediate transaction before provider or Crew execution. The claim inserts the `running` row, advances task runtime timestamps, and rejects a second running row for the same scheduled task. Completion updates that row. After a crash, the next startup marks the row interrupted and does not immediately replay a side-effecting schedule whose next time was already advanced.

New managed background processes are created in a platform process tree before being reported as running. Windows uses a kill-on-close Job Object; Unix uses a dedicated process group. A monitor thread owns both the child and tree guard, records normal/nonzero exits, clears the PID, and does not overwrite an explicit stop status. Explicit stop terminates the Windows process tree or Unix process group. Closing the app closes the process-tree guard, so descendants do not outlive the desktop runtime.

Open sessions, pending approvals, pending engine runs that never started, scheduled-task definitions, and deliberately waiting task states are not failed by startup recovery. They are persistent user workflow, not evidence of a live native execution. The `startup_recovery_status` command exposes only a timestamp and counts per state class. System diagnostics show the total count, the support bundle includes the validated count report, and the audit writer records a content-free `runtime.startup_recovery` event when at least one state changed.
