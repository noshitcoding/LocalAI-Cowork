# Contributing

Use [issues](https://github.com/noshitcoding/LocalAI-Cowork/issues) for bugs and
proposals; use the [private security path](SECURITY.md) for vulnerabilities.

V2 development runs from the repository root with its pinned toolchains and
lockfiles. Read the relevant product docs and run the README checks. Enterprise
work belongs under enterprise/ with its separate workspace and frontend
lockfile. Preserve its server/management-web closure; do not build the retained
historical desktop for a V2 task.

Preserve explicit permissions, original request identities, idempotency, source
and result hashes, and conservative recovery of uncertain effects. Test changed
behavior and report the actual platform/scope. Source compilation does not
qualify an installed package or deployment.

Never commit secrets, private profiles, runtime data, evidence or caches. Keep
fixed test fixtures unchanged unless their format/compatibility is part of the
task. Git preserves source bytes; formatters own deliberate normalization.
Describe each PR's problem, resulting behavior, validation and material limits.
Publication, signing, updates and deployments retain their separate gates.
