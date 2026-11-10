//! Read-only, per-user virtual model profiles. No imports, cached grants or
//! provider credentials cross the management callback. Legacy/local profiles
//! keep their existing behaviour; this does not impose a global model policy.
use crate::{error::ApiError, providers::ResolvedServerProvider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use cowork_contracts::{ProviderProfile, SCHEMA_VERSION};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{collections::HashSet, io::Read, path::Path, time::Duration};
use uuid::Uuid;

const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const ENV_PREFIX: &str = "COWORK_MANAGED_MODEL_SECRET_";
const SECRET_ROOT: &str = "/run/secrets/cowork-managed-models";
const PROXY_TOKEN_SECONDS: i64 = 3600;
pub(crate) mod execution;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProxyToken {
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    model_resource_id: Uuid,
    issued_at: i64,
    expires_at: i64,
}
pub(crate) fn model_proxy_token(
    key: &str,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
    model: Uuid,
    now: i64,
) -> Result<String, ApiError> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ProxyToken {
        organisation_id: org,
        instance_id: instance,
        user_id: user,
        model_resource_id: model,
        issued_at: now,
        expires_at: now + PROXY_TOKEN_SECONDS,
    })?);
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| unavailable())?;
    mac.update(b"cowork-managed-model-proxy-v1\0");
    mac.update(payload.as_bytes());
    Ok(format!(
        "cmp1.{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
pub(crate) fn verify_model_proxy_token(
    token: &str,
    key: &str,
    org: Uuid,
    instance: Uuid,
    user: Uuid,
    model: Uuid,
    now: i64,
) -> Result<(), ApiError> {
    let denied =
        || ApiError::Unauthorized("managed model proxy token is invalid or expired".into());
    if token.len() > 2048 {
        return Err(denied());
    }
    let mut parts = token.split('.');
    if parts.next() != Some("cmp1") {
        return Err(denied());
    }
    let payload = parts.next().ok_or_else(denied)?;
    let signature = parts.next().ok_or_else(denied)?;
    if parts.next().is_some() {
        return Err(denied());
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| denied())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| denied())?;
    mac.update(b"cowork-managed-model-proxy-v1\0");
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).map_err(|_| denied())?;
    let claims: ProxyToken =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| denied())?)
            .map_err(|_| denied())?;
    if (
        claims.organisation_id,
        claims.instance_id,
        claims.user_id,
        claims.model_resource_id,
    ) != (org, instance, user, model)
        || claims.issued_at > now + 30
        || claims.expires_at <= now
        || claims.expires_at <= claims.issued_at
        || claims.expires_at - claims.issued_at > PROXY_TOKEN_SECONDS
    {
        return Err(denied());
    }
    Ok(())
}

fn unavailable() -> ApiError {
    ApiError::Unavailable("managed model authority or local secret binding is unavailable".into())
}

struct Configuration {
    base: String,
    organisation: Uuid,
    instance: Uuid,
    key: String,
}
fn configuration() -> Result<Option<Configuration>, ApiError> {
    if !crate::fleet::enabled() {
        return Ok(None);
    }
    Ok(Some(Configuration {
        base: std::env::var("COWORK_FLEET_MANAGEMENT_URL").map_err(|_| unavailable())?,
        organisation: std::env::var("COWORK_ORGANISATION_ID")
            .map_err(|_| unavailable())?
            .parse()
            .map_err(|_| unavailable())?,
        instance: std::env::var("COWORK_INSTANCE_ID")
            .map_err(|_| unavailable())?
            .parse()
            .map_err(|_| unavailable())?,
        key: crate::fleet::secret("COWORK_FLEET_KEY_FILE")?,
    }))
}

#[derive(Clone, Deserialize)]
struct Resource {
    id: Uuid,
    kind: String,
    name: String,
    configuration: Value,
    secret_reference: Option<String>,
    revision: i64,
    enabled: bool,
    available: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}
