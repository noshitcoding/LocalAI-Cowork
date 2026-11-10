# Exact model accounting core

`cowork-model-accounting` implements artifact-bound tokenization, explicit archived
context selection and immutable integer monetary reservations. Its domain state is
executable Rust, with no network access, implicit model download, credential access,
provider retry or automatic context summarization. Storage and runtime integration
must use the transaction protocol below before advertising durable global limits.

The runtime now finalizes a `PreparedModelRequest` before the existing per-run
admission. It owns the exact provider-specific JSON bytes, parsed final endpoint,
complete frozen RunSpec/messages hashes and a separate wire digest format 1.
Ollama tool-argument conversion happens during preparation. The sending function
consumes those already serialized bytes and cannot rebuild messages or tools.
Credentials enter only the HTTP authorization header. The object has private
fields and no Clone, Debug or Deserialize implementation.

Both single-agent and native workflow admission events store this wire digest in
the same transaction as their original request reservation. A fresh exact run,
lease, context and attempt check precedes single-agent IO; workflow dispatch keeps
its existing clock, registry, parent and exact-turn fences. Historical logical
digest format 1 and its conservative byte guard remain unchanged. Extra wire
evidence does not change old workflow serialization, receipts or resume rights.
The provider and Runtime/Storage snapshot passed a separate 216-source
qualification (private development evidence; omitted from public sources):
238 Rust tests per platform, format, Clippy and daemon build, with serial Windows
execution explicitly recorded. Three separate actual PostgreSQL workflow cases
cover original wire dispatch, truncated response and failed response commit;
reopen sends no second request and unknown costs remain held. These receipts
qualify their frozen inputs only; ongoing monetary integration changes need
fresh evidence.

That receipt describes its historical sources. The current product source now
includes the immutable model register, native operator import/revocation, exact
full-template raw requests, explicit context archives and the existing personal
monetary transaction path. Exact JSON bytes alone remain insufficient proof of
the model's tokenizer input. Team tokens continue to be managed per team through
the selected LiteLLM/provider; this feature adds no internal team money/token
budget policy.

## Current product model/context paths

Migration 0047 stores immutable operator qualification bundles and original import
receipts. Desktop imports use the native file picker and native confirmation;
renderer input contains only the operation UUID. The original confirmed request is
written to a private immutable native journal before HTTP. Explicit retry uses those
same bytes and returns the original accepted receipt, including after a lost reply
or application restart. The server checks original replay before current profile
or qualification validation, and never revives an already revoked version.

The model register panel lists hashes/reference identity and supports native
revocation. Run and role selections freeze a particular model-register version.
Before a qualified dispatch, Runtime verifies the actual authenticated deployment
descriptor, freezes the full rendered prompt, and checks the response deployment
echo. Tool catalog, skills, messages and structured-output instructions are part of
that exact rendered input. Personal monetary admission independently renders the
same catalog/prompt under the model-register transaction fence; it reserves actual
artifact input plus output allowance rather than a whole nominal context window.

The composer context panel previews required versus confirmed historical groups,
archives original selected groups and explicitly applies the selection to future
requests. Original attempts, tool receipts and existing runs are unchanged. Archive
inspection/download and idempotent compaction receipts use the same durable Store.
System instructions, the current task and unknown effects remain required. The
small synthetic product fixture exercises raw tool request, actual artifact,
final reply, compaction/reopen, revocation and original import-receipt replay. It
qualifies neither real model quality nor a physical provider deployment.

## Actor-owned private Android model admission

Migration 0056 keeps private actor qualifications in a separate namespace keyed by
the actual identity, private project and immutable model/version. It never creates
owner provider rows or imports team/owner identities. The admitted context binds the
private project's revision, the actual UUID profile revision and complete profile
binding, the neutral provider metadata SHA256, the original selected source SHA256
and the original native import request SHA256. Project/profile changes and revocation
make the old reference unavailable for new requests; they do not rewrite receipts.

