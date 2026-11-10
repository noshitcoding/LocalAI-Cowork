# Process lifecycle supervision

`cowork-executor` supplies the lifecycle layer for the future native, container and remote
executor adapters. It currently executes **trusted adapter programs only**. It is not exposed
through the daemon API, renderer, model tool catalog or MCP. Terminal execution is not yet
enabled in the product. Neither process groups nor Job Objects provide the planned filesystem,
credential, network and tenant isolation by themselves.

## Implemented behavior

A private `TrustedLaunch` fixes an absolute existing executable and directory, an argument
vector, the complete child environment, a monotonic deadline and bounded byte captures.
The supervisor does not select a shell, expand arguments or inherit parent environment
variables. Batch files are rejected; a shell requires a future separately authorized adapter.
Standard input is EOF. Arguments, output and environment are not logged by the library.

The child starts once. The outcome records process ID, termination reason, exit code or
signal, elapsed time, separate original stdout/stderr bytes, observed byte totals, truncation
and UTF-8 validity of the retained bytes. Each stream retains at most the configured bound
(1 byte through 4 MiB) while continuing to drain excess output. Both streams use bounded
nonblocking polling so an output flood cannot starve cancellation or the deadline.

Cancellation requested before admission creates no process. Cancellation and timeout after
admission request termination and wait for a bounded acknowledgement. A normal root exit
also terminates remaining managed children. Final pipe draining has a deadline; a retained
foreign pipe is represented by `output_complete: false`, not an indefinitely blocked reader.
An IO failure is returned as an uncertain execution error. Nothing in this layer retries it.

The application storage layer now provides a [durable process journal](PROCESS-JOURNAL.md)
before launch and after completion; production runner integration is still required.
This supervisor library itself has no durable journal. A canceled, timed-out, failed
or unobserved execution can already have changed files or other resources; its outcome never
proves that no effect occurred. Resource ownership and uncertain-effect reconciliation remain
required at the application boundary.

## Windows

The supervisor creates a private unnamed Job Object with kill-on-close and no breakaway
permission. `PROC_THREAD_ATTRIBUTE_JOB_LIST` associates the new process atomically during
creation. The process is initially suspended and resumes only after its process handles are
owned. This avoids a suspended orphan between process creation and later job assignment.
Any startup failure closes the job and kills its admitted processes.

`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` allows only the child's three standard handles. The job,
parent read pipes and unrelated inheritable handles do not cross that list. The supplied
Unicode environment is sorted case-insensitively; argument quoting is verified with actual
child argv, including empty strings, quotes, trailing backslashes and shell metacharacters.

Tests kill the supervisor process, request cancellation, let a root exit before its child,
attempt ordinary `CREATE_BREAKAWAY_FROM_JOB`, and attempt to use an unrelated inheritable
event handle. The children are synthetic fixture programs, not unrestricted agent commands.

The separate experimental [Windows headless isolation entry point](WINDOWS-ISOLATION.md)
now adds a fresh LPAC identity, private staged directories, Win32k disablement, a private
desktop, resource limits and cleanup recovery. It is not a qualified production sandbox.
It changes only capsule-owned ACLs, so there is no shared V1 account or host-project ACL
grant/revocation to serialize. Each capsule has its own cross-process recovery lock.
Native network enforcement, subprocess/tool compatibility, disk quotas and production
runner integration remain open. Existing V1 accounts and ACLs were not changed or reused.
Windows termination acknowledgement now checks that the job has zero active processes,
in addition to waiting for the root process.
See Microsoft's [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
and [redirected process IO](https://learn.microsoft.com/en-us/windows/win32/procthread/creating-a-child-process-with-redirected-input-and-output).

## Linux

The adapter child gets its own process group, `no_new_privs`, an explicit environment and a
parent-death signal before exec. Only async-signal-safe system calls execute in `pre_exec`.
The parent is checked after setting the death signal to close the parent-exit race.

Exit observation uses `waitid(WNOWAIT)`: the root remains waitable until group termination,
so its PID cannot be recycled into an unrelated process group before the kill. Reaping occurs
after termination. Tests cover direct-child parent death, ordinary child-tree cancellation,
background children after root exit and closed output pipes.

A program can leave a process group, and parent-death signals are not a descendant-wide
container boundary. Linux still requires a qualified cgroup/container adapter, persistent
execution identity, resource limits and recovery after runner loss before untrusted programs
can run. This limitation is not relaxed by passing the lifecycle tests. On shared nodes the
approved design additionally requires gVisor; the inspected local Docker configuration currently
offers `runc`, `io.containerd.runc.v2` and `nvidia`, with no `runsc` runtime. No fallback to an
unqualified runtime or host shell was added. See the Linux
[parent-death signal specification](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
and [gVisor Docker setup](https://gvisor.dev/docs/user_guide/quick_start/docker/).

## Verification

Run `cargo test --locked -p cowork-executor` and
`cargo clippy --locked -p cowork-executor --all-targets -- -D warnings`.
The cross-platform synthetic executable lives under `crates/executor/tests/fixture` and
is never staged in a desktop or daemon release. Its deliberate root-before-child exit is
the narrowly documented exception to Clippy's zombie-process lint in test code; production
supervisor cleanup waits and reaps.

`python tools/qualification/linux_container.py --suite executor` checks this crate on an
unprivileged Linux filesystem with no external network or host mounts. It records a source
manifest, image ID and individual logs. This test container is a test harness, not the planned
gVisor executor. Platform results are recorded in [VERIFY.md](VERIFY.md).

## Required next integration

1. Versioned environment/adapter registration with verified program/image identity and actual
   isolation capabilities. The private [registry](ENVIRONMENTS.md) now stores immutable
   versions and validates trusted report bindings; actual adapter probes and product
   integration remain open. Missing required capabilities must reject preflight and launch.
2. Complete qualification of the experimental Windows restrictions and Linux gVisor/container resource boundaries, including
   negative filesystem, credentials, network, sibling-job and process-tree tests.
3. Connect the implemented durable process admission/completion and restart reconciliation to
   authenticated runner control, persistent execution identity and final launch fencing.
4. Complete the runner integration with the journal's shared policy, exact approvals and
   managed-workspace ownership, then retain file changes and the required audit evidence.
5. Stream persistence and terminal UI distinguishing human sessions from agent runs, followed
   by model, workflow and scheduled execution through that same path.
