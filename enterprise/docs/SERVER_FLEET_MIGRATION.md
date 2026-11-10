# Installed fleet: capacity and same-organisation cutover draft

Konkrete Entscheidung: Die sieben vorhandenen Instanzen passen mit den heutigen
Standardmaxima nicht in den unveränderten Parentplan. Die vorhandenen Aufgaben,
Daten und Instanzen bleiben erhalten. Als nächster Schritt braucht es eine
ausdrückliche Entscheidung über zusätzliche, eigenständig budgetierte
Ausführungshardware oder ein neu qualifiziertes Gesamtmodell einschließlich
Parentplan, Gastreserven und unterstützten Jobprofilen. Kleinere Dienst-/Jobprofile
allein reichen im unveränderten Plan nicht. Eine Queue,
Host-Neuzuordnung per SQL oder automatische Budgeterhöhung löst diese Grenze
nicht. Dieser Entwurf setzt nichts davon um.

This is a source-only planning contract. It changes no instance, capacity plan,
policy, host assignment, task, runtime image or canonical project documentation.
The companion calculator reads only three explicitly selected, SHA-pinned public
reports and performs integer arithmetic. A mathematical fit is never deployment
authorization or a physical measurement.

## Observed scope and what is still unknown

The passed public scope report is
`Open_Cowork/.internal/server-execution-core-2026-10-09/installed-instance-scope-3d5134d6ee1c4716b8600a077d5d1132.json`
(SHA `aee730c69a90547ce1639130405243983189449539ae86f57f858d79c8613ae8`).
It binds inventory SHA
`86519b43b0426034e4c63d2a8760a701a0e93e7f9155f59a78b338f876351a92`.
All seven instances were ready/compose, with the host column absent and creator
unknown. The observed organisation groups are:

| Existing organisation | Instance count |
|---|---:|
| `55cc98fc-e9ab-4c21-9359-46bb16d75718` | 3 |
| `cb4501f3-bb2a-4355-9074-41ccf7cf19fc` | 2 |
| `20bcc231-9faf-4b8c-b1af-a605e413ef03` | 1 |
| `07fce15a-8c7f-4aa4-bb8b-a07d821015a5` | 1 |

There are no invented target IDs or inferred administrator rights. A future
operator must re-read these exact instance revisions, organisations, owners,
policies, execution epochs, live operations and routing before preparing a
transition. This historical report does not prove today's policy or workload
state. It cannot authorize a migration by itself.

Root hat ausschließlich die öffentliche Pilotbindung aus den eigenen Metadaten
bestätigt: Trust-Domain `749d5d15-b078-4248-b4c2-cc9caa99692c`, rohe Metadaten-SHA
`1c3fbbc5b4f1c3b05dc929883c789584c6b7efdc0654bd3bb7c083daba6f0279`.
Sie entspricht keiner der vier oben beobachteten Organisationen. Diese Angabe
ist ausdrücklich ein Root-Befund; der Rechner liest keine private vm.json.
Eine frisch erzeugte Qualifikationsorganisation belegt ebenfalls keine Rechte
an der installierten Flotte. Der geschützte Pilot bleibt unverändert und voll
belastet; er wird nicht als eines der vier neuen Organisationsziele angerechnet.

The inventory has 39 original containers, finite ceilings of 35,000 CPU millis
and 60,928 MiB, and 25 containers without finite CPU/RAM limits. Ceilings are
neither current usage nor reservations. Docker Desktop's legacy workloads are
not silently covered by the Linux-account VM ledger. No global Windows/other
WSL-user physical guarantee follows from either report.

## Capacity equations under the unchanged plan

Public parent-plan SHA
`db4f474f229d56329501458c7dbdff0e3b8b7b9401921456ec7b9cd10d749d23`
specifies 12,000 CPU millis/14,336 MiB, with 2,000/4,096 reserved and
500/512 QEMU overhead per VM. Its bounded Linux observation is 22,000/27,400.
The latter is an upper bound, not an approved plan increase. Root supplies the
historical CAS-7 snapshot of pilot plus test VM: 9,000/9,216 charged. The calculator
does not read the pool and labels this separately from the dated public plan.

