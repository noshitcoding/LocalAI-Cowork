# Local executor VM candidate

The user selected the current Windows computer as the execution host. A separate
Ubuntu guest was therefore created under QEMU/KVM in `Ubuntu-24.04`, using real
hardware acceleration. This guest has its own Linux kernel and Docker daemon.
It does not mount the existing Docker Desktop socket or the project checkout.
This is one local pilot for a new trust domain, not a migration of the seven
installed instances or production hostile-tenant qualification.

## Installed pilot and evidence

- Private VM root in WSL:
  `/home/noah/.local/share/cowork/executor-vms/cowork-executor-fcadc54ed8a6476682d747b9e90bfe73`
- VM ID: `fcadc54e-d8a6-4766-82d7-47b9e90bfe73`
- Pilot trust domain: `749d5d15-b078-4248-b4c2-cc9caa99692c`
- Daemon ID: `f0bf75dd-c933-4ad6-b502-27f2ee54deb6`
- Guest kernel observed: `6.8.0-142-generic`
- Limits: 4 virtual CPUs, 4096 MiB guest RAM and a 20 GiB sparse disk.
- State after qualification: **stopped**, with disk and private keys retained.

The secret-free report is
`.internal/server-refactor-2026-10-08/executor-vm-qualification.json`.
Eleven real checks passed: unprivileged KVM, guest bootstrap, an independent
kernel, readonly state even for guest root, mTLS, refusal without a client
certificate, exact image transfer, real provisioner identity/state verification,
foreign-organisation refusal, wrong-daemon refusal and a bounded sandbox on the
guest kernel. All 39 original running container IDs/images were preserved.

The VM was stopped and started during qualification; its readonly state mount
survived that restart. The daemon's forwarded listener was also observed on
Windows `127.0.0.1:2376`. No public listener, global Docker context change, fleet
quota activation or WSL network change was used.

## Runtime boundary

QEMU runs as `noah`, not root, with its private QMP socket and PID file. The
controller checks the recorded process command and disk identity before stopping
it. SSH uses a generated key and a private known-host file; password/root SSH is
disabled. Docker requires mutual TLS and has exactly one operator role and
trust-domain label. Only host-loopback SSH and Docker ports are forwarded.

The only host filesystem shared with the guest is this pilot's private `shared`
directory, mounted readonly through 9p at the same absolute path. Certificates,
CA private key and SSH client key are outside that share. Guest root's write to
the share was refused. The real provisioner reads a fresh exact nonce through a
networkless, readonly, capacity-bounded helper with all capabilities dropped.
The helper explicitly uses the operator UID because its nonce is private `0600`;
an image's default sandbox UID must not determine this trusted read.

The proof job used the immutable core image
`sha256:7fbe73c08e667f6bde2883bd3797e1cd15173e40370ed8dc79cf27d08588942e`
with network `none`, readonly root, no capabilities, no new privileges,
0.25 CPU, 128 MiB and 64 PIDs. No daemon socket was passed into it.

## Control the retained pilot

From an Ubuntu-24.04 shell:

The current controller requires the explicit capacity plan and read-only
adoption described below before `start` or `qualify` can use a retained v1/v2
pilot. Its original qualification used the earlier frozen controller. This
initial capacity source phase did not modify the pilot. Its later explicit
read-only adoption retains the same allocation; adoption never permits retirement.

```sh
[Private development path omitted from this public documentation copy.]
vm_root='/home/noah/.local/share/cowork/executor-vms/cowork-executor-fcadc54ed8a6476682d747b9e90bfe73'
python3 "$vm_tool" start --root "$vm_root"
python3 "$vm_tool" stop --root "$vm_root"
```

`start` refuses an occupied SSH or Docker loopback port, an unsafe/conflicting
VM inventory or an unrelated recorded PID. It uses the ports in `vm.json`.
`stop` asks this VM to shut down, then uses its own verified QMP connection if
shutdown exceeds the bounded grace period. It does not delete disks or keys.

To repeat qualification, stop other temporary local Docker acceptance fixtures
or pass a fixed original-container baseline:

