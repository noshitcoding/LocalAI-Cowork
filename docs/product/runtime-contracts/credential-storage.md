---
title: Credential Storage and Redaction Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-10
last_verified: 2026-07-10
endpoint: Tauri IPC credential_set, credential_get, credential_delete, and secure_config_migrate; protected backend upsert and runtime commands
purpose: Keep provider, connector, crew, engine, MCP, terminal, memory-provider, tool-gateway, and sandbox secrets out of browser persistence, SQLite, exports, logs, and audit files.
userStory: As a user I can restart Open Cowork without re-entering credentials or exposing them in ordinary application data.
visibleText: secure-storage startup failure and field-level save failure
sizeToken: none
states: initializing, ready, unavailable, migrating, saved, deleted, save-failed
interactions: migrate legacy secret, save credential, load credential, delete credential, retry unavailable storage
dataSource: Windows Credential Manager for GA; volatile memory only in browser development and tests
accessibility: startup failure uses an alert region; field save failures use role alert and aria-invalid
tests: credential_store.rs tests, secure_config.rs tests, secure_config migration/runtime integration test, credentialPersistence.test.ts, memoryStore.security.test.ts, pipelineStore.test.ts, redaction.test.ts, audit_service.rs tests
source_files:
  - app/src-tauri/src/credential_store.rs
  - app/src-tauri/src/sensitive_data.rs
  - app/src-tauri/src/secure_config.rs
  - app/src/security/credentialVault.ts
  - app/src/security/credentialMigration.ts
  - app/src/security/credentialPersistence.ts
  - app/src/security/redaction.ts
  - app/src/App.tsx
canonical_for:
  - operating-system credential storage
  - legacy plaintext credential migration
  - secret-free Zustand persistence
  - referenced secure JSON configuration blobs
  - audit and application-log redaction
rationale: WebView localStorage, general SQLite JSON columns, exports, and logs are not credential stores. Migration must never erase the only surviving copy of a secret.
---

# Credential Storage and Redaction Contract

The Windows GA build stores credential values in Windows Credential Manager. The credential account name contains a version, an allowlisted scope, and a SHA-256 digest of the logical owner and field; profile IDs, connector IDs, environment names, and user values are not exposed in the account name. Access to the native backend is serialized because Windows credential operations against the same entry are not assumed to be concurrency-safe.

The IPC boundary accepts only allowlisted scopes and non-empty, bounded, control-character-free locator parts. Credential values are limited to 64 KiB. Empty writes delete an entry. Backend failures return fixed messages that contain neither the locator nor the value. Non-Windows production builds fail closed instead of silently selecting an in-memory credential backend. Browser development and unit tests use process-local volatile storage and therefore do not promise persistence across reloads.

At desktop startup, product routes remain unmounted until credential initialization finishes. For each configured field, an existing OS credential takes precedence. If no credential exists, a legacy in-memory value hydrated from localStorage is written to the OS store. Only after every migration and read succeeds are the stores updated, which rewrites browser persistence with empty values while retaining field and MCP environment-key metadata. A partial failure leaves legacy persistence untouched and presents a retry state. Retrying is idempotent.

Provider API keys, connector API keys and webhook URLs, crew-specific provider keys, the legacy engine key, and every MCP environment value follow this contract. Secret inputs commit on blur or Enter and update runtime state only after native storage succeeds. Escape discards an uncommitted edit. MCP runtime operations await environment persistence before starting, probing, restarting, or calling a tool. Crew exports always replace provider keys with empty strings.

Terminal backend configuration, memory-provider configuration, tool-gateway configuration, and worker-sandbox environment JSON are protected as complete blobs rather than by guessing sensitive field names. Each write creates a new random revision in the OS store and commits only a typed versioned marker to SQLite. If the database commit fails, the new revision is deleted and the prior marker remains runnable. Successful replacement removes the prior revision. Startup migration converts legacy plaintext rows idempotently before product routes mount. Malformed, cross-scope, missing, or unsupported references fail closed.

Backend list and get commands return only reference markers. Terminal and sandbox execution resolve a marker inside Rust immediately before constructing the process environment. Gateway configuration is never injected into model context. Backend-only credential scopes are rejected by the generic WebView credential IPC commands even if a caller knows the row ID and revision. Browser fallback migrates old memory-provider and gateway localStorage records once, removes their plaintext keys, and remains volatile when no desktop runtime exists.

Application log details and frontend audit payloads pass through recursive redaction before persistence or IPC. The Rust audit writer repeats redaction so direct backend events have the same boundary. Sensitive key families, complete environment/header objects, bearer credentials, and credential-like query parameters are replaced with `[REDACTED]`; circular or excessively deep frontend structures are truncated. Redaction preserves operational values such as status, duration, counts, and non-sensitive endpoint origins.

Deleting a profile, connector-owned value, MCP server, or crew credential deletes the corresponding OS entry before removing its metadata where the product exposes a delete action. An unavailable credential store is a release-blocking startup condition for the Windows desktop build. Recovery consists of unlocking or repairing Windows Credential Manager and retrying; Open Cowork never falls back to writing the value into localStorage or SQLite.
