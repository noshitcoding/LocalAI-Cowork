//! Provider-scoped directory authorities. A cold roster is never a current grant.
use super::*;
use std::{sync::Arc, time::Duration};
pub mod catalog;
mod ldap_transport;
mod ldap_worker;
mod origin;
mod resources;
mod scim;
mod store;
pub use ldap_worker::run;
pub use origin::{oidc_origin, stamp_code, stamp_session, validate_origin};
pub use scim::router as scim_router;
pub use store::{register, run_cli};
fn sha(value: &Value) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("JSON serialization"),
    ))
}
fn unavailable() -> ApiError {
    ApiError::Unauthorized("directory authority is inactive, changed or unavailable".into())
}
fn conflict() -> ApiError {
    ApiError::Conflict("reload the current directory revision and source before retrying".into())
}
fn secrets(state: &AppState) -> Result<&crate::storage::SecretStore, ApiError> {
    state
        .auth_secrets
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("directory encryption is unavailable".into()))
}
fn seal(
    state: &AppState,
    c: Uuid,
    purpose: &str,
    value: &Value,
) -> Result<crate::storage::SealedValue, ApiError> {
    secrets(state)?.seal_for_directory(
        c,
        &serde_json::to_vec(&json!({"version":1,"connector":c,"purpose":purpose,"value":value}))?,
    )
}
fn open(
    state: &AppState,
    c: Uuid,
    purpose: &str,
    s: &crate::storage::SealedValue,
) -> Result<Value, ApiError> {
    let bytes = secrets(state)?.open_for_directory(c, s)?;
    if bytes.len() > 2_097_152 {
        return Err(unavailable());
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    if v["version"] != 1 || v["connector"] != json!(c) || v["purpose"] != purpose {
        return Err(unavailable());
    }
    Ok(v["value"].clone())
}
fn sealed(
    row: &sqlx::postgres::PgRow,
    prefix: &str,
) -> Result<crate::storage::SealedValue, ApiError> {
    let nonce: Vec<u8> = row.try_get(format!("{prefix}_nonce").as_str())?;
    let wrap: Vec<u8> = row.try_get(format!("{prefix}_wrap_nonce").as_str())?;
    Ok(crate::storage::SealedValue {
        ciphertext: row.try_get(format!("{prefix}_ciphertext").as_str())?,
        encrypted_data_key: row.try_get(format!("{prefix}_key").as_str())?,
        nonce: nonce.try_into().map_err(|_| unavailable())?,
        wrap_nonce: wrap.try_into().map_err(|_| unavailable())?,
    })
}
#[derive(Clone)]
pub(super) struct Current {
    pub connector: Arc<catalog::Connector>,
    pub registry_epoch: Uuid,
    pub settings_epoch: Uuid,
    pub settings_revision: i64,
    pub provider_fingerprint: String,
    pub sso_revision: i64,
    pub policy_revision: i64,
    pub state: String,
    pub adopt: bool,
    pub provision: bool,
    pub retention: i32,
}
impl Current {
    fn stamp(&self) -> Value {
        json!({"connector_id":self.connector.id,"organisation_id":self.connector.org,"registry_epoch":self.registry_epoch,"settings_epoch":self.settings_epoch,"settings_revision":self.settings_revision,"provider_fingerprint":self.provider_fingerprint,"sso_revision":self.sso_revision,"policy_revision":self.policy_revision})
    }
}
pub(super) async fn current(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    allow_paused: bool,
) -> Result<Current, ApiError> {
    let c = state.directory.get(&id).cloned().ok_or_else(unavailable)?;
    // One ordering for settings, ingest, containment and login: organisation first.
    let policy: Value =
        sqlx::query_scalar("SELECT policy FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE")
            .bind(c.org)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(unavailable)?;
    let provider = state
        .organisation_oidc
        .get(&c.provider)
        .ok_or_else(unavailable)?;
    if provider.directory_identity(c.org) != Some((c.issuer.as_str(), c.client.as_str())) {
        return Err(unavailable());
    }
    let r=sqlx::query("SELECT epoch,revision FROM directory_connectors WHERE id=$1 AND organisation_id=$2 AND active AND provider_id=$3 AND issuer=$4 AND client_id=$5 AND kind=$6 AND fingerprint=$7 FOR UPDATE")
        .bind(id).bind(c.org).bind(&c.provider).bind(&c.issuer).bind(&c.client).bind(&c.kind).bind(&c.fingerprint).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    let s = sqlx::query("SELECT * FROM directory_settings WHERE connector_id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(unavailable)?;
    let sso=sqlx::query("SELECT revision,provider_fingerprint FROM management_sso WHERE organisation_id=$1 AND enabled AND provider_id=$2 AND provider_fingerprint=$3 FOR SHARE")
        .bind(c.org).bind(&c.provider).bind(&provider.fingerprint).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    let state_name: String = s.get("state");
    let p = policy["revision"].as_i64().ok_or_else(unavailable)?;
    if (!allow_paused && state_name == "paused")
        || s.get::<String, _>("provider_fingerprint") != provider.fingerprint
        || s.get::<i64, _>("sso_revision") != sso.get::<i64, _>("revision")
        || s.get::<i64, _>("policy_revision") != p
    {
        return Err(unavailable());
    }
    Ok(Current {
        connector: c,
        registry_epoch: r.get("epoch"),
        settings_epoch: s.get("epoch"),
        settings_revision: s.get("revision"),
        provider_fingerprint: provider.fingerprint.clone(),
        sso_revision: sso.get("revision"),
        policy_revision: p,
        state: state_name,
        adopt: s.get("adopt_provider_memberships"),
        provision: s.get("provision_accounts"),
        retention: s.get("retention_days"),
    })
}
pub(super) async fn recheck(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    old: &Current,
) -> Result<Current, ApiError> {
    let c = current(state, tx, old.connector.id, false).await?;
    if c.stamp() != old.stamp() || c.state != old.state {
        return Err(unavailable());
    }
    Ok(c)
}
pub fn management_router() -> Router<AppState> {
    Router::new()
        .route("/organisations/{org}/directory", get(store::list))
        .route(
            "/organisations/{org}/directory/{connector}",
            put(store::settings),
        )
        .route(
            "/organisations/{org}/directory/{connector}/containment",
            post(store::prepare_containment),
        )
        .route(
            "/organisations/{org}/directory/{connector}/containment/{plan}",
            post(store::confirm_containment),
        )
}
