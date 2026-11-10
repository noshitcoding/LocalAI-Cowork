# Explicit native folder access

This is the first M4 file broker. It supports directory listings and UTF-8 text files up to
1,000,000 bytes. It does not enable a shell, desktop automation, MCP processes or arbitrary
binary-file editing. The daemon is still a single-owner preview.

## Configure and use

The desktop now provides a native picker, explicit rights confirmation and durable revocation
under **Folder context**. Read-only is the default. See [native folder management](NATIVE-FOLDERS.md)
for the user flow, authority boundary and qualification limits. Registration no longer requires
editing configuration or restarting the daemon.

For controlled development fixtures, the older explicit configuration remains supported:

1. Create a local project and obtain its ID from `GET /api/v2/projects`.
2. Stop the development daemon after its active work has finished.
3. Create `folders.json` in its `COWORK_DATA_DIR`, using an explicitly chosen existing folder:

```json
[
  {
    "id": "my-work-folder",
    "project_id": "the-existing-project-id",
    "name": "My working documents",
    "path": "C:\\Users\\example\\Documents\\WorkingDocuments",
    "writable": false
  }
]
```

On Linux, use an absolute native path. Set `writable: true` only when edits are intended.
Restart the same daemon and select the folder using **Folder context** in the order composer.
Selection applies to the next run, starts empty when opening an order, and never changes an
existing run's frozen grants. A branch does not inherit external folder access; its own managed
working directory is copied from the selected historical snapshot (see WORKSPACES.md).
No directory is scanned or uploaded on registration. Reads requested during a run can enter
the context sent to the explicitly selected provider.

The browser cannot create host grants. Changing legacy configuration requires a daemon restart;
native registration and revocation do not. Old grants fail validation after a configuration or
root-identity change. Use native revocation rather than deleting legacy configuration while
an uncertain write still needs reconciliation. Exact journal reconciliation remains possible
after revocation without allowing new agent IO.

## Authority and path checks

- Each public grant contains a project, display name, write permission, configuration hash
  and physical-resource identity. The host path remains in native configuration.
- Preflight and execution validate exact grant snapshots. Tools use the common policy path;
  read-only denies writes, confirm-mutations asks for writes, confirm-all also asks for reads,
  and autonomous remains limited to the selected native grants.
- Only relative slash-separated paths are accepted. Traversal, absolute paths, ADS, Windows
  device names, trailing dots/spaces and broker temporary names are rejected.
- Open directory handles and capability-relative operations constrain traversal. Symlinks,
  junctions, hard-linked files, special files and oversized files are rejected. Listings are
  capped at 500 entries and indicate truncation.
- Overlapping roots are rejected. The daemon also rejects roots overlapping its private data
  directory, including grants that would expose the database, token, or version storage.
- A run with writable grants reserves those physical resources in the application database.
  Another writer waits in `waiting_for_executor`. Ownership survives approval waits and
  interruption, and is released with a terminal state only after uncertainty is cleared.
  Acquisition uses a stable resource order; claims scan beyond the first 100 blocked entries.
- OS file locks additionally serialize actual broker effects and reconciliation across
  processes using the same broker state directory. A reconciliation cannot pass while an old
  writer still holds its file lock. Locks are released when that process exits.

## Durable write protocol

`write_text_file` requires `expected_sha256` from a previous read, or null for creation only.
Parent directories must already exist. It cannot silently fall back to unconditional overwrite.

1. Validate scope, policy, arguments and the current file hash; acquire the writer lock.
2. Save the original and proposed contents in private content-addressed storage.
3. Commit the exact in-flight tool and proposed change to the run checkpoint and event stream.
4. Write and sync a temporary file, recheck the expected original, then publish it in the same
   directory. Creation uses a no-clobber hard link. Replacement uses rename. Linux also syncs
   the directory. Read back and verify the proposed hash.
5. In one transaction, record the file receipt, tool result and completion event and clear the
   in-flight marker. Hold the OS lock through this confirmation.

An error after publication, a stale worker, or a failed database confirmation preserves the
marker and interrupts the run. No automatic retry occurs. A known pre-publication conflict
returns a tool error without claiming a successful change.

The UI displays retained versions and hashes. For an interrupted uncertain change it offers:

- **Accept completed change:** only succeeds if the actual file equals the proposed hash.
- **Confirm unchanged original:** only succeeds if the original hash still matches, including
  absence when creating a new file.

Both decisions require the exact run/tool fingerprint. Reconciliation itself never repeats a
write or resumes the run. Resuming is a separate explicit action. If neither hash matches,
the file conflict remains visible and requires manual recovery; force-overwrite is unavailable.
Canceling cannot conceal an unresolved effect. Receipts are included in format-7 order exports;
those JSON exports do not contain the private file-version blobs and are not full backups.

## Generated documents

The built-in document tools (`document_write`, `image_edit`) publish engine output through
`prepare_generated` as `FileOperationKind::Generate`: always a new path, never a replacement, in
any writable grant of the run. The bytes are staged as `.cowork-generate-<uuid>`, inspected into
the version store and renamed without replacement under the exclusive folder lock. A PDF split
publishes one new directory. The receipt carries `generated_from` (tool, operation, engine
version, input paths and SHA-256, receipt SHA). Interruption uses the same `pending_file_operation`
reconciliation as other operations; restore uses `operation_after`. `read_binary` gives the engine
bounded (32 MiB) binary copies of granted files. `retain_preview`/`read_preview` keep rendered PNG
previews (at most 2 MiB) content-addressed; only PNG blobs are readable through that path.
See `docs/DOCUMENT-TOOLS.md`.

## Qualification boundaries

Tests cover disposable files, exact grants, policy modes, Windows junctions, Unix symlinks and
FIFOs, OS locks, durable ownership, retained versions, and a real filesystem effect followed
by an intentionally failed database completion. The HTTP process test verifies restart,
reconciliation, independently hashed export, and absence of repeated writes. See VERIFY.md
for platforms actually executed.

Managed per-order workspaces and branching file snapshots use this same broker; see
[WORKSPACES.md](WORKSPACES.md). Environment versioning and an interactive terminal writer
remain unimplemented. Independent installations
with different databases/state directories do not coordinate ownership. OS locks are not a
universal exclusion mechanism for arbitrary external editors; expected-hash checks reduce
conflicts but do not provide an atomic compare-and-swap against uncooperative filesystem
writers. Shared writable host folders are not a qualified hostile multi-process sandbox.
Complete file support and executor isolation remain mandatory before M4 production acceptance.

Network filesystems, ReFS identity behavior and power-loss durability on Windows have not been
qualified. Process-kill recovery does not establish physical power-failure guarantees.

The implementation uses the capability interfaces of
[cap-std](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html),
[cap-fs-ext](https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/trait.MetadataExt.html), and
[Rust file locks](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock).

Directory creation, binary-aware moves, retained removal and exact operation reconciliation
are described in [FILE-OPERATIONS.md](FILE-OPERATIONS.md). They use the same broker and policy.
