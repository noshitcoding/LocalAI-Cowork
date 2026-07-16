---
title: Crew Task Import Flow
type: flow
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
routes:
  - /tasks
  - /crew
component: TaskListPane
purpose: Convert crew template tasks into user-facing WorkTasks only when the user explicitly requests it.
userStory: As a user I can keep crew templates in Crew and import selected crew tasks into Tasks when I want to run or schedule them.
visibleText: Crew templates, Import from crew
sizeToken: task-list-pane, task-import-strip, ui-field, ui-button
states: no crews, no templates, imported, already imported
interactions: choose crew, import crew tasks, select imported task
dataSource: crewStore.crews[].tasks; workTasksStore.upsertMany
accessibility: Import select and button are visible in the task list pane and the button is disabled when no importable crew is selected.
tests: TaskPanels.test.tsx
source_files:
  - app/src/components/TasksView.tsx
  - app/src/components/tasks/TaskListPane.tsx
  - app/src/stores/crewStore.ts
  - app/src/stores/workTasksStore.ts
canonical_for:
  - explicit crew task import
steps:
  - User opens /tasks.
  - User chooses a crew in the list pane.
  - TaskListPane filters out crew templates whose ids already exist as WorkTasks or whose description is empty.
  - User clicks Import from crew.
  - TasksView maps crew.tasks to WorkTask records with runner crew and crewId.
  - workTasksStore persists the imported tasks and selects the first imported task.
rationale: This replaces the old hidden auto-import effect that made Crew and Tasks look like the same surface.
---

# Crew Task Import Flow

Crew tasks are templates. They stay in `/crew` until the user imports them in `/tasks`.

The import is intentionally one-way. After import, the WorkTask can be edited, scheduled, run, and deleted without mutating the original crew template.
