# Native Windows sandbox verification — 2026-09-24–25

This records a local development verification of version 0.3.12. The checkout already contained other application changes. No commit, push, public release or upgrade of the daily installation was performed.

## Findings and corrections

- The ignored Rust acceptance tests launched the libtest executable as the sandbox runner. They now use a separately built application, or an explicitly selected packaged executable. The executable override exists only in test builds.
- Verbatim Windows paths could make `cmd` fail and PowerShell select an unrelated working directory. Paths are normalized after canonicalization. PowerShell mounts the authorized directory as a private `LACowork:` drive and sets the native working directory; failure stops execution. This also supports directories beneath inaccessible parent folders without granting access to those parents.
- The common sandbox group permitted reading another run's workspace. Restricted child tokens now disable that group's allow entries. Temporary access is granted to the active logon and revoked after the complete process tree has exited.
- Windows reused the same logon SID for concurrent `CreateProcessWithLogonW` calls on this machine. Local native shell requests therefore use an exclusive file lock covering access grants, execution and revocation. Waiting requests can be cancelled without starting a shell. A protected recovery record allows abandoned grants to be revoked before the next execution; invalid recovery data stops execution.
- Default security descriptors for new process and IPC objects are scoped to the active logon. The shared account is not a write-restricting SID. A separate desktop ACL supports shell startup without restoring that broad permission.
- Writable roots are canonicalized and checked before any permissions change, rejecting parent traversal and junctions resolving outside the workspace. Failure to create or attach a job terminates the suspended helper.

Local shell commands are now serialized. This is a deliberate isolation constraint; model requests and unrelated application operations are not serialized by this lock.

## Reproduce the checks

On Windows, first complete **Settings → AI Sandbox → Set up sandbox**. Account creation requires the normal Windows administrator/UAC approval. The native sandbox does not require Docker, WSL or the Windows Sandbox optional feature.

From `app/`:

```powershell
npm run test:sandbox:native
```

The script builds a matching helper, runs real native acceptance tests and checks workspace snapshot/diff behavior. It uses the existing protected sandbox setup, temporary workspaces and the bundled Python archive. It does not rotate the account password, run an installer or modify the application database. Preparing the derived sandbox Python runtime is permitted if it is missing.

To verify an exact packaged executable:

```powershell
./scripts/test-native-sandbox.ps1 -SandboxExecutable 'C:\path to extracted package\app.exe'
```

The release installer smoke also calls this test script after verifying initial and repeated UAC setup. Its existing disposable-profile/CI guard remains in place.

## Verification evidence

| Check | Result |
| --- | --- |
| Frontend unit tests | 597 passed, 117 files |
| Installer/resource script tests | 31 passed |
| Browser UI regression checks | 12 passed; browser fixtures, not a fresh installed GUI |
| Rust library suite | 247 passed, 8 explicitly ignored |
| Native execution acceptance | Six tests passed separately against the packaged release binary, using real Windows processes |
| Workspace copies and diffs | Eight tests passed, included in the Rust suite |
| TypeScript, production frontend and style contract | Passed; 320 source files in the style contract |
| Dependency policy | Passed: 347 npm and 497 Cargo package records |
| Rust Clippy | Passed with warnings denied |
| Production runtime smoke | 24 artifacts validated; foreign working directory, offline MCP handshake, ignored debug override, missing-payload rejection and no pre-Tauri app-data initialization passed |
| Final NSIS package | Build, archive integrity, extraction, all 24 artifact hashes and extracted production runtime smoke passed |

Native acceptance covers PowerShell and `cmd`, relative file writes, Unicode and separate output streams, bounded capture, cancellation, timeout with a child proven to have started, denied access to originals and other workspaces, read-only roots, queued cancellation, and repeated execution of bundled Python. Ordinary unit tests also cover recovery records and traversal rejection. The six environment-dependent native cases are run explicitly; the remaining two ignored Rust cases concern separate audit scenarios.

## Installer and remaining platform boundary

Build with `npm run installer`. The local NSIS artifact is `dist-installers/LocalAI-Cowork-Setup.exe`. It is an unsigned development build without updater artifacts, retaining the existing 0.3.12 metadata.

Final package: **375,739,695 bytes** (approximately 358.3 MiB). Authenticode status: `NotSigned`.

```text
Installer SHA-256
f57b69153d78dc8179709d46ca4ef2b614ecb879a2054decdeaccb302ee866bf

Packaged app.exe SHA-256
c5f7f9a717e6be4f9232404ca2fc7efaa0e12ccaee5dc9e190551195000cfb42
```

The packaged executable matches the hash recorded during the successful release acceptance and staged runtime checks. After packaging, the build-directory executable differs only in three bundle-marker bytes (`NSS` versus `UNK`); use the packaged hash above when identifying this tested installer.

`7z t` passed for the final NSIS archive. Its extracted executable also passed `--lacowork-runtime-smoke --json` from an unrelated working directory, with all 24 payload hashes validated and without initializing application data. These checks did not run the normal installer or its registry/shortcut hooks.

The daily installed executable was not replaced. Building, checking the NSIS payload and running its executable do not establish a clean-profile installation or upgrade result. The full installer/UAC/first-launch test remains an isolated Windows-profile or CI check. Linux and remote Docker sandbox behavior are outside this native-Windows verification.

The access-control implementation uses Windows restricted-token semantics described in [Microsoft's restricted-token documentation](https://learn.microsoft.com/en-us/windows/win32/secauthz/restricted-tokens). The operating-system behavior reported above was reproduced locally; it is not a claim of a formal sandbox security certification.
