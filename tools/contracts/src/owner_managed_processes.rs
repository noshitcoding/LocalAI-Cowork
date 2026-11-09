use serde_json::{Value, json};

pub(super) fn extend(api: &mut Value) {
    api["paths"]["/orders/{order}/runs/{run}/managed-processes/{attempt}/reconcile"]["post"] = json!({
        "description":"Explicitly reconcile one interrupted original managed execution from its authentic signed completion. The controller reads retained history or GETs the pinned original runner receipt. No launch, lease renewal, environment activation or file grant occurs. finish_cancellation atomically closes the business run as canceled while preserving the actual physical result. A lost response must not cause an automatic retry; an exact original successful intent may be replayed.",
        "security":[{"daemonBearer":[]}],
        "parameters":[
            {"name":"order","in":"path","required":true,"schema":{"type":"string"}},
            {"name":"run","in":"path","required":true,"schema":{"type":"string"}},
            {"name":"attempt","in":"path","required":true,"schema":{"type":"string"}}
        ],
        "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ReconcileOwnerManagedProcess"}}}},
        "responses":{
            "200":{"description":"Original completion settled and original intent journaled atomically.","headers":{"Cache-Control":{"schema":{"type":"string","const":"no-store"}}},"content":{"application/json":{"schema":{"$ref":"#/components/schemas/OwnerManagedProcessReconciliation"}}}},
            "default":{"description":"Authentication, historical provenance, revision, state or original completion could not be verified. No new execution is authorized.","headers":{"Cache-Control":{"schema":{"type":"string","const":"no-store"}}},"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}
        }
    });
}
