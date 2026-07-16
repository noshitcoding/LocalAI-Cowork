---
title: Audit Integrity and Threat Model Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-11
last_verified: 2026-07-11
endpoint: Backend audit JSONL writer, startup verification, gateway health and log tail, and support bundle export
purpose: Make retained audit history tamper-evident without exposing signing material or overstating the guarantees of a local audit log.
userStory: As a user I can detect unexpected audit-history changes and still export content-minimized diagnostics without exposing credentials or event details.
visibleText: audit subsystem health, degraded legacy or partial-chain status, and fixed integrity failure states
sizeToken: none
states: empty, ok, legacy, tampered, unavailable
interactions: append signed event, verify retained chain, rotate archive, inspect health, export verified audit metadata
dataSource: rotated audit JSONL plus backend-only Windows Credential Manager signing key and signed-history marker
accessibility: integrity status is represented by the existing labeled gateway subsystem row and status semantics
tests: audit_service.rs HMAC, tamper, deletion, reorder, truncation, legacy, restart, marker, and rotation tests; support_bundle.rs tampered-tail exclusion test
source_files:
  - app/src-tauri/src/audit_service.rs
  - app/src-tauri/src/audit.rs
  - app/src-tauri/src/lib.rs
  - app/src-tauri/src/support_bundle.rs
canonical_for:
  - audit HMAC chain format and verification order
  - audit signing-key and history-marker trust boundary
  - legacy and rotation compatibility
  - audit integrity threat model and non-goals
rationale: A local log can provide useful tamper evidence only when its cryptographic key and existence marker live outside the editable log directory and its limitations are explicit.
---

# Audit Integrity and Threat Model Contract

## Protected assets and trust boundaries

The protected asset is the order and content of retained backend audit events. The audit directory is untrusted input during every verification, including at startup. The HMAC-SHA-256 signing key and the signed-history marker are fixed backend-only credentials in Windows Credential Manager. They are not exposed through Tauri IPC, serialized into support bundles, written to SQLite, or stored in the WebView.

The relevant attacker can edit, truncate, reorder, replace, or delete files in the application data directory while lacking access to the signing key and marker. The operating-system credential boundary, the running Rust process, and the released binary are trusted for this control. An attacker with equivalent same-user Credential Manager access, arbitrary code execution inside the process, debugger-level memory access, or control of the shipped binary is outside this guarantee.

## Signed record format

New JSONL records preserve the existing top-level `timestamp`, `area`, `action`, and `details` fields and add an `integrity` object with `version`, `sequence`, `previousMac`, and `mac`. Version `1` uses HMAC-SHA-256. The MAC covers a typed, deterministic JSON representation of all event fields plus integrity version, sequence, and previous MAC, but excludes the current MAC value.

Labels and details are redacted and bounded before signing. Verification reconstructs the same typed representation; it never accepts an arbitrary JSON map serialization as canonical input. MAC values use exactly 64 lowercase hexadecimal characters. Sequence `1` has no previous MAC; later records require one.

The writer holds a process-wide lock, verifies every retained archive from oldest to active, calculates the next sequence, rotates if required, appends one newline-terminated record, and calls `sync_data`. Any failed integrity check prevents the append. The key is generated from operating-system randomness only when no key and no non-empty audit history exist. Missing or malformed key material with existing history fails closed.

## Rotation and legacy compatibility

The active file remains limited to 5 MiB with three retained archives. Archives are verified oldest to newest. Because normal retention deletes the oldest archive, the first retained signed record is a cryptographically verified partial anchor when its sequence is greater than `1`. Subsequent records must have contiguous sequences and exact previous-MAC links. `chainComplete` is false for that expected partial-window condition, and gateway health reports it as degraded rather than claiming a complete history.

Unsigned legacy records are accepted only as one contiguous prefix before signed records. A legacy-only log or a valid legacy prefix produces status `legacy`; an unsigned record after signing begins is `tampered`. Malformed JSON, invalid UTF-8, empty interior records, unsupported versions, non-canonical MAC encoding, missing final newline, MAC mismatch, sequence gaps, and link mismatch are `tampered` with fixed non-sensitive error codes.

After the first signed event is synced, a separate Credential Manager marker records that signed history exists. Startup can recover the narrow crash window where the event was synced but the marker was not yet persisted. Once present, that marker makes complete removal of all signed files detectable. It contains no event data, sequence, MAC, path, or key material.

## Status and product behavior

- `empty`: no retained legacy or signed events and no contradictory signed-history marker.
- `ok`: every retained signed event verifies; `chainComplete` distinguishes a full chain from a retention anchor.
- `legacy`: a valid unsigned prefix exists and all following signed records verify.
- `tampered`: retained bytes, order, links, required history, or format violate the contract.
- `unavailable`: key, marker, lock, or readable storage required for verification is unavailable.

Startup verifies before database recovery and worker startup but does not crash the application on a failed audit check, because the user still needs health and support diagnostics. Audit appends fail closed. Gateway health uses fixed messages and path-free report fields. Gateway and support consume only active-file lines captured from the same locked byte snapshot that was cryptographically verified, eliminating a verify-then-read race. The WebView log tail is denied for `tampered` and `unavailable`. A support bundle still succeeds, includes the whitelisted integrity report, and exports an empty audit tail for those states.

## Guarantees and non-goals

Within the connected retained chain, the verifier detects event-field edits, secret-preserving or secret-removing rewrites, inserted unsigned records, middle deletion, sequence gaps, reordering, truncation, malformed records, and replacement signed with another key. The external marker also detects complete deletion after a signed event has been committed.

The control cannot prove whether an archive older than the configured retention window existed, and removal of only the oldest retained prefix is indistinguishable from normal retention; the partial anchor makes that limitation visible. It does not provide immutability, remote transparency, legal non-repudiation, trusted wall-clock time, protection from same-user credential compromise, or evidence against a compromised running process. No telemetry or remote audit copy is created.

These limitations are part of the product contract. UI and documentation must use `tamper-evident` or `integrity-verified`, never `immutable` or `non-repudiable`.
