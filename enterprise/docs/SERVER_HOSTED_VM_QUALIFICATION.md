# Dedicated local VM qualification

`server/cowork-provisioner/qualify_hosted_vm.py` creates a disposable management
fixture and a **new** unprivileged KVM VM under Ubuntu-24.04. It does not enroll,
restart, migrate or reuse an installed fleet, account, operator directory or VM.
Without `--execute`, the command validates arguments and creates no resources.
The source is prepared; a real run is a separate qualification with centrally
selected candidate image IDs. Passing unit tests is not that real qualification.

## Exact scope

The intended cycle creates fresh PostgreSQL databases, MinIO, management server,
bootstrap admin, TOTP, organisation, revocable host credential and immutable
host assignment. It provisions the assigned instance, performs real stop/start,
uses a management launch ticket to log in to the guest API, completes a raw
sandbox run under guest AppArmor and central admission, uploads and downloads a
small encrypted S3 artifact, then stops and deletes that instance. The report
records actual local image IDs separately from the fixture registry's distribution
manifest digests. Containerd's image ID can identify an OCI index while classic
Docker reports the config digest for the same package. The adapter preserves
original OCI index/manifest/blob bytes, and the report records both engines'
actual IDs plus the exact config digest and rootfs identities. A classic Docker
save without an OCI index needs a generated schema-2 distribution manifest; that
is marked explicitly and does not change the config or rootfs package identity.

All UUID fixture resources are private and fresh. The authoritative original
container baseline must match the current running inventory exactly before work
begins, and every original ID/image must remain running afterwards. Additional
parallel Docker fixtures refuse execution. Cleanup checks daemon ID, UUID names,
labels and exact recorded IDs; it never deletes a resource by a broad prefix.
Success stops the owned VM and removes the fixture containers/networks/volumes.
The canonical parent capacity ledger keeps the VM's conservative charge. Its
owner-private metadata, disk and VM credentials remain under
`~/.local/share/cowork/qualification`; no capacity release API exists. The report
distinguishes removed active fixture resources from this stopped, charged VM.
Failure stops its owned VM and recorded
fixture containers, retaining only the private UUID fixture for investigation.
It does not guess at a failed instance's lifecycle or automatically delete
unproven tenant state.
Failure evidence is written atomically and durably to the requested report path
before and after cleanup attempts, outside the private fixture. It records the
staged harness/transport hashes and a typed action code without argv or stderr.
The Windows Docker client uses an environment frozen before the native Linux
operator installs its separate `DOCKER_CONFIG`; parent calls and TCP relays all
use that same frozen environment.
The fresh internal control network uses an explicit UUID-derived RFC1918 /24.
All existing Docker networks' IPv4 and IPv6 IPAM subnets are inspected before
selection; overlapping private ranges and malformed inventories fail closed.
The report records the selected subnet. Docker still rejects a conflicting
concurrent allocation; the harness never changes daemon address pools or prunes
existing networks to make room.

The `source54-minimum` profile does not qualify backup, restore, update or a
version migration. The separate `host55-lifecycle` profile exercises those paths
only through an immutable fixture-only runtime opt-in, a registered service plan
and fresh manager host-capacity attestations. Production remote lifecycle and
execution enrollment remain gated. Passing source tests opens no deployment gate.
The manager policy retains `minimum_isolation=container`: one physically separate
VM dedicated to the new organisation is observed here; it is not a claim that a
general VM backend or mutually hostile tenants sharing its Docker socket are
qualified.

## Transport and secret boundary

`guest_endpoints.py` is an optional typed profile for exactly one host binding and
one instance. It pins guest PostgreSQL, S3 and management endpoints and the public
CA fingerprint in operator state. Retargeting or disabling an existing profile
requires explicit review; it never reinterprets retained local state. API and
worker receive only a public CA certificate in `runtime-secrets`, with PostgreSQL
`sslmode=verify-full`, `COWORK_FLEET_CA_CERTIFICATE_FILE` and
`COWORK_S3_CA_CERTIFICATE_FILE`. A non-certificate PEM payload is refused.

The guest API publishes only on guest `127.0.0.1`; the harness reaches it through
a private SSH forward with its own pinned known-hosts file. Docker administration
uses mTLS on the new VM's reserved loopback port. The operator verifies exact
daemon identity, trust domain and the readonly shared-state nonce before Compose
mounts are accepted. Guest AppArmor must be loaded in enforcement mode, and the
actual sandbox reports that profile and the guest's kernel; the VM DMI UUID also
must match its new state descriptor.

