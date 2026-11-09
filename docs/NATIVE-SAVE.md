# Native file delivery

The shared UI routes workspace downloads, historical files, text artifacts and order JSON
exports through one delivery helper. The web path hands a verified Blob to the browser and
reports only preparation. The desktop path invokes the Rust `save_file` command and reports
success only after native publication and readback. A canceled dialog is a separate result.

## Boundary and contract

- Rust owns `NativeSaveRequest`, `NativeSaveOutcome` and `UiLanguage`; generated validators
  cover the same IPC metadata as the API types. The payload is raw bytes with a bounded,
  percent-encoded metadata header, not a frontend-selected host path.
- Only the main local Tauri origin is accepted, plus the configured development origin in a
  debug build. The native picker supplies the destination. One picker/save may run at a time.
- No filesystem/dialog permission is added to the JavaScript capability. Rust uses the dialog
  plugin directly. There is no agent tool or HTTP endpoint for this ambient-path writer.
- Size and SHA-256 are checked before opening the picker and again in the broker. The 64 MiB
  limit applies to each delivery, including JSON exports. Full copies may exist in JavaScript,
  IPC and Rust memory; this is not a streaming transfer or an aggregate memory quota.
- The actual V2 data directory is protected. Device names, reserved broker names, traversal,
  hardlinks, symlinks, nonregular and readonly targets are rejected. Existing Windows EFS
  files are rejected rather than replaced by an unencrypted stage.

## Publication and failure behavior

The broker pins the canonical parent directory and records native identity, contents, size
and permissions of the selected existing destination. A unique `.cowork-save-*` file is
created in that directory. The replacement inherits the observed Windows DACL and protection
flag, or the Unix permission mode, before content is written. It is flushed and read back.

Immediately before publication, the directory identity, original target and stage must still
match. A new destination uses a kernel no-replace rename. Replacement of an existing file
uses the native dialog's overwrite decision and a same-directory rename after those checks.
The published identity, bytes, size and captured permissions are verified before returning
the receipt. Unix directory metadata is flushed as well.

On an ordinary unpublished failure, cleanup removes only the still-owned regular stage.
A foreign replacement at its name is preserved. A process killed before publication can
leave its stage in the selected directory; no automatic replay or orphan cleanup occurs.
Once publication may have happened, an unconfirmed result returns `save_uncertain` with
`retryable: false`. IPC disconnection or an invalid completion receipt is treated the same
way. There is no browser fallback and no automatic retry. The UI instructs the user to check
the selected destination first. Native destination paths do not enter API responses or logs.

## Qualification and limits

Windows tests exercise real binary/empty files, replacement and dropped preparation, protected
application data, readonly/hardlinked destinations, altered bytes/identity, corrupted stages,
foreign-stage retention and Windows protected DACL preservation. A child process is killed
both before publication and after confirmation; reopening finds the original or confirmed
contents respectively. Frontend tests cover raw IPC, hash checks, cancellation, conservative
errors, Unicode names and browser handoff. These are not simulated native-picker acceptance.

Interactive Windows/Linux picker, overwrite and cancellation acceptance remains pending.
Linux mode/symlink and child-process termination tests pass in the unprivileged Linux test container. Browser file
delivery also remains unqualified in the available embedded browser. Native build and desktop
process-lifetime results are listed separately in VERIFY.md.

This preview does not preserve every filesystem metadata class: owner/group changes, SACLs,
Unix extended ACLs, security labels, extended attributes and alternate streams are not
qualified. Replacement races with an uncooperative external editor are not a kernel
compare-and-swap guarantee. Power loss, network shares, ReFS, storage exhaustion and malicious
concurrent filesystem manipulation require further qualification. Use ordinary owner-controlled
local files for the current preview; these limits block general production acceptance.

## Required manual acceptance

Use disposable synthetic files outside the application's private directory. On both supported
desktop platforms, exercise current/historical binary download, artifact download and JSON
export; choose a new Unicode name and compare exact size/hash. Repeat with an existing target,
decline overwrite, cancel the dialog, then accept replacement and verify the old/new contents.
Check DE/EN labels, keyboard focus returning to the initiating control, double activation,
small windows and screen-reader announcements. Confirm that canceled saves create no file and
that uncertain outcomes do not trigger another save. Repeat with readonly/protected targets
and controlled disk-full conditions. Record platform, filesystem and build, rather than
inferring acceptance from unit tests or a successful build.

Dialog behavior is based on the [Tauri dialog Rust API](https://v2.tauri.app/plugin/dialog/)
and the [Windows file-dialog defaults](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/ne-shobjidl_core-_fileopendialogoptions).
The locked Windows backend retains the native overwrite prompt; the GTK backend enables
overwrite confirmation. Their interactive behavior still requires the acceptance above.
