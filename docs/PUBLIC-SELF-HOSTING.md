# Choose the correct self-hosting context

V2 and Enterprise use separate workspaces, lockfiles, server contracts,
deployment configurations and data profiles.

## V2

Run from the repository root. Follow the [V2 deployment guide](../deploy/v2/README.md)
or [native Linux server bundle guide](../tools/server/README.md), including their
stated acceptance limits. A V2 image uses the root context:

```sh
docker build -f deploy/v2/Dockerfile -t localai-cowork-v2:source .
```

This builds an image; it does not set credentials, deploy a server or qualify
production. Use the documented lifecycle tools with an explicit new private
deployment directory after reviewing the selected configuration.

## Enterprise

Run Enterprise guide commands from enterprise/. From the public root, use these
explicit contexts:

```sh
docker build -f enterprise/server/Dockerfile -t localai-cowork-enterprise-server:source enterprise
docker build -f enterprise/server/cowork-runner/Dockerfile -t localai-cowork-enterprise-runner:source enterprise
docker build -f enterprise/server/cowork-provisioner/Dockerfile -t localai-cowork-enterprise-provisioner:source enterprise
docker build -f enterprise/app/Dockerfile.web -t localai-cowork-enterprise-web:source enterprise
```

The retained app/ supplies management web/server dependencies. These commands
do not build its historical native desktop. Enterprise Compose files resolve
their existing relative paths within enterprise/.

Read the [Enterprise entry point](../enterprise/README.md),
[management deployment guide](../enterprise/deploy/enterprise/README.md) or
[single-host guide](../enterprise/deploy/README.md). Supply actual image digests
and operator-owned private secrets. Review DNS/TLS, identity, storage,
backup/recovery, runtime and platform acceptance before deployment.
This repository integration performs no live rollout or data migration.
