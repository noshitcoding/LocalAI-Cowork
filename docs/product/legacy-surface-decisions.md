---
title: Legacy Surface Decisions
type: overview
doc_type: decision-record
status: current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - legacy UI surface cleanup
  - duplicate route surface decisions
source_files:
  - app/src/App.tsx
  - app/src/components/CoworkView.tsx
  - app/src/components/LeftSidebar.tsx
  - app/src/components/RightSidebar.tsx
  - app/src/components/TasksView.tsx
removed_surfaces:
  - app/src/components/ChatView.tsx
  - app/src/components/TaskView.tsx
  - app/src/components/GlobalSearchView.tsx
  - app/src/components/WelcomeScreen.tsx
---

# Legacy Surface Decisions

## Decisions

### LS-001: Remove unrouted duplicate UI surfaces

`ChatView`, `TaskView`, `GlobalSearchView`, and `WelcomeScreen` were removed because they were not imported by the route tree and duplicated ownership now documented elsewhere:

- Chat belongs to `/` through `CoworkView`.
- User-facing work tasks belong to `/tasks` through `TasksView` and `WorkTask`.
- Search entry points stay in current command/search surfaces until a documented route is added.
- Empty chat state belongs to the active workspace shell, not a separate unrouted welcome screen.

### LS-002: Keep PlanApproval compatibility data, not the old task screen

Legacy `Task` and `task_steps` persistence remains as PlanApproval compatibility. The old `TaskView` UI is not a route and must not be reintroduced as the user-facing task surface.

### LS-003: Keep RightSidebar panels, remove the unused container export

`LeftSidebar` composes `ProgressPanel`, `DocumentWorkspacePanel`, `WorkingFolderPanel`, `OutputsPanel`, and `ContextPanel` directly. The old `RightSidebar` default container is not part of the shell layout and was removed while preserving the named panel exports.

## Review Rule

If a removed surface is needed again, add or update a route/element doc first, then wire it through the app route tree intentionally.
