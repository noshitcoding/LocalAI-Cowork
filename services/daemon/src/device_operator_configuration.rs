//! Live, encrypted installation settings. Public status never returns credentials.
use super::*;
use cowork_secrets::seal_bearer;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub revision: i64,
    pub origin: Option<String>,
    pub rp_id: Option<String>,
    pub fcm_account: Option<String>,
    pub fleet_key: Option<String>,
    pub organisation_id: Option<uuid::Uuid>,
    pub instance_id: Option<uuid::Uuid>,
}
impl Drop for Settings {
    fn drop(&mut self) {
        if let Some(v) = &mut self.fcm_account {
            v.zeroize();
        }
        if let Some(v) = &mut self.fleet_key {
            v.zeroize();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
    pub expected_revision: i64,
    pub origin: Option<String>,
    pub rp_id: Option<String>,
    #[serde(default)]
    pub fcm_account: Option<String>,
    #[serde(default)]
    pub clear_fcm: bool,
    #[serde(default)]
    pub fleet_key: Option<String>,
    #[serde(default)]
    pub clear_fleet: bool,
    pub organisation_id: Option<uuid::Uuid>,
    pub instance_id: Option<uuid::Uuid>,
}
impl Drop for Configure {
    fn drop(&mut self) {
        if let Some(v) = &mut self.fcm_account {
            v.zeroize();
        }
        if let Some(v) = &mut self.fleet_key {
            v.zeroize();
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Encrypted {
    digest: String,
    parts: Vec<SealedCredential>,
}
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn scope(key: KeyId, revision: i64, digest: &str, count: usize, index: usize) -> CredentialScope {
    CredentialScope {
        project_id: key.instance.to_string(),
        server_id: "operator-devices".into(),
        reference: format!("device-settings:{digest}:{count}:{index}"),
        revision,
        endpoint: "installation://devices/v1".into(),
    }
}
fn seal(keys: &dyn KeyStore, key: KeyId, settings: &Settings) -> Result<String> {
    let bytes = Zeroizing::new(serde_json::to_vec(settings).map_err(|_| invalid())?);
    if bytes.len() > 65536 {
        return Err(invalid());
    }
    let hash = digest(&bytes);
    let count = bytes.len().div_ceil(3000);
    let parts = bytes
        .chunks(3000)
        .enumerate()
        .map(|(i, part)| {
            let value = Zeroizing::new(URL_SAFE_NO_PAD.encode(part));
            seal_bearer(
                keys,
                key,
                &scope(key, settings.revision, &hash, count, i),
                &value,
            )
            .map_err(|_| Error::NativeKeyUnavailable)
        })
        .collect::<Result<Vec<_>>>()?;
    serde_json::to_string(&Encrypted {
        digest: hash,
        parts,
    })
    .map_err(|_| invalid())
}
fn open(keys: &dyn KeyStore, key: KeyId, revision: i64, payload: &str) -> Result<Settings> {
    let encrypted: Encrypted = serde_json::from_str(payload).map_err(|_| invalid())?;
    if encrypted.parts.is_empty() || encrypted.parts.len() > 22 || encrypted.digest.len() != 64 {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    for (i, part) in encrypted.parts.iter().enumerate() {
        let value = open_bearer(
            keys,
            &scope(key, revision, &encrypted.digest, encrypted.parts.len(), i),
            part,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(value.as_bytes())
                .map_err(|_| invalid())?,
        );
        bytes.extend_from_slice(&decoded);
    }
    if bytes.len() > 65536 || digest(&bytes) != encrypted.digest {
        return Err(invalid());
    }
    let settings: Settings = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if settings.revision != revision {
        return Err(Error::Conflict);
    }
    Ok(settings)
}
fn optional(variable: &str) -> Option<String> {
    std::env::var(variable).ok().filter(|v| !v.is_empty())
}

pub(super) fn build(settings: &Settings) -> Result<Arc<DeviceConfiguration>> {
    let origin = settings.origin.as_deref().filter(|v| !v.is_empty());
    let rp = settings.rp_id.as_deref().filter(|v| !v.is_empty());
    let webauthn = match (origin, rp) {
        (None, None) => None,
        (Some(origin), Some(rp)) => {
            let url = Url::parse(origin).map_err(|_| invalid())?;
            if url.host_str() != Some(rp)
                || url.origin().ascii_serialization() != origin
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
                || !(url.scheme() == "https" || url.scheme() == "http" && rp == "localhost")
            {
                return Err(Error::Validation("Passkeys require an exact HTTPS origin and matching RP hostname (HTTP only for localhost)".into()));
            }
            Some(Arc::new(
                WebauthnBuilder::new(rp, &url)
                    .map_err(|_| invalid())?
                    .rp_name("LocalAI Cowork")
                    .allow_subdomains(false)
                    .build()
                    .map_err(|_| invalid())?,
            ))
        }
        _ => {
            return Err(Error::Validation(
                "Passkey origin and RP ID must be configured together".into(),
            ));
        }
    };
    let push = if let Some(raw) = settings.fcm_account.as_ref() {
        if raw.len() > 32768 {
            return Err(invalid());
        }
        let account: Account = serde_json::from_str(raw)
            .map_err(|_| Error::Validation("The FCM service account JSON is invalid".into()))?;
        if account.token_uri != "https://oauth2.googleapis.com/token"
            || account.project_id.is_empty()
            || account.project_id.len() > 100
            || !account
                .project_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || account.client_email.len() > 254
            || !account.client_email.ends_with(".iam.gserviceaccount.com")
        {
            return Err(invalid());
        }
        let pem = account
            .private_key
            .strip_prefix("-----BEGIN PRIVATE KEY-----")
            .and_then(|v| v.trim_end().strip_suffix("-----END PRIVATE KEY-----"))
            .ok_or_else(invalid)?;
        let encoded = Zeroizing::new(
            pem.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>(),
        );
        let bytes = Zeroizing::new(STANDARD.decode(encoded.as_bytes()).map_err(|_| invalid())?);
        let key = ring::signature::RsaKeyPair::from_pkcs8(&bytes).map_err(|_| invalid())?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| invalid())?;
        Some(Arc::new(FcmSender {
            project: account.project_id.clone(),
            email: account.client_email.clone(),
            key,
            http,
            token: Mutex::new(None),
        }))
    } else {
        None
    };
    let fleet = if let Some(key) = settings.fleet_key.as_ref() {
        if key.len() < 32 || key.len() > 4096 || key.chars().any(char::is_control) {
            return Err(invalid());
        }
        let team = settings
            .organisation_id
            .filter(|v| !v.is_nil())
            .ok_or_else(invalid)?;
        let instance = settings
            .instance_id
            .filter(|v| !v.is_nil())
            .ok_or_else(invalid)?;
        Some(Arc::new(FleetAuthority {
            key: Zeroizing::new(key.clone()),
            team,
            instance,
        }))
    } else {
        None
    };
    Ok(Arc::new(DeviceConfiguration {
        revision: settings.revision,
        origin: origin.map(str::to_owned),
        rp_id: rp.map(str::to_owned),
        webauthn,
        push,
        fleet,
    }))
}
impl AndroidDeviceServices {
    pub async fn from_environment(
        store: Store,
        keys: Arc<dyn KeyStore>,
        key_id: KeyId,
        data: &Path,
        web: Option<&Path>,
    ) -> Result<Arc<Self>> {
        Self::from_environment_with_workflows(store, keys, key_id, data, web, None, Arc::new(()))
            .await
    }
    pub async fn from_environment_with_workflows(
        store: Store,
        keys: Arc<dyn KeyStore>,
        key_id: KeyId,
        data: &Path,
        web: Option<&Path>,
        crew_configuration: Option<Value>,
        hold: Arc<dyn Send + Sync>,
    ) -> Result<Arc<Self>> {
        let settings = if let Some((revision, payload)) =
            store.operator_device_configuration().await?
        {
            open(keys.as_ref(), key_id, revision, &payload)?
        } else {
            let fcm_account = std::env::var_os("COWORK_FCM_SERVICE_ACCOUNT_FILE")
                .filter(|v| !v.is_empty())
                .map(|v| private_account(data, web, &PathBuf::from(v)).map(|s| s.to_string()))
                .transpose()?;
            let fleet_key = std::env::var_os("COWORK_FLEET_KEY_FILE")
                .filter(|v| !v.is_empty())
                .map(|v| private_account(data, web, &PathBuf::from(v)).map(|s| s.trim().to_owned()))
                .transpose()?;
            Settings {
                revision: 0,
                origin: optional("COWORK_PASSKEY_ORIGIN"),
                rp_id: optional("COWORK_PASSKEY_RP_ID"),
                fcm_account,
                fleet_key,
                organisation_id: optional("COWORK_ORGANISATION_ID")
                    .map(|s| s.parse().map_err(|_| invalid()))
                    .transpose()?,
                instance_id: optional("COWORK_INSTANCE_ID")
                    .map(|s| s.parse().map_err(|_| invalid()))
                    .transpose()?,
            }
        };
        let config = build(&settings)?;
        let hub = crate::android_device_channel::DeviceHub::configured(store.clone(), web).await?;
        let private_runner =
            private_runner::Runner::from_environment(data, web, store.clone(), keys.clone())?;
        let private_workflows = Some(
            PrivateWorkflowServices::from_startup(
                store.clone(),
                key_id.instance,
                private_runner.as_ref().map(|runner| runner.binding()),
                crew_configuration,
                hold,
            )
            .await?,
        );
        Ok(Arc::new(Self {
            config: std::sync::RwLock::new(config),
            settings: Mutex::new(settings),
            store,
            keys,
            key_id,
            hub,
            private_runner,
            private_workflows,
            private_browser_receipts: private_browser::Receipts::default(),
            private_terminal: private_terminal::Runtime::default(),
            private_github_receipts: private_github::Receipts::default(),
            recovery_observer: uuid::Uuid::new_v4().to_string(),
            codex: Arc::new(cowork_runtime::codex_accounts::CodexBroker::new(
                data.to_path_buf(),
            )),
        }))
    }
    pub async fn configure(&self, input: Configure) -> Result<Value> {
        let mut previous = self.settings.lock().await;
        if input.expected_revision != previous.revision {
            return Err(Error::Conflict);
        }
        let revision = previous.revision.checked_add(1).ok_or(Error::Conflict)?;
        let mut next = Settings {
            revision,
            origin: input.origin.clone().filter(|s| !s.is_empty()),
            rp_id: input.rp_id.clone().filter(|s| !s.is_empty()),
            fcm_account: previous.fcm_account.clone(),
            fleet_key: previous.fleet_key.clone(),
            organisation_id: input.organisation_id,
            instance_id: input.instance_id,
        };
        if input.clear_fcm {
            next.fcm_account = None;
        } else if let Some(v) = input.fcm_account.as_ref().filter(|s| !s.is_empty()) {
            next.fcm_account = Some(v.clone());
        }
        if input.clear_fleet {
            next.fleet_key = None;
            next.organisation_id = None;
            next.instance_id = None;
        } else if let Some(v) = input.fleet_key.as_ref().filter(|s| !s.is_empty()) {
            next.fleet_key = Some(v.clone());
        }
        let config = build(&next)?;
        let payload = seal(self.keys.as_ref(), self.key_id, &next)?;
        self.store
            .save_operator_device_configuration(input.expected_revision, &payload)
            .await?;
        *self.config.write().map_err(|_| Error::Conflict)? = config;
        *previous = next;
        Ok(self.operator_configuration())
    }
    pub fn operator_configuration(&self) -> Value {
        let c = self.current();
        let mut result = self.configuration();
        result["revision"] = json!(c.revision);
        result["organisation_id"] = json!(c.fleet.as_ref().map(|v| v.team));
        result["instance_id"] = json!(c.fleet.as_ref().map(|v| v.instance));
        result["fcm_project"] = json!(c.push.as_ref().map(|v| &v.project));
        result["configuration_source"] = json!(if c.revision > 0 {
            "encrypted_operator_settings"
        } else {
            "environment"
        });
        result
    }
    pub async fn check_configuration(&self) -> Result<Value> {
        let c = self.current();
        let fcm = if let Some(sender) = &c.push {
            match sender.access_token().await {
                Ok(_) => "oauth_authenticated",
                Err(_) => "oauth_authentication_failed",
            }
        } else {
            "not_configured"
        };
        Ok(
            json!({"schema_version":2,"revision":c.revision,"passkeys":if c.webauthn.is_some(){"ready_for_device_registration"}else{"not_configured"},"fcm":fcm,"fleet":if c.fleet.is_some(){"ticket_verifier_ready"}else{"not_configured"},"agent_protocol_version":2,"remote_desktop":"requires_a_connected_executor_with_remote_control_capability"}),
        )
    }
}
