# Broker lock lifetime

The broker now returns an opaque `ResourceLock` guard instead of a raw
`std::fs::File`. It explicitly calls `File::unlock` when the owning operation
ends, then closes the descriptor. The guard cannot be cloned or used as a raw
handle. Prepared file writes, operations, snapshots and reconciliation retain
this guard through their existing durable-receipt boundary. Lock acquisition
remains nonblocking and still returns `workspace_busy` when an actual reader or
writer occupies the required resource. Revocation does not bypass active IO.

On Unix, closing one descriptor does not release a flock while a duplicated
reference to the same open-file description remains open. A concurrently spawned
child can temporarily inherit such a reference before exec closes it. An explicit
unlock releases at the operation boundary even while the duplicate remains.
Two deterministic Unix tests retain a duplicated descriptor: an independent
writer is blocked while the guard is live, and can acquire after the guard drops.
Both exclusive and shared owner locks are covered. A raw file dropped without
explicit unlock would fail these tests.

## Observed qualification failure

The first current Ubuntu workspace run stopped at
`paths_cannot_overlap_private_or_other_active_roots_and_identity_cannot_silently_change`,
where revocation unexpectedly returned `WorkspaceBusy` (registry.rs:218).
An unchanged targeted rerun failed instead at
`registration_is_durable_idempotent_scoped_and_revocable_without_restart`, where
an expected argument conflict was preempted by `WorkspaceBusy` (registry.rs:69).
The other registry test starts real child processes concurrently. Transient
inherited descriptors are a plausible explanation for these timing failures;
the deterministic duplicate-descriptor regression proves the old lock-lifetime
behavior independently. It does not prove that every historical registry failure
had this cause.

Reports preserve the failed initial run, the two new deterministic probes, 40
repetitions of all six substantive parallel registry scenarios, and the subsequent
full Windows/Ubuntu workspace checks. No test is skipped or serialized to make
the failure disappear. Process child fixtures remain ignored in the outer harness
and are executed by their parent tests. Current evidence is linked from
`.evidence/workflow-foundation-2026-10-08.json` after qualification finishes.

This change does not provide an OS sandbox, authenticated runner, terminal product
integration, metadata/quotas, network enforcement or enterprise isolation.
