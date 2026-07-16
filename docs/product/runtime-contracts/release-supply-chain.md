---
title: Release Supply Chain Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-11
last_verified: 2026-07-11
endpoint: npm verify and security scripts, Cargo/RustSec audit, GitHub CI, and Windows installer release workflow
purpose: Ensure every release is built from consistent locked inputs, passes vulnerability and license policy, and ships verifiable dependency and provenance metadata.
userStory: As a user or operator I can verify what entered a Windows release and whether its published artifacts match the audited build.
visibleText: GitHub release assets and workflow results only
sizeToken: none
states: source-verified, policy-passed, audited, built, inventoried, attested, published, blocked
interactions: verify source, audit dependencies, generate SBOM, generate notices, hash assets, attest build, publish release
dataSource: package.json, package-lock.json, Cargo.toml, Cargo.lock, Tauri config, pinned policy, git commit, and release artifacts
accessibility: release metadata is machine-readable JSON plus a plain SHA256SUMS file
tests: supply-chain.test.mjs policy, version, Tauri release-line, installer discovery, Authenticode ordering and bypass rejection, denied-license, Rust-floor, deterministic-SBOM, provenance, and workflow-hardening tests; official CycloneDX 1.6 schema validation; local NSIS release build and ephemeral-certificate Authenticode smoke test
source_files:
  - app/scripts/supply-chain.mjs
  - app/scripts/supply-chain.test.mjs
  - app/scripts/sign-windows-installer.ps1
  - app/scripts/authenticode-smoke.ps1
  - app/supply-chain-policy.json
  - app/scripts/verify.mjs
  - .github/workflows/ci.yml
  - .github/workflows/windows-installer.yml
  - .github/dependabot.yml
  - rust-toolchain.toml
canonical_for:
  - dependency vulnerability and license gates
  - release version and Rust toolchain consistency
  - CycloneDX SBOM and third-party notice generation
  - release provenance, checksums, and GitHub attestations
rationale: A release is not reproducible or supportable when dependency policy, build inputs, and artifact identities exist only in mutable workflow state.
---

# Release Supply Chain Contract

## Locked source and toolchain

`package.json`, the root entry in `package-lock.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json` must carry the same product version. A release tag must equal `v<version>`. Rust `tauri`, `@tauri-apps/api`, and `@tauri-apps/cli` must share one major/minor release line. The Rust package declares `rust-version = 1.89`, and `rust-toolchain.toml` plus every CI job pin Rust `1.89.0`. The supply-chain gate rejects a dependency whose declared Rust requirement exceeds that floor.

`npm ci` and Cargo `--locked` operations are the only accepted dependency resolution paths in CI. External GitHub Actions are referenced by full 40-character commits, not mutable major tags. Dependabot checks npm, Cargo, and workflow actions weekly; updates still require all gates.

## Vulnerability and license policy

The normal network-free `npm run verify` checks version consistency, the Rust floor, workflow hardening, and every npm and Windows-reachable Cargo license against `app/supply-chain-policy.json`. Missing, unknown, malformed, or denied license expressions fail closed. SPDX `OR` expressions pass when at least one branch is approved; every branch of `AND` must be approved; only listed exceptions are accepted. Release SBOM components exclude dev-only npm packages, while license policy still evaluates them because build tools execute in the release trust boundary.

The separate blocking security job runs `npm audit --audit-level=high` over production and development dependencies and exact `cargo-audit 0.22.2` against `Cargo.lock`. No `|| true` or equivalent bypass is allowed. Semgrep and Trivy remain blocking, with Trivy including unfixed findings. The July 11 baseline has zero npm audit findings and zero RustSec vulnerabilities. RustSec still reports non-vulnerability maintenance warnings, primarily GTK3 crates for non-Windows targets; they remain visible and do not weaken the zero Critical/High vulnerability gate.

## Release evidence

The release workflow reruns the complete product verification and both network vulnerability audits before building. It produces:

- `Open-Cowork-Setup-x64.exe`
- `open-cowork.cdx.json`, a deterministic CycloneDX 1.6 inventory
- `THIRD_PARTY_NOTICES.json`, the evaluated npm and Cargo license inventory
- `release-provenance.json`, containing subject hashes, source commit, target, tool versions, and material hashes
- `SHA256SUMS`, covering every published local release file except itself

The SBOM serial number derives from both lockfiles and its timestamp derives from the source commit or `SOURCE_DATE_EPOCH`, so identical locked source and context produce identical metadata. npm SHA-512 integrity values and Cargo SHA-256 registry checksums are represented in CycloneDX hexadecimal form. The checked baseline contains 527 release components and passes the official CycloneDX 1.6 JSON schema with zero errors.

GitHub Actions additionally creates Sigstore-backed build-provenance attestations for all release assets and an SBOM attestation binding the installer to `open-cowork.cdx.json`. These attestations complement the downloadable offline provenance; they do not replace Windows Authenticode signing.

## Windows Authenticode

The release job signs `Open-Cowork-Setup-x64.exe` before generating provenance, checksums, attestations, or release assets. `OPEN_COWORK_CODESIGN_PFX_BASE64`, `OPEN_COWORK_CODESIGN_PASSWORD`, and `OPEN_COWORK_CODESIGN_THUMBPRINT` are GitHub secrets and are never command-line arguments. The signer imports the PFX only into the ephemeral runner's current-user certificate store, requires exactly one valid private-key certificate with the code-signing EKU, compares its normalized thumbprint with the pinned secret, signs with SHA-256, and removes imported key material in a bounded cleanup process.

RFC 3161 timestamp services are restricted to the Sectigo, DigiCert, and GlobalSign hosts. Each signing and verification process has a hard timeout. Publication fails unless SignTool policy verification returns success, PowerShell reports `Valid`, the signer thumbprint still matches, and a timestamp certificate is present. The workflow policy requires signing before all release evidence and rejects test switches or test-mode environment variables.

`npm run smoke:authenticode -- -InstallerPath ..\dist-installers\Open-Cowork-Setup.exe` exercises the real PE mutation and exact-thumbprint verification with an ephemeral self-signed certificate. Its explicit test gate permits an untrusted, untimestamped certificate only for that local copy; it can never satisfy release verification and is forbidden in the workflow.

## Failure and change policy

Version drift, mutable action references, a non-blocking audit, missing required release evidence, denied licensing, a dependency above the Rust floor, any npm High/Critical advisory, any RustSec vulnerability, a failed scanner, or a failed product test blocks publication. New license identifiers require a reviewed policy change. Vulnerability exceptions must identify the advisory, affected path, exploitability analysis, owner, expiration, and compensating control; no implicit ignore is permitted.

The local acceptance build produced a valid 64-bit NSIS PE installer for version `0.1.7` after the same Tauri release-line check. Its Authenticode integration and PE-signing smoke test are implemented, but the checked local artifact remains intentionally unsigned because no public release certificate was supplied. Procuring and configuring that certificate, producing one publicly trusted and timestamped release artifact, signed update manifests, installer upgrade testing, and release-ring rollback remain GA gates.
