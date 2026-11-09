//! Durable quarantine admission and explicit current-content activation.
//! Caller must supply either the real native owner gate or a persisted Principal.
#[path = "activation.rs"]
mod activation;
#[path = "catalog.rs"]
mod catalog;
#[path = "cipher.rs"]
mod cipher;
#[path = "content.rs"]
mod content;
#[path = "file_lifecycle.rs"]
mod file_lifecycle;
#[path = "history.rs"]
mod history;
#[path = "key_references.rs"]
mod key_references;
pub use catalog::historical_preview_summary;
pub use key_references::HistoricalKeySelection;
pub(crate) async fn backfill_key_references(tx: &mut Transaction<'_, Any>) -> Result<()> {
    key_references::backfill(tx).await
}
#[path = "types.rs"]
mod types;
use super::verifier::{self, UntrustedHistoricalStreams};
use crate::*;
use cowork_identity::{ContentAction, ContentPermit, LockedIdentityQuota, Principal};
use cowork_secrets::{KeyId, KeyPurpose, KeyStore};
use sha2::{Digest, Sha256};
use sqlx::{Any, Transaction};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
pub use types::*;
use zeroize::Zeroizing;

/// Opaque native authority created only after both private native and owner bearer gates.
/// The installation is current; no historical actor is promoted to a Principal.
pub struct NativeHistoricalAuthority {
    state: crate::NativeSecretState,
}
impl NativeHistoricalAuthority {
    /// Runtime supplies the actual OS-proved current native state. No HTTP Deserialize.
    pub fn from_current_runtime_state(state: crate::NativeSecretState) -> Result<Self> {
        let active = state.active.as_ref().ok_or(Error::NativeKeyUnavailable)?;
        active.validate().map_err(|_| Error::NativeKeyUnavailable)?;
        if state.instance.is_nil()
            || active.key.instance != state.instance
            || state.pending_operation.is_some()
        {
            return Err(Error::NativeKeyUnavailable);
        }
        Ok(Self { state })
    }
    pub fn instance(&self) -> Uuid {
        self.state.instance
    }
    pub fn key_id(&self) -> Result<KeyId> {
        Ok(self
            .state
            .active
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?
            .key)
    }
}
pub enum HistoricalAuthority<'a> {
    PersonalNative(&'a NativeHistoricalAuthority),
    Organization {
        actor: &'a Principal,
        organization_id: Uuid,
        admission_project_id: Uuid,
    },
}
impl HistoricalAuthority<'_> {
    fn owner(&self) -> HistoricalOwner {
        match self {
            Self::PersonalNative(a) => HistoricalOwner::PersonalNative {
                installation_id: a.state.instance,
            },
            Self::Organization {
                actor,
                organization_id,
                admission_project_id,
            } => HistoricalOwner::Organization {
                organization_id: *organization_id,
                creator_id: actor.identity_id(),
                admission_project_id: *admission_project_id,
            },
        }
    }
}
fn identity_error(e: cowork_identity::IdentityError) -> Error {
    crate::team_service::identity_error(e)
}
fn invalid() -> Error {
    Error::Validation("Invalid historical product import".into())
}
fn sqltx<'a, 'b, 'c, 'd>(
    locked: &'a mut LockedIdentityQuota<'b, 'c, 'd>,
) -> Result<&'a mut Transaction<'d, Any>> {
    locked.transaction().map_err(identity_error)
}
async fn authorize(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    key_id: KeyId,
    action: ContentAction,
) -> Result<Option<ContentPermit>> {
    let permit = match authority {
        HistoricalAuthority::PersonalNative(a) => {
            let current = crate::native_secrets::lock_state(sqltx(locked)?).await?;
            if current != a.state
                || current.pending_operation.is_some()
                || current.active.as_ref().is_none_or(|p| p.key != key_id)
            {
                return Err(Error::NativeKeyUnavailable);
            }
            crate::native_key_retirement::allow_cipher(
                sqltx(locked)?,
                &key_id
                    .cipher_id()
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .await?;
            None
        }
        HistoricalAuthority::Organization {
            actor,
            organization_id,
            admission_project_id,
        } => Some(
            locked
                .authorize_content_in_transaction(
                    actor,
                    *organization_id,
                    *admission_project_id,
                    action,
                )
                .await
                .map_err(identity_error)?,
        ),
    };
    if matches!(authority, HistoricalAuthority::Organization { .. }) {
        crate::server_keys::server_admission(sqltx(locked)?, key_id).await?;
    }
    Ok(permit)
}
async fn recheck(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    permit: &Option<ContentPermit>,
) -> Result<()> {
    if let Some(p) = permit {
        locked
            .validate_content_in_transaction(p)
            .await
            .map_err(identity_error)?;
    }
    Ok(())
}
async fn reserve(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    permit: &Option<ContentPermit>,
    installation: Uuid,
    expected_revision: i64,
    key: &str,
    bytes: u64,
    sha: &str,
) -> Result<HistoricalImportLimit> {
    let limit = lock_limit(sqltx(locked)?, installation).await?;
    if limit.revision != expected_revision
        || bytes > limit.limit_bytes.saturating_sub(limit.reserved_bytes)
    {
        return Err(Error::BudgetExceeded);
    }
    if let HistoricalAuthority::Organization { actor, .. } = authority {
        let permit = permit.as_ref().ok_or(Error::Forbidden)?;
        // The existing per-charge bound is retained. All installments belong to
        // this one transaction and all either commit or roll back together.
        let mut remaining = bytes;
        let mut part = 0;
        while remaining > 0 {
            let n = remaining.min(512 * 1024 * 1024);
            locked
                .charge_storage_in_transaction(
                    actor,
                    permit,
                    &format!("hist:{key}:{part}"),
                    n as i64,
                    sha,
                )
                .await
                .map_err(identity_error)?;
            remaining -= n;
            part += 1;
        }
    }
    let n=sqlx::query("UPDATE historical_import_limit SET reserved_bytes=reserved_bytes+$1 WHERE installation_id=$2 AND revision=$3 AND reserved_bytes=$4")
        .bind(bytes as i64).bind(installation.to_string()).bind(limit.revision).bind(limit.reserved_bytes as i64).execute(&mut **sqltx(locked)?).await.map_err(storage_error)?.rows_affected();
    if n != 1 {
        return Err(Error::Conflict);
    }
    Ok(HistoricalImportLimit {
        reserved_bytes: limit.reserved_bytes + bytes,
        ..limit
    })
}
async fn lock_limit(
    tx: &mut Transaction<'_, Any>,
    installation: Uuid,
) -> Result<HistoricalImportLimit> {
    let n = sqlx::query(
        "UPDATE historical_import_limit SET revision=revision WHERE installation_id=$1",
    )
    .bind(installation.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage_error)?
    .rows_affected();
    if n != 1 {
        return Err(Error::Validation(
            "An explicit installation import-storage limit is required".into(),
        ));
    }
    let r=sqlx::query("SELECT revision,limit_bytes,reserved_bytes FROM historical_import_limit WHERE installation_id=$1").bind(installation.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let bytes: i64 = r.try_get("limit_bytes").map_err(storage_error)?;
    let reserved: i64 = r.try_get("reserved_bytes").map_err(storage_error)?;
    Ok(HistoricalImportLimit {
        installation_id: installation,
        revision: r.try_get("revision").map_err(storage_error)?,
        limit_bytes: u64::try_from(bytes).map_err(|_| Error::Conflict)?,
        reserved_bytes: u64::try_from(reserved).map_err(|_| Error::Conflict)?,
    })
}
async fn record(
    tx: &mut Transaction<'_, Any>,
    import: Uuid,
    owner: &HistoricalOwner,
) -> Result<HistoricalImportRecord> {
    sqlx::query("UPDATE historical_imports SET revision=revision WHERE id=$1 AND owner=$2")
        .bind(import.to_string())
        .bind(owner.locator())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let r=sqlx::query("SELECT payload,state,revision,request_sha256,declared_bytes,received_bytes FROM historical_imports WHERE id=$1 AND owner=$2").bind(import.to_string()).bind(owner.locator()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let v: HistoricalImportRecord =
        parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v.id != import
        || &v.owner != owner
        || v.state != r.try_get::<String, _>("state").map_err(storage_error)?
        || v.revision != r.try_get::<i64, _>("revision").map_err(storage_error)?
        || v.request_sha256
            != r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || v.declared_bytes
            != r.try_get::<i64, _>("declared_bytes")
                .map_err(storage_error)? as u64
        || v.received_bytes
            != r.try_get::<i64, _>("received_bytes")
                .map_err(storage_error)? as u64
    {
        return Err(Error::Conflict);
    }
    Ok(v)
}
async fn save(
    tx: &mut Transaction<'_, Any>,
    previous: &HistoricalImportRecord,
    next: &HistoricalImportRecord,
) -> Result<()> {
    let n=sqlx::query("UPDATE historical_imports SET state=$1,revision=$2,received_bytes=$3,payload=$4 WHERE id=$5 AND owner=$6 AND revision=$7 AND payload=$8")
        .bind(&next.state).bind(next.revision).bind(next.received_bytes as i64).bind(json_string(next)?).bind(next.id.to_string()).bind(next.owner.locator()).bind(previous.revision).bind(json_string(previous)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    if n != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn writer(keys: &dyn KeyStore, key: KeyId) -> Result<KeyId> {
    keys.key_for_purpose(&key, KeyPurpose::DataEncryption)
        .map_err(|_| Error::NativeKeyUnavailable)
}
fn stream(s: &str) -> String {
    format!("plan:{s}")
}
fn plan_sha(p: &HistoricalImportPreview) -> Result<String> {
    let mut p = p.clone();
    p.plan_sha256.clear();
    p.state = "preview".into();
    Ok(hash(json_string(&p)?.as_bytes()))
}
async fn load_plan(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &HistoricalOwner,
    preview: Uuid,
) -> Result<HistoricalImportPreview> {
    let r=sqlx::query("SELECT import_id,owner,plan_sha256,state,payload FROM historical_import_previews WHERE id=$1 AND owner=$2").bind(preview.to_string()).bind(owner.locator()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let import_text: String = r.try_get("import_id").map_err(storage_error)?;
    let import = Uuid::parse_str(&import_text).map_err(storage_error)?;
    let bytes = cipher::get_pages(
        tx,
        keys,
        &owner.locator(),
        import,
        &stream(&preview.to_string()),
    )
    .await?;
    let mut p: HistoricalImportPreview = serde_json::from_slice(&bytes).map_err(storage_error)?;
    if p.id != preview
        || p.import_id != import
        || &p.owner != owner
        || p.plan_sha256
            != r.try_get::<String, _>("plan_sha256")
                .map_err(storage_error)?
        || plan_sha(&p)? != p.plan_sha256
        || r.try_get::<String, _>("payload").map_err(storage_error)?
            != json_string(&(
                p.id,
                p.import_id,
                &p.owner,
                p.request_id,
                &p.request_sha256,
                &p.plan_sha256,
            ))?
    {
        return Err(Error::Conflict);
    }
    p.state = r.try_get("state").map_err(storage_error)?;
    Ok(p)
}
async fn configure_limit_in(
    tx: &mut Transaction<'_, Any>,
    installation: Uuid,
    expected_revision: i64,
    limit_bytes: u64,
) -> Result<HistoricalImportLimit> {
    if let Some(row)=sqlx::query("SELECT limit_bytes,original_receipt FROM historical_import_limit_operations WHERE installation_id=$1 AND expected_revision=$2").bind(installation.to_string()).bind(expected_revision).fetch_optional(&mut **tx).await.map_err(storage_error)? {
        if row.try_get::<i64,_>("limit_bytes").map_err(storage_error)?!=limit_bytes as i64 {return Err(Error::Conflict);}
        let receipt:HistoricalImportLimit=parse(&row.try_get::<String,_>("original_receipt").map_err(storage_error)?)?;
        if receipt.installation_id!=installation||receipt.revision!=expected_revision+1||receipt.limit_bytes!=limit_bytes{return Err(Error::Conflict);}
        return Ok(receipt);
    }
    let n = if expected_revision == 0 {
        sqlx::query("INSERT INTO historical_import_limit(installation_id,revision,limit_bytes,reserved_bytes,updated_at) VALUES($1,1,$2,0,$3) ON CONFLICT(installation_id) DO NOTHING").bind(installation.to_string()).bind(limit_bytes as i64).bind(now()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
    } else {
        sqlx::query("UPDATE historical_import_limit SET revision=revision+1,limit_bytes=$1,updated_at=$2 WHERE installation_id=$3 AND revision=$4 AND reserved_bytes<=$1").bind(limit_bytes as i64).bind(now()).bind(installation.to_string()).bind(expected_revision).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
    };
    if n != 1 {
        return Err(Error::Conflict);
    }
    let value = lock_limit(tx, installation).await?;
    sqlx::query("INSERT INTO historical_import_limit_operations(installation_id,expected_revision,limit_bytes,original_receipt) VALUES($1,$2,$3,$4)").bind(installation.to_string()).bind(expected_revision).bind(limit_bytes as i64).bind(json_string(&value)?).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(value)
}
impl Store {
    pub async fn historical_import_authority(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
        action: ContentAction,
    ) -> Result<()> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, action).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)
    }
    pub async fn historical_content_catalog(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<HistoricalContentCatalogEntry>> {
        if limit == 0 || limit > 100 {
            return Err(invalid());
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        if record.state == "receiving" {
            return Err(Error::Conflict);
        }
        let owner = authority.owner();
        let manifest = catalog::catalog_manifest(sqltx(&mut locked)?, keys, &owner, import).await?;
        let end = offset
            .checked_add(limit)
            .ok_or_else(invalid)?
            .min(manifest.count);
        let mut value = vec![];
        for ordinal in offset..end {
            value.push(
                catalog::read_catalog_entry(sqltx(&mut locked)?, keys, &owner, import, ordinal)
                    .await?,
            );
        }
        if serde_json::to_vec(&value).map_err(storage_error)?.len() > 3 * CHUNK_BYTES {
            return Err(Error::Validation(
                "Use the paged content catalogue and bounded original metadata pages".into(),
            ));
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_content_page(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        project_index: u32,
        page_index: u32,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        if record.state == "receiving" {
            return Err(Error::Conflict);
        }
        let entry = catalog::read_catalog_entry(
            sqltx(&mut locked)?,
            keys,
            &authority.owner(),
            import,
            project_index,
        )
        .await?;
        let reference = entry
            .pages
            .get(page_index as usize)
            .ok_or(Error::NotFound)?;
        let page = cipher::get(
            sqltx(&mut locked)?,
            keys,
            &authority.owner().locator(),
            import,
            &reference.stream,
            reference.chunk_index as i64,
        )
        .await?;
        if reference.sha256 != hash(&page) || reference.size != page.len() as u64 {
            return Err(Error::Conflict);
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(page)
    }
    /// Installation operator only; HTTP import routes never call this mutation.
    pub async fn configure_historical_import_limit(
        &self,
        installation: Uuid,
        expected_revision: i64,
        limit_bytes: u64,
    ) -> Result<HistoricalImportLimit> {
        if installation.is_nil()
            || expected_revision < 0
            || expected_revision == i64::MAX
            || limit_bytes == 0
            || limit_bytes > MAX_INTAKE_BYTES * 16
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let value =
            configure_limit_in(&mut tx, installation, expected_revision, limit_bytes).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_import_limit(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
    ) -> Result<HistoricalImportLimit> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let value = lock_limit(sqltx(&mut locked)?, key_id.instance).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn begin_historical_import(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
        input: BeginHistoricalImport,
    ) -> Result<HistoricalImportRecord> {
        checked_pins(&input.pins)?;
        let (mapping_maximum, _) = preview_format_limits(input.preview_format.as_deref())?;
        let mut kinds = std::collections::BTreeSet::new();
        let mut declared = 0u64;
        let mut reservation = 0u64;
        for f in &input.files {
            if !kinds.insert(f.stream)
                || f.size == 0
                || f.size
                    > if f.stream == HistoricalStream::Mapping {
                        mapping_maximum
                    } else {
                        f.stream.maximum()
                    }
                || f.sha256 != input.pins.stream(f.stream)
            {
                return Err(invalid());
            }
            declared = declared.checked_add(f.size).ok_or(Error::BudgetExceeded)?;
            reservation = reservation
                .checked_add(f.size.div_ceil(3) * 4 + f.size.div_ceil(CHUNK_BYTES as u64) * 1024)
                .ok_or(Error::BudgetExceeded)?;
        }
        if kinds.len() != 5 || declared > MAX_INTAKE_BYTES || input.request_id.is_nil() {
            return Err(invalid());
        }
        reservation += json_string(&input)?.len() as u64 * 4 + 8192;
        let owner = authority.owner();
        let request_sha256 = canonical_hash(&("historical-intake/v1", &owner, &input));
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        if let Some(found)=sqlx::query_scalar::<_,String>("SELECT id FROM historical_imports WHERE owner=$1 AND request_id=$2 AND request_sha256=$3").bind(owner.locator()).bind(input.request_id.to_string()).bind(&request_sha256).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?{
            let value=record(sqltx(&mut locked)?,Uuid::parse_str(&found).map_err(storage_error)?,&owner).await?;recheck(&mut locked,&permit).await?;drop(locked);tx.commit().await.map_err(storage_error)?;return Ok(value)
        }
        let identifier = Uuid::new_v4();
        reserve(
            &mut locked,
            &authority,
            &permit,
            key_id.instance,
            input.expected_limit_revision,
            &identifier.to_string(),
            reservation,
            &input.pins.quarantine_database,
        )
        .await?;
        let value = HistoricalImportRecord {
            id: identifier,
            owner: owner.clone(),
            request_id: input.request_id,
            request_sha256,
            state: "receiving".into(),
            revision: 1,
            declared_bytes: declared,
            received_bytes: 0,
            reserved_bytes: reservation,
            files: input.files,
            pins: input.pins,
            source_profile: None,
            source_family: None,
            original_rows: None,
            verifier_contract: None,
            preview_format: input.preview_format,
        };
        sqlx::query("INSERT INTO historical_imports(id,owner,request_id,request_sha256,state,revision,declared_bytes,received_bytes,payload) VALUES($1,$2,$3,$4,'receiving',1,$5,0,$6)").bind(value.id.to_string()).bind(owner.locator()).bind(value.request_id.to_string()).bind(&value.request_sha256).bind(declared as i64).bind(json_string(&value)?).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_import_record(
        &self,
        authority: HistoricalAuthority<'_>,
        key_id: KeyId,
        import: Uuid,
    ) -> Result<HistoricalImportRecord> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let value = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn write_historical_chunk(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        kind: HistoricalStream,
        index: u32,
        bytes: &[u8],
    ) -> Result<HistoricalChunkReceipt> {
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err(invalid());
        }
        let key_id = writer(keys, key_id)?;
        let owner = authority.owner();
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        crate::server_keys::cipher_writer(sqltx(&mut locked)?, key_id).await?;
        let previous = record(sqltx(&mut locked)?, import, &owner).await?;
        let declaration = previous
            .files
            .iter()
            .find(|f| f.stream == kind)
            .ok_or_else(invalid)?;
        let offset = index as u64 * CHUNK_BYTES as u64;
        if offset >= declaration.size
            || bytes.len() as u64 != (declaration.size - offset).min(CHUNK_BYTES as u64)
        {
            return Err(invalid());
        }
        let digest = hash(bytes);
        let receipt = HistoricalChunkReceipt {
            import_id: import,
            stream: kind,
            chunk_index: index,
            size: bytes.len() as u64,
            sha256: digest.clone(),
        };
        let original=sqlx::query("SELECT owner,size,sha256,sealed FROM historical_import_chunks WHERE import_id=$1 AND stream=$2 AND chunk_index=$3").bind(import.to_string()).bind(kind.locator()).bind(index as i64).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?;
        if let Some(r) = original {
            if r.try_get::<String, _>("owner").map_err(storage_error)? != owner.locator()
                || r.try_get::<i64, _>("size").map_err(storage_error)? != bytes.len() as i64
                || r.try_get::<String, _>("sha256").map_err(storage_error)? != digest
            {
                return Err(Error::Conflict);
            }
            let original = cipher::get(
                sqltx(&mut locked)?,
                keys,
                &owner.locator(),
                import,
                kind.locator(),
                index as i64,
            )
            .await?;
            if original.as_slice() != bytes {
                return Err(Error::Conflict);
            }
        } else {
            if previous.state != "receiving" {
                return Err(Error::Conflict);
            }
            cipher::put(
                sqltx(&mut locked)?,
                keys,
                key_id,
                &owner.locator(),
                import,
                kind.locator(),
                index as i64,
                bytes,
            )
            .await?;
            let mut next = previous.clone();
            next.received_bytes = next
                .received_bytes
                .checked_add(bytes.len() as u64)
                .ok_or(Error::BudgetExceeded)?;
            next.revision += 1;
            save(sqltx(&mut locked)?, &previous, &next).await?;
        }
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn read_historical_chunk(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        kind: HistoricalStream,
        index: u32,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        let value = cipher::get(
            sqltx(&mut locked)?,
            keys,
            &authority.owner().locator(),
            import,
            kind.locator(),
            index as i64,
        )
        .await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Plaintext is reconstructed only beneath the startup-owned private directory.
    /// The five immutable receipts are rehashed before the original verifier runs.
    pub async fn verify_historical_import(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        private_staging_root: &Path,
    ) -> Result<HistoricalImportRecord> {
        let current = self
            .historical_import_record(authority_ref(&authority), key_id, import)
            .await?;
        if current.state != "receiving" {
            return Ok(current);
        }
        if current.received_bytes != current.declared_bytes {
            return Err(Error::Conflict);
        }
        let directory = tempfile::Builder::new()
            .prefix("historical-product-")
            .tempdir_in(private_staging_root)
            .map_err(storage_error)?;
        let mut inputs = std::collections::BTreeMap::new();
        for kind in HistoricalStream::all() {
            let declaration = current
                .files
                .iter()
                .find(|d| d.stream == kind)
                .ok_or(Error::Conflict)?;
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                opts.share_mode(1).custom_flags(0x00200000);
            }
            let mut file = opts
                .open(directory.path().join(kind.locator()))
                .map_err(storage_error)?;
            let mut digest = Sha256::new();
            let mut size = 0u64;
            for index in 0..declaration.size.div_ceil(CHUNK_BYTES as u64) {
                let bytes = self
                    .read_historical_chunk(
                        authority_ref(&authority),
                        keys,
                        key_id,
                        import,
                        kind,
                        index as u32,
                    )
                    .await?;
                size += bytes.len() as u64;
                digest.update(bytes.as_slice());
                file.write_all(&bytes).map_err(storage_error)?;
            }
            if size != declaration.size || format!("{:x}", digest.finalize()) != declaration.sha256
            {
                return Err(Error::Conflict);
            }
            file.flush().map_err(storage_error)?;
            file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
            inputs.insert(kind, file);
        }
        let mut take = |kind| {
            inputs
                .remove(&kind)
                .map(|f| Box::new(f) as Box<dyn Read + Send>)
                .ok_or(Error::Conflict)
        };
        let streams = UntrustedHistoricalStreams {
            quarantine_manifest: take(HistoricalStream::QuarantineManifest)?,
            quarantine_database: take(HistoricalStream::QuarantineDatabase)?,
            mapping: take(HistoricalStream::Mapping)?,
            preview: take(HistoricalStream::Preview)?,
            cold_target_snapshot: take(HistoricalStream::ColdTargetSnapshot)?,
        };
        let archive = verifier::verify(streams, checked_pins(&current.pins)?)
            .await
            .map_err(|_| {
                Error::Validation("Original historical archive verification refused".into())
            })?;
        if archive.provenance().verifier_contract
            != preview_format_limits(current.preview_format.as_deref())?.1
        {
            return Err(Error::Validation(
                "The pinned original preview format does not match its admitted verifier contract"
                    .into(),
            ));
        }
        let mut projection_stage = content::stage_content(&archive, directory.path()).await?;
        let key_id = writer(keys, key_id)?;
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        crate::server_keys::cipher_writer(sqltx(&mut locked)?, key_id).await?;
        let previous = record(sqltx(&mut locked)?, import, &current.owner).await?;
        if previous.state != "receiving" {
            recheck(&mut locked, &permit).await?;
            drop(locked);
            tx.commit().await.map_err(storage_error)?;
            return Ok(previous);
        }
        if previous.revision != current.revision
            || json_string(&previous)? != json_string(&current)?
        {
            return Err(Error::Conflict);
        }
        let limit = lock_limit(sqltx(&mut locked)?, key_id.instance).await?;
        let mut catalog_count = 0u32;
        let mut catalog_digest = Sha256::new();
        let mut group_after = None;
        let mut group_index = 0usize;
        while let Some(group) = projection_stage.next_group(group_after.as_deref()).await? {
            let (projects, unsupported) = match projection_stage.materialize(&group).await {
                Ok(projects) => (projects, None),
                Err(Error::Validation(_) | Error::BudgetExceeded | Error::Conflict) => {
                    let reason = "This source project has an unsupported current content shape or exceeds a current editable-content field/partition limit; original bytes remain complete and require an explicit history-only disposition.";
                    (
                        vec![projection_stage.unsupported(&group, reason).await?],
                        Some(reason.to_owned()),
                    )
                }
                Err(e) => return Err(e),
            };
            for (sub_index, mut source) in projects.into_iter().enumerate() {
                let bytes = serde_json::to_vec(&source).map_err(storage_error)?;
                let pages = if unsupported.is_none() {
                    reserve(
                        &mut locked,
                        &authority,
                        &permit,
                        key_id.instance,
                        limit.revision,
                        &format!("{import}:content:{group_index}:{sub_index}"),
                        bytes.len() as u64 * 2 + 4096,
                        &hash(&bytes),
                    )
                    .await?;
                    cipher::put_pages(
                        sqltx(&mut locked)?,
                        keys,
                        key_id,
                        &current.owner.locator(),
                        import,
                        &format!("content:{group_index}:{sub_index}"),
                        &bytes,
                    )
                    .await?
                } else {
                    vec![]
                };
                for order in &mut source.orders {
                    order.draft.clear();
                    order.messages.clear();
                    order.artifacts.clear();
                }
                for document in &mut source.documents {
                    document.content.clear();
                }
                let entry = HistoricalContentCatalogEntry {
                    source,
                    pages,
                    content_bytes: bytes.len() as u64,
                    unsupported_reason: unsupported.clone(),
                };
                let item = catalog::put_entry(
                    &mut locked,
                    &authority,
                    &permit,
                    keys,
                    key_id,
                    limit.revision,
                    import,
                    catalog_count,
                    &entry,
                )
                .await?;
                catalog_digest.update(serde_json::to_vec(&item).map_err(storage_error)?);
                catalog_count = catalog_count.checked_add(1).ok_or(Error::BudgetExceeded)?;
            }
            group_after = Some(group);
            group_index = group_index.checked_add(1).ok_or(Error::BudgetExceeded)?;
        }
        projection_stage.close().await?;
        catalog::put_manifest(
            &mut locked,
            &authority,
            &permit,
            keys,
            key_id,
            limit.revision,
            import,
            catalog_count,
            format!("{:x}", catalog_digest.finalize()),
            current.pins.quarantine_database.clone(),
        )
        .await?;
        let mut next = previous.clone();
        next.state = "verified".into();
        next.revision += 1;
        next.source_profile = Some(archive.provenance().source_profile.clone());
        next.source_family = Some(archive.provenance().source_family.clone());
        next.original_rows = Some(archive.provenance().original_rows);
        next.verifier_contract = Some(archive.provenance().verifier_contract.into());
        save(sqltx(&mut locked)?, &previous, &next).await?;
        recheck(&mut locked, &permit).await?;
        drop(locked);
        tx.commit().await.map_err(storage_error)?;
        Ok(next)
    }
}
fn preview_format_limits(format: Option<&str>) -> Result<(u64, &'static str)> {
    match format {
        None | Some("localai-cowork-quarantine-preview/1") => Ok((
            2 * 1024 * 1024,
            "localai-cowork-rust-inert-history/1-installation-singleton",
        )),
        Some("localai-cowork-quarantine-preview/2") => Ok((
            256 * 1024 * 1024,
            "localai-cowork-rust-inert-history/2-current-cold-profile",
        )),
        _ => Err(Error::Validation(
            "Unknown explicitly admitted original preview format".into(),
        )),
    }
}
fn authority_ref<'a>(a: &HistoricalAuthority<'a>) -> HistoricalAuthority<'a> {
    match a {
        HistoricalAuthority::PersonalNative(n) => HistoricalAuthority::PersonalNative(n),
        HistoricalAuthority::Organization {
            actor,
            organization_id,
            admission_project_id,
        } => HistoricalAuthority::Organization {
            actor,
            organization_id: *organization_id,
            admission_project_id: *admission_project_id,
        },
    }
}

/// Root rotation invokes this in its admitted all-domain CAS transaction.
pub(crate) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    cipher::rewrap(tx, keys, target, false).await
}

pub(crate) async fn native_rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    native_quiescence(tx, target.instance).await?;
    cipher::rewrap(tx, keys, target, true).await
}
pub(crate) async fn native_quiescence(tx: &mut Transaction<'_, Any>, instance: Uuid) -> Result<()> {
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_file_operations WHERE owner=$1 AND state IN('prepared','uncertain') AND original_unsent_receipt IS NULL").bind(format!("native:{instance}")).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if pending != 0 {
        return Err(Error::UncertainEffect);
    }
    Ok(())
}
pub(crate) async fn required_native_keys(
    tx: &mut Transaction<'_, Any>,
    instance: Uuid,
) -> Result<std::collections::BTreeSet<Uuid>> {
    let mut keys = std::collections::BTreeSet::new();
    let mut after_import = String::new();
    let mut after_stream = String::new();
    let mut after_index = -1i64;
    loop {
        let rows=sqlx::query("SELECT import_id,stream,chunk_index,sealed FROM historical_import_chunks WHERE owner=$4 AND (import_id>$1 OR (import_id=$1 AND stream>$2) OR (import_id=$1 AND stream=$2 AND chunk_index>$3)) ORDER BY import_id,stream,chunk_index LIMIT 16").bind(&after_import).bind(&after_stream).bind(after_index).bind(format!("native:{instance}")).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after_import = row.try_get("import_id").map_err(storage_error)?;
            after_stream = row.try_get("stream").map_err(storage_error)?;
            after_index = row.try_get("chunk_index").map_err(storage_error)?;
            let raw: String = row.try_get("sealed").map_err(storage_error)?;
            if raw.len() > 2 * CHUNK_BYTES {
                return Err(Error::BudgetExceeded);
            }
            key_references::check(
                tx,
                &format!("native:{instance}"),
                Uuid::parse_str(&after_import).map_err(storage_error)?,
                &after_stream,
                after_index,
                &raw,
            )
            .await?;
            let sealed: cipher::Sealed = parse(&raw)?;
            if sealed.format != 1 || sealed.key_id.instance != instance {
                return Err(Error::NativeKeyUnavailable);
            }
            sealed
                .key_id
                .validate()
                .map_err(|_| Error::NativeKeyUnavailable)?;
            keys.insert(sealed.key_id.key);
        }
    }
    Ok(keys)
}
