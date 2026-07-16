---
title: Crew Task Import Control
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: crew-task-import
component: TaskListPane
purpose: Explicitly copy crew template tasks into WorkTasks without hidden synchronization.
userStory: As a user I can decide when crew templates become runnable tasks.
visibleText: Crew templates, Import from crew
sizeToken: ui-field, ui-button, task-import-strip
states: no crew selected, no crew tasks, import available, already imported, empty template description
interactions: choose crew, click import
dataSource: crewStore.crews[].tasks; workTasksStore.upsertMany
accessibility: Native select and text button; button disabled when no importable crew is selected.
tests: TaskPanels.test.tsx, crew-task-import.md flow doc, i18n:audit
codeRefs: app/src/components/TasksView.tsx, app/src/components/tasks/TaskListPane.tsx
screenshots: app/e2e/ui-quality.spec.ts-snapshots/tasks-wide-chromium-win32.png
rationale: This replaces the old automatic crew-task cloning effect.
---

# Crew Task Import Control

Import creates independent WorkTasks. Later edits in `/tasks` do not mutate the original crew template.

The import button is enabled only when the selected crew has at least one template with a non-empty description whose id is not already present in the current WorkTask list.
