use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output) in [
        ("/private-workflows", "get", "", "AndroidWorkflowPage"),
        (
            "/private-workflows",
            "post",
            "AndroidWorkflowWrite",
            "AndroidWorkflowOperationReceipt",
        ),
        (
            "/private-workflows/operations/{request}",
            "get",
            "",
            "AndroidWorkflowOperationReceipt",
        ),
        (
            "/private-workflows/{workflow}/versions",
            "get",
            "",
            "AndroidWorkflowVersionPage",
        ),
        (
            "/private-workflows/{workflow}/versions",
            "post",
            "AndroidWorkflowWrite",
            "AndroidWorkflowOperationReceipt",
        ),
        (
            "/private-workflows/{workflow}/versions/{version}",
            "get",
            "",
            "AndroidWorkflowVersion",
        ),
        (
            "/private-workflows/{workflow}/publication",
            "post",
            "AndroidWorkflowChange",
            "AndroidWorkflowOperationReceipt",
        ),
        ("/runs/{run}/workflow", "get", "", "AndroidWorkflowRunView"),
        (
            "/runs/{run}/workflow/preflight",
            "get",
            "",
            "AndroidWorkflowPreflight",
        ),
        ("/runs/{run}/workflow/results/{version}", "get", "", ""),
    ] {
        let mut op = json!({"security":[{"bearerAuth":[]}],"responses":{"200":{"description":"Current Actor read or immutable original receipt","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{output}")}}}},"404":{"description":"Current Actor/project rights or original receipt unavailable"},"409":{"description":"Original request/CAS/graph/configuration conflict"},"413":{"description":"Explicit published format or quota bound exceeded"}}});
        if output.is_empty() {
            op["responses"]["200"]["content"] =
                json!({"application/json":{"schema":{"type":"string","format":"binary"}}});
            op["responses"]["200"]["description"] = json!(
                "Complete original node-result JSON bytes, SHA256 and size bound to the actual run/node source"
            );
        }
        if !input.is_empty() {
            op["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}})
        }
        let mut parameters = Vec::new();
        for name in ["workflow", "version", "request", "run"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}))
            }
        }
        if method == "get" && !path.starts_with("/runs/") {
            parameters.push(json!({"name":"project_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}))
        }
        op["parameters"] = json!(parameters);
        document["paths"][format!("/api/v1{path}")][method] = op;
    }
}
