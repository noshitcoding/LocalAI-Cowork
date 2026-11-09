use super::native_client::{NativeClient, failure};
use cowork_contracts::{ApiError, SaveOwnerConnector, UiLanguage};
use serde_json::Value;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[tauri::command]
pub async fn connector_save(
    window: tauri::WebviewWindow,
    project: String,
    input: SaveOwnerConnector,
    language: UiLanguage,
) -> Result<Option<Value>, ApiError> {
    uuid::Uuid::parse_str(&project).map_err(|_| failure("invalid_request"))?;
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    if !input.recover_only {
        let destination = if let Some(raw) = &input.url {
            let u = reqwest::Url::parse(raw).map_err(|_| failure("invalid_request"))?;
            u.origin().ascii_serialization()
        } else {
            if matches!(language, UiLanguage::De) {
                "gespeichertes Ziel".into()
            } else {
                "stored destination".into()
            }
        };
        let de = matches!(language, UiLanguage::De);
        let (send, receive) = tokio::sync::oneshot::channel();
        window.dialog().message(if de{format!("Connector {} für {} speichern?\nDer mitgelieferte Python-Adapter wird nur für ausdrücklich ausgewählte MCP-Werkzeuge gestartet. Jede Nachricht benötigt weiterhin die konkrete Freigabe des Laufs. Die Zieladresse und der Schlüssel werden verschlüsselt gespeichert.",input.name,destination)}else{format!("Save connector {} for {}?\nThe bundled Python adapter starts only for explicitly selected MCP tools. Each message still requires its run's concrete approval. The destination and key are stored encrypted.",input.name,destination)})
            .title(if de{"Connector speichern"}else{"Save connector"}).parent(&window).kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancel).show(move|approved|{let _=send.send(approved);});
        if !receive
            .await
            .map_err(|_| failure("mcp_connection_failed"))?
        {
            return Ok(None);
        }
    }
    client
        .post(&format!("/native/connectors/{project}"), &input)
        .await
        .map(Some)
}
#[tauri::command]
pub async fn connector_action(
    window: tauri::WebviewWindow,
    project: String,
    id: String,
    operation: String,
    input: Value,
) -> Result<Value, ApiError> {
    uuid::Uuid::parse_str(&project).map_err(|_| failure("invalid_request"))?;
    uuid::Uuid::parse_str(&id).map_err(|_| failure("invalid_request"))?;
    if !matches!(operation.as_str(), "test" | "delete") {
        return Err(failure("invalid_request"));
    }
    let client = NativeClient::new(&window, "mcp_connection_failed", "uncertain_effect")?;
    client
        .post(
            &format!("/native/connectors/{project}/{id}/{operation}"),
            &input,
        )
        .await
}