Current fixed guests use 4,000 CPU millis/4,096 MiB, so each parent charge is
4,500/4,608. Preparing, stopped and unknown VMs retain the full charge. Time,
idle jobs, stopped guests, and a smaller instance policy release nothing.

| Historical scenario retaining two VMs | Parent CPU millis | Parent MiB | Unchanged plan fits? |
|---|---:|---:|---|
| Existing two plus parent reserve | 11,000 | 13,312 | Arithmetic only |
| One further fixed guest | 15,500 | 17,920 | No |
| Four new guests, one minimum engine per organisation | 29,000 | 31,744 | No; also exceeds observed Linux bound |
| Seven new guests with the currently qualified one-instance guest profile | 42,500 | 45,568 | No |

In that two-VM snapshot free space was only 1,000 CPU millis/1,024 MiB, so no next fixed
guest fit. A queue cannot make these guests fit. A retirement
would need the existing explicit physical removal/tombstone/CAS proof; this
draft never assumes that a pilot, test VM or legacy instance has been deleted.

Der separate de666-Retirementbeleg (development artifact omitted from public sources)
und der spätere 5bb-CAS13-Recoverybeleg (development artifact omitted from public sources)
bestätigen die dauerhafte physische Entfernung genau dieser eigenen fehlgeschlagenen Test-VMs.
Im datierten Abschlusszustand vom 9. Oktober 2026, 04:08:01 UTC, hält CAS14 nur
den unveränderten Pilot mit 4,500/4,608 belastet. Stop genügt nicht. Erst exakter physischer Ruhe-/Entfernungsbeweis,
Tombstone und Parent-CAS erlauben die neue Bestandszahl; der Rechner beobachtet
oder vollzieht diese Freigabe nicht. In diesem datierten Ein-VM-Snapshot sind einschließlich Parentreserve
6,500/8,704 belegt. Eine zusätzliche feste VM bringt die Summe auf
11,000/13,312 und passt rechnerisch in den unveränderten Plan. Vier neue
Organisations-VMs ergeben dagegen 24,500/27,136: CPU überschreitet sogar die
beobachtete 22,000-Obergrenze. Der Pilot wird weder verkleinert noch als
Organisationsziel wiederverwendet. Rechnerische Passung sagt nichts über freie
physische Ressourcen während der bestehenden Docker-Flotte aus. Die
separate Planprüfung (development artifact omitted from public sources)
besteht 21 Tests und berechnet für diesen datierten CAS14-Zustand genau einen weiteren festen VM-Slot.
Sie prüft ausdrücklich keinen aktuellen Ledger und keine aktuelle Docker-Inventur.

`resource_budget.py:18-23` freezes four service maxima per instance:
API 1,000/512, worker 500/512, runner 500/256, egress proxy 250/128. Sum:
2,250 CPU millis/1,408 MiB. `crates/cowork-contracts/src/enterprise.rs:178-185` defaults to an
aggregate sandbox pool of 4,000 CPU millis/8,192 MiB and two concurrent runs.
That pool is charged once, not multiplied by two. A real GUI job separately
requests 2,000 CPU millis/4,096 MiB (`desktop.rs:1549-1552`,
`crates/cowork-contracts/src/lib.rs:792-801`). The job request does not replace the frozen pool.

For seven instances, retained service plus default sandbox maxima are
43,750/67,200. Even replacing the pool with the optimistic single-GUI-per-instance
floor would need 29,750/38,528 before any guest reserve, QEMU or parent reserve.
The mathematical four-engine consolidation lower bounds below additionally
retain both existing VMs, guest reserve 1,000/1,024 per engine, QEMU 500/512 per
engine and parent reserve 2,000/4,096:

| Optimistic four-engine lower bound | Parent CPU millis | Parent MiB |
|---|---:|---:|
| Seven default services, zero sandbox capacity | 32,750 | 29,312 |
| Seven default services plus one GUI envelope each | 46,750 | 57,984 |
| Seven default services plus default aggregate pools | 60,750 | 86,656 |

