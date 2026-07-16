---
title: Task Scheduler Panel
type: element
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
route: /tasks
element_id: task-scheduler-panel
component: TaskSchedulerPanel
purpose: Save, pause, and remove the scheduler entry for the selected WorkTask.
userStory: As a user I can attach a repeat schedule to a WorkTask and see the last and next scheduled run timestamps.
visibleText: Scheduler, Last run, Next run, Expression, Active, Job active, Job paused, Save, Remove, Source
sizeToken: task-scheduler-panel, task-scheduler-grid, ui-field, ui-button, ui-button--sm
states: no expression, unsaved valid expression, schedule saved, active, paused, crew snapshot source visible, missing crew
interactions: edit expression, toggle active, create active schedule row, save schedule without changing active state, remove schedule
dataSource: coworkStore.scheduledTasks active state; workTasksStore schedule draft fields; workTaskScheduleService crew snapshot metadata
accessibility: Native checkbox and labeled expression field; save/remove buttons expose text labels and disabled states.
tests: TaskPanels.test.tsx, TasksView.schedule.test.tsx, db.rs delete_work_task_removes_matching_schedule
codeRefs: app/src/components/tasks/TaskSchedulerPanel.tsx, app/src/engine/tasks/workTaskScheduleService.ts
screenshots: C:/Users/Riege/AppData/Local/Temp/open-cowork-qa/tasks-desktop.png
rationale: Scheduling belongs to WorkTask identity; the matching scheduled_tasks row uses the same id.
---

# Task Scheduler Panel

The scheduler panel edits the schedule contract for the selected WorkTask. It does not create independent task identities. Toggling Active with a valid expression creates the matching scheduler row if one does not already exist.

When a scheduler row exists, the Active checkbox and hint are rendered from `scheduled.active`. `task.scheduleEnabled` is only the fallback for drafts without a loaded scheduler row. Saving schedule details preserves the existing `scheduled.active` value; only toggling Active changes it.
