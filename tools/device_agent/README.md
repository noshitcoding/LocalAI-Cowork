# Standalone V2 device agent

`cowork-device-agent-v2` connects an explicitly paired Windows or Linux device
to a LocalAI Cowork V2 server. It runs independently of the desktop app and of
the V1 daemon. A personal device uses `personal_device`; a team-managed Windows
device uses `managed_windows` and its registered pool. The current agent accepts
one concurrent run. Pairing metadata and capabilities are never silently changed.

Extract the matching Windows or Linux agent package on the device. The package
contains the executable, `configure.py`, `ui.py`, `windows_private.py` and the
read-only `crew-runtime-probe.py` metadata entry point. Python 3 with Tk is
needed to import a profile and use the convenience launcher. The agent itself is
a native executable. No global installation or V1 runtime is required.

The composer distinguishes granted folders with a qualified isolated process
environment from native programs with the signed-in user's permissions. Native
shells and GUI applications are not confined by the folder picker. Changing the
mode clears the previous native program selection; the run freezes the existing
`environment` and `native_tools` fields before dispatch.

Use `cowork-device-agent-v2 --local-capabilities` for nonsecret detection, or
`--config ABS/agent.json --local-capabilities` to inspect the independently
configured local rights. Detection does not authorize tools or qualify a runtime.

## Optional disposable Windows test computer

`ui.py` includes a Windows test computer tab. Open the host agent profile, export
a **different** personal-device pairing for the guest, then select only terminal,
Windows desktop and optional external-model capabilities. Remote control must be
off or confirm each session. The guest uses the existing protocol-2 device queue,
not a separate executor API. A reachable HTTPS server is required; the host's
localhost is not the guest's localhost.

Choose explicit read-only or writable folders. Writable shares change host
files permanently. Drive roots, overlapping grants, credential/bootstrap paths,
symbolic links and reparse points are refused. Choose a bounded project folder;
preparation checks up to 100,000 entries. Only explicit networking and guest-tool
permission checkboxes enable those features. Enabled networking includes local
networks; Windows Sandbox's switch does not enforce a server-only allowlist.
Clipboard, camera, microphone,
printer and vGPU sharing remain disabled. The executable and profile are copied
to private guest storage; a visible guest console approves exact actions using
the existing local consent command. Host applications are not installed there.

The same preparation is available from the CLI:

```text
python windows_sandbox.py --discover
python windows_sandbox.py --profile C:\Operator\guest-device.json --agent-binary C:\Operator\cowork-device-agent-v2.exe --output C:\Operator\new-vm --host-executor-id HOST-UUID --read-only C:\Projects\Input --read-write C:\Projects\Output --network --allow-guest-tools
```

Preparation writes a new private bundle without starting it. Add `--launch` to
explicitly launch the newly prepared VM. The native executable validates the
guest configuration before connecting. The launch manifest binds exact payload
bytes and is checked before launch. This does not constitute physical runtime
qualification: enable Windows Sandbox on a supported Windows edition with
virtualization, then verify the guest, pairing, approval and file workflow on the
actual host. Only one Windows Sandbox instance can run; closing it discards guest
state. Revoke its pairing and remove the private bootstrap bundle when finished.

## Local setup and approval window

Open `python ui.py` beside the native executable. The setup tab imports the
exported profile into a new private directory and accepts the device's own
model URL/name/key file, selected program aliases, GitHub key and MCP bindings.
No connection starts until **Start agent** is selected. After import,
`python <output>/start-gui.py` reopens that private profile. The window shows
the current owned agent and concrete local requests, their run, action and
expiry. Approve/reject calls the native one-shot consent command. Stop writes
an instance-bound private request; the agent cancels its lease and stops its
owned tool/desktop work. Closing an agent started by this window waits for
graceful stop. There is no local HTTP listener or remote consent API.
Python without Tk can use the native executable and consent CLI below.

## Pair and configure

1. In **Geräte & Android → Eigenen Geräteagent koppeln**, select the actual
   device kind and capabilities, then download the secret device profile.
2. Select that profile and, for model/Crew execution, an HTTPS or loopback
   device-owned model endpoint/name. Direct Office-only agents can omit both.
   A device-owned model key is optional for a keyless local model.
3. Run the configurator into a **new** private directory whose parent exists:

```powershell
python C:\Operator\Agent\configure.py `
  --profile C:\Operator\device-profile.json `
  --output C:\Operator\device-config `
  --model-url https://models.example/v1 `
  --model-name my-model `
  --model-key-file C:\Operator\model-key.txt
```

