---
title: Observability and Support Bundle Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-11
last_verified: 2026-07-11
endpoint: Tauri IPC audit_event, gateway_logs_tail, and support_bundle_create; SQLite run, scheduler, crew, and audit sinks
purpose: Provide bounded operational diagnostics without turning logs or support exports into a secondary store for prompts, tool output, configuration, or credentials.
userStory: As a user I can export useful diagnostics for support without disclosing my work content or secrets by default.
visibleText: support bundle progress, success, and failure states in System settings
sizeToken: none
states: idle, writing, rotated, retained, exporting, exported, export-failed
interactions: append audit event, inspect gateway tail, create support bundle, overwrite selected bundle
dataSource: bounded SQLite diagnostic tables and rotated audit JSONL metadata
accessibility: export is a labeled icon button; completion uses a status region and failure uses an alert region
tests: sensitive_data.rs tests, audit_service.rs rotation and redaction tests, db.rs diagnostic sink and retention tests, support_bundle.rs whitelist/leak tests, SettingsView.test.tsx support export test
source_files:
  - app/src-tauri/src/sensitive_data.rs
  - app/src-tauri/src/audit_service.rs
  - app/src-tauri/src/audit.rs
  - app/src-tauri/src/db.rs
  - app/src-tauri/src/support_bundle.rs
  - app/src-tauri/src/ollama.rs
  - app/src-tauri/src/lib.rs
  - app/src/components/SettingsView.tsx
canonical_for:
  - diagnostic payload bounds and retention
  - audit JSONL rotation
  - support bundle contents and exclusions
  - support bundle manifest and overwrite behavior
rationale: Operational data must be useful enough to diagnose failures while remaining bounded, content-minimized, and independently protected at every persistence and export boundary.
---

# Observability and Support Bundle Contract

Every persistent diagnostic boundary applies redaction and size limits inside Rust, even when the caller already sanitized its payload. Summary fields are limited to 8 KiB, free-form diagnostic text to 64 KiB, structured diagnostic JSON to 256 KiB, and a serialized audit event to 128 KiB. Truncation is UTF-8 safe. Oversized valid JSON remains valid through a bounded truncation envelope. Sensitive keys, complete environment and header objects, bearer credentials, credential-like query values, and common provider token formats are replaced before persistence.

SQLite enforces these rules in the insert/update methods for engine runs and events, checkpoints, scheduled-run results and errors, crew snapshots, crew logs and events, and database audit events. Retention is enforced at the same boundary: the newest 2,000 engine events per run, 2,000 crew events per run, 5,000 crew log records per run, 500 scheduler runs per task, and 10,000 database audit events are retained. Sequence values remain monotonic when old engine events are removed.

The JSONL audit writer serializes concurrent appends through a process-wide lock. Every new event is redacted and bounded before it is linked through the versioned HMAC-SHA-256 contract in `audit-integrity.md`. The active file rotates before it would exceed 5 MiB, and at most three prior files are retained. Area and action are diagnostic identifiers rather than free-form text; invalid labels are replaced. The gateway log tail captures at most 1,000 active-file lines from the same locked byte snapshot used for verification, denies tampered or unavailable logs, and repeats redaction plus a 16 KiB per-line limit before returning data to the WebView. Native Ollama logs record endpoint origins, model names, durations, sizes, and error classes; URL credentials and raw non-JSON stream content are not logged.

The support bundle is a ZIP written to an absolute user-selected `.zip` path. It is assembled through an explicit field whitelist, not by copying application directories or recursively redacting arbitrary objects. It contains `README.txt`, `diagnostics/system.json`, `diagnostics/database.json`, `logs/audit.jsonl`, and `manifest.json`. The manifest declares schema version `1`, redaction policy `whitelist-v1`, byte size, and SHA-256 for every payload file. A replacement is written and synced to a unique sibling file first; an existing target is preserved under a temporary previous name until the new bundle is installed.

System diagnostics contain only app version, bundle version, timestamp, operating-system family, CPU architecture, the validated startup-recovery timestamp/count report, and the path-free audit-integrity report. Database diagnostics contain schema version, aggregate row counts, and recent run states with timestamps and boolean error presence. Audit export contains only timestamp, area, and action for the latest 500 active-file events, and only when integrity status permits reading. Tampered or unavailable history produces an empty audit tail while preserving the safe integrity status and fixed error code. All labels and timestamps are validated again while assembling the bundle.

The bundle never includes the SQLite database, database backups, credentials, secure-configuration references, provider configuration, environment variables, headers, prompts, messages, model responses, reasoning, tool input or output, file paths, workspace names, file contents, audit details, application log files, or crash payloads. Adding any new field requires updating the whitelist, leak test, manifest contract, and this document. Export failures return a fixed UI failure state without displaying raw backend errors or the selected path.
