# Hermes Memory P0 Implementation Backlog

Status: complete as of 2026-07-16.

- [x] Correct duplicate detection so a stable key can update when content changes.
- [x] Add bounded curated `add`, `replace`, and `remove` mutations for agent memory and user profile.
- [x] Reject ambiguous substring edits, invisible controls, and prompt-injection content.
- [x] Include agent, shared, and user memory in frozen session-start snapshots.
- [x] Persist the snapshot before the first model turn and preserve it across later session saves.
- [x] Add a model-callable `SessionSearch` tool backed by persisted session/chat data.
- [x] Read user-profile memory from its dedicated table.
- [x] Remove double memory injection from the system prompt.
- [x] Repair snake-case/camel-case IPC mismatches for entries, snapshots, hints, compaction, and sessions.
- [x] Make unfiltered memory reads include all scopes and apply search filters before result limiting.
- [x] Add deterministic automatic high-signal draft capture with secret/injection filtering.
- [x] Include `.cowork/DRAFT_KNOWLEDGE.md`, `.cowork/DRAFT_MEMORY.md`, `MEMORY.md`, and `USER.md` in project context discovery.
- [x] Register all model-facing bridge commands in the central slash registry.
- [x] Await registry execution and report asynchronous failures truthfully.
- [x] Add registry validation and an all-command smoke test.
- [x] Cover memory mutations, snapshots, automatic capture, session search, IPC mappings, tool calls, and command execution with tests.
