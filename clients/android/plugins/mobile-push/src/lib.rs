#![cfg(mobile)]

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    Manager, Runtime,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FirebaseConfig<'a> {
    project_id: &'a str,
    application_id: &'a str,
    api_key: &'a str,
    sender_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResponse {
    pub granted: bool,
    pub requested: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushEvent {
    pub run_id: String,
    pub event_kind: String,
    pub sequence: i64,
    pub received_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushEventsResponse {
    pub events: Vec<PushEvent>,
}

pub struct MobilePush<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> MobilePush<R> {
    fn token(&self, config: FirebaseConfig<'_>) -> Result<TokenResponse> {
        self.0.run_mobile_plugin("token", config).map_err(Into::into)
    }

    fn permission_status(&self) -> Result<PermissionResponse> {
        self.0
            .run_mobile_plugin("permissionStatus", ())
            .map_err(Into::into)
    }

    fn request_permission(&self) -> Result<PermissionResponse> {
        self.0
            .run_mobile_plugin("requestPermission", ())
            .map_err(Into::into)
    }

    fn consume_events(&self) -> Result<PushEventsResponse> {
        self.0
            .run_mobile_plugin("consumeEvents", ())
            .map_err(Into::into)
    }
}

trait MobilePushExt<R: Runtime> {
    fn mobile_push(&self) -> &MobilePush<R>;
}

impl<R: Runtime, T: Manager<R>> MobilePushExt<R> for T {
    fn mobile_push(&self) -> &MobilePush<R> {
        self.state::<MobilePush<R>>().inner()
    }
}

#[tauri::command]
async fn token<R: Runtime>(
    app: tauri::AppHandle<R>,
    project_id: String,
    application_id: String,
    api_key: String,
    sender_id: String,
) -> Result<TokenResponse> {
    app.mobile_push().token(FirebaseConfig {
        project_id: &project_id,
        application_id: &application_id,
        api_key: &api_key,
        sender_id: &sender_id,
    })
}

#[tauri::command]
async fn permission_status<R: Runtime>(app: tauri::AppHandle<R>) -> Result<PermissionResponse> {
    app.mobile_push().permission_status()
}

#[tauri::command]
async fn request_permission<R: Runtime>(app: tauri::AppHandle<R>) -> Result<PermissionResponse> {
    app.mobile_push().request_permission()
}

#[tauri::command]
async fn consume_events<R: Runtime>(app: tauri::AppHandle<R>) -> Result<PushEventsResponse> {
    app.mobile_push().consume_events()
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("mobile-push")
        .invoke_handler(tauri::generate_handler![
            token,
            permission_status,
            request_permission,
            consume_events
        ])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let handle =
                api.register_android_plugin("dev.opencowork.mobile_push", "MobilePushPlugin")?;
            app.manage(MobilePush(handle));
            Ok(())
        })
        .build()
}
