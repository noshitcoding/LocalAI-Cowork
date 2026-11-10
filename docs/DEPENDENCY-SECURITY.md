# Dependency security scope for alpha.8

Reviewed on 9 November 2026 against RustSec database commit
`7a40350e1b8ba5b656d909f88222a8059a83d88d` (1,422 advisories).
The adopted core lock still reports **six vulnerabilities** and the desktop lock
**four vulnerabilities**, plus informational warnings. No advisory ignore list
was added. This source review does not qualify a signed package or deployment.

The narrow lock updates cover rustls 0.23.46, rustls-webpki 0.103.16,
webpki-roots 1.0.10 and Tokio 1.53.3; desktop also updates webpki-root-certs
1.0.10 and async-executor 1.14.1. Existing unrelated dependency bindings are
preserved. Final source checks must run with these adopted locks.

## Core: active OIDC dependencies, public verification

The remaining rsa 0.9.10 ([RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html))
and crypto-bigint 0.5.5 ([RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html))
are active through openidconnect 4.0.1 and cowork-identity. They are not merely
unused database dependencies.

The current Android OIDC verifier builds a CoreClient without a client signing
key and verifies provider ID tokens with an explicit algorithm allowlist.
The pinned SDK constructs RSA public keys and p256/p384 verification keys from
provider data. The reviewed application path performs public-key verification;
it does not construct RSA/EC private signing keys or decrypt through this SDK.
That limits exposure to the cited secret-arithmetic timing flaws, while the raw
findings remain visible. Adding private-key operations requires reassessment.

## Core and desktop ZIP findings

The four extraction advisories
[RUSTSEC-2026-0367](https://rustsec.org/advisories/RUSTSEC-2026-0367.html),
[RUSTSEC-2026-0368](https://rustsec.org/advisories/RUSTSEC-2026-0368.html),
[RUSTSEC-2026-0369](https://rustsec.org/advisories/RUSTSEC-2026-0369.html) and
[RUSTSEC-2026-0429](https://rustsec.org/advisories/RUSTSEC-2026-0429.html)
remain for core zip 2.4.2 and desktop zip 4.6.1. They concern filesystem
extraction, including privileged permissions, escaping symlinks and unbounded
allocation for declared symlink sizes.

Core's `crates/storage/src/productivity_archive.rs` only writes or reads one
in-memory `manifest.json` entry. Input and expanded output have 16 MiB bounds;
directories and symlink-mode entries are rejected, and reads use `take(limit+1)`.
It does not call the ZIP filesystem extraction APIs. Managed-runtime extraction
uses a separate capability-based archive implementation.

Desktop's zip is a real Windows dependency of tauri-plugin-updater 2.13.2.
Its upstream Windows ZIP installer calls `ZipArchive::extract`; this is not
merely a build dependency. The application's Windows update branch instead
hands signature-verified raw NSIS PE bytes to its own installer observer and
does not call the plugin's `Update::install`. The application's install call
is restricted to non-Windows builds; upstream Linux installation uses tar/flate2.
The main WebView capability exposes no updater plugin commands, and checked-in
updater keys/endpoints are empty. Direct plugin installation or Windows ZIP
updates require a fresh review. A broader updater/ZIP major migration remains
separate work; the scanner findings are retained.

## Linux GUI and release limits

Desktop glib 0.18.5 retains
[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html),
an unsound VariantStrIter implementation in the Linux GTK dependency family.
A search of direct application calls does not prove that transitive GUI code
cannot reach it. This warning remains unresolved, along with unmaintained
dependency notices in the raw report. Windows does not compile that GTK path.

The V2 JavaScript audit and native Rust findings have different dependency
graphs. Enterprise also has its own [security status](../enterprise/docs/PREVIEW-SECURITY-STATUS.md),
including real WebPush secret-key arithmetic and an open production gate.
Passing source checks does not establish installed-package acceptance, signing,
update-feed configuration or release readiness for a platform.
