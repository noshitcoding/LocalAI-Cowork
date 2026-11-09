use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        ("/library/personalities", "get", "", "Personality", true),
        (
            "/library/personalities",
            "post",
            "WritePersonality",
            "Personality",
            false,
        ),
        (
            "/library/personalities/{id}",
            "get",
            "",
            "Personality",
            false,
        ),
        (
            "/library/personalities/{id}",
            "put",
            "WritePersonality",
            "Personality",
            false,
        ),
        (
            "/library/personalities/{id}",
            "delete",
            "DeleteLibraryEntry",
            "LibraryDeleted",
            false,
        ),
        ("/library/agent-templates", "get", "", "AgentTemplate", true),
        (
            "/library/agent-templates",
            "post",
            "WriteAgentTemplate",
            "AgentTemplate",
            false,
        ),
        (
            "/library/agent-templates/{id}",
            "get",
            "",
            "AgentTemplate",
            false,
        ),
        (
            "/library/agent-templates/{id}",
            "put",
            "WriteAgentTemplate",
            "AgentTemplate",
            false,
        ),
        (
            "/library/agent-templates/{id}",
            "delete",
            "DeleteLibraryEntry",
            "LibraryDeleted",
            false,
        ),
        ("/preferences", "get", "", "PreferencesRecord", false),
        (
            "/preferences",
            "put",
            "WritePreferences",
            "PreferencesRecord",
            false,
        ),
    ] {
        for actor in [false, true] {
            let schema = if array {
                json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
            } else {
                json!({"$ref":format!("#/components/schemas/{output}")})
            };
            let mut op = json!({"security":[{"daemonBearer":[]}],
                "responses":{"200":{"description":"Private content or original immutable acknowledgement","content":{"application/json":{"schema":schema}}},"400":{"description":"Invalid typed configuration"},"401":{"description":"Authentication required"},"404":{"description":"No entry in the authenticated scope"},"409":{"description":"Revision or idempotency conflict"}}});
            if actor {
                op["security"] = json!([{"actorSession":[]}]);
                op["servers"] = json!([{"url":"/"}]);
            }
            if !input.is_empty() {
                op["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}})
            }
            if path.contains("{id}") {
                op["parameters"] = json!([{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}])
            }
            let path = if actor {
                format!("/api/v1{path}")
            } else {
                path.to_owned()
            };
            document["paths"][path][method] = op;
        }
    }
}
