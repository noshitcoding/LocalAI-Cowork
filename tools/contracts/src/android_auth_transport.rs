use serde_json::{Value, json};
/// Authentication transports retain exact original inputs and per-operation authority.
pub fn extend(document: &mut Value) {
    let routes = [
        (
            "/api/v1/auth/security-context",
            "get",
            "",
            "AndroidAuthSecurityContext",
            false,
            "session",
        ),
        (
            "/api/v1/auth/native/authorize",
            "post",
            "AndroidNativeAuthorization",
            "AndroidNativeAuthorizationCode",
            false,
            "public",
        ),
        (
            "/api/v1/auth/native/token",
            "post",
            "AndroidNativeTokenExchange",
            "AndroidAuthTokens",
            false,
            "public",
        ),
        (
            "/api/v1/auth/refresh",
            "post",
            "AndroidRefreshSession",
            "AndroidAuthTokens",
            false,
            "public",
        ),
        (
            "/api/v1/auth/browser/refresh",
            "post",
            "",
            "AndroidBrowserAuthTokens",
            false,
            "cookie",
        ),
        (
            "/api/v2/operator/auth-security",
            "get",
            "",
            "AndroidAuthManagementPolicy",
            false,
            "owner",
        ),
        (
            "/api/v2/operator/auth-security",
            "put",
            "SetAndroidAuthManagementPolicy",
            "AndroidAuthManagementPolicy",
            false,
            "owner",
        ),
        (
            "/api/v2/native/auth-transports",
            "get",
            "",
            "NativeAuthTransportView",
            true,
            "native",
        ),
        (
            "/api/v2/native/auth-transports",
            "post",
            "NativeAuthTransportRequest",
            "NativeAuthTransportView",
            false,
            "native",
        ),
        (
            "/api/v2/native/auth-transports/{operation}",
            "get",
            "",
            "NativeAuthTransportReplay",
            false,
            "native",
        ),
        (
            "/api/v2/native/auth-transports/capture",
            "post",
            "NativeAuthTransportCapture",
            "NativeAuthTransportView",
            false,
            "native",
        ),
        (
            "/api/v2/native/auth-transports/resolve",
            "post",
            "NativeAuthTransportResolution",
            "NativeAuthTransportView",
            false,
            "native",
        ),
    ];
    document["components"]["securitySchemes"]["androidRefreshCookie"] = json!({"type":"apiKey","in":"cookie","name":"__Host-cowork_refresh","description":"Secure, HttpOnly, SameSite=Strict, host-only refresh cookie; requires current configured HTTPS origin and an exact browser mode header."});
    for (path, method, request, response, array, authority) in routes {
        let security = match authority {
            "session" => json!([{"androidSession":[]}]),
            "owner" => json!([{"daemonBearer":[]}]),
            "native" => json!([{"daemonBearer":[],"nativeAuthority":[]}]),
            "cookie" => json!([{"androidRefreshCookie":[]}]),
            _ => json!([]),
        };
        let result = json!({"$ref":format!("#/components/schemas/{response}")});
        let mut operation = json!({"servers":[{"url":"/"}],"security":security,"responses":{"200":{"description":"Committed original result; replay requires the original operation and still-live authority.","content":{"application/json":{"schema":if array{json!({"type":"array","items":result})}else{result}}}},"default":{"description":"Denied, stale or unconfirmed original operation; no replacement ceremony or session is issued."}}});
        if !request.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}});
        }
        let mut parameters = Vec::new();
        if path.contains("{operation}") {
            parameters.push(json!({"name":"operation","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
        }
        if path.starts_with("/api/v1/") && method == "post" {
            parameters.push(json!({"name":"Idempotency-Key","in":"header","required":false,"schema":{"type":"string","format":"uuid"},"description":"Canonical nonnil original UUID. Keep the same key and exact input after response loss. Legacy omission starts one new operation and cannot recover a consumed grant."}));
        }
        if authority == "cookie" {
            parameters.push(json!({"name":"X-Cowork-Session-Mode","in":"header","required":true,"schema":{"type":"string","const":"browser-cookie"}}));
            parameters.push(json!({"name":"Origin","in":"header","required":true,"schema":{"type":"string"},"description":"Must exactly match current configured HTTPS origin and actual Host; forwarded headers are not authority."}));
            operation["responses"]["200"]["headers"]["Set-Cookie"] = json!({"description":"Original host-only successor refresh cookie. JSON contains no refresh token.","schema":{"type":"string"}});
        }
        operation["parameters"] = json!(parameters);
        document["paths"][path][method] = operation;
    }
    let native_paths = [
        (
            "/api/v1/auth/native/passkey/register/start",
            true,
            json!({"label":{"type":"string","minLength":1,"maxLength":200},"device_id":{"type":"string","format":"uuid"},"code_challenge":{"type":"string","pattern":"^[A-Za-z0-9_-]{43}$"},"state":{"type":"string","minLength":43,"maxLength":128},"redirect_uri":{"type":"string"}}),
            vec![
                "label",
                "device_id",
                "code_challenge",
                "state",
                "redirect_uri",
            ],
        ),
        (
            "/api/v1/auth/native/passkey/register/options",
            false,
            json!({"ticket":{"type":"string","pattern":"^cw2a_[0-9a-f]{64}$"}}),
            vec!["ticket"],
        ),
        (
            "/api/v1/auth/native/passkey/register/finish",
            false,
            json!({"ticket":{"type":"string","pattern":"^cw2a_[0-9a-f]{64}$"},"credential":{"type":"object"}}),
            vec!["ticket", "credential"],
        ),
        (
            "/api/v1/auth/native/passkey/finish",
            false,
            json!({"challenge_id":{"type":"string","format":"uuid"},"authorization_id":{"type":"string","format":"uuid"},"credential":{"type":"object"}}),
            vec!["challenge_id", "authorization_id", "credential"],
        ),
    ];
    for (path, authenticated, properties, required) in native_paths {
        let mut op = json!({"servers":[{"url":"/"}],"security":if authenticated{json!([{"androidSession":[]}])}else{json!([])},"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}}},"responses":{"200":{"description":"Original durable native handoff response; Finish replay uses the same credential and original challenge, never a new WebAuthn ceremony."},"default":{"description":"Denied or expired original operation."}}});
        if authenticated {
            op["parameters"] = json!([{"name":"Idempotency-Key","in":"header","required":false,"schema":{"type":"string","format":"uuid"},"description":"Original registration-start UUID. Challenge, encrypted ticket and original response commit together."}]);
        }
        let response_schema = if path.ends_with("register/start") {
            json!({"type":"object","required":["ticket","expires_at"],"properties":{"ticket":{"type":"string","pattern":"^cw2a_[0-9a-f]{64}$"},"expires_at":{"type":"string","format":"date-time"}},"additionalProperties":false})
        } else if path.ends_with("register/options") {
            json!({"type":"object","required":["public_key","label","state","redirect_uri"],"properties":{"public_key":{"type":"object"},"label":{"type":"string"},"state":{"type":"string"},"redirect_uri":{"type":"string"}},"additionalProperties":false})
        } else if path.ends_with("register/finish") {
            json!({"type":"object","required":["redirect_url","expires_at"],"properties":{"redirect_url":{"type":"string","format":"uri"},"expires_at":{"type":"string","format":"date-time"}},"additionalProperties":false})
        } else {
            json!({"type":"object","required":["schema_version","redirect_url","expires_at"],"properties":{"schema_version":{"type":"integer","const":2},"redirect_url":{"type":"string","format":"uri"},"expires_at":{"type":"string","format":"date-time"}},"additionalProperties":false})
        };
        op["responses"]["200"]["content"]["application/json"]["schema"] = response_schema;
        document["paths"][path]["post"] = op;
    }
    document["paths"]["/api/v1/auth/native/passkey/register/ack"]["post"] = json!({"servers":[{"url":"/"}],"security":[],"parameters":[{"name":"Idempotency-Key","in":"header","required":false,"schema":{"type":"string","format":"uuid"}}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/AndroidNativeTokenExchange"}}}},"responses":{"200":{"description":"Original registration receipt; exact PKCE verifier, device and still-live original session required."},"default":{"description":"Denied or expired original operation."}}});
    document["paths"]["/api/v1/auth/native/passkey/register/ack"]["post"]["responses"]["200"]["content"]
        ["application/json"]["schema"] = json!({"type":"object","required":["registered","passkey_id"],"properties":{"registered":{"type":"boolean","const":true},"passkey_id":{"type":"string","format":"uuid"}},"additionalProperties":false});
    let browser = json!({"oneOf":[{"$ref":"#/components/schemas/AndroidAuthTokens"},{"$ref":"#/components/schemas/AndroidBrowserAuthTokens"}]});
    document["paths"]["/api/v1/auth/login"]["post"]["responses"]["200"]["content"]["application/json"]
        ["schema"] = browser;
}
