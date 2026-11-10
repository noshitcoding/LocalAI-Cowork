# Native V2 device agent

The native executable is `cowork-device-agent-v2` on Linux and
`cowork-device-agent-v2.exe` on Windows. Its source package is
`services/device-agent`. It connects directly to the V2 device protocol and
executes on the paired device; no V1 daemon, named pipe or Unix IPC bridge is
required. The server continues to own scheduling, device credentials, run state
and device remote-control policy. Model and tool credentials belong to the
device operator.

Personal devices work on Windows and Linux. Managed Windows agents belong to an
explicitly registered Windows pool. A managed Linux pool is not a registration
kind in this release. The agent supports one concurrent run; pairing metadata
with a different concurrency limit is rejected with an instruction to pair the
device again. The configurator does not rewrite registered device capabilities
to hide an unsupported configuration.

## Installable agent package

Extract the package for the device's operating system, pair the device in the
V2 UI and download the secret profile. Import it with the bundled
`tools/device_agent/configure.py` helper or the helper shipped alongside the
agent executable. The helper requires Python 3. It discovers the executable in
the same directory or the bundle's adjacent `bin` directory; an explicit
`--agent-binary` is also accepted.

The full Windows and Linux command examples are in
[`tools/device_agent/README.md`](../tools/device_agent/README.md). Importing is a
local operation only. It writes a new private directory, never starts a service,
never contacts the server and never installs an operating-system dependency.
Add `--check` for a read-only check of the exported profile and selected paths.

The exported profile contract is unchanged:

```json
{
  "format": 1,
  "protocol_version": 2,
  "schema_version": 2,
  "server_url": "https://cowork.example",
  "executor_id": "<paired-device-uuid>",
  "registration": "<exact DeviceExecutorRegistration object>",
  "token": "<secret device credential>"
}
```

`registration` is an object in an actual exported profile. It contains schema
and protocol version 2, the executor ID, actual kind and pool, display name,
exact capability descriptors, labels, remote-control policy and
`max_concurrent_runs: 1`. The agent uses that exact registration rather than
inventing capabilities from the process environment.

## Runtime configuration

The native command is:

```text
cowork-device-agent-v2 --config <absolute-private-agent.json>
```

`agent.json` has this private, local format:

| Field | Meaning |
| --- | --- |
| `format` | Configuration format `1`. |
| `server_url` | Canonical HTTPS server origin; loopback HTTP is accepted for local use. |
| `registration` | Exact exported protocol-2 registration object. |
| `agent_token_file` | Absolute path to the copied private device credential. |
| `model.base_url` | Explicit device-owned model API base URL. |
| `model.model_name` | Exact model name at that endpoint. |
| `model.api_key_file` | Copied private model key file, or `null` for keyless local models. |
| `workspace_root` | Dedicated local run workspace. |
| `toolbox_data_dir` | Dedicated local toolbox state directory. |
| `github_token_file` | Copied private GitHub key file, or `null`. |
| `mcp_bindings_file` | Copied private MCP bindings JSON file, or `null`. |
| `rfb_endpoint` | Reserved optional field; generated configurations use `null`. |

The generated `.env` contains only the configuration-file path. `start.py`
passes that path to the native executable, excludes inherited `COWORK_*`
variables and puts no API key in arguments. A device can use its own LiteLLM
connection independently of the team's configured LiteLLM token. There is no
additional internal team token-budget ledger.

Windows profile files have a protected current-operator/SYSTEM DACL. Linux
directories use 0700 and files use 0600. Model and GitHub input keys are copied
into these private files so deleting the original input does not break the
agent. The agent configuration, exported profile, keys and local run data must
remain outside source archives and public delivery metadata.

## Tools and desktop operation

The device's exact declared capabilities must match its available native
dependencies. Workspace and toolbox directories default to private
subdirectories under the new profile. Explicit existing dedicated directories
are accepted. Native execution requires the explicit local
`--allow-native-tools` flag and writes `native_owner_rights: true` into the
private `toolbox/toolbox.json`. Model execution and workspace file tools can be
configured without that flag. Concrete effects require server approval;
personal devices also require an explicit local decision. The running agent
prints only the pending request and run identifiers. The local setup window
shows the exact action without putting document text or native paths into logs. In another terminal, resolve it using the selected
agent executable:

