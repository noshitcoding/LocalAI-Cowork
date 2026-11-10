use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output) in [
        ("/offline/runs/{run}/mcp/tasks", "get", "", "McpAsyncTask"),
        (
            "/offline/runs/{run}/mcp/tasks/{task}",
            "get",
            "",
            "McpTaskDetail",
        ),
        ("/offline/runs/{run}/mcp/tasks/{task}/result", "get", "", ""),
        (
            "/offline/runs/{run}/mcp/tasks/{task}/cancel-receipt",
            "get",
            "",
            "Value",
        ),
        (
            "/offline/runs/{run}/mcp/interactions/{interaction}/receipt",
            "get",
            "",
            "Value",
        ),
        (
            "/offline/runs/{run}/mcp/tasks/{task}/cancel",
            "post",
            "McpAsyncTaskRequest",
            "Value",
        ),
        (
            "/offline/runs/{run}/mcp/interactions",
            "get",
            "",
            "OfflineMcpInteractionPage",
        ),
        (
            "/offline/runs/{run}/mcp/interactions/{interaction}/answer",
            "post",
            "AnswerMcpTaskInteraction",
            "Value",
        ),
    ] {
        let schema = if output.is_empty() {
            json!({"type":"string","format":"binary"})
        } else if output == "Value" {
            json!({})
        } else if output == "McpAsyncTask" {
            json!({"type":"array","items":{"$ref":"#/components/schemas/McpAsyncTask"}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut op = json!({"security":[{"daemonBearer":[],"nativeAuthority":[]}],"responses":{"200":{"description":"Original local device task, source and immutable response","content":{"application/json":{"schema":schema}}},"409":{"description":"Original once operation or current signed grant conflict"}}});
        if !input.is_empty() {
            op["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}})
        }
        let mut parameters = vec![
            json!({"name":"run","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        for name in ["task", "interaction"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}))
            }
        }
        if path.ends_with("/interactions") {
            parameters.push(json!({"name":"after","in":"query","required":false,"schema":{"type":"string","format":"uuid"}}));
            parameters.push(json!({"name":"only_waiting","in":"query","schema":{"type":"boolean","default":false}}));
        }
        if path.ends_with("/receipt") || path.ends_with("/cancel-receipt") {
            parameters.push(json!({"name":"expected_revision","in":"query","required":true,"schema":{"type":"integer","minimum":1}}));
            parameters.push(json!({"name":"idempotency_key","in":"query","required":true,"schema":{"type":"string","minLength":1,"maxLength":128}}));
        }
        op["parameters"] = json!(parameters);
        document["paths"][path][method] = op;
    }
}
