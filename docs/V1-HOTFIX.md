# V1 permission hotfix

The current V1 working tree at Open_Cowork was preserved before modification in a private,
source-only archive with per-file SHA-256 hashes. The isolated patch in
v1-permission-hotfix.patch is against that working tree, not the older Git HEAD.

Changes:

- Chat, task start and durable scheduling call one permission mapping.
- Standard/default maps to confirm_mutations.
- Explicit bypass maps to autonomous.
- strict, plan and unknown imported values map to read_only.
- Missing tool policy in the local/Codex run and schedule adapters defaults to confirm_mutations.
- Crew's separate governance path was not rewritten.

Validation: permission mapping, local daemon adapter and task scheduling tests, plus V1
TypeScript compilation. This is a source hotfix only. No V1 installer, running service,
container, installed application or update channel was replaced.

The private baseline excludes live databases, user data, secrets and generated runtime
payloads. It is not a recoverable backup of any deployed installation.