```text
cowork-device-agent-v2 consent --data <private-workspace-root> --request <request-uuid> --decision approve
cowork-device-agent-v2 consent --data <private-workspace-root> --request <request-uuid> --decision reject
```

`--data` is the configured workspace root, defaulting to `<output>/workspaces`.
On Windows use the selected `.exe` path. The request is bound to the concrete
action and expires; a server decision does not create its local decision file.
Managed Windows runs use the server's human approval.

Declare model capabilities as `model.external` or `model.chat`. The other
supported capability pairs are `tool.terminal` / `terminal`, `tool.browser` /
`browser`, `windows.office` / `office`, `tool.github` / `github`, and
`tool.mcp.invoke` / `mcp`. Select actual executables using repeated
`--program alias=absolute-file` arguments. Terminal requires at least one
selected program; browser requires `chromium`; Office requires `python` and its
selected Office modules, with `libreoffice` additionally needed for conversion.
GitHub requires `gh` and the device's own GitHub key. MCP requires the private
bindings file and exposes the model-facing `mcp_call` function. There is no V1
Crew capability.

The Windows interactive desktop uses the actual `desktop.windows` capability
and the paired personal remote-control policy. A session is not automatically
confirmed by importing a profile. Linux desktop streaming uses the explicit
`desktop.linux` capability in the operator's own active, unlocked logind session.
X11 uses libX11/libXtst; optional xclip provides clipboard integration. Wayland
uses ScreenCast/RemoteDesktop portals and the portal-granted PipeWire stream.
The operator must install system Python3, PyGObject with Gio/GLib/Gst/GstVideo,
the GStreamer PipeWire source and a working desktop portal backend. These are
checked without opening a capture session. Profile import installs none of them.

Each Wayland stream opens a fresh portal session and asks the compositor to grant
one monitor and, for control, keyboard/pointer devices. Local Cowork consent and
compositor consent both apply, including when the Cowork policy is `unattended`.
No persistent portal permission or restore token
is stored. RemoteDesktop clipboard support is optional and is used only when
the portal explicitly grants it. There is no ambient clipboard fallback.
Locking, loss of the local session or portal owner, cancellation and connection
loss terminate the owned helper and stream. The native agent kills/reaps its
original helper and joins its IO worker before releasing the desktop gate;
parent death also terminates the helper. Changed monitor geometry requires a
new explicit session. The configurator does not activate an RFB proxy or install
a VNC server, and Wayland refusal does not switch to X11.

Current source checks do not qualify a physical compositor, its consent dialog,
PipeWire frames, input, clipboard or lock/revocation behavior. Previously built
agent archives do not automatically include this Wayland source extension.

## Device-owned MCP bindings

`--mcp-bindings-file` accepts a JSON array of connections with unique `name`
values. Each connection uses `transport: "streamable_http"` with an HTTPS or
loopback `endpoint`, or `transport: "stdio"` with an absolute existing `command`.
An HTTP connection can carry a private `bearer`; stdio uses private
`environment` values and requires a selected `python` helper program. Optional
`allowed_tools` narrows the selected catalogue
and `catalog_sha256` pins its digest. An empty allowlist exposes the local
catalogue, with approval still required for every action. Select at most eight
connections. The exact private
array is copied into the profile; it is never sent to the team server as part of
the public pairing metadata.

```json
[
  {
    "name": "device-mcp",
    "transport": "streamable_http",
    "endpoint": "http://127.0.0.1:8080/mcp",
    "allowed_tools": ["echo"],
    "catalog_sha256": null
  }
]
```

Start and stop the executable explicitly. Neither profile import nor package
extraction registers a background service or schedules automatic startup.
Device approval, installed tool dependencies, the real model endpoint and any
physical desktop session remain local operator inputs.

## Concrete V1-compatible device adapters

The new interop source implements operations behind the original capability
families; protocol-1 device credentials, old user identities and running V1
processes are never adopted. The exported registration stays protocol 2.

