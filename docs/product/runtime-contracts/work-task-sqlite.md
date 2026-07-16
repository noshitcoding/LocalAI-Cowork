---
title: WorkTask SQLite Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
endpoint: Tauri IPC work_task_list, work_task_upsert, work_task_delete, work_task_update_status
purpose: Persist user-facing WorkTasks in SQLite while keeping legacy PlanApproval tables compatible.
userStory: As the app runtime I can load, update, schedule, and delete WorkTasks without touching legacy approval tasks.
visibleText: none
sizeToken: none
states: idle, waiting_approval, running, completed, failed, canceled
interactions: list, upsert, delete, update status, migrate localStorage backup once
dataSource: SQLite table work_tasks; localStorage key open-cowork-work-tasks as backup and migration source
accessibility: not applicable
tests: workTasksStore.test.ts, coworkStore.test.ts, TasksView.schedule.test.tsx, db.rs work_task_lifecycle_round_trip, db.rs delete_work_task_removes_matching_schedule
source_files:
  - app/src-tauri/src/db.rs
  - app/src-tauri/src/lib.rs
  - app/src/stores/workTasksStore.ts
canonical_for:
  - WorkTask persistence
  - WorkTask Tauri commands
rationale: WorkTask is the only user-facing task model. Legacy tasks and task_steps remain PlanApproval compatibility tables.
---

# WorkTask SQLite Contract

Schema version `23` creates `work_tasks` with the fields used by the React `WorkTask` model: id, title, prompt, expected output, working folder, linked chat thread, runner, crew id, model override, schedule expression, schedule enabled flag, status, output, error, last run timestamp, and timestamps.

The Tauri commands are:

- `work_task_list`
- `work_task_upsert`
- `work_task_delete`
- `work_task_update_status`

`work_task_upsert` accepts an empty `prompt` so edit-in-progress drafts can stay consistent between UI state and SQLite. The create form still requires a non-empty prompt before creating a new WorkTask, and execution remains disabled when the prompt is empty.

`work_task_delete` also removes `scheduled_tasks` where `scheduled_tasks.id` equals the WorkTask id.

`TasksView` also removes the loaded scheduler row through `coworkStore.removeScheduledTask` before removing the WorkTask so browser fallback and in-memory UI state do not keep a stale schedule. `coworkStore.removeScheduledTask` must keep updating local state when the Tauri scheduler command is unavailable.

The frontend migrates all normalized `open-cowork-work-tasks` entries into SQLite once when Tauri is available, including edit-in-progress drafts with empty prompts, then sets `open-cowork-work-tasks-sqlite-migrated=true`. The old LocalStorage value remains as a backup and the browser fallback still uses it.
