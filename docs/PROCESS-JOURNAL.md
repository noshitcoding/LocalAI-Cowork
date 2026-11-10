# Durable process admission and reconciliation

The domain/storage layers implement the durable side of process execution. They use the
existing run, policy, approval, lease, event and managed-workspace contracts. This is private
application infrastructure: the model catalog, daemon command API and terminal UI do not yet
offer `execute_program`. No untrusted program or production environment is enabled here.

## Admission

`RunSpec.environment` freezes environment/version IDs and definition/qualification hashes.
Old configurations omit this optional field. Their serialized identity and approval hash
remain unchanged. A configured reference becomes part of the exact approval fingerprint;
the storage mutation boundary rejects changes to the entire frozen run configuration.

An invocation contains only a registered program name, argument vector and timeout. It
cannot choose a host path, working directory, shell, environment variables or credentials.
`execute_program` is classified as mutating; all four existing permission modes apply.
Read-only denies it, the default mode requires an exact approval, and autonomous mode still
requires the allowed tool and workspace/environment boundaries. Live organization/device
policy layers remain part of the future enterprise integration.

The first admission checks the current worker lease, the exact pending tool, remaining
step/time budget, selected executor, current environment qualification and own writable
managed workspace. A fresh trusted runner observation is required. Environment changes
are serialized with admission by locking the registry row before it is read.

One transaction then commits all of:

- the original worker fence as a hash, never the raw token;
- the unique `(run_id, tool_call_id)` process attempt and request hash;
- the in-flight checkpoint and durable workspace writer ownership;
- the run revision and sequenced `process_launch_committed` event.

Only a confirmed first admission returns an in-memory, nonserializable launch permit.
An exact duplicate returns the existing record, including after a lost acknowledgement.
It never creates a replacement permit. Reusing the call ID with different arguments is a
conflict. SQL retry closures perform no OS launch or other external side effect.

`launch_committed` means the permission was committed; it does not assert that an OS
process actually started. A failure between commit and launch is deliberately uncertain.
The actual runner still needs durable execution identity, fencing and an authorization
recheck at its spawn boundary. This storage transaction alone cannot close that boundary.

## Completion, cancellation and failure

Completion verifies the original worker fence and current lease. The process result,
checkpoint tool result, consumed step, run revision and event commit together. Repeating
the same completion returns its first confirmed record; a different result conflicts.
Revoking an environment blocks new admissions but does not prevent recording an already
observed result.

A cancellation request retains the workspace owner while the process is in flight.
`canceled` becomes confirmed only when the trusted executor supplies its termination
result. The checkpoint and writer ownership are released in that same completion
transaction. Generic checkpoint mutation cannot erase or skip an admitted process, and
no terminal run state is accepted while an uncertain external action remains in flight.

After lease loss or failed completion, the run becomes `interrupted`. Its process record,
in-flight marker and writer ownership survive. Neither resume nor cancel can erase the
uncertainty. Reconciliation must match the original request and record either reviewed
completion evidence or reviewed evidence that launch never happened. It consumes the old
call without relaunching it and leaves the run interrupted until an explicit resume.

The reconciliation entry point is private to the trusted application layer. A PID or a
SHA-256 string is not proof of runner identity, process termination or absence of effects.
The future runner/review integration must retain and authenticate that evidence, fence
old workers and verify process termination before settling. No public endpoint accepts
a user-supplied proof hash or a model's assertion that a process did not start.

## Results, persistence and compatibility

Results retain original stdout/stderr bytes, each bounded to 64 KiB, with SHA-256,
observed byte totals and truncation flags. They include termination reason, diagnostic
PID, exit code/signal, duration and whether capture finished. The checkpoint receives
a lossy UTF-8 summary for model context; the original bytes remain in the attempt.
Events contain identities, hashes and termination metadata, not arguments or output.

Migration 11 creates `process_attempts` in SQLite and PostgreSQL. Runtime event schema 6
adds the process vocabulary. Order-export format 8 includes the attempts in the same
consistent snapshot and integrity hash as the run. Empty attempt arrays and absent
environment references are omitted, so old stored records remain readable without
rewriting historical events. The raw worker fence is never exported.

Reads verify record IDs, call IDs and revisions against their database columns and check
the project, workspace, request hash and environment against the frozen run. Detail reads,
exports and repeated admission reject inconsistent records without rewriting them. This
detects corruption; it is not authentication against an administrator rewriting the database.

The generated TypeScript/runtime validators come from the Rust contracts. Structural
validation rejects extra invocation/environment fields and invalid binary byte values;
the application domain additionally verifies hashes, budgets and cross-field invariants.

## Verification

Run `cargo test --locked -p cowork-storage --test processes`. The common policy,
concurrent-admission, completion and export contract also runs against a fresh dedicated
PostgreSQL database using:

```text
cargo test --locked -p cowork-storage --test processes postgres_process_contract -- --ignored
```

The suite covers all permission modes, changed requests, revoked qualifications, missing
programs/capabilities, stale leases, failed admission/completion transactions, concurrent
completion, retained writer ownership, exact reconciliation, explicit resume and reopen.
A corruption test changes each indexed identity or frozen binding independently and checks
that detail, export and repeated admission fail while the original run and corrupt evidence
remain untouched.
A real worker starts a synthetic child through `cowork-executor`; that child writes and
syncs an effect. The test kills the worker before database completion, independently
reopens the database and verifies that recovery does not repeat the effect. Original
binary results and independently calculated export hashes are compared.

This verifies durable execution semantics for trusted fixture programs. It does not
qualify a Windows restricted identity, Linux/gVisor sandbox, authenticated remote runner,
interactive terminal, resource enforcement, stream pagination or aggregate output quotas.
Those remain required before the product exposes arbitrary program execution. Platform
results and exact evidence locations are recorded in [VERIFY.md](VERIFY.md).
