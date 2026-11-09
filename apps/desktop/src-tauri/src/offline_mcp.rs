//! Commands share the existing native-only OfflineDevice owner gate.
use super::native_client::failure;
use super::offline::{local_result, native};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_contracts::*;
use sha2::Digest;
use uuid::Uuid;
#[tauri::command]
pub async fn offline_mcp_tasks(
    window: tauri::WebviewWindow,
    run: Uuid,
) -> Result<Vec<McpAsyncTask>, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(
        reqwest::Method::GET,
        &format!("/offline/runs/{run}/mcp/tasks"),
    ))
    .await
}
#[tauri::command]
pub async fn offline_mcp_task_detail(
    window: tauri::WebviewWindow,
    run: Uuid,
    task: Uuid,
) -> Result<McpTaskDetail, ApiError> {
    let n = native(&window).await?;
    local_result(n.request(
        reqwest::Method::GET,
        &format!("/offline/runs/{run}/mcp/tasks/{task}"),
    ))
    .await
}
#[tauri::command]
pub async fn offline_mcp_interactions(
    window: tauri::WebviewWindow,
    run: Uuid,
    after: Option<String>,
    only_waiting: Option<bool>,
) -> Result<OfflineMcpInteractionPage, ApiError> {
    let n = native(&window).await?;
    let request = n
        .request(
            reqwest::Method::GET,
            &format!("/offline/runs/{run}/mcp/interactions"),
        )
        .query(&[("only_waiting", only_waiting.unwrap_or(false))]);
    local_result(if let Some(v) = after {
        request.query(&[("after", v)])
    } else {
        request
    })
    .await
}
#[tauri::command]
pub async fn offline_mcp_answer(
    window: tauri::WebviewWindow,
    run: Uuid,
    interaction: Uuid,
    input: AnswerMcpTaskInteraction,
) -> Result<serde_json::Value, ApiError> {
    native(&window)
        .await?
        .post(
            &format!("/offline/runs/{run}/mcp/interactions/{interaction}/answer"),
            &input,
        )
        .await
}
#[tauri::command]
pub async fn offline_mcp_cancel(
    window: tauri::WebviewWindow,
    run: Uuid,
    task: Uuid,
    input: McpAsyncTaskRequest,
) -> Result<serde_json::Value, ApiError> {
    native(&window)
        .await?
        .post(
            &format!("/offline/runs/{run}/mcp/tasks/{task}/cancel"),
            &input,
        )
        .await
}
#[tauri::command]
pub async fn offline_mcp_result(
    window: tauri::WebviewWindow,
    run: Uuid,
    task: Uuid,
) -> Result<OfflineMcpResultBytes, ApiError> {
    let n = native(&window).await?;
    let mut response = n
        .request(
            reqwest::Method::GET,
            &format!("/offline/runs/{run}/mcp/tasks/{task}/result"),
        )
        .send()
        .await
        .map_err(|_| failure("offline_operation_unconfirmed"))?;
    if !response.status().is_success() {
        return Err(failure("offline_original_result_unavailable"));
    }
    let expected = response
        .headers()
        .get("x-cowork-original-sha256")
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| failure("offline_original_result_invalid"))?
        .to_owned();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("offline_original_receive_failed"))?
    {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err(failure("offline_original_result_too_large"));
        }
        bytes.extend_from_slice(&chunk)
    }
    let digest = sha2::Sha256::digest(&bytes);
    let sha: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if sha != expected {
        return Err(failure("offline_original_result_invalid"));
    }
    Ok(OfflineMcpResultBytes {
        size: bytes.len() as u64,
        sha256: sha,
        bytes_base64: STANDARD.encode(bytes),
    })
}
#[tauri::command]
pub async fn offline_mcp_original_receipt(
    window: tauri::WebviewWindow,
    run: Uuid,
    id: Uuid,
    kind: String,
    input: McpAsyncTaskRequest,
) -> Result<Option<serde_json::Value>, ApiError> {
    let path = match kind.as_str() {
        "answer" => format!("/offline/runs/{run}/mcp/interactions/{id}/receipt"),
        "cancel" => format!("/offline/runs/{run}/mcp/tasks/{id}/cancel-receipt"),
        _ => return Err(failure("invalid_request")),
    };
    let n = native(&window).await?;
    local_result(n.request(reqwest::Method::GET, &path).query(&input)).await
}
