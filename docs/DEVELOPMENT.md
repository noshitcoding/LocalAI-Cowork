# Development setup

This guide covers a reproducible development checkout for the desktop,
distributed server, Web client and Android shell. Platform-specific release
signing is intentionally separate from normal development.

## Toolchains

- Git and Git LFS where required by your environment
- Node.js 22 and npm 10 or newer
- Rust 1.95.0 for the current CI/release line; the workspace declares its
  minimum supported Rust version in `Cargo.toml`
- Windows: Visual Studio 2022 Build Tools, WebView2 and PowerShell 7/Windows
  PowerShell for installer scripts
- Linux desktop: Tauri 2/WebKitGTK 4.1 prerequisites
- Python 3.12.10 x64 for the pinned Windows Crew runtime; supported Linux Crew
  development uses Python 3.10–3.13
- Distributed tests: Docker Engine, Compose and PostgreSQL client tools
- Android: JDK 17, Android SDK 35, Build Tools 35.0.0, NDK
  27.2.12479018 and Rust Android targets

CI is the authoritative source for exact hosted-runner versions and pinned
third-party actions.

## Bootstrap the checkout

```powershell
git clone https://github.com/noshitcoding/LocalAI-Cowork.git
cd LocalAI-Cowork
npm ci --prefix app
npm ci --prefix clients/android
cargo fetch --locked
```

Do not commit generated Android Gradle output, local `.env` files, runtime
credentials, installers, target directories or test artifacts.

## Desktop and Web development

Desktop:

```powershell
cd app
npm run tauri dev
```

Frontend only:

```powershell
cd app
npm run dev
```

Isolated Web build:

```powershell
cd app
npm run build:web
npm run test:web
```

The shared React code reaches native or remote services only through the
runtime client abstraction. Do not import Tauri-only APIs into a route used by
the Web or Android build without a lazy platform boundary and a test.

## Rust services and agents

```powershell
cargo run -p cowork-server
cargo run -p cowork-local-daemon
cargo run -p cowork-device-agent
```

The server requires PostgreSQL, S3-compatible storage and its configuration;
use the Compose stack for normal end-to-end work. The daemon and device agent
READMEs document their required local variables and credentials.

## Contracts

The canonical schemas are derived from shared source contracts:

```powershell
cd app
npm run contracts:generate
npm run contracts:check
```

Commit source and generated v1/v2 artifacts together. Protocol changes must
retain N-1 compatibility or introduce an explicit version boundary and tests.
Never hand-edit `contracts/generated/*` as the only change.

## Test layers

Fast local gate:

```powershell
cd app
npm run typecheck
npm run lint:ci
npm run test:ci
npm run test:scripts
npm run build
```

Rust gate:

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --manifest-path app/src-tauri/Cargo.toml --lib
```

Browser/UI gate:

```powershell
cd app
npm run test:ui
npm run test:web
```

Distributed acceptance is intentionally split into scripts so destructive or
resource-intensive fixtures stay explicit. Examples include:

```powershell
npm --prefix app run test:gateway
npm --prefix app run test:local-daemon
npm --prefix app run test:browser-session
npm --prefix app run test:storage-chaos
npm --prefix app run test:storage-pressure
```

Additional scripts under `scripts/` cover snapshot, quota, OIDC, passkey,
desktop, Office, worker-soak, security and upgrade/rollback scenarios. Read a
script's parameters before running it; many create disposable containers,
databases or large sparse fixtures.

## Build artifacts

Windows installer:

```powershell
cd app
npm run installer
```

Linux packages:

```bash
cd app
npm ci
npm run tauri build -- --bundles appimage,deb,rpm
```

Unsigned Android acceptance:

```bash
cd clients/android
npm ci
npm run android:init -- --ci
npm run android:build -- --apk --target aarch64
```

An unsigned APK is a build artifact, not an installable production release.
The signed internal workflow and required secrets are documented in the
[Android README](../clients/android/README.md).

## Version and release discipline

Desktop, Rust workspace, Android shell and generated release metadata share one
SemVer release line. Version changes must update npm, Cargo and Tauri manifests,
locks and VEX product identifiers together. `npm --prefix app run test:scripts`
contains a fail-closed metadata drift test.

Normal feature work does not create a release tag. Tagged publishing runs only
from a commit contained in `main`, after required GitHub Actions, signing,
provenance and release-environment checks pass. See the root README and current
release workflow before changing release automation.

## Pull-request checklist

- Keep standalone desktop behavior functional without a server.
- Hide incomplete distributed functionality behind capability/status checks.
- Add or update unit, contract and platform-boundary tests.
- Regenerate contracts and snapshots only when their source behavior changed.
- Do not weaken sandbox, egress, authentication, secret or audit boundaries to
  make a test pass.
- Update the relevant component README and implementation-status matrix.
- Run the smallest relevant tests during development and the full required CI
  matrix before merge.
