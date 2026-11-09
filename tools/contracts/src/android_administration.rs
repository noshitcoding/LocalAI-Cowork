#[path = "android_administration_extended.rs"]
mod extended;
use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    extended::extend(document);
    for (path, method, input, output) in [
        ("/content-access", "get", "", "AndroidContentAccess"),
        (
            "/projects/{project}/members",
            "get",
            "",
            "AndroidProjectMemberPage",
        ),
        (
            "/projects/{project}/members",
            "post",
            "AndroidProjectMemberWrite",
            "",
        ),
        ("/projects/{project}/members/{user}", "delete", "", ""),
        (
            "/projects/{project}/members/operations/{key}",
            "get",
            "",
            "AndroidPrivateProjectMemberReceipt",
        ),
    ] {
        let schema = if output.is_empty() {
            json!({})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let schema = if path == "/projects/{project}/members" && method == "get" {
            json!({"oneOf":[{"$ref":"#/components/schemas/AndroidProjectMemberPage"},{"$ref":"#/components/schemas/AndroidPrivateProjectMemberPage"}]})
        } else {
            schema
        };
        let mut operation = json!({"security":[{"actorSession":[]}],"responses":{
            "200":{"description":"Current identity content permissions or immutable original grant receipt",
                   "content":{"application/json":{"schema":schema}}},
            "403":{"description":"Current session, membership or content authority unavailable"},
            "409":{"description":"Original operation or expected revision conflict"}}});
        let mut parameters = Vec::new();
        for name in ["project", "user"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if path.ends_with("/operations/{key}") {
            parameters.push(json!({"name":"key","in":"path","required":true,"schema":{"type":"string","minLength":1,"maxLength":200}}));
            parameters.push(json!({"name":"request_sha256","in":"query","required":true,"schema":{"type":"string","pattern":"^[a-f0-9]{64}$"}}));
        }
        if path == "/content-access" {
            parameters.push(json!({"name":"project_id","in":"query","schema":{"type":"string","format":"uuid"}}));
        }
        if method == "get" && path.ends_with("/members") {
            parameters.push(
                json!({"name":"after","in":"query","schema":{"type":"string","format":"uuid"}}),
            );
            parameters.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100,"default":50}}));
        }
        if method != "get" {
            parameters.push(json!({"name":"idempotency-key","in":"header","schema":{"type":"string","minLength":1,"maxLength":200}}));
        }
        if method == "delete" {
            parameters.push(json!({"name":"expected_revision","in":"query","schema":{"type":"integer","minimum":0}}));
        }
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        operation["parameters"] = json!(parameters);
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
}
