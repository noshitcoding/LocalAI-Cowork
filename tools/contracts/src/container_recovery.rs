use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (suffix, method, input, output) in [
        ("", "get", "", "TeamContainerRecoveryPlan"),
        (
            "/retire",
            "post",
            "RetireTeamContainer",
            "TeamContainerRecoveryReceipt",
        ),
        ("/reject", "post", "RetireTeamContainer", ""),
        (
            "/operations/{operation}",
            "get",
            "",
            "TeamContainerRecoveryReceipt",
        ),
    ] {
        let path = format!("/teams/{{team}}/runs/{{run}}/container-recovery{suffix}");
        let reference = json!({"$ref":format!("#/components/schemas/{output}")});
        let schema = if suffix.is_empty() {
            json!({"type":"array","maxItems":64,"items":reference})
        } else if suffix.starts_with("/operations/") {
            json!({"oneOf":[reference,{"type":"null"}]})
        } else if output.is_empty() {
            json!({"type":"object"})
        } else {
            reference
        };
        let mut parameters = vec![
            json!({"name":"team","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
            json!({"name":"run","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        if suffix.contains("{operation}") {
            parameters.push(json!({"name":"operation","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        let mut operation = json!({"security":[{"actorSession":[]}],"parameters":parameters,"responses":{
            "200":{"description":"Actual original container evidence or immutable retirement receipt","content":{"application/json":{"schema":schema}}},
            "403":{"description":"Current content or original-owner authority unavailable"},
            "409":{"description":"Live worker, original source, host or expected revision conflict"}}});
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][path][method] = operation;
    }
}
