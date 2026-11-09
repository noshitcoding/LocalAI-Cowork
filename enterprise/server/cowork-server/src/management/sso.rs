//! Operator-owned provider credentials, tenant-owned admission rules.
use super::*;
use crate::{config::OidcConfig, oidc};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderConfig {
    id: String,
    label: String,
    organisation_ids: Vec<Uuid>,
    issuer: String,
    client_id: String,
    client_secret_file: String,
    public_origin: String,
    #[serde(default = "default_groups_claim")]
    groups_claim: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    assurance: Option<crate::oidc_assurance::Policy>,
}
fn default_groups_claim() -> String {
    "groups".into()
}

pub struct Provider {
    pub id: String,
    label: String,
    organisations: HashSet<Uuid>,
    pub fingerprint: String,
    pub groups_claim: String,
    pub scopes: Vec<String>,
    config: OidcConfig,
    cached: Mutex<Option<(Instant, Arc<oidc::OidcService>)>>,
}
pub type Registry = HashMap<String, Arc<Provider>>;

pub fn load(management: bool) -> anyhow::Result<Registry> {
    if !management {
        return Ok(HashMap::new());
    }
    let path = match std::env::var("COWORK_ORGANISATION_OIDC_PROVIDERS_FILE") {
        Ok(path) => path,
        Err(std::env::VarError::NotPresent) => return Ok(HashMap::new()),
        Err(error) => return Err(error.into()),
    };
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(
        bytes.len() <= 1_048_576,
        "SSO provider catalog is too large"
    );
    let configs: Vec<ProviderConfig> = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(configs.len() <= 256, "too many SSO providers");
    let mut registry = HashMap::new();
    for mut entry in configs {
        anyhow::ensure!(
            !entry.id.is_empty()
                && entry.id.len() <= 64
                && entry
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "invalid SSO provider id"
        );
        anyhow::ensure!(
            !entry.label.trim().is_empty() && entry.label.len() <= 120,
            "invalid SSO provider label"
        );
        anyhow::ensure!(
            !entry.organisation_ids.is_empty() && entry.organisation_ids.len() <= 1000,
            "SSO provider needs an explicit organisation allowlist"
        );
        anyhow::ensure!(
            !entry.client_id.is_empty() && entry.client_id.len() <= 512,
            "invalid SSO client id"
        );
        anyhow::ensure!(
            !entry.groups_claim.is_empty()
                && entry.groups_claim.len() <= 120
                && !entry.groups_claim.chars().any(char::is_control),
            "invalid SSO groups claim"
        );
        anyhow::ensure!(
            !["iss", "sub", "aud", "exp", "iat", "nonce", "email", "azp"]
                .contains(&entry.groups_claim.as_str()),
            "reserved SSO groups claim"
        );
        anyhow::ensure!(
            entry.scopes.len() <= 16
                && entry.scopes.iter().all(|s| !s.is_empty()
                    && s.len() <= 120
                    && s.bytes()
                        .all(|b| (0x21..=0x7e).contains(&b) && !b"\"\\".contains(&b))),
            "invalid SSO scopes"
        );
        let issuer = oidc::validated_https_or_loopback_url(&entry.issuer, "issuer")?;
        anyhow::ensure!(
            issuer.query().is_none()
                && issuer.fragment().is_none()
                && entry.issuer == entry.issuer.trim(),
            "invalid SSO issuer"
        );
        let origin = oidc::validated_https_or_loopback_url(&entry.public_origin, "public_origin")?;
        anyhow::ensure!(
            origin.path() == "/" && origin.query().is_none() && origin.fragment().is_none(),
            "SSO public_origin must be an origin"
        );
        let secret = std::fs::read_to_string(&entry.client_secret_file)?
            .trim()
            .to_owned();
        anyhow::ensure!(
            !secret.is_empty() && secret.len() <= 8192,
            "invalid SSO client secret file"
        );
        entry.organisation_ids.sort();
        entry.organisation_ids.dedup();
        entry.scopes.sort();
        entry.scopes.dedup();
        entry.assurance = entry
            .assurance
            .map(|policy| policy.validate(&entry.issuer, &entry.client_id))
            .transpose()?;
        let mut fingerprint_fields = json!([
            entry.id,
            entry.issuer,
            entry.client_id,
            entry.public_origin,
            entry.groups_claim,
            entry.scopes,
            entry.organisation_ids
        ]);
        // Preserve the exact legacy fingerprint when no mapping is configured.
        if let Some(policy) = &entry.assurance {
            fingerprint_fields
                .as_array_mut()
                .unwrap()
                .push(json!(policy.fingerprint()));
        }
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&fingerprint_fields)?));
        let provider = Provider {
            id: entry.id.clone(),
            label: entry.label,
            organisations: entry.organisation_ids.into_iter().collect(),
            fingerprint,
            groups_claim: entry.groups_claim,
            scopes: entry.scopes,
            config: OidcConfig {
                issuer: entry.issuer,
                client_id: entry.client_id,
                client_secret: secret,
                public_origin: entry.public_origin,
                auto_provision: false,
                assurance: entry.assurance,
            },
            cached: Mutex::new(None),
        };
        anyhow::ensure!(
            registry.insert(entry.id, Arc::new(provider)).is_none(),
            "duplicate SSO provider id"
        );
    }
    Ok(registry)
}
impl Provider {
    pub(crate) fn directory_identity(&self, org: Uuid) -> Option<(&str, &str)> {
        self.organisations
            .contains(&org)
            .then_some((self.config.issuer.as_str(), self.config.client_id.as_str()))
    }
    pub fn assurance_policy(&self) -> Option<&crate::oidc_assurance::Policy> {
        self.config.assurance.as_ref()
    }
    pub async fn service(&self, auto_provision: bool) -> Result<Arc<oidc::OidcService>, ApiError> {
        // Discovery outages affect this provider only. Do not hold database locks here.
        let mut cache = self.cached.lock().await;
        if cache
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(300))
        {
            let service = oidc::build(Some(&self.config))
                .await
                .map_err(|_| {
                    ApiError::Unavailable("organisation identity provider is unavailable".into())
                })?
                .ok_or_else(denied)?;
            *cache = Some((Instant::now(), Arc::new(service)));
        }
        let mut service = (*cache.as_ref().unwrap().1).clone();
        service.auto_provision = auto_provision;
        Ok(Arc::new(service))
    }
}
fn provider(state: &AppState, org: Uuid, id: &str) -> Option<Arc<Provider>> {
    state
        .organisation_oidc
        .get(id)
        .filter(|p| p.organisations.contains(&org))
        .cloned()
}

