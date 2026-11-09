# Team execution binding provenance

The next common plan must distinguish the authenticated team session from the
installation owner. `execution_actor.rs` and `team_execution.rs` contain the
staged review types. They do not change the accepted `ExecutionPlan`, `RunSpec`,
`RunRequest` or `RunPreflight` unions, expose a new route, or grant execution.
The current Enterprise artifact pin remains an owner source contract.

The resolver must build the actor inside the TeamService transaction from a
verified `Principal`. A client cannot submit an actor as part of
`TeamExecutionRequest`; reading/deserializing a review authenticates nobody.

| Review fact | Existing source | Existing fresh check |
| --- | --- | --- |
| Identity/session IDs | Verified identity `Principal` | `identity_sessions` joined to active identity, correct identity ID, unrevoked session and unexpired database clock |
| Session revision | Locked session row | Exact equality to the admitted binding |
| Organization ID/epoch | `Organization.authorization_epoch` | Active organization and unchanged authorization epoch |
| Membership ID/revision | Membership joined to session identity and organization | Active membership, exact ID/revision, role other than viewer |
| Project resource ID/revision | Identity resource and TeamOrder project | Same organization, unchanged resource revision, unarchived team project |
| Content grant revision | Grant for the membership and resource | Active grant, same revision, read and execute permission bits |
| Order revision/prompt | `TeamOrder` | Unchanged order revision and unarchived order |
| Provider revision/model | Team LiteLLM configuration | Exact provider revision, no client provider credential or owner fallback |
| Documents | Selected team/project documents | Exact ID, kind and scope at resolution; revision and archive rechecked at effects |
| MCP configuration | `integrations::freeze_tools` | Server revision, catalog SHA and selected tool set, validated by the existing integration gate |
| GitHub integration | Frozen GitHub revision | Current integration gate |
| Personal knowledge | Explicit creator selection and administrator permission | Existing knowledge-access checks; frozen memory IDs/revisions must be included in the common review |

`TeamService::create_run_in_transaction` and `preflight_execution` now share
the private resolver in `crates/storage/src/team_execution.rs`.
`validate_binding` rechecks the applicable rows before model/tool effects,
together with lease and scheduled-run gates. The review performs no run
insertion, provider-key opening or executor call. It reports `ready:false` with
`unbound_execution_target` until the common target adapter can prove the
executor/receiver binding. It also includes the actual project-profile revision
and model destination. Common prepare/commit must use this resolver rather than
maintaining separate role/grant checks.

`identity_installation` has no stable public installation UUID for this binding.
No installation epoch or device identity is invented. The common plan must bind
the authenticated executor/receiver and immutable input snapshot separately.
Owner format-1 receipts must remain readable. A new common plan version and a
new Enterprise pin follow only after the integrated resolver and snapshot
binding are ready.

Current verification: three new contract tests reject forged/unknown authority
fields, incomplete team actors and attempts to put a Team request through the
current owner request union. All five tests across the two contract libraries
pass. Full Team/Enterprise/device admission and effects are still pending.

The shared resolver has five current SQLite integration cases and the same five
cases on separate PostgreSQL databases. They verify real session/membership and
content rights, changed project/order/provider/document versions, knowledge and
personal-memory policy, no review side effects, original receipt replay and
project revision gates. All five temporary PostgreSQL databases were dropped and
their absence independently checked in the catalog. Four existing knowledge
cases, one Team synchronization case and all five contract-library tests pass.
These checks do not qualify common PlanV2 admission or an installed runner.

## Proposed common format switch

1. Keep format-1 owner plans and accepted receipts readable with their original
   bytes and hashes. New format-2 plans use `ExecutionActor`, a genuine Team
   preflight variant and the immutable snapshot/receiver binding. A new Team
   request is the distinct `TeamExecutionRequest` wrapper, not an owner
   `CreateRun` with changed labels.
2. Extract the existing TeamService resource resolution into one private
   transaction helper shared by review and admission. Review performs no model
   dispatch or run insertion. Commit re-resolves and compares the exact actor,
   request, order/provider/documents/integrations and target/snapshot before
   atomic admission. Keep the original idempotent Team receipt on repetition.
   Do not create a parallel set of role/grant checks for this adapter.
3. Resolve Team requests only from a verified identity-session Principal. An
   installation-owner bearer cannot substitute for a Team session. The common
   request union by itself is not authorization; the route must select its
   authenticated resolver before any tenant reads or admission.
4. Introduce a private resolved Team variant rather than forcing a tenant run
   through owner-native `RunSpec`. The common runtime must own the model/effect
   journal and result promotion. Team LiteLLM credentials and limits retain
   their tenant scope; there is no personal wallet/provider fallback. Executor,
   connector transmission and snapshot/receiver authority must be explicit
   before preflight can report readiness.
5. Qualify review/commit races, session/grant/document/integration revocation,
   conflicting idempotency keys, original receipt replay, actor/target mismatch
   and interruption with uncertain effects through the integrated product path.
   Only then generate the new closed contract artifact and update the explicit
   Enterprise pin. No new migration number is reserved by this proposal.