| Capability | Explicit local selections and actual execution |
| --- | --- |
| `files` | Selected `python`: pinned read/edit/mkdir/copy/move/delete/glob/grep in this run. Reads return explicit line ranges and a `has_more` flag. |
| `shell` / `git` | Selected `python` plus `shell` / `git`: one approved original native command. Git refuses linked foreign worktrees, config includes, external filters and indirect program flags. |
| `web.fetch` | Selected `python` plus `--web-search-url https://host/search?q={query}`: HTTPS fetch/search, pinned public DNS addresses, no redirects or ambient credentials. Private addresses require an explicit `--allowed-host`. |
| `office.ooxml` | Selected `python`; operator-installed python-docx, python-pptx, openpyxl and reportlab as needed: structured DOCX/PPTX/XLSX/PDF, exact XML placeholder templates, or separate native/template outputs. |
| `office.libreoffice` | Selected `python` plus `libreoffice`: original input preflight and a fresh private user profile for each conversion. |
| `office.microsoft` | Windows, selected `python`, `powershell`, `word`, `excel`, `powerpoint`, an active unlocked operator session: the thirteen original Word/Excel/PowerPoint action pairs, direct `windows_office` requests and an optional PDF preview. |
| `browser.headless` / `browser.visible` | Selected `chromium`, and for visible mode `python` with Playwright: headless adapter or a newly owned visible context. Visible contexts never restore an old profile or adopt a foreign browser; screenshots become SHA-bound run artifacts. |
| `model.ollama` | Explicit model URL/name with `--model-api ollama --model-implementation ollama`: native `/api/chat`, real usage counters and tool-call translation. |
| `model.vllm` | Explicit model URL/name with `--model-api openai --model-implementation vllm`: the device's selected OpenAI-compatible deployment; the descriptor does not attest a remote server's implementation. |
| `crew.python` | Selected `python`, an explicitly installed CrewAI distribution, own model endpoint and the distribution version/source pin: actual sequential/hierarchical CrewAI Agents/Tasks, device-owned model/tool RPC, original DAG/results and per-agent configured tool/MCP grants. |

Direct Office-only profiles may omit model URL/name. Native programs still need
`--allow-native-tools`; this authorizes the selected programs to use the
operator's OS rights and is not filesystem isolation. Microsoft Office runs on
a fresh private desktop/STA and binds Native Object Model objects only through
windows of the held original child. It never uses CreateObject/GetActiveObject,
global clipboard clearing or global Office process termination. Original input
bytes, program hashes, parameters and fresh output destinations are pinned.
Preflight rejects macros including XLM, active controls, embedded objects,
external relationships, DDE and active fields before opening a document.
Legacy XLS NAME/formula records require safe re-encoding before native opening:
opaque RPN is not treated as XLM-free based on a text scan. Policy receipts
identify bounded static inspection and do not assert runtime qualification.
Unexpected dialogs/handoff, interrupted IO or uncertain effects stop without
replaying the operation. Unsupported security content is not enabled as a repair.

The setup window can inspect the selected installed CrewAI distribution without
importing/executing CrewAI. It hashes its actual import tree and distribution
metadata, excluding console launchers that the bridge does not execute. This is
a local distribution pin, not qualification of every transitive Python package.
No dependency, model or application is downloaded. Original foreign profile
references require an explicit `--crew-profile-binding reference=own-model-name`;
they cannot supply another endpoint or adopt its credentials. Legacy governance
restrictions are intersected with the run's workspace/tool grants. Native shell,
Git and external tools are refused when their OS execution cannot preserve an
original per-path restriction; the adapter never discards that restriction.

The device UI provides direct Microsoft, OOXML/template/parallel, LibreOffice
and full Crew-definition forms. It saves the original start body without a token,
bound to the actual signed-in actor/server/project/thread. Lost acknowledgement
or browser restart exposes the same original POST/key for reconciliation; a new
request is offered after acknowledgement. Selected project-file IDs and versions
are frozen by the server; their names are the actual run-relative source paths.
Device checkpoint reads/downloads are separate from private-server continuation;
a device `safe_to_resume` marker never authorizes replay.