Under **Account & security → Private model qualifications for Android**, choose an
actual own private project and server profile. Export its credential-free neutral
provider metadata for the operator's gateway configuration and independent bundle
qualification. Import that reviewed bundle with the paired local desktop's real
native file picker and confirmation. The native dialog identifies the exact actor,
project/profile revisions and source digest. Browser callers cannot supply a host
file path or establish qualification with JSON. The actor session is sent only in a
transient sensitive header alongside the distinct native/owner credentials; it is
never included in the journal, metadata export or local import-selection record.

The native actor journal is separate from the owner journal. It records the
confirmed original request before HTTP and the original accepted result after
acknowledgement. Lost replies and restart continue the same request/key. Original
receipt retrieval requires the current actor's live private project authority,
including for a locally cached receipt. Returning an old receipt after profile
changes or revocation grants no new model dispatch. New imports require a newer
model version; retained source and request hashes remain inspectable in the UI.

Identity-scoped read routes are `/api/v2/actor-model-registry/{projects,profiles}`,
`/context?project_id=UUID&profile_id=UUID` and `/entries?project_id=UUID`. Their bearer
must be the actual global identity session. The returned
`ActorModelRegistryView.qualification.binding` is the existing `ModelRegistryRef`;
a private server run must select it explicitly in `input.model_binding`. The
private runner resolves that reference under the original run/session/project/
profile/lease fences and verifies the complete rendered input with the same
artifact tokenizer and logical-request digest. An unqualified remote profile keeps
the conservative chat path. Every qualified send still requires the authenticated
live deployment descriptor and response deployment echo.

Source integration and narrow fixtures do not qualify an operator's actual model,
Gateway deployment, Android hardware session or long-running production operation.
Those require their own source-bound deployment evidence and operator acceptance.
The narrow Actor fixture receipt (private development evidence; omitted from public sources)
records the actual SQLite migration/admission/lease case (one test, 1.00 s), the
Windows daemon/native checks and generated-contract/TypeScript checks. Its 441
source files were byte-identical before and after. The fixture uses explicitly
synthetic tokenizer/reference vectors and qualifies no deployed model. It also
does not establish an HTTP lost-ACK transport, a complete private qualified
provider response loop, a physical native dialog or a Linux build of this slice.

## Owned model gateway: installable raw-token backend

`cowork-model-gateway` is a separate native Windows/Linux x64 program. It starts
exactly the selected local `llama-server` executable through the owned process
supervisor, with an exact environment and a newly created private backend token.
The spawn observer pins the original child through a Windows process handle or
Linux pidfd before it can be reaped. Read-only OS listener inventories must identify
that same live child at the exact backend address before readiness/tokenization
and completion dispatch. A foreign listener winning the ephemeral port is refused.
It does not connect to an existing receiver, download weights, adopt another
process or choose a fallback model. Its public listener is authenticated loopback
HTTP; the application/provider credential is the separate private gateway token.

The reviewed deployment digest includes the gateway executable SHA256, every
selected backend runtime file SHA256, exact GGUF SHA256, model ID, context limit,
tokenizer SHA256 and template SHA256. Runtime directories/config/token files have
private native permissions. Missing, extra, linked or changed backend files refuse
startup/dispatch. Windows retains original file/directory handles; Linux rechecks
original identities/metadata and the exact runtime namespace before tokenization.

At startup all independently reviewed reference requests are compared against
the actual owned backend's `/tokenize` result. Every request then repeats the actual
HF-versus-backend token-ID comparison with `add_special:false, parse_special:true`.
The gateway forwards a literal integer token array to `/v1/completions`, disables
prompt-cache reuse, and refuses a context/output overflow. Integer prefill avoids
implicit BOS insertion; see the [llama.cpp server API](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md).
The gateway's own matching vectors do not establish independent qualification:
the original operator-reviewed bundle remains mandatory.

Every send uses the application's exact attempt hash and deployment header. Before
the backend completion request, SQLite commits its original intent under FULL
synchronous durability. An owned task keeps execution/receive journaling alive if
the HTTP caller disappears. The terminal stream must contain one finished choice,
one exact usage object and `[DONE]`. Only after its original bytes are journaled
does the gateway return the buffered SSE response. A duplicate exact request
returns that original response; changed bytes conflict. Pending/unknown requests
never submit a second completion. Authenticated `GET /requests/{id}` and
`GET /requests/{id}/response` expose receipts and actual received bytes, including
failed/truncated responses, without granting replay authority.