Under WSL NAT there is no assumed connection to Windows loopback or a shared
Docker socket. `hosted_vm_transport.py` runs bounded loopback TLS listeners and
uses a fresh limited relay container in the private control network. Channels are
fixed `docker.exe exec` TCP relays to the four known fixture roles. Runtime manager
requests can use only the five instance-key-protected access/admission/content
paths; claim and admin paths receive 404. Runtime PostgreSQL checks the startup
database and user against the instance's UUID-derived role. Runtime S3 can address
only its UUID-derived bucket. The full profile additionally allows each exact
fresh generation's database/role/bucket after checking its claimed operation,
host, instance, organisation, allocation revision/plan and source/candidate epochs.
Administrative endpoints have separate listeners.
The readonly registry serves content-hashed blobs and genuine schema-2 manifests;
the guest trusts its fixture CA only under the new registry address.

Global manager keys, host operator tokens, PostgreSQL admin credentials, MinIO
admin credentials, TLS private keys and SSH private keys remain outside the
readonly 9p share. Only instance runtime secrets, generated manifests/configs and
the public CA enter it. The provisioner runs natively in Linux using files, Docker
CLI, Compose plugin and `mc` extracted and hashed from the exact provisioner
candidate. This mode is recorded explicitly; it is not a claim that an unchanged
container can reach WSL loopback. It uses a private `DOCKER_CONFIG` and does not
alter installed Docker contexts, WSL integration or retained VM state.

## Inputs and limits

Run the harness as the unprivileged Ubuntu-24.04 user with KVM access. It requires
QEMU, cloud-localds, OpenSSL, SSH, psql and prlimit, plus the explicit Windows
Docker executable and `desktop-linux` context. VM bootstrap downloads a new
Ubuntu cloud image, verifies its published SHA256 and installs Docker in that
guest. The baseline JSON is the original array of `{ID, Image}` records, with
12–64 hexadecimal ID prefixes or full image IDs. All nine image inputs below are
required exact local `docker image inspect .Id` values, never mutable tags or
registry-manifest digests.

```text
python3 server/cowork-provisioner/qualify_hosted_vm.py \
  --server-image-id sha256:<new-server-image-id> \
  --runner-image-id sha256:<new-runner-image-id> \
  --provisioner-image-id sha256:<new-provisioner-image-id> \
  --postgres-image-id sha256:<postgres-image-id> \
  --storage-image-id sha256:<minio-image-id> \
  --sandbox-core-image-id sha256:<core-image-id> \
  --sandbox-gui-image-id sha256:<gui-image-id> \
  --sandbox-crew-image-id sha256:<crew-image-id> \
  --egress-proxy-image-id sha256:<egress-image-id> \
  --schema-version 54 \
  --parent-capacity-plan-file /absolute/private/parent-capacity-plan.json \
  --expected-parent-capacity-revision 0 \
  --retained-pilot-root /home/noah/.local/share/cowork/executor-vms/cowork-executor-fcadc54ed8a6476682d747b9e90bfe73 \
  --retained-pilot-vm-id fcadc54e-d8a6-4766-82d7-47b9e90bfe73 \
  --retained-pilot-known-stopped \
  --parent-docker-client '/mnt/c/Program Files/Docker/Docker/resources/bin/docker.exe' \
  --parent-docker-context desktop-linux \
  --baseline-file /absolute/path/live-containers-before-qualification.json \
  --report /absolute/new/path/hosted-vm-qualification.json
```

Add `--execute` only for the authorized isolated real run. The candidate must
contain `guest_endpoints.py`, and the server must implement both private-CA
clients. The schema argument must match the actual frozen server revision;
source 54 qualification does not qualify later host-budget/HA migrations.

## Parent plan and full lifecycle

Before execution the operator explicitly reviews a private `0600` plan. The
controller validates it against real CPU, memory and cgroup bounds. The following
is a proposed allocation ceiling, not a measurement or an automatic reservation:

```json
{
  "version": 1,
  "pool_id": "<fresh canonical UUID, or the existing pool UUID>",
  "cpu_millis": 12000,
  "memory_mib": 14336,
  "reserved_cpu_millis": 2000,
  "reserved_memory_mib": 4096,
  "qemu_overhead_cpu_millis": 500,
  "qemu_overhead_memory_mib": 512,
  "inventory_complete": true,
  "existing_vm_roots": ["/home/noah/.local/share/cowork/executor-vms/cowork-executor-fcadc54ed8a6476682d747b9e90bfe73"]
}
```

This plan charges each retained VM for 4500 CPU millis and 4608 MiB, including
the stopped pilot. The harness adopts only the specifically authorized pilot's
validated `vm.json`, checking its SHA before and after. It reads no pilot private
key/log, starts no pilot and changes no pilot metadata. Configuration/adoption is
serialized through the canonical account pool and parent inventory locks. An
existing pool requires its exact current revision. Unknown or missing retained
roots fail closed; failed starts and stopped VMs do not release charges. This
account plan does not claim exclusive coverage of Windows or other WSL users.

