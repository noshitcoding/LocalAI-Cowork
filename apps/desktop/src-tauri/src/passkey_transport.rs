//! Exact native auth POSTs are retained before sending; recovery never starts a ceremony.
use super::native_client::NativeClient;
use cowork_contracts::{
    NativeAuthTransportCapture, NativeAuthTransportReplay, NativeAuthTransportRequest,
    NativeAuthTransportResolution, NativeAuthTransportView,
};
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};
fn client(window: &tauri::WebviewWindow) -> Result<NativeClient, String> {
    NativeClient::new(
        window,
        "native_auth_journal_unavailable",
        "native_auth_journal_unconfirmed",
    )
    .map_err(|e| e.code)
}
pub(crate) async fn pending(
    window: &tauri::WebviewWindow,
    endpoint: &str,
) -> Result<Vec<NativeAuthTransportView>, String> {
    let c = client(window)?;
    let rows: Vec<NativeAuthTransportView> = c
        .result(
            c.request(reqwest::Method::GET, "/native/auth-transports"),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    Ok(rows
        .into_iter()
        .filter(|v| {
            v.endpoint == endpoint
                && matches!(v.purpose.as_str(), "native_token" | "registration_ack")
        })
        .collect())
}
pub(crate) async fn ready(window: &tauri::WebviewWindow, endpoint: &str) -> Result<(), String> {
    let c = client(window)?;
    let status: cowork_contracts::NativeSecretsStatus = c
        .result(c.request(reqwest::Method::GET, "/native/secrets"), false)
        .await
        .map_err(|e| e.code)?;
    if !status.configured || !status.key_available || status.pending_operation_id.is_some() {
        return Err("native_auth_key_setup_required".into());
    }
    if !pending(window, endpoint).await?.is_empty() {
        return Err("native_auth_original_pending".into());
    }
    Ok(())
}
fn request(
    endpoint: &str,
    purpose: &str,
    body: &str,
    actor: Option<(uuid::Uuid, uuid::Uuid)>,
) -> Result<NativeAuthTransportRequest, String> {
    let url = super::passkey::server(endpoint)?;
    if !matches!(purpose, "native_token" | "registration_ack") {
        return Err("native_auth_request_invalid".into());
    }
    Ok(NativeAuthTransportRequest {
        operation_id: uuid::Uuid::new_v4(),
        endpoint: url.as_str().into(),
        purpose: purpose.into(),
        body_json: body.into(),
        actor_id: actor.map(|v| v.0),
        session_id: actor.map(|v| v.1),
    })
}
fn path(purpose: &str) -> Result<&'static str, String> {
    match purpose {
        "native_token" => Ok("api/v1/auth/native/token"),
        "registration_ack" => Ok("api/v1/auth/native/passkey/register/ack"),
        _ => Err("native_auth_request_invalid".into()),
    }
}
fn validate_result(v: &NativeAuthTransportRequest, response: &str) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(response).map_err(|_| "native_auth_response_invalid")?;
    if v.purpose == "native_token" {
        let parsed: cowork_contracts::AndroidAuthTokens =
            serde_json::from_value(value.clone()).map_err(|_| "native_auth_response_invalid")?;
        if parsed.schema_version != 2 || parsed.user_id.is_nil() || parsed.session_id.is_nil() {
            return Err("native_auth_response_invalid".into());
        }
    } else if value["registered"] != true
        || value["passkey_id"]
            .as_str()
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
            .is_none_or(|v| v.is_nil())
    {
        return Err("native_auth_response_invalid".into());
    }
    Ok(value)
}
pub(crate) async fn actor(
    http: &reqwest::Client,
    endpoint: &reqwest::Url,
    token: &str,
) -> Result<(uuid::Uuid, uuid::Uuid), String> {
    let value = super::passkey::bounded(
        http.get(
            endpoint
                .join("api/v1/auth/security-context")
                .map_err(|_| "native_auth_address_invalid")?,
        )
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "native_auth_session_unavailable")?,
    )
    .await?;
    let context: cowork_contracts::AndroidAuthSecurityContext =
        serde_json::from_value(value).map_err(|_| "native_auth_context_invalid")?;
    if context.schema_version != 2 || context.user_id.is_nil() || context.session_id.is_nil() {
        return Err("native_auth_context_invalid".into());
    }
    Ok((context.user_id, context.session_id))
}
async fn send(
    http: &reqwest::Client,
    v: &NativeAuthTransportRequest,
) -> Result<Zeroizing<String>, String> {
    let endpoint = super::passkey::server(&v.endpoint)?;
    let builder = || -> Result<reqwest::RequestBuilder, String> {
        Ok(http
            .post(
                endpoint
                    .join(path(&v.purpose)?)
                    .map_err(|_| "native_auth_address_invalid")?,
            )
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", v.operation_id.to_string())
            .body(v.body_json.clone()))
    };
    // Repeated contact is the identical authentication operation. The receiver
    // returns its committed original receipt; no second session is issued.
    let mut last = "native_auth_response_unconfirmed".to_string();
    for _ in 0..2 {
        match builder()?.send().await {
            Ok(r) => {
                let value = super::passkey::bounded(r).await?;
                return Ok(Zeroizing::new(
                    serde_json::to_string(&value).map_err(|_| "native_auth_response_invalid")?,
                ));
            }
            Err(_) => last = "native_auth_response_unconfirmed".into(),
        }
    }
    Err(last)
}
fn outcome(v: &NativeAuthTransportRequest, response: &str) -> Value {
    json!({"operation_id":v.operation_id,"purpose":v.purpose,"response_json":response})
}
pub(crate) async fn perform(
    window: &tauri::WebviewWindow,
    http: &reqwest::Client,
    endpoint: &str,
    purpose: &str,
    body: &str,
    actor: Option<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Value, String> {
    let v = request(endpoint, purpose, body, actor)?;
    let c = client(window)?;
    let saved: NativeAuthTransportView = c
        .post("/native/auth-transports", &v)
        .await
        .map_err(|e| e.code)?;
    if saved.operation_id != v.operation_id
        || saved.endpoint != v.endpoint
        || saved.purpose != v.purpose
        || saved.state != "prepared"
    {
        return Err("native_auth_journal_mismatch".into());
    }
    let response = send(http, &v).await?;
    validate_result(&v, &response)?;
    let _: NativeAuthTransportView = c
        .post(
            "/native/auth-transports/capture",
            &NativeAuthTransportCapture {
                operation_id: v.operation_id,
                response_json: response.to_string(),
            },
        )
        .await
        .map_err(|e| e.code)?;
    Ok(outcome(&v, &response))
}
pub(crate) async fn resume(
    window: &tauri::WebviewWindow,
    http: &reqwest::Client,
    endpoint: &str,
    operation: uuid::Uuid,
    current_token: Option<&str>,
) -> Result<Value, String> {
    let endpoint = super::passkey::server(endpoint)?;
    let c = client(window)?;
    let mut saved: NativeAuthTransportReplay = c
        .result(
            c.request(
                reqwest::Method::GET,
                &format!("/native/auth-transports/{operation}"),
            ),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    let v = &saved.request;
    if v.operation_id != operation
        || v.endpoint != endpoint.as_str()
        || !matches!(v.purpose.as_str(), "native_token" | "registration_ack")
    {
        return Err("native_auth_journal_mismatch".into());
    }
    if v.purpose == "registration_ack" {
        let actual = actor(
            http,
            &endpoint,
            current_token.ok_or("native_auth_original_session_required")?,
        )
        .await?;
        if Some(actual.0) != v.actor_id || Some(actual.1) != v.session_id {
            return Err("native_auth_original_session_changed".into());
        }
    }
    let response = if let Some(original) = &saved.response_json {
        let value = validate_result(v, original)?;
        if v.purpose == "native_token" {
            let token = Zeroizing::new(
                value["access_token"]
                    .as_str()
                    .ok_or("native_auth_response_invalid")?
                    .to_owned(),
            );
            let actual = actor(http, &endpoint, &token).await?;
            if value["user_id"] != json!(actual.0) || value["session_id"] != json!(actual.1) {
                return Err("native_auth_original_session_changed".into());
            }
            Zeroizing::new(original.clone())
        } else {
            let repeated = send(http, v).await?;
            if repeated.as_str() != original {
                return Err("native_auth_original_response_changed".into());
            }
            repeated
        }
    } else {
        let response = send(http, v).await?;
        validate_result(v, &response)?;
        let _: NativeAuthTransportView = c
            .post(
                "/native/auth-transports/capture",
                &NativeAuthTransportCapture {
                    operation_id: operation,
                    response_json: response.to_string(),
                },
            )
            .await
            .map_err(|e| e.code)?;
        response
    };
    let result = outcome(v, &response);
    saved.request.body_json.zeroize();
    if let Some(v) = &mut saved.response_json {
        v.zeroize();
    }
    Ok(result)
}
pub(crate) async fn resolve(
    window: &tauri::WebviewWindow,
    operation: uuid::Uuid,
    acknowledged: bool,
) -> Result<(), String> {
    let c = client(window)?;
    let _: NativeAuthTransportView = c
        .post(
            "/native/auth-transports/resolve",
            &NativeAuthTransportResolution {
                operation_id: operation,
                acknowledged,
            },
        )
        .await
        .map_err(|e| e.code)?;
    Ok(())
}
