//! Original login method is separate from optional provider MFA evidence.
use super::*;
pub async fn oidc_origin(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    issuer: &str,
    client: &str,
    subject: &str,
    ctx: Option<&sso::Context>,
) -> Result<Value, ApiError> {
    let identity=sqlx::query("SELECT id,directory_owner_connector_id,directory_owner_resource_id FROM oidc_identities WHERE user_id=$1 AND issuer=$2 AND subject=$3 AND binding_method<>'legacy_unverified' FOR SHARE")
        .bind(user).bind(issuer).bind(subject).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    let identity_id: Uuid = identity.get("id");
    let owner: Option<Uuid> = identity.get("directory_owner_connector_id");
    let mut v = json!({"version":1,"kind":"oidc","user_id":user,"identity_id":identity_id,"issuer":issuer,"client_id":client,"subject_sha256":hex::encode(Sha256::digest(subject.as_bytes()))});
    if let Some(ctx) = ctx {
        v["organisation"] = json!({"id":ctx.org,"provider_id":ctx.provider.id,"provider_fingerprint":ctx.provider.fingerprint,"sso_revision":ctx.configuration.revision});
    }
    let directory_managed = if let Some(ctx) = ctx {
        sqlx::query_scalar::<_,bool>("SELECT coalesce((SELECT directory_managed FROM management_members WHERE organisation_id=$1 AND user_id=$2),false)").bind(ctx.org).bind(user).fetch_one(&mut **tx).await?
    } else {
        false
    };
    if owner.is_some() || directory_managed {
        let ctx = ctx.ok_or_else(unavailable)?;
        let candidates=sqlx::query("SELECT r.id,r.connector_id FROM directory_resources r JOIN directory_connectors c ON c.id=r.connector_id WHERE r.user_id=$1 AND r.identity_id=$2 AND c.organisation_id=$3 AND c.provider_id=$4 AND c.issuer=$5 AND c.client_id=$6 AND ($7::UUID IS NULL OR c.id=$7) ORDER BY c.id LIMIT 257")
            .bind(user).bind(identity_id).bind(ctx.org).bind(&ctx.provider.id).bind(issuer).bind(client).bind(owner).fetch_all(&mut **tx).await?;
        if candidates.len() > 256 {
            return Err(unavailable());
        }
        let mut source = None;
        for row in candidates {
            let c = match current(state, tx, row.get("connector_id"), false).await {
                Ok(c) => c,
                Err(ApiError::Unauthorized(_)) => continue,
                Err(e) => return Err(e),
            };
            let resource: Uuid = row.get("id");
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_resources r JOIN directory_membership_contributions x ON x.connector_id=r.connector_id AND x.user_resource_id=r.id AND x.identity_id=r.identity_id WHERE r.id=$1 AND r.connector_id=$2 AND r.kind='User' AND r.active AND NOT r.deleted AND NOT r.quarantined_legacy AND r.user_id=$3 AND r.identity_id=$4 AND r.observed_registry_epoch=$5 AND r.observed_settings_epoch=$6 AND r.observed_settings_revision=$7 AND x.active AND x.registry_epoch=$5 AND x.settings_epoch=$6 AND x.settings_revision=$7 AND directory_effective_role($8,$3) IS NOT NULL)")
                .bind(resource).bind(c.connector.id).bind(user).bind(identity_id).bind(c.registry_epoch).bind(c.settings_epoch).bind(c.settings_revision).bind(ctx.org).fetch_one(&mut **tx).await?;
            if c.state == "active" && valid {
                let mut stamp = c.stamp();
                stamp["resource_id"] = json!(resource);
                source = Some(stamp);
                break;
            }
        }
        v["directory"] = source.ok_or_else(unavailable)?;
    }
    Ok(v)
}
pub async fn validate_origin(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    origin: Option<Value>,
) -> Result<(), ApiError> {
    let Some(v) = origin else {
        let guarded:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_containment_users WHERE user_id=$1) OR EXISTS(SELECT 1 FROM oidc_identities WHERE user_id=$1 AND directory_owner_connector_id IS NOT NULL)").bind(user).fetch_one(&mut **tx).await?;
        return if guarded { Err(unavailable()) } else { Ok(()) };
    };
    if v["version"] != 1 {
        return Err(unavailable());
    }
    match v["kind"].as_str() {
        Some("local" | "instance_launch") => return Ok(()),
        Some("oidc") => {}
        _ => return Err(unavailable()),
    }
    if v["user_id"] != json!(user) {
        return Err(unavailable());
    }
    let identity = v["identity_id"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or_else(unavailable)?;
    let i=sqlx::query("SELECT issuer,subject,directory_owner_connector_id FROM oidc_identities WHERE id=$1 AND user_id=$2 AND binding_method<>'legacy_unverified' FOR SHARE").bind(identity).bind(user).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    if v["issuer"] != i.get::<String, _>("issuer")
        || v["subject_sha256"]
            != hex::encode(Sha256::digest(i.get::<String, _>("subject").as_bytes()))
    {
        return Err(unavailable());
    }
    if let Some(d) = v.get("directory") {
        let id = d["connector_id"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(unavailable)?;
        let c = current(state, tx, id, false).await?;
        let resource = d["resource_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(unavailable)?;
        let mut expected = c.stamp();
        expected["resource_id"] = json!(resource);
        if &expected != d || c.state != "active" || v["client_id"] != c.connector.client {
            return Err(unavailable());
        }
        let good:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM directory_resources r JOIN directory_membership_contributions x ON x.connector_id=r.connector_id AND x.user_resource_id=r.id AND x.identity_id=r.identity_id WHERE r.id=$1 AND r.connector_id=$2 AND r.identity_id=$3 AND r.user_id=$4 AND r.active AND NOT r.deleted AND NOT r.quarantined_legacy AND r.observed_registry_epoch=$5 AND r.observed_settings_epoch=$6 AND r.observed_settings_revision=$7 AND x.active AND x.registry_epoch=$5 AND x.settings_epoch=$6 AND x.settings_revision=$7 AND directory_effective_role($8,$4) IS NOT NULL)")
            .bind(resource).bind(id).bind(identity).bind(user).bind(c.registry_epoch).bind(c.settings_epoch).bind(c.settings_revision).bind(c.connector.org).fetch_one(&mut **tx).await?;
        if !good {
            return Err(unavailable());
        }
    } else if i
        .get::<Option<Uuid>, _>("directory_owner_connector_id")
        .is_some()
    {
        return Err(unavailable());
    }
    Ok(())
}
pub async fn stamp_code(
    tx: &mut Transaction<'_, Postgres>,
    code: &str,
    value: &Value,
) -> Result<(), ApiError> {
    let hash = Sha256::digest(code.as_bytes());
    let n=sqlx::query("UPDATE native_authorization_codes SET auth_origin=$2 WHERE code_hash=$1 AND consumed_at IS NULL AND expires_at>now()").bind(hash.as_slice()).bind(value).execute(&mut **tx).await?.rows_affected();
    if n != 1 {
        return Err(unavailable());
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
    validate_origin(state, tx, user, value.clone()).await?;
    let n=sqlx::query("UPDATE auth_sessions SET auth_origin=$3 WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>now()").bind(session).bind(user).bind(value).execute(&mut **tx).await?.rows_affected();
    if n != 1 {
        return Err(unavailable());
    }
    Ok(())
}
