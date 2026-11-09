# Added third-party components in Alpha.4

Quellstand 12. Oktober 2026: Aktuelle Paketierungen ergänzen `sbom.spdx.json`.
Sie enthält gesperrte Rust-/npm-Quellgraphen und genaue Payloadprüfsummen.
Die private Werkzeug-Einrichtung ergänzt eine tatsächliche Softwaredatei-,
Python-Paket- und native Bibliotheksinventur mit SHA256-Bindung im Setup-Beleg.
Externe OS-Abhängigkeiten bleiben Anforderungen; unbekannte Lizenzen bleiben
`NOASSERTION`. Eine Stückliste ersetzt keine Lizenzprüfung. Ältere Paketbytes
werden dadurch nicht verändert.

Remote desktop renders the RFB protocol using **noVNC 1.7.0**, copyright the
noVNC authors. Its core is licensed under MPL 2.0; vendor components retain their
included notices. The unmodified installed library source, AUTHORS, license
texts and byte manifest are included in `docs/third-party/novnc-1.7.0/` inside
`SOURCES.zip`. Upstream: https://github.com/novnc/noVNC/tree/v1.7.0 .

Passkeys use **webauthn-rs, webauthn-rs-core and webauthn-rs-proto 0.5.5**,
licensed under MPL 2.0. The unmodified library source and Cargo metadata from
the locked registry packages, the license text and a byte manifest are included
under `docs/third-party/webauthn-rs-0.5.5/` in `SOURCES.zip`.
Upstream: https://github.com/kanidm/webauthn-rs .

The Windows daemon builds **OpenSSL 3.6.3** statically through the locked
openssl-src package. Its Apache 2.0 license and the Rust wrappers' license texts
are included under `docs/third-party/openssl-windows/` in `SOURCES.zip`.
Upstream: https://github.com/openssl/openssl/tree/openssl-3.6.3 .

The built-in document tools (October 2026, not yet packaged) download into the
private tool runtime, never into the installer: **python-docx** (MIT),
**pypdfium2** (Apache-2.0 or BSD-3-Clause; bundles PDFium, BSD-3-Clause) next to the
existing pinned lxml, openpyxl, Pillow, pypdf, python-pptx, reportlab and xlsxwriter
wheels, and **Tectonic 0.17.0** (MIT) with TeX Live bundle files fetched by Tectonic
(each file under its own license, mostly LPPL; fonts under OFL/GUST). The tool setup
records them in its SPDX inventory.

The project license does not replace the licenses of its dependencies.
Exact npm and Rust dependency versions remain recorded in the lockfiles.
