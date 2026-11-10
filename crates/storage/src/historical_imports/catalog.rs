//! Bounded authenticated editable-content catalogue and preview transport.
use super::*;
const FORMAT: &str = "localai-cowork-content-catalog/2";
const HEADER: &str = "catalog:manifest";
const ITEMS: &str = "catalog:item";
async fn activation_receipt_sha(
    tx: &mut Transaction<'_, Any>,
    owner: &HistoricalOwner,
    import: Uuid,
    source: &str,
    manifest: &HistoricalContentCatalogManifest,
) -> Result<Option<String>> {
    // Read only the original part receipt. Target objects confer no authority here.
    let row = sqlx::query("SELECT a.owner,a.preview_id,a.part_receipt_sha256,a.activated_at,CASE WHEN LENGTH(a.part_receipt)<=$3 THEN a.part_receipt ELSE NULL END AS part_receipt,p.import_id AS preview_import,p.owner AS preview_owner,p.plan_sha256 AS preview_plan,p.state AS preview_state,CASE WHEN p.original_receipt IS NOT NULL THEN 1 ELSE 0 END AS preview_received FROM historical_import_activated_parts a LEFT JOIN historical_import_previews p ON p.id=a.preview_id WHERE a.import_id=$1 AND a.source_id=$2")
        .bind(import.to_string()).bind(source).bind(MAX_PROJECTED_BYTES as i64)
        .fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(row) = row else { return Ok(None) };
    let raw: Option<String> = row.try_get("part_receipt").map_err(storage_error)?;
    let raw = raw.ok_or(Error::Conflict)?;
    if raw.len() > MAX_PROJECTED_BYTES {
        return Err(Error::Conflict);
    }
    let receipt: Value = parse(&raw)?;
    let sha: String = row.try_get("part_receipt_sha256").map_err(storage_error)?;
    let preview: String = row.try_get("preview_id").map_err(storage_error)?;
    let plan: Option<String> = row.try_get("preview_plan").map_err(storage_error)?;
    let original_catalog = &receipt["catalog_sha256"];
    if sha != hash(raw.as_bytes())
        || row.try_get::<String, _>("owner").map_err(storage_error)? != owner.locator()
        || row
            .try_get::<Option<String>, _>("preview_owner")
            .map_err(storage_error)?
            .as_deref()
            != Some(owner.locator().as_str())
        || row
            .try_get::<Option<String>, _>("preview_import")
            .map_err(storage_error)?
            .as_deref()
            != Some(import.to_string().as_str())
        || row
            .try_get::<Option<String>, _>("preview_state")
            .map_err(storage_error)?
            .as_deref()
            != Some("active")
        || row
            .try_get::<i64, _>("preview_received")
            .map_err(storage_error)?
            != 1
        || receipt["format"] != "historical-current-part-activation/1"
        || receipt["owner"] != serde_json::to_value(owner).map_err(storage_error)?
        || receipt["import_id"] != import.to_string()
        || receipt["source_part_id"] != source
        || receipt["source_sha256"] != manifest.source_sha256
        || receipt["preview_id"] != preview
        || plan.as_deref().is_none_or(|p| receipt["plan_sha256"] != p)
        || receipt["activated_at"]
            != row
                .try_get::<i64, _>("activated_at")
                .map_err(storage_error)?
        || (!original_catalog.is_null()
            && original_catalog != &Value::String(manifest.sha256.clone()))
        || receipt["target"]["kind"] == "history_only"
    {
        return Err(Error::Conflict);
    }
    Ok(Some(sha))
}
pub(super) async fn catalog_manifest(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &HistoricalOwner,
    import: Uuid,
) -> Result<HistoricalContentCatalogManifest> {
    let present:i64=sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_chunks WHERE import_id=$1 AND owner=$2 AND stream=$3 AND chunk_index=0").bind(import.to_string()).bind(owner.locator()).bind(HEADER).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if present == 1 {
        let raw = cipher::get(tx, keys, &owner.locator(), import, HEADER, 0).await?;
        let value: HistoricalContentCatalogManifest =
            serde_json::from_slice(&raw).map_err(storage_error)?;
        if value.format != FORMAT {
            return Err(Error::Conflict);
        }
        let current = record(tx, import, owner).await?;
        if value.source_sha256 != current.pins.quarantine_database || current.state == "receiving" {
            return Err(Error::Conflict);
        }
        Ok(value)
    } else {
        let raw = cipher::get_pages(tx, keys, &owner.locator(), import, "content").await?;
        let entries: Vec<HistoricalContentCatalogEntry> =
            serde_json::from_slice(&raw).map_err(storage_error)?;
        let current = record(tx, import, owner).await?;
        Ok(HistoricalContentCatalogManifest {
            format: "localai-cowork-content-catalog/1".into(),
            count: u32::try_from(entries.len()).map_err(|_| Error::Conflict)?,
            sha256: hash(&raw),
            source_sha256: current.pins.quarantine_database,
        })
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "put_entry: Keep historical authority/locked quota, import ordinal/manifest hashes and native key custody explicit"
)]
pub(super) async fn put_entry(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    permit: &Option<ContentPermit>,
    keys: &dyn KeyStore,
    key_id: KeyId,
    limit_revision: i64,
    import: Uuid,
    ordinal: u32,
    entry: &HistoricalContentCatalogEntry,
) -> Result<HistoricalContentCatalogItem> {
    let owner = authority.owner();
    let bytes = Zeroizing::new(serde_json::to_vec(entry).map_err(storage_error)?);
    if bytes.len() > MAX_PROJECTED_BYTES {
        return Err(Error::BudgetExceeded);
    }
    reserve(
        locked,
        authority,
        permit,
        key_id.instance,
        limit_revision,
        &format!("{import}:catalog:{ordinal}"),
        bytes.len() as u64 * 2 + (bytes.len().div_ceil(CHUNK_BYTES) as u64 + 1) * 4096,
        &hash(&bytes),
    )
    .await?;
    let metadata_pages = cipher::put_pages(
        sqltx(locked)?,
        keys,
        key_id,
        &owner.locator(),
        import,
        &format!("catalog:entry:{ordinal}"),
        &bytes,
    )
    .await?;
    let item = HistoricalContentCatalogItem {
        ordinal,
        source_id: entry.source.source_id.clone(),
        name: entry.source.name.clone(),
        orders: entry.source.orders.len() as u32,
        documents: entry.source.documents.len() as u32,
        files: entry.source.files.len() as u32,
        content_bytes: entry.content_bytes,
        unsupported_reason: entry.unsupported_reason.clone(),
        metadata_pages,
    };
    let raw = serde_json::to_vec(&item).map_err(storage_error)?;
    if raw.len() > CHUNK_BYTES {
        return Err(Error::BudgetExceeded);
    }
    reserve(
        locked,
        authority,
        permit,
        key_id.instance,
        limit_revision,
        &format!("{import}:catalog-item:{ordinal}"),
        raw.len() as u64 * 2 + 4096,
        &hash(&raw),
    )
    .await?;
    cipher::put(
        sqltx(locked)?,
        keys,
        key_id,
        &owner.locator(),
        import,
        ITEMS,
        i64::from(ordinal),
        &raw,
    )
    .await?;
    let locator = serde_json::to_vec(&(ordinal, &entry.source.source_id)).map_err(storage_error)?;
    reserve(
        locked,
        authority,
        permit,
        key_id.instance,
        limit_revision,
        &format!("{import}:catalog-source:{ordinal}"),
        locator.len() as u64 * 2 + 4096,
        &hash(&locator),
    )
    .await?;
    cipher::put(
        sqltx(locked)?,
        keys,
        key_id,
        &owner.locator(),
        import,
        &format!("catalog:source:{}", hash(entry.source.source_id.as_bytes())),
        0,
        &locator,
    )
    .await?;
    Ok(item)
}
#[expect(
    clippy::too_many_arguments,
    reason = "put_manifest: Keep historical authority/locked quota, import ordinal/manifest hashes and native key custody explicit"
)]
pub(super) async fn put_manifest(
    locked: &mut LockedIdentityQuota<'_, '_, '_>,
    authority: &HistoricalAuthority<'_>,
    permit: &Option<ContentPermit>,
    keys: &dyn KeyStore,
    key_id: KeyId,
    limit_revision: i64,
    import: Uuid,
    count: u32,
    digest: String,
    source_sha256: String,
) -> Result<()> {
    let manifest = HistoricalContentCatalogManifest {
        format: FORMAT.into(),
        count,
        sha256: digest,
        source_sha256,
    };
    let raw = serde_json::to_vec(&manifest).map_err(storage_error)?;
    reserve(
        locked,
        authority,
        permit,
        key_id.instance,
        limit_revision,
        &format!("{import}:catalog-manifest"),
        raw.len() as u64 * 2 + 4096,
        &hash(&raw),
    )
    .await?;
    cipher::put(
        sqltx(locked)?,
        keys,
        key_id,
        &authority.owner().locator(),
        import,
        HEADER,
        0,
        &raw,
    )
    .await
}
pub(super) async fn catalog_item(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &HistoricalOwner,
    import: Uuid,
    ordinal: u32,
    manifest: &HistoricalContentCatalogManifest,
) -> Result<HistoricalContentCatalogItem> {
    if ordinal >= manifest.count {
        return Err(Error::NotFound);
    }
    if manifest.format == FORMAT {
        let raw = cipher::get(
            tx,
            keys,
            &owner.locator(),
            import,
            ITEMS,
            i64::from(ordinal),
        )
        .await?;
        let item: HistoricalContentCatalogItem =
            serde_json::from_slice(&raw).map_err(storage_error)?;
        if item.ordinal != ordinal
            || item.metadata_pages.iter().enumerate().any(|(i, p)| {
                p.chunk_index as usize != i
                    || p.stream != format!("catalog:entry:{ordinal}")
                    || p.size > CHUNK_BYTES as u64
            })
        {
            return Err(Error::Conflict);
        }
        Ok(item)
    } else {
        let entry = Box::pin(read_catalog_entry(tx, keys, owner, import, ordinal)).await?;
        let bytes = serde_json::to_vec(&entry).map_err(storage_error)?;
        Ok(HistoricalContentCatalogItem {
            ordinal,
            source_id: entry.source.source_id,
            name: entry.source.name,
            orders: entry.source.orders.len() as u32,
            documents: entry.source.documents.len() as u32,
            files: entry.source.files.len() as u32,
            content_bytes: entry.content_bytes,
            unsupported_reason: entry.unsupported_reason,
            metadata_pages: bytes
                .chunks(CHUNK_BYTES)
                .enumerate()
                .map(|(i, p)| HistoricalContentPage {
                    stream: format!("legacy-catalog:{ordinal}"),
                    chunk_index: i as u32,
                    size: p.len() as u64,
                    sha256: hash(p),
                })
                .collect(),
        })
    }
}
pub(super) async fn read_catalog_entry(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &HistoricalOwner,
    import: Uuid,
    ordinal: u32,
) -> Result<HistoricalContentCatalogEntry> {
    let present:i64=sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_chunks WHERE import_id=$1 AND owner=$2 AND stream=$3 AND chunk_index=0").bind(import.to_string()).bind(owner.locator()).bind(HEADER).fetch_one(&mut **tx).await.map_err(storage_error)?;
    if present == 0 {
        let raw = cipher::get_pages(tx, keys, &owner.locator(), import, "content").await?;
        let entries: Vec<HistoricalContentCatalogEntry> =
            serde_json::from_slice(&raw).map_err(storage_error)?;
        return entries
            .into_iter()
            .nth(ordinal as usize)
            .ok_or(Error::NotFound);
    }
    let manifest = catalog_manifest(tx, keys, owner, import).await?;
    let item = catalog_item(tx, keys, owner, import, ordinal, &manifest).await?;
    let raw = cipher::get_pages(
        tx,
        keys,
        &owner.locator(),
        import,
        &format!("catalog:entry:{ordinal}"),
    )
    .await?;
    if raw.len() > MAX_PROJECTED_BYTES
        || raw.chunks(CHUNK_BYTES).count() != item.metadata_pages.len()
        || raw
            .chunks(CHUNK_BYTES)
            .zip(&item.metadata_pages)
            .any(|(p, pin)| p.len() as u64 != pin.size || hash(p) != pin.sha256)
    {
        return Err(Error::Conflict);
    }
    let entry: HistoricalContentCatalogEntry =
        serde_json::from_slice(&raw).map_err(storage_error)?;
    if entry.source.source_id != item.source_id
        || entry.source.name != item.name
        || entry.source.orders.len() != item.orders as usize
        || entry.source.documents.len() != item.documents as usize
        || entry.source.files.len() != item.files as usize
        || entry.content_bytes != item.content_bytes
        || entry.unsupported_reason != item.unsupported_reason
    {
        return Err(Error::Conflict);
    }
    Ok(entry)
}
pub(super) async fn find_catalog_entry(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: &HistoricalOwner,
    import: Uuid,
    source_id: &str,
) -> Result<HistoricalContentCatalogEntry> {
    let manifest = catalog_manifest(tx, keys, owner, import).await?;
    if manifest.format == FORMAT {
        let raw = cipher::get(
            tx,
            keys,
            &owner.locator(),
            import,
            &format!("catalog:source:{}", hash(source_id.as_bytes())),
            0,
        )
        .await?;
        let (ordinal, original): (u32, String) =
            serde_json::from_slice(&raw).map_err(storage_error)?;
        if original != source_id || ordinal >= manifest.count {
            return Err(Error::Conflict);
        }
        return read_catalog_entry(tx, keys, owner, import, ordinal).await;
    }
    for ordinal in 0..manifest.count {
        let item = catalog_item(tx, keys, owner, import, ordinal, &manifest).await?;
        if item.source_id == source_id {
            return read_catalog_entry(tx, keys, owner, import, ordinal).await;
        }
    }
    Err(Error::NotFound)
}
pub fn historical_preview_summary(
    plan: &HistoricalImportPreview,
) -> Result<HistoricalPreviewSummary> {
    let mut projects = vec![];
    for (ordinal, p) in plan.projects.iter().enumerate() {
        let raw = serde_json::to_vec(p).map_err(storage_error)?;
        if raw.len() > MAX_PROJECTED_BYTES {
            return Err(Error::BudgetExceeded);
        }
        projects.push(HistoricalPreviewProjectSummary {
            ordinal: ordinal as u32,
            source_id: p.source.source_id.clone(),
            name: p.source.name.clone(),
            project_id: p.project_id.clone(),
            target: p.target.clone(),
            pages: raw
                .chunks(CHUNK_BYTES)
                .map(|p| HistoricalHistoryPagePin {
                    size: p.len() as u64,
                    sha256: hash(p),
                })
                .collect(),
        });
    }
    let value = HistoricalPreviewSummary {
        format: "localai-cowork-current-preview-summary/1".into(),
        id: plan.id,
        import_id: plan.import_id,
        plan_sha256: plan.plan_sha256.clone(),
        state: plan.state.clone(),
        source_profile: plan.source_profile.clone(),
        original_rows: plan.original_rows,
        projected_bytes: plan.projected_bytes,
        limit: plan.limit.clone(),
        history_only_file_ids: plan.history_only_file_ids.clone(),
        catalog_sha256: plan.catalog_sha256.clone(),
        catalog_count: plan.catalog_count,
        remainder_history_only_reason: plan.remainder_history_only_reason.clone(),
        projects,
    };
    if serde_json::to_vec(&value).map_err(storage_error)?.len() > 3 * CHUNK_BYTES {
        return Err(Error::BudgetExceeded);
    }
    Ok(value)
}
impl Store {
    pub async fn historical_content_catalog_page(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<HistoricalContentCatalogPage> {
        if limit == 0 || limit > 10 {
            return Err(invalid());
        }
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let owner = authority.owner();
        let current = record(sqltx(&mut locked)?, import, &owner).await?;
        if current.state == "receiving" {
            return Err(Error::Conflict);
        }
        let manifest = catalog_manifest(sqltx(&mut locked)?, keys, &owner, import).await?;
        let end = offset
            .checked_add(limit)
            .ok_or_else(invalid)?
            .min(manifest.count);
        let mut entries = vec![];
        let mut activated_parts = std::collections::BTreeMap::new();
        for ordinal in offset..end {
            let item = catalog_item(
                sqltx(&mut locked)?,
                keys,
                &owner,
                import,
                ordinal,
                &manifest,
            )
            .await?;
            if let Some(sha) = activation_receipt_sha(
                sqltx(&mut locked)?,
                &owner,
                import,
                &item.source_id,
                &manifest,
            )
            .await?
            {
                activated_parts.insert(item.source_id.clone(), sha);
            }
            entries.push(item);
        }
        let result = HistoricalContentCatalogPage {
            manifest: manifest.clone(),
            offset,
            next_offset: (end < manifest.count).then_some(end),
            entries,
            activated_parts: Some(activated_parts),
            editable_selection_closed: (current.state == "active").then_some(true),
        };
        if serde_json::to_vec(&result).map_err(storage_error)?.len() > CHUNK_BYTES {
            return Err(Error::BudgetExceeded);
        }
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn historical_catalog_metadata_page(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        ordinal: u32,
        page: u32,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let owner = authority.owner();
        record(sqltx(&mut locked)?, import, &owner).await?;
        let manifest = catalog_manifest(sqltx(&mut locked)?, keys, &owner, import).await?;
        let item = catalog_item(
            sqltx(&mut locked)?,
            keys,
            &owner,
            import,
            ordinal,
            &manifest,
        )
        .await?;
        let pin = item
            .metadata_pages
            .get(page as usize)
            .ok_or(Error::NotFound)?;
        let bytes = if manifest.format == FORMAT {
            cipher::get(
                sqltx(&mut locked)?,
                keys,
                &owner.locator(),
                import,
                &pin.stream,
                i64::from(page),
            )
            .await?
        } else {
            let entry =
                read_catalog_entry(sqltx(&mut locked)?, keys, &owner, import, ordinal).await?;
            let raw = serde_json::to_vec(&entry).map_err(storage_error)?;
            Zeroizing::new(
                raw.chunks(CHUNK_BYTES)
                    .nth(page as usize)
                    .ok_or(Error::NotFound)?
                    .to_vec(),
            )
        };
        if bytes.len() > CHUNK_BYTES || bytes.len() as u64 != pin.size || hash(&bytes) != pin.sha256
        {
            return Err(Error::Conflict);
        }
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
    pub async fn historical_preview_project_page(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        ordinal: u32,
        page: u32,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let plan = self
            .historical_import_preview(authority, keys, key_id, preview)
            .await?;
        let project = plan.projects.get(ordinal as usize).ok_or(Error::NotFound)?;
        let raw = Zeroizing::new(serde_json::to_vec(project).map_err(storage_error)?);
        if raw.len() > MAX_PROJECTED_BYTES {
            return Err(Error::BudgetExceeded);
        }
        Ok(Zeroizing::new(
            raw.chunks(CHUNK_BYTES)
                .nth(page as usize)
                .ok_or(Error::NotFound)?
                .to_vec(),
        ))
    }
}
