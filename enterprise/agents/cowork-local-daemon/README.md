# Cowork local daemon

`cowork-local-daemon` is the per-user durable runtime for standalone desktop
work. It owns local Runs, schedules, events, encrypted model bindings and SQLite
state. Closing the desktop window does not cancel a daemon-owned Run.

## Packaged behavior

The signed desktop package:

1. verifies the bundled daemon manifest and every declared sidecar hash;
2. copies a versioned binary into stable per-user application data;
3. creates a 256-bit IPC token with user-only permissions;
4. registers startup at user login without administrator rights; and
5. connects through a per-user Windows Named Pipe or Unix Domain Socket.

One process lock is allowed per user. On restart, active unsafe tool steps are
marked `interrupted` for user review rather than replayed.

## Source run

Copy `.env.example` to an untracked local file or set equivalent environment
variables, then:

```powershell
cargo run -p cowork-local-daemon
```

Important settings:

- `COWORK_DAEMON_DEVICE_ID`: stable device UUID; it must match the personal
  device agent's executor ID.
- `COWORK_DAEMON_USER_ID`: local contract-valid UUID.
- `COWORK_DAEMON_DATA_DIR`: optional SQLite, token, lock and artifact root.
- `COWORK_DAEMON_IPC_ENDPOINT`: optional explicit pipe/socket.
- `COWORK_DAEMON_IPC_TOKEN_FILE`: token path shared with the desktop bridge and
  personal device agent.
- `COWORK_MODEL_BASE_URL`, `COWORK_MODEL_NAME`, `COWORK_MODEL_API_KEY`: fallback
  local/OpenAI-compatible model binding.
- `COWORK_CREW_PYTHON`, `COWORK_CREW_SCRIPT`: absolute paths to the verified
  pinned Crew adapter when Crew execution is enabled.

Do not put production secrets into `.env.example`, command-line arguments, Run
input or synchronized metadata. The desktop's native bridge resolves provider
credentials from the OS credential store and sends them only to the daemon's
encrypted binding store.

## Data and offline behavior

- SQLite and local artifacts stay in the per-user data directory.
- Local project paths and active terminals are device-specific and are never
  synchronized as portable metadata.
- Provider profile names/model defaults can sync, while endpoint availability
  and credentials remain bound per device.
- A server-originated personal-device Run can execute only while the daemon and
  outbound device agent are available. A local standalone Run needs no server.
- Shutdown checkpoints the Run where safe. An unsafe action is never silently
  repeated after restart.

## IPC safety

The protocol is bounded newline-delimited JSON RPC. Requests require the local
token, method names and payload sizes are validated, and secrets are resolved
by native code rather than exposed to the React layer. Never place the socket
on a network filesystem or share the token between OS users.

## Validation

```powershell
cargo test -p cowork-local-daemon
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-local-daemon-lifecycle.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-local-daemon-model-listener.ps1
```

Additional Crew, Office and personal-device bridge tests live under `scripts/`.
Run them with disposable workspaces and the prerequisites described in each
script.
