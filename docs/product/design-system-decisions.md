---
title: Design System Decisions
type: overview
doc_type: decision-record
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - current design-system conventions
  - UI styling decisions
source_files:
  - app/src/App.css
  - app/src/index.css
  - app/src/components/Layout.tsx
  - app/src/components/SettingsView.tsx
  - app/src/components/FeaturesView.tsx
  - app/src/components/CrewPanel.tsx
related_docs:
  - docs/product/current-routes.md
---

# Design System Decisions

## Current Foundation

The app uses Tailwind base layers, but the implemented design system is custom CSS in `app/src/App.css`. Tokens live in `:root` and `[data-theme='dark']`.

## Decisions

### DS-001: Use existing CSS tokens first

New UI should reuse `--bg-*`, `--text-*`, `--border-*`, `--accent`, semantic status colors, radius tokens, shadows, and `--transition` before adding new colors or spacing systems.

### DS-002: Desktop productivity shell is the base layout

The primary product shape is a dense desktop shell: 48px top bar, persistent left sidebar, route content area, panels, compact controls, and scan-friendly lists. Avoid landing-page or marketing-page composition for app screens.

### DS-003: Icons come from `lucide-react`

Navigation, toolbar, panel, and action icons should use `lucide-react` where an icon exists. Text-only buttons are acceptable for clear commands, but destructive or compact actions should use existing icon-button patterns.

### DS-004: Cards and panels have different roles

Use `panel` for grouped settings/tool surfaces and `card` for repeated items such as feature cards. Avoid creating a new nested-card pattern. Crew screens currently use a more expressive rounded panel style; do not spread Crew-specific styling outside Crew without an intentional redesign.

### DS-005: Theme and density are user preferences

Light/dark theme is applied through `data-theme`; compact mode and font scale are applied globally from preferences. New UI must remain usable with compact mode, dark theme, and 85-120% font scale.

### DS-006: Accessibility states are part of the style contract

Interactive controls should include focus-visible styles, stable hit targets, labels or aria labels, and keyboard behavior when they are resizable, modal, tabbed, or navigational. Existing examples include the sidebar resize separator, settings tablist, shortcut dialog, and project delete modal.

### DS-007: Internationalization is current behavior

Use `tr(...)` or `useTranslation()` for user-visible strings in app UI. Current content is mixed English/German in places; new docs should describe current behavior and new UI should avoid adding untranslated strings unless the surrounding file already uses literal strings.

## UI Update Rule

When changing visual conventions, update this doc with the decision and source files. For purely local styling fixes, prefer editing the component/CSS and only update this doc if the convention changes.
