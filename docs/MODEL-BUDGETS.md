# Durable model budgets and request accounting

The local runtime now reserves capacity before sending a model request. This extends the
existing wall-clock and step budgets. It is a per-run admission limit, not organization-wide
quota allocation or a financial spending guarantee.

## User-visible limits

The composer exposes **Limits for the next run**: maximum steps, elapsed seconds, output tokens
per reply and total run tokens. Defaults come from the Rust contract generator: 64 steps,
1,800 seconds, 4,096 output tokens and 262,144 total tokens. The server validates supported
ranges and freezes the configuration in the run. A retry keeps the same configuration and
idempotency key. Changing a limit with the same key is a conflict. Existing runs are not edited.

Elapsed time includes approval/input waits. Token budget is charged across every model request,
including explicit continuation after interruption. The total limit must cover at least the
selected model's configured context window. The per-reply limit must leave input capacity.
The composer shows the reservation required for the selected model.

The run view distinguishes budget charged, provisionally held capacity and the remaining
arithmetic balance. A remaining balance does not override an expired run or a reply-limit breach.
Reported input/output totals are marked incomplete after missing measurements or a lost
request. Unknown cost remains unknown. Native import/restoration runs do not call a provider
and do not acquire a model reservation; their existing time/step limits remain in force.

## Reservation and settlement

1. Check the complete logical request and the frozen provider profile. Finalize
   exact provider JSON bytes and the final endpoint in a private one-use prepared
   request before admission, including Ollama's structured tool arguments.
2. In the run transaction, append a model attempt, increment the step counter and commit a
   `model_started` event. Record a unique attempt ID, request digest, reservation and timestamp.
   The event also stores `prepared_request` wire digest format 1, bound to final
   endpoint, provider kind and SHA256 of the exact payload bytes.
3. After commit, recheck the exact run, lease, context, attempt and remaining
   deadline, then consume the already prepared bytes for the configured provider. There is no automatic
   provider retry or fallback.
4. Commit reply, usage settlement, pending tools and completion event together. An old lease
   cannot settle an attempt, and an attempt cannot settle twice.
5. If provider usage exceeds the reserved context window or the requested output limit, record
   the actual reported usage and expire the run before executing any returned tool.

Until a qualified tokenizer can provide a tighter bound, each request reserves one complete
configured context window. Complete input/output measurements replace the reservation with
their sum. With incomplete measurements, the charge is the greater of the reservation and
the reported partial sum. This is deliberately distinct from actual measured usage.

On cancellation, failure, shutdown or lease recovery, an unfinished model attempt becomes
interrupted. Its capacity remains charged. A failed settlement transaction also keeps the
original reservation. Resuming is explicit; another request requires enough remaining capacity
for a new reservation. Finishing/canceling a run does not erase its accounting history.

This is not an assertion that a remote provider obeys its configured limits or reports all
usage correctly. The runtime detects reported overruns and blocks subsequent actions; it
cannot reverse provider work already performed. Financial limits require additional provider
enforcement and the planned versioned pricing and monetary reservation implementation.

## Context and provider behavior

Preflight and run creation share the same context assembly: system instructions, explicit
folder capabilities, inherited history, this order's messages and the current prompt. Before
each actual model request, the runtime checks context again. The guard includes serialized
tool definitions, UTF-8 bytes and a framing allowance, alongside the output allowance. Its
digest also binds the provider profile and output limit. No message is silently discarded.

This remains a conservative size guard, not a model-specific tokenizer or context compaction.
It may reject conversations that a qualified tokenizer could fit. Tokenizer qualification,
model-specific chat templates, explicit context compaction and provider capacity negotiation
remain required for full M2 acceptance. Preflight does not replace the execution-time check.

Ollama's `prompt_eval_count` and `eval_count` identify reported input and output tokens;
see its [chat API](https://docs.ollama.com/api/chat). The compatible streaming adapter reads
`prompt_tokens` and `completion_tokens`; vLLM's request protocol distinguishes total model
length and output limits in its [chat protocol](https://docs.vllm.ai/en/latest/api/vllm/entrypoints/openai/chat_completion/protocol/).
Missing or interrupted-stream usage is not treated as zero. Provider prices are not inferred.

## Persistence and compatibility

Model attempts are stored inside the durable run record and included in run detail and order
export. The budget summary is computed by the Rust domain from that journal. SQLite and
PostgreSQL use the same transaction and lease checks. This is not yet a global organization,
instance or device reservation ledger.

Migration 9 guards the new accounting behavior against older preview binaries. It reconstructs
previous preview attempts from unchanged `model_started`/`model_completed` events. Historical
request digests remain absent because their exact request was not recorded. Unfinished attempts
retain their context-window reservation. If only legacy steps/aggregate usage remain, one
explicitly unmeasured legacy entry holds at least the configured context window. Reconstruction
does not establish a previously enforced token limit or complete historical billing accuracy.

At the original migration-9 budget revision, run/event schema was 5 and order export
format was 7. Current workflow qualification uses event schema 8 and export 13;
the additional prepared-request event evidence has its own wire format 1 and does
not rewrite old payloads or the legacy logical digest. Default budget fields are omitted from canonical
request serialization so earlier default start requests keep their idempotency identity.
Historical event payloads are not rewritten, and existing result hashes stay unchanged.
Stop all old daemon/worker processes before upgrading a shared preview database. A schema
guard does not revoke a process that already has the database open.

## Verification and remaining scope

Tests cover concurrent admission, stale leases, double settlement, partial usage, arithmetic
overflow, exact request identity, rejected provider overruns, context including tool definitions,
and migration with byte-for-byte preserved event payloads. Real-process qualification kills a
daemon during an HTTP request, restarts it, waits for lease recovery, then verifies that explicit
resume cannot reuse held capacity. A database fault after a provider reply verifies transactional
rollback, retained reservation, restart persistence and absence of automatic replay.

Browser qualification covers invalid preflight without a new run, retained draft, measured and
partial usage, an output-limit breach without the proposed tool effect, and immutable historical
limits while editing the next run. DE/EN, light/dark and 390/1076/1600px were checked. Expanded
settings use continuous conversation scrolling and preserve the top navigation during focus
changes. A synthetic prompt to the existing local `qwen3:4b` returned 1,348 input and 257 output
tokens, settling its reservation to 1,605; this is not a performance benchmark.

Current platform results are recorded in [VERIFY.md](VERIFY.md). Organization-wide fairness,
cost/price sources, monetary caps, offline budget reservations, tokenizer/compaction, aggregate
resource quotas and full load qualification remain separate unfinished requirements.
