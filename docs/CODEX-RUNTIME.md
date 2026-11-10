# Codex execution in V2

First configure the existing extracted, pinned App Server bundle and its OS
keyring as described in [CODEX-ACCOUNTS-V2.md](CODEX-ACCOUNTS-V2.md). Sign in from
Settings, then select an actually discovered account model and reasoning choice.
The resulting immutable model profile can be copied into a chat, workflow role
or task. Native owner and server actor accounts use separate authenticated
namespaces, isolated homes and scoped database records. Server execution needs
its own available keyring; a desktop login does not supply server credentials.

The transport starts the verified `0.147.0` binary with a private per-account
home. It preserves Windows `SYSTEMROOT`/`WINDIR` required by the runtime, without
adopting a global Codex home. A one-use loopback gateway obtains the runtime's
account-authentication headers in memory. The harness sees constant synthetic
messages and a constant response. The actual V2 prompt, Responses tool catalog,
model output and encrypted continuation items remain in the V2 gateway. The
production upstream is fixed to the ChatGPT Codex Responses endpoint; redirects
and transport retries are disabled.

The gateway sends the already admitted V2 request with its frozen model,
reasoning, generation, output schema and budget. It validates the actual streamed
response and converts only recognized function calls into ordinary V2 tool
calls. Those calls still require V2 permissions and durable approvals. Built-in
shell, file, web and other native Codex actions cannot receive the actual content
or execute on its behalf. Cancellation, uncertain sends, usage accounting and
output limits follow the ordinary V2 request journal.

With `store=false`, the gateway requests encrypted reasoning content and stores
the original bounded Responses output items alongside the assistant checkpoint.
Their original order, message phase and function-call identifiers are restored
before the following tool results. Continuations bind to the exact frozen
provider profile. A changed profile cannot adopt another profile's continuation.
This follows the official [reasoning continuation guidance](https://developers.openai.com/api/docs/guides/reasoning).
The pinned [Responses request source](https://github.com/openai/codex/blob/rust-v0.147.0/codex-rs/codex-api/src/common.rs)
is also used to review protocol compatibility.

The qualification test launches the real pinned Windows binary against a local
HTTP fixture, completes a tool call, closes and reopens SQLite, and completes a
second model request with the original reasoning, phase and tool result. It uses
synthetic fixture authentication and never reads or modifies a real account.

```powershell
$env:COWORK_TEST_CODEX_MANIFEST = 'C:\absolute\path\runtime-bundle-manifest.json'
cargo test --locked -p cowork-runtime --lib pinned_auth_gateway_routes_real_v2_response_without_exposing_it_to_codex -- --ignored --nocapture
```

This proves the pinned process, gateway isolation and durable two-turn protocol.
Live ChatGPT subscription credentials and the live backend's acceptance of every
configured parameter require a real-account deployment check. In particular,
V2 keeps the explicit output-token limit; a backend rejection is surfaced and
never retried after silently dropping that limit. No plaintext authentication
fallback or automatic runtime installation is provided.
