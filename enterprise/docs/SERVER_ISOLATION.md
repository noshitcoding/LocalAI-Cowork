# Server resource bounds and executor isolation

This change prepares deployment configuration and provisioner safeguards. It does
not recreate services, reduce a running instance's resources, move volumes,
upgrade an installed image, or qualify a real remote host.

## Resource accounting

An instance policy's `cpu_millis` and `memory_mib` are the aggregate admitted
sandbox ceilings enforced by its runner. They are **not the whole instance's
physical allocation**. API, worker, runner and egress proxy consume resources too.
The worker previously duplicated the sandbox policy; new instance manifests give
it an independent operator-owned service cap.

| Per-instance service | CPU, millicores | Memory, MiB | PID ceiling |
| --- | ---: | ---: | ---: |
| API | 1000 | 512 | 256 |
| Worker | 500 | 512 | 256 |
| Runner | 500 | 256 | 128 |
| Egress proxy | 250 | 128 | 64 |
| Service subtotal | 2250 | 1408 | 704 |

For a sandbox policy of 4000 millicores and 8192 MiB, the instance maximum is
**6250 millicores and 9600 MiB**. Sandbox memory already includes its own tmpfs
within each container's cgroup. PID caps above apply to services; they are not a
fleet-wide sandbox PID guarantee. Sandbox task PID ceilings remain separately
enforced by the runner.

`instance_service_limits` can override any named service with all three integer
fields, within the bounds validated by `resource_budget.py`. Existing recorded
service caps stay pinned for that instance. Image replacement does not silently
change them. A control-resource migration requires a separately reviewed stopped
instance migration. Limits are maxima rather than host scheduling reservations.

`resource-budget.json` records the exact service and sandbox ceilings and their
sum. The record is persisted before the Compose manifest/start. Stopped instances
and a crash between recording and writing the manifest retain their reservation.
Only verified permanent deletion removes it. A tombstone with retained allocation
evidence, an extra service/replica, unbounded fields, unknown generations, a legacy
manifest without allocation evidence, or changed manifests block capacity admission
and require operator review. Existing installations are not automatically imported.

Host allocation is an **optional, default-disabled** operator gate:

```json
{
  "fleet_host_budget": {
    "cpu_millis": 16000,
    "memory_mib": 32768,
    "reserved_cpu_millis": 2000,
    "reserved_memory_mib": 4096
  }
}
```

The provisioner reserves every retained instance maximum, adds the platform
reserve and rejects a sum above either host ceiling. Once a host budget has been
persisted, removing its configuration is rejected. Budget configuration and its
evidence remain private operator state; they contain no credentials. For Docker
Desktop's local entrypoint, set **all four** `COWORK_FLEET_HOST_CPU_MILLIS`,
`COWORK_FLEET_HOST_MEMORY_MIB`, `COWORK_FLEET_RESERVED_CPU_MILLIS` and
`COWORK_FLEET_RESERVED_MEMORY_MIB`, or leave all unset. Enabling it over legacy
instances requires reviewing their stopped manifests and recording verified
allocations first; no automatic live cutover exists.

The guarantee requires one provisioner, one exclusive state inventory and its
existing operator lock **per daemon**. Two operators with different state roots on
the same daemon cannot coordinate allocations. Every unrelated workload, database,
storage service, gateway, operating-system service and maintenance helper must fit
inside the operator's reserve. Workloads started outside this operator, Docker
manual scaling and host administration remain trusted operations. This is not a
global Docker scheduling or storage-quota system. Disk/storage capacity continues
to require an external filesystem/object-storage quota; `storage_gib` is a policy
value, not proof of a Docker volume quota.

All services in the standalone, enterprise management, enterprise local and
executor operator Compose profiles now have finite CPU, memory and PID ceilings.
Validate their rendered configuration without starting containers:

