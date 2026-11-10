# Central execution resource consumer

The upgraded Enterprise instance exposes `GET /api/v1/managed-resources` and
accepts a distinct, bounded `input.managed_resource_ids` selection on an immediate
Linux server agent run. The authenticated user identity comes from the instance
session. Organisation and instance identities come from its operator configuration.

The instance requests management `resource-entitlements` with
`consumer_kind=execution_context&consumer_version=2`. Only currently assigned,
enabled resources are accepted. An agent role references other assigned resources;
selecting a role does not grant its dependencies. Each prepared run retains the
exact resource revisions and assignment revision. A changed or unavailable
management authority prevents the next model or tool dispatch. These checks do not
replace the run's capabilities, tool approval or execution target policy.

Skills enter the frozen runtime context. One selected role contributes its system
prompt and optional assigned model. Selected MCP entries become execution-target
bindings. Provider credentials stay on the existing management model proxy.

MCP credentials require an explicit target-local binding. `env:` references accept
only `COWORK_MANAGED_MCP_SECRET_<identifier>`. `secret:` references accept a leaf
filename below `/run/secrets/cowork-managed-mcp`. Symlinks, traversal, arbitrary
process variables and `vault:` references are rejected. HTTP MCP receives a Bearer
header; stdio MCP receives `COWORK_MCP_TOKEN`. Neither value is returned in the
resource list or frozen run specification. A missing binding makes the resource
unavailable before preparation.

Local Docker acceptance exercises real management and instance APIs, role and
skill context, the managed-model proxy and an actual sandbox MCP process. Its
model is a controlled local protocol fixture. Revoking a dependency after the
model response prevents the next tool dispatch and new preparation. A typed
dispatch denial releases capacity only when the run has no tool/runner attempts,
all model requests returned to a trusted safe cursor, and desktop cleanup is
confirmed. Unknown effects remain fenced. A second instance executes afterward.

This consumer currently covers server agents and organisation/instance grants.
Environment-bound execution, Crew and device consumers remain incomplete and are
rejected explicitly. The management UI reports partial delivery. This document
does not qualify those consumers or an external production MCP service.

## Common V2 execution contract artifact

The approved rebuild converges execution on the common V2 runtime. Enterprise
consumes a pinned source artifact from the active V2 checkout; it does not define
a second set of execution-plan types. Generate it in that checkout with:

```powershell
[Private development path omitted from this public documentation copy.]
[Private development path omitted from this public documentation copy.]
```

The exporter keeps a closed, deterministic schema/OpenAPI subset and source
hashes under `contracts/v2-runtime/<sha256>/`. It refuses changed immutable
bundles and unrelated source files. The generated Rust pin is an explicit choice
for the next local build. Later unrelated generator additions do not require a
new pin when the selected schema/OpenAPI bytes are identical; the read-only
checker separately reports whether the entire producer snapshot is still equal.

The current source pin is
`b26930323954804895a67b025cc5d7975d21af2950a72f7904490b29df4bf2fd`.
Its 92-definition closure contains 16 roots and 22 routes: the original Owner
plan, Owner PlanV2, confirmed model-free restoration/import, input prepare/inspect/retire
and read-only working-copy files/text and exact original workflow input snapshots.
The optional original-input reference conveys data identity, not runtime rights.
The common `waiting_for_timer` RunState remains included. Original
PlanV1, PlanV2 and actual native restoration/import prepare/review/run samples, exact canonical public hash
bytes, rejected actor injection/unsupported targets/missing canonical bytes,
ten exporter cases and five native embedded-bundle cases pass. Actual native input
reference/review/run samples retain canonical large-integer bytes. Temporary
Windows directory-publication locks use bounded atomic retries; permanent locks
still fail without publishing partial bundles. Previous immutable bundles are
retained. The new local image also passes its offline read-only descriptor and
authenticated metadata HTTP checks in management and workspace API mode. Both
checks use exact copies of the retained test databases; all captured public
table row digests in the source databases remain unchanged. Anonymous and revoked access is
rejected, test sessions are revoked, and the new test containers/database copies
are positively removed. These are source-contract metadata checks; they do not
qualify a model, runner, input transfer or workload admission. The local image
`cowork-remediation-server:workflow-input-contract-20261030` is bound to 275 captured build
inputs. Both metadata API modes return the exact newly pinned bytes. The
workspace probe disables execution workers because this is a metadata-only
check. Evidence is retained in the V2 checkout under
`.local/remediation-20261024/workflow-input-source-qualification.json` and the separate
`enterprise-v2-workflow-input-contract-{ajv,image,http}.json` reports. The previous
restore-only bundle and its independent evidence remain retained.

Both the server image and native build embed the selected bytes. Startup verifies
the pin, file hashes, supported manifest, provenance paths and all local references.
The offline `cowork-server --v2-runtime-contract` descriptor and authenticated
`GET /api/v1/runtime-contracts/v2` expose the actual built artifact. Sibling
`/schema` and `/openapi` routes return those same verified bytes. These routes use
the existing authentication boundary in management and workspace mode.

This artifact proves source-contract compatibility only. It is not a signature,
an installed runtime attestation, an authenticated Enterprise actor, a snapshot
binding, or permission to execute. The current owner plan must not be treated as
team/device authority. Completing environment/Crew/device delivery still requires
the common runtime's authenticated actor/resource bindings, fresh revocation checks
and immutable snapshot/effect/result path. Existing narrower server-agent checks
and their historical acceptance remain separate.