#[derive(Deserialize)]
struct Consumer {
    kind: String,
    version: u16,
}
#[derive(Deserialize)]
struct Manifest {
    contract_version: u16,
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    environment_id: Option<Uuid>,
    assignment_revision: i64,
    items: Vec<Resource>,
    dependencies: Vec<Resource>,
    consumer: Consumer,
}
impl Manifest {
    fn validate(&self, config: &Configuration, user: Uuid) -> Result<(), ApiError> {
        if self.contract_version != 1
            || self.organisation_id != config.organisation
            || self.instance_id != config.instance
            || self.user_id != user
            || self.environment_id.is_some()
            || self.assignment_revision < 0
            || self.consumer.kind != "model_provider"
            || self.consumer.version != 1
            || self.items.len() > 200
            || self.dependencies.len() > 200
        {
            return Err(unavailable());
        }
        let mut ids = HashSet::new();
        for item in self.items.iter().chain(&self.dependencies) {
            if item.id.is_nil()
                || item.revision < 1
                || !item.enabled
                || !item.available
                || item.deleted_at.is_some()
                || !ids.insert(item.id)
            {
                return Err(unavailable());
            }
        }
        if self.items.iter().any(|r| r.kind != "model")
            || self.dependencies.iter().any(|r| r.kind != "endpoint")
        {
            return Err(unavailable());
        }
        Ok(())
    }
}
async fn fetch(config: &Configuration, user: Uuid) -> Result<Manifest, ApiError> {
    let client = crate::management_transport::client(&config.base, Duration::from_secs(5))?;
    let mut response = client
        .get(format!(
            "{}/management/runner/v1/resource-entitlements",
            config.base.trim_end_matches('/')
        ))
        .header("x-cowork-instance-key", &config.key)
        .query(&[
            ("organisation_id", config.organisation.to_string()),
            ("instance_id", config.instance.to_string()),
            ("user_id", user.to_string()),
            ("consumer_kind", "model_provider".to_owned()),
            ("consumer_version", "1".to_owned()),
        ])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_MANIFEST_BYTES as u64)
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    manifest.validate(config, user)?;
    Ok(manifest)
}

fn leaf(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn supported_reference(reference: Option<&str>) -> bool {
    supported_resource_reference(reference, ENV_PREFIX)
}
fn supported_resource_reference(reference: Option<&str>, prefix: &str) -> bool {
    match reference {
        None => true,
        Some(value) => {
            value
                .strip_prefix("env:")
                .is_some_and(|name| name.strip_prefix(prefix).is_some_and(leaf))
                || value.strip_prefix("secret:").is_some_and(leaf)
        }
    }
}
pub(crate) fn resolve_secret(reference: Option<&str>) -> Result<Option<String>, ApiError> {
    resolve_resource_secret(reference, ENV_PREFIX, SECRET_ROOT)
}
fn resolve_resource_secret(
    reference: Option<&str>,
    prefix: &str,
    secret_root: &str,
) -> Result<Option<String>, ApiError> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    if !supported_resource_reference(Some(reference), prefix) {
        return Err(unavailable());
    }
    let value = if let Some(name) = reference.strip_prefix("env:") {
        std::env::var(name).map_err(|_| unavailable())?
    } else {
        let name = reference.strip_prefix("secret:").ok_or_else(unavailable)?;
        let root = Path::new(secret_root)
            .canonicalize()
            .map_err(|_| unavailable())?;
        let path = root.join(name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| unavailable())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 32768
            || path.canonicalize().map_err(|_| unavailable())?.parent() != Some(root.as_path())
        {
            return Err(unavailable());
        }
        let mut value = String::new();
        std::fs::File::open(path)
            .map_err(|_| unavailable())?
            .take(32769)
            .read_to_string(&mut value)
            .map_err(|_| unavailable())?;
        value
    };
    let value = value.trim();
    if value.is_empty()
        || value.len() > 32768
        || value.contains('\0')
        || value.contains('\r')
        || value.contains('\n')
    {
        return Err(unavailable());
    }
    Ok(Some(value.to_owned()))
}