#[derive(Deserialize)]
pub struct Configuration {
    pub revision: i64,
    pub enabled: bool,
    pub provider_id: Option<String>,
    pub provider_fingerprint: Option<String>,
    pub auto_provision: bool,
    pub group_mappings: Vec<SsoGroupMapping>,
}
pub struct Context {
    pub org: Uuid,
    pub configuration: Configuration,
    pub provider: Arc<Provider>,
}
async fn configuration(state: &AppState, org: Uuid) -> Result<Configuration, ApiError> {
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM management_organisations WHERE id=$1 AND archived_at IS NULL)",
    )
    .bind(org)
    .fetch_one(&state.pool)
    .await?;
    if !active {
        return Err(denied());
    }
    let value: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(s) FROM management_sso s WHERE organisation_id=$1")
            .bind(org)
            .fetch_optional(&state.pool)
            .await?;
    Ok(match value {
        Some(v) => serde_json::from_value(v)?,
        None => Configuration {
            revision: 0,
            enabled: false,
            provider_id: None,
            provider_fingerprint: None,
            auto_provision: false,
            group_mappings: vec![],
        },
    })
}
fn available(state: &AppState, org: Uuid, c: &Configuration) -> Option<Arc<Provider>> {
    let p = provider(state, org, c.provider_id.as_deref()?)?;
    (c.provider_fingerprint.as_deref() == Some(p.fingerprint.as_str())).then_some(p)
}
pub async fn context(state: &AppState, org: Uuid) -> Result<Context, ApiError> {
    let c = configuration(state, org).await?;
    if !c.enabled {
        return Err(denied());
    }
    let p = available(state, org, &c).ok_or_else(denied)?;
    Ok(Context {
        org,
        configuration: c,
        provider: p,
    })
}
async fn lock_org(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT id FROM management_organisations WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)?;
    Ok(())
}
pub(super) async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    principal: &Principal,
) -> Result<(), ApiError> {
    lock_org(tx, org).await?;
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN management_members m ON m.user_id=u.id AND m.organisation_id=$1 WHERE u.id=$2 AND u.deleted_at IS NULL AND (u.platform_admin OR m.role='admin'))").bind(org).bind(principal.user_id).fetch_one(&mut **tx).await?;
    if principal.bootstrap || !allowed {
        return Err(denied());
    }
    Ok(())
}
pub async fn recheck(tx: &mut Transaction<'_, Postgres>, ctx: &Context) -> Result<(), ApiError> {
    lock_org(tx, ctx.org).await?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_sso WHERE organisation_id=$1 AND enabled AND revision=$2 AND provider_fingerprint=$3 AND provider_id=$4)")
        .bind(ctx.org).bind(ctx.configuration.revision).bind(&ctx.provider.fingerprint).bind(&ctx.provider.id).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(ApiError::Unauthorized(
            "organisation SSO changed; start sign-in again".into(),
        ));
    }
    Ok(())
}

