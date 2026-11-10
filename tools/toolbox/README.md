# Explicit private tool setup

The desktop embeds `runtime_install.py`, `requirements-documents.lock`,
`requirements-browser.lock`, `browser-runtimes.lock.json`, `linux-git.lock.json`
and `bootstrap-windows.ps1` into the executable. Setup has two profiles:

- **documents** runs automatically once after the desktop connected to its local
  instance (`toolbox_setup_ensure_documents`): the document wheels plus a pinned
  private Tectonic with an offline LaTeX cache, no browsers. Without a valid
  receipt it retries with backoff (10 min, 1 h, 6 h, then daily, recorded in
  `<data>/tool-setup-auto.json`); cancelling an automatic run pauses it until the
  user starts setup in **Werkzeuge einrichten**. An existing private CPython from
  an earlier setup is reused, so earlier browser packages stay importable.
- **full** (the explicit **Private Laufzeit installieren** click, and the default
  of `install.py`) installs both locks, Chromium, Chrome for Testing and Firefox,
  plus optional Git, GitHub CLI and LibreOffice.

The panel reports each stage, supports choosing existing executables, retries and
cancellation, and reloads the running local toolbox through private native IPC.
Closing the desktop cancels and joins its exact setup worker and children.

CPython is downloaded into a fresh private directory. Windows uses the official
CPython 3.13.16 embeddable archive; Linux x86_64 uses the pinned Astral standalone
3.13.16 archive. Both downloads have an exact SHA-256 bound. No Python install
manager, machine PATH, registry, package manager or global pip is changed.
Both wheel locks contain complete pinned binary-wheel hashes for every platform
wheel (Windows and manylinux x86_64; resolution checked for CPython 3.11–3.13). The documents lock
(`python-docx`, `openpyxl`, `python-pptx`, `pypdf`, `pypdfium2`, `pillow`,
`reportlab`, `xlsxwriter`, `lxml` and their dependencies) is about 19 MB; the
browser lock only adds Playwright, greenlet and pyee and is never installed
without the documents lock. Chromium belongs to the private data directory.

Tectonic 0.17.0 comes from the official GitHub release (Windows `msvc` zip,
Linux static `musl` tarball); both the archive and the extracted binary have a
pinned SHA-256, and only that single file is extracted. Setup then compiles 20
embedded warm-up documents (including the generated template's 10/11/12-point
math fonts, KOMA-Script, beamer, TikZ/pgfplots, siunitx, babel,
fontspec/unicode-math, natbib/BibTeX styles and the Latin Modern, TeX Gyre,
Noto, Fandol, HaranoAji and Un fonts) into `<data>/tool-runtime/tectonic-cache`
(about 115 MB, 7–8 minutes on first run), compiles them again with
`--only-cached` behind a dead proxy, checks the bundle digest and writes
`<data>/tool-runtime/tectonic-warm.json`, which binds the cache to the binary's
SHA-256. On Windows the cache path may not exceed 115 characters (Tectonic cannot
write paths of 260 characters). A user-selected Tectonic is warmed instead of
being replaced; a stale private copy is replaced. Receipts use
`localai-cowork-tool-setup/3` with one entry per profile and keep the top-level
`ready` of `/2`. The daemon still keeps LaTeX locked until Tectonic runs inside
an OS sandbox (see `docs/DOCUMENT-TOOLS.md`).

Optional Git and GitHub CLI use official, pinned archives. Linux Git is a private
extraction of 43 exact Debian bookworm/security packages, including its curl,
TLS and Perl dependencies. Its wrapper confines `GIT_EXEC_PATH`, library paths,
Perl paths and a public CA bundle to that Git process. No dpkg configuration or
package scripts run. The supported Linux baseline is x86_64/glibc >=2.36.

LibreOffice 26.8.1 is downloaded with the official SHA-256. Windows extraction
reads MSI tables and embedded cabinets using MSI's database APIs and Windows
`expand.exe`; no MSI installer action runs. Linux extracts the DEB payloads into
the private runtime. The installed distribution's copyright/license files stay
with the extracted tools. Graphical system libraries and fonts come from the
native desktop prerequisites or the supplied server/team Docker image. Adapter
imports, a blank Chromium page and selected CLI versions are checked as part of
the user-requested setup. A failed check produces an actionable error; readiness
is recorded only after these checks and the configuration publication.

For an operator's existing server, Python 3.12+ can run the same helper
(`--profile documents` skips the browsers):

```sh
python3 tools/toolbox/install.py --data-dir /your/private/v2-data --git --github-cli --libreoffice
```

Image builds use `runtime_install.py --data-dir /opt/cowork/document-runtime
--tectonic-only`: it installs and warms only Tectonic; the image makes the
result read-only and points `COWORK_DOCUMENT_RUNTIME_DIR` at it.

Restart that operator-managed instance afterwards. The supplied server and
separate team-tool images already contain these tools. Docker remains an
explicitly selected operator prerequisite because a system/container service is
not installed by a desktop tool-runtime click.

Authoritative download metadata:

- https://www.python.org/downloads/release/python-31316/
- https://github.com/astral-sh/python-build-standalone/releases/tag/20261009
- https://github.com/git-for-windows/git/releases/tag/v2.56.0.windows.2
- https://github.com/cli/cli/releases/tag/v2.102.0
- https://download.documentfoundation.org/libreoffice/stable/26.8.1/
- https://pypi.org/
- https://github.com/tectonic-typesetting/tectonic/releases/tag/tectonic%400.17.0
- https://relay.fullyjustified.net/default_bundle_v33.tar (TeX Live 2022 based `tlextras-2022.0r0` bundle)
- https://deb.debian.org/debian/dists/bookworm/main/binary-amd64/
- https://security.debian.org/debian-security/dists/bookworm-security/main/binary-amd64/
