//! Routes implemented in the actual daemon native/Team/private MCP leaves.
use serde_json::{Value, json};
fn route(
    document: &mut Value,
    path: &str,
    method: &str,
    input: &str,
    output: &str,
    array: bool,
    security: Value,
) {
    let mut parameters = Vec::new();
    for part in path.split('/') {
        if let Some(name) = part.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","minLength":1,"maxLength":128}}));
        }
    }
    let schema = if output == "@object" {
        json!({"type":"object","additionalProperties":true})
    } else {
        json!({"$ref":format!("#/components/schemas/{output}")})
    };
    let schema = if array {
        json!({"type":"array","items":schema})
    } else {
        schema
    };
    let mut op = json!({"servers":[{"url":"/"}],"security":security,"parameters":parameters,"responses":{"200":{"description":"Original scope-bound MCP metadata or receipt; no token or refresh secret is returned","content":{"application/json":{"schema":schema}}},"409":{"description":"Original revision, source or once-operation conflicts; unknown IO is never replayed"},"503":{"description":"Original key, endpoint or current execution authority unavailable"}}});
    if !input.is_empty() {
        let schema = if input == "@object" {
            json!({"type":"object","additionalProperties":true})
        } else {
            json!({"$ref":format!("#/components/schemas/{input}")})
        };
        op["requestBody"] =
            json!({"required":true,"content":{"application/json":{"schema":schema}}});
    }
    document["paths"][path][method] = op;
}
pub fn extend(document: &mut Value) {
    let native = json!([{"daemonBearer":[],"nativeAuthority":[]}]);
    let team = json!([{"teamSession":[]}]);
    let personal = json!([{"androidSession":[]}]);
    for (base, security) in [
        ("/api/v2/native/mcp/oauth/{project}", native.clone()),
        ("/api/v2/teams/{team}/mcp/oauth", team.clone()),
        ("/api/v1/projects/{project}/mcp/oauth", personal.clone()),
    ] {
        route(
            document,
            base,
            "get",
            "",
            "McpOAuthProfile",
            true,
            security.clone(),
        );
        route(
            document,
            base,
            "post",
            "ConfigureMcpOAuth",
            "McpOAuthProfile",
            false,
            security.clone(),
        );
        route(
            document,
            &format!("{base}/discovery"),
            "post",
            "McpOAuthDiscovery",
            "McpOAuthMetadata",
            false,
            security.clone(),
        );
        if !base.starts_with("/api/v2/native/") {
            route(
                document,
                &format!("{base}/setup"),
                "get",
                "",
                "@object",
                false,
                security,
            );
        }
    }
    for (base, security) in [
        ("/api/v2/native/mcp/oauth", native.clone()),
        ("/api/v2/teams/{team}/mcp/oauth", team.clone()),
        ("/api/v1/mcp/oauth", personal.clone()),
    ] {
        route(
            document,
            &format!("{base}/profile/{{profile}}/start"),
            "post",
            "McpOAuthRequest",
            "McpOAuthFlow",
            false,
            security.clone(),
        );
        route(
            document,
            &format!("{base}/flow/{{flow}}"),
            "get",
            "",
            "McpOAuthFlow",
            false,
            security.clone(),
        );
        route(
            document,
            &format!("{base}/flow/{{flow}}/complete"),
            "post",
            "",
            "@object",
            false,
            security,
        );
    }
    for (base, security) in [
        ("/api/v2/runs/{run}", json!([{"daemonBearer":[]}])),
        ("/api/v2/teams/{team}/runs/{run}", team.clone()),
        ("/api/v1/runs/{run}", personal.clone()),
    ] {
        route(
            document,
            &format!("{base}/mcp-tasks"),
            "get",
            "",
            "McpAsyncTask",
            true,
            security.clone(),
        );
        route(
            document,
            &format!("{base}/mcp-tasks/{{task}}"),
            "get",
            "",
            "McpTaskDetail",
            false,
            security.clone(),
        );
        route(
            document,
            &format!("{base}/mcp-tasks/{{task}}/cancel"),
            "post",
            "McpAsyncTaskRequest",
            "@object",
            false,
            security.clone(),
        );
        if base != "/api/v2/runs/{run}" {
            route(
                document,
                &format!("{base}/mcp-interactions"),
                "get",
                "",
                "McpTaskInteraction",
                true,
                security.clone(),
            );
            route(
                document,
                &format!("{base}/mcp-interactions/{{interaction}}"),
                "post",
                "AnswerMcpTaskInteraction",
                "@object",
                false,
                security,
            );
        }
    }
    route(
        document,
        "/api/v2/native/mcp/tasks/{run}/interactions",
        "get",
        "",
        "McpTaskInteraction",
        true,
        native.clone(),
    );
    route(
        document,
        "/api/v2/native/mcp/tasks/{run}/interactions/{interaction}",
        "post",
        "AnswerMcpTaskInteraction",
        "@object",
        false,
        native,
    );
    route(
        document,
        "/api/v1/projects/{project}/mcp-bindings/endpoints",
        "get",
        "",
        "@object",
        true,
        personal.clone(),
    );
    route(
        document,
        "/api/v1/projects/{project}/mcp-metadata/{entity}",
        "put",
        "@object",
        "@object",
        false,
        personal,
    );
    document["paths"]["/api/v1/projects/{project}/mcp-metadata/{entity}"]["put"]["parameters"].as_array_mut().expect("parameters").push(json!({"name":"idempotency-key","in":"header","required":true,"schema":{"type":"string","minLength":1,"maxLength":128}}));
    for path in ["/mcp/oauth/callback", "/api/v1/mcp/oauth/callback"] {
        document["paths"][path]["get"] = json!({"servers":[{"url":"/"}],"security":[],"description":"Evidence-only original browser callback. Exactly one bounded code or error is captured using the original random state, provider issuer, redirect and encrypted flow. No user principal or token POST is constructed; authenticated original-session completion is separate.","parameters":[{"name":"state","in":"query","required":true,"schema":{"type":"string","minLength":64,"maxLength":64}},{"name":"code","in":"query","schema":{"type":"string","minLength":1,"maxLength":8192}},{"name":"error","in":"query","schema":{"type":"string","maxLength":128}},{"name":"iss","in":"query","schema":{"type":"string","maxLength":8192}},{"name":"error_description","in":"query","schema":{"type":"string","maxLength":4096}}],"responses":{"200":{"description":"Static return-to-app HTML with no code/state/provider reflection","content":{"text/html":{"schema":{"type":"string"}}}},"409":{"description":"Original response mismatch or captured state"}}});
    }
}
