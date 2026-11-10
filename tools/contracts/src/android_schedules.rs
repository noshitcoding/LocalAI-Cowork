use serde_json::{Value, json};

pub fn extend(document: &mut Value) {
    for (path, method, input, output, status, array) in [
        ("/schedules", "get", "", "AndroidScheduleRecord", 200, true),
        (
            "/schedules",
            "post",
            "AndroidCreateSchedule",
            "AndroidScheduleRecord",
            201,
            false,
        ),
        (
            "/thread-schedules",
            "post",
            "AndroidCreateThreadSchedule",
            "AndroidThreadSchedule",
            201,
            false,
        ),
        (
            "/schedules/{id}",
            "put",
            "AndroidUpdateSchedule",
            "AndroidScheduleRecord",
            200,
            false,
        ),
        ("/schedules/{id}", "delete", "", "", 204, false),
        (
            "/schedules/{id}/history",
            "get",
            "",
            "AndroidScheduleHistory",
            200,
            false,
        ),
        (
            "/schedules/preview",
            "get",
            "",
            "AndroidSchedulePreview",
            200,
            false,
        ),
        (
            "/schedules/operations/{key}",
            "get",
            "",
            "AndroidScheduleOriginal",
            200,
            false,
        ),
        (
            "/schedules/tasks",
            "post",
            "AndroidCreateScheduleTask",
            "@object",
            201,
            false,
        ),
        (
            "/schedules/{id}/occurrences/{occurrence}/resolution",
            "get",
            "",
            "AndroidScheduleResolutionView",
            200,
            false,
        ),
        (
            "/schedules/{id}/occurrences/{occurrence}/resolution",
            "post",
            "AndroidScheduleResolutionRequest",
            "AndroidScheduleResolutionReceipt",
            200,
            false,
        ),
    ] {
        let mut parameters = Vec::new();
        if path.contains("{id}") {
            parameters.push(json!({"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        if path.contains("{key}") {
            parameters.push(json!({"name":"key","in":"path","required":true,"schema":{"type":"string","minLength":1,"maxLength":200}}));
            parameters.push(json!({"name":"request_sha256","in":"query","required":true,"schema":{"type":"string","pattern":"^[0-9a-f]{64}$"}}));
        }
        if path.contains("{occurrence}") {
            parameters.push(json!({"name":"occurrence","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        if method != "get" {
            parameters.push(json!({"name":"idempotency-key","in":"header","required":path.ends_with("/resolution"),"description":"Original operation identity. Exact requests replay the original receipt; altered input conflicts.","schema":{"type":"string","maxLength":200}}));
        }
        if method == "delete" {
            parameters.push(json!({"name":"expected_revision","in":"query","required":false,"schema":{"type":"integer","minimum":1}}));
        }
        if path.ends_with("/history") {
            parameters
                .push(json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0}}));
            parameters.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}}));
        }
        if path.ends_with("/preview") {
            for name in ["cron", "timezone"] {
                parameters.push(
                    json!({"name":name,"in":"query","required":true,"schema":{"type":"string"}}),
                );
            }
        }
        let mut success = json!({"description":"Original private schedule result"});
        if !output.is_empty() {
            let mut schema = if output == "@object" {
                json!({"type":"object","additionalProperties":true})
            } else {
                json!({"$ref":format!("#/components/schemas/{output}")})
            };
            if array {
                schema = json!({"type":"array","items":schema});
            }
            success["content"] = json!({"application/json":{"schema":schema}});
        }
        let mut operation = json!({"servers":[{"url":"/"}],"security":[{"androidSession":[]}],"parameters":parameters,"responses":{(status.to_string()):success,"409":{"description":"Original input or revision conflicts; unknown effects are never replayed"}}});
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
}
