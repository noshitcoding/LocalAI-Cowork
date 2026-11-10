use super::*;
use crate::library_config::LibraryOwner;
use cowork_contracts::{
    DeleteExternalMemoryProvider, ExternalMemoryDefinition, ExternalMemoryProvider,
    ExternalMemoryRetrieval, RetrieveExternalMemory, WriteExternalMemoryProvider,
};
use cowork_identity::ContentAction;
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use sqlx::{Any, Transaction};
use zeroize::Zeroizing;
async fn permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    owner: LibraryOwner<'_>,
    action: ContentAction,
) -> Result<()> {
    if let LibraryOwner::Actor(a) = owner {
        store
            .identity()
            .require_global_session_in_transaction(tx, a)
            .await
            .map_err(team_service::identity_error)?;
        store
            .identity()
            .require_session_action_in_transaction(tx, a, action)
            .await
            .map_err(team_service::identity_error)?;
    }
    Ok(())
}
pub fn validate(value: &ExternalMemoryDefinition) -> Result<()> {
    valid_text(&value.name, 200)?;
    valid_text(&value.endpoint, 4000)?;
    valid_text(&value.content_field, 100)?;
    if !matches!(value.method.as_str(), "GET" | "POST")
        || !matches!(value.auth_scheme.as_str(), "none" | "Bearer" | "Token")
        || value.method == "GET" && value.request_body.is_some()
        || value.items_pointer.len() > 1000
        || !value.items_pointer.is_empty() && !value.items_pointer.starts_with('/')
        || value
            .id_field
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 100)
    {
        return Err(Error::Validation("Invalid external memory protocol".into()));
    }
    let url = reqwest::Url::parse(&value.endpoint)
        .map_err(|_| Error::Validation("Invalid memory endpoint".into()))?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query_pairs().any(|(k, _)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "apikey"
                    | "key"
                    | "token"
                    | "access_token"
                    | "authorization"
                    | "secret"
                    | "password"
            )
        })
        || url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
                ))
    {
        return Err(Error::Validation(
            "Memory endpoint requires HTTPS or loopback HTTP".into(),
        ));
    }
    fn secret(v: &Value) -> bool {
        match v {
            Value::Object(m) => m.iter().any(|(k, v)| {
                matches!(
                    k.to_ascii_lowercase().as_str(),
                    "api_key"
                        | "apikey"
                        | "access_token"
                        | "accesstoken"
                        | "refresh_token"
                        | "refreshtoken"
                        | "credential"
                        | "token"
                        | "password"
                        | "authorization"
                        | "secret"
                ) || secret(v)
            }),
            Value::Array(a) => a.iter().any(secret),
            _ => false,
        }
    }
    if value
        .request_body
        .as_ref()
        .is_some_and(|v| secret(v) || serde_json::to_vec(v).map_or(true, |v| v.len() > 16000))
    {
        return Err(Error::Validation(
            "Memory request body must be bounded and contain no credentials".into(),
        ));
    }
    Ok(())
}
fn scope(owner: LibraryOwner<'_>, id: &str, revision: i64, endpoint: &str) -> CredentialScope {
    CredentialScope {
        project_id: owner.namespace(),
        server_id: id.into(),
        reference: "external-memory/v1".into(),
        revision,
        endpoint: endpoint.into(),
    }
}
async fn quota(tx: &mut Transaction<'_, Any>, owner: LibraryOwner<'_>, table: &str) -> Result<()> {
    let sql = match table {
        "operations" => "SELECT COUNT(*) FROM external_memory_operations WHERE scope=$1",
        "retrievals" => "SELECT COUNT(*) FROM external_memory_retrievals WHERE scope=$1",
        _ => return Err(Error::Forbidden),
    };
    let count: i64 = sqlx::query_scalar(sql)
        .bind(owner.namespace())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if count >= 100000 {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
pub(crate) async fn rewrap(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    let rows=sqlx::query("SELECT scope,id,revision,payload,secret_scope,sealed FROM external_memory_providers WHERE server_managed=1 AND sealed IS NOT NULL ORDER BY scope,id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100000 {
        return Err(Error::BudgetExceeded);
    }
    let mut count = 0u64;
    for row in rows {
        let original: String = row.try_get("sealed").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let namespace: String = row.try_get("scope").map_err(storage_error)?;
        let id: String = row.try_get("id").map_err(storage_error)?;
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        let provider: ExternalMemoryProvider =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let actual: CredentialScope = parse(
            &row.try_get::<String, _>("secret_scope")
                .map_err(storage_error)?,
        )?;
        let expected = CredentialScope {
            project_id: namespace.clone(),
            server_id: id.clone(),
            revision,
            reference: "external-memory/v1".into(),
            endpoint: provider.value.endpoint,
        };
        if actual != expected || provider.id != id || provider.revision != revision {
            return Err(Error::Forbidden);
        }
        let plain = open_bearer(keys, &actual, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let next =
            seal_bearer(keys, target, &actual, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        if sqlx::query("UPDATE external_memory_providers SET sealed=$1 WHERE scope=$2 AND id=$3 AND revision=$4 AND sealed=$5").bind(json_string(&next)?).bind(namespace).bind(id).bind(revision).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        count += 1;
    }
    Ok(count)
}
impl Store {
    pub async fn external_memory_providers(
        &self,
        owner: LibraryOwner<'_>,
    ) -> Result<Vec<ExternalMemoryProvider>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT payload FROM external_memory_providers WHERE scope=$1 AND deleted=0 ORDER BY id LIMIT 101").bind(owner.namespace()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 100 {
            return Err(Error::BudgetExceeded);
        }
        let result = rows.iter().map(|v| parse(v)).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn write_external_memory_provider(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        input: &WriteExternalMemoryProvider,
        keys: &dyn KeyStore,
        key: KeyId,
        server: bool,
    ) -> Result<ExternalMemoryProvider> {
        validate(&input.value)?;
        Uuid::parse_str(id).map_err(|_| Error::Validation("Invalid memory provider ID".into()))?;
        if input.request_id.is_nil()
            || input.expected_revision < 0
            || input.remove_key && input.api_key.is_some()
        {
            return Err(Error::Conflict);
        }
        let fingerprint = keys
            .load(
                &keys
                    .key_for_purpose(&key, cowork_secrets::KeyPurpose::RequestFingerprint)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(serde_json::to_vec(&(id, input)).map_err(storage_error)?);
        let request_sha = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, fingerprint.as_ref()),
            &bytes,
        )
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        if let Some((sha,receipt))=sqlx::query_as::<_,(String,String)>("SELECT request_sha,receipt FROM external_memory_operations WHERE scope=$1 AND request_id=$2").bind(owner.namespace()).bind(input.request_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if sha!=request_sha{return Err(Error::Conflict);}let result=parse(&receipt)?;tx.commit().await.map_err(storage_error)?;return Ok(result);}
        quota(&mut tx, owner, "operations").await?;
        let tombstone: Option<i64> = sqlx::query_scalar(
            "SELECT deleted FROM external_memory_providers WHERE scope=$1 AND id=$2",
        )
        .bind(owner.namespace())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        if tombstone == Some(1) {
            return Err(Error::Conflict);
        }
        let old=sqlx::query("SELECT revision,payload,secret_scope,sealed FROM external_memory_providers WHERE scope=$1 AND id=$2 AND deleted=0").bind(owner.namespace()).bind(id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0)
            != input.expected_revision
        {
            return Err(Error::Conflict);
        }
        let secret = if input.remove_key {
            None
        } else if let Some(secret) = &input.api_key {
            cowork_secrets::validate_bearer(secret)
                .map_err(|_| Error::Validation("Invalid memory credential".into()))?;
            Some(Zeroizing::new(secret.clone()))
        } else if let Some(old) = &old {
            match (
                old.try_get::<Option<String>, _>("secret_scope")
                    .map_err(storage_error)?,
                old.try_get::<Option<String>, _>("sealed")
                    .map_err(storage_error)?,
            ) {
                (Some(bound), Some(sealed)) => {
                    let prior: ExternalMemoryProvider =
                        parse(&old.try_get::<String, _>("payload").map_err(storage_error)?)?;
                    let expected = scope(owner, id, input.expected_revision, &prior.value.endpoint);
                    let actual: CredentialScope = parse(&bound)?;
                    if actual != expected {
                        return Err(Error::Forbidden);
                    }
                    Some(
                        open_bearer(keys, &actual, &parse::<SealedCredential>(&sealed)?)
                            .map_err(|_| Error::NativeKeyUnavailable)?,
                    )
                }
                (None, None) => None,
                _ => return Err(Error::Forbidden),
            }
        } else {
            None
        };
        if input.value.auth_scheme != "none" && secret.is_none() {
            return Err(Error::Validation("Memory credential required".into()));
        }
        let revision = input
            .expected_revision
            .checked_add(1)
            .ok_or(Error::BudgetExceeded)?;
        let bound = scope(owner, id, revision, &input.value.endpoint);
        let sealed = secret
            .as_ref()
            .map(|s| seal_bearer(keys, key, &bound, s).map_err(|_| Error::NativeKeyUnavailable))
            .transpose()?;
        if server && sealed.is_some() {
            crate::server_keys::cipher_writer(&mut tx, key).await?;
        }
        let result = ExternalMemoryProvider {
            id: id.into(),
            revision,
            value: input.value.clone(),
            has_secret: sealed.is_some(),
        };
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM external_memory_providers WHERE scope=$1 AND deleted=0",
        )
        .bind(owner.namespace())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 100 && old.is_none() {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO external_memory_providers(scope,id,revision,payload,secret_scope,sealed,server_managed,deleted) VALUES($1,$2,$3,$4,$5,$6,$7,0) ON CONFLICT(scope,id) DO UPDATE SET revision=excluded.revision,payload=excluded.payload,secret_scope=excluded.secret_scope,sealed=excluded.sealed,server_managed=excluded.server_managed,deleted=0").bind(owner.namespace()).bind(id).bind(revision).bind(json_string(&result)?).bind(sealed.as_ref().map(|_|json_string(&bound)).transpose()?).bind(sealed.as_ref().map(json_string).transpose()?).bind(i64::from(server)).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("INSERT INTO external_memory_operations(scope,request_id,request_sha,receipt) VALUES($1,$2,$3,$4)").bind(owner.namespace()).bind(input.request_id.to_string()).bind(request_sha).bind(json_string(&result)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn delete_external_memory_provider(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        input: &DeleteExternalMemoryProvider,
    ) -> Result<Value> {
        if input.request_id.is_nil() || input.expected_revision < 1 {
            return Err(Error::Conflict);
        }
        let sha = canonical_hash(&("delete-external-memory/1", id, input));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        if let Some((previous,receipt))=sqlx::query_as::<_,(String,String)>("SELECT request_sha,receipt FROM external_memory_operations WHERE scope=$1 AND request_id=$2").bind(owner.namespace()).bind(input.request_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)? {if previous!=sha{return Err(Error::Conflict);}return parse(&receipt);}
        quota(&mut tx, owner, "operations").await?;
        let revision = input
            .expected_revision
            .checked_add(1)
            .ok_or(Error::BudgetExceeded)?;
        let n=sqlx::query("UPDATE external_memory_providers SET deleted=1,revision=$4,sealed=NULL,secret_scope=NULL WHERE scope=$1 AND id=$2 AND revision=$3 AND deleted=0").bind(owner.namespace()).bind(id).bind(input.expected_revision).bind(revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if n != 1 {
            return Err(Error::Conflict);
        }
        let receipt = json!({"deleted":true,"revision":revision});
        sqlx::query("INSERT INTO external_memory_operations(scope,request_id,request_sha,receipt) VALUES($1,$2,$3,$4)").bind(owner.namespace()).bind(input.request_id.to_string()).bind(sha).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn external_memory_retrieval_admit(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        input: &RetrieveExternalMemory,
        keys: &dyn KeyStore,
    ) -> Result<
        std::result::Result<
            (ExternalMemoryProvider, Option<Zeroizing<String>>),
            ExternalMemoryRetrieval,
        >,
    > {
        if input.request_id.is_nil() {
            return Err(Error::Conflict);
        }
        let request_sha = canonical_hash(&(id, input));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        if let Some((sha,receipt))=sqlx::query_as::<_,(String,Option<String>)>("SELECT request_sha,receipt FROM external_memory_retrievals WHERE scope=$1 AND request_id=$2").bind(owner.namespace()).bind(input.request_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if sha!=request_sha{return Err(Error::Conflict);}let receipt=parse(&receipt.ok_or(Error::UncertainEffect)?)?;tx.commit().await.map_err(storage_error)?;return Ok(Err(receipt));}
        quota(&mut tx, owner, "retrievals").await?;
        let row=sqlx::query("SELECT payload,secret_scope,sealed FROM external_memory_providers WHERE scope=$1 AND id=$2 AND revision=$3 AND deleted=0").bind(owner.namespace()).bind(id).bind(input.expected_revision).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let provider: ExternalMemoryProvider =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let secret = if provider.value.auth_scheme == "none" {
            None
        } else {
            let expected = scope(owner, id, input.expected_revision, &provider.value.endpoint);
            let actual: CredentialScope = parse(
                &row.try_get::<String, _>("secret_scope")
                    .map_err(storage_error)?,
            )?;
            if expected != actual {
                return Err(Error::Forbidden);
            }
            Some(
                open_bearer(
                    keys,
                    &actual,
                    &parse::<SealedCredential>(
                        &row.try_get::<String, _>("sealed").map_err(storage_error)?,
                    )?,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?,
            )
        };
        sqlx::query("INSERT INTO external_memory_retrievals(scope,request_id,provider_id,provider_revision,request_sha,receipt) VALUES($1,$2,$3,$4,$5,NULL)").bind(owner.namespace()).bind(input.request_id.to_string()).bind(id).bind(input.expected_revision).bind(request_sha).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Ok((provider, secret)))
    }
    pub async fn external_memory_received(
        &self,
        namespace: &str,
        receipt: &ExternalMemoryRetrieval,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::android_devices::device_writer(&mut tx).await?;
        let raw = json_string(receipt)?;
        let previous:Option<String>=sqlx::query_scalar("SELECT receipt FROM external_memory_retrievals WHERE scope=$1 AND request_id=$2 AND provider_id=$3 AND provider_revision=$4").bind(namespace).bind(receipt.request_id.to_string()).bind(&receipt.provider_id).bind(receipt.provider_revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if let Some(previous) = previous {
            if previous != raw {
                return Err(Error::Conflict);
            }
        } else {
            sqlx::query("UPDATE external_memory_retrievals SET receipt=$1 WHERE scope=$2 AND request_id=$3 AND receipt IS NULL").bind(raw).bind(namespace).bind(receipt.request_id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
