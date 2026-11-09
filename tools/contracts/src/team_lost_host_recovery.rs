use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (suffix, method, input, output) in [
        ("", "get", "", "TeamLostHostPlan"),
        ("/plans/{nonce}", "get", "", "TeamLostHostPlan"),
        (
            "/prepare",
            "post",
            "PrepareTeamLostHostRetirement",
            "TeamLostHostPlan",
        ),
        (
            "/receive",
            "post",
            "ReceiveTeamLostHostRetirement",
            "TeamLostHostRetirementReceipt",
        ),
        ("/operations/{operation}", "get", "", ""),
    ] {
        let path = format!("/teams/{{team}}/runs/{{run}}/lost-host-recovery{suffix}");
        let schema = if output.is_empty() {
            json!({"oneOf":[{"type":"object"},{"type":"null"}]})
        } else if suffix.is_empty() {
            json!({"type":"array","maxItems":64,"items":{"$ref":format!("#/components/schemas/{output}")}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut parameters = vec![
            json!({"name":"team","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
            json!({"name":"run","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        if suffix.contains("{nonce}") {
            parameters.push(json!({"name":"nonce","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        if suffix.contains("{operation}") {
            parameters.push(json!({"name":"operation","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        let mut op = json!({"security":[{"actorSession":[]}],"parameters":parameters,"responses":{"200":{"description":"Current original pre-enrolled host plan or immutable independent retirement receipt","content":{"application/json":{"schema":schema}}},"403":{"description":"Current owner/admin/Execute authority unavailable"},"409":{"description":"Original source, active worker, reviewed manifest or profile CAS differs"}}});
        if !input.is_empty() {
            op["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][path][method] = op;
    }
}
