---
title: Task Run Toolbar
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: task-run-toolbar
component: TaskRunToolbar
purpose: Provide the primary execution actions for the selected WorkTask.
userStory: As a user I can open or create the task chat, start work, stop a running task, or delete a task from one stable toolbar.
visibleText: Chat erstellen, Chat öffnen, Start, Stop, Delete
sizeToken: task-run-toolbar, ui-button, btn-stop
states: idle, running, waiting_approval, invalid run, delete disabled
interactions: open chat, run task, cancel task, delete task
dataSource: TasksView handlers; workTasksStore status; chatStore thread linkage
accessibility: All actions are text buttons; Start is disabled when the task cannot run safely; Stop remains available for running and waiting_approval tasks.
tests: TaskPanels.test.tsx, TasksView.schedule.test.tsx
codeRefs: app/src/components/tasks/TaskRunToolbar.tsx
screenshots: app/e2e/ui-quality.spec.ts-snapshots/tasks-wide-chromium-win32.png
rationale: Execution controls are split from field editing so button sizing and disabled-state logic stay consistent.
---

# Task Run Toolbar

The toolbar is the only primary action row for a selected WorkTask. Stop is available for both `running` and `waiting_approval` so a task cannot get stuck while waiting for crew approval. Delete delegates to `TasksView`, which removes the matching scheduler row before removing the WorkTask.
