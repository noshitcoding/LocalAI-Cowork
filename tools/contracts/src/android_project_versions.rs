use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        (
            "/projects/{project}/versions",
            "get",
            "",
            "AndroidProjectVersion",
            true,
        ),
        (
            "/projects/{project}/versions",
            "post",
            "AndroidCreateProjectVersion",
            "AndroidProjectVersion",
            false,
        ),
        (
            "/projects/{project}/versions/{version}/apply",
            "post",
            "AndroidApplyProjectVersion",
            "AndroidProjectVersion",
            false,
        ),
        (
            "/projects/{project}/merge-review",
            "get",
            "",
            "AndroidProjectMergeReview",
            false,
        ),
        (
            "/projects/{project}/merge-apply",
            "post",
            "AndroidApplyProjectMerge",
            "AndroidProjectVersion",
            false,
        ),
        (
            "/projects/{project}/fork",
            "post",
            "AndroidForkProject",
            "AndroidProjectForkReceipt",
            false,
        ),
        (
            "/projects/{project}/versions/state",
            "get",
            "",
            "@object",
            false,
        ),
        (
            "/projects/{project}/versions/{version}/files",
            "get",
            "",
            "@object",
            true,
        ),
        ("/project-copy-operations/{id}", "get", "", "@object", false),
        (
            "/project-copy-operations/{id}/reconcile",
            "post",
            "@object",
            "@object",
            false,
        ),
        (
            "/project-copy-operations/{id}/retire",
            "post",
            "",
            "@object",
            false,
        ),
    ] {
        let mut parameters = Vec::new();
        for name in ["project", "version", "id"] {
            if path.contains(&format!("{{{name}}}")) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if method == "post" {
            parameters.push(json!({"name":"idempotency-key","in":"header","required":false,"schema":{"type":"string","maxLength":200},"description":"Optional for original V1 bodies, explicit original identity used by current GUI. Changed request under the same key conflicts."}));
        }
        if path.ends_with("merge-review") {
            for name in ["base_version_id", "current_version_id", "result_version_id"] {
                parameters.push(json!({"name":name,"in":"query","required":true,"schema":{"type":"string","format":"uuid"}}));
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
        let mut operation = json!({"servers":[{"url":"/"}],"security":[{"androidSession":[]}],"parameters":parameters,"responses":{"200":{"description":"Original scoped content result","content":{"application/json":{"schema":schema}}},"409":{"description":"Current CAS or original request conflict, unknown object effects are never replayed"}}});
        if !input.is_empty() {
            let schema = if input == "@object" {
                json!({"type":"object","additionalProperties":true})
            } else {
                json!({"$ref":format!("#/components/schemas/{input}")})
            };
            operation["requestBody"] =
                json!({"required":true,"content":{"application/json":{"schema":schema}}});
        }
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
}
