# Scoped host capacity coverage

The Host55 Python client is opt-in. It supplements the manager's permanent static
allocation ledger with short-lived observations of the selected Docker engine.
It does not add a second dynamic resource charge or release an allocation when a
run ends, a service stops, an operation fails or coverage expires.

Use the dedicated remote entrypoint with an operator-owned private JSON file.
In addition to its immutable executor host, execution-admission image allowlist,
full `fleet_host_budget` and service limits, configure:

```json
{"host_capacity_attestation":{"version":1,"host_revision":1}}
```

The revision must match the manager's registered host revision. The complete
host budget, including reserved CPU/RAM, and all four service caps must exactly
match that registration. This option is disabled when absent; a Host55 operation
carrying an allocation cannot run through a disabled or legacy client. Once the
client's private state marker exists, removal or revision changes require
operator review. A missing endpoint, 404, invalid token, changed binding or TLS
error never changes a hosted operation into a local/NULL-host operation.

## Observation and renewal

The client requests a fresh one-use `/host-capacity/challenge`, verifies its
complete host identity, host revision, allocation revision, frozen plans and
current epochs, then independently inspects every container on the bound daemon.
Actual CPU count and memory come from `docker info`, including its measured
`MemTotal`; VM configuration and manager-supplied capacity are not measurements.
The container inventory is read again after collection. A concurrent create or
remove makes the observation uncertain rather than a complete filtered view.

For each retained allocation, the client verifies the owned manifest's canonical
SHA256, persisted resource budget and active execution epoch. Observed singleton
API, worker, runner and egress-proxy containers must use the exact manifest image,
environment, Compose labels and CPU/RAM/PID limits. Extra replicas, hidden
services, unknown containers, another operator's retained state and changed caps
fail closed. Admitted job, desktop, terminal and transfer containers are checked
against the one shared sandbox pool; their aggregate CPU/RAM/count cannot exceed
that pool. The manager's full ledger is reported even for stopped instances.

An allocation is reported `reserved` only before first deployment, with no
manager runtime evidence and no local manifest, resource budget or containers.
An applied local manifest that has not yet been committed by manager completion
is reported `unknown`. It cannot masquerade as manager-current `applied`
evidence. Receipt completion remains possible, followed immediately by a fresh
challenge and observation of the committed revision. Permanent deletion retains
its charge until the manager commits its exact frozen allocation binding and
verified deletion inventory; the client never sends a release request.

A background thread renews every 20 seconds even while a lifecycle command is
blocked. Collection must finish within 25 monotonic seconds. The manager alone
uses its DB clock to enforce the 30-second challenge and 120-second coverage
deadline; local wall-clock skew cannot extend either. Renewal failure clears
local qualification and leaves manager admission to expire or reject coverage.
Existing completion and bounded stop/delete recovery remain available; new
activation requires a fresh qualified observation.
The private fixture-only remote lifecycle profile also requests fresh coverage
before backup, restore, upgrade or rollback. Staged local evidence remains unknown
until manager completion and never releases the static charge.

`host-capacity-status.json` records only qualification, a fixed reason code,
binding/revisions and challenge identity. It contains no token, certificate path,
endpoint, private key or raw Docker inspection. Its recorded server timestamp is
diagnostic information, not an independent admission deadline. Only manager
coverage is authoritative.

## Qualification boundary

Unit tests exercise malformed/scoped challenges, real local evidence files,
unknown workloads, exact container caps and environments, shared pool exhaustion,
clock skew, collection expiry, background renewal, durable journal/receipt
fences and legacy fallback refusal. They do not prove a deployed Host55
manager-to-VM cycle. The separately frozen Source54 VM fixture cannot establish
Host55 or later HA qualification. Remote advanced lifecycle remains closed by
default; the separately bound private fixture-only opt-in prepares its real
transport matrix. Execution enrollment remains closed in every remote profile.

```text
python -m unittest discover -s server/cowork-provisioner -p test_host_capacity_attestation.py -v
```
