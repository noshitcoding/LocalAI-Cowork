# Repository boundaries

- The current desktop/web app is V2 at the root. Read README.md, package.json
  and relevant docs before changing apps/, packages/, crates/ or services/.
- enterprise/ is a separate Enterprise server workspace with its own Cargo/npm
  lockfiles and deployment contexts. Do not merge its workspace into V2.
- enterprise/app/ is retained where server builds and management web need it;
  do not build that historical native desktop for a V2 task.
- Preserve existing work and source-bound synthetic fixtures. Historical V1
  remains available through Git history and release tags.
- Never commit operator profiles, credentials, runtime data, private evidence,
  archives, installed payloads or caches.
- Commits, pushes, signing, release uploads and deployments are explicit
  operations. Follow the checked preview gates and exact package acceptance.
