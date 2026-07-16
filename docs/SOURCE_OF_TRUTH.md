---
title: Documentation Source Of Truth
type: overview
doc_type: source-of-truth-index
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - current documentation precedence
  - source-of-truth maintenance rules
source_files:
  - app/src/App.tsx
  - app/src/components/Layout.tsx
  - app/src-tauri/src/lib.rs
supersedes:
  - docs/ARCHITECTURE.md
related_docs:
  - docs/ARCHITECTURE_CURRENT.md
  - docs/product/current-routes.md
  - docs/product/core-domain-decisions.md
  - docs/product/design-system-decisions.md
  - docs/product/runtime-compatibility.md
  - docs/product/documentation-decisions.md
---

# Documentation Source Of Truth

This file defines which docs describe the current product. It does not change runtime behavior.

## Precedence

1. Current app code is authoritative for implemented behavior.
2. `docs/SOURCE_OF_TRUTH.md`, `docs/ARCHITECTURE_CURRENT.md`, and `docs/product/**/*.md` are the current documentation source of truth.
3. `README.md` and operational docs summarize public or task-specific workflows and should link back here when behavior conflicts.
4. `docs/V1_*` and other V1 planning docs are historical planning artifacts unless a current source-of-truth doc explicitly adopts a decision from them.
5. `docs/ARCHITECTURE.md` is superseded and retained as historical context.

## Current Canonical Docs

| File | Canonical For |
| --- | --- |
| `docs/ARCHITECTURE_CURRENT.md` | Current implementation architecture and major boundaries |
| `docs/product/current-routes.md` | Current route map, navigation shell, and settings sections |
| `docs/product/core-domain-decisions.md` | Current product/domain model decisions |
| `docs/product/design-system-decisions.md` | Current UI/design-system conventions |
| `docs/product/runtime-compatibility.md` | Current supported runtime assumptions and version compatibility |
| `docs/product/documentation-decisions.md` | Documentation rules, update triggers, and frontmatter contract |

## Frontmatter Contract

Use these fields where applicable:

- `title`: human-readable document title.
- `doc_type`: one of `source-of-truth-index`, `current-state`, `decision-record`, or `compatibility`.
- `status`: use `seed-current`, `current`, `needs-review`, `deprecated`, or `superseded`.
- `owner`: owning docs area or team.
- `last_updated`: ISO date of the latest edit.
- `last_verified`: ISO date when the source files were last checked.
- `canonical_for`: list of behaviors or decisions this doc owns.
- `source_files`: implementation files used to verify the content.
- `supersedes`: older docs replaced by this doc.
- `related_docs`: docs that should be read together.

## Update Rule

When app behavior changes, update the smallest canonical doc that owns that behavior in the same change. If the behavior is not owned yet, add a concise source-of-truth section instead of expanding historical V1 docs.
