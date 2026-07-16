---
title: WorkTask Run And Schedule Flow
type: flow
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
component: TaskDetailPane
purpose: Run or schedule a WorkTask while preserving chat and project context boundaries.
userStory: As a user I can run a task now or save a schedule without reconfiguring crew or project data inside Tasks.
visibleText: Start, Stop, Chat erstellen, Chat öffnen, Scheduler, Save, Remove
sizeToken: task-run-toolbar, task-scheduler-panel, ui-button, ui-field, ui-button--sm
states: idle, running, waiting_approval, completed, failed, canceled, schedule active, schedule paused
interactions: create chat, open chat, run model task, run crew task, stop running task, save schedule, toggle schedule, remove schedule, delete task
dataSource: work_tasks, scheduled_tasks, chat_threads, project_thread_membership
accessibility: Start is disabled for missing crew, empty prompt, busy task, or invalid relative workDir; schedule controls retain text labels.
tests: TaskPanels.test.tsx, TasksView.schedule.test.tsx, workTasksStore.test.ts, coworkStore.test.ts, db.rs delete_work_task_removes_matching_schedule
source_files:
  - app/src/components/TasksView.tsx
  - app/src/components/tasks/TaskDetailPane.tsx
  - app/src/components/tasks/TaskRunToolbar.tsx
  - app/src/components/tasks/TaskSchedulerPanel.tsx
  - app/src/engine/tasks/workTaskExecutionService.ts
  - app/src/engine/tasks/workTaskScheduleService.ts
canonical_for:
  - WorkTask execution flow
  - WorkTask scheduling flow
steps:
  - User selects a WorkTask in /tasks.
  - Detail resolves chat and project context from threadId.
  - User starts a model or crew run.
  - TasksView creates a chat when needed and streams or records output.
  - User saves schedule; scheduled_tasks.id equals work_tasks.id.
  - Saving schedule details preserves an existing scheduled_tasks.active value.
  - User toggles Active; if no scheduler row exists yet, TasksView creates one with the WorkTask id.
  - Scheduler controls display the loaded scheduled_tasks.active state when a matching row exists.
  - Deleting a WorkTask removes the loaded scheduler row in the frontend and deletes its matching scheduled task in SQLite.
rationale: Scheduling belongs to a WorkTask, while project context remains attached to chats.
---

# WorkTask Run And Schedule Flow

`TaskDetailPane` exposes the run toolbar and schedule editor for the selected WorkTask.

The scheduler entry uses the WorkTask id as its id. Saving or enabling a valid schedule creates or updates that matching row. When a matching scheduler row is loaded, its `active` field is the UI source of truth for the Active checkbox, and Save preserves that value unless the user explicitly toggles Active. Deleting a task removes the visible scheduler state before deleting the WorkTask, and the backend delete also removes the SQLite row. This keeps delete cleanup deterministic and avoids a second task identity model.
