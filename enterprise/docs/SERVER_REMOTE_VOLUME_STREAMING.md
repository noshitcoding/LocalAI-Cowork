# Remote volume streaming qualification integration

`remote_volume_streaming.py` is integrated into the actual `InstanceBackups`,
`InstanceUpdates` and `InstanceRollbacks` paths through a private, fixture-only
qualification opt-in. Remote backup, restore, upgrade and rollback remain refused
by default; enrollment is always refused. Source integration prepares a real
matrix and does not establish production or VM lifecycle qualification.

The transport uses a digest-pinned trusted Python helper on the bound mTLS
Docker daemon. It receives one exact named workspace volume, no Docker socket,
network, host directory, shared operator state, private key or staging mount.
Capture mounts the source readonly; restore writes only a new candidate-generation
volume. Native bounded pipes carry the archive, so the guest never needs access
to the operator's age key or plaintext backup directory.

The bounded transport option alone cannot open a lifecycle gate:

```json
{"remote_volume_streaming_preparation":{"version":1,"max_archive_bytes":268435456,"max_file_bytes":67108864,"max_files":10000,"timeout_seconds":120}}
```

CPU/RAM overhead must already fit the host's reserved capacity. Each helper has
0.5 CPU, 256 MiB RAM and swap, 64 PIDs, a readonly root, no network, all ordinary
capabilities dropped and no new privileges. Capture adds only DAC_OVERRIDE to
read the stopped sandbox volume; restore additionally adds CHOWN/FOWNER to map
files back to sandbox UID/GID 10001. These are trusted operator helpers, not
tenant workload containers. Their exact recorded ID, UUID name, labels, image,
caps and sole volume mount are verified before attachment and removal.

Archives have finite byte, file-count, per-file and monotonic-time bounds. The
controller verifies expected bytes/SHA256 and archive members before creating a
restore volume. Paths must be canonical and relative; duplicates, links, special
files and path traversal are rejected. Regular data, directories and ordinary
permission bits are preserved; privileged mode bits and arbitrary ownership are
not restored. Workspaces containing unsupported links or special files fail
closed. Received capture bytes and transmitted restore bytes are hashed.

A missing source volume is inspected and refused before any helper or staging
file is created; Docker cannot silently create an empty source. A restore refuses
an existing target and checks the new volume's operator labels, including a
concurrent create race. Failure kills only its exact verified helper and retains
any partial new target for explicit recovery. No source volume, active-generation
pointer, manager allocation or execution reservation is deleted or released.
Source and candidate epochs and the immutable host/organisation/operation binding
are checked before transfer. All instance containers must already be stopped.

## Private fixture scope

A fresh operator root may configure `remote_lifecycle_qualification` with exact
keys `version:1`, `mode:"fixture_only"`, a fresh UUID `fixture_id`, exact
`instance_id` and `executor_host_binding`, `max_workspace_archives` (1–1000) and
`max_workspace_archive_bytes` (1 MiB–8 GiB). Each stream archive ceiling must fit
the total ceiling. Host55 attestation, immutable guest runtime endpoints,
enforced AppArmor and central execution epochs are required. The private marker
cannot be removed, retargeted or attached to existing instance inventory. Every
operation retains its frozen host, allocation and source/candidate epoch envelope.

The single-instance guest fixture refuses every unknown daemon container and
requires all runtime containers and DB connections stopped. Retained services
must have exact singleton names, Compose labels and manifest images. Fresh Host55
coverage is required before starting an advanced operation. Uncommitted local
staging is reported as unknown until manager completion and a fresh applied
attestation. All static allocations remain held through stop, failure, timeout
and staging. Completion and explicit stop/delete recovery remain possible.

Native pg_dump, pg_restore and fencing use the private loopback TLS operator
endpoint with the original instance role/password/database. They do not elevate
to the operator role. The CA is mapped from its guest mount to the owned public
controller file; ambient PG overrides are discarded. Admin mc commands require a
private HTTPS loopback alias outside shared state and discard ambient alias,
insecure and CA overrides. Plaintext backups, age identities, DB admin keys and
TLS private keys remain outside the guest share.

Runtime listeners must allow exactly the newly generated DB roles and S3 buckets
before a staged API/worker starts. The Source54 minimum fixture's source-only
proxies are insufficient. The private API update probe removes published ports
and retains existing network isolation. Its signed runner probe uses a fresh UUID
and is a package/executor check, not proof of manager admission.

## Preserved contract and remaining real matrix

The integration retains the existing age encryption, outer backup manifest,
workspace suffix inventory, DB snapshot/quiescence, S3 payload, current-policy
checks and tamper-before-activation rules. Every inner workspace tar is checked
before generation creation or DB/volume mutation, in addition to outer hash/byte
verification. Remote archives bind the exact host and fixture UUID. Restore stages
a fresh generation and applies auth/session/run/schedule fences. Update and
rollback retain their exact source image, generation, schedule and
receipt contracts. Host55 coverage must reflect transitions as unknown until
manager completion, and static allocations remain held throughout. A failed
stream is not permission to replay an uncertain restore or release a charge.

Required real tests include legacy and bound chat volumes, isolated chat suffixes,
empty workspaces, byte limits, corruption, timeout/half-close, daemon/TLS/host
changes, missing sources, pre-existing targets, interrupted helper recovery,
source immutability, candidate activation and complete backup/restore/update/
rollback/deletion cycles. Current tests run the fixed helper and bounded pipes on
fresh local filesystems and reject a correctly hashed outer backup containing an
unsafe inner workspace through the actual restore implementation before probe,
generation, DB or activation. They do not qualify Docker or a VM.

```text
python -m unittest discover -s server/cowork-provisioner -p test_remote_volume_streaming.py -v
python -m unittest discover -s server/cowork-provisioner -p test_remote_lifecycle_qualification.py -v
```
