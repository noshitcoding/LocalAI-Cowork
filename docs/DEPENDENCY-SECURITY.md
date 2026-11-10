# Dependency security scope for alpha.8

Reviewed on 9 November 2026 against RustSec database commit
`7a40350e1b8ba5b656d909f88222a8059a83d88d` (1,422 advisories).
The adopted core lock still reports **six vulnerabilities** and the desktop lock
**four vulnerabilities**, plus informational warnings. No advisory ignore list
was added. This source review does not qualify a signed package or deployment.

A separate GitHub Dependabot snapshot on 10 November 2026 reports 63 open
manifest alerts: 35 in frozen migration reference locks, 21 in the independent
Enterprise workspace, and seven in active V2 locks. The seven V2 alerts are
three ZIP advisories in each of the core and desktop locks, plus glib in the
desktop lock. These per-manifest GitHub counts describe a different catalogue
and scope from the pinned RustSec counts above. The alerts remain open; no
advisory ignore or dismissal was added.

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

## GitHub ZIP advisories and reviewed callers

The live GitHub catalogue also identifies the following mechanisms:

| Advisory | Mechanism and first patched release |
| --- | --- |
| [GHSA-8hxf-qr8p-62x4](https://github.com/zip-rs/zip2/security/advisories/GHSA-8hxf-qr8p-62x4), CVE-2026-115906 | AES entry authentication can be bypassed on premature EOF in password-bearing decryption APIs; affected `>=0.6.0,<9.0.0`, patched in 9.0.0. |
| [GHSA-q7v9-x93c-xqv3](https://github.com/zip-rs/zip2/security/advisories/GHSA-q7v9-x93c-xqv3), CVE-2026-115909 | Symlink-target allocation during extraction can abort the process; affected `>=1.3.0,<9.0.0`, patched in 9.0.0. This is the RUSTSEC-2026-0429 finding already listed above. |
| [GHSA-m665-v7pm-7grj](https://github.com/zip-rs/zip2/security/advisories/GHSA-m665-v7pm-7grj), CVE-2026-115905 | Different central-directory and local-header interpretations can bypass inspection before a later streaming extraction; affected `<=9.0.4`, patched in 9.0.5. |

AES authentication and the central/local-header differential are additional
mechanisms to the four RustSec extraction findings described above. The current
core ZIP configuration disables default features and enables deflate, without
AES support. Desktop's Windows updater ZIP dependency also disables default
features and does not enable AES. The reviewed application calls supply no
password and use no decrypt API; the normal entry reader rejects encrypted
entries. This bounds the reviewed AES path without clearing the library alerts.

Core reads one central-directory `manifest.json` entry in memory and returns
bounded parsed JSON for application validation and import. The reviewed flow
does not pass the original ZIP to a local-header streaming extractor or ZIP
filesystem extraction. Inconsistent local-header names are not explicitly
rejected by that reader. Size limits, JSON validation and artifact hashes do not
prove header consistency or archive authenticity. The published mixed-parser
file-overwrite chain was not found in this reviewed flow; that is a bounded
caller finding, not a general claim that the ZIP library is unaffected.

Desktop retains the real Windows updater runtime dependency and its upstream
`ZipArchive::extract` implementation. The application's Windows path downloads
signature-verified raw NSIS bytes for its own installer observer; it does not
invoke the plugin's Windows ZIP installation path. Its plugin install call is
restricted to non-Windows builds. The checked main capability grants no updater
IPC commands, and checked-in updater keys and endpoints remain empty. Enabling
those commands, introducing ZIP updates, or changing the reviewed caller flow
requires reassessment; the upstream extraction implementation remains present.

A future migration intended to address all three GitHub ZIP advisories must
target at least 9.0.5; 9.0.0 does not resolve the reported header differential.
The current core ZIP major and updater's Windows ZIP 4 constraint require a
separately checked migration rather than an assumed lock-only replacement.
No such upgrade, parser hardening or general non-affected declaration is made
by this documentation update. The raw alerts and unresolved glib finding remain.

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
