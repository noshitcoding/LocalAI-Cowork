//! Routes mirror the canonical request types used by the actual handlers.
use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    document["components"]["securitySchemes"]["deviceAgent"] = json!({"type":"http","scheme":"bearer","description":"Current paired executor credential; requires its separately admitted current project sync grant"});
    for (path, method, input, device) in [
        ("/mobile-cache/imports", "get", "", false),
        (
            "/mobile-cache/imports",
            "post",
            "AndroidMobileCacheBegin",
            false,
        ),
        ("/mobile-cache/imports/{id}", "get", "", false),
        (
            "/mobile-cache/imports/{id}/parts/{ordinal}",
            "put",
            "binary",
            false,
        ),
        (
            "/mobile-cache/imports/{id}/index",
            "post",
            "AndroidAdministrationRevision",
            false,
        ),
        ("/mobile-cache/imports/{id}/entries", "get", "", false),
        ("/mobile-cache/imports/{id}/bytes", "get", "", false),
        (
            "/mobile-cache/imports/{id}/activate",
            "post",
            "AndroidMobileCacheActivate",
            false,
        ),
        (
            "/mobile-cache/imports/{id}/retain",
            "post",
            "AndroidAdministrationRevision",
            false,
        ),
        ("/administration/operations/{key}", "get", "", false),
        ("/support-grants", "get", "", false),
        ("/support-grants", "post", "AndroidSupportGrantWrite", false),
        (
            "/support-grants/{id}/revoke",
            "post",
            "AndroidSupportGrantRevoke",
            false,
        ),
        ("/storage-quotas/{actor}", "get", "", false),
        (
            "/storage-quotas/{actor}",
            "put",
            "AndroidPersonalStorageQuotaWrite",
            false,
        ),
        ("/executor-pools", "get", "", false),
        ("/executor-pools", "post", "AndroidExecutorPoolWrite", false),
        ("/executor-pools/{pool}/projects", "get", "", false),
        (
            "/executor-pools/{pool}/projects",
            "post",
            "AndroidExecutorPoolProjectWrite",
            false,
        ),
        ("/operations/status", "get", "", false),
        ("/operations/support-bundle", "get", "", false),
        ("/executors/{executor}/sync-grants", "get", "", false),
        (
            "/executors/{executor}/sync-grants",
            "post",
            "AndroidExecutorSyncGrantWrite",
            false,
        ),
        ("/agent/executors/{executor}/sync/changes", "get", "", true),
        (
            "/agent/executors/{executor}/sync/changes",
            "post",
            "AndroidExecutorSyncBatch",
            true,
        ),
        (
            "/agent/executors/{executor}/sync/entities/{kind}",
            "get",
            "",
            true,
        ),
        ("/agent/executors/{executor}/sync/events", "get", "", true),
        ("/sync/device", "get", "", false),
        ("/sync/operations/{operation}", "get", "", false),
        ("/sync/changes", "post", "AndroidWireSyncBatch", false),
    ] {
        let bytes = path.ends_with("/bytes");
        let sse = path.ends_with("/events");
        let description = if bytes {
            "Original bounded byte page with current authority"
        } else if sse {
            "Current-authorized original sync_change event stream"
        } else {
            "Current state, real page cursor, or immutable original response"
        };
        let schema = if bytes || sse {
            json!({"type":"string","format":if bytes{"binary"}else{"text"}})
        } else {
            json!({"type":"object"})
        };
        let mime = if bytes {
            "application/octet-stream"
        } else if sse {
            "text/event-stream"
        } else {
            "application/json"
        };
        let mut op = json!({"responses":{"200":{"description":description,"content":{}},"403":{"description":"Current actor/device/session/project authority unavailable"},"409":{"description":"Original request, source or revision conflict"},"413":{"description":"Explicit bounded request/page exceeded"}}});
        let mut auth = serde_json::Map::new();
        auth.insert(
            if device {
                "deviceAgent"
            } else {
                "actorSession"
            }
            .to_owned(),
            json!([]),
        );
        op["security"] = json!([auth]);
        op["responses"]["200"]["content"][mime] = json!({"schema":schema});
        if !input.is_empty() {
            op["requestBody"] = if input == "binary" {
                json!({"required":true,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary","maxLength":1048576}}}})
            } else {
                json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}})
            };
        }
        let mut p = Vec::new();
        for name in [
            "id",
            "actor",
            "pool",
            "executor",
            "operation",
            "key",
            "ordinal",
            "kind",
        ] {
            if path.contains(&format!("{{{name}}}")) {
                p.push(json!({"name":name,"in":"path","required":true,"schema":if name=="ordinal"{json!({"type":"integer","minimum":0})}else{json!({"type":"string"})}}));
            }
        }
        if method == "get" {
            if path.contains("/sync/") && device {
                p.push(json!({"name":"project_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if path.ends_with("/entries") {
                p.push(
                    json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0}}),
                );
                p.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}}));
            }
            if path == "/mobile-cache/imports"
                || path == "/support-grants"
                || path == "/executor-pools"
                || path == "/executor-pools/{pool}/projects"
            {
                p.push(
                    json!({"name":"after","in":"query","schema":{"type":"string","format":"uuid"}}),
                );
                p.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}}));
            }
            if path == "/executor-pools" || path == "/executor-pools/{pool}/projects" {
                p.push(json!({"name":"organization_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if path == "/support-grants" || path.starts_with("/operations/") {
                p.push(json!({"name":"project_id","in":"query","schema":{"type":"string","format":"uuid"}}));
            }
            if path.ends_with("/{key}") || path.ends_with("/{operation}") {
                p.push(json!({"name":"request_sha256","in":"query","required":true,"schema":{"type":"string","pattern":"^[a-f0-9]{64}$"}}));
            }
            if path.ends_with("/{key}") {
                p.push(json!({"name":"purpose","in":"query","required":true,"schema":{"type":"string"}}));
            }
            if bytes {
                p.push(json!({"name":"stream","in":"query","required":true,"schema":{"type":"string","description":"archive, payload, or entry:<positive ordinal>"}}));
                p.push(json!({"name":"ordinal","in":"query","required":true,"schema":{"type":"integer","minimum":0}}));
                op["responses"]["200"]["headers"] = json!({"x-content-sha256":{"schema":{"type":"string","pattern":"^[a-f0-9]{64}$"}},"x-content-size":{"schema":{"type":"integer","minimum":0,"maximum":1048576}}});
            }
            if device && path.ends_with("/changes") {
                p.push(
                    json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0}}),
                );
                p.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":1000}}));
            }
            if device && path.contains("/entities/") {
                p.push(
                    json!({"name":"after","in":"query","schema":{"type":"string","format":"uuid"}}),
                );
                p.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":1000}}));
            }
        }
        op["parameters"] = json!(p);
        document["paths"][format!("/api/v1{path}")][method] = op;
    }
}