The private actor workspace uses the same forms with the actual actor, private
project/thread and personally owned device. Its file choices come from committed
private snapshot metadata: file ID, snapshot ID and that file's actual revision.
The selected revision map is checked without a fallback, and every authenticated
chunk and the complete file SHA/size are verified before the frozen snapshot is
committed. Different historical snapshots remain separate original file IDs;
the UI does not invent old revisions under a current file ID.

New device starts store their exact initial queued or waiting-for-snapshot response
in the same transaction as the run. Reconciliation matches the original sender
digest/key and current actor/project/run read rights before new creation checks,
so a later project revision or completed run cannot replace the original ACK.
The response grants no execution or resume permission. Older runs without this
original receipt are not backfilled from their mutable payloads.

Large original actions/results use complete SHA/size-bound run artifacts instead
of truncated JSON. Remote approval requires explicitly reading/verifying the
original action; the local window verifies its own private original file. Action
arguments, file contents and helper paths are not printed to stderr. All owned
helper supervisors remain in the run registry and are canceled/reaped on run
cancellation, future loss or agent shutdown. Restart cannot reconstruct their
in-memory effect permits or adopt a previously launched process.

This interop source is separate from the earlier native-build/demo receipts.
Its final Windows/Linux integration builds, real Office edition/bitness/dialog
behaviour, visible Chromium/locked-session behaviour, installed CrewAI execution,
external-runtime inventory and newly produced installable package still require
qualification. Existing packages and their source hashes remain historical bytes.

## Original MCP tasks and human responses

Select the device's own MCP bindings file and, for stdio, its absolute selected
Python/program paths. HTTP uses the configured device bearer; stdio uses the
explicit local command/arguments/environment. No Team, server-owner or private
server credential is inherited. The supervised stdio bridge retains one newly
owned MCP process and its original pipes from initialize/discovery through call,
task result and closure; it does not rediscover by replacing the process.

The actual negotiated 2025-11-25 server task capabilities govern optional/required
task calls. A real original task ACK starts bounded get/result/cancel operations;
it is never marked as a completed tool result. Each original operation and reply
is encrypted in `<toolbox_data_dir>/mcp-journal` with an independently provisioned
OS-keystore key. Windows needs the configured operator's credential store; Linux
needs that user's available Secret Service session. A locked/unavailable key
blocks sending without a plaintext fallback or replacement key.

The run's existing Requests panel now presents device task forms, cancellation
and two-phase model review. Inspect the original server request, enter/review
form values and accept/decline/cancel. Sampling first reviews editable text,
then explicitly approves one use of the device's original current model/role;
personal-device local consent is additionally required. Review the actual
generated response and separately release or withhold it. The remote MCP server
cannot choose another model, key, context or tools. Runs without an actual held
model context advertise only form interaction. Ordinary and sampled model
admissions share the configured original device step limit.

Task cancellation is requested through the corresponding run input and is sent
before the next available original task RPC. Canceling the entire run interrupts
the owned local process tree, but does not assert that the remote operation had
no effects. Failed/cancelled tasks still retain their original tasks/result error
or result. Missing acknowledgements, transport loss, invalid metadata and unknown
effects remain unconfirmed and never replay the original tool or model request.

Local operator diagnostics (private configuration file, exact original run and
source UUID from run events) are provided by:

```text
cowork-device-agent-v2 mcp-journal-inspect CONFIG RUN_UUID SOURCE_UUID
cowork-device-agent-v2 mcp-journal-confirm-key CONFIG
```

Inspection is read-only and decrypts original evidence for the current OS owner;
it grants no execution/resume rights. Treat its output as private user data.
`confirm-key` is an explicit recovery of the **same** durably reserved OS key
after an unconfirmed first key publication. It verifies that original key and
publishes its confirmation proof; it cannot create/adopt another key, reactivate
a tool or replay IO. Preserve the journal, candidate locator and original OS key
together until the operator has separately resolved retention/recovery. A real
new device package, native keystore operation and selected external MCP servers
still require their platform/runtime delivery evidence.
