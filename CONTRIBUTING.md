# Contributing

Thanks for taking a look at Open Cowork. The project is still early, so focused contributions are the most useful.

## Before You Start

- Check the existing documentation in `README.md` and `docs/`.
- Keep pull requests small enough to review.
- Prefer existing UI, store, and Rust command patterns over new abstractions.
- Avoid committing generated build output.

## Development Setup

```powershell
cd app
npm install
npm run doctor
npm run tauri dev
```

## Validation

Run the relevant checks before opening a pull request:

```powershell
cd app
npm run lint
npm run typecheck
npm run test:ci
npm run build
```

For Rust changes:

```powershell
cd app/src-tauri
cargo check
cargo test
cargo clippy -- -D warnings
```

For desktop behavior changes, run or update:

```powershell
cd app
npm run smoke:desktop
```

## Pull Request Checklist

- Describe the user-facing change or bug fix.
- Mention any local checks you ran.
- Add tests when changing shared behavior, persistence, permissions, or tool execution.
- Update docs when setup, commands, architecture, or public behavior changes.

## Security And Privacy

Open Cowork can interact with local files, terminals, MCP servers, and model providers. Treat changes in those areas as security-sensitive. Use explicit approval flows for risky operations and avoid logging secrets, API keys, or full private file contents unless a feature explicitly requires it.
