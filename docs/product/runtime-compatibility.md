---
title: Runtime Compatibility
type: overview
doc_type: compatibility
status: seed-current
owner: product-docs
last_updated: 2026-07-02
last_verified: 2026-07-02
canonical_for:
  - current runtime assumptions
  - supported development and packaging targets
source_files:
  - README.md
  - app/README.md
  - app/package.json
  - app/src-tauri/Cargo.toml
  - app/src-tauri/tauri.conf.json
  - app/src-tauri/src/lib.rs
related_docs:
  - docs/ARCHITECTURE_CURRENT.md
---

# Runtime Compatibility

## Current Target

Open Cowork is currently Windows-first. The Tauri stack can support more platforms later, but current installer, bundled resources, desktop-control assumptions, smoke docs, and public README target Windows 10/11.

## Development Runtime

| Area | Current Requirement |
| --- | --- |
| Node | Node.js 22+ |
| npm | npm 10+ |
| Frontend | React 19.2.4, React Router 7.14.1, Vite 8.0.4, TypeScript ~6.0.2 |
| Desktop shell | Tauri 2.10.3 |
| Rust | Rust 1.77.2 minimum from `Cargo.toml` |
| WebView | Microsoft WebView2 Runtime |
| Local model | Ollama optional, default endpoint `http://localhost:11434`, default model `llama3.1:8b` |

## Development Commands

The frontend dev server is strict-bound to:

```text
http://127.0.0.1:5173
```

Primary dev command:

```powershell
cd app
npm run tauri dev
```

Useful checks:

```powershell
cd app
npm run doctor
npm run lint
npm run typecheck
npm run test:ci
npm run build
```

## Packaging Runtime

The packaged app uses Tauri NSIS output. `npm run installer` copies the expected installer to:

```text
dist-installers/Open-Cowork-Setup.exe
```

Bundled desktop resources include PDFium, the Python crew runtime, crew runtime requirements, and WebView2 loader resources.

## Browser Versus Tauri

Some UI and store behavior can run in a plain browser through localStorage and `safeInvoke` fallbacks. Full functionality requires Tauri because privileged actions are exposed through backend commands: filesystem writes, shell/terminal, desktop control, Office/PDF rendering, SQLite persistence, MCP runtime, scheduler, and crew runtime.

## Provider Runtime Modes

The backend can resolve provider URLs for host versus isolated runtimes. Current behavior keeps localhost for host mode and maps loopback URLs to `host.docker.internal` for isolated/docker-like modes. External hosts such as OpenRouter remain unchanged.

## Compatibility Update Rule

Update this file when runtime versions, supported OS targets, bundle resources, dev ports, installer behavior, or provider URL mapping rules change.
