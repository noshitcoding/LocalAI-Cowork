//! Explicit operator mappings for already verified ID-token claims. This module
//! never decodes an unverified JWT, accepts client evidence, or renews evidence
//! from a refresh token. A persisted policy epoch fences older API replicas.
use crate::{auth, error::ApiError, management::sso::Context, oidc::OidcService, AppState};
use chrono::{DateTime, Duration, Utc};
use openidconnect::core::CoreIdTokenClaims;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{collections::HashSet, io::Read, path::Path};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub issuer: String,
    pub client_id: String,
    pub max_age_seconds: u32,
    pub mappings: Vec<Mapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub id: String,
    pub acr: Option<String>,
    pub amr: Vec<String>,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
fn claim_string(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}
fn valid_scope(scope: &str) -> bool {
    scope == "global" || scope.strip_prefix("organisation:").is_some_and(identifier)
}

impl Policy {
    pub fn validate(mut self, issuer: &str, client_id: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.version == 1,
            "unsupported OIDC assurance policy version"
        );
        anyhow::ensure!(
            self.issuer == issuer && self.client_id == client_id,
            "OIDC assurance policy must match the exact configured issuer and client_id"
        );
        let url = crate::oidc::validated_https_or_loopback_url(&self.issuer, "assurance issuer")?;
        anyhow::ensure!(
            url.query().is_none()
                && url.fragment().is_none()
                && self.issuer == self.issuer.trim()
                && !client_id.is_empty()
                && client_id.len() <= 512,
            "invalid OIDC assurance provider identity"
        );
        anyhow::ensure!(
            (1..=43_200).contains(&self.max_age_seconds),
            "OIDC assurance max_age_seconds must be between 1 and 43200"
        );
        anyhow::ensure!(
            !self.mappings.is_empty() && self.mappings.len() <= 64,
            "configure between 1 and 64 explicit OIDC assurance mappings"
        );
        let mut ids = HashSet::new();
        let mut tuples = HashSet::new();
        for mapping in &mut self.mappings {
            anyhow::ensure!(
                identifier(&mapping.id) && ids.insert(mapping.id.clone()),
                "invalid or duplicate OIDC assurance mapping id"
            );
            anyhow::ensure!(
                mapping.acr.as_deref().is_none_or(claim_string)
                    && mapping.acr.as_deref() != Some("0")
                    && mapping.amr.len() <= 32
                    && mapping.amr.iter().all(|v| claim_string(v))
                    && (mapping.acr.is_some() || !mapping.amr.is_empty()),
                "each OIDC assurance mapping needs an explicit nonzero acr or amr tuple"
            );
            mapping.amr.sort();
            anyhow::ensure!(
                !mapping.amr.windows(2).any(|w| w[0] == w[1]),
                "duplicate amr value in OIDC assurance mapping"
            );
            anyhow::ensure!(
                tuples.insert((mapping.acr.clone(), mapping.amr.clone())),
                "duplicate OIDC assurance claim tuple"
            );
        }
        self.mappings.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(self)
    }

    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("policy is serializable"),
        ))
    }

    fn match_claims(
        &self,
        claims: &CoreIdTokenClaims,
        now: DateTime<Utc>,
    ) -> Option<(String, DateTime<Utc>, DateTime<Utc>)> {
        // These are typed claims from the caller's cryptographically verified
        // ID token. Missing claims, an unknown tuple, or future/stale times do
        // not prevent ordinary login; they simply provide no second factor.
        if claims.issuer().as_str() != self.issuer
            || !claims
                .audiences()
                .iter()
                .any(|a| a.as_str() == self.client_id)
        {
            return None;
        }
        let authenticated_at = claims.auth_time()?;
        if authenticated_at > now
            || claims.issue_time() < authenticated_at
            || claims.issue_time() > now
            || claims.expiration() <= now
        {
            return None;
        }
        let valid_until = (authenticated_at + Duration::seconds(self.max_age_seconds.into()))
            .min(claims.expiration());
        if valid_until <= now {
            return None;
        }
        let acr = claims.auth_context_ref().map(|v| v.as_str());
        if acr.is_some_and(|v| !claim_string(v)) {
            return None;
        }
        let mut amr: Vec<String> = claims
            .auth_method_refs()
            .map(|values| values.iter().map(|v| v.as_str().to_owned()).collect())
            .unwrap_or_default();
        if amr.len() > 32 || amr.iter().any(|v| !claim_string(v)) {
            return None;
        }
        amr.sort();
        if amr.windows(2).any(|w| w[0] == w[1]) {
            return None;
        }
        // Exact, case-sensitive tuples: AND within a tuple; OR between mappings.
        // Extra unconfigured amr values or an unconfigured acr do not match.
        let mapping = self
            .mappings
            .iter()
            .find(|m| m.acr.as_deref() == acr && m.amr == amr)?;
        Some((mapping.id.clone(), authenticated_at, valid_until))
    }
}

