use super::native_client::{NativeClient, failure};
use cowork_contracts::{ApiError, AuditV1Verification, UiLanguage, VerifyAuditV1};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
#[tauri::command]
pub async fn native_audit_verify_v1(
    window: tauri::WebviewWindow,
    input: VerifyAuditV1,
    language: UiLanguage,
) -> Result<Option<AuditV1Verification>, ApiError> {
    let client = NativeClient::new(
        &window,
        "audit_connection_failed",
        "audit_verification_unconfirmed",
    )?;
    if input.request_id.is_nil()
        || input.archives.is_empty()
        || input.archives.len() > 4
        || input.archives.iter().any(|a| {
            a.request_id.is_nil()
                || a.content_sha256.len() != 64
                || a.content_sha256
                    .bytes()
                    .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
        })
    {
        return Err(failure("invalid_request"));
    }
    let german = matches!(language, UiLanguage::De);
    let hashes = input
        .archives
        .iter()
        .map(|a| a.content_sha256.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let message = if german {
        format!(
            "Die ausgewählten lokalen Originalarchive werden mit dem ursprünglichen V1-Schlüssel im Windows-Anmeldeinformationsspeicher geprüft. Der Schlüssel wird nur gelesen und weder exportiert noch ersetzt. Reihenfolge: ältestes Archiv zuerst. Eine Prüfung erteilt keine Ausführungsrechte.\n\nOriginal-SHA-256:\n{hashes}\n\nVorgang: {}",
            input.request_id
        )
    } else {
        format!(
            "Verify the selected local original archives with the original V1 key in Windows Credential Manager. The key is read only and is never exported or replaced. Order: oldest archive first. Verification grants no execution rights.\n\nOriginal SHA-256:\n{hashes}\n\nOperation: {}",
            input.request_id
        )
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .message(message)
        .title(if german {
            "V1-HMAC-Originalhistorie prüfen"
        } else {
            "Verify original V1 HMAC history"
        })
        .parent(&window)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            if german {
                "Prüfen".into()
            } else {
                "Verify".into()
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
        .map_err(|_| failure("audit_verification_unconfirmed"))?
    {
        return Ok(None);
    }
    client
        .post("/native/audit/v1/verify", &input)
        .await
        .map(Some)
}
