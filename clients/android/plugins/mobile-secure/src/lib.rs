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
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretLocator<'a> {
    namespace: &'a str,
    key: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreRequest<'a> {
    namespace: &'a str,
    key: &'a str,
    value: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrieveResponse {
    pub value: Option<String>,
}

pub struct MobileSecure<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> MobileSecure<R> {
    pub fn store(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        self.0
            .run_mobile_plugin(
                "store",
                StoreRequest {
                    namespace,
                    key,
                    value,
                },
            )
            .map_err(Into::into)
    }

    pub fn retrieve(&self, namespace: &str, key: &str) -> Result<RetrieveResponse> {
        self.0
            .run_mobile_plugin("retrieve", SecretLocator { namespace, key })
            .map_err(Into::into)
    }

    pub fn remove(&self, namespace: &str, key: &str) -> Result<()> {
        self.0
            .run_mobile_plugin("remove", SecretLocator { namespace, key })
            .map_err(Into::into)
    }
}

pub trait MobileSecureExt<R: Runtime> {
    fn mobile_secure(&self) -> &MobileSecure<R>;
}

impl<R: Runtime, T: Manager<R>> MobileSecureExt<R> for T {
    fn mobile_secure(&self) -> &MobileSecure<R> {
        self.state::<MobileSecure<R>>().inner()
    }
}

#[tauri::command]
async fn store<R: Runtime>(
    app: tauri::AppHandle<R>,
    namespace: String,
    key: String,
    value: String,
) -> Result<()> {
    app.mobile_secure().store(&namespace, &key, &value)
}

#[tauri::command]
async fn retrieve<R: Runtime>(
    app: tauri::AppHandle<R>,
    namespace: String,
    key: String,
) -> Result<RetrieveResponse> {
    app.mobile_secure().retrieve(&namespace, &key)
}

#[tauri::command]
async fn remove<R: Runtime>(
    app: tauri::AppHandle<R>,
    namespace: String,
    key: String,
) -> Result<()> {
    app.mobile_secure().remove(&namespace, &key)
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("mobile-secure")
        .invoke_handler(tauri::generate_handler![store, retrieve, remove])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let handle =
                api.register_android_plugin("dev.opencowork.mobile_secure", "MobileSecurePlugin")?;
            app.manage(MobileSecure(handle));
            Ok(())
        })
        .build()
}
