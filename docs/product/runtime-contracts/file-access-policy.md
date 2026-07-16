---
title: File Access and Sandbox Policy Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-10
last_verified: 2026-07-10
endpoint: Tauri IPC fs_*, exec_command, shell_command_validate, worker_sandbox_*
purpose: Enforce canonical filesystem roots and sandbox boundaries in Rust for agent-driven file and shell operations.
userStory: As a user I can grant explicit workspace roots and know that agent file operations cannot escape them through traversal, sibling prefixes, links, backup names, or sandbox identifiers.
visibleText: none
sizeToken: none
states: allowed, denied, stale-root, read-only, sandboxed
interactions: grant root, validate path, read, mutate, restore backup, create sandbox, validate AI shell command
dataSource: SQLite allowed_folders and worker_sandboxes; app-data backups and worker_sandboxes directories
accessibility: not applicable
tests: file_safety.rs tests, worker_sandbox.rs tests, lib.rs security tests, registry.filesystem.test.ts, registry.terminal.test.ts
source_files:
  - app/src-tauri/src/file_safety.rs
  - app/src-tauri/src/worker_sandbox.rs
  - app/src-tauri/src/lib.rs
  - app/src/engine/tools/registry.ts
canonical_for:
  - filesystem allow-root enforcement
  - worker sandbox path containment
  - AI shell policy validation
rationale: Frontend permission checks improve UX but are not a security boundary. Privileged paths are canonicalized and authorized again in Rust.
---

# File Access and Sandbox Policy Contract

Agent-driven file access is deny-by-default when no valid allowed folder exists. Paths must be absolute. Existing paths are canonicalized directly; missing targets are resolved from their nearest existing ancestor before the allow-root comparison. Component-aware `Path::starts_with` checks prevent sibling-prefix matches such as `workspace-escape` for an allowed `workspace` root. Missing or invalid stored roots are ignored while remaining valid roots continue to work.

Read and mutation commands use the same Rust policy boundary. Worker runs use their persisted `allowed_roots_json`; ordinary runs use SQLite `allowed_folders`. Read-only sandbox roots additionally reject writes. Agent mutation commands also enforce the active toolset in Rust: writes and deletes map to `edit_file`, while create-directory, move, and copy operations use their dedicated capabilities. A disabled profile therefore remains authoritative even if a caller bypasses the frontend dispatcher.

Recursive copy and move fallback never follow symbolic links or Windows junctions. Backup restore accepts only a single file name, canonicalizes the selected backup, and verifies that it remains a regular file under the app-owned backup directory.

Sandbox identifiers are opaque ASCII identifiers containing only letters, digits, `-`, and `_`, with a maximum length of 128 bytes. Snapshot sources must be inside either the active parent sandbox roots or globally allowed folders. Duplicate sandbox IDs are rejected before files are changed. Destroy operations require both a valid ID and a persisted sandbox record.

The visible AI terminal calls `shell_command_validate` before writing to its PTY. This applies the same tool flag, sandbox capability, working-directory, traversal, absolute-path, elevation, and dangerous-pattern checks used by `exec_command`. Rejected commands are never written to the terminal session.

Bounded Windows commands are attached to a Job Object with `KILL_ON_JOB_CLOSE`; timeout termination uses the job directly and keeps `taskkill /T /F` only as a compatibility fallback. Unix commands run in a dedicated process group whose guard terminates remaining descendants. The guard is released before output-reader joins, so successful parents cannot leave background children holding inherited pipes open. Integration tests cover timeout and normal-parent-exit descendants.

## Security Boundary

Path validation is authoritative for Rust file commands. Shell validation is a policy guard, not a complete operating-system filesystem sandbox: an arbitrary interpreter can derive paths dynamically or use capabilities not visible in command text. Strong isolation therefore requires `workspace_copy` mode with the smallest possible roots; untrusted workloads must set `allow_shell_execution=false`. Raw terminal input is considered an explicit user action and is not classified as an autonomous agent tool call.