pub(super) async fn sso_settings(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrganisationSsoSettings>, ApiError> {
    membership(&state, &principal, org, true).await?;
    settings(&state, org).await.map(Json)
}
async fn settings(state: &AppState, org: Uuid) -> Result<OrganisationSsoSettings, ApiError> {
    let c = configuration(state, org).await?;
    let mut providers: Vec<_> = state
        .organisation_oidc
        .values()
        .filter(|p| p.organisations.contains(&org))
        .map(|p| OrganisationSsoProvider {
            id: p.id.clone(),
            label: p.label.clone(),
            issuer: p.config.issuer.clone(),
            groups_claim: p.groups_claim.clone(),
        })
        .collect();
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(OrganisationSsoSettings {
        organisation_id: org,
        revision: c.revision as u64,
        enabled: c.enabled,
        available: available(state, org, &c).is_some(),
        provider_id: c.provider_id,
        auto_provision: c.auto_provision,
        group_mappings: c.group_mappings,
        providers,
    })
}
pub(super) async fn update_sso(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(org): Path<Uuid>,
    Json(request): Json<UpdateOrganisationSso>,
) -> Result<Json<OrganisationSsoSettings>, ApiError> {
    let mut tx = state.pool.begin().await?;
    authorize(&mut tx, org, &principal).await?;
    let revision: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM management_sso WHERE organisation_id=$1")
            .bind(org)
            .fetch_optional(&mut *tx)
            .await?;
    if request.expected_revision != revision.unwrap_or(0) as u64 {
        return Err(ApiError::Conflict(
            "reload SSO settings before saving".into(),
        ));
    }
    let provider = match request.provider_id.as_deref() {
        Some(id) => Some(
            provider(&state, org, id)
                .ok_or_else(|| invalid("provider is not allowed for this organisation"))?,
        ),
        None => None,
    };
    if request.enabled && provider.is_none() {
        return Err(invalid("select an available provider"));
    }
    let mut seen = HashSet::new();
    if request.group_mappings.len() > 100
        || request.group_mappings.iter().any(|m| {
            m.group.is_empty()
                || m.group.len() > 256
                || m.group.trim() != m.group
                || m.group.chars().any(char::is_control)
                || !seen.insert(m.group.clone())
        })
    {
        return Err(invalid(
            "use at most 100 unique, nonempty group names (maximum 256 bytes)",
        ));
    }
    let manual_admin:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_members m JOIN users u ON u.id=m.user_id WHERE m.organisation_id=$1 AND m.role='admin' AND m.sso_provider_id IS NULL AND u.deleted_at IS NULL)").bind(org).fetch_one(&mut *tx).await?;
    if !manual_admin {
        return Err(ApiError::Conflict(
            "retain a manually assigned organisation administrator before changing SSO".into(),
        ));
    }
    let next = revision
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("SSO revision overflow"))?;
    sqlx::query("INSERT INTO management_sso(organisation_id,revision,enabled,provider_id,provider_fingerprint,auto_provision,group_mappings) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(organisation_id) DO UPDATE SET revision=$2,enabled=$3,provider_id=$4,provider_fingerprint=$5,auto_provision=$6,group_mappings=$7,updated_at=now()")
        .bind(org).bind(next).bind(request.enabled).bind(&request.provider_id).bind(provider.as_ref().map(|p|&p.fingerprint)).bind(request.auto_provision).bind(serde_json::to_value(&request.group_mappings)?).execute(&mut *tx).await?;
    let suspended = suspend(&mut tx, org).await?;
    append_audit(&mut tx,org,principal.user_id,"sso.update",org,json!({"revision":next,"enabled":request.enabled,"provider_id":request.provider_id,"auto_provision":request.auto_provision,"group_mappings":request.group_mappings,"suspended_memberships":suspended})).await?;
    tx.commit().await?;
    settings(&state, org).await.map(Json)
}
async fn suspend(tx: &mut Transaction<'_, Postgres>, org: Uuid) -> Result<u64, ApiError> {
    Ok(sqlx::query("UPDATE management_members SET role='suspended',content_admin=false WHERE organisation_id=$1 AND sso_provider_id IS NOT NULL AND role<>'suspended'").bind(org).execute(&mut **tx).await?.rows_affected())
}
pub async fn reconcile(state: &AppState) -> Result<(), ApiError> {
    let orgs: Vec<Uuid> =
        sqlx::query_scalar("SELECT organisation_id FROM management_sso ORDER BY organisation_id")
            .fetch_all(&state.pool)
            .await?;
    for org in orgs {
        let mut tx = state.pool.begin().await?;
        lock_org(&mut tx, org).await?;
        let value: Value =
            sqlx::query_scalar("SELECT to_jsonb(s) FROM management_sso s WHERE organisation_id=$1")
                .bind(org)
                .fetch_one(&mut *tx)
                .await?;
        let c: Configuration = serde_json::from_value(value)?;
        if !c.enabled || available(state, org, &c).is_none() {
            let count = suspend(&mut tx, org).await?;
            // Persist the denial so a still-running replica with an older
            // catalog cannot admit the withdrawn provider during a rollout.
            if c.enabled {
                sqlx::query("UPDATE management_sso SET enabled=false,revision=revision+1,updated_at=now() WHERE organisation_id=$1").bind(org).execute(&mut *tx).await?;
            }
            if count > 0 || c.enabled {
                append_audit(
                    &mut tx,
                    org,
                    Uuid::nil(),
                    "sso.provider_unavailable",
                    org,
                    json!({"suspended_memberships":count,"disabled":c.enabled}),
                )
                .await?;
            }
        }
        tx.commit().await?;
    }
    Ok(())
}
pub async fn public_config(
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrganisationSsoPublic>, ApiError> {
    let ctx = context(&state, org).await?;
    let name: String = sqlx::query_scalar(
        "SELECT name FROM management_organisations WHERE id=$1 AND archived_at IS NULL",
    )
    .bind(org)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(OrganisationSsoPublic {
        enabled: true,
        organisation_id: org,
        organisation_name: name,
        provider_label: Some(ctx.provider.label.clone()),
    }))
}