```powershell
docker compose -f deploy/docker-compose.yml config --format json --no-interpolate | python deploy/validate_resources.py
docker compose -f deploy/enterprise/docker-compose.yml config --format json --no-interpolate | python deploy/validate_resources.py
docker compose -f deploy/enterprise/docker-compose.local.yml config --format json --no-interpolate | python deploy/validate_resources.py
docker compose -f deploy/enterprise/docker-compose.executor.yml config --format json --no-interpolate | python deploy/validate_resources.py
```

With defaults the enterprise local control/dependency services have a combined
ceiling of 11000 millicores and 5632 MiB. That sum can exceed a development
machine's CPU allocation: the local fixture is not a qualified globally reserved
deployment. Sandbox instance budgets are additional. The standalone profile's
service subtotal includes its short-lived image initialization services and
reports sandbox budgets separately.

The instance Helm chart now uses separate per-replica API/worker ceilings and a
namespace ResourceQuota equal to their total. The instance API requires exactly
one replica; the chart refuses horizontal API scaling. With one replica of each,
the default service total is 1500 millicores and 1024 MiB. Its external runner and
sandbox budget are additional; this chart does not qualify their placement or
resource enforcement. Kubernetes PID limits require operator/kubelet configuration.
Compose host allocation is not a Kubernetes cluster admission guarantee.

## Shared Docker daemon limits

The runner and provisioner are trusted daemon administrators. A mounted Docker
socket can create privileged containers, mount host filesystems and access other
containers' secrets. Read-only mounting of the socket does not reduce API
privileges. A simple endpoint allowlist/socket proxy does not by itself make
Docker's create/mount/exec API safe for mutually hostile tenants.

Separate containers, per-instance networks, AppArmor/seccomp and restricted
service credentials reduce accidental cross-instance access. They do not make a
shared host kernel/daemon a strong hostile-tenant boundary. Virtual-machine
isolation requests still fail closed; they are never downgraded to containers.
An organisation/trust domain requiring stronger separation needs a dedicated
host or VM and a qualified isolation stack. Management and its master secrets
should stay on a different host from execution.

## Dedicated executor transport candidate

`deploy/enterprise/docker-compose.executor.yml` is an opt-in operator transport
profile. It does **not** provision remote infrastructure. The provisioner runs on
the trusted control/operator host without a local Docker socket. It contacts one
dedicated executor daemon using mutual TLS. The entire instance Compose project
(API, worker, runner, proxy and sandboxes) lives on that executor. Its runner uses
that executor's local socket. Docker networks never span hosts implicitly.

An operator-owned JSON config passed to `provisioner.py --config` adds:

```json
{
  "backend": "compose",
  "state_directory": "/srv/cowork-executor/instances",
  "executor_host": {
    "id": "11111111-1111-4111-8111-111111111111",
    "trust_domain": "22222222-2222-4222-8222-222222222222",
    "endpoint": "tcp://executor.example.invalid:2376",
    "daemon_id": "REPLACE_WITH_OPERATOR_VERIFIED_DOCKER_DAEMON_ID",
    "ca_file": "/run/secrets/executor_tls_ca",
    "cert_file": "/run/secrets/executor_tls_certificate",
    "key_file": "/run/secrets/executor_tls_key"
  },
  "fleet_host_budget": {
    "cpu_millis": 16000,
    "memory_mib": 32768,
    "reserved_cpu_millis": 2000,
    "reserved_memory_mib": 4096
  }
}
```

This is a configuration fragment, not a runnable complete configuration. The
usual pinned images, HTTPS management endpoint, private provisioner token,
database/object-store endpoints and administration files, backup root, private
`mc` alias/config, external executor services/routing networks and image security
profile are still required. Create the remote daemon's operator labels
`cowork.role=executor` and `cowork.trust-domain=<organisation UUID>`. Only one role
and trust-domain label is accepted. One trust domain corresponds to one organisation
in this candidate. Host CPU/RAM declarations must fit the daemon's reported capacity.

