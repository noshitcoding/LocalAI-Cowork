# Linux core qualification in Docker

`python tools/qualification/linux_container.py` provides a repeatable alternative when a
native Linux/WSL development shell is unavailable. It qualifies the core on a Linux filesystem;
it does not qualify a desktop WebView, native file picker, installer, gVisor or Kubernetes.

The runner creates `.local/linux-qualification/<unique-id>/context` from an explicit source
allowlist: Rust workspace files, crate/service sources, contract tooling and process tests.
It excludes build caches, repository history, daemon state, artifacts and credentials. It
records the SHA-256 of each source and the manifest. Inspect this context before adapting
the allowlist for additional components.

The image uses the repository's Rust 1.94.1 version and a pinned official Rust Bookworm image
digest. Image construction installs Python and Rust lint/format components and fetches the
locked crates. The resulting image ID is recorded; package repository resolution during
construction means the base digest alone is not a complete release reproducibility claim.

The test container runs as UID 10001 with all capabilities dropped, no new privileges,
no external network, no Docker socket and no host mounts. Limits are four CPUs, 6 GiB memory
and 512 processes. Cargo uses `--locked --offline`; the model providers used by process
tests listen only inside the test container. No real model or personal project is used.

Checks run in this order and stop at the first failure:

1. Rust format, workspace tests and Clippy with warnings denied.
2. Daemon build.
3. Durability, files, workspaces, file operations/restoration, uploads, native folders and
   model budget process tests against that exact daemon binary.

For a change confined to the new process supervisor, `--suite executor` runs workspace
format validation plus only that crate's actual child-process tests and Clippy. The report
records the selected suite and fixture executable hash. It does not imply a rerun of the
unchanged core, daemon process suites or native desktop.

`--suite environments` runs the private environment storage/lifecycle tests, including its
hard process-end test, both schema migration tests, and domain/storage Clippy. Qualification
reports in these tests are synthetic; this suite does not qualify an executor sandbox.

Host metadata, the source context and copied `evidence` directory remain under the unique
output directory. Per-step logs and `result.json` include command, duration and exit status;
the successful result also records the daemon hash. The specifically named stopped container
and image are retained for inspection. The runner does not prune Docker resources or touch
other projects' containers. A new run creates a new identifier and does not reuse prior test data.

PostgreSQL contracts run separately on a fresh disposable database. Native Linux desktop
and manual accessibility tests still use their own acceptance procedures. Current results
and any platform defects found here are recorded in [VERIFY.md](VERIFY.md).

The process-journal follow-up used a private compiled-dependency cache from the initial
qualification container, with a new allowlisted source snapshot and manifest. Its cache
image ID and layer ancestry are recorded in the new run's host metadata. All format,
test, lint, build and process checks were executed again offline with the same container
limits. The initial failed report remains separate. This cache-assisted local rerun is
not the default runner's cold build or a release reproducibility claim.

The MCP follow-up fetched newly locked schema-library dependencies during the image
build, then ran the actual qualification offline. Its final snapshot includes empty
SSE priming-event compatibility. The extra HTTP servers are synthetic loopback fixtures
inside the container; no personal MCP endpoint or credential is used. This adds protocol
coverage. The subsequent journal qualification adds durable admission, reconciliation
and a real worker crash after a remote file effect; OAuth, stdio, real MCP servers and
product-interface acceptance remain open. See MCP-JOURNAL.md and VERIFY.md.

The persistent-session follow-up adds encrypted checkpoint binding, an atomic connection
lifecycle and three actual worker-termination stages against a stateful synthetic HTTP
fixture. Its preceding Linux snapshot passed 173 Rust tests and the same seven daemon
process suites. It reuses the compiled dependency cache under the same isolation limits;
no external MCP account, credential or production endpoint is accessed. Native/server key
custody and product integration remain pending. See MCP-SESSIONS.md and VERIFY.md.

The native credential follow-up passes 180 regular Rust tests, the seven daemon
process suites and one additional actual Secret Service process test. Its isolated
image adds dbus-daemon and gnome-keyring during construction, then fetches the pinned
new dependencies. Qualification runs offline with the same UID, mount and capability
boundaries. `run_linux.py --native-secrets` invokes `native_secrets.py` through a
fresh dbus-run-session; a temporary HOME/XDG profile and generated test-only keyring
password keep this fixture separate from desktop accounts. Windows uses the real
machine-local Credential Manager in its separate native test. See MCP-SECRETS.md
and the current reports in VERIFY.md. The default cold-build Dockerfile does not
install the optional Secret Service fixture packages; that flag requires an image
containing dbus-daemon and gnome-keyring, as recorded in the current test context.

The durable native setup follow-up passes 188 regular Rust tests, seven daemon
process suites and two additional real Secret Service process tests. The new test
hard-kills a child after confirmed publication and after a deliberately failed
SQLite publication, then reconciles the same native key without creating a replacement.
The native fixture runner now executes both the custody and setup tests inside its
private DBus session. Current source manifests and boundaries are in VERIFY.md and
`.evidence/native-secret-setup-2026-10-08.json`; the preceding reports remain historical.
