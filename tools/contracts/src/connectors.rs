use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array, native) in [
        (
            "/projects/{project}/connectors",
            "get",
            "",
            "OwnerConnector",
            true,
            false,
        ),
        (
            "/projects/{project}/connectors",
            "post",
            "SaveOwnerConnector",
            "OwnerConnector",
            false,
            false,
        ),
        (
            "/projects/{project}/connectors/{id}/delete",
            "post",
            "DeleteOwnerConnector",
            "OwnerConnector",
            false,
            false,
        ),
        (
            "/projects/{project}/connectors/{id}/test",
            "post",
            "",
            "OwnerConnectorProbe",
            false,
            false,
        ),
        (
            "/native/connectors/{project}",
            "post",
            "SaveOwnerConnector",
            "OwnerConnector",
            false,
            true,
        ),
        (
            "/native/connectors/{project}/{id}/delete",
            "post",
            "DeleteOwnerConnector",
            "OwnerConnector",
            false,
            true,
        ),
        (
            "/native/connectors/{project}/{id}/test",
            "post",
            "",
            "OwnerConnectorProbe",
            false,
            true,
        ),
    ] {
        let schema = if array {
            json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut operation = json!({"security":[{"daemonBearer":[]}],"description":"Installation-owner connector. Native writes additionally require the native custody credential; owner browser requests cannot bypass confirmation. URL and key are write-only encrypted environment configuration. Immutable MCP versions retain prior accepted runs; delete revokes every version. Probe sends only HEAD with no redirects or retries and never sends a message. recover_only recovers the exact staged operation without browser-persisted credentials.","responses":{"200":{"description":"Secret-free original receipt or current metadata","content":{"application/json":{"schema":schema}}},"400":{"description":"Invalid settings or unavailable runtime"},"401":{"description":"Owner credential required"},"403":{"description":"Wrong origin/custody"},"404":{"description":"Unknown connector"},"409":{"description":"Stale revision, original operation incomplete or conflicting request"}}});
        if native {
            operation["security"] = json!([{"daemonBearer":[],"nativeAuthority":[]}]);
        }
        let params: Vec<Value> = ["project", "id"]
            .iter()
            .filter(|name| path.contains(&format!("{{{name}}}")))
            .map(|name| json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
            .collect();
        operation["parameters"] = json!(params);
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][path][method] = operation;
    }
}