```sh
python3 /home/operator/agent/configure.py \
  --profile /home/operator/device-profile.json \
  --output /home/operator/device-config \
  --model-url http://127.0.0.1:11434/v1 \
  --model-name my-local-model
```

The helper discovers `cowork-device-agent-v2.exe` or `cowork-device-agent-v2`
beside itself or in the bundle's adjacent `bin` directory. Use
`--agent-binary <absolute-path>` to select another already built V2 executable.
The Linux executable must retain its executable bit; if an extraction tool
removed it, explicitly run `chmod +x /home/operator/agent/cowork-device-agent-v2`.
`--check` validates the same selections and prints nonsecret metadata without
creating files, contacting a server or starting the agent. It can check a profile
before the executable is available.

4. When ready, start it explicitly:

```powershell
python C:\Operator\device-config\start.py
```

```sh
python3 /home/operator/device-config/start.py
```

The launcher invokes only the selected native executable with
`--config <absolute-path-to-agent.json>`. Credentials are never command-line
arguments. Stop the agent with Ctrl+C. Creating a configuration does not start
an agent, register a service, download dependencies or contact any endpoint.

## Private files and local tools

The import creates `agent.json`, `registration.json`, `configuration.json`,
`agent-token.txt`, a path-only `.env` and `start.py`. Model and GitHub keys are
copied to private `model-key.txt` / `github-key.txt` files when selected; the
original key-file path is not retained. The registered executor's exact
capabilities, labels, ID, kind and remote-control policy remain unchanged.

Workspace and toolbox state default to new private `workspaces` and `toolbox`
subdirectories. Both personal and managed Windows devices execute locally;
personal devices require no V1 IPC endpoint. Select an existing dedicated local
directory using `--workspace` or `--toolbox-data-dir` when needed. Only advertise
tools whose dependencies are installed on that device. Selected state
directories must already be owned and private; their permissions are not
silently repaired. Native programs execute
with the operator's operating-system rights, so pair the agent only with a
server and workspace you intend to authorize.

Optional device-owned services:

- `--allow-native-tools` explicitly authorizes local native execution with the
  operator's OS rights. Without it, configure model execution and workspace file
  tools only. Declare the native capabilities while pairing as well.
- `--program python=C:\Operator\Runtime\python.exe` selects an actual native
  executable. Repeat `--program` for each alias. Browser needs `chromium`;
  Office needs `python` and its selected Office modules, with `libreoffice`
  additionally required for conversion; GitHub needs `gh` and a key.
- `--github-key-file <absolute-file>` copies the device's own GitHub credential.
- `--mcp-bindings-file <absolute-json-file>` selects private device-local MCP
  bindings. The JSON array is copied into the private configuration directory.

Supported capability names are `model.external` / `model.chat`,
`tool.terminal` / `terminal`, `tool.browser` / `browser`, `windows.office` /
`office`, `tool.github` / `github`, and `tool.mcp.invoke` / `mcp`. A Windows
interactive desktop uses `desktop.windows` and the paired remote-control policy.
`desktop.linux` supports a local X11 display with libX11/libXtst and an active,
unlocked logind session (`DISPLAY`, `XDG_SESSION_ID`, `XDG_SESSION_TYPE=x11`).
It captures the X11 framebuffer and uses XTest for keyboard, pointer and wheel.
Optional `xclip` provides X11 clipboard integration; owned foreground clipboard
children are closed when their stream ends. Wayland uses a new explicitly
granted ScreenCast/RemoteDesktop portal session and its PipeWire stream. Supply
system Python3, PyGObject Gio/GLib/Gst/GstVideo, GStreamer pipewiresrc and the
desktop's portal backend. Every stream requires the compositor's current grant;
optional clipboard access also needs its explicit grant. No restore token or
ambient clipboard is used. Only the same operator's active, unlocked Wayland
logind session is accepted. Cancellation, locking or portal/connection loss
closes the session and the native agent terminates its original helper. A refused
Wayland session does not switch to X11. No packages are installed. Windows captures
the virtual monitor desktop, including negative monitor origins. Both platforms
retain separate view/control consent, lease fencing and held-input release.
Physical Wayland acceptance and rebuilt current agent packages remain separate
from the source implementation; older binaries do not contain this extension.

