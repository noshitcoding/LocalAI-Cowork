# LocalAI Cowork documentation

This directory is the documentation entry point for the desktop application and
the optional distributed runtime. LocalAI Cowork remains local-first: the
desktop works without an account or server, while the self-hosted server, Web
client, Android shell and external executors are opt-in.

> The distributed runtime is suitable for development and trusted pilot
> networks. Before an Internet-facing or multi-tenant deployment, review every
> open gate in [Distributed implementation status](DISTRIBUTED_IMPLEMENTATION_STATUS.md).
> A capability described in the architecture is not automatically a production
> guarantee; the status matrix and executable acceptance tests are the source of
> truth.

## Choose a path

| Goal | Start here | Required components |
| --- | --- | --- |
| Use the app locally with Ollama or a hosted API | [Setup guide](SETUP.md#standalone-desktop) | Desktop only |
| Build or change the repository | [Development setup](DEVELOPMENT.md) | Node, Rust and platform toolchains |
| Run durable jobs after closing the laptop | [Server deployment](SERVER_DEPLOYMENT.md) | Linux Compose server |
| Control server runs from Android | [Android README](../clients/android/README.md) | Server plus Android shell |
| Let a personal laptop execute server-routed work | [Personal executor](../agents/cowork-device-agent/README.md#personal-device) | Local daemon plus outbound agent |
| Run real Word, Excel, PowerPoint or Windows UI automation | [Managed Windows executor](../agents/cowork-device-agent/README.md#managed-windows-executor) | Licensed Windows executor pool |
| Understand security and runtime boundaries | [Distributed architecture](DISTRIBUTED_ARCHITECTURE.md) | No deployment required |

## User documentation

- [Setup and deployment selection](SETUP.md)
- [Ollama configuration](OLLAMA_CONFIGURATION.md)
- [Linux installation](LINUX_INSTALLATION.md)
- [Desktop control and computer use](DESKTOP_CONTROL_AND_COMPUTER_USE.md)
- [Developer browser and GitHub workbench](DEVELOPER_BROWSER_AND_GITHUB.md)

## Operator documentation

- [Single-host server deployment](SERVER_DEPLOYMENT.md)
- [Compose entry point](../deploy/README.md)
- [Runtime secrets](../deploy/secrets/README.md)
- [Personal daemon](../agents/cowork-local-daemon/README.md)
- [Personal and managed executors](../agents/cowork-device-agent/README.md)
- [Android build, signing and connection](../clients/android/README.md)

## Architecture and delivery

- [Distributed architecture](DISTRIBUTED_ARCHITECTURE.md)
- [Implementation and acceptance status](DISTRIBUTED_IMPLEMENTATION_STATUS.md)
- [Development setup and test matrix](DEVELOPMENT.md)
- [Security policy](../SECURITY.md)
- [Contributing](../CONTRIBUTING.md)
- [Release notes](releases/)

## Documentation rules

- Commands are written from the repository root unless a preceding `cd` says
  otherwise.
- `npm ci` is used for reproducible builds. `npm install` is appropriate only
  while intentionally changing dependencies and must update the lockfile.
- Never copy a real token, provider key, Android keystore or service-account
  JSON into a tracked `.env` file. Examples contain placeholders only.
- The canonical deployment has one HTTPS origin. Alternate domains or
  subdomains redirect to it so cookies, passkeys, PKCE callbacks and WebSocket
  origins remain unambiguous.
- Versioned contracts under `contracts/generated/` are generated artifacts.
  Change the Rust/TypeScript sources first, then run the documented generator.
- Release documentation describes tagged packages. Work on `main` may contain
  newer pilot features that are not yet part of a published desktop release.
