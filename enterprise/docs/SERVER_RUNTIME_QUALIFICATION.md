# Server candidate qualification — 2026-10-09

The V1 server refactor and a local executor VM pilot are implemented. The final
candidate passed the isolated checks below. All 39 installed containers retained
their identities, images and running state; no installed tenant was migrated.

## Results

| Check | Result |
| --- | --- |
| Rust server / runner / contracts | 95 / 13 / 13 passed (121 total) |
| Clippy, all targets, warnings denied | Passed |
| Provisioner on Windows | 135 passed; 2 platform skips (137 collected) |
| Those two POSIX journal-lock tests on Ubuntu | Both passed |
| Fresh PostgreSQL 16 and API/worker process fixture | 15 checks passed |
| PostgreSQL notification, ownership, leases and desktop tests | 10 passed |
| Real Docker workspace / terminal / desktop fixture | 12 checks passed |
| Fresh enterprise lifecycle with real workspaces | 22 checks passed |
| Separate local KVM executor VM | 11 checks passed; VM stopped afterwards |
| Compose limits and Helm singleton/resource schema | Positive and refusal cases passed |
| Temporary lifecycle/workspace resources | Removed; existing 39 containers preserved |

The normal Rust run ignores eleven integration tests. Ten PostgreSQL tests then
passed against a disposable database; the external-S3 test remains unqualified.
Claims, external recovery and renewals mint expiry using database time. The
worker anchors local deadlines to the monotonic request start, so application
clock skew and response delays cannot extend execution authority. Regression
tests cover delayed initial claims, hung renewals and synthetic clock offsets.

Operational tests cover migration 52, real API/worker processes and loop health,
ownership loss/replacement, authenticated sync-stream revocation, shutdown,
request IDs and metrics. Docker tests cover persistent workspace reuse, chat
isolation, serialization across workers, frozen bindings, terminal and desktop.

The lifecycle fixture creates its own fleet, admin/TOTP, organisation, tenant,
PostgreSQL and MinIO. It exercises encrypted backup, tamper refusal before
activation, restoration into fresh volumes, credential rotation and old-token
refusal, real post-restore chat execution, schedule pausing, canary, promotion,
rollback and deletion. The transition changes the immutable package while
retaining the same binary; it does not certify upgrades between software versions.
Its two private fixture overrides enable chat workspaces and keep tenant egress
internal. No installed feature gate is activated by this test.

The VM pilot runs on this Windows computer inside Ubuntu WSL with unprivileged
QEMU/KVM, 4 vCPU, 4 GiB RAM and its own Linux kernel/Docker daemon. Real mutual
TLS, daemon and organisation binding, read-only state-share proof, rejection
without a client certificate, and a bounded sandbox job passed. The VM is stopped;
its private disk and keys remain outside Git. See
[executor-vm/README.md](../deploy/executor-vm/README.md) for reproduction and limits.

## Candidate identities

These are immutable local `docker inspect .Id` values. Registry publication is
not certified; retained local references are recorded separately in the reports.

- Server: `sha256:edb08fb843da235eab25c416b7cb5a4a7db3c258fced5426eb1102a116d75bdd`
- Runner: `sha256:0d69c9387daf14eaefdc6d798a54e6b98e22abce1898a4ecb3eba3d2c3a8cbe7`
- Provisioner: `sha256:09bdffb03e96bee9e8ab0593f2d8b5a407d55f5c699ce6c4e72a7d93aa496e68`
- Native Linux binary SHA-256: `383e6ae5af9e478d6e4d89ed4bbe8e638db37f38c9c0c10a77859d16e7742239`

Build inputs match the frozen source copy outside Nextcloud. Only staging
Dockerfiles bound compilation parallelism; repository Dockerfiles are unchanged.
The manifest records both sets of Dockerfile hashes and the actual copied runtime
inputs. Acceptance scripts are hashed separately from provisioner image contents.

## Evidence and reproduction

Evidence under `.internal/server-refactor-2026-10-08/`:

- `qualification-manifest.json`: source/report hashes, exact identities, counts,
  bounded before/after evidence and explicit limits. Missing baseline hashes do
  not establish that this task created a file.
- `native-qualification-final-db-clock.json`: isolated PostgreSQL and native-process results.
- `docker-workspace-db-clock/.internal/enterprise-local/chat-workspaces-docker-results.json`: final-image workspace checks and cleanup proof.
- `lifecycle-qualification-final.json`: final-image lifecycle, runtime snapshots and cleanup proof.
- `executor-vm-qualification.json`: separate-kernel executor pilot and stopped-state proof.
- `rust-validation-db-clock.json` and validation logs: final source tests/build.

```sh
cargo test --quiet --locked -p cowork-server -p cowork-runner -p cowork-contracts -j 4
cargo clippy --quiet --locked -p cowork-server -p cowork-runner -p cowork-contracts --all-targets -j 4 -- -D warnings
cargo build --quiet --locked -p cowork-server -j 4
```

`server/cowork-server/tests/qualify_runtime.py` creates its own temporary database
and ports. `app/scripts/test-chat-workspaces.mjs` uses the candidate IDs above and
an isolated `COWORK_WORKSPACE_TEST_ROOT`. The new lifecycle harness is
`server/cowork-provisioner/qualify_candidate_lifecycle.py`; pass exact local IDs
using `--server-image-id`, `--runner-image-id` and `--provisioner-image-id`. It
creates private UUID-labelled resources and uses no installed credentials.

## Remaining rollout and integration work

The installed fleet remains on Docker Desktop. The VM pilot does not qualify
hostile tenants, remote lifecycle operations, automatic certificate rotation,
disk quotas or manager dispatch across hosts. Remote backup/update/enrollment
therefore remain gated. Production S3, Kubernetes, failover, offline devices and
40-task load require their own acceptance. Multiple API replicas still require
distributed command/stream routing; the ownership guard permits one instance API.

The other app chat was informed of source boundaries and candidate results. Its
LocalAI-Cowork-V2 repository is separate. This task made no shared app, contracts,
runtime or Cargo manifest edits; the V2 adapter follows its stable interfaces.
