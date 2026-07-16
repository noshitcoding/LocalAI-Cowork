---
title: Task Detail Pane
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: task-detail-pane
component: TaskDetailPane
purpose: Edit the selected WorkTask and show its chat/project context.
userStory: As a user I can inspect and edit one task without losing the list context.
visibleText: No task selected, Project context, No linked project, No task chat yet, Title, Execution, Crew, Model (optional), Expected output, Working folder (absolute), Task
sizeToken: task-detail-pane, task-context-strip, task-edit-grid, ui-field, ui-button, ui-badge
states: no selection, selected, crew runner, model runner, invalid workDir, missing crew, output present, error present
interactions: edit fields, choose workDir, create or open chat, run, stop, delete, schedule
dataSource: workTasksStore.updateTask; projectStore.projects through linked threadId; chatStore through task thread
accessibility: Detail section has aria label; form fields use labels; invalid working folder displays inline text and disables Start.
tests: TaskPanels.test.tsx
codeRefs: app/src/components/tasks/TaskDetailPane.tsx
screenshots: app/e2e/ui-quality.spec.ts-snapshots/tasks-wide-chromium-win32.png
rationale: WorkTask owns execution fields; project ownership remains resolved through chat-thread membership.
---

# Task Detail Pane

The detail pane is the editing surface for a selected WorkTask. It deliberately shows project context as read-only information.
