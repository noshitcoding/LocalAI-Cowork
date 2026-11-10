//! Encrypted native credential custody only. These snapshots attest no remote HTTP effect.
use super::native_client::NativeClient;
use cowork_contracts::{
    NativeAuthTransportReplay, NativeAuthTransportRequest, NativeAuthTransportResolution,
    NativeAuthTransportView,
};
use zeroize::Zeroizing;
fn client(window: &tauri::WebviewWindow) -> Result<NativeClient, String> {
    NativeClient::new(
        window,
        "native_oidc_custody_unavailable",
        "native_oidc_custody_unconfirmed",
    )
    .map_err(|e| e.code)
}
#[tauri::command]
pub async fn native_oidc_store(
    window: tauri::WebviewWindow,
    endpoint: String,
    body_json: String,
    actor_id: Option<uuid::Uuid>,
    session_id: Option<uuid::Uuid>,
) -> Result<NativeAuthTransportView, String> {
    let endpoint = super::passkey::server(&endpoint)?.to_string();
    let body = Zeroizing::new(body_json);
    let request = NativeAuthTransportRequest {
        operation_id: uuid::Uuid::new_v4(),
        endpoint,
        purpose: "oidc_state".into(),
        body_json: body.to_string(),
        actor_id,
        session_id,
    };
    client(&window)?
        .post("/native/auth-transports", &request)
        .await
        .map_err(|e| e.code)
}
#[tauri::command]
pub async fn native_oidc_pending(
    window: tauri::WebviewWindow,
    endpoint: String,
) -> Result<Vec<NativeAuthTransportView>, String> {
    let endpoint = super::passkey::server(&endpoint)?.to_string();
    let c = client(&window)?;
    let values: Vec<NativeAuthTransportView> = c
        .result(
            c.request(reqwest::Method::GET, "/native/auth-transports"),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    Ok(values
        .into_iter()
        .filter(|v| v.endpoint == endpoint && v.purpose == "oidc_state")
        .collect())
}
#[tauri::command]
pub async fn native_oidc_read(
    window: tauri::WebviewWindow,
    operation_id: uuid::Uuid,
) -> Result<NativeAuthTransportReplay, String> {
    let c = client(&window)?;
    let value: NativeAuthTransportReplay = c
        .result(
            c.request(
                reqwest::Method::GET,
                &format!("/native/auth-transports/{operation_id}"),
            ),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    if value.request.purpose != "oidc_state" {
        return Err("native_oidc_custody_mismatch".into());
    }
    Ok(value)
}
#[tauri::command]
pub async fn native_oidc_forget(
    window: tauri::WebviewWindow,
    operation_id: uuid::Uuid,
) -> Result<NativeAuthTransportView, String> {
    let c = client(&window)?;
    let value: NativeAuthTransportReplay = c
        .result(
            c.request(
                reqwest::Method::GET,
                &format!("/native/auth-transports/{operation_id}"),
            ),
            false,
        )
        .await
        .map_err(|e| e.code)?;
    if value.request.purpose != "oidc_state" {
        return Err("native_oidc_custody_mismatch".into());
    }
    c.post(
        "/native/auth-transports/resolve",
        &NativeAuthTransportResolution {
            operation_id,
            acknowledged: false,
        },
    )
    .await
    .map_err(|e| e.code)
}
