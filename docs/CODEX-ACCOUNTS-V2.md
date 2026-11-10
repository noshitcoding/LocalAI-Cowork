# V2 Codex account connections

V2 manages owner-private and actor-private Codex profiles separately. Profiles
contain only names and priority. Account authentication remains in the operating
system keyring, using an isolated `CODEX_HOME` per owner and profile. V1/global
Codex homes are never adopted. No runtime is downloaded or installed by this API.

Configure `codex-runtime.json` inside the actual V2 daemon data directory:

```json
{"manifest":"C:/absolute/path/to/codex/runtime-bundle-manifest.json"}
```

The manifest uses the V1 bundle format, pinned to version `0.147.0` and protocol
`app-server-0.147.0`. Its relative binary and license paths must remain within the
manifest directory, without symlinks/reparse points. Binary SHA256 and optional
size, plus the license hash when present, are verified before process startup.
The runtime must already be extracted. An unavailable system keyring is reported;
there is no plaintext `auth.json` fallback. Headless Linux needs a working OS
keyring/Secret Service; account setup alone cannot make that service available.

Owner endpoints live at `/api/v2/codex`; true actor-session endpoints at
`/api/v1/codex`. Both provide `/runtime`, `/accounts`, `/accounts/{id}`,
`/accounts/{id}/status`, `/models`, `/login`, and `/logout`. Login and logout have
durable one-shot operation receipts. An unresolved dispatched action is not
automatically replayed. Changing profile metadata uses revision CAS.

The broker implements account connections, browser/device login, logout, model
discovery, reasoning choices and quota reads. An account/model/reasoning choice
is materialized by `/accounts/{id}/model-profile` into an immutable scoped model
profile with an exact operation receipt. Chats, workflow roles, tasks and library
templates can select that profile. Generic provider editing cannot forge or
change a Codex account binding. Explicit deletion of an account or model profile
revokes future transport, including previously accepted work.

Actual model execution uses the V2 Responses gateway described in
[CODEX-RUNTIME.md](CODEX-RUNTIME.md). No generic RPC endpoint is exposed. Native
Codex tool and approval requests remain rejected; actual action requests return
to V2's ordinary durable permission, budget and tool routes.
