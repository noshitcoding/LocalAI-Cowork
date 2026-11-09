# Structured final JSON output

The optional `output` contract freezes a name and JSON Schema with the original
personal run, workflow role, team plan/agent or V2 device input. The UI provides a
German/English schema editor and shows the frozen contract in the corresponding
run or immutable workflow version. Ordinary requests without `output` retain
their existing serialization and request fingerprints.

```json
{
  "name": "result",
  "schema": {
    "type": "object",
    "properties": {"summary": {"type": "string"}},
    "required": ["summary"],
    "additionalProperties": false
  }
}
```

Schemas are validated locally using JSON Schema draft 2020-12. The root must be
an object schema; the schema is limited to 64 KiB, 64 levels and 4096 nodes.
Only local JSON Pointer references are accepted. External references and `$id`
are rejected, and the validator has no remote resolver. Providers may support a
smaller schema subset. A provider rejection never silently changes the schema,
provider, model or response format.

Personal OpenAI-compatible requests, Team LiteLLM and the device-owned
OpenAI-compatible adapter send `response_format.type=json_schema` with the
frozen name/schema and `strict=true`. Personal Ollama requests send the frozen
schema in `format`. The exact wire request and its logical fingerprint include
the output contract before durable model admission. Synthetic test providers
cannot accept a structured-output request.

Validation applies to the actual final answer when no tool calls remain. No
Markdown fence removal, JSON repair or automatic corrective model call is
performed. Received responses and usage stay in their existing durable receipt
or journal. Invalid JSON or a schema mismatch fails the run/node. Intermediate
tool calls use the existing approval, authority and effect journals.

Workflow roles independently freeze their final answer schema. The overall
personal workflow view continues to show its existing collection of root
results. A Team plan can set an overall schema as well as per-agent schemas.
Hierarchical workflows apply the overall schema to the existing manager rollup;
without it, the manager's own schema applies. Sequential/parallel workflows with
an overall schema add one explicit, journaled model rollup after all worker
checkpoints. The UI explains that this additional call is required. Worker
checkpoints and original model/tool receipts are reused on an authorized
recovery; unknown effects are never automatically repeated.

The standalone device agent stores the received model response and contract hash
before final validation. The device server independently validates `result.content`
against the original `spec.input.output`, derives the parsed structured result
itself, and records an invalid success submission as a failure with its received
result. The original completion request hash remains the ACK-recovery key.
Existing device result size limits continue to apply.

The separately signed native offline delegation path also supports the contract.
Grants with an output schema use version 2 and the `grant-v2` signature purpose;
the device signs version-2 reports using `run-report-v2`. Each admitted local run
freezes the schema hash. Local reads/writes reject a changed grant, schema hash,
device, ordinal, admission time or deadline. The offline provider receives the
same JSON response format. Invalid received JSON remains a failed signed history
record; it never triggers a corrective request. Server reconciliation validates
any claimed completed answer against the signed grant schema independently.
Grants without an output contract retain version 1 and its original signature
purposes. New optional fields are omitted, preserving old signed grant/report
serialization. Existing version-1 tickets remain readable and verifiable.

Source implementation is present. Real-provider schema compatibility, provider
tool/JSON combinations, process-death/reconciliation scenarios, native installer
and production qualification still require their own acceptance evidence.
Targeted compiler/type checks do not constitute that functional acceptance.
