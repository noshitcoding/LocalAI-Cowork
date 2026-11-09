# Versioned execution environment registry

Current integration (2026-10-17): the owner API and shared environment picker
stage, physically qualify and select these records. `linux_image` retains its
existing gVisor and hard-quota contract. `linux_kata_image` requires Kata;
`local_linux_image` explicitly selects a single-owner Docker Engine with hard
CPU/memory/PID bounds and a monitored storage budget. Historical format-1 JSON
omits `network`, preserving its existing hashes. Format 2 requires explicit
denied networking or, for Kata only, an operator-defined web-proxy policy ID.
No runtime fallback changes an existing pin. See
[current architecture](AGENT-SANDBOX-ARCHITECTURE.md) and
[operations](AGENT-SANDBOX-OPERATIONS.md). The foundation-stage description below
records the original registry scope; its former missing HTTP/UI integration is
now supplied by the implementation above.

The domain and storage layers now implement a private environment registry. This is a
control-plane foundation for M4/M5; no environment-management HTTP route, package installer,
terminal command or untrusted executor is enabled by it. RunSpec can now contain a frozen
binding, and private process admission validates it in the run transaction. Current product
start commands do not yet select these pins. The runner/runtime integration remains required.

## Immutable definitions and activation

An environment belongs to one project. Its versions contain a format version, an exact
Linux image digest or Windows bundle SHA-256, package versions and source hashes, relative
program paths, and CPU, memory, process and scratch-space limits. A canonical hash binds
the entire definition. Format 1 permits only denied networking. It has no secret values,
host paths, arbitrary environment variables, shell command strings or mutable image tags
without a digest. Program paths use a deliberately limited portable alphabet.

Creating a candidate does not alter the active version. A candidate is initially `staged`,
then becomes either `qualified` or `rejected`. Qualification and rejection cannot be undone
by changing the same candidate. A new installation or a new runner generation needs a new
candidate. Activation is a separate, revision-checked operation with a fresh observation.
Old definitions remain available; activation never rewrites a previously issued pin.

Every transition has an idempotency key, request hash, revision, version reference and time.
State and receipt are committed together. Repeating a confirmed activation returns its
original receipt, even if another version is active now; it cannot reactivate the old version.
Reusing a key with different arguments conflicts. Competing changes from the same revision
produce one winner and an explicit conflict. A failed write confirms neither state nor receipt.

SQL migration 10 adds the project-scoped registry to SQLite and PostgreSQL. It does not update
existing run configurations, approval fingerprints, messages, events or artifacts. A record is
locked before reading during mutations; SQLite independent connections and PostgreSQL row
locking use the same domain transitions. Manifest hashes, row ownership and revision are
validated on reads. The lifecycle is reconstructed from the ordered receipts and compared
with the stored state. Corrupt or inconsistent state is rejected without rewriting history.

## Qualification is a trusted adapter boundary

Qualification records bind the exact definition and artifact, runner ID/generation, adapter
build hash, report hash and verified isolation capabilities. The required capabilities are:

| Common | Windows bundle | Linux image |
| --- | --- | --- |
| Filesystem boundary, denied network, isolated credentials, process-tree containment, enforced resource limits | Restricted identity, private desktop, ACL recovery and ACL serialization | gVisor |

The registry checks completeness and exact binding of that record. It does **not** perform
the platform probes and cannot authenticate a report merely because it has a hash. Only the
future authenticated, qualified runner/control plane may submit qualification. Renderer,
model, MCP metadata and user-supplied capability booleans are not trusted reports. No current
product entry point accepts one, and no production environment has been qualified here.

Activation and pin validation require a fresh trusted observation no older than 30 seconds.
Future observations, observations preceding qualification, changed runner generation/build,
changed artifacts or definitions, and missing required isolation all deny admission. Recorded
transition time moving backwards also prevents a new transition, except terminal revocation.
The actual observed time of that revocation remains recorded. These checks do not grant
filesystem, network, organization, credential or tool rights.

A pin includes the environment/project/version IDs, immutable definition and qualification
hash. It can still resolve after a different version becomes active. Revoking its version or
the entire environment blocks it. A changed runner requires a newly qualified candidate and
explicit reassignment; there is no fallback to another executor or a less isolated runtime.

## Verification and bounds

`cargo test --locked -p cowork-storage --test environments` exercises the lifecycle,
failed installation, stable historical pins, exact replay, competing registrations/changes,
revocation, stale/future observations, runner changes, each missing capability, wrong-project
IDs, definition tampering and failed database writes. A real child process acknowledges an
activation, is forcibly terminated, and the parent independently reopens and compares the
complete persisted record and receipts.

Run the same PostgreSQL contract with a dedicated `COWORK_TEST_POSTGRES` database:
`cargo test --locked -p cowork-storage --test environments postgres_environment_contract -- --ignored`.
`python tools/qualification/linux_container.py --suite environments` runs the environment and
migration tests plus domain/storage Clippy in the isolated Linux test harness.

Tests use synthetic qualification reports. They prove the registry and admission rules;
they are not tests of actual gVisor, Windows token/ACL/network isolation or signed evidence.
Current platform outcomes are listed in [VERIFY.md](VERIFY.md).

One record currently holds at most 256 versions and 2,048 ordinary transition receipts.
One extra receipt is reserved for revoking the entire environment, so full history or a
clock rollback cannot prevent that stop. Reaching a limit blocks other new changes; no
history is discarded. Archival/compaction, aggregate quotas
and administrative recovery are not yet implemented. The registry is not an enterprise
identity boundary or protection against a database owner rewriting the complete database.

## Remaining integration

1. Implement adapters that verify artifact identity and execute negative isolation probes;
   authenticate their reports, retain the report artifacts, and enforce resource limits.
2. Implement the isolated installation/build operation and pin all transitive dependencies.
   Failed installation must leave active files and versions untouched.
3. Add environment selection to product start/workflow/schedule commands and the shared
   management UI. The optional frozen RunSpec binding and generated read contract now exist.
4. Complete runner-side fencing and revocation checks at actual launch. The private
   [process journal](PROCESS-JOURNAL.md) now couples environment validation, worker lease,
   policy, exact approvals and workspace ownership in one admission transaction. A database
   check alone cannot close the later OS launch race or prove enforcement of the limits.
5. Add permissioned network allowlists and explicit credential delivery through a new supported
   definition format, then qualify the complete Windows/Linux execution paths.