```sh
python3 "$vm_tool" qualify --root "$vm_root" \
[Private development path omitted from this public documentation copy.]
[Private development path omitted from this public documentation copy.]
```

Qualification uses the explicit Windows `docker.exe --context desktop-linux`
only to inspect original containers and export a local image. A separate private
Linux client drives only the TLS daemon. Docker Desktop's Ubuntu integration
remains disabled. Static client 28.4.0 was downloaded over verified HTTPS from
Docker's official endpoint into the private VM root, and its hash is recorded.
It is a test client, not an automatically maintained system installation.

## Create another explicitly scoped pilot

The host needs `qemu-system-x86`, `qemu-utils`, `cloud-image-utils`, OpenSSL, SSH
and read/write access to `/dev/kvm` for the unprivileged user. These QEMU/cloud
tools were installed in Ubuntu-24.04; `noah` was added to its `kvm` group. No
Hyper-V feature enablement, Windows reboot or Docker Desktop restart was needed.

First configure the account-wide capacity plan below. A different `--parent`
does not select a different pool or bypass this requirement.

```sh
python3 "$vm_tool" create --organisation-id '<new trust-domain UUID>' --docker-port 0
python3 "$vm_tool" install-cli --root '<new private VM root>' --docker-cli-version 28.4.0
```

Creation downloads Canonical's Ubuntu Noble cloud image over verified HTTPS and
checks it against the published SHA256SUMS before creating the writable disk.
The image URL and exact hash are recorded. This is a checksum verification,
not a claimed GPG-signature verification. It creates fresh SSH/TLS identities;
all private material and disk images remain outside the repository under an
owner-only directory. Do not copy `user-data`, seed images or keys into Git.

`--docker-port 0` selects an available loopback host port; an explicit port must
be in `1024..65535`, for example `--docker-port 2476`. SSH receives a different
available host port. The guest Docker daemon still uses port 2376. QEMU forwarding,
TLS requests, Docker CLI selection and provisioner selection all use the recorded
host port. The retained version 1 pilot continues using `tcp://localhost:2376`.

Before download or bootstrap, creation takes a short POSIX inventory lock and
atomically publishes an owner-only `0600` `vm.json` in the private VM directory.
This early record reserves both host ports, including while `bootstrap_state`
is `preparing` or the completed VM is stopped. Another creation cannot reuse
either port. UUIDs, owner, permissions, endpoints and conflicts are checked for
every VM entry; unsafe or incomplete records block allocation rather than being
ignored. Bootstrap failure retains the reservation for operator review. An
unfinished VM cannot start. Ports stay reserved until its directory is explicitly
reconciled by the operator. Deleting a directory does not free its capacity or
permit a new allocation. Do not edit the recorded ports to move a retained VM.
The capacity ledger also prevents reuse across different registered parents.

New metadata also freezes an `operator_state_id` UUID for the provisioner's
private state binding. For the retained version 1 record, the controller derives
a stable UUID from its existing VM identity without rewriting the private file.
Loading and stopping that pilot remain compatible; starting requires explicit
capacity adoption. If its previously
qualified `shared/executor-host.json` lacks the new state identity, repeating
qualification reaches the provisioner's existing
`executor_host_migration_requires_review` gate. The controller does not rewrite
or automatically adopt that older operator state.

These port/inventory tests run as an unprivileged Linux user and use only their
own temporary directories, child lock contenders and real loopback sockets.
QEMU calls are mocked; no VM, Docker workload or download is started:

```sh
[Private development path omitted from this public documentation copy.]
PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v test_local_executor_vm.PortInventoryTests
```

## Retained parent capacity pool

The current controller uses one canonical pool for the Linux account, determined
from the OS account database at
`<account-home>/.local/share/cowork/executor-vm-capacity`. `HOME`, `XDG_DATA_HOME`
and `--parent` cannot choose another pool. There is no CLI pool override.
Its directory is owner-only `0700`; lock, identity anchor and atomically replaced
ledger are owner-only `0600`. A POSIX pool lock always precedes the short parent
port lock. A retained revision/hash identity anchor makes a missing or older
ledger fail closed; interrupted anchor/ledger writes require reconciliation.

