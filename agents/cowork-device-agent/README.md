# Cowork device and Windows executor agent

`cowork-device-agent` makes a device available to the self-hosted control plane
through an outbound-only authenticated connection. It has two distinct modes:

- `personal_device`: the owner's laptop/desktop, paired with the local daemon;
- `managed_windows`: a dedicated administrator-managed Windows executor in a
  granted pool.

The server never opens an inbound port on the device. The agent connects to the
canonical HTTPS origin and upgrades the authenticated device channel.

## Common setup

1. Build the agent with the same release/contract line as the server:

   ```powershell
   cargo build --release -p cowork-device-agent
   ```

2. Create/register the executor and obtain a revocable agent credential through
   the server administration flow described in
   [Server deployment](../../docs/SERVER_DEPLOYMENT.md).
3. Store the credential in a user/service-readable file outside the repository.
4. Copy `.env.example` to an untracked configuration and set the canonical
   server URL, executor UUID, token file, kind and exact capabilities.
5. Start the agent interactively, verify registration/heartbeat, then install it
   under the appropriate per-user or managed service lifecycle.

Never advertise a capability the host cannot execute and clean up. Routing
trusts the capability contract and then revalidates every operation at runtime.

## Personal device

Required relationships:

- `COWORK_AGENT_KIND=personal_device`
- `COWORK_EXECUTOR_ID` equals `COWORK_DAEMON_DEVICE_ID`
- daemon IPC endpoint/token point to the already running local daemon
- the executor is selectable only by its owner

Remote-control policy is one of:

- `off`: no remote GUI control;
- `confirm_each_session`: local confirmation for every session (default);
- `unattended`: no local confirmation, only for a deliberately managed device.

On interactive Linux desktop streaming, `DISPLAY`, `xdotool`, ImageMagick
`import`, a clipboard tool and a local confirmation dialog implementation must
be available. Advertise `desktop.windows` or `desktop.linux`, never both.

The personal agent delegates durable execution to the daemon. It does not copy
the owner's arbitrary local files to the server; private files enter a remote
Run only through an explicit encrypted snapshot.

## Managed Windows executor

Use a dedicated VM/PC and Windows account. Do not reuse an employee's profile.
The operator is responsible for Windows, RDS and Microsoft 365 licensing and
for keeping one interactive desktop session available.

Minimum operating rules:

- one interactive executor runs at most one Run concurrently;
- use a dedicated `COWORK_AGENT_WORKSPACE_ROOT` with no personal files;
- configure a pool ID and grant the pool only to intended teams/projects;
- keep Office macros, add-ins and external data disabled unless an explicit
  signed-content policy permits them;
- allow only outbound HTTPS/WebSocket access to the canonical server;
- run the agent inside the same interactive session used by Office/UI
  automation, with a supervisor responsible for restart and health;
- after every Run, remove the workspace, clipboard contents, temporary files
  and remaining Office processes before returning healthy.

Typical capabilities:

```text
model.external,files,shell,git,web.fetch,office.ooxml,office.microsoft,desktop.windows,crew.python
```

`office.microsoft` means installed Word, Excel and PowerPoint through the
interactive COM/GUI adapter. Linux server Office uses separate
`office.ooxml`/`office.libreoffice` capabilities. Microsoft does not recommend
unattended Office automation as a server service; this project intentionally
models it as best-effort interactive executor automation.

## Crew and MCP

`crew.python` requires absolute paths to the verified pinned Python executable
and adapter script. Optional executor-local MCP bindings use
`COWORK_MCP_BINDINGS_FILE`; list responses expose only names/capabilities, not
commands, arguments or environment values. The agent invokes configured stdio
tools without a shell and keeps each Crew agent on its explicit MCP allowlist.

## Health and recovery

- A disconnected executor stops receiving leases and becomes unavailable.
- Reconnect uses the same executor identity and revocable credential.
- An expired lease does not cause an unsafe tool action to be replayed.
- Unexpected Office dialogs become Run events and may pause for an authenticated
  manual takeover.
- Repeated cleanup or Office health failures must quarantine the executor until
  an operator repairs it.

## Validation

```powershell
cargo test -p cowork-device-agent
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-personal-device-bridge.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows-desktop-relay.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows-office.ps1
```

Physical Microsoft Office and GUI acceptance must run on every supported image
and Office update channel before that pool is made available to users.
