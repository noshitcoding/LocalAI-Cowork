# Native directory and file operations

The shared tool broker supports `stat_file`, `create_directory`, `rename_file` and
`remove_file`, plus receipt-based `restore_file`, for managed working directories and explicitly selected writable folder
grants. The four access modes use the same policy and exact approval path as text writes.
`stat_file` is a read action; every other operation is a mutation.

## Contracts and retained content

`stat_file` returns an optional `FileNode` with type, SHA-256, size and native identity.
For a directory, the hash covers the sorted tree manifest. Reading a tree also retains
its regular-file content in the private content-addressed version store. The source
directory itself is unchanged.

Creating a directory requires an absent path and an existing parent. Rename/move accepts
a source, an absent destination within the same folder grant, and the source hash returned
by `stat_file`. It cannot replace another entry or move a directory into itself. Removal
requires the same expected source hash and retains both the contents and manifest.
The broker rechecks content and native identity immediately before publishing an operation.
Binary regular files and empty directories are supported. Existing broker limits apply:
64 MiB per retained file, 512 MiB per tree, 10,000 entries and depth 64. The UTF-8 text
editing interface retains its separate 1 MiB limit.

Removal renames the entry to a reserved hidden sibling in the same directory. This keeps
the move within its filesystem and avoids recursive host deletion. Normal listings and
run snapshots exclude this internal tombstone. Private version blobs and the receipt's
manifest retain the pre-operation contents independently. Agents cannot address reserved
`.cowork-` paths. A directory created for publication also starts under a reserved name.

There is no garbage collection yet. Tombstones, unreferenced prepared
directories and retained blobs continue to consume space. Export format 7 includes
operation receipts and manifests, **not** the retained binary contents; an order JSON
export is not a full backup. Retention quotas and verified cleanup remain
required work before full file-management acceptance.

## Restoring retained content

Text receipts offer the previous or written version. Directory-operation receipts offer
the retained tree. The UI opens a destination dialog and creates a separate durable run
through `POST /api/v2/file-restorations`. Its default mode requires approval of the exact
restore action. Starting it preserves the order's draft. Model profiles and credentials
are unnecessary: the frozen run has `provider: null` and one `file_action`.

The action still uses the common queue, workspace ownership, policy, approvals, leases,
tool journal, cancellation, reconciliation and final workspace snapshot. It cannot call a
model or gain other tools. Denied or failed native actions end as failed, not completed.
The active UI labels these as native file actions rather than showing a model name.

The source names an immutable receipt, its run and the selected before/after version.
Runtime checks require the same order, project, folder identity and current writable
grant. Raw version hashes and private paths cannot be used as restore capabilities.
Every source blob and directory manifest is verified again, including size, structure,
path and tree limits. Contents are materialized into independent files in a hidden stage
under the target parent, then published without replacing an existing entry. The original
tombstone and retained blobs stay intact. If the original parent is gone, the user must
choose an existing parent and a free destination.

Restoration receipts record `restored_from` provenance. An uncertain restoration follows
the same exact-state reconciliation and separate resume flow as other mutations. The
process test removes all configured providers and injects a failed restoration receipt
after actual publication, then verifies recovery without a second restore or model call.

Migration 6 distinguishes `agent` and `file_action` runs and prevents older preview
binaries from accepting a database containing the new run type. The current runtime event
schema is 4; the API path remains `/api/v2`. Existing model-run JSON and source-history hashes
stay readable. Export format 7 includes native action plans, upload metadata, model accounting and provenance.
Import uses the same execution and receipt machinery, with its explicit replacement rules
described in FILE-UPLOADS.md.

## Durable execution and reconciliation

1. Validate the frozen grant, policy, expected hash and paths; acquire the broker's native
   writer lock; preserve contents and prepare the exact receipt.
2. Store the in-flight tool, receipt and start event in one database transaction.
3. Publish using a native rename which refuses an existing destination (an explicitly checked
   import replacement is the separate exception described in FILE-UPLOADS.md). Verify the full
   resulting state; Linux also syncs the pinned parent directories.
4. Commit the receipt, tool result, completion event and cleared in-flight marker together.
   Keep the native lock through that confirmation.

The runtime renews its execution lease while waiting for native IO and observes cancellation,
shutdown and time budgets. Dropping the wait does not claim that a blocking filesystem
operation was canceled. Its native lock remains owned until that operation exits; the
persisted in-flight marker requires reconciliation if the outcome is uncertain.

After an uncertain completion, the run is interrupted. Resume and cancel cannot hide the
unresolved effect. The user chooses either the unchanged original or the completed result.
The broker checks native identity, content hashes and both relevant paths, including the
removal tombstone. Only an exact match clears the marker. This decision never performs the
operation again and never resumes the run automatically. A separate resume is required.

Schema migration 5 stores operation receipts with a unique `(run_id, tool_call_id)` key.
The shared SQLite/PostgreSQL contract checks that a duplicate receipt rolls back state,
events and text-change receipts together.

## Platform implementation and limits

Linux uses `renameat2(RENAME_NOREPLACE)` with pinned directory descriptors. Windows uses
`NtSetInformationFile` and `FILE_RENAME_INFORMATION` with a target directory handle and
`ReplaceIfExists = false`. There is no replacing or cross-volume copy fallback. Microsoft
documents these fields and the native call in
[FILE_RENAME_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)
and [NtSetInformationFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetinformationfile).

Symlinks/junctions, hard-linked files, special files, traversal and unsupported names remain
rejected. This broker is not an isolated executor for hostile native programs. Uncooperative
external editors can race filesystem checks; shared hostile writers, network filesystems,
ReFS and physical power failure have not been qualified. The OS-enforced absent destination
prevents replacement even when a target appears after preparation.

`tests/system/file_operations.py` executes actual native operations against disposable data.
It injects a failed database receipt after each of the three mutations, kills/restarts the
daemon, rejects incorrect reconciliation and confirms no operation replay. It additionally
checks retained content, export hashing and idempotency. Run it with an explicit `--binary`
for a separately built Linux daemon. See [VERIFY.md](VERIFY.md) for the platform-specific
results and remaining qualification gaps.
