//! Public Android recovery contracts; original bytes remain bounded page data.
use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        (
            "/runs/{run}/checkpoints",
            "get",
            "",
            "AndroidRunCheckpoint",
            true,
        ),
        (
            "/runs/{run}/checkpoints/page",
            "get",
            "",
            "AndroidRunCheckpointPage",
            false,
        ),
        (
            "/runs/{run}/checkpoints/{checkpoint}/state/{page}",
            "get",
            "",
            "binary",
            false,
        ),
        ("/runs/{run}/waiting", "get", "", "AndroidRunWaiting", false),
        (
            "/runs/{run}/resume",
            "post",
            "AndroidResumeRun",
            "AndroidRunResumeReceipt",
            false,
        ),
    ] {
        let schema = if output == "binary" {
            json!({"type":"string","format":"binary"})
        } else {
            let item = json!({"$ref":format!("#/components/schemas/{output}")});
            if array {
                json!({"type":"array","items":item})
            } else {
                item
            }
        };
        let media = if output == "binary" {
            "application/octet-stream"
        } else {
            "application/json"
        };
        let mut response = json!({"description":"Original operation or current authorized read","content":{media:{"schema":schema}}});
        if output == "binary" {
            response["description"] = json!(
                "At most 1 MiB of original state, bound to descriptor page and full-state SHA-256 pins"
            );
        }
        let mut operation = json!({"security":[{"bearerAuth":[]}],"responses":{"200":response,"404":{"description":"Current run read/execute authority required"},"409":{"description":"Original effects, physical stop, authority or CAS unresolved"},"413":{"description":"Legacy list requires explicit checkpoint pagination"}}});
        let mut params = vec![
            json!({"name":"run","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        if path.contains("{checkpoint}") {
            params.push(json!({"name":"checkpoint","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            params.push(json!({"name":"page","in":"path","required":true,"schema":{"type":"integer","minimum":0,"maximum":15}}));
        }
        if path.ends_with("/page") {
            params.push(json!({"name":"after_sequence","in":"query","schema":{"type":"integer","minimum":0,"default":0}}));
            params.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":20,"default":10}}));
        }
        operation["parameters"] = json!(params);
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
}
