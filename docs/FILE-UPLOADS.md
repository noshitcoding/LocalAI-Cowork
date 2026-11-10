# Binary uploads and explicit workspace imports

This is a single-owner preview feature. It does not implement team synchronization,
remote execution, S3 storage or enterprise quotas.

## Uploading and importing are separate actions

The working-files view accepts a user-selected file. The client computes its byte size
and SHA-256 and reserves an upload using an idempotency key. It sends the original bytes
to an authenticated binary endpoint. The daemon verifies the size and digest, flushes
the immutable blob, reads it back and only then confirms `ready` in the database.
The original name is metadata; it never grants a destination path. Uploading does not
change the working directory, invoke a model or start a run.

The UI lists both incomplete and ready uploads. An incomplete transfer can be retried
by explicitly selecting matching bytes. This retries the whole file; it is not a
chunk-resumption protocol. A failed database confirmation can leave a retained blob
with a `reserved` record. Retrying that same upload verifies the existing blob and
confirms the record without creating a second upload. Corrupt retained content is
rejected rather than silently repaired.

Import opens a separate destination dialog. The user checks the target before starting.
An absent target creates a new file. An existing regular file requires an explicit
replacement selection and its observed SHA-256. Directories cannot be replaced by a file.
The destination must be inside this order's managed workspace; configured external
folder grants are not import destinations in this increment.

## Durable native action

`POST /api/v2/file-imports` creates an idempotent run with one frozen `import_file`
action and no provider. The run preserves the unrelated order draft. Read-only mode
rejects import; the other modes use the same policy and exact approval rules as
agent mutations. The approval binds the upload, metadata, path, expected previous
hash, run, executor and policy revision.

At execution the broker rechecks upload ownership, ready state, metadata, workspace
rights and retained bytes. It acquires the workspace writer lock, retains any old
file, prepares a private sibling stage, flushes it and captures its native identity.
The runtime journals the exact planned effect before publication. Create-only imports
use a native no-replace rename. Replacements require the observed contents and identity
to remain unchanged; Windows DACL or Unix mode is carried to the new file. Read-only
and Windows EFS-encrypted targets are rejected.

The broker verifies the prepared stage and resulting file. The receipt and tool
completion are committed together while the native lock is still held. The receipt
records `imported_from`, the before/after hashes and native identities. A previous
binary version can be restored through the existing receipt-based restoration flow
to an unused destination.

An uncertain effect interrupts the run. A restart cannot replay it. Exact-state
reconciliation must succeed, followed by an explicit resume. The process test injects
database receipt failures after real create and replace operations and verifies this
behavior without any configured model.

## API and compatibility

- `POST /api/v2/orders/{order}/uploads`: reserve metadata with an idempotency key.
- `PUT /api/v2/orders/{order}/uploads/{upload}/content`: verified raw octet-stream.
- `GET /api/v2/orders/{order}/uploads?offset=0`: scoped, paginated upload records.
- `GET /api/v2/orders/{order}/workspace/stat?path=...`: inspect the proposed target.
- `POST /api/v2/file-imports`: accept a native import run.

Database migration 7 adds upload records. The current runtime event schema 5 and order-export
format 7 cover import actions, model accounting and provenance. Exports contain upload metadata and
receipts, not blob contents; they are not a complete binary backup. The Rust generator
currently produces 79 shared contract definitions, OpenAPI, TypeScript and runtime validators.

## Current bounds and outstanding qualification

- 64 MiB per file, including empty and arbitrary binary files. Full-memory buffers
  are used; this is not streaming to disk.
- Two active body/native-IO transfers per daemon process and a 60-second HTTP body
  deadline. Authentication, origin checks and transfer admission precede body collection.
  Native IO retains its permit if the HTTP caller disconnects.
- No automatic import or network retry after an uncertain result. The UI keeps an
  uncertain start request for an explicit retry with the same idempotency key.
- No aggregate disk quota, retained-data garbage collector or upload deletion yet.
  Reserved records, retained blobs and stages abandoned by process termination can
  consume space. Normal unwinding removes only a verified unpublished import stage.
- Full owner/group/SACL/extended-ACL/alternate-stream preservation is not promised.
  Hostile concurrent filesystem writers, disk-full, network filesystems and physical
  power loss still require qualification. See NATIVE-SAVE.md and FILE-BROKER.md.
- The current Windows broker, runtime, HTTP, SQLite/PostgreSQL and real process tests
  pass. Broker/runtime/HTTP/SQLite and process tests also pass in the unprivileged Linux
  test container. UI and native desktop qualification are tracked separately in VERIFY.md.

Run `python tests/system/uploads.py` against a built daemon. It uses disposable
synthetic data, an ephemeral loopback port, its own child processes and injected
SQLite faults. It does not touch a live installation or require a provider.