// Call only AFTER verifying the signature, issuer, audience, expiry and nonce of
// this exact ID token. CoreClient intentionally discards custom claims.
pub fn verified_group_role(token: &str, ctx: &Context) -> Option<OrganisationRole> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    if token.len() > 262_144 {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    let claims: Value = serde_json::from_slice(&payload).ok()?;
    let groups = claims.get(&ctx.provider.groups_claim)?.as_array()?;
    if groups.len() > 1024
        || groups
            .iter()
            .any(|g| g.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
    {
        return None;
    }
    ctx.configuration
        .group_mappings
        .iter()
        .filter(|m| groups.iter().any(|g| g.as_str() == Some(&m.group)))
        .map(|m| m.role)
        .max_by_key(|r| match r {
            OrganisationRole::Admin => 3,
            OrganisationRole::Member => 2,
            OrganisationRole::Auditor => 1,
        })
}

/// Preflight before creating/linking any identity. A denied existing identity's
/// revocation must commit even though the callback does not issue credentials.
pub async fn admit(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &Context,
    user: Option<Uuid>,
    role: Option<OrganisationRole>,
) -> Result<bool, ApiError> {
    if let Some(user) = user {
        if let Some(row)=sqlx::query("SELECT role,sso_provider_id,directory_managed FROM management_members WHERE organisation_id=$1 AND user_id=$2").bind(ctx.org).bind(user).fetch_optional(&mut **tx).await? {
            if row.get::<bool,_>("directory_managed") {return Ok(row.get::<String,_>("role")!="suspended");}
            if row.get::<Option<String>,_>("sso_provider_id").is_none() {return Ok(row.get::<String,_>("role")!="suspended");}
            if role.is_some() {return Ok(true);}
            if row.get::<String,_>("role")!="suspended" {
                sqlx::query("UPDATE management_members SET role='suspended',content_admin=false WHERE organisation_id=$1 AND user_id=$2").bind(ctx.org).bind(user).execute(&mut **tx).await?;
                append_audit(tx,ctx.org,user,"sso.membership_suspended",user,json!({"provider_id":ctx.provider.id,"revision":ctx.configuration.revision})).await?;
            }
            return Ok(false);
        }
    }
    Ok(ctx.configuration.auto_provision && role.is_some())
}
pub async fn sync_member(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &Context,
    user: Uuid,
    role: Option<OrganisationRole>,
) -> Result<(), ApiError> {
    let Some(role) = role else {
        return Ok(());
    }; // A manual membership was admitted.
    let changed=sqlx::query("INSERT INTO management_members(organisation_id,user_id,role,content_admin,sso_provider_id) VALUES($1,$2,$3,false,$4) ON CONFLICT(organisation_id,user_id) DO UPDATE SET role=$3,content_admin=false,sso_provider_id=$4 WHERE NOT management_members.directory_managed AND management_members.sso_provider_id IS NOT NULL AND (management_members.role<>$3 OR management_members.sso_provider_id<>$4 OR management_members.content_admin)")
        .bind(ctx.org).bind(user).bind(tag(role)?).bind(&ctx.provider.id).execute(&mut **tx).await?.rows_affected();
    if changed > 0 {
        append_audit(tx,ctx.org,user,"sso.membership_sync",user,json!({"role":role,"provider_id":ctx.provider.id,"revision":ctx.configuration.revision})).await?;
    }
    Ok(())
}
