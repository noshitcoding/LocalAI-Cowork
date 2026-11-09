use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, status) in [
        (
            "/runs/{run}/terminal-sessions",
            "post",
            "AndroidCreateTerminalSession",
            "AndroidTerminalSessionTicket",
            201,
        ),
        (
            "/runs/{run}/terminal-sessions",
            "get",
            "",
            "AndroidTerminalSessionPage",
            200,
        ),
        (
            "/runs/{run}/terminal-sessions/{session}",
            "get",
            "",
            "AndroidTerminalSessionPage",
            200,
        ),
    ] {
        let mut parameters = Vec::new();
        for name in ["run", "session"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if method == "post" {
            parameters.push(json!({"name":"idempotency-key","in":"header","required":false,"description":"Original creation identity; exact replay preserves the original, possibly expired ticket and never renews stream authority.","schema":{"type":"string","maxLength":200}}));
        }
        if path.ends_with("/{session}") {
            parameters
                .push(json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0}}));
        }
        let mut operation = json!({"servers":[{"url":"/"}],"security":[{"androidSession":[]}],"parameters":parameters,"responses":{(status.to_string()):{"description":"Original private terminal metadata","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{output}")}}}},"409":{"description":"Original terminal effect or baseline remains unresolved; never replay input"}}});
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
    document["paths"]["/api/v1/terminal-sessions/{session}/stream"]["get"] = json!({"servers":[{"url":"/"}],"security":[],"description":"One-use original WebSocket. Binary/UTF-8 input. JSON operation=resize requires explicitly negotiated terminal.binary.v1; legacy text frames remain terminal input. Client disconnect closes the original guest. A new explicit ticket starts a new isolated session, never adopts or replays old input.","parameters":[{"name":"session","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},{"name":"ticket","in":"query","required":true,"schema":{"type":"string","minLength":43,"maxLength":43}}],"responses":{"101":{"description":"Original WebSocket stream"},"409":{"description":"Original ticket has already been consumed or original state is uncertain"}}});
    let observation_parameters=["run","session","version"].map(|name|json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
    document["paths"]["/api/v1/runs/{run}/terminal-sessions/{session}/observations/{version}"]["get"] = json!({"servers":[{"url":"/"}],"security":[{"androidSession":[]}],"description":"Read the immutable original control-frame or partial-byte observation with fresh current owner read authority; this grants no execution or replay.","parameters":observation_parameters,"responses":{"200":{"description":"Authenticated original byte observation","content":{"application/json":{"schema":{"type":"object"}}}}}});
}