The operator supplies a private `0600` JSON host plan. The following values are
an example, not measurements or recommendations for this computer. Both finite
ceilings must fit the current Linux `MemTotal`, CPU affinity and all visible
ancestor cgroup-v2 memory/CPU limits. `inventory_complete: true` is the operator's
explicit assertion that `existing_vm_roots` lists every retained VM created with
an earlier controller, including stopped and unfinished VMs across all parents.
An empty list is valid only when that inventory is actually empty.

```json
{
  "version": 1,
  "pool_id": "<fresh canonical UUID>",
  "cpu_millis": 16000,
  "memory_mib": 16384,
  "reserved_cpu_millis": 2000,
  "reserved_memory_mib": 2048,
  "qemu_overhead_cpu_millis": 250,
  "qemu_overhead_memory_mib": 256,
  "inventory_complete": true,
  "existing_vm_roots": []
}
```

`reserved_*` is the explicit reserve for Linux/WSL and other fleet workloads.
Each VM additionally retains its fixed 4000 CPU-millis, 4096 MiB guest allocation
and the plan's per-VM QEMU overhead. The VM profile remains 4 vCPU/4096 MiB.
This prevents allocation oversubscription within this account's declared plan;
it does not independently measure exclusive Windows physical coverage or hard
cap the QEMU emulator's actual overhead. Other Windows processes, Docker Desktop,
other Linux users and other WSL distributions need separate parent budgeting.
No Windows setting or `.wslconfig` is changed.

```sh
python3 "$vm_tool" configure-capacity --host-plan '<absolute private plan.json>'
python3 "$vm_tool" inspect-capacity
# Repeat only for each explicitly selected old v1/v2 root listed in the plan:
python3 "$vm_tool" adopt-capacity --root '<exact retained private VM root>'
python3 "$vm_tool" inspect-capacity
```

Declared old roots are charged immediately and block new starts/creation until
explicit adoption verifies their UUID, owner, permissions, organisation,
operator-state identity, profile and ports. Adoption reads and freezes their
binding in the pool without rewriting their metadata, keys or disks. For v1 the
existing stable derived operator-state UUID is retained. Newly encountered old
roots also require explicit adoption and enough remaining capacity. Adoption is
idempotent for the exact same binding. It does not qualify an old guest's manager
profile, provisioner state or physical-host attestation.

Capacity is persisted before the early VM metadata and before bootstrap.
Preparing, ready, stopped, failed and missing/unknown allocations remain fully
charged. There is no timeout release. The explicit retirement workflow below is
the only supported way to release a newly registered v2 allocation.
Missing roots, changed immutable bindings, omitted inventory and unsafe files
block creation/start for operator reconciliation. `stop` remains available for
safely shutting down a retained VM without adoption and never changes the ledger.
Every start rechecks the current Linux ceiling; a reduced ceiling blocks a new
start without freeing any allocation. It does not automatically stop running VMs.

`inspect-capacity` reports the current revision, held totals, unresolved adoption
and a stable refusal reason. To change finite ceilings/reserves, supply an updated
private plan with `--expected-capacity-revision <reported revision>`. The pool
identity, complete initial inventory declaration and per-VM overhead are immutable;
the new plan must still cover every held allocation and fit the Linux ceiling.
Do not delete or edit pool files to forget capacity. Lost/corrupt state requires
explicit operator reconciliation, which this controller does not automate.

These tests use only own temporary pools, inventories, fake kernel/cgroup files,
real loopback sockets and two real competing POSIX processes across two parents.
QEMU and downloads are mocked, and no real user pool is configured:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v \
  test_local_executor_vm.PortInventoryTests test_capacity_pool.CapacityPoolTests
