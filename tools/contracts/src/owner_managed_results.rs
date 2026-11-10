//! Owner-only local result views; no execution or native custody DTOs.
use serde_json::{Value, json};

pub(super) fn extend(api: &mut Value) {
    let root = "/orders/{order}/runs/{run}";
    for (suffix, method, response, description) in [
        (
            "/managed-results",
            "get",
            "OwnerManagedResultList",
            "Read local metadata for all original completed managed attempts in this run. At most 128 entries; larger runs return an explicit error. Does not contact the runner or allocate a reception.",
        ),
        (
            "/managed-results/{attempt}/receive",
            "post",
            "OwnerManagedResultEntry",
            "Explicitly receive or resume the original result as one local immutable version. A lost reply is not permission for an automatic retry. An already Ready version is verified locally without the runner.",
        ),
        (
            "/managed-results/{attempt}/delivery",
            "get",
            "OwnerManagedResultDeliveryView",
            "Read original local delivery and retirement history only. Does not send an acknowledgement or grant content access.",
        ),
        (
            "/managed-results/{attempt}/delivery/prepare",
            "post",
            "OwnerManagedResultDeliveryView",
            "Prepare the exact delivery confirmation under current Owner/content rights and verified local Ready custody. Does not contact the runner.",
        ),
        (
            "/managed-results/{attempt}/delivery/confirm",
            "post",
            "OwnerManagedResultDeliveryView",
            "Explicitly commit the displayed identifier and request hash, then send that original confirmation once. A lost or invalid reply remains uncertain; retry requires a separate explicit action with the same intent. Does not retire local Ready data.",
        ),
        (
            "/managed-results/{attempt}/delivery/retirement/refresh",
            "post",
            "OwnerManagedResultDeliveryView",
            "Explicitly read an existing original acknowledgement for a previously committed confirmation, or refresh signed retirement history after acceptance. Each call uses only its separate read-only native endpoint. Never first accepts a delivery, resends an unaccepted confirmation, recreates cleanup authority or deletes local Ready data.",
        ),
        (
            "/managed-result-versions/{version}/manifest",
            "get",
            "OwnerManagedResultManifest",
            "Read a fully verified local Ready manifest under current Owner, order, run, controller, source and working-copy rights. Does not contact the runner or repair missing content.",
        ),
        (
            "/managed-result-versions/{version}/chunk",
            "get",
            "",
            "Read one verified local Ready chunk of at most 4 MiB. The version ID, path and hash do not grant access. Current local rights and the original immutable manifest are required.",
        ),
    ] {
        let path = format!("{root}{suffix}");
        let mut parameters = vec![
            json!({"name":"order","in":"path","required":true,"schema":{"type":"string"}}),
            json!({"name":"run","in":"path","required":true,"schema":{"type":"string"}}),
        ];
        for name in ["attempt", "version"] {
            if suffix.contains(&format!("{{{name}}}")) {
                parameters.push(
                    json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                );
            }
        }
        let cache = json!({"Cache-Control":{"description":"Private result responses are never cached.","schema":{"type":"string","const":"no-store"}}});
        let mut operation = json!({
            "description":description,"security":[{"daemonBearer":[]}],"parameters":parameters,
            "responses":{"200":{"description":"Verified local Owner result.","headers":cache,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{response}")}}}},
            "default":{"description":"Owner authentication, current rights, integrity or transfer check failed.","headers":cache,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}}
        });
        if suffix.ends_with("/delivery/confirm") {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ConfirmOwnerManagedResultDelivery"}}}});
        }
        if suffix.ends_with("/chunk") {
            for name in ["path", "offset", "expected_sha256"] {
                operation["parameters"].as_array_mut().unwrap().push(json!({"name":name,"in":"query","required":true,"schema":{"$ref":format!("#/components/schemas/ReadWorkspaceTransferChunk/properties/{name}")}}));
            }
            operation["responses"]["200"] = json!({"description":description,"headers":{
                "Cache-Control":{"schema":{"type":"string","const":"no-store"}},
                "Content-Disposition":{"description":"Attachment with a UTF-8 percent-encoded filename*.","schema":{"type":"string"}},
                "Content-Length":{"schema":{"type":"integer","minimum":1,"maximum":4194304}},
                "X-Cowork-File-Name":{"description":"Percent-encoded DownloadMetadata.file_name.","schema":{"type":"string"}},
                "X-Cowork-Sha256":{"schema":{"$ref":"#/components/schemas/DownloadMetadata/properties/sha256"}}
            },"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}});
        }
        api["paths"][path][method] = operation;
    }
}