pub fn load_policy_file(path: &Path) -> anyhow::Result<Policy> {
    let original = std::fs::symlink_metadata(path)?;
    let canonical = std::fs::canonicalize(path)?;
    if original.file_type().is_symlink() {
        // Kubernetes read-only Secret projections use local symlinks. Only a
        // target inside the explicitly configured directory is accepted.
        let parent =
            std::fs::canonicalize(path.parent().ok_or_else(|| {
                anyhow::anyhow!("OIDC assurance policy needs a parent directory")
            })?)?;
        anyhow::ensure!(
            canonical.starts_with(&parent),
            "OIDC assurance policy link leaves its operator directory"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                std::fs::metadata(parent)?.mode() & 0o022 == 0,
                "OIDC assurance policy directory is writable by another account"
            );
        }
    }
    let metadata = std::fs::symlink_metadata(&canonical)?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "OIDC assurance policy must resolve to a regular operator-owned file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1 && metadata.mode() & 0o022 == 0,
            "OIDC assurance policy must have one link and no group/other write permission"
        );
    }
    let mut file = std::fs::File::open(canonical)?;
    let held = file.metadata()?;
    anyhow::ensure!(
        held.is_file() && held.len() <= 131_072,
        "OIDC assurance policy is not a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            held.dev() == metadata.dev()
                && held.ino() == metadata.ino()
                && held.nlink() == 1
                && held.mode() & 0o022 == 0,
            "OIDC assurance policy changed while opening or is writable by another account"
        );
    }
    let mut bytes = Vec::new();
    file.by_ref().take(131_073).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 131_072, "OIDC assurance policy is too large");
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganisationEvidence {
    pub id: Uuid,
    pub provider_id: String,
    pub provider_fingerprint: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub version: u32,
    pub user_id: Uuid,
    pub identity_id: Uuid,
    pub scope: String,
    pub issuer: String,
    pub client_id: String,
    pub policy_fingerprint: String,
    pub policy_revision: i64,
    pub policy_epoch: Uuid,
    pub mapping_id: String,
    pub authenticated_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub organisation: Option<OrganisationEvidence>,
}

/// Only called after the exact ID token's signature, issuer, audience, azp,
/// expiry, nonce and optional at_hash have been checked by oidc::callback.
pub async fn from_verified_claims(
    tx: &mut Transaction<'_, Postgres>,
    service: &OidcService,
    ctx: Option<&Context>,
    user: Uuid,
    claims: &CoreIdTokenClaims,
) -> Result<Option<Evidence>, ApiError> {
    let Some(policy) = service.assurance_policy() else {
        return Ok(None);
    };
    let Some((mapping_id, authenticated_at, valid_until)) = policy.match_claims(claims, Utc::now())
    else {
        return Ok(None);
    };
    let scope = ctx.map_or_else(
        || "global".to_owned(),
        |c| format!("organisation:{}", c.provider.id),
    );
    let fingerprint = policy.fingerprint();
    let slot = sqlx::query("SELECT revision,epoch FROM oidc_assurance_policies WHERE scope=$1 AND issuer=$2 AND client_id=$3 AND fingerprint=$4 AND active FOR SHARE")
        .bind(&scope).bind(&policy.issuer).bind(&policy.client_id).bind(&fingerprint)
        .fetch_optional(&mut **tx).await?;
    let Some(slot) = slot else {
        return Ok(None);
    };
    let identity: Option<Uuid> = sqlx::query_scalar("SELECT id FROM oidc_identities WHERE user_id=$1 AND issuer=$2 AND subject=$3 AND binding_method<>'legacy_unverified' FOR SHARE")
        .bind(user).bind(&policy.issuer).bind(claims.subject().as_str()).fetch_optional(&mut **tx).await?;
    let Some(identity_id) = identity else {
        return Ok(None);
    };
    Ok(Some(Evidence {
        version: 1,
        user_id: user,
        identity_id,
        scope,
        issuer: policy.issuer.clone(),
        client_id: policy.client_id.clone(),
        policy_fingerprint: fingerprint,
        policy_revision: slot.get("revision"),
        policy_epoch: slot.get("epoch"),
        mapping_id,
        authenticated_at,
        valid_until,
        organisation: ctx.map(|c| OrganisationEvidence {
            id: c.org,
            provider_id: c.provider.id.clone(),
            provider_fingerprint: c.provider.fingerprint.clone(),
            revision: c.configuration.revision,
        }),
    }))
}

