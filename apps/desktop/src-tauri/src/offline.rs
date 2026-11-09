//! All keys and local execution use NativeClient, never renderer HTTP authority.
use super::native_client::{NativeClient, failure};
use cowork_contracts::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use uuid::Uuid;

pub(super) async fn native(window: &tauri::WebviewWindow) -> Result<NativeClient, ApiError> {
    if !super::application_window(window) {
        return Err(failure("forbidden"));
    }
    // Initialize/adopt the matching local sidecar even while the renderer's
    // selected workspace is a remote team. Keep its owner token inside Rust.
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| failure("offline_native_unavailable"))?;
    NativeClient::new(
        window,
        "offline_native_unavailable",
        "offline_operation_unconfirmed",
    )
}
async fn confirm(window: &tauri::WebviewWindow, title: &str, body: &str) -> Result<(), ApiError> {
    if !super::application_window(window) {
        return Err(failure("forbidden"));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(body)
        .title(title)
        .parent(window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Ausdrücklich vertrauen".into(),
            "Abbrechen".into(),
        ))
        .show(move |answer| {
            let _ = sender.send(answer);
        });
    if receiver
        .await
        .map_err(|_| failure("offline_confirmation_lost"))?
    {
        Ok(())
    } else {
        Err(failure("offline_confirmation_declined"))
    }
}
fn origin(value: &str) -> Result<reqwest::Url, ApiError> {
    let u = reqwest::Url::parse(value).map_err(|_| failure("invalid_offline_server_origin"))?;
    if value != u.origin().ascii_serialization()
        || !matches!(u.scheme(), "https" | "http")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(failure("invalid_offline_server_origin"));
    }
    Ok(u)
}
async fn remote<T: DeserializeOwned>(
    url: String,
    session: &str,
    body: &impl Serialize,
) -> Result<T, ApiError> {
    if session.len() > 4096 {
        return Err(failure("team_login_required"));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| failure("offline_team_connection_failed"))?;
    let bytes = serde_json::to_vec(body).map_err(|_| failure("invalid_request"))?;
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .body(bytes);
    if !session.is_empty() {
        request = request.bearer_auth(session);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| failure("offline_team_connection_failed"))?;
    let success = response.status().is_success();
    let mut bytes = Vec::new();
    const LIMIT: usize = 8 * 1024 * 1024;
    if response.content_length().is_some_and(|v| v > LIMIT as u64) {
        return Err(failure("offline_response_too_large"));
    }
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("offline_team_receive_failed"))?
    {
        if bytes.len() + chunk.len() > LIMIT {
            return Err(failure("offline_response_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !success {
        return Err(failure("offline_team_permission_or_revision_rejected"));
    }
    serde_json::from_slice(&bytes).map_err(|_| failure("offline_team_response_invalid"))
}
pub(super) async fn local_result<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, ApiError> {
    let mut response = request
        .send()
        .await
        .map_err(|_| failure("offline_native_unavailable"))?;
    let success = response.status().is_success();
    const LIMIT: usize = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    if response.content_length().is_some_and(|v| v > LIMIT as u64) {
        return Err(failure("offline_response_too_large"));
    }
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("offline_operation_unconfirmed"))?
    {
        if bytes.len() + chunk.len() > LIMIT {
            return Err(failure("offline_response_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !success {
        return Err(serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| failure("offline_operation_unconfirmed")));
    }
    serde_json::from_slice(&bytes).map_err(|_| failure("offline_native_response_invalid"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIssueOfflineGrant {
    #[serde(default)]
    pub output: Option<StructuredOutput>,
    pub source_origin: String,
    pub session: String,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub idempotency_key: String,
    pub prompt: String,
    pub offline_base_url: String,
    pub offline_model: String,
    pub allowed_tools: Vec<String>,
    pub expires_in_seconds: u32,
    pub maximum_runs: u32,
    pub maximum_duration_seconds: u32,
}
#[tauri::command]
pub async fn offline_device(window: tauri::WebviewWindow) -> Result<OfflineDevice, ApiError> {
    native(&window)
        .await?
        .post("/offline/device", &serde_json::json!({}))
        .await
}
#[tauri::command]
pub async fn offline_grants(
    window: tauri::WebviewWindow,
) -> Result<Vec<OfflineLocalGrantView>, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(reqwest::Method::GET, "/offline/grants")).await
}
#[tauri::command]
pub async fn offline_runs(
    window: tauri::WebviewWindow,
    cursor: Option<String>,
) -> Result<OfflineRunPage, ApiError> {
    let n = native(&window).await?;
    let request = n
        .request(reqwest::Method::GET, "/offline/runs")
        .query(&[("cursor", cursor.unwrap_or_default())]);
    local_result(request).await
}
#[tauri::command]
pub async fn offline_run(window: tauri::WebviewWindow, run: Uuid) -> Result<OfflineRun, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(reqwest::Method::GET, &format!("/offline/runs/{run}"))).await
}
#[tauri::command]
pub async fn offline_ticket(
    window: tauri::WebviewWindow,
    grant: Uuid,
) -> Result<SignedOfflineGrant, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(reqwest::Method::GET, &format!("/offline/grants/{grant}"))).await
}
#[tauri::command]
pub async fn offline_issue_grant(
    window: tauri::WebviewWindow,
    input: NativeIssueOfflineGrant,
) -> Result<SignedOfflineGrant, ApiError> {
    if input.session.is_empty() {
        return Err(failure("team_login_required"));
    }
    let url = origin(&input.source_origin)?;
    let n = native(&window).await?;
    if url.scheme() == "http" {
        confirm(&window,"Unverschlüsselter Teamserver",&format!("Die Teamsitzung wird ausschließlich an den ausdrücklich ausgewählten Server {} gesendet. HTTP schützt die Übertragung nicht. Diesen Betreiberendpunkt bewusst verwenden?",input.source_origin)).await?;
    }
    let request = OfflineDeviceProofRequest {
        issuer_origin: input.source_origin.clone(),
        team_id: input.team_id,
        project_id: input.project_id,
        order_id: input.order_id,
        idempotency_key: input.idempotency_key,
    };
    let proof: OfflineDeviceProof = n.post("/offline/device/proof", &request).await?;
    let body = IssueOfflineGrant {
        output: input.output,
        proof,
        prompt: input.prompt,
        offline_base_url: input.offline_base_url,
        offline_model: input.offline_model,
        allowed_tools: input.allowed_tools,
        expires_in_seconds: input.expires_in_seconds,
        maximum_runs: input.maximum_runs,
        maximum_duration_seconds: input.maximum_duration_seconds,
    };
    let ticket: SignedOfflineGrant = remote(
        format!(
            "{}/api/v2/team-offline/{}/{}/grants",
            input.source_origin, input.team_id, input.project_id
        ),
        &input.session,
        &body,
    )
    .await?;
    if ticket.grant.issuer_origin != input.source_origin
        || ticket.grant.team_id != input.team_id
        || ticket.grant.project_id != input.project_id
        || ticket.grant.order_id != input.order_id
        || ticket.grant.device != body.proof.device
        || ticket.grant.output != body.output
    {
        return Err(failure("offline_server_scope_mismatch"));
    }
    if url.scheme() == "http" {
        confirm(&window,"Offline-Serverschlüssel pinnen",&format!("Server: {}\nÖffentlicher Ed25519-Schlüssel: {}\nDiesen Schlüssel für Offlinefreigaben bewusst auf diesem Gerät pinnen?",input.source_origin,ticket.grant.issuer_public_key)).await?;
    }
    // HTTPS returned this key through the authenticated, no-redirect connection.
    // HTTP requires the distinct native key confirmation above. The local DB rejects key changes.
    n.post(
        "/offline/grants/install",
        &InstallOfflineGrant {
            confirmed_issuer_origin: input.source_origin,
            confirmed_issuer_public_key: ticket.grant.issuer_public_key.clone(),
            ticket,
        },
    )
    .await
}
#[tauri::command]
pub async fn offline_import_grant(
    window: tauri::WebviewWindow,
    ticket: SignedOfflineGrant,
) -> Result<SignedOfflineGrant, ApiError> {
    origin(&ticket.grant.issuer_origin)?;
    let n = native(&window).await?;
    confirm(&window,"Übertragene Offlinefreigabe bestätigen",&format!("Server: {}\nÖffentlicher Ed25519-Schlüssel: {}\nGerät: {}\nTeam: {}\nProjekt: {}\nAuftrag: {}\nNur vertrauen, wenn dieser Schlüssel unabhängig mit dem Betreiber abgeglichen wurde. Eine mitgelieferte Signatur allein belegt keine vertrauenswürdige Herkunft.",ticket.grant.issuer_origin,ticket.grant.issuer_public_key,ticket.grant.device.id,ticket.grant.team_id,ticket.grant.project_id,ticket.grant.order_id)).await?;
    n.post(
        "/offline/grants/install",
        &InstallOfflineGrant {
            confirmed_issuer_origin: ticket.grant.issuer_origin.clone(),
            confirmed_issuer_public_key: ticket.grant.issuer_public_key.clone(),
            ticket,
        },
    )
    .await
}
#[tauri::command]
pub async fn offline_configure_key(
    window: tauri::WebviewWindow,
    input: ConfigureOfflineCredential,
) -> Result<serde_json::Value, ApiError> {
    native(&window)
        .await?
        .post("/offline/model-key", &input)
        .await
}
#[tauri::command]
pub async fn offline_configure_tools(
    window: tauri::WebviewWindow,
    input: ConfigureOfflineTools,
) -> Result<OfflineToolConfigurationView, ApiError> {
    native(&window)
        .await?
        .post("/offline/tool-config", &input)
        .await
}
#[tauri::command]
pub async fn offline_tool_configuration(
    window: tauri::WebviewWindow,
    grant: Uuid,
) -> Result<OfflineToolConfigurationView, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(
        reqwest::Method::GET,
        &format!("/offline/grants/{grant}/tool-config"),
    ))
    .await
}
#[tauri::command]
pub async fn offline_cancel(
    window: tauri::WebviewWindow,
    run: Uuid,
) -> Result<OfflineRun, ApiError> {
    native(&window)
        .await?
        .post(
            &format!("/offline/runs/{run}/cancel"),
            &serde_json::json!({}),
        )
        .await
}
#[tauri::command]
pub async fn offline_workspace_file(
    window: tauri::WebviewWindow,
    run: Uuid,
    path: String,
) -> Result<Vec<u8>, ApiError> {
    let n = native(&window).await?;
    let mut response = n
        .request(
            reqwest::Method::GET,
            &format!("/offline/runs/{run}/workspace-file"),
        )
        .query(&[("path", path)])
        .send()
        .await
        .map_err(|_| failure("offline_workspace_download_unconfirmed"))?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > 32 * 1024 * 1024)
    {
        return Err(failure("offline_workspace_download_rejected"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("offline_workspace_download_interrupted"))?
    {
        if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
            return Err(failure("offline_workspace_download_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[tauri::command]
pub async fn offline_workspace_upload(
    window: tauri::WebviewWindow,
    run: Uuid,
    input: OfflineWorkspaceUpload,
) -> Result<OfflineRun, ApiError> {
    native(&window)
        .await?
        .post(&format!("/offline/runs/{run}/workspace-file"), &input)
        .await
}
#[tauri::command]
pub async fn offline_start(
    window: tauri::WebviewWindow,
    input: StartOfflineRun,
) -> Result<OfflineRun, ApiError> {
    native(&window).await?.post("/offline/runs", &input).await
}
#[tauri::command]
pub async fn offline_approve(
    window: tauri::WebviewWindow,
    run: Uuid,
    input: ApproveOfflineTools,
) -> Result<OfflineRun, ApiError> {
    native(&window)
        .await?
        .post(&format!("/offline/runs/{run}/approve"), &input)
        .await
}
#[tauri::command]
pub async fn offline_export_run(
    window: tauri::WebviewWindow,
    run: Uuid,
) -> Result<OfflineRunExport, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(reqwest::Method::GET, &format!("/offline/runs/{run}/export"))).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeReconcileOffline {
    pub source_origin: String,
    pub session: String,
    pub run: Uuid,
}
#[tauri::command]
pub async fn offline_reconcile(
    window: tauri::WebviewWindow,
    input: NativeReconcileOffline,
) -> Result<OfflineReconciliationReceipt, ApiError> {
    if input.session.is_empty() {
        return Err(failure("team_login_required"));
    }
    let u = origin(&input.source_origin)?;
    let n = native(&window).await?;
    let report: OfflineRunExport = local_result(n.request(
        reqwest::Method::GET,
        &format!("/offline/runs/{}/export", input.run),
    ))
    .await?;
    if report.ticket.grant.issuer_origin != input.source_origin {
        return Err(failure("offline_source_server_changed"));
    }
    if u.scheme() == "http" {
        confirm(
            &window,
            "Offlineverlauf übertragen",
            &format!(
                "Signierten Verlauf mit der aktuellen Teamsitzung ausdrücklich an {} übertragen?",
                input.source_origin
            ),
        )
        .await?;
    }
    remote(
        format!(
            "{}/api/v2/team-offline/{}/{}/reconcile",
            input.source_origin, report.ticket.grant.team_id, report.ticket.grant.project_id
        ),
        &input.session,
        &report,
    )
    .await
}
#[tauri::command]
pub async fn offline_check_grant(
    window: tauri::WebviewWindow,
    grant: Uuid,
) -> Result<OfflineGrantStatus, ApiError> {
    let n = native(&window).await?;
    let ticket: SignedOfflineGrant =
        local_result(n.request(reqwest::Method::GET, &format!("/offline/grants/{grant}"))).await?;
    origin(&ticket.grant.issuer_origin)?;
    let request: OfflineGrantStatusRequest = n
        .post(
            &format!("/offline/grants/{grant}/status-request"),
            &serde_json::json!({}),
        )
        .await?;
    // Only device proof is sent. No teamsession is needed to receive a signed revocation notice.
    let status: SignedOfflineGrantStatus = remote(
        format!(
            "{}/api/v2/offline-authority/status",
            ticket.grant.issuer_origin
        ),
        "",
        &request,
    )
    .await?;
    if status.status.grant_id != grant
        || status.status.nonce != request.nonce
        || status.status.issuer_origin != ticket.grant.issuer_origin
    {
        return Err(failure("offline_status_scope_mismatch"));
    }
    n.post("/offline/grants/status", &status).await
}
