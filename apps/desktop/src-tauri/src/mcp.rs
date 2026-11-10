use super::native_client::{NativeClient, failure};
use cowork_contracts::{ApiError, UiLanguage};
use serde::Deserialize;
use serde_json::Value;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioInput {
    project_id: String,
    server_id: String,
    program: String,
    arguments: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    environment: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    expected_revision: i64,
}
#[tauri::command]
pub async fn configure_mcp_stdio(
    window: tauri::WebviewWindow,
    input: StdioInput,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    if input.program.is_empty()
        || input.program.len() > 128
        || input.arguments.len() > 128
        || input
            .arguments
            .iter()
            .any(|s| s.len() > 8192 || s.contains('\0'))
    {
        return Err(failure("invalid_request"));
    }
    if input
        .cwd
        .as_ref()
        .is_some_and(|value| value.len() > 4096 || value.chars().any(char::is_control))
    {
        return Err(failure("invalid_request"));
    }
    let environment_names = input
        .environment
        .as_ref()
        .map(|values| values.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| {
            if matches!(language, UiLanguage::De) {
                "gespeicherte Werte beibehalten".into()
            } else {
                "retain stored values".into()
            }
        });
    let cwd = input.cwd.as_deref().unwrap_or(".").to_owned();
    let body = serde_json::json!({"project_id":input.project_id,"server_id":input.server_id,"program":input.program,"arguments":input.arguments,"cwd":input.cwd,"environment":input.environment,"expected_revision":input.expected_revision});
    let (send, receive) = tokio::sync::oneshot::channel();
    let german = matches!(language, UiLanguage::De);
    let message = if german {
        format!(
            "MCP-Prozess starten: {}\nArgumente: {}\nArbeitsverzeichnis: {}\nUmgebungsvariablen: {}\nProjekt: {}\nDer Prozess arbeitet mit Ihren lokalen Betriebssystemrechten. Werkzeugaufrufe benötigen weiterhin die konkrete Freigabe des jeweiligen Laufs.",
            body["program"].as_str().unwrap_or(""),
            body["arguments"],
            cwd,
            environment_names,
            body["project_id"]
        )
    } else {
        format!(
            "Start MCP process: {}\nArguments: {}\nWorking directory: {}\nEnvironment variables: {}\nProject: {}\nThe process uses your local operating system permissions. Tool calls still require the concrete approval of their run.",
            body["program"].as_str().unwrap_or(""),
            body["arguments"],
            cwd,
            environment_names,
            body["project_id"]
        )
    };
    window
        .dialog()
        .message(message)
        .title(if german {
            "Persönlichen MCP-Server verbinden"
        } else {
            "Connect personal MCP server"
        })
        .parent(&window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            if german {
                "Verbinden".into()
            } else {
                "Connect".into()
            },
            if german {
                "Abbrechen".into()
            } else {
                "Cancel".into()
            },
        ))
        .show(move |approved| {
            let _ = send.send(approved);
        });
    if !receive
        .await
        .map_err(|_| failure("mcp_connection_failed"))?
    {
        return Ok(None);
    }
    client
        .result(
            client
                .request(reqwest::Method::POST, "/native/mcp/stdio")
                .timeout(std::time::Duration::from_secs(90))
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&body).map_err(|_| failure("invalid_request"))?),
            true,
        )
        .await
        .map(Some)
}
#[tauri::command]
pub async fn save_mcp_credential(
    window: tauri::WebviewWindow,
    input: Value,
) -> Result<Value, ApiError> {
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    // Only the fixed daemon route accepts and encrypts this endpoint-bound secret.
    client.post("/native/mcp/credential", &input).await
}
#[tauri::command]
pub async fn mcp_credential_status(
    window: tauri::WebviewWindow,
    project: String,
    server: String,
) -> Result<Value, ApiError> {
    uuid::Uuid::parse_str(&project).map_err(|_| failure("invalid_request"))?;
    uuid::Uuid::parse_str(&server).map_err(|_| failure("invalid_request"))?;
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    client
        .result(
            client.request(
                reqwest::Method::GET,
                &format!("/native/mcp/credential/{project}/{server}"),
            ),
            false,
        )
        .await
}
#[tauri::command]
pub async fn mcp_stdio_factories(
    window: tauri::WebviewWindow,
    project: String,
) -> Result<Value, ApiError> {
    uuid::Uuid::parse_str(&project).map_err(|_| failure("invalid_request"))?;
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    client
        .result(
            client.request(
                reqwest::Method::GET,
                &format!("/native/mcp/stdio/{project}"),
            ),
            false,
        )
        .await
}
#[tauri::command]
pub async fn disconnect_mcp_stdio(
    window: tauri::WebviewWindow,
    project: String,
    server: String,
    expected_revision: i64,
) -> Result<Value, ApiError> {
    uuid::Uuid::parse_str(&project).map_err(|_| failure("invalid_request"))?;
    uuid::Uuid::parse_str(&server).map_err(|_| failure("invalid_request"))?;
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    client
        .post(
            &format!("/native/mcp/stdio/{project}/{server}/disconnect"),
            &serde_json::json!({"expected_revision":expected_revision}),
        )
        .await
}
#[tauri::command]
pub async fn mcp_oauth_native(
    window: tauri::WebviewWindow,
    operation: String,
    target: String,
    input: Option<Value>,
) -> Result<Value, ApiError> {
    uuid::Uuid::parse_str(&target).map_err(|_| failure("invalid_request"))?;
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    let (path, mutation) = match operation.as_str() {
        "profiles" => (format!("/native/mcp/oauth/{target}"), false),
        "discover" => (format!("/native/mcp/oauth/{target}/discovery"), true),
        "configure" => (format!("/native/mcp/oauth/{target}"), true),
        "start" => (format!("/native/mcp/oauth/profile/{target}/start"), true),
        "flow" => (format!("/native/mcp/oauth/flow/{target}"), false),
        "complete" => (format!("/native/mcp/oauth/flow/{target}/complete"), true),
        "interactions" => (format!("/native/mcp/tasks/{target}/interactions"), false),
        "answer_interaction" => {
            let id = input
                .as_ref()
                .and_then(|v| v["interaction_id"].as_str())
                .ok_or_else(|| failure("invalid_request"))?;
            uuid::Uuid::parse_str(id).map_err(|_| failure("invalid_request"))?;
            (
                format!("/native/mcp/tasks/{target}/interactions/{id}"),
                true,
            )
        }
        _ => return Err(failure("invalid_request")),
    };
    let input = if operation == "answer_interaction" {
        Some(
            input
                .as_ref()
                .and_then(|v| v.get("response"))
                .cloned()
                .ok_or_else(|| failure("invalid_request"))?,
        )
    } else {
        input
    };
    let result: Value = if mutation {
        let request = client
            .request(reqwest::Method::POST, &path)
            .timeout(std::time::Duration::from_secs(90))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&input.unwrap_or(serde_json::json!({})))
                    .map_err(|_| failure("invalid_request"))?,
            );
        client.result(request, true).await?
    } else {
        client
            .result(client.request(reqwest::Method::GET, &path), false)
            .await?
    };
    if operation == "start" && result["state"] == "awaiting_callback" {
        // Browser target comes from the captured native flow, never a renderer
        // endpoint. Repeated handoff uses the same original state/PKCE flow.
        let url = result["authorization_url"]
            .as_str()
            .ok_or_else(|| failure("invalid_request"))?;
        let parsed = reqwest::Url::parse(url).map_err(|_| failure("invalid_request"))?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || url.len() > 16384
            || url.chars().any(char::is_control)
        {
            return Err(failure("forbidden"));
        }
        open_oauth_browser(url)?;
    }
    Ok(result)
}
fn open_oauth_browser(url: &str) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let target = std::ffi::OsStr::new(url)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let verb = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize <= 32 {
            return Err(failure("mcp_connection_failed"));
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|_| failure("mcp_connection_failed"))?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = url;
        Err(failure("forbidden"))
    }
}