Even the service-only bound fails the observed Linux ceiling. This does not
propose sizing a real guest to an arithmetic lower bound. Current hosted
qualification is a one-instance profile: attested 4,000/3,584, guest reserve
1,000/1,024, small sandbox pool 500/512. It fits its own small workload but cannot
admit the current 4-GiB GUI request. Two and three instances with default service
maxima also exceed its available CPU before sandbox allocation. Four engines
for seven instances would require separately qualified multi-instance guests;
the current one-instance endpoint binding cannot simply be reused seven times.

The next decision must therefore be explicit: retain existing tasks/data on the
legacy fleet and provide separately budgeted hardware/execution hosts, or qualify
a new complete capacity model including parent plan, reserves and supported jobs.
Smaller service and sandbox envelopes alone cannot fit four organisation engines
under the unchanged plan: pilot, parent reserve, four guest reserves and QEMU
already require 12,500 CPU millis/14,848 MiB before any service or job pool.
No smaller resource values or performance sufficiency are invented here. A
profile proposal needs measured API/worker/runner/snapshot/migration memory,
actual job demands and headroom, then immutable plan registration and bounded
operator attestation. Raising a plan to the observed Linux ceiling alone still
does not solve the default fleet, and cannot be performed by this calculator.

Die konkreten notwendigen Profilbedingungen für jede Organisation `o` sind:

```
sum_i_in_o(service_max_i + sandbox_pool_max_i) + guest_reserve_o <= guest_cap_o
pilot_charge + parent_reserve + sum_o(guest_cap_o + qemu_overhead_o) <= parent_plan
actual_job_CPU/RAM + other_starting/live/unknown_job_CPU/RAM <= reserved_pool_i
```

CPU und MiB werden jeweils getrennt geprüft, zusätzlich zu Global-/Org-/User-/
Instanz- und Runner-Job-Anzahlgrenzen. `service_max` und `reserved_pool` sind
eingefrorene Maxima; STOP/TTL/Queue dürfen sie nicht verkleinern. Gleichzeitige
GUI- und separate Toolcontainer müssen ihre echten Ressourcen gemeinsam halten;
eine Run-Anzahl ist kein Ersatz für die gesamte physische Jobmenge.

Nach ausschließlich der eigenen Test-VM-Retirement, mit unverändertem Pilot und
vier *neuen* Domains, beanspruchen allein Pilot, Parentreserve, vier heutige
Gastreserven und QEMU-Overheads mindestens 12,500 CPU millis/14,848 MiB. Im
unveränderten 12,000/14,336-Plan bleibt für alle sieben Dienste und Jobpools
bereits -500/-512: kein positives Profil kann diese Bedingungen erfüllen.

Nur als obere **Sizing-Grenze**, falls später eine ausdrücklich freigegebene
größere Planung überhaupt physisch nachgewiesen werden könnte, lässt der
beobachtete Linuxwert 22,000/27,400 höchstens eine Summe von 9,500 CPU millis und
12,552 MiB für *sämtliche sieben Dienst- plus Poolmaxima* zu. Bei hypothetisch
gleichen Instanzen wären das höchstens 1,357/1,793 pro Instanz; das ist keine
Profilempfehlung oder Messung ausreichender Leistung. Schon die aktuellen
Dienste allein liegen 6,250 CPU millis darüber. Für die aktuellen Dienste
verbleiben rechnerisch -6,250 CPU millis/2,696 MiB für alle Pools zusammen.

