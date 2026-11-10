# Native folder registration and revocation

The desktop can register an existing local folder without editing configuration or restarting
the daemon. This is a single-owner preview capability; it does not implement organization,
device or offline grants. The browser can select existing project grants for a run but cannot
register host paths.

## User flow

Open **Folder context** in an order. In the desktop, choose a folder using the operating system
picker. Access defaults to read-only; allow changes only when intended. A separate native
confirmation shows the project, resolved absolute path and requested rights. Canceling either
dialog changes nothing. Registration reads directory identity, not file contents.

The folder then becomes available in this project. Select it separately for the next run.
Registration does not add it to a pending run or change a previous run's frozen permissions.
Reads during a run may send content to that run's explicitly selected model.

The native manager lists active and revoked registrations, including their canonical paths.
Revocation requires an explicit confirmation. It does not delete files, previous results or
receipts. Removing access from the next-run selection alone does not revoke the project grant.
An old registration request remains revoked when retried; a fresh picker selection creates a
new grant rather than reviving the old one.

## Authority boundary

The renderer's `choose_native_folder` command accepts project, request ID, language and a write
flag. It accepts no host path. The native shell owns the picker, confirmation and privileged
request. IPC checks the application window and its origin. The ordinary browser receives
neither the privileged key nor canonical paths from the public folder endpoint.

`/api/v2/native/folders`, `/native/folder-requests` and `/native/folder-revocations` require both
the normal daemon bearer and a different `x-cowork-native` key. The latter is generated in
`COWORK_DATA_DIR/native.token` and stays in the native processes. Middleware rejects every
request carrying an Origin header, and unauthorized requests are rejected before body polling.
The native HTTP client uses a fixed loopback endpoint, disables proxies and redirects, and
never passes credentials through URL parameters or renderer responses.

These are boundaries against a renderer/browser client, not against the operating system
account that owns the data directory. Keep that directory private. This is not enterprise
authorization or an encrypted credential-store implementation.

## Durable registry and execution

The authoritative file is `file-broker/folder-registry.json`, format 1. It holds registrations,
physical identities, request hashes and revocation tombstones. A process-shared writer lock,
synced temporary file, atomic rename and verified readback govern updates. Linux additionally
syncs the directory. Readers reopen the registry for authorization; a stale process-local list
cannot authorize an action after another process confirms revocation.

The native shell pins the selected directory's physical identity before confirmation. The
daemon compares that identity while registering it and on subsequent access. Overlapping
active roots, duplicate physical identities and paths overlapping private daemon data are
rejected. A changed directory identity is a conflict, not implicit access to a replacement.
Invalid registry data fails closed.

Revocation takes the same physical-resource lock as file IO. If a native action or its receipt
is still in progress, revocation reports busy and requires an explicit retry. A successful
revocation therefore does not overtake an unconfirmed in-progress broker action. Every later
operation rechecks the grant after acquiring its IO lock. Existing approval decisions do not
override revocation. A run can still finish work in its own managed workspace.

An already journaled uncertain effect remains auditable after revocation. The original
run-scoped human reconciliation can inspect the expected path and compare its content to the
journal. It grants no new agent action, cannot repeat the write and does not resume the run.
Resume remains separate. Changed identities or unmatched contents keep the conflict blocked.

Migration 8 prevents older preview binaries from reopening a database that now relies on
durable native revocations. It is a schema compatibility guard; registrations themselves are
in the broker registry, not PostgreSQL/SQLite. This is not a V1 migration.

## Recovery and limits

Back up the registry together with the matching database, broker blobs and managed workspaces.
A database-only backup is insufficient. Do not delete or recreate the registry to fix an error:
that could discard revocations. Secret files require the same private handling as other
credentials. Complete coordinated backup/restore qualification remains outstanding.

Registration retries use the original request ID. After an uncertain transport result the
desktop looks up that ID before showing another picker. There is no automatic retry of the
mutation. Revocations are idempotent for the exact grant revision. Reopening the manager shows
the durable state even if the previous window closed before displaying its response.

The registry is bounded to 1 MiB, 1,024 registrations and 2,048 revocation records. Tombstones
are retained; compaction and orphaned temporary-file cleanup are not implemented. Configuration
via `folders.json` remains supported, and those grants can also be revoked. Removing legacy
configuration entirely may remove information needed to reconcile its historical effects;
prefer the native revocation command.

Windows qualification includes cross-process lock release, restart persistence, native HTTP
authority, exact identity checks, and real write/receipt failure followed by revocation and
reconciliation. The browser view and IPC client contracts are separately checked. Interactive
OS picker/confirmation acceptance is still open because native UI control is unavailable in
this session. The same core and real-process checks now pass in the isolated unprivileged
Linux test container; the current native Linux desktop/picker is still unqualified.
Power-loss, disk-full, network-filesystem and hostile concurrent filesystem-writer behavior
are not qualified. See [verification](VERIFY.md) for executed checks.
