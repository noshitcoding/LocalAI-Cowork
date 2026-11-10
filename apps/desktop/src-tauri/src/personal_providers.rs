//! Fresh personal model credentials use private native authority, never a key GET.
use super::native_client::{NativeClient, failure};
use cowork_contracts::{ApiError, ConfigurePersonalProvider, ProviderProfile};
use tauri::Manager;

#[tauri::command]
pub async fn configure_personal_provider(
    window: tauri::WebviewWindow,
    input: ConfigurePersonalProvider,
) -> Result<ProviderProfile, ApiError> {
    if !super::application_window(&window) {
        return Err(failure("forbidden"));
    }
    super::connection_info(
        window.app_handle().clone(),
        window.state::<tokio::sync::Mutex<()>>(),
    )
    .await
    .map_err(|_| failure("personal_provider_native_unavailable"))?;
    NativeClient::new(
        &window,
        "personal_provider_native_unavailable",
        "personal_provider_configuration_unconfirmed",
    )?
    .post("/native-providers/with-credential", &input)
    .await
}