Alle sieben Instanzen mit unverändertem 4-GiB-GUI-Job bedienen zu können erfordert
im heutigen Modell mindestens sieben 4-GiB-Instanzpools. Deren 28,672 MiB
überschreiten schon ohne einen Dienst die 12,552-MiB-Sizing-Grenze. Ein globales
Runlimit von eins hebt diese dauerhafte Poolreservierung nicht auf. Ein neues
gemeinsames Poolmodell wäre eine eigene Kapazitäts-/Isolationsänderung mit neuer
Qualifikation; es wird hier nicht vorausgesetzt. Für kleinere unterstützte
Jobs müssen explizite Dienstprofile und echte Job-Envelopes innerhalb der obigen
Ungleichungen qualifiziert werden. Falls die vorhandenen GUI-Aufgaben diese
Ressourcen weiter benötigen, ist die minimale ehrliche Entscheidung zusätzliche
budgetierte Ausführungskapazität statt stiller Verkleinerung vorhandener Aufgaben.

## Admission and fairness: separate levels

Existing `management/execution_admission.rs:343-478` locks rollout, global,
organisation and host scopes, then checks current membership/instance/epoch,
host enablement, qualified static coverage, and platform/org/user/instance held
run counts. Defaults are 40/40/2 for platform/org/user. Host55 allocation sums
retain instance services plus pool maxima (`host_capacity.rs:79-84,203-219`);
Python mirrors that in `resource_budget.py:168-207`. Neither a central run-count
reservation nor an instance queue provides extra physical VM RAM.

A future supported profile must have both permanent capacity and live gates:

1. Parent plan contains every retained VM charge, OS/fleet reserve and QEMU
   overhead, across parents. Unknown inventories refuse new reservations.
2. Each organisation has a distinct enrolled engine/trust domain. Manager host
   allocation contains every instance service maximum and aggregate sandbox
   pool once. Fresh attestation binds the complete inventory, exact manifest,
   engine/operator identity, execution epoch and host revision. Disable,
   unknown inventory, stale attestation or plan mismatch fails closed without
   releasing existing charge.
3. Job admission uses the actual CPU/RAM/count vector inside that already
   reserved pool. In an 8-GiB/4-CPU pool, at most two existing GUI envelopes fit;
   in the small 512-MiB/0.5-CPU fixture, zero fit. Held/starting/unknown physical
   attempts remain charged until exact stop is proven. Required snapshot or
   migration helpers need an explicit bounded service/maintenance envelope.
4. Global/org/user/instance count limits remain independent bounds. A future
   fair queue rotates authorised org/user queues with stable thread ordering;
   it must not hold a physical job slot for a blocked head. Preserve existing
   local claim rotation and token fences. Persist an authorised, content-free
   waiting reason for the specific failing gate, with recheck time. Do not
   claim current local rotation already implements weighted cross-org fairness.

Unknown physical job stop, host disable and lease timeout never free capacity.
No double counting: the static aggregate pool is charged to its host once;
individual job vectors are checked against that pool, not added again to the
parent VM's RAM charge. Canary source/target instances on distinct retained
hosts both consume capacity. The existing max-of-generations rule is only for
qualified sequential generations of one instance with no parallel workloads.

## Same-organisation import contract and qualification boundary

The design below now has a source implementation and limited current
target-readback acceptance (development artifact omitted from public sources).
It binds 254 source inputs and covers the real authenticated router/PG fences,
original age backup verification and target-content readback tests. Native
Windows/Linux and the Management import interface have separate source-bound
checks; see [implementation status](SERVER_IMPLEMENTATION_STATUS.md).
The arithmetic below is unchanged. Physical source/target engines, service-side
old-credential rejection, complete fleet transfer and live rollout remain open.
No route, installed data, host binding or capacity budget was changed.

Migration0053 (`0053_executor_host_assignments.sql:46-58`) rejects any host change,
including NULL to UUID, and gives a composite same-organisation FK. Host engine
and operator identities are immutable and unique. `management.rs:1588-1593`
and `instance_backups.py:82-89,348-365` require catalog and manifest to match the
same instance and organisation. Upgrade/rollback select the current instance's
host/release qualification. Consequently a backup does not authorize rehoming
an existing legacy instance, and ordinary new-instance Restore cannot import
that source backup. Both remain deliberate fences, not SQL fixes to bypass.

The smallest new private operator contract is a same-org source-to-new-instance
import operation. It leaves the old instance and its NULL binding immutable:

