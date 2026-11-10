# Windows headless isolation primitives

This is an experimental private executor building block, **not a qualified production
runner**. The daemon tool catalog, HTTP API and UI do not expose it. It does not activate
an environment or authorize a process-journal entry. Shell, browser, MCP, Office and desktop
automation remain unavailable through this path.

One reviewed exception is in product use: the document engine compiles LaTeX with Tectonic
through a **plain-AppContainer variant** of the capsule. It has its own boundary, limits
and checks, described in [Tectonic: reviewed plain-AppContainer launch](#tectonic-reviewed-plain-appcontainer-launch).
Nothing else may use that variant without its own review.

The reference host is Windows 11 Pro, build 10.0.26200, x86_64, with Rust 1.94.1.
No result here establishes Windows service/session-0, other Windows releases, ARM64,
Linux sandbox, enterprise or arbitrary third-party program compatibility.

## Boundary and launch

`cowork_executor::windows_isolation::Capsule` creates a fresh package identity, a private
desktop and three private areas: control files, an executable bundle and a writable work
directory. It uses a unique `LocalAI.Cowork.V2.<UUID>` AppContainer profile. Existing profiles
are never adopted. V1 accounts, profiles and ACLs are not reused; no firewall exception or
user-account change is required.

Only the capsule's newly created directories receive ACLs. The owner and SYSTEM retain
full access. The package SID gets read/execute on the bundle and modify access on the work
directory. Its root access permits traversal, not reading control-file contents. A different
capsule SID is not granted access. Approved input arrives as staged bytes rather than a
permission change on a host project folder. Program staging checks the supplied SHA-256,
allows a bounded `.exe` filename and uses create-new semantics. Launch rechecks the hash
and size while holding a no-follow file handle that denies concurrent writes and replacement.

The process starts suspended with all of these settings applied during creation:

- AppContainer package SID, **zero capability SIDs**, and mandatory
  `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` opt-out (LPAC).
- A named Job Object, associated atomically with `PROC_THREAD_ATTRIBUTE_JOB_LIST`;
  an existing job of the same name is rejected. Kill-on-close is mandatory, with no breakaway.
- Job-wide committed-memory limit, active-process limit and CPU hard-cap rate.
- All eight Job Object UI restriction flags.
- **Win32k system calls disabled** through the process-creation mitigation policy.
- The capsule's private desktop and exactly the three standard handles. Stdin is EOF.

Before resume, the host checks the exact AppContainer SID, AppContainer status, zero
capabilities, Low-or-lower integrity, membership in the intended job and active Win32k
disablement. It also performs a positive/negative access check with a duplicated token:
the package's explicit SID must grant read access, while `ALL APPLICATION PACKAGES` must
not. Any failed setup or check terminates the suspended process; no weaker launch is tried.

The environment contains only the Windows API-derived SystemRoot, this package's own
AppContainer storage as LOCALAPPDATA, and TEMP/TMP pointing to its work directory. It
contains no inherited PATH, user-profile path, provider credential or general parent
environment. The OS's per-package storage is additional temporary storage; it is not an
artifact source or a substitute for the work directory.

This is **not a hermetic filesystem**. Windows still supplies system resources accessible
to the restricted token. The host owner, kernel and broker are trusted. A hostile process
already running as the unrestricted host user is outside this boundary. Capabilities,
brokers, IPC, device access and supported executable dependencies need further review before
arbitrary programs can be admitted.

## Findings that changed the implementation

1. CreateProcess initially returned error 203 with the empty environment. The isolated
   launch now supplies explicit SystemRoot and per-package LOCALAPPDATA; parent environment
   inheritance was not enabled.
2. `GetTokenInformation(TokenIsLessPrivilegedAppContainer)` returned error 87 on the reference
   build. The code does not turn that failure into an affirmative LPAC result or use
   undocumented token attributes. It requires the documented creation opt-out and checks
   concrete positive/negative access behavior. A real file granted to ALL APPLICATION
   PACKAGES is also denied in the child test. This is a tested behavior on this host, not
   a claim of a universally supported token-class-46 detection contract.
3. A private desktop, even with Job UI restrictions, did **not** prevent OpenInputDesktop
   from returning a handle on this host. Therefore the final headless profile requires
   Win32k disablement. The host verifies the mitigation before resume. A separate synthetic
   child, without static user32/GDI imports, successfully runs ordinary headless work and
   observes user32 initialization failure (1114). A program with a static user32 dependency
   exited with DLL initialization failure (`0xC0000142`). There is no GUI-compatible fallback.
4. A direct child CreateProcess attempt returned error 5 even when the process-count limit
   allowed more than one process. Changing private-desktop permissions did not resolve it;
   those temporary diagnostic changes were removed. **Child-program compatibility remains
   unresolved.** Its failing capability probe is recorded separately and is not counted as
   proof of descendant containment or LPAC breakaway protection.
5. Winsock startup returned 10107 before a TCP connection could be attempted. The loopback
   listener is independently reachable from the host, and no connection arrives from the
   capsule. This proves the observed probe cannot connect; it does **not** qualify general
   WFP enforcement, network allowlists, UDP, remote destinations, DNS or direct device IO.

## Resource and termination semantics

`Limits.memory_bytes` bounds committed memory across the job, not RSS/working-set size.
The probe requests a 512-MiB allocation under a 256-MiB job and observes 1455. With a
one-process limit, creating a second process returns 1816. The latter is distinguished
from the separate child-compatibility failure above.

CPU millicores are converted to a Windows job rate using the active logical processor count,
rounded down and capped at 10000. Values below the platform's rate resolution are rejected.
The reference host has 22 logical processors. A short busy-loop probe under 100 millicores
measures CPU and wall time and checks observable throttling, allowing scheduler burst effects.
It does not establish strict instantaneous CPU usage, behavior after CPU hotplug, nested-job
accounting on other hosts or the enterprise mixed-load targets.

Each capsule may attempt one launch. Cancellation before start consumes that attempt without
creating a process. Nonzero exit, timeout, cancellation and IO uncertainty never rearm it.
There is no automatic retry, profile reuse, provider fallback or environment upgrade.

The Windows supervisor now confirms **zero active processes in the job**, in addition to
root exit, before returning successful termination acknowledgement. Failure to observe this
within the bound is uncertain, not a successful stop. Existing trusted-supervisor tests use
real descendants and cover cancellation, root-first exit, owner death and breakaway rejection.
The LPAC tests currently prove stop/recovery for an actually running **single process**;
the two evidence sets are not interchangeable.

## Cleanup and crash recovery

The host writes and flushes an intent journal before profile creation. After successfully
creating its new identity it writes and flushes a separate ownership receipt. Control files
are size-bounded, opened without following a final reparse point, and checked as ordinary
files. Directory/profile UUID binding and version are validated. A per-capsule file lock
prevents concurrent recovery. Recovery also checks that its named job has no active processes.

`close` reports OS cleanup failure. The OS profile and private desktop are closed; a flushed
closed marker records success. Tests verify removal of the newly created profile's storage.
The workspace and bundle remain available to the trusted host. There is no recursive
workspace deletion or automatic output import. Drop is best-effort only and must not be used
as a successful cleanup receipt.

Recovery is cleanup-only: it never returns a launch-capable capsule and never repeats a
command. A killed owner loses its final Job handle; the running probe stops, the lock is
released, and explicit recovery removes the owned profile. Corrupt, oversized, mismatched
or missing ownership/control data blocks cleanup.

**Crash window:** if the host dies between profile creation and the ownership receipt,
ownership is unconfirmed. Automatic recovery deliberately refuses to delete that profile.
The source now supplies the local signed operator reconciliation described in
[PROCESS-RUNNER-V2](PROCESS-RUNNER-V2.md#windows-administrative-orphan-klärung).
It binds the original pre-ownership journal, exact native file/SID identities,
original confirmed VHD and separate control-key approval. An orphan never gains
an `owned` marker or a new execution permit. A completed signed operator receipt
is required before receiver disk detach; a lost reply reconciles positive native
absence and never deletes a reappeared profile. A single real Windows profile
fixture covers cleanup, original receipt replay and same-name replacement refusal.
The whole VHD/service/operator exchange remains unqualified. File flushing here is not
a completed power-loss, disk-full, filesystem-corruption or metadata-durability qualification.
The receipt is trusted local provenance, not remote authentication or a cryptographic runner
attestation.

## Tectonic: reviewed plain-AppContainer launch

`cowork_executor::supervise_confined` runs one program inside a fresh capsule through
`Capsule::run_granted`. Its only caller is the document engine's LaTeX compile
(`crates/toolbox/src/document_engine.rs`, `compile_tex`). Tectonic's own `--untrusted`
mode does not confine TeX: `\input`, `\openin`, images and `.bib` files read any user file,
and `\openout` writes or overwrites any user file before any post-run check could object.
The OS boundary below is the only thing that limits this. The evidence is in
`.local/document-tools-20261018/TECTONIC-SPIKE.md` and `TEX-ISOLATION-NOTES.md`.

### Why the headless capsule cannot host Tectonic

- Under **LPAC**, Tectonic exits 1 with `Unable to find standard directories for platform`.
  It resolves Windows known folders even when `TECTONIC_CACHE_DIR` is set.
- With **Win32k disabled**, it fails with `0xC0000142` (DLL initialization). `tectonic.exe`
  statically imports `shell32`, `oleaut32`, `combase`, `userenv` and `ws2_32`, and these
  pull in `user32`.
- With the capsule's **owner-only lockdown of the package's own AppContainer folder**, it
  fails with the same known-folder error (observed while building this variant).

### What is enforced

- **Fresh identity per compile.** `Capsule::create` makes a new `LocalAI.Cowork.V2.<UUID>`
  profile with the same journal, ownership receipt, lock, private desktop and named Job
  Object as every capsule. Existing profiles are never adopted. Each capsule allows one
  launch.
- **Process creation (`Profile::AppContainer`).**
  - Package SID with **zero capability SIDs**, so no internet, private-network,
    enterprise-auth or device capability.
  - **No** LPAC opt-out attribute, and **no** Win32k mitigation.
  - Atomic job association with kill-on-close and no breakaway.
  - Job limits: 2 GiB committed memory, **one active process** (Tectonic starts no
    children), CPU hard cap of 2000 millicores, and all eight UI restriction flags.
  - Private desktop. Stdin is EOF.
- **Pre-resume verification.**
  - The token is an AppContainer token with the exact package SID, zero capabilities and
    Low or lower integrity, and the process is in the intended job.
  - A duplicated-token access check must show that the package SID **and** ALL
    APPLICATION PACKAGES both grant access. This positively confirms a plain AppContainer,
    the inverse of the capsule's LPAC check.
  - Any failure terminates the suspended process. There is no weaker retry.
- **Environment.** Only `SystemRoot`, `LOCALAPPDATA` (this package's own AppContainer
  folder; without it CreateProcess fails with 203), `TECTONIC_CACHE_DIR`, and `TEMP`/`TMP`
  set to `<stage>/work/latex-out/tmp`. There is no `PATH`, `HOME`, `USERPROFILE`, proxy or
  credential variable. A stage `USERPROFILE` would break Tectonic (spike finding 4).
- **Per-run grants to this package SID only.**
  - Read/execute on the LaTeX source folder: `<stage>/in/tex` for `compile_latex`, or the
    engine-written `<stage>/work/latex` for `create_pdf`.
  - Modify on `<stage>/work/latex-out`.
  - Nothing else in the stage. `request.json`, `out/`, other inputs and the stage root
    stay closed. The spike showed that Modify on the whole stage would let `\openout`
    tamper with `request.json` or `out/` before `latex_finalize`.
  - Grants are merged into the existing DACL as explicit inheritable ACEs. NULL DACLs and
    reparse-point targets are refused.
- **Shared read-only trees.** These carry a kept read/execute ACE for **ALL APPLICATION
  PACKAGES** (S-1-15-2-1), like Program Files:
  - Tectonic's private directory `<data>/tool-runtime/tectonic-*`. The toolbox accepts
    only setup's private copy, a `tectonic-*` directory directly inside the runtime
    directory, so a foreign folder's ACL is never changed. Any other location keeps LaTeX
    unavailable on Windows.
  - The warm cache `<data>/tool-runtime/tectonic-cache`.

  Both hold only public data: the pinned binary, TeX Live files, and a format built from
  them. Granting the fresh package SID instead would rewrite every cache file's DACL twice
  per compile. On this host that measured about 1.5 s to grant plus 1.3 s to revoke for
  3000 files, with concurrent-edit hazards between parallel compiles. The full grant runs
  once per process, which also repairs a propagation a crash interrupted. Later runs only
  confirm the root ACE, and re-grant a tree that was recreated. A warm `--only-cached` run
  modifies nothing in the cache, and the boundary test compares the cache listing.
- **Cleanup.** `supervise_with` returns only after the job reports zero processes, so the
  trees are static when cleanup starts.
  - The read grant is revoked **with** propagation. Its tree is host-written, and the
    container had no write access to it.
  - The write grant is removed from the `latex-out` root **without** propagation
    (`SetKernelObjectSecurity`). A host-side ACL walk must never follow content the
    container wrote, such as junctions. Inherited copies inside name the deleted package
    SID, and they disappear with the stage.
  - `close` then deletes the profile, closes desktop and job, and writes the closed
    marker. The capsule journal directory under `<toolbox>/tex-confinement` is removed.
  - Before the unconfined `latex_finalize` phase reads `latex-out`, the toolbox deletes
    every symlink, reparse point, hard-linked file and special file in that tree.
- **Crash recovery.**
  - If the daemon dies, kill-on-close ends Tectonic with it.
  - The first confined run of the next process scans `<toolbox>/tex-confinement`. It
    recovers each `capsule-*` journal with the existing `recover`: the job must be empty,
    then the profile is deleted and the closed marker written. The journal directory is
    then removed.
  - Live runs hold their capsule lock and are skipped.
  - The pre-ownership crash window is the same as for capsules. No ACL is granted before
    ownership is recorded, so only an unconfirmed profile registration can remain.
  - Stage directories with stale ACEs for the deleted SID are wiped when the toolbox
    starts.

### What is not enforced (review limits)

- **Reads beyond the stage.** The process reads whatever ALL APPLICATION PACKAGES can read,
  and that content can appear in the PDF, the log and the model-facing diagnostics:
  - the Windows directory, Program Files and system fonts (`\setmainfont{Arial}` reads
    `C:\Windows\Fonts`);
  - the parts of HKLM and HKCU that AppContainers can read;
  - any folder an installer or the user opened to ALL APPLICATION PACKAGES;
  - the shared Tectonic trees;
  - volumes without ACLs, such as FAT/exFAT drives.

  User profile data is **not** readable: Documents, Desktop, AppData and the user's temp
  folder. The tests prove this with canaries in the user's temp folder.
- **Win32k is allowed.** The process loads `user32` on a private desktop with all Job UI
  restrictions. Finding 3 above showed that a private desktop is not by itself an
  interactive-desktop boundary. TeX primitives cannot reach the desktop. A
  memory-corruption exploit inside Tectonic could use whatever a Low-integrity
  AppContainer process may do on the desktop (UIPI still blocks higher-integrity windows).
  This is not qualified.
- **System services.** RPC, COM, named-pipe and broker surfaces that admit AppContainers
  stay reachable. They are not qualified.
- **Network.** There are no capabilities, and a TCP connect from the same launch to a live
  host loopback listener did not arrive. This is not a WFP qualification: UDP, DNS and
  remote destinations were not tested separately.
- **Disk.** Writes to `latex-out` and the package's AppContainer folder are bounded only
  by the job and toolbox time limits, not by a quota. Both are deleted after the run.
- **Tectonic's own correctness.** `--untrusted` still disables shell escape. The boundary
  above is what limits file access, not Tectonic.

### How to verify

Use only a scratch Tectonic runtime, never the real data directory: Windows keeps the
ALL APPLICATION PACKAGES grant on these trees. Place the binary as
`<scratch>/tectonic-<x>/tectonic.exe`, with the cache as `<scratch>/tectonic-cache` warmed
for the test corpus. Then run:

```
set COWORK_REQUIRE_TEX_TESTS=1
set COWORK_TEST_TECTONIC=<scratch>\tectonic-<x>\tectonic.exe
set COWORK_TEST_TECTONIC_CACHE=<scratch>\tectonic-cache
cargo test -p cowork-executor --test tex_confinement -- --nocapture --test-threads=1
cargo test -p cowork-toolbox --lib document_engine
```

`tex_confinement` runs two tests:

- **Tectonic in the toolbox stage layout.** A legitimate multi-file document with two
  images and a bibliography must compile. Two controls must trigger: a staged canary is
  detected, and the same attacks leak and overwrite when run **unconfined**. Then all of
  these must fail: `\input{C:/abs}`, `\input{../../x}`, `\openin`, a `^^3a`/`^^2f` path,
  `\bibliography{C:/abs}`, `\includegraphics{C:/abs.png}`, and `\openout` to an absolute
  path, to parent paths (`request.json`, `out/`, `in/tex`), and into the cache or the
  program directory. Every target must stay unchanged and the cache listing identical.
- **A fixture under the same launch.** Outside reads and writes, the TCP connect, process
  creation and opening the host process must all fail. Writing its own tree must succeed.

It also prints one observation that is not a boundary: `win.ini` in the Windows directory
is readable under confinement. That is the first limit listed above.

On 2026-10-19 both passed on the reference host, together with the unchanged capsule
suite (`windows_isolation`, 15 cases). No AppContainer profile, folder or mapping was
left behind.

## Reproducible checks

Run `python tools/qualification/windows_isolation.py` from the repository. It uses a separate
build directory, runs workspace formatting/tests/Clippy and captures the isolated-process
observations. It records source hashes, OS/toolchain, fixture hashes and individual logs under
`.local/windows-isolation-<id>`. It neither restarts nor upgrades the existing preview.

The 15 Windows integration cases plus one profile-cleanup unit test cover actual headless
execution, own input/output, protected control data, a separately activated capsule's files,
an ALL APPLICATION PACKAGES canary, executable integrity, host-process injection/handle
access denial, Win32k restrictions, resource behavior, timeout/cancellation, nonzero effects,
owner death, lock protection, recovery without replay and malformed recovery state. Two
observations explicitly record unqualified network/subprocess capabilities.

The headless fixture is separate from the trusted host fixture. Neither is staged into a
desktop/runtime release. On Linux the headless fixture has only its cross-platform
`confined-probe` mode, used by `tex_confinement` against the Landlock/seccomp launch, and
executes no Windows isolation tests. Its full workspace regression is a compatibility check only.

## Required before product activation

- Qualify actual tool bundles and their DLLs, processes, named pipes, COM/RPC/device surfaces
  and any explicitly approved capability SIDs. Do not relax Win32k for Office/GUI work;
  build and qualify the separate intended execution boundary.
- Resolve and test subprocess compatibility, inherited token restrictions and real descendant
  termination inside that qualified boundary. No claim of this acceptance is made here.
- Enforce disk/scratch and total-output quotas, including per-package storage; validate
  junction/symlink/ADS/device cases and safe output promotion. Current per-file staging and
  capture limits do not enforce these aggregate quotas.
- Connect durable process admission to an authenticated runner, durable executor identity,
  last-moment policy/environment verification and stale-runner fencing. Capsule control files
  do not replace the process journal or its authority.
- Add restart/power-loss/cleanup-failure reconciliation, verified retained-data cleanup and
  registry/profile orphan management. Never delete unconfirmed profiles by prefix alone.
- Qualify supported Windows builds and service execution; produce a trusted
  EnvironmentQualification only when every required boundary has actual passing evidence.

## API references

The launch design follows Microsoft's [AppContainer implementation](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
and [process attribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
contracts. The additional GUI boundary uses the documented
[system-call mitigation](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_system_call_disable_policy);
[Job UI restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_ui_restrictions)
alone were insufficient for the observed OpenInputDesktop check. Resource semantics follow
[job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information)
and [CPU rate control](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information).
Profile lifecycle uses [CreateAppContainerProfile](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-createappcontainerprofile)
and [DeleteAppContainerProfile](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-deleteappcontainerprofile).
