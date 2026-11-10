use super::native_client::{NativeClient, failure};
use cowork_contracts::*;
use std::future::Future;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct SecretsGate(tokio::sync::Mutex<()>);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Setup,
    Reconcile,
    Abandon,
}
trait Transport {
    fn status(&self) -> impl Future<Output = Result<NativeSecretsStatus, ApiError>> + Send;
    fn lookup(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<NativeSecretsRequestLookup, ApiError>> + Send;
    fn setup(
        &self,
        input: &NativeSecretsSetup,
    ) -> impl Future<Output = Result<NativeSecretsReceipt, ApiError>> + Send;
    fn resolve(
        &self,
        action: Action,
        input: &NativeSecretsResolution,
    ) -> impl Future<Output = Result<NativeSecretsReceipt, ApiError>> + Send;
}
impl Transport for NativeClient {
    async fn status(&self) -> Result<NativeSecretsStatus, ApiError> {
        self.result(self.request(reqwest::Method::GET, "/native/secrets"), false)
            .await
    }
    async fn lookup(&self, key: &str) -> Result<NativeSecretsRequestLookup, ApiError> {
        self.result(
            self.request(reqwest::Method::GET, "/native/secrets/requests")
                .query(&[("key", key)]),
            false,
        )
        .await
    }
    async fn setup(&self, input: &NativeSecretsSetup) -> Result<NativeSecretsReceipt, ApiError> {
        self.post("/native/secrets", input).await
    }
    async fn resolve(
        &self,
        action: Action,
        input: &NativeSecretsResolution,
    ) -> Result<NativeSecretsReceipt, ApiError> {
        let path = match action {
            Action::Reconcile => "/native/secrets/reconcile",
            Action::Abandon => "/native/secrets/abandon",
            Action::Setup => return Err(failure("invalid_request")),
        };
        self.post(path, input).await
    }
}
fn validate_status(status: &NativeSecretsStatus) -> Result<(), ApiError> {
    if status.revision < 0
        || (status.key_available && !status.configured)
        || status
            .pending_operation_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(failure("secrets_connection_failed"));
    }
    Ok(())
}
fn validate_receipt(receipt: &NativeSecretsReceipt) -> Result<(), ApiError> {
    validate_status(&receipt.store).map_err(|_| failure("secrets_setup_uncertain"))?;
    if uuid::Uuid::parse_str(&receipt.operation_id).is_err() {
        return Err(failure("secrets_setup_uncertain"));
    }
    Ok(())
}
async fn setup<T, C, F>(
    client: &T,
    input: &NativeSecretsSetup,
    confirm: C,
) -> Result<Option<NativeSecretsReceipt>, ApiError>
where
    T: Transport,
    C: FnOnce(NativeSecretsStatus) -> F,
    F: Future<Output = Result<bool, ApiError>>,
{
    cowork_domain::valid_text(&input.idempotency_key, 128)
        .map_err(|_| failure("invalid_request"))?;
    if !(0..=i64::MAX - 2).contains(&input.base_revision) {
        return Err(failure("invalid_request"));
    }
    // An existing request must still be checked against its original base revision
    // by the daemon. Its exact replay cannot create another key or reselect one.
    if client
        .lookup(&input.idempotency_key)
        .await?
        .receipt
        .is_none()
    {
        let status = client.status().await?;
        validate_status(&status)?;
        if status.revision != input.base_revision || status.pending_operation_id.is_some() {
            return Err(failure("revision_conflict"));
        }
        if !confirm(status).await? {
            return Ok(None);
        }
    }
    let receipt = client.setup(input).await?;
    validate_receipt(&receipt)?;
    Ok(Some(receipt))
}
async fn resolve<T, C, F>(
    client: &T,
    action: Action,
    input: &NativeSecretsResolution,
    confirm: C,
) -> Result<Option<NativeSecretsReceipt>, ApiError>
where
    T: Transport,
    C: FnOnce(NativeSecretsStatus) -> F,
    F: Future<Output = Result<bool, ApiError>>,
{
    if uuid::Uuid::parse_str(&input.operation_id).is_err()
        || input.base_revision <= 0
        || input.base_revision == i64::MAX
    {
        return Err(failure("invalid_request"));
    }
    let status = client.status().await?;
    validate_status(&status)?;
    if status.revision != input.base_revision
        || status.pending_operation_id.as_deref() != Some(&input.operation_id)
    {
        return Err(failure("revision_conflict"));
    }
    if !confirm(status).await? {
        return Ok(None);
    }
    let receipt = client.resolve(action, input).await?;
    validate_receipt(&receipt)?;
    let expected = if action == Action::Reconcile {
        NativeSecretsSetupState::Activated
    } else {
        NativeSecretsSetupState::Abandoned
    };
    if receipt.operation_id != input.operation_id || receipt.operation_state != expected {
        return Err(failure("secrets_setup_uncertain"));
    }
    Ok(Some(receipt))
}
async fn confirm(
    window: &tauri::WebviewWindow,
    language: UiLanguage,
    action: Action,
    status: NativeSecretsStatus,
) -> Result<bool, ApiError> {
    let messages: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/ui/src/secretMessages.json"
    ))
    .map_err(|_| failure("secrets_connection_failed"))?;
    let locale = if matches!(language, UiLanguage::De) {
        "de"
    } else {
        "en"
    };
    let text = |key: &str| {
        messages[locale][key]
            .as_str()
            .ok_or_else(|| failure("secrets_connection_failed"))
    };
    let (title, message, button) = match (action, status.configured) {
        (Action::Setup, false) => (
            "secretConfirmSetupTitle",
            "secretConfirmSetupBody",
            "secretConfirmSetupButton",
        ),
        (Action::Setup, true) => (
            "secretConfirmRotateTitle",
            "secretConfirmRotateBody",
            "secretRotate",
        ),
        (Action::Reconcile, _) => (
            "secretConfirmReconcileTitle",
            "secretConfirmReconcileBody",
            "secretConfirmReconcileButton",
        ),
        (Action::Abandon, _) => (
            "secretConfirmAbandonTitle",
            "secretConfirmAbandonBody",
            "secretConfirmAbandonButton",
        ),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .message(text(message)?)
        .title(text(title)?)
        .parent(window)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            text(button)?.into(),
            text("secretConfirmCancel")?.into(),
        ))
        .show(move |answer| {
            let _ = sender.send(answer);
        });
    receiver
        .await
        .map_err(|_| failure("secrets_connection_failed"))
}
fn client(window: &tauri::WebviewWindow) -> Result<NativeClient, ApiError> {
    NativeClient::new(
        window,
        "secrets_connection_failed",
        "secrets_setup_uncertain",
    )
}
#[tauri::command]
pub async fn native_secrets_status(
    window: tauri::WebviewWindow,
) -> Result<NativeSecretsStatus, ApiError> {
    let status = client(&window)?.status().await?;
    validate_status(&status)?;
    Ok(status)
}
#[tauri::command]
pub async fn native_secrets_request(
    window: tauri::WebviewWindow,
    key: String,
) -> Result<NativeSecretsRequestLookup, ApiError> {
    cowork_domain::valid_text(&key, 128).map_err(|_| failure("invalid_request"))?;
    let lookup = client(&window)?.lookup(&key).await?;
    if let Some(receipt) = &lookup.receipt {
        validate_receipt(receipt)?;
    }
    Ok(lookup)
}
#[tauri::command]
pub async fn native_secrets_setup(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, SecretsGate>,
    input: NativeSecretsSetup,
    language: UiLanguage,
) -> Result<Option<NativeSecretsReceipt>, ApiError> {
    let client = client(&window)?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    setup(&client, &input, |status| {
        confirm(&window, language, Action::Setup, status)
    })
    .await
}
#[tauri::command]
pub async fn native_secrets_reconcile(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, SecretsGate>,
    input: NativeSecretsResolution,
    language: UiLanguage,
) -> Result<Option<NativeSecretsReceipt>, ApiError> {
    let client = client(&window)?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    resolve(&client, Action::Reconcile, &input, |status| {
        confirm(&window, language, Action::Reconcile, status)
    })
    .await
}
#[tauri::command]
pub async fn native_secrets_abandon(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, SecretsGate>,
    input: NativeSecretsResolution,
    language: UiLanguage,
) -> Result<Option<NativeSecretsReceipt>, ApiError> {
    let client = client(&window)?;
    let _guard = gate.0.try_lock().map_err(|_| failure("secrets_busy"))?;
    resolve(&client, Action::Abandon, &input, |status| {
        confirm(&window, language, Action::Abandon, status)
    })
    .await
}

#[cfg(test)]
mod tests;
