# LocalAI Cowork App

This directory contains the desktop application.

## Stack

- Tauri 2 desktop shell
- React 19 and TypeScript
- Vite frontend build
- Rust backend commands and SQLite persistence
- Zustand stores for app state
- Vitest and Testing Library for frontend tests

## Local Development

```powershell
npm install
npm run tauri dev
```

The frontend dev server runs on:

```text
http://127.0.0.1:5173
```

Tauri launches the desktop window against that dev URL.

## Checks

```powershell
npm run doctor
npm run lint
npm run typecheck
npm run test:ci
npm run build
```

Rust checks:

```powershell
cd src-tauri
cargo check
cargo test
cargo clippy -- -D warnings
```

## Installer

```powershell
npm run installer
```

The repository-level installer copy is written to:

```text
../dist-installers/LocalAI-Cowork-Setup.exe
```

## Important Paths

```text
src/                      React UI, stores, engine, utilities
src-tauri/src/            Rust commands and backend modules
src-tauri/tauri.conf.json Tauri product and bundle config
scripts/                  Build, smoke-test, doctor, and MCP scripts
smoke-screenshots/        Captured desktop smoke-test images
```

## Notes

Keep generated build output out of source control. When changing public behavior, update the root README or the relevant file in `docs/`.

## License

LocalAI Cowork is licensed under the Apache License, Version 2.0. See `../LICENSE`,
`../NOTICE`, and `../DISCLAIMER.md`.
