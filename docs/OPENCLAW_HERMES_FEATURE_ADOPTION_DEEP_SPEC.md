# Hermes Memory and Command Adoption

Status: implemented and covered by automated tests as of 2026-07-16.

## Scope

OpenCowork adopts the durable-memory behavior that matters for local agent work while keeping its SQLite and Tauri architecture. This is behavioral compatibility, not a byte-for-byte copy of Hermes Agent.

## Memory lifecycle

1. A new engine session is persisted before its first model turn.
2. The backend freezes agent memory, shared knowledge, and the user profile into that session.
3. The frontend renders the frozen snapshot once and passes it separately to `QueryEngine`, which owns the single `<memory>` injection point.
4. `MemoryWrite` supports `add`, `replace`, and `remove` against bounded curated `memory` and `user` targets. Exact duplicates are ignored; replace/remove require one unique substring match.
5. Writes become canonical context in the next session. `SessionSearch` retrieves exact details from persisted earlier conversations.
6. High-signal statements are also captured automatically as reviewable candidates in `.cowork/DRAFT_KNOWLEDGE.md` and the `shared/draft_knowledge` database category. Secrets, ordinary chat, and prompt-injection text are rejected.

## Compatibility matrix

| Capability | OpenCowork implementation | Verification |
| --- | --- | --- |
| Bounded curated agent memory | 2,200 Unicode characters | Rust capacity tests |
| Bounded user profile | 1,375 Unicode characters | Shared mutation implementation and usage response |
| Frozen session-start memory | Snapshot stored in `sessions.memory_snapshot_json` | Snapshot and idempotent-session tests |
| Add/replace/remove | `memory_mutate` plus `MemoryWrite` | Rust and TypeScript tool-contract tests |
| Exact duplicate prevention | Normalized content comparison | Rust duplicate test |
| Unique-substring editing | Ambiguous and absent matches fail without mutation | Rust mutation tests |
| Safety scan | Invisible controls and common prompt-injection patterns rejected | Rust safety test |
| Past-session recall | SQLite search across linked persisted chat messages | DB and `SessionSearch` tool tests |
| Automatic candidate capture | Deterministic high-signal classifier; file and DB draft | Memory IPC tests |
| Project draft files | `.cowork/DRAFT_KNOWLEDGE.md` and `.cowork/DRAFT_MEMORY.md` are loaded as context | Memory builder tests |
| Shared knowledge in UI/search | Unscoped reads query every memory scope | Store and backend search tests |
| Single prompt injection | Builder returns base prompt and memory separately | Memory IPC contract test |

## Slash-command contract

`commandRegistryStore.ts` is the source for the command palette, autocomplete, help, and Cowork fallback execution. It currently validates 86 unique slash commands at construction. Every command exposed through the Claude bridge is present in that registry. Execution is awaited; failed asynchronous commands are not logged or displayed as successful.

The registry test invokes every registered command without arguments, verifies uniqueness and syntax, and verifies coverage of the model-facing bridge definitions. Commands with required arguments return usage/help or perform no unsafe action when invoked without them.

## Intentional differences

- OpenCowork stores curated memory in SQLite and renders a session snapshot; `MEMORY.md` and `USER.md` remain supported project-context files.
- Automatic capture writes only to a draft knowledge base. Promotion to curated memory remains explicit or model-driven so a heuristic cannot silently rewrite canonical long-term memory.
- OpenCowork keeps its existing memory panel, shared-knowledge categories, runtime instructions, and provider records alongside the Hermes-style core.
