# LocalAI Cowork Enterprise server

This is the independent Enterprise server workspace. The current V2 app is at
the [repository root](../README.md). Enterprise has its own Cargo workspace,
lockfile, Rust toolchain and app/package-lock.json. Its 0.3.x version identifies
the server component, not the V2 application.

Keep this subtree intact: management web, agents, clients, runtime contracts
and server build dependencies can require app/ files. Those retained files are
not the current desktop application.

Run from this directory:

```sh
cargo check --locked --workspace --all-targets
cd app
npm ci
npm run build:web
```

For Docker builds from the public root, use enterprise/ as context. The
[complete context commands](../docs/PUBLIC-SELF-HOSTING.md) cover server, runner,
provisioner and management web. Existing Compose paths retain their structure.

- [Enterprise management deployment](deploy/enterprise/README.md)
- [Single-host deployment](deploy/README.md)
- [Implementation and limits](docs/ENTERPRISE_IMPLEMENTATION.md)
- [Server deployment](docs/SERVER_DEPLOYMENT.md)
- [Runtime secrets](deploy/secrets/README.md)
- [Current Enterprise security scope](docs/PREVIEW-SECURITY-STATUS.md)

Source integration does not deploy an instance, migrate a database, provide
keys or qualify DNS/TLS, identity-provider, VM/container, storage or recovery
operation. Keep generated operator profiles and evidence outside Git.
Do not run tauri build here for a V2 release; use the current root app.
