# Confirmed restoration into a new working copy

The Owner UI restores retained file and directory versions through common
`ExecutionPlanV2` admission. A restoration receives a fresh independent copy of
the selected current input folders. The retained version is added at the
reviewed, previously absent destination. Original folders and prior run
copies/results are unchanged. This operation calls no model or program.

The authenticated Owner prepares input custody first, then posts
`PrepareRestoreExecutionPlanV2` to
`POST /api/v2/execution-plans/v2/file-restorations`. The request contains the
original `CreateRestoreRun`, input reference and Broker target kind. Actor and
actual controller identity/generation are resolved by the application. The
source must be a retained receipt in the same order/project. The UI pins its
displayed source SHA; the server independently resolves and checks it. Neither
a source reference nor a hash is an IO credential.

The `restore` preflight binds source version/hash, destination, copy grants,
approval mode and budgets. The client checks canonical public plan bytes and
their SHA. Confirmation uses the same
`/orders/{order}/execution-plans/v2/{plan}/commit` route, native copy
materialization, controller revalidation, SQL queue/receipt transaction and
retained result version as model-backed Owner plans. The queued action contains
the expected source hash. Effects recheck it and the stored copy action before
reading retained bytes. Missing or changed copies never fall back to originals.

Approval policy still applies after confirmation. New runs preserve the chat
draft and have no model attempts. Repeated commit returns the exact original
admission receipt after completion or a lost acknowledgement, without a second
run. The UI persists original request, input selection, full plan intent and
confirmation reference across reloads. A changed destination needs a new intent
after explicit retirement; it cannot rewrite a pending request.

`/execution-plans/v2/file-restorations/retire-request` retires the full request.
`/execution-plans/v2/file-restorations/retire-intent` fences an original
`CreateRestoreRun` before its input acknowledgement arrives. Retirement
serializes against confirmation and cannot erase an accepted run. Legacy
`POST /api/v2/file-restorations` recovers accepted legacy requests; new starts
require a confirmed plan. Native compatibility helpers retain their resource
identity checks.

Existing Owner plans and legacy restore requests retain serialized bytes: the
private request union is untagged, and absent
`RestoreFile.expected_source_sha256` remains omitted. No migration is introduced.
Team/device/remote admission, import/document cutover and managed program runtime
qualification remain separate unfinished work.

Evidence is under `.local/remediation-20261024/`: five identical-artifact SQLite
and isolated PostgreSQL cases with actual file/directory effects, restart,
concurrent commit and retirement; three authenticated HTTP cases; two legacy
native restore cases; 718 generated contracts; typecheck and 511 frontend cases.
The real CUA file flow covers reload, exact approval, zero model calls, preserved
draft/original/prior result, browser download SHA, original receipt and a retired
followup with no run. Historical fixture failures and unavailable-action
screenshots remain. Full A–I/M0–M10 acceptance is still open.
