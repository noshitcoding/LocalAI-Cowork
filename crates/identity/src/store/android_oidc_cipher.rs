//! Ciphertext is inventoried only through actual profile/flow/exchange/receipt references.
use super::*;
use cowork_secrets::{
    KeyId, KeyStore, McpSecretScope, SealedMcpBlob, open_mcp_blob, rewrap_mcp_blob, seal_mcp_blob,
};
use zeroize::Zeroizing;
#[expect(
    clippy::too_many_arguments,
    reason = "Keep the transaction, encryption key and original ciphertext scope explicit."
)]
pub(super) async fn put(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    profile: &AndroidOidcProfile,
    record: Uuid,
    purpose: &str,
    value: &[u8],
    at: i64,
) -> Result<Uuid> {
    let scope = McpSecretScope {
        owner_kind: "personal".into(),
        owner_id: profile.id.to_string(),
        project_id: profile
            .organization_id
            .map(|v| v.to_string())
            .unwrap_or_else(|| "oidc-global".into()),
        server_id: profile.id.to_string(),
        record_id: record.to_string(),
        revision: profile.revision,
        epoch: profile.epoch,
        configuration_sha256: profile.config_sha256.clone(),
        purpose: purpose.into(),
    };
    put_scope(tx, keys, key, scope, value, at).await
}
pub(super) async fn put_scope(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    scope: McpSecretScope,
    value: &[u8],
    at: i64,
) -> Result<Uuid> {
    let record = identifier(scope.record_id.clone())?;
    let purpose = scope.purpose.clone();
    let sealed = seal_mcp_blob(keys, key, &scope, value)
        .map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
    if let Some(registry) =
        sqlx::query_scalar::<_, String>("SELECT payload FROM server_key_registry WHERE id=1")
            .fetch_optional(&mut **tx)
            .await?
    {
        let registry: Value =
            serde_json::from_str(&registry).map_err(|_| IdentityError::Corrupt)?;
        if registry["instance_id"] == json!(key.instance)
            && (registry["pending"] != Value::Null || registry["active"] != json!(key))
        {
            return Err(IdentityError::NativeAuditKeyUnavailable);
        }
    }
    if let Some(native) =
        sqlx::query_scalar::<_, String>("SELECT payload FROM native_secret_state WHERE id='local'")
            .fetch_optional(&mut **tx)
            .await?
    {
        let native: Value = serde_json::from_str(&native).map_err(|_| IdentityError::Corrupt)?;
        if native["instance"] == json!(key.instance)
            && (native["pending_operation"] != Value::Null || native["active"]["key"] != json!(key))
        {
            return Err(IdentityError::NativeAuditKeyUnavailable);
        }
    }
    let retired: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM native_key_retirements WHERE cipher_id=$1")
            .bind(key.cipher_id().map_err(|_| IdentityError::Corrupt)?)
            .fetch_one(&mut **tx)
            .await?;
    if retired != 0 {
        return Err(IdentityError::NativeAuditKeyUnavailable);
    }
    sqlx::query("INSERT INTO android_oidc_ciphertext(id,purpose,scope,sealed,plain_sha256,created_at,key_instance) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(record.to_string()).bind(purpose).bind(encoded(&scope)?).bind(encoded(&sealed)?).bind(digest(value)).bind(at).bind(key.instance.to_string()).execute(&mut **tx).await?;
    Ok(record)
}
pub(super) async fn read(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    id: Uuid,
    profile: Uuid,
    purpose: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    let row = sqlx::query("SELECT * FROM android_oidc_ciphertext WHERE id=$1 AND purpose=$2")
        .bind(id.to_string())
        .bind(purpose)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IdentityError::Corrupt)?;
    let scope: McpSecretScope = decoded(&row.try_get::<String, _>("scope")?)?;
    if scope.server_id != profile.to_string()
        || scope.record_id != id.to_string()
        || scope.purpose != purpose
    {
        return Err(IdentityError::Corrupt);
    }
    let sealed: SealedMcpBlob = decoded(&row.try_get::<String, _>("sealed")?)?;
    if sealed.key_id().instance.to_string() != row.try_get::<String, _>("key_instance")? {
        return Err(IdentityError::Corrupt);
    }
    let plain = open_mcp_blob(keys, &scope, &sealed)
        .map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
    if digest(&plain) != row.try_get::<String, _>("plain_sha256")? {
        return Err(IdentityError::Corrupt);
    }
    Ok(plain)
}
const REFERENCES: &str = "SELECT c.id,c.scope,c.sealed,c.key_instance FROM android_oidc_ciphertext c WHERE EXISTS(SELECT 1 FROM android_oidc_profiles p WHERE p.secret_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_flows f WHERE f.payload_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_exchanges e WHERE e.response_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_receipts r WHERE r.payload_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_profile_operations o WHERE o.receipt_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_flow_closures x WHERE x.receipt_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_compat_flows v WHERE v.payload_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_sso_operations s WHERE s.receipt_id=c.id) OR EXISTS(SELECT 1 FROM android_oidc_denials d WHERE d.receipt_id=c.id) ORDER BY c.id LIMIT 100001";
impl IdentityStore {
    pub async fn oidc_cipher_keys_in_transaction(
        tx: &mut Transaction<'_, Any>,
        instance: Uuid,
    ) -> Result<Vec<KeyId>> {
        let rows = sqlx::query(REFERENCES).fetch_all(&mut **tx).await?;
        if rows.len() > 100000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let mut out = Vec::new();
        for row in rows {
            let sealed: SealedMcpBlob = decoded(&row.try_get::<String, _>("sealed")?)?;
            sealed.validate().map_err(|_| IdentityError::Corrupt)?;
            if sealed.key_id().instance.to_string() != row.try_get::<String, _>("key_instance")? {
                return Err(IdentityError::Corrupt);
            }
            if sealed.key_id().instance == instance && !out.contains(&sealed.key_id()) {
                out.push(sealed.key_id())
            }
        }
        Ok(out)
    }
    pub async fn oidc_cipher_rewrap_in_transaction(
        tx: &mut Transaction<'_, Any>,
        keys: &dyn KeyStore,
        target: KeyId,
    ) -> Result<u64> {
        let rows = sqlx::query(REFERENCES).fetch_all(&mut **tx).await?;
        if rows.len() > 100000 {
            return Err(IdentityError::QuotaExceeded);
        }
        let mut n = 0;
        for row in rows {
            let raw: String = row.try_get("sealed")?;
            let scope: McpSecretScope = decoded(&row.try_get::<String, _>("scope")?)?;
            let sealed: SealedMcpBlob = decoded(&raw)?;
            if sealed.key_id().instance.to_string() != row.try_get::<String, _>("key_instance")? {
                return Err(IdentityError::Corrupt);
            }
            if sealed.key_id().instance != target.instance {
                continue;
            }
            let next = rewrap_mcp_blob(keys, &scope, &sealed, target)
                .map_err(|_| IdentityError::NativeAuditKeyUnavailable)?;
            if sealed.key_id() != target {
                if sqlx::query("UPDATE android_oidc_ciphertext SET sealed=$1 WHERE id=$2 AND sealed=$3 AND scope=$4").bind(encoded(&next)?).bind(row.try_get::<String,_>("id")?).bind(raw).bind(encoded(&scope)?).execute(&mut **tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict)}
                n += 1
            }
        }
        Ok(n)
    }
}
