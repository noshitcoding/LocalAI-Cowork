//! Operator-configured WebAuthn and optional FCM. No owner token is a device identity.
#[cfg(windows)]
use crate::private_secret_file;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cowork_domain::{Error, Result};
use cowork_secrets::{CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer};
use cowork_storage::{AndroidPushAttempt, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use webauthn_rs::prelude::{Url, WebauthnBuilder};
use zeroize::{Zeroize, Zeroizing};

#[path = "device_operator_configuration.rs"]
pub mod operator;
#[path = "android_private_browser.rs"]
mod private_browser;
#[path = "android_private_github.rs"]
mod private_github;
#[path = "android_private_mcp_worker.rs"]
mod private_mcp_worker;
#[path = "android_private_run_recovery.rs"]
mod private_run_recovery;
#[path = "android_private_runner.rs"]
mod private_runner;
#[path = "android_private_schedule_worker.rs"]
mod private_schedule_worker;
#[path = "android_private_server_worker.rs"]
mod private_server_worker;
#[path = "android_private_stdio.rs"]
mod private_stdio;
#[path = "android_private_terminal.rs"]
mod private_terminal;
#[path = "android_private_workflow_worker.rs"]
mod private_workflow_worker;
pub use private_workflow_worker::PrivateWorkflowServices;
pub(super) struct DeviceConfiguration {
    pub revision: i64,
    pub webauthn: Option<Arc<webauthn_rs::Webauthn>>,
    pub origin: Option<String>,
    pub rp_id: Option<String>,
    pub push: Option<Arc<FcmSender>>,
    pub fleet: Option<Arc<FleetAuthority>>,
}
pub struct AndroidDeviceServices {
    config: std::sync::RwLock<Arc<DeviceConfiguration>>,
    settings: Mutex<operator::Settings>,
    store: Store,
    keys: Arc<dyn KeyStore>,
    key_id: KeyId,
    pub(crate) hub: Arc<crate::android_device_channel::DeviceHub>,
    private_runner: Option<Arc<private_runner::Runner>>,
    private_workflows: Option<Arc<PrivateWorkflowServices>>,
    private_browser_receipts: private_browser::Receipts,
    private_terminal: private_terminal::Runtime,
    private_github_receipts: private_github::Receipts,
    recovery_observer: String,
    codex: Arc<cowork_runtime::codex_accounts::CodexBroker>,
}

fn invalid() -> Error {
    Error::Validation(
        "Android device operator configuration is invalid or the secret file is not private".into(),
    )
}
fn private_account(data: &Path, web: Option<&Path>, path: &Path) -> Result<Zeroizing<String>> {
    let data = std::path::absolute(data).map_err(|_| invalid())?;
    if data
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    if !path.is_absolute() {
        return Err(invalid());
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    let absolute = path.to_path_buf();
    if !absolute.starts_with(&data)
        || absolute.starts_with(data.join("toolbox"))
        || web.is_some_and(|w| absolute.starts_with(w))
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    let mut held = Vec::new();
    for parent in absolute.ancestors().skip(1) {
        let info = std::fs::symlink_metadata(parent).map_err(|_| invalid())?;
        if !info.is_dir() || info.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            if info.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
            let directory = OpenOptions::new()
                .access_mode(0x20081)
                .share_mode(3)
                .custom_flags(0x02200000)
                .open(parent)
                .map_err(|_| invalid())?;
            if parent.starts_with(&data) {
                private_secret_file::validate_directory(&directory).map_err(|_| invalid())?;
            }
            held.push(directory);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // The caller supplies a private data namespace. No foreign-writable parent may substitute it.
            let uid = unsafe { libc::geteuid() };
            if (info.uid() != 0 && info.uid() != uid)
                || (info.mode() & 0o022 != 0 && info.mode() & 0o1000 == 0)
                || parent.starts_with(&data) && (info.uid() != uid || info.mode() & 0o022 != 0)
            {
                return Err(invalid());
            }
        }
    }
    let info = std::fs::symlink_metadata(&absolute).map_err(|_| invalid())?;
    if !info.is_file() || info.file_type().is_symlink() || info.len() > 65536 {
        return Err(invalid());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        if info.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(&absolute).map_err(|_| invalid())?;
    let held_info = file.metadata().map_err(|_| invalid())?;
    #[cfg(windows)]
    private_secret_file::validate(&file).map_err(|_| invalid())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if held_info.uid() != unsafe { libc::geteuid() }
            || held_info.mode() & 0o777 != 0o600
            || held_info.nlink() != 1
        {
            return Err(invalid());
        }
    }
    if held_info.len() > 65536 {
        return Err(invalid());
    }
    let mut raw = Zeroizing::new(String::new());
    file.take(65537)
        .read_to_string(&mut raw)
        .map_err(|_| invalid())?;
    if raw.len() > 65536 {
        return Err(invalid());
    }
    Ok(raw)
}
#[derive(Deserialize)]
struct Account {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}
impl Drop for Account {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}
pub(crate) struct FcmSender {
    project: String,
    email: String,
    key: ring::signature::RsaKeyPair,
    http: reqwest::Client,
    token: Mutex<Option<(Instant, Zeroizing<String>)>>,
}
pub(crate) struct FleetAuthority {
    pub(crate) key: Zeroizing<String>,
    pub(crate) team: uuid::Uuid,
    pub(crate) instance: uuid::Uuid,
}
impl AndroidDeviceServices {
    pub(super) async fn private_workflow_available(&self, kind: &str) -> Result<bool> {
        let Some(services) = &self.private_workflows else {
            return Ok(false);
        };
        let configuration = match services.configuration(kind) {
            Ok(value) => value,
            Err(Error::Forbidden | Error::NotFound) => return Ok(false),
            Err(error) => return Err(error),
        };
        match self
            .store
            .android_workflow_engine_inspect(configuration)
            .await
        {
            Ok(value) => Ok(value["active"] == true && value["configured_matches"] == true),
            Err(Error::Forbidden | Error::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }
    /// Read the same OS-private runner configuration used by normal startup.
    /// This does not register, activate, or launch the runner.
    pub fn private_runner_operator_binding(
        data: &Path,
        web: Option<&Path>,
        store: Store,
        keys: Arc<dyn KeyStore>,
    ) -> Result<Option<Value>> {
        Ok(
            private_runner::Runner::from_environment(data, web, store, keys)?
                .map(|runner| runner.binding()),
        )
    }
    pub async fn shutdown(&self) {
        self.private_terminal.shutdown().await;
        self.hub.shutdown().await;
        self.private_browser_receipts.drain().await;
        self.private_github_receipts.drain().await;
    }
    pub(super) fn current(&self) -> Arc<DeviceConfiguration> {
        self.config
            .read()
            .expect("device configuration lock")
            .clone()
    }
    pub fn configuration(&self) -> Value {
        let c = self.current();
        json!({"schema_version":2,"passkeys_enabled":c.webauthn.is_some(),"rp_id":c.rp_id,"origin":c.origin,"fcm_enabled":c.push.is_some(),"fleet_enabled":c.fleet.is_some(),"push_delivery":if c.push.is_some(){"configured"}else{"operator_credentials_required"},"minimum_compatible_schema_version":2,"agent_protocol_version":2})
    }
    pub async fn run(self: Arc<Self>, stop: CancellationToken) {
        let private_worker = tokio::spawn(self.clone().run_private_server(stop.clone()));
        let mut timer = tokio::time::interval(Duration::from_secs(3));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=stop.cancelled()=>break,_=timer.tick()=>{
                let _=self.store.android_push_queue_terminal_runs().await;
                let _=self.store.android_device_expire_leases().await;
                if let Some(sender)=self.current().push.clone() {for _ in 0..20 {
                    let Ok(Some(attempt))=self.store.android_push_claim().await else{break};
                    let _=self.store.android_push_deliver(&attempt,||async {match self.open_push(&attempt){Ok(token)=>sender.send(&token,&attempt.payload).await,Err(_)=>(false,true,"invalid_private_subscription")}}).await;
                }}
            }}
        }
        self.hub.shutdown().await;
        let _ = private_worker.await;
    }
    fn open_push(&self, v: &AndroidPushAttempt) -> Result<Zeroizing<String>> {
        let sealed: SealedCredential =
            serde_json::from_str(&v.sealed_token).map_err(|_| invalid())?;
        let encoded = open_bearer(
            self.keys.as_ref(),
            &push_scope(v.identity_id, v.device_id, v.subscription_id, v.revision),
            &sealed,
        )
        .map_err(|_| invalid())?;
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded.as_bytes())
                .map_err(|_| invalid())?,
        );
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        if text.len() > 2048
            || text.is_empty()
            || text.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(invalid());
        }
        Ok(Zeroizing::new(text.to_owned()))
    }
}
pub(crate) fn push_scope(
    identity: uuid::Uuid,
    device: uuid::Uuid,
    id: uuid::Uuid,
    revision: i64,
) -> CredentialScope {
    CredentialScope {
        project_id: identity.to_string(),
        server_id: device.to_string(),
        reference: format!("android-push:{id}"),
        revision,
        endpoint: "fcm://subscription".into(),
    }
}
async fn bounded(response: reqwest::Response) -> Result<Value> {
    let mut response = response;
    let mut raw = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
        if raw.len() + chunk.len() > 65536 {
            return Err(invalid());
        }
        raw.extend_from_slice(&chunk)
    }
    serde_json::from_slice(&raw).map_err(|_| invalid())
}
impl FcmSender {
    async fn access_token(&self) -> Result<Zeroizing<String>> {
        let mut cached = self.token.lock().await;
        if let Some((expiry, value)) = &*cached
            && *expiry > Instant::now() + Duration::from_secs(60)
        {
            return Ok(Zeroizing::new(value.to_string()));
        }
        let issued = chrono::Utc::now().timestamp();
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\",\"typ\":\"JWT\"}");
        let claims=URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"iss":self.email,"scope":"https://www.googleapis.com/auth/firebase.messaging","aud":"https://oauth2.googleapis.com/token","iat":issued,"exp":issued+3600})).map_err(|_|invalid())?);
        let signing = format!("{header}.{claims}");
        let mut signature = vec![0u8; self.key.public().modulus_len()];
        self.key
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                signing.as_bytes(),
                &mut signature,
            )
            .map_err(|_| invalid())?;
        let assertion = Zeroizing::new(format!("{signing}.{}", URL_SAFE_NO_PAD.encode(signature)));
        let response = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|_| invalid())?;
        if !response.status().is_success() {
            return Err(invalid());
        }
        let mut value = bounded(response).await?;
        let token = value["access_token"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or_else(invalid)?
            .to_owned();
        if let Some(Value::String(s)) = value.get_mut("access_token") {
            s.zeroize()
        }
        let seconds = value["expires_in"]
            .as_u64()
            .filter(|n| (60..=3600).contains(n))
            .ok_or_else(invalid)?;
        *cached = Some((
            Instant::now() + Duration::from_secs(seconds),
            Zeroizing::new(token.clone()),
        ));
        Ok(Zeroizing::new(token))
    }
    pub(crate) async fn send(
        &self,
        registration: &str,
        payload: &Value,
    ) -> (bool, bool, &'static str) {
        let Ok(token) = self.access_token().await else {
            return (false, true, "provider_auth_unavailable");
        };
        let body = json!({"message":{"token":registration,"data":payload,"notification":{"title":"LocalAI Cowork","body":"Der Status eines Auftrags hat sich geändert."},"android":{"priority":"normal","ttl":"86400s"}}});
        match self
            .http
            .post(format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                self.project
            ))
            .bearer_auth(token.as_str())
            .json(&body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => match bounded(response).await {
                Ok(value) if value["name"].as_str().is_some() => (true, false, "provider_accepted"),
                _ => (false, false, "provider_response_unknown"),
            },
            Ok(response) => {
                let status = response.status().as_u16();
                if matches!(status, 400 | 401 | 403 | 404) {
                    (false, true, "provider_rejected")
                } else {
                    (false, false, "provider_response_unknown")
                }
            }
            Err(_) => (false, false, "provider_delivery_unknown"),
        }
    }
}
