use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        (
            "/projects/{project}/automation-triggers/{trigger}/file-watch",
            "get",
            "",
            "AutomationFileWatchStatus",
            false,
        ),
        (
            "/projects/{project}/automation-triggers",
            "get",
            "",
            "AutomationTrigger",
            true,
        ),
        (
            "/projects/{project}/automation-triggers",
            "post",
            "WriteAutomationTrigger",
            "AutomationTrigger",
            false,
        ),
        (
            "/projects/{project}/automation-triggers/{trigger}/preview",
            "post",
            "SubmitAutomationEvent",
            "AutomationEventPreview",
            false,
        ),
        (
            "/projects/{project}/automation-triggers/{trigger}/events",
            "post",
            "SubmitAutomationEvent",
            "AutomationEvent",
            false,
        ),
        (
            "/projects/{project}/automation-triggers/{trigger}/events",
            "get",
            "",
            "AutomationEventPage",
            false,
        ),
        (
            "/projects/{project}/automation-events/{event}/retire",
            "post",
            "RetireAutomationEvent",
            "AutomationEvent",
            false,
        ),
        (
            "/projects/{project}/automation-triggers/{trigger}/webhook",
            "put",
            "ConfigureAutomationWebhook",
            "AutomationWebhookGrant",
            false,
        ),
    ] {
        let schema = json!({"$ref":format!("#/components/schemas/{output}")});
        let schema = if array {
            json!({"type":"array","items":schema})
        } else {
            schema
        };
        let mut operation = json!({"responses":{"200":{"description":"Original automation receipt","content":{"application/json":{"schema":schema}}}}});
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        let mut parameters = Vec::new();
        for name in ["project", "trigger", "event"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(
                    json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                );
            }
        }
        if method == "get" && path.ends_with("/events") {
            parameters.push(
                json!({"name":"before","in":"query","schema":{"type":"integer","minimum":1}}),
            );
            parameters.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}}));
        }
        operation["parameters"] = json!(parameters);
        document["paths"][path][method] = operation;
    }
    document["paths"]["/projects/{project}/automation-triggers/{trigger}/webhook"]["get"] = json!({"parameters":[{"name":"project","in":"path","required":true,"schema":{"type":"string"}},{"name":"trigger","in":"path","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"Current secret-free grant","content":{"application/json":{"schema":{"anyOf":[{"$ref":"#/components/schemas/AutomationWebhookGrant"},{"type":"null"}]}}}}}});
    document["components"]["schemas"]["ConfigureAutomationWebhook"]["properties"]["token"]["writeOnly"] =
        json!(true);
    document["components"]["securitySchemes"]["automationWebhookBearer"] = json!({"type":"http","scheme":"bearer","description":"A revocable, expiring credential scoped to one trigger"});
    document["paths"]["/api/automation-webhooks/{trigger}"]["post"] = json!({"servers":[{"url":"/"}],"security":[{"automationWebhookBearer":[]}],"parameters":[{"name":"trigger","in":"path","required":true,"schema":{"type":"string"}}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/SubmitAutomationEvent"}}}},"responses":{"200":{"description":"Original ingress receipt","content":{"application/json":{"schema":{"$ref":"#/components/schemas/AutomationEvent"}}}}}});
}
