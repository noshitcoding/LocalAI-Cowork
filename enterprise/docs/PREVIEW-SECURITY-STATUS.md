# Enterprise security scope for the V2 preview

Enterprise remains an independent source workspace. The V2 preview does not
publish qualified Enterprise binaries or approve a production server deployment.
The Enterprise production security gate remains open.

The 2026-11-09 RustSec review of the existing native lockfile reports 18
vulnerabilities without advisory ignores. The source candidate applies only
ctutils 0.4.4, quinn-proto 0.11.18, rustls 0.23.46,
rustls-webpki 0.103.16, webpki-roots 1.0.10 and Tokio 1.53.3.
The adopted lock retains six raw vulnerability findings; it is not an audit-zero
result. Tokio also addresses the separately reported AsyncFd unsoundness notice.
Two identical locked metadata checks validate the dependency graph. The private
Windows compile remains pending; a fresh Linux Enterprise CI compile is required.
Source adoption and compilation are separate checks.

The management-web npm lock was separately updated from 20 reported findings to
zero using targeted security updates and three parent-scoped minimatch 11.0.5
overrides. Original direct dependency ranges, ESLint 10.8.1 and typescript-eslint
8.66 are retained. A fresh install, separate npm audit, TypeScript check and real
management-web build passed under Node 24.15.0/npm 11.6.2 on the exact adopted
manifest and lock bytes. Strict lint still reports six errors and ten warnings
in retained Enterprise sources; those are not declared fixed by the dependency
updates. These JavaScript results do not clear the native findings below.

| Remaining dependency | Exact scope and follow-up |
| --- | --- |
| rsa 0.9.10, RUSTSEC-2023-0071 | Compiled through OpenID Connect and WebPush, rather than only an unused database feature. The application verifies provider signatures for OIDC and explicitly chooses ES256 for WebPush. FCM RSA signing uses OpenSSL. No maintained RustCrypto RSA private-key operation was found. RSA 0.10 is patched, but current upstream parents still require 0.9; review their migration before changing the crypto graph. |
| crypto-bigint 0.5.5, RUSTSEC-2026-0387 | Compiled through elliptic-curve/p256/p384/k256. WebPush uses real VAPID signing and ECDH secrets, so this is an unresolved crypto dependency risk. The latest reviewed web-push-native 0.5 still requires p256 0.13 and jwt-simple 0.12. A parent version bump alone does not remove it. |
| zip 8.6.0, RUSTSEC-2026-0367, -0368, -0369 and -0429 | Direct dependency of cowork-local-daemon. Current OOXML code uses ZipArchive entry reads and ZipWriter with explicit expanded-size bounds. It does not call ZipArchive::extract or ZipStreamReader::extract, which the four advisories concern. A checked migration to zip 9 or newer remains follow-up work; no major upgrade or blanket exception is used to erase these findings. |

The existing historical OpenVEX and installed-version reports describe their
stated product and review scope. They do not establish acceptance of this source
candidate. The current Cargo audit configuration keeps all findings visible:

```sh
cd enterprise
cargo audit --file Cargo.lock
cargo tree --locked --workspace -i rsa --edges normal,build
cargo tree --locked --workspace -i crypto-bigint --edges normal,build
cargo tree --locked --workspace -i zip --edges normal,build
```

References: [RSA advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html),
[crypto-bigint advisory](https://rustsec.org/advisories/RUSTSEC-2026-0387.html),
[zip 9 release](https://github.com/zip-rs/zip2/releases/tag/v9.0.0).
