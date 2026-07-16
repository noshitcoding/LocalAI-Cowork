---
title: Task List Pane
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: task-list-pane
component: TaskListPane
purpose: Show current WorkTasks as selectable rows and host the explicit crew-template import action.
userStory: As a user I can scan my tasks, see runner/status/schedule badges, select one task, and import templates from a crew when needed.
visibleText: Your tasks, task(s), Crew templates, Import from crew, No tasks yet. Create your first task above or import templates from a crew.
sizeToken: task-list-pane, task-list-item, ui-field, ui-button, ui-badge, task-pill
states: empty, selected row, scheduled badge, no crews, import disabled
interactions: select task, choose crew, import crew tasks
dataSource: workTasksStore.tasks; coworkStore.scheduledTasks; crewStore.crews
accessibility: The pane has an aria label, task rows are buttons, and status metadata remains textual.
tests: TaskPanels.test.tsx
codeRefs: app/src/components/tasks/TaskListPane.tsx
screenshots: C:/Users/Riege/AppData/Local/Temp/open-cowork-qa/tasks-desktop.png
rationale: Listing and importing are grouped because both operate on task selection and WorkTask inventory.
---

# Task List Pane

The list pane is a navigation surface, not an editor. Editing happens in the detail pane for the selected WorkTask.
