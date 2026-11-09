//! Authenticated complete-table browser. Its index grants no runtime authority.
use super::*;
#[path = "history_stage.rs"]
mod stage;
use stage::{HistoryStage, INDEX_FORMAT, StageRecord};

const HEADER: &str = "history:header";
fn table_stream(table: u32) -> String {
    format!("history:table:{table}")
}
fn row_stream(table: u32) -> String {
    format!("history:row:{table}")
}
fn cells_stream(table: u32, position: u64) -> String {
    format!("history:cells:{table}:{position}")
}
fn values_stream(table: u32, position: u64, column: u32) -> String {
    format!("history:value:{table}:{position}:{column}")
}
fn bounded<T: serde::Serialize>(value: T) -> Result<T> {
    if serde_json::to_vec(&value).map_err(storage_error)?.len() > CHUNK_BYTES {
        return Err(Error::BudgetExceeded);
    }
    Ok(value)
}
fn original_build_request(
    index: &HistoricalHistoryIndex,
    owner: &HistoricalOwner,
    import: Uuid,
    input: &BuildHistoricalHistoryIndex,
) -> Result<()> {
    if index.request_id == input.request_id
        && index.request_sha256
            != hash(json_string(&(INDEX_FORMAT, owner, import, input))?.as_bytes())
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn has_index(
    tx: &mut Transaction<'_, Any>,
    owner: &HistoricalOwner,
    import: Uuid,
) -> Result<bool> {
    let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM historical_import_chunks WHERE import_id=$1 AND owner=$2 AND stream=$3 AND chunk_index=0").bind(import.to_string()).bind(owner.locator()).bind(HEADER).fetch_one(&mut **tx).await.map_err(storage_error)?;
    Ok(n == 1)
}
async fn root(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    record: &HistoricalImportRecord,
) -> Result<HistoricalHistoryIndex> {
    if record.state == "receiving" {
        return Err(Error::Conflict);
    }
    let raw = cipher::get(tx, keys, &record.owner.locator(), record.id, HEADER, 0).await?;
    let v: HistoricalHistoryIndex = serde_json::from_slice(&raw).map_err(storage_error)?;
    let mut unsigned = v.clone();
    unsigned.receipt_sha256.clear();
    if v.format != INDEX_FORMAT
        || v.import_id != record.id
        || v.owner != record.owner
        || v.original_intake_sha256 != record.request_sha256
        || json_string(&v.pins)? != json_string(&record.pins)?
        || Some(&v.source_profile) != record.source_profile.as_ref()
        || Some(&v.source_family) != record.source_family.as_ref()
        || Some(&v.verifier_contract) != record.verifier_contract.as_ref()
        || Some(v.rows) != record.original_rows
        || v.receipt_sha256 != hash(json_string(&unsigned)?.as_bytes())
    {
        return Err(Error::Conflict);
    }
    bounded(v)
}
async fn get_table(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    index: &HistoricalHistoryIndex,
    ordinal: u32,
) -> Result<HistoricalHistoryTable> {
    let expected = index.tables.get(ordinal as usize).ok_or(Error::NotFound)?;
    let raw = cipher::get(
        tx,
        keys,
        &index.owner.locator(),
        index.import_id,
        &table_stream(ordinal),
        0,
    )
    .await?;
    if raw.len() as u64 != expected.size || hash(&raw) != expected.sha256 {
        return Err(Error::Conflict);
    }
    let table: HistoricalHistoryTable = serde_json::from_slice(&raw).map_err(storage_error)?;
    if table.ordinal != ordinal {
        return Err(Error::Conflict);
    }
    bounded(table)
}
async fn get_row(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    index: &HistoricalHistoryIndex,
    table: &HistoricalHistoryTable,
    position: u64,
) -> Result<HistoricalHistoryRow> {
    if position == 0 || position > table.rows {
        return Err(Error::NotFound);
    }
    let raw = cipher::get(
        tx,
        keys,
        &index.owner.locator(),
        index.import_id,
        &row_stream(table.ordinal),
        i64::try_from(position - 1).map_err(|_| Error::NotFound)?,
    )
    .await?;
    let row: HistoricalHistoryRow = serde_json::from_slice(&raw).map_err(storage_error)?;
    if row.position != position
        || row.columns as usize != table.columns.len()
        || row.cell_pages.len() != table.columns.len().div_ceil(stage::CELLS_PER_PAGE)
        || !row
            .rowid
            .parse::<i64>()
            .is_ok_and(|id| id.to_string() == row.rowid)
    {
        return Err(Error::Conflict);
    }
    bounded(row)
}
async fn get_cells(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    index: &HistoricalHistoryIndex,
    table: &HistoricalHistoryTable,
    row: &HistoricalHistoryRow,
    page: u32,
) -> Result<HistoricalHistoryCellsPage> {
    let expected = row.cell_pages.get(page as usize).ok_or(Error::NotFound)?;
    let raw = cipher::get(
        tx,
        keys,
        &index.owner.locator(),
        index.import_id,
        &cells_stream(table.ordinal, row.position),
        i64::from(page),
    )
    .await?;
    if raw.len() as u64 != expected.size || hash(&raw) != expected.sha256 {
        return Err(Error::Conflict);
    }
    let cells: HistoricalHistoryCellsPage = serde_json::from_slice(&raw).map_err(storage_error)?;
    let start = page as usize * stage::CELLS_PER_PAGE;
    if cells.table_ordinal != table.ordinal
        || cells.position != row.position
        || cells.rowid != row.rowid
        || cells.page_index != page
        || cells.cells.len() != (table.columns.len() - start).min(stage::CELLS_PER_PAGE)
        || cells
            .cells
            .iter()
            .enumerate()
            .any(|(i, c)| c.column_index as usize != start + i)
    {
        return Err(Error::Conflict);
    }
    bounded(cells)
}
struct TableDigest {
    digest: Sha256,
    bytes: u64,
}
struct CopyRow<'a> {
    file: &'a mut std::fs::File,
    digest: &'a mut TableDigest,
}
impl Write for CopyRow<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.digest.digest.update(&bytes[..n]);
        self.digest.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Store {
    /// Once-only atomic derivation. Existing original payload and activation receipts are not rewritten.
    pub async fn build_historical_history_index(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        input: BuildHistoricalHistoryIndex,
        private_staging: &Path,
    ) -> Result<HistoricalHistoryIndex> {
        if input.request_id.is_nil()
            || input.expected_import_revision <= 0
            || input.expected_limit_revision <= 0
        {
            return Err(invalid());
        }
        let owner = authority.owner();
        let identity = self.identity();
        let current = self
            .historical_import_record(authority_ref(&authority), key_id, import)
            .await?;
        if current.state == "receiving" {
            return Err(Error::Conflict);
        }
        // Replays require current read authority and return the original committed receipt.
        {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let mut locked = identity
                .lock_quota_in_transaction(&mut tx)
                .await
                .map_err(identity_error)?;
            let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
            let current = record(sqltx(&mut locked)?, import, &owner).await?;
            if has_index(sqltx(&mut locked)?, &owner, import).await? {
                let result = root(sqltx(&mut locked)?, keys, &current).await?;
                original_build_request(&result, &owner, import, &input)?;
                recheck(&mut locked, &permit).await?;

                tx.commit().await.map_err(storage_error)?;
                return Ok(result);
            }
            recheck(&mut locked, &permit).await?;

            tx.commit().await.map_err(storage_error)?;
        }
        if current.revision != input.expected_import_revision {
            return Err(Error::Conflict);
        }
        self.historical_import_authority(authority_ref(&authority), key_id, ContentAction::Write)
            .await?;
        let limit = self
            .historical_import_limit(authority_ref(&authority), key_id)
            .await?;
        if limit.revision != input.expected_limit_revision {
            return Err(Error::Conflict);
        }
        let directory = tempfile::Builder::new()
            .prefix("historical-browser-")
            .tempdir_in(private_staging)
            .map_err(storage_error)?;
        let mut inputs = std::collections::BTreeMap::new();
        for kind in HistoricalStream::all() {
            let declaration = current
                .files
                .iter()
                .find(|d| d.stream == kind)
                .ok_or(Error::Conflict)?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1).custom_flags(0x00200000);
            }
            let mut file = options
                .open(directory.path().join(kind.locator()))
                .map_err(storage_error)?;
            let mut digest = Sha256::new();
            let mut size = 0;
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
        let archive = verifier::verify(
            UntrustedHistoricalStreams {
                quarantine_manifest: take(HistoricalStream::QuarantineManifest)?,
                quarantine_database: take(HistoricalStream::QuarantineDatabase)?,
                mapping: take(HistoricalStream::Mapping)?,
                preview: take(HistoricalStream::Preview)?,
                cold_target_snapshot: take(HistoricalStream::ColdTargetSnapshot)?,
            },
            checked_pins(&current.pins)?,
        )
        .await
        .map_err(|_| {
            Error::Validation("Original historical archive verification refused".into())
        })?;
        if Some(&archive.provenance().source_profile) != current.source_profile.as_ref()
            || Some(&archive.provenance().source_family) != current.source_family.as_ref()
            || Some(archive.provenance().original_rows) != current.original_rows
            || Some(archive.provenance().verifier_contract) != current.verifier_contract.as_deref()
            || archive.provenance().verifier_contract
                != preview_format_limits(current.preview_format.as_deref())?.1
        {
            return Err(Error::Conflict);
        }
        let mut stage = HistoryStage::new(
            directory.path(),
            limit.limit_bytes.saturating_sub(limit.reserved_bytes),
        )?;
        let tables = archive.tables();
        let mut row_file = HistoryStage::row_file(directory.path())?;
        let mut cursor = archive.browser_rows().await.map_err(|_| Error::Conflict)?;
        let mut next = cursor.next().await.map_err(|_| Error::Conflict)?;
        for (ordinal, source) in tables.iter().enumerate() {
            let mut table = HistoricalHistoryTable {
                ordinal: ordinal as u32,
                name: source.name.clone(),
                columns: source.columns.clone(),
                rows: source.rows,
                canonical_bytes: 0,
                canonical_sha256: String::new(),
            };
            let mut observed = 0;
            let mut digest = TableDigest {
                digest: Sha256::new(),
                bytes: 0,
            };
            while next
                .as_ref()
                .is_some_and(|row| row.source_table == source.name)
            {
                let row = next.take().ok_or(Error::Conflict)?;
                observed += 1;
                if row.position != observed || observed > source.rows {
                    return Err(Error::Conflict);
                }
                row_file.set_len(0).map_err(storage_error)?;
                row_file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
                cursor
                    .write_current_to(&mut CopyRow {
                        file: &mut row_file,
                        digest: &mut digest,
                    })
                    .await
                    .map_err(|_| Error::Conflict)?;
                if row_file.metadata().map_err(storage_error)?.len() != row.canonical_bytes {
                    return Err(Error::Conflict);
                }
                stage.row(
                    &mut row_file,
                    &table,
                    row.position,
                    &row.source_rowid.to_string(),
                    &row.canonical_sha256,
                )?;
                next = cursor.next().await.map_err(|_| Error::Conflict)?;
            }
            if observed != source.rows {
                return Err(Error::Conflict);
            }
            table.canonical_bytes = digest.bytes;
            table.canonical_sha256 = format!("{:x}", digest.digest.finalize());
            let pin = stage.put_json(&table_stream(table.ordinal), 0, &table)?;
            stage.table_pins.push(pin);
        }
        if next.is_some() || stage.rows != archive.provenance().original_rows {
            return Err(Error::Conflict);
        }
        cursor.close().await.map_err(|_| Error::Conflict)?;
        let key_id = writer(keys, key_id)?;
        let request_sha256 = hash(json_string(&(INDEX_FORMAT, &owner, import, &input))?.as_bytes());
        stage.rewind()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        crate::server_keys::cipher_writer(sqltx(&mut locked)?, key_id).await?;
        let retained = record(sqltx(&mut locked)?, import, &owner).await?;
        if has_index(sqltx(&mut locked)?, &owner, import).await? {
            let original = root(sqltx(&mut locked)?, keys, &retained).await?;
            original_build_request(&original, &owner, import, &input)?;
            recheck(&mut locked, &permit).await?;

            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        if retained.revision != current.revision
            || retained.revision != input.expected_import_revision
            || retained.request_sha256 != current.request_sha256
        {
            return Err(Error::Conflict);
        }
        let raw_original: String =
            sqlx::query_scalar("SELECT payload FROM historical_imports WHERE id=$1 AND owner=$2")
                .bind(import.to_string())
                .bind(owner.locator())
                .fetch_one(&mut **sqltx(&mut locked)?)
                .await
                .map_err(storage_error)?;
        let mut result = HistoricalHistoryIndex {
            format: INDEX_FORMAT.into(),
            import_id: import,
            owner: owner.clone(),
            request_id: input.request_id,
            request_sha256,
            original_record_sha256: hash(raw_original.as_bytes()),
            original_intake_sha256: retained.request_sha256.clone(),
            pins: retained.pins.clone(),
            source_profile: archive.provenance().source_profile.clone(),
            source_family: archive.provenance().source_family.clone(),
            verifier_contract: archive.provenance().verifier_contract.into(),
            tables: stage.table_pins.clone(),
            rows: stage.rows,
            cells: stage.cells,
            original_value_bytes: stage.value_bytes,
            index_sha256: stage.digest(),
            reserved_bytes: stage.reserved_bytes,
            built_at: now(),
            receipt_sha256: String::new(),
        };
        // Header charge has fixed-size digest/counter fields; calculate using its final encoded size.
        let preliminary = json_string(&result)?;
        result.reserved_bytes = result
            .reserved_bytes
            .checked_add(preliminary.len() as u64 * 2 + 8192)
            .ok_or(Error::BudgetExceeded)?;
        result.receipt_sha256 = hash(json_string(&result)?.as_bytes());
        let header = Zeroizing::new(json_string(&bounded(result.clone())?)?.into_bytes());
        reserve(
            &mut locked,
            &authority,
            &permit,
            key_id.instance,
            input.expected_limit_revision,
            &format!("{import}:history-index"),
            result.reserved_bytes,
            &result.index_sha256,
        )
        .await?;
        let mut copied = Sha256::new();
        for _ in 0..stage.records {
            let mut length = [0; 4];
            stage.file.read_exact(&mut length).map_err(storage_error)?;
            copied.update(length);
            let length = u32::from_be_bytes(length) as usize;
            if length > 4096 {
                return Err(Error::Conflict);
            }
            let mut raw = vec![0; length];
            stage.file.read_exact(&mut raw).map_err(storage_error)?;
            copied.update(&raw);
            let item: StageRecord = serde_json::from_slice(&raw).map_err(storage_error)?;
            if item.size > CHUNK_BYTES as u64
                || !item.stream.starts_with("history:")
                || item.stream == HEADER
            {
                return Err(Error::Conflict);
            }
            let mut bytes = Zeroizing::new(vec![0; item.size as usize]);
            stage.file.read_exact(&mut bytes).map_err(storage_error)?;
            copied.update(bytes.as_slice());
            if hash(&bytes) != item.sha256 {
                return Err(Error::Conflict);
            }
            cipher::put(
                sqltx(&mut locked)?,
                keys,
                key_id,
                &owner.locator(),
                import,
                &item.stream,
                item.index,
                &bytes,
            )
            .await?;
        }
        let mut extra = [0];
        if stage.file.read(&mut extra).map_err(storage_error)? != 0
            || format!("{:x}", copied.finalize()) != result.index_sha256
        {
            return Err(Error::Conflict);
        }
        cipher::put(
            sqltx(&mut locked)?,
            keys,
            key_id,
            &owner.locator(),
            import,
            HEADER,
            0,
            &header,
        )
        .await?;
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn historical_history_index(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
    ) -> Result<HistoricalHistoryIndex> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        let value = root(sqltx(&mut locked)?, keys, &record).await?;
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn historical_history_tables(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<HistoricalHistoryTablesPage> {
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
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        let index = root(sqltx(&mut locked)?, keys, &record).await?;
        let end = (offset as usize + limit as usize).min(index.tables.len());
        let mut tables = vec![];
        for ordinal in offset as usize..end {
            tables.push(get_table(sqltx(&mut locked)?, keys, &index, ordinal as u32).await?);
        }
        let value = bounded(HistoricalHistoryTablesPage {
            index_sha256: index.index_sha256,
            offset,
            next_offset: (end < index.tables.len()).then_some(end as u32),
            tables,
        })?;
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "historical_history_rows: Keep historical read authority, import/table/cell/page coordinates and native key custody explicit"
    )]
    pub async fn historical_history_rows(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        table: u32,
        offset: u64,
        limit: u32,
    ) -> Result<HistoricalHistoryRowsPage> {
        if limit == 0 || limit > 25 {
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
        let index = root(sqltx(&mut locked)?, keys, &record).await?;
        let source = get_table(sqltx(&mut locked)?, keys, &index, table).await?;
        let end = offset
            .checked_add(limit as u64)
            .ok_or_else(invalid)?
            .min(source.rows);
        let mut rows = vec![];
        for position in offset..end {
            rows.push(get_row(sqltx(&mut locked)?, keys, &index, &source, position + 1).await?);
        }
        let value = bounded(HistoricalHistoryRowsPage {
            index_sha256: index.index_sha256,
            table_ordinal: table,
            offset,
            next_offset: (end < source.rows).then_some(end),
            rows,
        })?;
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "historical_history_cells: Keep historical read authority, import/table/cell/page coordinates and native key custody explicit"
    )]
    pub async fn historical_history_cells(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        table: u32,
        position: u64,
        page: u32,
    ) -> Result<HistoricalHistoryCellsPage> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        let index = root(sqltx(&mut locked)?, keys, &record).await?;
        let table = get_table(sqltx(&mut locked)?, keys, &index, table).await?;
        let row = get_row(sqltx(&mut locked)?, keys, &index, &table, position).await?;
        let value = get_cells(sqltx(&mut locked)?, keys, &index, &table, &row, page).await?;
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "historical_history_value_page: Keep historical read authority, import/table/cell/page coordinates and native key custody explicit"
    )]
    pub async fn historical_history_value_page(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        import: Uuid,
        table: u32,
        position: u64,
        column: u32,
        page: u32,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let record = record(sqltx(&mut locked)?, import, &authority.owner()).await?;
        let index = root(sqltx(&mut locked)?, keys, &record).await?;
        let table = get_table(sqltx(&mut locked)?, keys, &index, table).await?;
        let row = get_row(sqltx(&mut locked)?, keys, &index, &table, position).await?;
        let cells = get_cells(
            sqltx(&mut locked)?,
            keys,
            &index,
            &table,
            &row,
            column / stage::CELLS_PER_PAGE as u32,
        )
        .await?;
        let value = cells
            .cells
            .iter()
            .find(|c| c.column_index == column)
            .ok_or(Error::NotFound)?;
        let pages = match &value.value {
            HistoricalHistoryCell::TextBytes { pages, .. }
            | HistoricalHistoryCell::Blob { pages, .. } => pages,
            _ => return Err(Error::NotFound),
        };
        let expected = pages.get(page as usize).ok_or(Error::NotFound)?;
        let bytes = cipher::get(
            sqltx(&mut locked)?,
            keys,
            &authority.owner().locator(),
            import,
            &values_stream(table.ordinal, position, column),
            i64::from(page),
        )
        .await?;
        if bytes.len() > CHUNK_BYTES
            || bytes.len() as u64 != expected.size
            || hash(&bytes) != expected.sha256
        {
            return Err(Error::Conflict);
        }
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(bytes)
    }
}
