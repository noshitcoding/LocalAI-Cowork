//! Narrow key selection is an index of original cipher bytes, never key/trust adoption.
use super::*;
#[derive(Clone)]
pub enum HistoricalKeySelection {
    Active,
    Chunk {
        import: Uuid,
        stream: HistoricalStream,
        index: u32,
    },
    Import {
        import: Uuid,
    },
    Preview {
        preview: Uuid,
    },
    /// Internal current-request locators, never renderer-supplied secret IDs.
    IndexChunks {
        import: Uuid,
        chunks: Vec<(String, i64)>,
    },
}
pub(super) async fn put(
    tx: &mut Transaction<'_, Any>,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    key: KeyId,
    raw: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO historical_import_key_references(import_id,owner,stream,chunk_index,key_instance,key_id,sealed_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(import.to_string()).bind(owner).bind(stream).bind(index).bind(key.instance.to_string()).bind(key.key.to_string()).bind(hash(raw.as_bytes())).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
pub(super) async fn check(
    tx: &mut Transaction<'_, Any>,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    raw: &str,
) -> Result<KeyId> {
    if raw.len() > 2 * CHUNK_BYTES {
        return Err(Error::BudgetExceeded);
    }
    let header: cipher::Sealed = parse(raw)?;
    header
        .key_id
        .validate()
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let row=sqlx::query("SELECT key_instance,key_id,sealed_sha256 FROM historical_import_key_references WHERE import_id=$1 AND owner=$2 AND stream=$3 AND chunk_index=$4").bind(import.to_string()).bind(owner).bind(stream).bind(index).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
    if header.format != 1
        || row
            .try_get::<String, _>("key_instance")
            .map_err(storage_error)?
            != header.key_id.instance.to_string()
        || row.try_get::<String, _>("key_id").map_err(storage_error)?
            != header.key_id.key.to_string()
        || row
            .try_get::<String, _>("sealed_sha256")
            .map_err(storage_error)?
            != hash(raw.as_bytes())
    {
        return Err(Error::Conflict);
    }
    Ok(header.key_id)
}
#[expect(
    clippy::too_many_arguments,
    reason = "replace: Keep original owner/import/stream/index and old/new native key/ciphertext references explicit"
)]
pub(super) async fn replace(
    tx: &mut Transaction<'_, Any>,
    owner: &str,
    import: Uuid,
    stream: &str,
    index: i64,
    old: KeyId,
    new: KeyId,
    original: &str,
    next: &str,
) -> Result<()> {
    let n=sqlx::query("UPDATE historical_import_key_references SET key_instance=$1,key_id=$2,sealed_sha256=$3 WHERE import_id=$4 AND owner=$5 AND stream=$6 AND chunk_index=$7 AND key_instance=$8 AND key_id=$9 AND sealed_sha256=$10").bind(new.instance.to_string()).bind(new.key.to_string()).bind(hash(next.as_bytes())).bind(import.to_string()).bind(owner).bind(stream).bind(index).bind(old.instance.to_string()).bind(old.key.to_string()).bind(hash(original.as_bytes())).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
    if n != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
/// Called once inside migration66, before its checksum commits. Ciphertext, AAD,
/// original plaintext pins and old receipts are deliberately untouched.
pub(crate) async fn backfill(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let mut after_import = String::new();
    let mut after_stream = String::new();
    let mut after_index = -1i64;
    loop {
        let rows=sqlx::query("SELECT import_id,owner,stream,chunk_index,sealed FROM historical_import_chunks WHERE import_id>$1 OR (import_id=$1 AND stream>$2) OR (import_id=$1 AND stream=$2 AND chunk_index>$3) ORDER BY import_id,stream,chunk_index LIMIT 16").bind(&after_import).bind(&after_stream).bind(after_index).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after_import = row.try_get("import_id").map_err(storage_error)?;
            after_stream = row.try_get("stream").map_err(storage_error)?;
            after_index = row.try_get("chunk_index").map_err(storage_error)?;
            let owner: String = row.try_get("owner").map_err(storage_error)?;
            let raw: String = row.try_get("sealed").map_err(storage_error)?;
            if raw.len() > 2 * CHUNK_BYTES {
                return Err(Error::BudgetExceeded);
            }
            let header: cipher::Sealed = parse(&raw)?;
            header
                .key_id
                .validate()
                .map_err(|_| Error::NativeKeyUnavailable)?;
            if header.format != 1
                || (!owner.starts_with("org:")
                    && owner != format!("native:{}", header.key_id.instance))
            {
                return Err(Error::Conflict);
            }
            put(
                tx,
                &owner,
                Uuid::parse_str(&after_import).map_err(storage_error)?,
                &after_stream,
                after_index,
                header.key_id,
                &raw,
            )
            .await?;
        }
    }
    Ok(())
}
impl Store {
    pub async fn native_historical_inventory(
        &self,
        selection: HistoricalKeySelection,
    ) -> Result<(crate::NativeSecretState, Vec<cowork_secrets::KeyProof>)> {
        if !self.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        if state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let active = state.active.as_ref().ok_or(Error::NativeKeyUnavailable)?;
        let owner = format!("native:{}", state.instance);
        let mut selected_chunks = None;
        let (import, stream, index) = match selection {
            HistoricalKeySelection::Active => (String::new(), String::new(), -1),
            HistoricalKeySelection::Chunk {
                import,
                stream,
                index,
            } => (
                import.to_string(),
                stream.locator().to_owned(),
                i64::from(index),
            ),
            HistoricalKeySelection::Import { import } => (import.to_string(), String::new(), -1),
            HistoricalKeySelection::Preview { preview } => {
                let import: String = sqlx::query_scalar(
                    "SELECT import_id FROM historical_import_previews WHERE id=$1 AND owner=$2",
                )
                .bind(preview.to_string())
                .bind(&owner)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
                (import, String::new(), -1)
            }
            HistoricalKeySelection::IndexChunks { import, chunks } => {
                if chunks.is_empty()
                    || chunks.len() > 64
                    || chunks
                        .iter()
                        .any(|(s, i)| !s.starts_with("history:") || s.len() > 200 || *i < 0)
                {
                    return Err(Error::Forbidden);
                }
                selected_chunks = Some(chunks);
                (import.to_string(), String::new(), -1)
            }
        };
        let mut required = std::collections::BTreeSet::from([active.key.key]);
        if !import.is_empty() {
            let rows = if let Some(chunks) = selected_chunks {
                let mut rows = vec![];
                for (stream, index) in chunks {
                    if let Some(row)=sqlx::query("SELECT key_instance,key_id FROM historical_import_key_references WHERE owner=$1 AND import_id=$2 AND stream=$3 AND chunk_index=$4").bind(&owner).bind(&import).bind(stream).bind(index).fetch_optional(&mut *tx).await.map_err(storage_error)? { rows.push(row); }
                }
                rows
            } else {
                sqlx::query("SELECT DISTINCT key_instance,key_id FROM historical_import_key_references WHERE owner=$1 AND import_id=$2 AND ($3='' OR stream=$3) AND ($4<0 OR chunk_index=$4) LIMIT 1025").bind(&owner).bind(import).bind(stream).bind(index).fetch_all(&mut *tx).await.map_err(storage_error)?
            };
            if rows.len() > 1024 {
                return Err(Error::BudgetExceeded);
            }
            for row in rows {
                let instance: Uuid = Uuid::parse_str(
                    &row.try_get::<String, _>("key_instance")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?;
                let key: Uuid =
                    Uuid::parse_str(&row.try_get::<String, _>("key_id").map_err(storage_error)?)
                        .map_err(storage_error)?;
                if instance != state.instance || key.is_nil() {
                    return Err(Error::NativeKeyUnavailable);
                }
                required.insert(key);
            }
        }
        for key in &required {
            crate::native_key_retirement::allow_cipher(
                &mut tx,
                &KeyId {
                    instance: state.instance,
                    key: *key,
                }
                .cipher_id()
                .map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .await?;
        }
        let proofs =
            crate::native_key_retirement::required_proofs(&mut tx, &state, &required).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((state, proofs.into_values().collect()))
    }
}