Every Docker command receives explicit `--host`, `--tlsverify`, CA, client
certificate and key arguments. The daemon ID and trust-domain labels are verified
again before Docker operations. Ambient `DOCKER_HOST`, context and TLS variables
cannot retarget lifecycle commands. Local operation also selects the local Unix
socket explicitly. There is no unencrypted TCP or local-daemon fallback.
TLS files must be regular nonsymlink operator-owned files; group/world writes are
rejected and the client key requires `0600` on Linux. The profile's secret files
must therefore have ownership matching the provisioner (root in the supplied image).
Client certificates grant daemon administration and remain trusted secrets.

Docker resolves bind sources on the daemon host. Both hosts must therefore have
the **same operator-controlled private filesystem at the same absolute path**
`/srv/cowork-executor`. Before remote Compose writes/starts, a bounded, networkless
readonly helper must read a fresh exact nonce from that filesystem. Without this
proof, mounting runtime secrets/configs is refused. Secure and qualify that
filesystem's transport, ownership and export scope; a writable general-purpose
tenant share is unsuitable. The provisioner TLS key and management master token
remain in operator secrets, outside the shared instance tree.

The global and per-instance `executor-host.json` binding outlives operation
journals and permanent deletion. Changing/removing host selection, reusing local
state on a remote host, and assigning another organisation are rejected. Recovery,
stop and deletion inventory commands use the same pinned daemon, never the local
one. Provision/start/resume/stop/suspend/delete are the candidate's allowed actions.
Remote **backup, restore, upgrade, rollback and execution enrollment are explicitly
refused**: their staging/release/migration contracts have not been qualified for
this transport. Operators must not bypass this gate by removing configuration.

The private manager host registry assigns each hosted instance immutably to a
host and trust domain. Revocable scoped credentials fence claims, pending work,
completion, reconciliation and host release inventory. The legacy credential
can claim only explicitly local/NULL-host instances. Journals and receipts retain
the same host binding; no unscoped remote fallback exists. The opt-in Host55
capacity client observes the full selected daemon and renews the manager's
DB-clock coverage without releasing permanent static charges. See
[SERVER_HOST_CAPACITY_CLIENT.md](SERVER_HOST_CAPACITY_CLIENT.md) for its exact
configuration and qualification boundary.

Production qualification still needs real dedicated hosts/VMs, verified daemon
TLS certificates with the exact endpoint DNS/IP identity, secure shared state,
firewall-restricted management and execution networks, encrypted/trusted
database/S3/control routes, operator-owned external ingress, AppArmor/seccomp
installation, image qualification, disk quotas, runtime stop/delete/crash tests,
certificate renewal and lifecycle backup/update qualification. Python mocks and
Compose rendering prove configuration/command binding;
they do not prove physical isolation or these infrastructure properties.

Docker's [daemon TLS documentation](https://docs.docker.com/engine/security/protect-access/)
describes daemon administrator certificates and verified TLS.
Its [bind-mount documentation](https://docs.docker.com/engine/storage/bind-mounts/)
explains that source paths are resolved on the daemon host.

## Qualified local VM pilot

An unprivileged QEMU/KVM pilot on the current Windows computer passed eleven
real checks with its own guest kernel, verified mTLS daemon, exact state nonce,
read-only state export and a bounded sandbox job. It is stopped after testing;
the installed tenant fleet remains on Docker Desktop. See
[executor-vm/README.md](../deploy/executor-vm/README.md) and
[SERVER_RUNTIME_QUALIFICATION.md](SERVER_RUNTIME_QUALIFICATION.md) for evidence
and the remaining remote-lifecycle, routing and production limits.

## Validation

```powershell
python -m unittest discover -s server/cowork-provisioner -p 'test_*.py' -q
```

Resource tests cover exact totals, independent worker caps, CPU and RAM exhaustion,
platform reserves, stopped and crash-pending allocations, legacy manifests,
manifest tampering, extra services/replicas and retained deletion records. Executor
tests cover mTLS command selection, ambient context removal, changed daemon ID or
domain, durable host selection, rejection of legacy/local state, forbidden actions,
organisation/VM mismatch and exact shared-state nonce verification. Source Compose
validation additionally uses PyYAML when available; rendered-JSON validation uses
only the Python standard library.