| Frozen binding | Required meaning |
|---|---|
| Source and actor | Exact observed source instance/org/revisions/release/epoch; current operator and org admin authorization |
| Target | Manager-created fresh instance UUID on a newly enrolled same-org host, immutable host/daemon/operator IDs and current revision |
| Export | Verified original source catalog ID/ciphertext digest, original manifest scope/digest, complete workspace/object/DB inventory |
| Runtime authority | Exact old reservation and runner-attempt sets, current source generation, physical quiescence receipts; no JWT reuse |
| Target resources | Frozen service/pool plan, independently charged parent VM, qualified release/schema/profile, fresh target execution epoch |
| Cutover | Unique operation/request digest, route alias plus expected route revision, signed completion/reconciliation receipts, phase CAS |

Do not rewrite the signed backup manifest to pretend it belongs to the target.
The new import validator checks its original source identity and a separately
authorised source-to-target binding. Tenant data IDs, thread/workspace bindings
and storage-key-required content remain exact; runtime bootstrap/signing keys,
executor credentials, auth/reauth/stream grants and process ownership are freshly
fenced for the target. Imported unfinished runs and schedules remain interrupted
and disabled for explicit review. No automatic tool replay or cross-engine
Resume authority is inferred from a copied safe checkpoint. Old reservations
must be reconciled against the old engine and exact original attempts, not
completed merely because a copy or TTL exists.

Proposed persisted phases are prepared, source_draining, source_quiescent,
export_verified, target_restored_paused, qualified, cutover_committed and
source_retained. Unknown execution stays at a charged, non-dispatching phase.
Each phase transition takes the current rollout/global/org/host order, rechecks
actor/operation revisions and immutable bindings, and persists its receipt
before publication. Long I/O happens after a claimed durable phase; reconciliation
uses exact identities and does not replay unknown creates or physical stops.

The source is drained and physically stopped before the export. Preserve all
existing tasks, workspace data and histories in the export and retained source.
The fresh target restores into a private generation with admission and schedules
paused. Qualification verifies schema/data digests/workspace bindings, credential
revocation, creator rights, budgets, exact host/epoch proof and one bounded
supported job. Public routing changes only through an atomic expected-revision
cutover after source and target stop/authority gates pass. Re-enable reviewed
schedules deliberately; copying next_run_at is not permission to catch up.

Before target writes, a proven-stopped failed target may be abandoned and source
routing resumed after current source authority checks. After target writes, an
automatic rollback would lose data: require a new target export and authorised
reverse transfer, or keep both paused for operator reconciliation. Keep both
charges until permanent deletion is physically proven. User-facing waiting
reasons should distinguish migration drain, unknown stop, target capacity,
unqualified release and pending cutover. No manual host rebinding, fence removal,
catalog reassignment, policy shrinking or JWT reuse is part of the contract.

## Narrow future implementation and qualification

Coordinate a new private leaf module and migration for the import operation,
binding/phase CAS and routing transition. Integrate only manager operation
prepare/claim/pending/complete/reconcile; provisioner source-manifest validation,
bounded import staging/receipts; existing host-capacity and execution-generation
hooks. Preserve ordinary Backup/Restore and NULL-legacy compatibility. No V2
bridge, SharedContracts change or remote/HA release gate is necessary for this
planning step. New live rollout authorization remains a later explicit action.

Production-core PG regressions must cover same-org success and cross-org/source
catalog mismatch refusal, immutable NULL binding, current-auth revocation,
concurrent import/CAS/idempotency, source and target epoch changes, omitted
workspaces/attempts, unknown old stop, capacity races across parents/hosts,
stale/disabled host attestation, old/new receipt mismatch, expired tickets,
paused schedules, and rollback before/after first target write. Actual isolated
qualification must use genuine source and target engines, owned fresh instances,
encrypted backup integrity, exact workspace data reuse, proven old physical
stop/no replay, supported bounded job envelopes, retained-charge inventory and
accurate cleanup. None of these missing import capabilities is claimed deployed
or qualified by this arithmetic draft.