fn current_policy<'a>(state: &'a AppState, evidence: &Evidence) -> Option<&'a Policy> {
    match &evidence.organisation {
        None if evidence.scope == "global" => state.oidc.as_ref()?.assurance_policy(),
        Some(org) if evidence.scope == format!("organisation:{}", org.provider_id) => {
            let provider = state.organisation_oidc.get(&org.provider_id)?;
            if provider.fingerprint != org.provider_fingerprint {
                return None;
            }
            provider.assurance_policy()
        }
        _ => None,
    }
}

pub async fn valid_in_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    evidence: &Evidence,
) -> Result<bool, ApiError> {
    let Some(policy) = current_policy(state, evidence) else {
        return Ok(false);
    };
    let now = Utc::now();
    if evidence.version != 1
        || evidence.user_id != user
        || evidence.authenticated_at > now
        || evidence.valid_until <= now
        || evidence.valid_until <= evidence.authenticated_at
        || evidence.valid_until
            > evidence.authenticated_at + Duration::seconds(policy.max_age_seconds.into())
        || evidence.issuer != policy.issuer
        || evidence.client_id != policy.client_id
        || evidence.policy_fingerprint != policy.fingerprint()
        || !policy.mappings.iter().any(|m| m.id == evidence.mapping_id)
    {
        return Ok(false);
    }
    let slot = sqlx::query("SELECT revision,epoch,active,fingerprint,issuer,client_id FROM oidc_assurance_policies WHERE scope=$1 FOR SHARE")
        .bind(&evidence.scope).fetch_optional(&mut **tx).await?;
    let Some(slot) = slot else {
        return Ok(false);
    };
    if !slot.get::<bool, _>("active")
        || slot.get::<i64, _>("revision") != evidence.policy_revision
        || slot.get::<Uuid, _>("epoch") != evidence.policy_epoch
        || slot.get::<String, _>("fingerprint") != evidence.policy_fingerprint
        || slot.get::<String, _>("issuer") != evidence.issuer
        || slot.get::<String, _>("client_id") != evidence.client_id
    {
        return Ok(false);
    }
    let identity: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_identities WHERE id=$1 AND user_id=$2 AND issuer=$3 AND binding_method<>'legacy_unverified')")
        .bind(evidence.identity_id).bind(user).bind(&evidence.issuer).fetch_one(&mut **tx).await?;
    if !identity {
        return Ok(false);
    }
    if let Some(org) = &evidence.organisation {
        let configured: Option<bool> = sqlx::query_scalar("SELECT true FROM management_sso s JOIN management_members m ON m.organisation_id=s.organisation_id WHERE s.organisation_id=$1 AND s.enabled AND s.revision=$2 AND s.provider_id=$3 AND s.provider_fingerprint=$4 AND m.user_id=$5 AND m.role<>'suspended' AND (m.sso_provider_id IS NULL OR m.sso_provider_id=$3) FOR SHARE OF s,m")
            .bind(org.id).bind(org.revision).bind(&org.provider_id).bind(&org.provider_fingerprint)
            .bind(user).fetch_optional(&mut **tx).await?;
        if configured != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn evidence(value: Option<Value>) -> Option<Evidence> {
    serde_json::from_value(value?).ok()
}

pub async fn session_evidence(
    state: &AppState,
    user: Uuid,
    value: Option<Value>,
) -> Result<Option<Evidence>, ApiError> {
    let Some(evidence) = evidence(value) else {
        return Ok(None);
    };
    let mut tx = state.pool.begin().await?;
    let valid = valid_in_tx(state, &mut tx, user, &evidence).await?;
    tx.commit().await?;
    Ok(valid.then_some(evidence))
}

pub async fn stamp_native(
    tx: &mut Transaction<'_, Postgres>,
    code: &str,
    proof: &Evidence,
) -> Result<(), ApiError> {
    let hash = auth::opaque_token_hash(code);
    let changed = sqlx::query("UPDATE native_authorization_codes SET oidc_assurance=$2 WHERE code_hash=$1 AND user_id=$3 AND consumed_at IS NULL AND expires_at>now() AND oidc_assurance IS NULL AND second_factor_verified_at IS NULL")
        .bind(hash.as_slice()).bind(serde_json::to_value(proof)?).bind(proof.user_id)
        .execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "OIDC assurance code is unavailable".into(),
        ));
    }
    Ok(())
}