```

Do not describe this mechanism as already managing a fleet of VMs. The existing
provisioner transport still permits only its documented Compose/container policy;
this pilot does not bypass the `minimum_isolation=virtual_machine` backend gate.

## Explicit retirement of a new owned VM

`retire` permanently deletes the selected VM's disk, keys, logs and shared state.
It is a separate explicit operation; normal `stop`, a failed bootstrap, a missing
root or a deadline never releases capacity. Only a new v2 allocation originally
registered by this controller is eligible. Adopted roots, v1 roots and every root
declared in the plan's initial inventory are excluded. The retained pilot is
therefore excluded even when stopped.

Read the exact VM UUID and SHA256 of its private `vm.json`, and the current pool
revision. Then select this irreversible action for that exact VM:

```sh
python3 "$vm_tool" retire --root '<exact own new VM root>' \
  --expected-vm-id '<exact registered UUID>' \
  --expected-metadata-sha256 '<unchanged vm.json SHA256>' \
  --expected-capacity-revision '<current pool revision>'
```

The command holds the canonical pool lock before the parent lock. It verifies
the registered organisation, operator state, profile and path; the exact metadata
hash; owner-private directories; regular single-link files; and the root/parent
device and inode. Symlinks, writable or foreign entries, extra sockets, mounts,
unknown inventory and a changed binding refuse retirement. A live recorded PID
(including a reused unrelated PID) or a listening QMP socket also refuses it.
It sends no QMP state-changing command and does not stop a running VM.

The process check reads UID/status, descriptors, working directories and memory
maps from Linux `/proc`, and checks both paths and frozen device/inode identities.
An open or mapped disk remains detectable after a rename. Every process carrying
this account's UID must be readable; missing or inaccessible evidence fails
closed. It reads no command lines, environments, private key contents or complete
logs. Privileged OS administrators and uncoordinated hostile programs running
under the same operator account are outside this controller's trust boundary.
Controller start/creation operations cooperate through the same locks.

Before removing anything, an owner-private pending record freezes the full
allocation, metadata hash, exact entry identities and a unique quarantine name.
The full CPU/RAM charge remains in the ledger. The exact directory is renamed
within its checked parent to `.retire-<retirement UUID>` and the parent is fsynced.
Only frozen entries are removed using checked directory descriptors; each parent
is fsynced. Each actual unlink/rmdir obtains an external, append-only deletion
witness while its exact entry descriptor is still open: `fstat` must report the
frozen device/inode/mode and a link count of zero after the parent fsync. These
witnesses live in the private canonical pool, outside the removed VM directory.
New or changed entries refuse further deletion. The original name
must stay absent. After the quarantined root is absent, the checked parent is
fsynced again and process evidence is rechecked. A matching positive root witness
and every entry witness are required; missing paths alone prove nothing. An atomically published,
append-only retirement tombstone then records the deletion proof and the ledger
releases exactly that allocation. VM ID, root and operator-state ID cannot be
reused through reservation or adoption.

An interruption keeps the allocation held. Invoke the same command with the
original VM UUID/hash and the current pool revision to resume; it verifies only
the remaining frozen entries and accepts a removed entry only with its durable
positive witness, even when `vm.json` was already removed. A moved directory or
file cannot be reported as deletion. If a crash occurs after unlink/rmdir but
before its witness is durable, the charge remains held with
`retirement_delete_proof_missing_unknown`; this uncertain window cannot
automatically release capacity. A v2 ledger
stores prepared/deleting/retired states. A retirement-only write-ahead record
contains the exact old/new ledger states before advancing the revision/hash
anchor. Ordinary pool access refuses an unfinished record. Only explicit
`retire` can recover it, and only with the exact announced next revision:
`capacity_retirement_recovery_required:revision:<number>`. Missing anchors,
unrelated state changes or a root that reappears refuse recovery. Other
interrupted pool writes retain the existing fail-closed behavior; there is no
general reconciliation, release, reassignment or metadata-rebind command.

The pure tests passed with temporary pools and a readable process fixture backed
by the real test process. They do not qualify deletion of an installed VM. In
this WSL account, two other own-UID processes denied descriptor inspection during
the source checks. Production retirement therefore remains blocked with
`retirement_process_inventory_unknown` until complete OS-readable evidence is
available or the explicit pinned read-only auditor below is selected.
The controller does not silently exempt those processes.

The optional `--retirement-audit-profile <private 0600 JSON>` selects the standalone
`vm_retirement_auditor.py` reader. It leaves the controller unprivileged. The
profile is explicit trusted launcher input, with this exact shape:

```json
{
  "version": 1,
  "mode": "wsl_root_read_only_v1",
  "auditor_sha256": "<reviewed immutable auditor-file SHA256>",
  "wsl_executable": "/mnt/c/Windows/System32/wsl.exe",
  "wsl_executable_sha256": "<trusted System32 executable SHA256>",
  "distribution": "Ubuntu-24.04",
  "allowed_binding": {
    "vm_id": "<exact new VM UUID>",
    "root": "<exact canonical VM root>",
    "metadata_sha256": "<unchanged vm.json SHA256>",
    "operator_uid": 1000,
    "pool_id": "<existing canonical pool UUID>",
    "initial_revision": 2,
    "boot_id": "<current Linux kernel boot UUID>"
  }
}
```

The profile UUID/hash/revision must match the explicit retirement arguments; its
UID must match the unprivileged controller. For a resumed journal, the trusted
launcher must supply the exact new expected revision. This is not a default
privilege escalation or a general executable/command option. The canonical WSL
executable is pinned by hash and rechecked before each call. The already-read,
hash-checked reader bytes are passed as argv to root's `/usr/bin/python3 -I -S -B
-c`; no operator module is imported and no Python cache is written. Structured
bounded input travels over stdin. Root returns a direct child-pipe verdict,
never a user-selected receipt file.

The reader independently checks canonical pool/anchor/journal metadata, the
selected allocation and actual root/parent/entry identities, and reads descriptor,
map, cwd and mount evidence for **all** process UIDs. It uses the explicit
operator UID to verify filesystem ownership. It never exempts an unknown PID,
reads command lines/environments/keys/log contents, writes filesystem data or
sends signals/QMP commands. Its metadata streams and the controller's frozen
root/entry descriptors close before process inspection. Each critical phase and
journal recovery requires a new nonce, exact request/ledger/record binding,
same kernel boot ID and bounded monotonic freshness; a timeout, wrong identity
or unreadable evidence refuses the action. A final release still needs every
positive deletion witness, even when root audit observes both paths absent.
This optional adapter has pure protocol/metadata tests; it has not yet qualified
an actual privileged audit or retirement of the installed fixture.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v test_vm_retirement.RetirementTests
PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v test_vm_retirement_auditor.ReadOnlyAuditorTests
PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest discover -v
```

