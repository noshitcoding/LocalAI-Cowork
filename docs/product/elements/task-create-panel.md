---
title: Task Create Panel
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: task-create-panel
component: TaskCreatePanel
purpose: Capture the minimum fields needed to create a user-facing WorkTask.
userStory: As a user I can define a new task, choose a runner, optionally pick a working folder, and create the task from one compact form.
visibleText: New task, Title (optional), Execution, Crew, Model (optional), Expected output (optional), Working folder (optional, absolute), Choose folder, Task, Create task
sizeToken: panel, grid, ui-field, ui-button, task-inline-field
states: empty form, crew runner, model runner, invalid workDir, disabled submit, enabled submit
interactions: type title, choose runner, choose crew, type model override, type expected output, type or choose workDir, type prompt, create task
dataSource: workTasksStore.addTask; crewStore.crews for crew options; configStore.ollama for default model placeholder
accessibility: Labels wrap all form controls; create button disables until prompt and runner requirements are valid.
tests: TaskPanels.test.tsx covers invalid workDir behavior through detail; TasksView create flow is covered by typecheck and integration store tests.
codeRefs: app/src/components/tasks/TaskCreatePanel.tsx
screenshots: app/e2e/ui-quality.spec.ts-snapshots/tasks-wide-chromium-win32.png
rationale: Creation is separated from list/detail so TasksView can orchestrate state without owning the form markup.
---

# Task Create Panel

The create panel is the only place where new WorkTasks are created from scratch. It does not create crew templates and does not attach project context directly.