pub async fn stamp_session(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    session: Uuid,
    value: Option<Value>,
) -> Result<(), ApiError> {
    let Some(proof) = evidence(value) else {
        return Ok(());
    };
    if !valid_in_tx(state, tx, user, &proof).await? {
        return Ok(());
    }
    let changed = sqlx::query("UPDATE auth_sessions SET oidc_assurance=$3 WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>now() AND oidc_assurance IS NULL AND second_factor_verified_at IS NULL")
        .bind(session).bind(user).bind(serde_json::to_value(proof)?).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "OIDC assurance session is unavailable".into(),
        ));
    }
    Ok(())
}

pub async fn register(state: &AppState) -> anyhow::Result<()> {
    let mut policies = Vec::new();
    if let Some(policy) = state.oidc.as_ref().and_then(|s| s.assurance_policy()) {
        policies.push(("global".to_owned(), policy));
    }
    for provider in state.organisation_oidc.values() {
        if let Some(policy) = provider.assurance_policy() {
            policies.push((format!("organisation:{}", provider.id), policy));
        }
    }
    for (scope, policy) in policies {
        // A restart must not reverse an explicit disable or replace a changed
        // policy. None on another API replica is never a revocation request.
        sqlx::query("INSERT INTO oidc_assurance_policies(scope,issuer,client_id,fingerprint,revision,epoch,active) VALUES($1,$2,$3,$4,1,$5,true) ON CONFLICT(scope) DO NOTHING")
            .bind(&scope).bind(&policy.issuer).bind(&policy.client_id).bind(policy.fingerprint())
            .bind(Uuid::new_v4()).execute(&state.pool).await?;
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_assurance_policies WHERE scope=$1 AND active AND issuer=$2 AND client_id=$3 AND fingerprint=$4)")
            .bind(&scope).bind(&policy.issuer).bind(&policy.client_id).bind(policy.fingerprint())
            .fetch_one(&state.pool).await?;
        if !current {
            tracing::warn!(scope, "OIDC assurance policy is disabled or differs from this replica; ordinary login remains available; use local oidc-assurance inspect and explicit activate after reviewing the current revision");
        }
    }
    Ok(())
}

pub async fn global_available(state: &AppState) -> Result<bool, ApiError> {
    // A management login must use an administrator-enabled organisation provider.
    // Do not advertise a legacy global reauthentication route that is disabled.
    if state.management_mode {
        return Ok(false);
    }
    let Some(policy) = state.oidc.as_ref().and_then(|s| s.assurance_policy()) else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_assurance_policies WHERE scope='global' AND active AND issuer=$1 AND client_id=$2 AND fingerprint=$3)")
        .bind(&policy.issuer).bind(&policy.client_id).bind(policy.fingerprint()).fetch_one(&state.pool).await?)
}

pub async fn reauthentication(
    state: &AppState,
    user: Uuid,
    original: Option<Value>,
) -> Result<(bool, Option<Uuid>), ApiError> {
    if let Some(proof) = evidence(original).filter(|e| e.user_id == user) {
        if let Some(org) = proof.organisation {
            if let Ok(ctx) = crate::management::sso::context(state, org.id).await {
                if let Some(policy) = ctx.provider.assurance_policy() {
                    let scope = format!("organisation:{}", ctx.provider.id);
                    let available: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_assurance_policies p JOIN management_members m ON m.organisation_id=$1 WHERE p.scope=$2 AND p.active AND p.issuer=$3 AND p.client_id=$4 AND p.fingerprint=$5 AND m.user_id=$6 AND m.role<>'suspended')")
                        .bind(org.id).bind(scope).bind(&policy.issuer).bind(&policy.client_id)
                        .bind(policy.fingerprint()).bind(user).fetch_one(&state.pool).await?;
                    if available {
                        return Ok((true, Some(org.id)));
                    }
                }
            }
        }
    }
    Ok((global_available(state).await?, None))
}

fn checked_scope(scope: &str) -> Result<(), ApiError> {
    if !valid_scope(scope) {
        return Err(ApiError::Unprocessable(
            "use global or organisation:<provider-id> as policy scope".into(),
        ));
    }
    Ok(())
}

