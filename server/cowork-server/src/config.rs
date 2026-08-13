use std::{env, fs, net::SocketAddr, time::Duration};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cowork_contracts::Capability;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessMode {
    Api,
    Worker,
    All,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mode: ProcessMode,
    pub listen_addr: SocketAddr,
    pub database_url: String,
    pub bootstrap_token: String,
    pub bootstrap_user_id: Uuid,
    pub database_max_connections: u32,
    pub worker_id: Uuid,
    pub worker_poll_interval: Duration,
    pub maintenance_every_polls: u64,
    pub lease_duration: Duration,
    pub model_base_url: Option<String>,
    pub model_api_key: Option<String>,
    pub model_name: String,
    pub model_input_cost_micros_per_million: Option<u64>,
    pub model_output_cost_micros_per_million: Option<u64>,
    pub server_capabilities: Vec<Capability>,
    pub runner_url: Option<String>,
    pub runner_signing_key: Option<String>,
    pub object_store: Option<ObjectStoreConfig>,
    pub push: PushConfig,
    pub passkeys: Option<PasskeyConfig>,
    pub oidc: Option<OidcConfig>,
}

#[derive(Debug, Clone)]
pub struct ObjectStoreConfig {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub addressing_style: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    pub master_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct PushConfig {
    pub fcm: Option<FcmConfig>,
    pub web_push: Option<WebPushConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FcmConfig {
    pub project_id: String,
    pub client_email: String,
    pub private_key: String,
    pub token_uri: String,
}

#[derive(Debug, Clone)]
pub struct WebPushConfig {
    pub vapid_private_key: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct PasskeyConfig {
    pub rp_id: String,
    pub origin: String,
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub public_origin: String,
    pub auto_provision: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let mode = match env_or("COWORK_MODE", "all").as_str() {
            "api" => ProcessMode::Api,
            "worker" => ProcessMode::Worker,
            "all" => ProcessMode::All,
            other => bail!("COWORK_MODE must be api, worker, or all; got {other}"),
        };
        let bootstrap_token = required_secret("COWORK_BOOTSTRAP_TOKEN")?;
        if bootstrap_token.len() < 32 {
            bail!("COWORK_BOOTSTRAP_TOKEN must contain at least 32 characters");
        }

        let runner_url = nonempty("COWORK_RUNNER_URL");
        let runner_signing_key = optional_secret("COWORK_RUNNER_SIGNING_KEY")?;
        if runner_url.is_some() != runner_signing_key.is_some() {
            bail!("COWORK_RUNNER_URL and COWORK_RUNNER_SIGNING_KEY(_FILE) must be configured together");
        }
        let object_store = if let Some(endpoint) = nonempty("COWORK_S3_ENDPOINT") {
            Some(ObjectStoreConfig {
                endpoint,
                region: env_or("COWORK_S3_REGION", "us-east-1"),
                bucket: env_or("COWORK_S3_BUCKET", "cowork-blobs"),
                addressing_style: env_or("COWORK_S3_ADDRESSING_STYLE", "path"),
                access_key: required_secret("COWORK_S3_ACCESS_KEY")?,
                secret_key: required_secret("COWORK_S3_SECRET_KEY")?,
                session_token: optional_secret("COWORK_S3_SESSION_TOKEN")?,
                master_key: required_secret("COWORK_STORAGE_MASTER_KEY")?,
            })
        } else {
            None
        };
        let fcm = optional_secret("COWORK_FCM_SERVICE_ACCOUNT")?
            .map(|value| {
                let parsed: FcmConfig = serde_json::from_str(&value)
                    .context("COWORK_FCM_SERVICE_ACCOUNT must be valid service-account JSON")?;
                if parsed.project_id.trim().is_empty()
                    || parsed.project_id.contains('/')
                    || parsed.client_email.trim().is_empty()
                    || !parsed.private_key.contains("BEGIN PRIVATE KEY")
                    || !parsed.token_uri.starts_with("https://")
                {
                    bail!("COWORK_FCM_SERVICE_ACCOUNT contains invalid required fields");
                }
                Ok(parsed)
            })
            .transpose()?;
        let web_push = if parse_or("COWORK_WEB_PUSH_ENABLED", true)? {
            let vapid_private_key = match optional_secret("COWORK_WEB_PUSH_VAPID_PRIVATE_KEY")? {
                Some(key) => Some(key),
                None => object_store.as_ref().map(|storage| {
                    let digest = Sha256::digest(
                        format!("open-cowork-web-push-v1\0{}", storage.master_key).as_bytes(),
                    );
                    URL_SAFE_NO_PAD.encode(digest)
                }),
            };
            vapid_private_key.map(|vapid_private_key| WebPushConfig {
                vapid_private_key,
                subject: env_or("COWORK_WEB_PUSH_SUBJECT", "mailto:admin@localhost.invalid"),
            })
        } else {
            None
        };
        let public_origin = nonempty("COWORK_PUBLIC_ORIGIN");
        let passkeys = match (nonempty("COWORK_WEBAUTHN_RP_ID"), public_origin.clone()) {
            (Some(rp_id), Some(origin)) => Some(PasskeyConfig { rp_id, origin }),
            (None, _) => None,
            (Some(_), None) => {
                bail!("COWORK_PUBLIC_ORIGIN is required when COWORK_WEBAUTHN_RP_ID is configured")
            }
        };
        let oidc_issuer = nonempty("COWORK_OIDC_ISSUER");
        let oidc_client_id = nonempty("COWORK_OIDC_CLIENT_ID");
        let oidc_client_secret = optional_secret("COWORK_OIDC_CLIENT_SECRET")?;
        let oidc = match (oidc_issuer, oidc_client_id, oidc_client_secret, public_origin) {
            (Some(issuer), Some(client_id), Some(client_secret), Some(public_origin)) => {
                Some(OidcConfig {
                    issuer,
                    client_id,
                    client_secret,
                    public_origin,
                    auto_provision: parse_or("COWORK_OIDC_AUTO_PROVISION", false)?,
                })
            }
            (None, None, None, _) => None,
            _ => bail!("COWORK_OIDC_ISSUER, COWORK_OIDC_CLIENT_ID, COWORK_OIDC_CLIENT_SECRET(_FILE), and COWORK_PUBLIC_ORIGIN must be configured together"),
        };
        let model_input_cost_micros_per_million =
            optional_parse("COWORK_MODEL_INPUT_COST_MICROS_PER_MILLION")?;
        let model_output_cost_micros_per_million =
            optional_parse("COWORK_MODEL_OUTPUT_COST_MICROS_PER_MILLION")?;
        if model_input_cost_micros_per_million.is_some()
            != model_output_cost_micros_per_million.is_some()
        {
            bail!("input and output model prices must be configured together");
        }
        let maintenance_every_polls = parse_or("COWORK_MAINTENANCE_EVERY_POLLS", 60_u64)?;
        if maintenance_every_polls == 0 {
            bail!("COWORK_MAINTENANCE_EVERY_POLLS must be greater than zero");
        }

        Ok(Self {
            mode,
            listen_addr: env_or("COWORK_LISTEN_ADDR", "0.0.0.0:8080")
                .parse()
                .context("invalid COWORK_LISTEN_ADDR")?,
            database_url: required_secret("DATABASE_URL")?,
            bootstrap_token,
            bootstrap_user_id: env_or(
                "COWORK_BOOTSTRAP_USER_ID",
                "00000000-0000-0000-0000-000000000001",
            )
            .parse()
            .context("invalid COWORK_BOOTSTRAP_USER_ID")?,
            database_max_connections: parse_or("COWORK_DATABASE_MAX_CONNECTIONS", 20)?,
            worker_id: env::var("COWORK_WORKER_ID")
                .ok()
                .map(|value| value.parse().context("invalid COWORK_WORKER_ID"))
                .transpose()?
                .unwrap_or_else(Uuid::new_v4),
            worker_poll_interval: Duration::from_millis(parse_or(
                "COWORK_WORKER_POLL_MS",
                1_000_u64,
            )?),
            maintenance_every_polls,
            lease_duration: Duration::from_secs(parse_or("COWORK_LEASE_SECONDS", 90_u64)?),
            model_base_url: nonempty("COWORK_MODEL_BASE_URL"),
            model_api_key: optional_secret("COWORK_MODEL_API_KEY")?,
            model_name: env_or("COWORK_MODEL_NAME", "gpt-5-mini"),
            model_input_cost_micros_per_million,
            model_output_cost_micros_per_million,
            server_capabilities: env_or(
                "COWORK_SERVER_CAPABILITIES",
                "model.external,tool.mcp.invoke",
            )
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Capability::from)
            .collect(),
            runner_url,
            runner_signing_key,
            object_store,
            push: PushConfig { fcm, web_push },
            passkeys,
            oidc,
        })
    }
}

fn optional_parse<T>(name: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    nonempty(name)
        .map(|value| {
            value
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
        })
        .transpose()
}

fn required_secret(name: &str) -> Result<String> {
    optional_secret(name)?
        .with_context(|| format!("missing {name} or {name}_FILE environment variable"))
}

fn optional_secret(name: &str) -> Result<Option<String>> {
    if let Some(value) = nonempty(name) {
        return Ok(Some(value));
    }
    let file_name = format!("{name}_FILE");
    let Some(path) = nonempty(&file_name) else {
        return Ok(None);
    };
    let value = fs::read_to_string(&path)
        .with_context(|| format!("failed to read secret file {path} from {file_name}"))?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("secret file configured through {file_name} is empty");
    }
    Ok(Some(value))
}

fn nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_or<T>(name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(name) {
        Ok(value) => value.parse().with_context(|| format!("invalid {name}")),
        Err(_) => Ok(default),
    }
}