Build a new native archive from a stable source tree:

```text
python tools/model_gateway/package.py --build --cargo ABSOLUTE_CARGO --output ABSOLUTE_NEW_ARCHIVE
```

Windows uses `.zip`, Linux `.tar.gz`. The archive contains the native gateway,
configurator, native ACL helper, source/file manifest, SPDX 2.3 dependency/native
import inventory, this guide and licence. It contains no llama.cpp runtime, model,
operator config or credential. Python 3.11+ is required for setup and is an explicit
operator-supplied requirement rather than a claimed bundled runtime.

After extracting the archive, create the exact deployment manifest for separate
operator review (use the packaged `configure.py` directly):

```text
python configure.py manifest --gateway ABSOLUTE_GATEWAY --backend-directory ABSOLUTE_NATIVE_RUNTIME --server ABSOLUTE_LLAMA_SERVER --model ABSOLUTE_GGUF --model-id FIXED_MODEL_ID --context-tokens CONTEXT_LIMIT --tokenizer-sha256 EXACT_SHA256 --chat-template-sha256 EXACT_SHA256 --output ABSOLUTE_NEW_DEPLOYMENT_JSON
```

The independent reviewer supplies a qualification bundle with that deployment
digest, original tokenizer/template artifacts and externally obtained complete
request/reference token vectors. The selected authenticated OpenAI-compatible
profile must identify the exact loopback address, model/context and credential-ready
revision, and its canonical JSON SHA256 must match the bundle. Then stage a new
private installation:

```text
python configure.py install --gateway ABSOLUTE_GATEWAY --backend-directory ABSOLUTE_NATIVE_RUNTIME --server ABSOLUTE_LLAMA_SERVER --model ABSOLUTE_GGUF --manifest ABSOLUTE_REVIEWED_DEPLOYMENT --bundle ABSOLUTE_REVIEWED_BUNDLE --profile ABSOLUTE_EXACT_PROFILE --listen 127.0.0.1:7441 --install-directory ABSOLUTE_NEW_PRIVATE_DIRECTORY
ABSOLUTE_INSTALLED_GATEWAY --config ABSOLUTE_PRIVATE_DIRECTORY/gateway.json
```

The install command creates a separate backend/model copy, gateway executable,
state/config and private `gateway-token.txt`. It validates them with the actual
native executable's `--inspect-config` without starting the backend. Configure that
token through the existing native/provider secret flow and import the reviewed
bundle through the model-register panel. The gateway does not edit daemon keys,
team tokens or an existing server. A failed stage remains for explicit inspection.
The private installer also writes `BACKEND-RUNTIME-INVENTORY.json` and
`INSTALLED-SBOM.spdx.json` from the actual copied backend software namespace and
gateway executable, including observed native imports. Weights, configuration and
token files are excluded. Optional `--backend-upstream-sbom` attaches the original
SPDX 2.3 component graph bound to that exact selected server executable; absent
opaque/static component evidence is not replaced with inferred dependencies.

Current bounds are one active backend request, 1,024 retained request identities,
4 MB received bytes per request, 30-minute backend request timeout and a maximum
24-hour owned backend supervisor session. A new session reuses the same original
journal; unknown attempts are not retried. These limits are explicit failures,
not permission to truncate prompts or erase receipts. Real llama.cpp/GGUF/template
qualification, native installation and longer platform operation need their own
source-bound evidence. Synthetic HTTP/SQLite fixtures cannot prove those claims.

## Tokenizer authority and exact request identity