These tests use own temporary roots, real test-process descriptors and UNIX
sockets, simulated crash points and readable kernel fixtures. They cover exact
ID/hash/revision, adopted exclusion, open/mapped disks, active/reused PIDs,
foreign/reappearing roots, symlinks/hardlinks, lost anchors, journal crash recovery,
partial deletion, parent fsync, immutable tombstones, duplicate requests and
charge totals after proven removal. No actual VM, Docker service or real pool
is changed by the tests or by this source preparation.

## Integration limits

The installed tenants still use Docker Desktop. The pilot uses a fresh test
organisation identity. Host-aware manager dispatch, full tenant control/data
routing onto the guest, production profiles, disk/workspace quotas, certificate
rotation and remote backup/update/enrollment remain unqualified. Local lifecycle
acceptance and this VM transport acceptance are separate evidence. The remote
transport continues refusing its unsupported lifecycle actions.

The pilot's certificates expire after 90 days. A stopped pilot consumes disk
space and retains its host-port and declared RAM/CPU allocation reservations,
although its guest is not running.
No autostart or recurring automation was set.

Implementation references: [Ubuntu cloud-init/QEMU](https://docs.cloud-init.io/en/24.1/tutorial/qemu.html),
[Docker daemon TLS](https://docs.docker.com/engine/security/protect-access/) and
[Docker binary installation for testing](https://docs.docker.com/engine/install/binaries/).
