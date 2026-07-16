---
title: Tasks Route
type: route
doc_type: current-state
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
component: TasksView
purpose: Create and operate user-facing WorkTasks without duplicating crew or project ownership.
userStory: As a user I can define repeatable work, choose a crew or model runner, link a chat, and schedule the task from one route.
visibleText: Tasks, New task, Your tasks, Import from crew, Scheduler, Project context
sizeToken: page-shell, panel, ui-button, ui-field, ui-badge, task-pill
states: empty, selected, running, waiting_approval, completed, failed, canceled, scheduled
interactions: create task, select task, import crew tasks, create chat, open chat, run, stop, delete, save schedule, activate schedule, remove schedule
dataSource: SQLite work_tasks through workTasksStore; scheduled_tasks through coworkStore; crew templates through crewStore; project context through linked chat
accessibility: List pane is labeled; detail pane is labeled; buttons expose text labels; invalid working folders disable Start and show inline text.
tests: TaskPanels.test.tsx, TasksView.schedule.test.tsx, workTasksStore.test.ts, db.rs work_task_lifecycle_round_trip
source_files:
  - app/src/components/TasksView.tsx
  - app/src/components/tasks/TaskCreatePanel.tsx
  - app/src/components/tasks/TaskListPane.tsx
  - app/src/components/tasks/TaskDetailPane.tsx
  - app/src/components/tasks/TaskSchedulerPanel.tsx
  - app/src/components/tasks/TaskRunToolbar.tsx
  - app/src/stores/workTasksStore.ts
canonical_for:
  - /tasks ownership
  - WorkTask user interface
elements:
  - task-create-panel
  - task-list-pane
  - crew-task-import
  - task-detail-pane
  - task-run-toolbar
  - task-scheduler-panel
rationale: Tasks owns runnable work. Crew only supplies templates and executors; Projects own context through chats.
---

# Tasks Route

`/tasks` is now a list plus detail workspace. The route owns runnable `WorkTask` records and does not automatically clone `crew.tasks`.

The top panel creates new tasks. The left pane lists current tasks and exposes the explicit one-shot crew import. The detail pane edits the selected task, shows linked project context through the task chat, controls run/cancel/delete, and owns the schedule editor.

Project context remains informational. A WorkTask stores `threadId`, and project ownership is resolved through project-thread membership rather than a task-level `projectId`.
