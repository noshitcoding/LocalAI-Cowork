use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        ("/codex/runtime", "get", "", "CodexRuntimeStatus", false),
        ("/codex/accounts", "get", "", "CodexAccount", true),
        (
            "/codex/accounts",
            "post",
            "WriteCodexAccount",
            "CodexAccount",
            false,
        ),
        ("/codex/accounts/{id}", "get", "", "CodexAccount", false),
        (
            "/codex/accounts/{id}",
            "put",
            "WriteCodexAccount",
            "CodexAccount",
            false,
        ),
        (
            "/codex/accounts/{id}",
            "delete",
            "DeleteLibraryEntry",
            "LibraryDeleted",
            false,
        ),
        (
            "/codex/accounts/{id}/status",
            "get",
            "",
            "CodexAccountStatus",
            false,
        ),
        ("/codex/accounts/{id}/models", "get", "", "CodexModel", true),
        (
            "/codex/accounts/{id}/login",
            "post",
            "CodexLoginRequest",
            "CodexLoginView",
            false,
        ),
        (
            "/codex/accounts/{id}/logout",
            "post",
            "CodexAccountAction",
            "",
            false,
        ),
    ] {
        for actor in [false, true] {
            let schema = if output.is_empty() {
                json!({"type":"object"})
            } else if array {
                json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
            } else {
                json!({"$ref":format!("#/components/schemas/{output}")})
            };
            let mut operation = json!({"security":[{"daemonBearer":[]}],"responses":{"200":{"description":"Private Codex metadata or original account-action acknowledgement","content":{"application/json":{"schema":schema}}},"401":{"description":"Authentication required"},"404":{"description":"No account in authenticated scope"},"409":{"description":"Revision, original action or uncertain effect conflict"},"502":{"description":"Configured runtime or OS keyring unavailable"}}});
            if actor {
                operation["security"] = json!([{"actorSession":[]}]);
                operation["servers"] = json!([{"url":"/"}]);
            }
            if !input.is_empty() {
                operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
            }
            if path.contains("{id}") {
                operation["parameters"] = json!([{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}]);
            }
            document["paths"][if actor {
                format!("/api/v1{path}")
            } else {
                path.into()
            }][method] = operation;
        }
    }
}
