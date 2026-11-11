# Dependency security scope for alpha.8

## ZIP migration review — 10 November 2026

The release candidate pins **zip 9.0.5** in both active V2 locks and updates
the desktop updater dependency to **tauri-plugin-updater 2.14.1**. Core keeps
default ZIP features disabled and enables deflate. Its only codec API change
maps the new fallible `entry.name()` result to the existing archive validation
error; the one-entry, filename, size, directory and symlink checks remain.
Desktop also disables ZIP default features through the updater dependency.

Version 9.0.5 is outside the published affected ranges of
[GHSA-8hxf-qr8p-62x4](https://github.com/zip-rs/zip2/security/advisories/GHSA-8hxf-qr8p-62x4),
[GHSA-q7v9-x93c-xqv3](https://github.com/zip-rs/zip2/security/advisories/GHSA-q7v9-x93c-xqv3)
and [GHSA-m665-v7pm-7grj](https://github.com/zip-rs/zip2/security/advisories/GHSA-m665-v7pm-7grj).
The last advisory concerns central-directory/local-header inconsistencies and
is first patched in 9.0.5; upgrading only to 9.0.0 would be insufficient.
Updater 2.14.1 permits ZIP 9, but its release-tag lock used 9.0.3. This
candidate therefore resolves the desktop lock explicitly to 9.0.5.

These locks are normal Cargo resolver output, not manually repaired graphs.
The migration also updates minisign-verify to 0.3.0, introduces typed-path and
removes the unused arbitrary/derive_arbitrary pair. Existing desktop packages
have permitted Windows/getrandom dependency-edge rebindings without version
changes; this does not demonstrate a defect in the former lock. Core deflate
now selects the upstream zlib-rs backend. Both complete `cargo metadata
--locked` graphs resolved successfully: 557 core packages and 581 desktop
packages, with unchanged lock bytes during metadata inspection. Metadata
resolution does not compile or typecheck the codec adaptation.

## OIDC migration and actual pinned RustSec results - 11 November 2026

Core now pins [openidconnect 4.1.0](https://github.com/ramosbugs/openidconnect-rs/releases/tag/4.1.0).
The normally resolved core lock selects **crypto-bigint 0.7.11** and removes
0.5.5. This is outside [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html)'s
affected range; its published patched range starts at 0.7.6. This does not
resolve RSA or the independent desktop GTK findings.

On 11 November, that core lock was audited with **cargo-audit 0.22.2** against
the unchanged 1,422-advisory RustSec database commit
`7a40350e1b8ba5b656d909f88222a8059a83d88d` (tree
`e206c84b629992c9f861e5336baed4136eda4216`). All 1,441 tracked database files
matched before and after. The command used `--no-fetch --file Cargo.lock
--json`, no advisory ignores or target filters, and neither `--no-yanked`
nor an offline flag. The report has no explicit fresh-yanked-completion field
and no yanked warning; a completed fresh yanked-status check is not claimed.

| Lock and report | Vulnerabilities | Other reported findings | Audit exit |
| --- | --- | --- | --- |
| Updated core, 11 November | 1: rsa 0.9.10 | 4 unmaintained warnings | 1 |
| Unchanged desktop, retained 10 November report | 0 in the vulnerability category | **Unsound glib 0.18.5**, plus 4 unmaintained warnings | 0 |

The current core report has no crypto-bigint 0.5.5 finding. The earlier ZIP
review's core report (two vulnerabilities) and desktop report remain retained;
those commands used `--no-fetch --no-yanked`, so **yanked status was not checked**
there. An outer evidence-path comparison error after the new audit is retained
separately; a read-only comparison confirmed the database maps matched. No audit
retry occurred and the actual audit exit remains 1.

[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
now lists RSA **>=0.10.0** as patched. OpenID Connect 4.1.0 still requires
`rsa ^0.9`, resolving to 0.9.10, so that upstream fix is outside its permitted
range. The retained caller review below describes public-key verification;
it does not waive the scanner finding or qualify the new dependency graph.

Zero desktop vulnerabilities does not mean zero findings or Linux GUI
acceptance. [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
remains for glib 0.18.5's unsound VariantStrIter. Its published fix is
**>=0.20.0**, outside the existing Tauri/GTK dependency family's 0.18 range.
The desktop lock is unchanged and has not been re-audited in this OIDC step.
A compatible upstream resolution remains open. No ignore, vendored patch,
security-clean claim or claim of unreachable transitive GUI code was added.

The earlier GitHub snapshot of **63 open manifest alerts** (35 reference,
21 Enterprise, seven active V2) is historical and uses a different catalogue.
Checking these three ZIP version predicates is not a fresh remote alert scan
or dismissal. The independent Enterprise workspace is unchanged; its
[security status](../enterprise/docs/PREVIEW-SECURITY-STATUS.md) remains
separate from V2.

## Validation limits for the OIDC candidate

The earlier Windows compatibility run passed `cargo check --locked --workspace
--all-targets` and the existing `cowork-identity` tests: **34 passed, zero failed,
four PostgreSQL cases ignored**. Subsequent Windows validation passed workspace
fmt, strict Identity Clippy and the entire Identity suite: **39 passed, zero
failed, four PostgreSQL cases ignored and zero filtered**. All five dedicated
offline Android OIDC verifier cases passed, covering independent Ring
ES256/ES384/EdDSA signatures, authorized-party validation and rejection of invalid
signatures, `none` and HS256. These remain bounded compatibility/regression
results, not full candidate or full-matrix qualification. Previous source results
do not qualify this changed graph. Full platform checks, actual packages and
installed acceptance remain separate, as do signing and update-feed requirements
in [the release procedure](V2-PREVIEW-RELEASE.md).

## Historical review before the ZIP migration

The following retained review describes the earlier locks and caller analysis
on 9–10 November 2026. Its six-core/four-desktop and 63-alert counts, old ZIP
versions and statements about a future migration are historical, not the
candidate status above.

<details>
<summary>Earlier alpha.8 dependency and caller review (original text)</summary>

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

</details>
