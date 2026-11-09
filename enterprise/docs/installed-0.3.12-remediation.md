# Installed 0.3.12 remediation

Source: private development checkout, branch `codex/internal-development-v0.3.11`, based on `c3bfcf1e9a5459c13ee8a39cf40672e7c59897c0`. The pre-existing local version changes to 0.3.12 are preserved. This is an implementation/verification record, not a release or installation claim. No commit or push was performed. A final read-only hash check confirmed the user's installed executable still matches the original audit baseline; the implementation has not replaced it.

## Runtime and execution

Windows production resources are rooted at the raw `GetModuleFileNameW` module directory. Tauri's canonicalized resource path can resolve inside the launching packaged application's virtualized cache and must not anchor the installed Windows runtime. `runtime-layout.json` binds app version, target, relative resource names and SHA-256 hashes to one coherent directory. Traversal, rooted paths, resource junctions, missing files and hash/version mismatches fail closed.

Generate the layout **after** preparing Crew, Codex and the daemon, and after changes to bundled Python files. The build and verification pipelines do this automatically. Debug builds may explicitly use `LACOWORK_RUNTIME_ROOT`; production does not accept this override.

Daemon activation uses content-addressed binary/sidecar directories, atomically updates validated runtime paths, and checks authenticated IPC health, version and child process identity before updating login startup. A known provisioning failure is returned directly to the caller. Previous content-addressed binaries remain available for rollback.

The built-in DuckDuckGo MCP is `app.exe --lacowork-mcp-websearch`, dispatched before Tauri initialization. Discovery uses no network or Node installation. Exact legacy built-in Node configurations migrate; custom relative stdio commands require an explicit working directory. Imported file configuration uses its JSON parent as that directory. Stored environment secrets remain on the credential path, not diagnostic metadata.

## Crew, providers and diagnostics

One application controller coalesces runtime status/bootstrap operations, owns a single automatic offline first-run attempt, ignores stale generations and exposes a manual retry. Readiness requests also carry generations, including repeated checks of an unchanged definition. Runtime status does not extract archives or start Python. Successful heavy validation records a compatibility marker; the UI panels only subscribe.

Rust preflight is shared by validation and execution. Empty tasks, inactive agents, missing/cyclic dependencies, unavailable providers/models and runtime readiness produce explicit blocking issue codes. Pinned API agents in a native mixed Crew resolve the same saved database profile and model override as the executor; unused Crew-wide defaults do not replace those selections. Hierarchical managers are orchestration-only: execution copies lose tools/MCP access, while stored definitions and historical snapshots remain unchanged.

New or empty OpenRouter profiles select `openrouter/free`. Nonempty saved models are retained; unavailable catalog selections require the user's explicit replacement. Persisted catalogs remain selection data, not proof of current availability: readiness uses only recent successful, session-local model-list responses matching the endpoint/authentication/TLS target. Credential mutations invalidate in-flight catalog generations; outdated replies cannot reintroduce stale evidence in either backend or frontend. Model-list requests must match the saved endpoint/TLS configuration before a stored credential can be read or attached. System diagnostics resolve the currently selected API/Codex target, resolve credentials in the backend and reject stale probe results. Optional unconfigured subsystems are neutral and inactive terminals are idle.

## Audit integrity and recovery

`AuditLedger` owns inspection, tail reads, append, rotation and recovery. Lock ordering is process mutex then the operating-system exclusive `audit/ledger.lock`. Contention times out after five seconds with `audit_lock_busy`; callers can retry. No event or recovery file is changed on lock timeout.

Classification occurs before key creation. Clean unsigned history without the credential history marker is preserved byte-for-byte before beginning a signed chain. Damaged legacy history, signed tampering and missing signed history cannot silently heal. The System settings card requires confirmation bound to an opaque assessment of the exact current bytes. Transient storage/credential failures expose retry instead of destructive recovery.

Preserved files live under `audit/preserved/<recoveryId>/` with a size/hash manifest. The sequence-one signed record binds its hash, the legacy/malformed counts and prior trust. Preserving history does **not** authenticate old events. A durable HMAC-authenticated recovery journal resumes interrupted phases; only the authorized recovery entry can create a key, never an on-disk journal. Originals are only removed after all archive bytes and the manifest have been synchronized. A signed pending-record WAL repairs only its own authenticated exact partial tail. Rotation authenticates its staged snapshots and checks every live destination against the original/already-rotated bytes before writing; unexpected changes fail closed.

Diagnostic V2 reports contain counts, trust, stable reason codes and opaque recovery metadata, never event text or paths. Unverified audit tails are omitted from support bundles. The user's existing installed audit file was not used as a mutable test fixture.

## Interface changes

At 899 CSS pixels or narrower, selecting a route/subroute/search result closes the modal application menu and focuses the destination. Escape/explicit close returns focus to the menu trigger. Desktop navigation remains modeless and persistent at 900 pixels and wider.

Collapsed Tasks subtitles wrap at narrow widths. The Browser toolbar has two rows below 620 pixels and the production initial URL is `https://example.com` (no automatic navigation). Embedded Server pages use an h2 under the shell's h1; standalone Server keeps one h1. Touched English/German text and accessible input labels use canonical translation keys. The i18n audit now detects the formerly missed fragments and scoped raw JSX/aria text.