For the proposed separate `ExecutionPlanV2`, the minimum Team resources are the
order revision, provider revision/model (and the reviewed model destination from
`TeamProvider.base_url`), selected document ID/kind/revision/content SHA, personal
memory ID/revision, MCP server revision/catalog SHA/selected tools and optional
GitHub revision. These accompany the actor's identity/resource/grant binding.
The resolver must also capture the project-profile revision when using the
existing `create_run_at_project_revision` path. No credential enters the plan.

`TeamExecutor` currently exposes supported plan/tools and recovery behavior; it
does not supply a common `control_id`, `runner_id`, generation, qualified
artifact or capability digest. Consequently those proposed receiver/runner facts
must come from the common trusted target registry, not from fabricated values or
the Team session. Keep Team readiness false until that adapter can provide and
freshly validate the target binding. The optional portable inputmanifest also
needs explicit target support: a model-only run must not imply native file or
process capabilities merely because it has an actor.

The common private runner registry now exists. The native Runtime exposes
`execution_target_binding()` with an active freshly checked target reference and
the actual public runner identity; an inactive configured target supplies no
execution binding. Its registry revision and actual runner generation are
different values. See [execution targets](EXECUTION-TARGETS.md). This getter does
not connect `TeamExecutor` to common PlanV2, grant input transfer or release a
runtime. Team readiness therefore remains false.

## Current model and database qualification

Original admission receipt recovery now checks fresh authenticated Read authority
before returning the exact stored initial receipt. Its scope remains the same
creator identity, Team, order, creation key and request digest. A later loss of
Execute authority, unavailable executor or archived order cannot erase a readable
accepted receipt. Missing current Read authority is denied; conflicting intent
under the same key remains a conflict. Fresh admission still checks current
Execute authority, executor support and unarchived order before dispatch.

This fixes F40. Both failures were reproduced before the change. Four Identity
and eleven Team cases pass with identical captured native artifacts on SQLite
and fifteen isolated PostgreSQL databases, all removed afterward. The actual
local model run replays the same initial receipt after order archival and Execute
revocation, with exactly one dispatch. These are existing-dispatcher compatibility
checks, not common Team PlanV2 readiness. See
`.local/remediation-20261024/team-receipt-qualification.json`.

The shared Actor checker now lives in `identity/src/store/execution_actor.rs`.
`resolve_team_execution_actor_in_transaction` requires a genuinely authenticated
`Principal`; it resolves both read and execute authority under the existing
identity writer. The retained-actor and retained-approver checks reuse that same
identity path. Their serialized facts are not credentials, they return no
`ContentPermit`, and they must only be applied after loading the authenticated
product's trusted plan/run/approval record.

This fixes F39: Team effects and approving-session checks had duplicated the
identity SQL, bypassing its UTC high-water, session-scope ceiling and source
membership gates. A real admitted synthetic effect demonstrates acceptance
despite a future high-water before the fix and rejection afterward. A distinct
approving session can complete its exact approval while unchanged; a later
read-only ceiling blocks its claim while the creator remains authorized.
Provider, project/order, document, integration, schedule, server-key and lease
gates remain separately enforced. Existing binding payloads and original receipts
are unchanged; no migration or generated public contract is introduced here.

Four identity cases and eight Team service cases pass on SQLite and separately
created PostgreSQL databases using the same captured native artifacts. The
negative cases include changed review facts, expiry, revocation, lost read or
execute rights, scoped sessions and clock rollback. All twelve PostgreSQL
databases are dropped and their absence verified in the catalog. A fresh actual
local `qwen3:4b` run preserves frozen skill context, exactly one dispatch and the
original receipt. This is still compatibility of the existing Team dispatcher,
not common PlanV2/runner readiness. Evidence:
`.local/remediation-20261024/team-actor-postgres.json` and
`team-actor-real-model.json`.

The shared resolver's five SQLite and five isolated PostgreSQL cases pass with
schema 106. A separate real local Ollama `qwen3:4b` run confirms the selected
skill's unpredictable nonce reaches the model, exactly one dispatch occurs and
repetition returns the original creation receipt. This qualifies compatibility
of the existing Team dispatcher; common PlanV2 readiness remains false.

That real path exposed F33: the MCP step-limit queries already joined
`team_effect_intents.request_id`, but migration 38 never created that column.
Additive migration 106 projects the original wire ID from each unchanged intent,
checks redundant journal metadata and rejects missing or duplicate IDs atomically.
New intents store the wire ID explicitly. Two SQLite and two PostgreSQL migration
cases preserve original intent/receipt bytes; both older PostgreSQL upgrade
fixtures also pass. The retained local profile upgrades without changing its
projects, orders, messages, events, runs or original migration receipts. That
profile contains no Team intents; the 17-row synthetic historical fixtures
provide the separate backfill evidence. Source-bound proofs and test binaries
are recorded under `.local/remediation-20261024/`.