Native programs and `native_owner_rights` are written to the private
`toolbox/toolbox.json`. An existing toolbox configuration is never overwritten;
its selected programs and rights must match the new profile import. Per-action
server approval still applies after granting native-tool access. Personal
devices additionally ask for a local decision on each concrete action. The agent
prints the exact request and command; use another terminal to approve or reject:

```text
cowork-device-agent-v2 consent --data <private-workspace-root> --request <request-uuid> --decision approve
cowork-device-agent-v2 consent --data <private-workspace-root> --request <request-uuid> --decision reject
```

The default data path is `<output>/workspaces`. On Windows use the selected
`cowork-device-agent-v2.exe` path. No server decision creates this local approval
on the operator's behalf. Managed Windows runs use the server's human approval.

The model-facing MCP function is `mcp_call`; the registered capability remains
`tool.mcp.invoke` or `mcp`.

The server must be a canonical HTTPS origin with no path or embedded credential;
HTTP is accepted only for `localhost`, `127.0.0.1` or `::1`. Model endpoints may
include their API base path and follow the same HTTPS/loopback HTTP rule. A model
or GitHub key remains device-owned; team tokens are not exported to this agent.

Windows creates a protected DACL granting the current operator and SYSTEM.
Linux creates directories with mode 0700 and files with mode 0600. Existing
files are never overwritten. Keep the exported profile and private output
directory outside Git and shared folders. Remove the exported profile yourself
when it is no longer needed. Revoking the agent credential on the V2 server
removes that device's future access.

For the configuration contract and server/device operation, see
[`docs/DEVICE-AGENT-V2.md`](../../docs/DEVICE-AGENT-V2.md).

## Interop setup

The actual adapter/dependency table and original-operation rules are maintained
in [DEVICE-AGENT-V2.md](../../docs/DEVICE-AGENT-V2.md#concrete-v1-compatible-device-adapters).
No capability is renamed to conceal a missing runtime. Select concrete local
programs (`python`, `shell`, `git`, `libreoffice`, `chromium`, or Windows
`word`/`excel`/`powerpoint`/`powershell`) and the operator-installed Python packages.
The selected native programs run with the operator's OS rights after approval.

For native Ollama use a base URL such as `http://127.0.0.1:11434`, together with
`--model-api ollama --model-implementation ollama`. For a deliberately selected
vLLM endpoint use `--model-implementation vllm`; this is a configured adapter
choice, not independent evidence of a remote deployment's implementation.

Crew setup accepts `--crewai-version` and `--crewai-source-sha256`. The local
window's **Inspect installed CrewAI sources** button reads the selected Python
installation's actual CrewAI import-tree/metadata digest; it runs no Crew/model or
tool. Optional `--crew-profile-binding original-reference=own-model-name` resolves
an original logical model reference at this same device-owned endpoint. It never
copies a foreign profile or credential. `--web-search-url` configures a credential-
free HTTPS query endpoint with one `{query}` placeholder. Each `--allowed-host`
is an explicit operator authorization for that hostname's private addresses.

The new direct Office/Crew start forms and native helpers are source additions.
Old binaries/build or demo receipts do not cover them. Real Microsoft Office,
Chromium and CrewAI qualification and refreshed Windows/Linux packages remain
separate work after final source integration.


## Native build and packaging

Build on each actual Windows/Linux x64 host using the same frozen checkout.
The native builder binds the locked release Cargo artifact, complete source
hashes and actual PE/ELF headers, and runs no tests or services:

```text
python tools/device_agent/build.py --output-dir ABS/new-native-agent --target-dir ABS/own-cargo-cache --offline
```

Omit `--offline` when the locked dependencies have not yet been cached. A source
change during the build refuses publication of its receipt. Collect the two
native executables and their original `receipt.json` files without changing
their bytes, then use the explicitly requested quick packaging path:

```text
python tools/device_agent/package.py --build-only --windows ABS/windows/cowork-device-agent-v2.exe --linux ABS/linux/cowork-device-agent-v2 --windows-receipt ABS/windows/receipt.json --linux-receipt ABS/linux/receipt.json --output ABS/new-agent-delivery
```

Both platform receipts must bind the current identical complete source inputs,
version and actual executable hashes. The bundle includes the setup/consent UI,
original public build receipts, sources, per-platform SBOMs and an independently
checked ZIP. `build_only` remains recorded; this command does not install,
sign, publish, or claim physical Office/desktop/MCP operation. The separate
original demonstration-receipt path remains available.
