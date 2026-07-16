---
title: Documentation Decisions
type: overview
doc_type: decision-record
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - documentation governance
  - current docs update rules
source_files:
  - docs/SOURCE_OF_TRUTH.md
  - docs/ARCHITECTURE_CURRENT.md
related_docs:
  - docs/SOURCE_OF_TRUTH.md
---

# Documentation Decisions

## Decisions

### DD-001: Current docs live outside V1 planning artifacts

Current source-of-truth docs are `docs/SOURCE_OF_TRUTH.md`, `docs/ARCHITECTURE_CURRENT.md`, and `docs/product/**/*.md`. Do not update V1 planning docs to describe current behavior unless the user explicitly asks for V1 maintenance.

### DD-002: Seed docs describe implemented behavior only

These docs intentionally avoid roadmap promises. Planned behavior belongs in a planning doc or backlog until implemented and verified against source files.

### DD-003: Frontmatter is required for current source-of-truth docs

Use the frontmatter fields listed in `docs/SOURCE_OF_TRUTH.md`. `source_files` should point to the implementation files used for verification.

### DD-004: The old architecture doc is superseded, not deleted

`docs/ARCHITECTURE.md` is retained for historical context and should carry only a short notice pointing to `docs/ARCHITECTURE_CURRENT.md`.

### DD-005: Keep canonical docs concise

The canonical docs should answer "what is true now" and "where is it implemented." Detailed audits, screenshots, plans, and migration narratives belong in separate evidence or planning docs.

### DD-006: Code changes should update docs at the ownership boundary

When a code change alters routes, domain objects, UI conventions, runtime support, or architecture boundaries, update the matching source-of-truth doc in the same change.

## Review Triggers

- Route tree or navigation changes.
- New persisted domain object or persistence migration.
- New model/provider runtime mode.
- New privileged tool or changed policy behavior.
- Tauri, Node, React, Vite, TypeScript, Rust, or packaging target changes.
- UI shell/design-token changes.
