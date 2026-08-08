# Linux Installation

Linux x86_64 releases are available in three formats:

- `LocalAI-Cowork-x86_64.AppImage` for the broadest compatibility across desktop distributions
- `LocalAI-Cowork-x86_64.deb` for Debian, Ubuntu, Linux Mint, and related distributions
- `LocalAI-Cowork-x86_64.rpm` for Fedora, openSUSE, RHEL-compatible distributions, and related systems

The packages are built on Ubuntu 22.04, the oldest GitHub-hosted baseline used by this project that provides Tauri 2's required WebKitGTK 4.1 development packages. This improves compatibility with newer distributions, but no glibc-based desktop binary can guarantee support for literally every Linux distribution. The current artifacts target x86_64; ARM64 and musl-only systems such as Alpine are not release targets yet.

## AppImage

Download the AppImage from the latest GitHub release, make it executable, and start it:

```bash
chmod +x LocalAI-Cowork-x86_64.AppImage
./LocalAI-Cowork-x86_64.AppImage
```

Use the AppImage when your distribution is not Debian- or RPM-based. AppImage releases participate in the signed in-app updater.

## Debian and Ubuntu

```bash
sudo apt install ./LocalAI-Cowork-x86_64.deb
```

## Fedora, openSUSE, and RPM-Based Systems

```bash
sudo dnf install ./LocalAI-Cowork-x86_64.rpm
```

On openSUSE, use `sudo zypper install ./LocalAI-Cowork-x86_64.rpm` instead.

## Runtime Requirements

- A graphical x86_64 Linux desktop with GTK 3 and WebKitGTK 4.1
- Ollama when using local models
- Python 3.10 through 3.13 when using Crew workflows

The Windows release contains a verified offline Python and CrewAI bundle. The Linux beta intentionally does not ship those Windows-only artifacts. On Linux, Crew initialization creates an isolated virtual environment from the compatible system Python and downloads its pinned top-level requirements. Set `LOCALAI_COWORK_CREW_PYTHON` to an explicit interpreter path if `python3` is not discoverable from the desktop session.

## Current Linux Limitations

The main desktop workspace, Ollama route, files, MCP tools, terminal, tasks, and signed AppImage updates are release targets. Native Windows sandboxing, Windows Credential Manager integration, Windows Office automation, and the bundled Windows PDFium library are unavailable on Linux. Treat the Linux release as beta-quality and report distribution-specific issues with the distribution name and version.

Verify downloads with `SHA256SUMS` and the GitHub build attestations included with every release.