`QualifiedTokenizer::load` reads a complete local Hugging Face `tokenizer.json` with
the pinned Rust `tokenizers` 0.23.1 engine. It checks the exact artifact SHA256,
sorted complete vocabulary digest, immutable provider/model deployment digest,
chat-template digest, reference token-ID vectors and externally trusted qualification
record digest. It rejects active truncation or padding, wrong artifacts, unknown
deployment aliases, different template versions and failed reference vectors.
The Rust implementation loads serialized tokenizer artifacts and encodes them using
the model, normalizers, pre-tokenizer and post-processor in the artifact.
See the [versioned HF Rust API](https://docs.rs/tokenizers/0.23.1/tokenizers/tokenizer/struct.TokenizerImpl.html).

Constructing a tokenizer, generating matching vectors from itself, or providing a
model name is insufficient authority. `TrustedTokenizerBinding` is deliberately
not a deserializable client request. The application must obtain its immutable
qualification hash from a trusted register whose evidence establishes reference
parity for the exact deployed model and full provider framing.

`count` requires a `RenderedPrompt` with this same `ModelIdentity` and the complete
logical request digest. Its input count is the actual encoded ID length, checked
together with the frozen output allowance against the exact context limit. It
records the rendered-text hash and qualification hash for admission and dispatch
revalidation. Chat-template rendering must include tool definitions, message roles,
control tokens and the generation prompt. The complete rendered text is encoded
with `add_special_tokens=false`; the template already supplies those tokens.
HF documents that different chat templates can differ even for models derived from
the same base, and that separately adding special tokens can duplicate template
tokens. See [chat-template semantics](https://huggingface.co/docs/transformers/main/en/chat_templating).

An OpenAI-compatible chat endpoint which does not expose and qualify its exact
rendering cannot be represented as an exact tokenizer match merely by serializing
its JSON messages. Unknown rendering, multimodal accounting or unknown vocabulary
keeps the existing conservative accounting path and an explicit unqualified status.
Tool preview images for vision models (`capabilities.image_input`) are bound logically by
their SHA-256 reference only; the base64 bytes enter the wire payload, are excluded from the
Codex/planning byte guard, and are not estimated locally. The provider's reported
`prompt_tokens` remain the authoritative image input count (see `docs/DOCUMENT-TOOLS.md`).
The unit-test tokenizer is an explicit synthetic WordLevel fixture; its token-ID
parity tests do not qualify Qwen, Ollama, another real model, or model quality.

## Explicit context compaction

`compact` processes a revisioned `ContextSnapshot` of complete exchange groups.
`Required` groups include system instructions and rights, the current user request,
tool call/result boundaries, pending effects, unresolved approvals/questions,
unconfirmed usage and immutable receipts. They cannot be removed.

The application must derive retention from durable server state. A model-supplied
`ConfirmedHistory` label is never proof of effect completion. A separately authorized
`CompactionAuthority` binds the original snapshot, exact frozen scope, immutable
archive receipt and exact hashes of the complete historical groups permitted for
removal. Application storage must confirm that this archive exists durably and that
the user authorized this exact operation before handing authority to the domain.

The function renders and tokenizes the complete retained context each time. It
removes whole authorized historical groups in their original order only while
the exact count exceeds the limit. All retained messages and their order remain
byte-equivalent JSON values. It returns an immutable receipt containing original,
retained, archive and removed-group hashes, as well as the new exact token count.
Repeated execution with the same snapshot/authority yields the same receipt.
If required context still cannot fit, the operation fails without changing the
caller snapshot. There is no generated summary, inferred semantic equivalence,
silent truncation, or change to prior charges/reservations.

`ExactPromptRenderer` is a trusted adapter contract. The application must verify
its template parity and must send the same logical request represented by its
rendered digest. Implementing that trait by an estimate does not qualify a model.

## Versioned prices and monetary capacity

`PriceVersion` binds a provider, exact model digest, immutable version, approved
price-source hash, currency and integer unit scale. Input, output and optional
cached-input rates are expressed as integer currency units per million tokens.
Currencies never convert implicitly. The ledger rejects a changed price under an
existing identity; later versions leave all historical price snapshots intact.

`cost` calculates uncached input, cached input and output using checked `u128`
intermediates, sums the exact rational numerator, rounds upward once, and rejects
a result that cannot fit `u64`. Cached input is a subset of total input. Reservation
assumes the higher input/cache rate and never assumes an unconfirmed discount.
Zero rates are accepted only as explicit price definitions, never inferred from
an absent provider price. Unknown prices cause `PriceUnknown`.

Each `ReservationSpec` binds the immutable request digest, attempt identity,
provider/model, price version, exact input/output upper bounds and every required
budget pool. Supported scopes include run, project, organization, instance and
offline device. Trusted scope resolution passes the complete required scope list
separately to `admit`; a request cannot omit an organization pool. Pool bindings
are unique and sorted, and their currencies must equal the price currency.

`MonetaryLedger` is serializable transaction state with a CAS revision. It checks
all pools before creating one reservation. Balances are derived from the entire
immutable reservation history, including old policy versions, rather than from a
cached mutable total. New pool-policy versions must advance monotonically. Lowering
a limit leaves existing usage charged and blocks further insufficient admissions.
Reusing an attempt ID with different data conflicts. Exact replay returns the
original accepted receipt and `newly_reserved=false`, which never grants another
provider dispatch.

Held or uncertain requests keep their complete original monetary reservation.
Partial measurements, missing provider receipt, or unknown cached usage retain at
least that reservation and additionally charge any larger reported lower bound.
Only complete, receipt-bound measurements settle to measured cost. Settlement is
immutable and idempotent; conflicting later measurements do not rewrite it.
Reported overruns are retained and prevent a new admission in the affected pools.
Arithmetic or transaction failure leaves the old reservation intact. Cancellation,
shutdown, clock expiry and worker loss do not release unknown spending.

Prices are explicit accounting definitions. A local spending admission cannot
reverse an already submitted request or guarantee an arbitrary remote provider's
billing behavior. The runtime must reject effects after a reported model-limit
overrun and preserve uncertain reservations when usage cannot be verified.

## Storage and runtime integration protocol

`persistence::migrate` creates one fixed global ledger row and immutable operation
receipts using portable SQLite/PostgreSQL SQL. The application calls it under its
central migration lock. `load_locked` acquires a database writer lock before reading
balances and validates the serialized ledger's hash, format and revision. Its
`LockedLedger::persist` uses the original revision and payload hash for CAS. These
functions operate on the caller's `sqlx::Transaction<Any>`; they never open another
transaction, commit, or issue a model IO permit. `snapshot` is a checked preflight
read and grants no admission authority.

`record_operation` stores the exact accepted receipt after ledger persistence in
the same transaction. Exact replay returns the original immutable operation;
conflicting identities fail. Caller-owned model attempts, run revisions and events
must share that transaction. The public domain and persistence APIs are trusted
server integration interfaces: actor identity, required scopes and authorization
must be derived from durable application state, rather than accepted as client
assertions.

1. Resolve exact frozen model/price identities and the complete run/project/org/local
   scope on the server. Verify the trusted tokenizer/rendering authority when using
   a qualified count; retain conservative bounds for explicitly unqualified models.
2. Lock the shared durable ledger in a fixed transaction order. Load it, call
   `validate`, verify its expected revision, and apply `admit` to a candidate.
3. Persist ledger, run step/model attempt, original request digest, lease fence and
   sequenced admission event in the same transaction. Only a newly committed
   reservation returns a one-shot provider dispatch permit.
4. Immediately before IO, revalidate run/lease/deadline/revocation and the exact
   request/model/tokenizer/template/price identities. Do not reprepare a different
   request or automatically retry a replayed/uncertain admission.
5. Settle verified usage, response and sequenced receipt under the same ledger/run
   transaction. On lost acknowledgement, read the original receipt. On uncertain
   effects or failed settlement, preserve the old reservation and stop new effects.
6. Archive and explicitly authorize context compaction separately. Commit its
   receipt and original/retained snapshot identities before using a compacted
   context for a new, independently reserved model attempt.

The crate's concurrent domain tests exercise admission under an actual shared
mutex. Its persistent contract tests use actual SQLite and an explicitly selected,
fresh private PostgreSQL database. They cover caller transaction rollback across
the ledger, immutable operation and a caller effect row, concurrent distinct run
pools sharing one organization cap, and replay without a second effect. SQLite
also tests payload/header corruption and unknown-format rejection. The caller
effect row is a transaction-contract fixture; it does not establish integration
with the application's real model attempts or run events. Real model parity,
integrated storage/runtime restart behavior and process sandbox qualification
require their own source-bound evidence.