## Verification and remaining acceptance gates

Automated tests use temporary directories, fixture providers and mocked credentials. Run `npm run verify` for the integrated checks; the slow eight-process audit regression is also explicit in the release-candidate workflow. The integrated script includes all seven release-script test files and the flat-design contract, not only the bundler. `npm run verify:crew-runtime` validates the existing Python/wheel archives and runs the current Python tests offline in disposable storage without rebuilding the bundle. It uses a Windows-compatible temporary profile, verifies Shell data-directory resolution before importing CrewAI, disables live integrations and telemetry, and checks source/archive/manifest/layout hashes remain unchanged. The expanded installer smoke belongs on an isolated Windows runner/profile: it installs the application and sets up its runtime, so it must not be pointed at a user's daily installation as a read-only check.

Verified locally on 2026-09-03: all 597 Vitest cases; 52 Playwright cases with inspected screenshots and unchanged baselines; the complete Rust library suite (243 passing, four explicitly ignored); the separate eight-process/1,600-event audit test; frontend build, TypeScript, ESLint, Rust Clippy with warnings denied, the 320-file flat-style contract and i18n audit (1,841 keys in each language, zero findings). A contradictory Settings test fixture initially expected the OpenRouter card open despite selecting Ollama as the global API; it now verifies both actual global selections and manual expansion, without a production change or a weakened assertion. The real isolated **release daemon** lifecycle test passed model response, tool execution, encrypted provider/MCP metadata, schedules, restart, process replacement, graceful checkpoint and authenticated IPC. The audit concurrency test injects a test signing key; it does not claim to verify Windows Credential Manager first-key creation across processes.

The newly built production executable also passed `app/scripts/staged-runtime-smoke.ps1`: 24 hash-validated payloads staged in an isolated temporary directory, an unrelated working directory, an ignored debug-only root override, an offline built-in MCP handshake, and fail-closed behavior for a removed staged manifest. No normal GUI, installer, registry entry or application-data initialization was invoked. This checks production CLI/resource behavior, not a fresh-install/upgrade or packaged-parent GUI acceptance test. The executable was built before the final Crew preflight refinements; it is not a release artifact representing every final source edit.

The full offline Python run used the archived Python 3.12.10 and hash-locked CrewAI 1.15.8 wheelhouse: 55 tests discovered, 52 passed, three live-integration cases explicitly skipped. Runtime status reported compatible; all source/archive/manifest/layout hashes remained unchanged. A temporary-profile layout error initially prevented Windows Shell known-folder resolution; the corrected isolated profile passed the pre-import path check and complete suite. No system Python, download, installer or normal user-data directory was used.

The 31 release-script tests, toolchain doctor, generated-contract check and supply-chain policy (347 npm / 497 Cargo records) also passed. These checks were executed individually; no complete release/publishing workflow was dispatched. The source graph was rebuilt AST-only after the final code changes (10,209 nodes, 24,653 edges); no external model was used for that update. The pre-existing VEX version drift was corrected only after the targeted revalidation below.

### Targeted VEX metadata revalidation

Only the desktop and server product IDs changed from 0.3.8 to the already-existing 0.3.12 source versions; Android remains 0.3.8. No vulnerability status, justification or affected dependency version was broadened. This is not a general security audit.

For [GLib's iterator advisory](https://rustsec.org/advisories/RUSTSEC-2024-0429.html), the locked Windows graph contains no GLib; the Linux graph brings GLib 0.18.5 through Tauri/GTK. A search of all 742 resolved Cargo package source directories for `VariantStrIter` and `array_iter_str` completed without scan errors and found only GLib's own implementation, re-export, documentation and tests. Its constructor is crate-private and no external factory call was found.

For the [RustCrypto RSA timing advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), the locked server graph retains RSA 0.9.10 through OpenID Connect and WebPush. The maintained server paths use OpenSSL for FCM signing, ES256 for WebPush and provider-signature verification for OIDC; no RustCrypto RSA private-key construction or operation was found. Evidence commands: `cargo tree --locked --offline -p cowork-server -i rsa`, target-specific `cargo tree ... -i glib`, and resolved-source search from `cargo metadata --locked --offline --format-version 1`.

Lockfile hashes at revalidation: desktop `C39599906CD8E08E639DA1C199CAF643AAF3C3CF3CE4EE2F092CB0F2EF327B42`; workspace `67DACAE6CE92729975727AEBA9E9AB35E2A1FCAA097F69367D46942B3D6322E1`.

Before publishing a new version, complete fresh-install and upgrade acceptance from 0.3.12 in an isolated Windows profile/VM, including launch from a neutral process and a packaged parent; perform a real free OpenRouter chat, Crew run, and 29-route/CRUD restart matrix; test configured external GitHub/Server destinations only with dedicated disposable targets. NVDA, 200% zoom and Windows high-contrast checks remain separate manual gates. Unavailable credentials or environments are **not** passing results.