For the full profile add these arguments to the command above, use new exact
target image IDs, replace `--schema-version` with the target's actual schema
(at least 56), and select a new report path in a separate host55 evidence folder:

```text
  --qualification-profile host55-lifecycle \
  --source-schema-version 54 \
  --source-server-image-id sha256:533de1b42f7101d9b17ff24fde2af1f67b3f61b2eab9276a585c2df3648df02f \
  --source-runner-image-id sha256:aad8b1b0b9dfe185bddb5c9fbaf8a02b54fc1a6641302c6cd0e2e0dd1d03c586
```

Full qualification also requires native age/age-keygen/pg_dump/pg_restore. The
source and target server ELF SHA256 must differ; actual local image IDs and
distribution package identities are recorded separately. Source API/worker use
the compatible source runner; promotion changes all three to the target package.
Schema52 lacks the private-CA runtime clients and cannot serve as this remote
source; its separate local cross-version matrix remains a different qualification.

The full cycle covers legacy volumes, persistent chat files/tool/messages,
separate chats, unfinished-run/session/credential/schedule fences, an actual
encrypted backup, tamper rejection before activation, post-backup mutations
excluded by a fresh restore, retained source files unchanged, private canary
rollback to the exact source, promotion with a real centrally admitted target
chat run, and rollback to the source schema/images. It checks committed epoch
and API/worker markers plus refusal of fresh reservations from each stale epoch.
Permanent tenant deletion verifies all generation databases, guest volumes and
the exact frozen manager host allocation's release. The parent VM remains
stopped and charged. The private chat flag hook affects API and worker across
generations and is recorded; health checks are unchanged.

The harness owns its QEMU `Popen` directly without daemonize. QMP readiness is
bounded and bound to that child's PID/private directory. Private stderr is drained
with a 64 KiB retention limit; reports include only byte counts, hash, truncation,
exit code and a fixed classification. SIGKILL is not called an OOM without proof.
Guest readiness fails immediately after child exit, and safe VM stop is followed
by exact-child reaping. The guest remains 4 GiB; QEMU's 8 GiB address-space limit
allows its mappings and is not a larger guest RAM allocation.

The new VM has 4 vCPUs, 4096 MiB RAM and a 20 GiB virtual disk. Its conservative
Docker allocation ceiling is 4000 CPU millis and 3584 MiB, including reserved
1000 CPU millis/1024 MiB. The single instance reserves 500 CPU millis/512 MiB for
its shared sandbox pool plus all four bounded services (2250 CPU millis/1408
MiB), for a total 2750 CPU millis/1920 MiB. This is the existing static allocation,
not an additional dynamic run charge. Each fresh control container has explicit
CPU, memory and PID limits; the relay has 0.5 CPU/256 MiB/192 PIDs. Native processes
have effective soft limits of 1536 MiB address space and 1800 CPU seconds, plus
512 descriptors and bounded file sizes. Their hard ceilings permit the dedicated
QEMU child to receive its own 8192 MiB address-space/5400 CPU-second limits, while
its guest's physical RAM remains 4096 MiB. A scoped `prlimit` wrapper applies those
limits only to the new QEMU process; other native children inherit the smaller
soft limits. These finite CPU times are not independent CPU-rate quotas. TLS listeners
limit simultaneous HTTP, registry and PostgreSQL channels and body sizes. The
harness requires at least 60 GiB free temporary storage before creating anything.

## Local verification without Docker or VM starts

```text
python -m unittest discover -s server/cowork-provisioner -p 'test_*.py' -q
wsl.exe -d Ubuntu-24.04 -- python3 -m unittest discover -s /mnt/c/<repo>/server/cowork-provisioner -p test_hosted_vm_transport.py -v
```

The narrow tests exercise endpoint/host/instance fences, immutable profile
binding, resource totals, public-only CA copying, genuine manifest/config digest
separation, image/layer tampering, runtime route authorization and refusal to
clean up original/unlabelled containers. A POSIX test also performs real ephemeral
TLS handshakes and checks the manager-route and PostgreSQL database fences using
local test sockets, with no Docker or VM resources.

The local 6 GiB QEMU address-space cap produced SIGABRT with an actual
`qemu_thread_create: Resource temporarily unavailable` error. The owning
cgroup had available PIDs and no OOM events. A separate bounded 8 GiB diagnostic
kept the same 4 GiB guest running for 150 seconds and stopped it cleanly, with
17 threads and a measured virtual peak of 5,776,276 KiB. That observation
justifies the test cap correction; it does not certify guest readiness or prove
a host OOM. Production capacity still charges guest RAM and its declared
overhead, and the native operator retains its 1536 MiB soft limit.