struct Model<'a> {
    resource: &'a Resource,
    endpoint: &'a Resource,
    model_id: &'a str,
}
fn models(manifest: &Manifest) -> Vec<Model<'_>> {
    manifest
        .items
        .iter()
        .filter_map(|resource| {
            let endpoint_id = resource
                .configuration
                .get("endpoint_resource_id")?
                .as_str()?
                .parse::<Uuid>()
                .ok()?;
            let endpoint = manifest.dependencies.iter().find(|r| r.id == endpoint_id)?;
            if endpoint.configuration.get("protocol")?.as_str()? != "openai_compatible" {
                return None;
            }
            let model_id = resource.configuration.get("model_id")?.as_str()?;
            let base_url = endpoint.configuration.get("base_url")?.as_str()?;
            let url = reqwest::Url::parse(base_url).ok()?;
            if model_id.trim().is_empty()
                || model_id.len() > 200
                || base_url.len() > 2048
                || !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return None;
            }
            let reference = resource
                .secret_reference
                .as_deref()
                .or(endpoint.secret_reference.as_deref());
            if !supported_reference(reference) {
                return None;
            }
            Some(Model {
                resource,
                endpoint,
                model_id,
            })
        })
        .collect()
}
fn proxy_url(config: &Configuration, user: Uuid, model: Uuid) -> String {
    format!(
        "{}/management/runner/v1/resource-models/{}/{}/{}/{}/v1",
        config.base.trim_end_matches('/'),
        config.organisation,
        config.instance,
        user,
        model
    )
}
fn profiles_from(manifest: &Manifest, config: &Configuration) -> Vec<ProviderProfile> {
    models(manifest).into_iter().map(|model| {
        let r = model.resource;
        ProviderProfile {
            schema_version: SCHEMA_VERSION, id: r.id, revision: r.revision,
            etag: format!("W/\"managed:{}:{}:{}:{}\"", r.id, r.revision, model.endpoint.revision, manifest.assignment_revision),
            owner_user_id: None, team_id: None, name: r.name.clone(), provider_kind: "openai_compatible".into(),
            model_defaults: json!({"base_url":proxy_url(config,manifest.user_id,r.id),"model":model.model_id,"endpoint_binding":"server","auth_mode":"bearer",
                "source":"management_catalog","read_only":true,"managed_resource_id":r.id,"managed_endpoint_id":model.endpoint.id,"managed_assignment_revision":manifest.assignment_revision,
                "context_window":r.configuration.get("context_window"),"max_output_tokens":r.configuration.get("max_output_tokens"),"verify_tls_certificates":true}),
            has_secret: true, created_at: r.created_at, updated_at: r.updated_at.max(model.endpoint.updated_at), deleted_at: None,
        }
    }).collect()
}
fn resolve_from(
    manifest: &Manifest,
    config: &Configuration,
    id: Uuid,
) -> Result<Option<ResolvedServerProvider>, ApiError> {
    let Some(model) = models(manifest).into_iter().find(|m| m.resource.id == id) else {
        return Ok(None);
    };
    Ok(Some(ResolvedServerProvider {
        base_url: proxy_url(config, manifest.user_id, id),
        api_key: Some(model_proxy_token(
            &config.key,
            config.organisation,
            config.instance,
            manifest.user_id,
            id,
            Utc::now().timestamp(),
        )?),
        model: model.model_id.to_owned(),
        timeout: Duration::from_secs(20 * 60),
        max_steps: 64,
        verify_tls_certificates: true,
    }))
}
pub(crate) async fn profiles(user: Uuid) -> Result<Vec<ProviderProfile>, ApiError> {
    let Some(config) = configuration()? else {
        return Ok(vec![]);
    };
    Ok(profiles_from(&fetch(&config, user).await?, &config))
}
pub(crate) async fn resolve(
    user: Uuid,
    id: Uuid,
) -> Result<Option<ResolvedServerProvider>, ApiError> {
    let Some(config) = configuration()? else {
        return Ok(None);
    };
    resolve_from(&fetch(&config, user).await?, &config, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::get, Json, Router};
    use std::sync::{Arc, Mutex};

    #[test]
    fn proxy_tokens_bind_exact_resource_scope_expiry_and_current_key() {
        let ids = [
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        ];
        let now = 1_790_000_000;
        let token =
            model_proxy_token("fixture-current-key", ids[0], ids[1], ids[2], ids[3], now).unwrap();
        assert!(verify_model_proxy_token(
            &token,
            "fixture-current-key",
            ids[0],
            ids[1],
            ids[2],
            ids[3],
            now + 1
        )
        .is_ok());
        for index in 0..4 {
            let mut foreign = ids;
            foreign[index] = Uuid::new_v4();
            assert!(verify_model_proxy_token(
                &token,
                "fixture-current-key",
                foreign[0],
                foreign[1],
                foreign[2],
                foreign[3],
                now
            )
            .is_err());
        }
        assert!(verify_model_proxy_token(
            &token,
            "fixture-new-key",
            ids[0],
            ids[1],
            ids[2],
            ids[3],
            now
        )
        .is_err());
        assert!(verify_model_proxy_token(
            &token,
            "fixture-current-key",
            ids[0],
            ids[1],
            ids[2],
            ids[3],
            now + 3600
        )
        .is_err());
        assert!(verify_model_proxy_token(
            &token,
            "fixture-current-key",
            ids[0],
            ids[1],
            ids[2],
            ids[3],
            now - 60
        )
        .is_err());
        assert!(!token.contains("fixture-current-key"));
    }

    fn fixture(config: &Configuration, user: Uuid, model: Uuid, endpoint: Uuid) -> Value {
        let item = |id, kind, configuration| {
            json!({"id":id,"kind":kind,"name":"Managed fixture","configuration":configuration,"secret_reference":null,
            "revision":1,"enabled":true,"available":true,"created_at":"2026-10-17T00:00:00Z","updated_at":"2026-10-17T00:00:00Z","deleted_at":null})
        };
        json!({"contract_version":1,"organisation_id":config.organisation,"instance_id":config.instance,"user_id":user,"environment_id":null,"assignment_revision":1,
            "consumer":{"kind":"model_provider","version":1},"items":[item(model,"model",json!({"model_id":"fixture-model","endpoint_resource_id":endpoint}))],
            "dependencies":[item(endpoint,"endpoint",json!({"base_url":"https://models.example.invalid/v1","protocol":"openai_compatible"}))]})
    }

    #[tokio::test]
    async fn fresh_manifest_revocation_foreign_scope_and_transport_failure_fail_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Configuration {
            base: format!("http://{}", listener.local_addr().unwrap()),
            organisation: Uuid::new_v4(),
            instance: Uuid::new_v4(),
            key: "fixture-current-instance-key".into(),
        };
        let user = Uuid::new_v4();
        let model = Uuid::new_v4();
        let endpoint = Uuid::new_v4();
        let original = fixture(&config, user, model, endpoint);
        let body = Arc::new(Mutex::new(original.clone()));
        let app = Router::new()
            .route(
                "/management/runner/v1/resource-entitlements",
                get(|State(body): State<Arc<Mutex<Value>>>| async move {
                    Json(body.lock().unwrap().clone())
                }),
            )
            .with_state(body.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let first = fetch(&config, user).await.unwrap();
        assert_eq!(profiles_from(&first, &config)[0].id, model);
        assert_eq!(
            resolve_from(&first, &config, model).unwrap().unwrap().model,
            "fixture-model"
        );
        body.lock().unwrap()["items"] = json!([]);
        assert!(
            resolve_from(&fetch(&config, user).await.unwrap(), &config, model)
                .unwrap()
                .is_none(),
            "revocation must not use the previous manifest"
        );
        *body.lock().unwrap() = original.clone();
        body.lock().unwrap()["organisation_id"] = json!(Uuid::new_v4());
        assert!(fetch(&config, user).await.is_err());
        *body.lock().unwrap() = original.clone();
        body.lock().unwrap()["environment_id"] = json!(Uuid::new_v4());
        assert!(
            fetch(&config, user).await.is_err(),
            "normal model execution has no trusted environment scope"
        );
        *body.lock().unwrap() = original;
        body.lock().unwrap()["items"][0]["available"] = json!(false);
        assert!(fetch(&config, user).await.is_err());
        server.abort();
        let _ = server.await;
        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.base = format!("http://{}", unused.local_addr().unwrap());
        drop(unused);
        assert!(
            fetch(&config, user).await.is_err(),
            "authority failure cannot reuse a grant"
        );
    }

    #[test]
    fn provider_subset_and_secret_bindings_never_read_arbitrary_configuration() {
        for reference in [
            "env:PATH",
            "env:COWORK_AUTH_MASTER_KEY",
            "secret:../auth_master_key",
            "secret:dir/token",
            "vault:provider/key",
            "env:COWORK_MANAGED_MODEL_SECRET_",
        ] {
            assert!(!supported_reference(Some(reference)));
            assert!(resolve_secret(Some(reference)).is_err());
        }
        assert!(supported_reference(Some(
            "env:COWORK_MANAGED_MODEL_SECRET_TEST"
        )));
        assert!(supported_reference(Some("secret:provider-key")));
        let config = Configuration {
            base: "http://127.0.0.1".into(),
            organisation: Uuid::new_v4(),
            instance: Uuid::new_v4(),
            key: String::new(),
        };
        let mut value = fixture(&config, Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        value["dependencies"][0]["configuration"]["protocol"] = json!("anthropic");
        assert!(profiles_from(&serde_json::from_value(value.clone()).unwrap(), &config).is_empty());
        value["dependencies"][0]["configuration"]["protocol"] = json!("openai_compatible");
        value["dependencies"][0]["secret_reference"] = json!("env:COWORK_AUTH_MASTER_KEY");
        assert!(profiles_from(&serde_json::from_value(value).unwrap(), &config).is_empty());
    }
}
