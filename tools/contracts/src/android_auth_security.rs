use serde_json::{Value, json};
/// Explicit full V1 paths with a per-operation root server. Existing V2 paths
/// and their authorization schemes remain distinct and byte-defined above.
pub fn extend(document: &mut Value) {
    document["components"]["securitySchemes"]["androidSession"] =
        json!({"type":"http","scheme":"bearer"});
    let routes = [
        (
            "/auth/login",
            "post",
            "AndroidPasswordCredentials",
            "AndroidAuthTokens",
            200,
            false,
            false,
        ),
        (
            "/auth/assurance",
            "get",
            "",
            "AndroidAuthenticationAssurance",
            200,
            false,
            true,
        ),
        (
            "/auth/assurance/verify",
            "post",
            "AndroidTotpCode",
            "AndroidAuthenticationAssurance",
            200,
            false,
            true,
        ),
        (
            "/auth/sessions",
            "get",
            "",
            "AndroidAuthSession",
            200,
            true,
            true,
        ),
        (
            "/auth/sessions/{session}",
            "delete",
            "",
            "",
            204,
            false,
            true,
        ),
        (
            "/auth/totp",
            "get",
            "",
            "AndroidTotpStatus",
            200,
            false,
            true,
        ),
        (
            "/auth/totp/setup",
            "post",
            "",
            "AndroidTotpSetup",
            200,
            false,
            true,
        ),
        (
            "/auth/totp/enable",
            "post",
            "AndroidTotpCode",
            "AndroidRecoveryCodes",
            200,
            false,
            true,
        ),
        (
            "/auth/totp/disable",
            "post",
            "AndroidDisableTotp",
            "",
            204,
            false,
            true,
        ),
        (
            "/auth/totp/recovery-codes",
            "post",
            "AndroidTotpCode",
            "AndroidRecoveryCodes",
            200,
            false,
            true,
        ),
        (
            "/auth/invitations",
            "post",
            "AndroidCreateInvitation",
            "AndroidInvitationSecret",
            201,
            false,
            true,
        ),
        (
            "/auth/invitations/accept",
            "post",
            "AndroidAcceptInvitation",
            "AndroidAuthTokens",
            201,
            false,
            false,
        ),
        (
            "/teams",
            "post",
            "AndroidCreateTeam",
            "AndroidTeam",
            201,
            false,
            true,
        ),
        (
            "/teams/{team}/members",
            "get",
            "",
            "AndroidTeamMember",
            200,
            true,
            true,
        ),
        (
            "/teams/{team}/members",
            "post",
            "AndroidSetTeamMember",
            "",
            204,
            false,
            true,
        ),
        (
            "/teams/{team}/members/{user}",
            "delete",
            "",
            "",
            204,
            false,
            true,
        ),
    ];
    for (path, method, request, response, status, array, authenticated) in routes {
        let mut operation = json!({"servers":[{"url":"/"}],"security":if authenticated{json!([{"androidSession":[]}])}else{json!([])},"responses":{"default":{"description":"Compatibility failure with stable nonsecret code","content":{"application/json":{"schema":{"type":"object","required":["error","message"],"properties":{"error":{"type":"string"},"message":{"type":"string"}},"additionalProperties":false}}}}}});
        operation["responses"][status.to_string()] = if response.is_empty() {
            json!({"description":"Confirmed durable original operation"})
        } else {
            let item = json!({"$ref":format!("#/components/schemas/{response}")});
            let schema = if array {
                json!({"type":"array","items":item})
            } else {
                item
            };
            json!({"description":"Actual V2 identity-bound result in original V1 shape","content":{"application/json":{"schema":schema}}})
        };
        if !request.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}});
        }
        let mut parameters = Vec::new();
        for part in path.split('/') {
            if let Some(name) = part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if method == "post" || method == "delete" && path.contains("/members/") {
            parameters.push(json!({"name":"Idempotency-Key","in":"header","required":false,"description":"Retain the original key and exact input after acknowledgement loss. Auth operations require a canonical UUID; omission starts a new original operation for legacy V1 callers.","schema":if path.starts_with("/auth/"){json!({"type":"string","format":"uuid"})}else{json!({"type":"string","minLength":1,"maxLength":128})}}));
        }
        operation["parameters"] = json!(parameters);
        document["paths"][format!("/api/v1{path}")][method] = operation;
    }
}
