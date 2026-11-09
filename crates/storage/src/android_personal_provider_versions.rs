//! Exact accepted actor model revisions. Current deletion remains explicit revocation.
use super::*;

pub(crate) async fn persist(tx: &mut Transaction<'_, Any>, owner: Uuid, id: Uuid) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM android_personal_provider_versions WHERE owner_id=$1",
    )
    .bind(owner.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if count >= 100_000 {
        return Err(Error::BudgetExceeded);
    }
    sqlx::query("INSERT INTO android_personal_provider_versions(profile_id,owner_id,revision,payload,scope,sealed,revoked) SELECT p.id,p.owner_id,p.revision,p.payload,s.scope,s.sealed,p.deleted FROM android_personal_provider_profiles p LEFT JOIN android_personal_provider_secrets s ON s.profile_id=p.id AND s.profile_revision=p.revision WHERE p.id=$1 AND p.owner_id=$2 ON CONFLICT(profile_id,revision) DO NOTHING")
        .bind(id.to_string()).bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}

pub(crate) async fn frozen(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    bound: &Value,
) -> Result<Value> {
    let id = crate::android_personal::wire_uuid(bound, "id")?;
    let revision = bound["revision"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or(Error::Forbidden)?;
    let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_personal_provider_profiles WHERE id=$1 AND owner_id=$2 AND deleted=0").bind(id.to_string()).bind(owner.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if active != 1 {
        return Err(Error::Forbidden);
    }
    let raw:Option<String>=sqlx::query_scalar("SELECT payload FROM android_personal_provider_versions WHERE profile_id=$1 AND owner_id=$2 AND revision=$3 AND revoked=0").bind(id.to_string()).bind(owner.to_string()).bind(revision).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let profile: Value = parse(&raw.ok_or(Error::Forbidden)?)?;
    if binding(&profile)? != *bound || profile["owner_user_id"] != json!(owner) {
        return Err(Error::Forbidden);
    }
    Ok(profile)
}

pub(crate) async fn credential(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    profile: &Value,
    keys: &dyn KeyStore,
) -> Result<Option<Zeroizing<String>>> {
    let profile = frozen(tx, owner, &binding(profile)?).await?;
    if profile["model_defaults"]["endpoint_binding"] == "per_device" {
        return Err(Error::Forbidden);
    }
    let expected = credential_scope(&profile)?;
    let stored:Option<(Option<String>,Option<String>)>=sqlx::query_as("SELECT scope,sealed FROM android_personal_provider_versions WHERE profile_id=$1 AND owner_id=$2 AND revision=$3 AND revoked=0").bind(&expected.server_id).bind(owner.to_string()).bind(expected.revision).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let (scope, sealed) = stored.ok_or(Error::Forbidden)?;
    if profile["model_defaults"]["auth_mode"] == "none" {
        return Ok(None);
    }
    if profile["has_secret"] != true
        || parse::<CredentialScope>(&scope.ok_or(Error::Forbidden)?)? != expected
    {
        return Err(Error::Forbidden);
    }
    let secret = open_bearer(
        keys,
        &expected,
        &parse::<SealedCredential>(&sealed.ok_or(Error::Forbidden)?)?,
    )
    .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(Some(secret))
}

pub(crate) async fn revoke(tx: &mut Transaction<'_, Any>, owner: Uuid, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE android_personal_provider_versions SET revoked=1,scope=NULL,sealed=NULL WHERE profile_id=$1 AND owner_id=$2").bind(id.to_string()).bind(owner.to_string()).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