pub async fn inspect(pool: &PgPool, scope: &str) -> Result<Value, ApiError> {
    checked_scope(scope)?;
    Ok(
        sqlx::query_scalar("SELECT to_jsonb(p) FROM oidc_assurance_policies p WHERE scope=$1")
            .bind(scope)
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| json!({"scope":scope,"revision":0,"active":false})),
    )
}

async fn change(
    pool: &PgPool,
    scope: &str,
    expected: i64,
    policy: Option<Policy>,
) -> Result<Value, ApiError> {
    checked_scope(scope)?;
    if expected < 1 {
        return Err(ApiError::Unprocessable(
            "expected revision must be positive; register the configured policy first".into(),
        ));
    }
    let policy = policy
        .map(|p| {
            let issuer = p.issuer.clone();
            let client = p.client_id.clone();
            p.validate(&issuer, &client)
        })
        .transpose()?;
    let action = if policy.is_some() {
        "activate"
    } else {
        "disable"
    };
    let request_sha = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
        1,
        scope,
        expected,
        action,
        policy.as_ref().map(Policy::fingerprint)
    ]))?));
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT issuer,client_id,fingerprint,revision FROM oidc_assurance_policies WHERE scope=$1 FOR UPDATE")
        .bind(scope).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError::NotFound("configured assurance policy has not been registered".into()))?;
    if let Some(old) = sqlx::query("SELECT request_sha256,receipt FROM oidc_assurance_policy_operations WHERE scope=$1 AND expected_revision=$2")
        .bind(scope).bind(expected).fetch_optional(&mut *tx).await?
    {
        if old.get::<String,_>("request_sha256") != request_sha {
            return Err(ApiError::Conflict("the original revision was consumed by another policy operation".into()));
        }
        let receipt = old.get("receipt"); tx.commit().await?; return Ok(receipt);
    }
    if row.get::<i64, _>("revision") != expected {
        return Err(ApiError::Conflict(
            "inspect the current assurance policy revision before changing it".into(),
        ));
    }
    let revision = expected
        .checked_add(1)
        .ok_or_else(|| ApiError::Conflict("policy revision overflow".into()))?;
    let issuer = policy
        .as_ref()
        .map(|p| p.issuer.clone())
        .unwrap_or_else(|| row.get("issuer"));
    let client = policy
        .as_ref()
        .map(|p| p.client_id.clone())
        .unwrap_or_else(|| row.get("client_id"));
    let fingerprint = policy
        .as_ref()
        .map(Policy::fingerprint)
        .unwrap_or_else(|| row.get("fingerprint"));
    let epoch = Uuid::new_v4();
    let operation = Uuid::new_v4();
    sqlx::query("UPDATE oidc_assurance_policies SET issuer=$2,client_id=$3,fingerprint=$4,revision=$5,epoch=$6,active=$7,updated_at=now() WHERE scope=$1 AND revision=$8")
        .bind(scope).bind(&issuer).bind(&client).bind(&fingerprint).bind(revision).bind(epoch)
        .bind(policy.is_some()).bind(expected).execute(&mut *tx).await?;
    let receipt = json!({"version":1,"operation_id":operation,"scope":scope,
        "action":action,"expected_revision":expected,"revision":revision,"epoch":epoch,
        "issuer":issuer,"client_id":client,"fingerprint":fingerprint,"active":policy.is_some(),
        "request_sha256":request_sha});
    sqlx::query("INSERT INTO oidc_assurance_policy_operations(id,scope,expected_revision,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)")
        .bind(operation).bind(scope).bind(expected).bind(&request_sha).bind(&receipt).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_events(id,action,target_type,target_id,metadata) VALUES($1,$2,'oidc_assurance_policy',$1,$3)")
        .bind(operation).bind(format!("auth.oidc.assurance.{action}")).bind(&receipt).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(receipt)
}

/// Local operator CLI only; never routed through a tenant or HTTP API.
pub async fn run_cli(pool: &PgPool, args: &[String]) -> anyhow::Result<()> {
    let result = match args {
        [command, scope] if command == "inspect" => inspect(pool, scope).await?,
        [command, scope, revision] if command == "disable" => change(pool, scope, revision.parse()?, None).await?,
        [command, scope, file, revision] if command == "activate" =>
            change(pool, scope, revision.parse()?, Some(load_policy_file(Path::new(file))?)).await?,
        _ => anyhow::bail!("usage: cowork-server oidc-assurance inspect <global|organisation:provider-id> | disable <scope> <expected-revision> | activate <scope> <policy-file> <expected-revision>"),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
