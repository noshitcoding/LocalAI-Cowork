# Explicit runner target registration

The common runner registry binds the actual `runner_id`, generation, adapter
build SHA-256 and separate controller/runner public keys from the existing
private `COWORK_PROCESS_RUNNER_CONFIG`. Reading a configuration or registering
it does not qualify an environment, install a runtime or launch a process.
There is no renderer, tenant-session or remote enrollment route for this registry.

The target ID is a domain-separated digest of the actual controller public key
and runner ID. It is independent of an owner, organization, Team or device
session. Runner-key/build changes require a strictly higher generation and
produce a new inactive revision. A different controller is a different target;
it cannot adopt or activate the old controller's record.

## Operator workflow

Set `COWORK_DATA_DIR` and `COWORK_PROCESS_RUNNER_CONFIG` to the intended existing
installation and its private configuration. Use its current daemon executable:

```text
cowork-daemon execution-target inspect --configured
cowork-daemon execution-target register --configured 0
cowork-daemon execution-target activate --configured 1
```

The initial registration is `registered`, not `active`. Every state change
requires the exact inspected revision. A lost command response is resolved by
inspection, not by inventing a new registration or resetting its generation.
Repeating an unchanged registration at the current revision changes nothing.
The commands reuse the existing private-file checks and verify that the actual
controller signing key matches the configured public key. Private signing keys
and transport tokens never appear in their output or registry rows.

Normal startup keeps the control plane available even if the configured target
is missing, inactive or belongs to another generation. Such a configuration
supplies no execution reference. An active exact registration lets the Runtime
freeze its reference; it checks that reference before process
preflight, installation, activation, new lifecycle effects and preview grants.
Existing process/workflow watchers repeat the check and use the original signed
fence/stop path on revocation. Inspection, exact receipt recovery and cleanup of
already owned sessions remain possible after revocation, including after restart
with the original verified private configuration. No old guest is relaunched
to recover a receipt.

```text
cowork-daemon execution-target revoke --configured 2
```

Reactivation is a separate explicit operator action at the new revision. A live
Runtime keeps its old reference and cannot silently adopt it; a deliberate restart
binds a newly active revision. Existing environment qualification and native
process/isolation checks still apply in addition to this registry.

## Retained plans and database custody

Migration 107 adds current targets and immutable revision records. Mutation
serialization bounds the registry to 1,024 targets and 10,000 revisions per
target. Reads validate both the current projection and its original revision
payload. Missing or inconsistent history cannot establish authority.

Migration 108 retains a native Owner plan's target reference separately in the
same prepare transaction. Format-1 plan bytes, hashes and existing acceptance
receipts remain unchanged. Admission checks that exact reference under the
registry writer before inserting a run. Revoke/reactivate followed by restart
cannot make an unconfirmed old plan use the new target revision. Previously
accepted plans still return their original receipt. Older unconfirmed native
plans without this binding must be retired and prepared again.

These private references are not the forthcoming common public `ExecutionPlanV2`.
The full actor/resource/input-snapshot/receiver binding, common Team/device
dispatch, complete shipped runtime and physical platform qualification remain
open. Team preflight continues to report `ready:false`. Current source changes
and scoped tests must not be presented as final-package or isolation acceptance.

## Current verification

The current native Windows artifact passes all 24 Runner cases, including eight
commit/revoke races, actual child fencing, the fresh target-binding getter and
receipt/cleanup recovery after restart with an inactive target. Eight selected
cases also pass against separate PostgreSQL 16 databases at schema 110. The
signed lifecycle transport remains a fixture; it does not qualify Kubernetes
or physical sandbox isolation. The private WSL cluster is positively stopped
and its directory removed after the checks.

Four SQLite registry cases, five SQLite historical migration cases and twelve
regular Owner plan regressions pass. The four registry cases and both historical
PostgreSQL upgrades also pass at schema 110 with their captured native test
artifacts; the separate private cluster and databases are positively retired.
The actual operator executable also passes
five cold HTTP startups: missing, registered, active, revoked and a newly
registered generation. All six revision receipts remain; no registration is
implicitly activated and no runner endpoint, model or tool is contacted.

The retained walkthrough profile has upgraded from schema 108 to 110 with all
23 captured content, plan, Team, blob and registry tables and original migration
receipts unchanged. A verified database backup precedes the upgrade. No target,
old native-plan binding, input custody or timer wake is created automatically.
The exact qualified executable serves the retained profile and passes its
authenticated HTTP health check. The current web build passes TypeScript,
contract consistency for 696 types and 485 frontend tests.

Private evidence is under `.local/remediation-20261024/`, including
`execution-target-runtime-postgres-v3.json`, `execution-target-cli-v2.json` and
`target-availability-retained-profile.json`. Earlier failures and older
qualifications remain separate. The new PostgreSQL checks use an isolated local
WSL cluster. Docker Desktop was subsequently recovered by retaining its stopped
stale IPC directory. The separate new Enterprise image and both authenticated
metadata API modes now pass with the timer-compatible source pin. These metadata
checks do not qualify common execution or physical isolation.
