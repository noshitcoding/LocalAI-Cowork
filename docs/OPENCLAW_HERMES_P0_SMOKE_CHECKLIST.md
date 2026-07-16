# Hermes Memory P0 Smoke Checklist

Use this checklist for a packaged desktop build after the automated suite passes.

## Curated memory

- Start a new chat and ask the model to remember one durable project fact.
- Approve `MemoryWrite` if the active permission profile asks for confirmation.
- Confirm the tool response reports usage and says the write applies to future sessions.
- Start a second session and confirm the fact appears in the frozen memory context.
- Add the exact same fact again and confirm no duplicate is created.
- Replace it using a unique substring, then remove it using a unique substring.
- Try an ambiguous substring and confirm nothing changes.

## Automatic draft knowledge

- Send `Merke dir: Das Projekt nutzt SQLite fuer lokale Langzeit-Memory.`
- Confirm `.cowork/DRAFT_KNOWLEDGE.md` contains one `[memory]` candidate.
- Repeat the sentence and confirm the file does not gain a duplicate line.
- Send a normal question and confirm it is not captured.
- Send a fake secret assignment or prompt-injection sentence and confirm it is not captured.

## Session recall

- Complete a session containing a distinctive phrase and start another session.
- Ask the model to find that phrase in old conversations.
- Confirm `SessionSearch` returns the earlier session title, timestamp, role, and matching content.

## Slash commands

- Type `/` and confirm autocomplete and the command palette show the same registered set.
- Run `/help` and confirm it lists the registry plus enabled plugin skills.
- Run representative synchronous and asynchronous commands: `/mode plan`, `/permissions`, `/memory`, `/doctor`, `/terminal-setup`, and `/settings`.
- Force an asynchronous backend failure and confirm the UI reports `failed` rather than `executed`.
- Run an unknown command and confirm the UI points to `/help`.

## Release gate

- `npm run typecheck`
- `npm run test:ci`
- `cargo test --lib`
- `npm run lint:ci`
- `node scripts/validate-agent-discipline.mjs --runs 50`
- `graphify update .`
