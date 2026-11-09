//! Dispatch CAS, original readback and explicit closure of proved unsent IO.
use super::activation::{authorize_plan_read, current_target};
use super::*;
async fn original(
    tx: &mut Transaction<'_, Any>,
    plan: &HistoricalImportPreview,
    source: &str,
) -> Result<(HistoricalFileOperation, i64, Option<String>, Option<i64>)> {
    sqlx::query("UPDATE historical_import_file_operations SET state=state WHERE preview_id=$1 AND source_id=$2 AND owner=$3").bind(plan.id.to_string()).bind(source).bind(plan.owner.locator()).execute(&mut **tx).await.map_err(storage_error)?;
    let row=sqlx::query("SELECT payload,state,request_sha256,dispatch_admitted,original_unsent_receipt,reserved_bytes FROM historical_import_file_operations WHERE preview_id=$1 AND source_id=$2 AND owner=$3").bind(plan.id.to_string()).bind(source).bind(plan.owner.locator()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    let mut operation: HistoricalFileOperation =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let target = plan
        .projects
        .iter()
        .find(|p| {
            p.project_id == operation.project_id
                && p.source
                    .files
                    .iter()
                    .zip(&p.file_ids)
                    .any(|(f, id)| f.source_id == source && *id == operation.target_id)
        })
        .ok_or(Error::Conflict)?;
    let file_index = target
        .source
        .files
        .iter()
        .position(|f| f.source_id == source)
        .ok_or(Error::Conflict)?;
    let file = &target.source.files[file_index];
    let request = HistoricalFileStageRequest {
        source_id: source.into(),
        backend_id: operation.backend_id.clone(),
        size: operation.size,
        sha256: operation.sha256.clone(),
    };
    if operation.preview_id != plan.id
        || operation.import_id != plan.import_id
        || operation.owner != plan.owner
        || operation.plan_sha256 != plan.plan_sha256
        || operation.source_id != source
        || operation.request_sha256
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
        || matches!(target.target, HistoricalProjectTarget::HistoryOnly { .. })
        || plan.history_only_file_ids.iter().any(|s| s == source)
        || operation.state != "prepared"
        || Some(&operation.order_id) != target.order_ids.last()
        || operation.name != file.name
        || operation.media_type != file.media_type
        || file.size.is_some_and(|s| s != operation.size)
        || file.sha256.as_ref().is_some_and(|s| s != &operation.sha256)
        || operation.request_sha256
            != canonical_hash(&(
                "historical-file-stage/v1",
                &plan.owner,
                plan.id,
                &plan.plan_sha256,
                &request,
                &target.project_id,
                &operation.target_id,
            ))
    {
        return Err(Error::Conflict);
    }
    operation.state = row.try_get("state").map_err(storage_error)?;
    Ok((
        operation,
        row.try_get("dispatch_admitted").map_err(storage_error)?,
        row.try_get("original_unsent_receipt")
            .map_err(storage_error)?,
        row.try_get("reserved_bytes").map_err(storage_error)?,
    ))
}
fn checked_closure(raw: &str, op: &HistoricalFileOperation) -> Result<HistoricalUnsentFileReceipt> {
    let mut receipt: HistoricalUnsentFileReceipt = parse(raw)?;
    let digest = receipt.closure_sha256.clone();
    receipt.closure_sha256.clear();
    let mut expected = op.clone();
    expected.state = "prepared".into();
    if receipt.format != "historical-original-unsent-close/1"
        || receipt.original_operation.request_sha256 != op.request_sha256
        || json_string(&receipt.original_operation)? != json_string(&expected)?
        || receipt.released_bytes != op.size
        || canonical_hash(&receipt) != digest
    {
        return Err(Error::Conflict);
    }
    receipt.closure_sha256 = digest;
    Ok(receipt)
}
impl Store {
    pub async fn historical_file_operation(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        source: &str,
    ) -> Result<HistoricalFileOperation> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let plan = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        // This is the owner's immutable original operation, not current target
        // content. Target ACL/CAS changes must not strand a proved unsent close.
        let (mut op, _, closed, _) = original(sqltx(&mut locked)?, &plan, source).await?;
        if let Some(receipt) = closed {
            checked_closure(&receipt, &op)?;
            op.state = "unsent_closed".into();
        }
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(op)
    }
    pub async fn admit_historical_file_dispatch(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        op: &HistoricalFileOperation,
    ) -> Result<HistoricalFileStagePermit> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let read = authorize(&mut locked, &authority, key_id, ContentAction::Read).await?;
        let plan = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), op.preview_id).await?;
        let (actual, dispatch, closed, _) =
            original(sqltx(&mut locked)?, &plan, &op.source_id).await?;
        let mut expected = op.clone();
        expected.state = actual.state.clone();
        if json_string(&actual)? != json_string(&expected)? || closed.is_some() {
            return Err(Error::Conflict);
        }
        let send = if dispatch == 1 {
            authorize_plan_read(&mut locked, &authority, &plan).await?;
            false
        } else if dispatch == 0 && actual.state == "prepared" && plan.state == "preview" {
            let write = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
            let target = plan
                .projects
                .iter()
                .find(|p| p.project_id == op.project_id && p.file_ids.contains(&op.target_id))
                .ok_or(Error::Conflict)?;
            let target_permit = current_target(&mut locked, &authority, &target.target).await?;
            let n=sqlx::query("UPDATE historical_import_file_operations SET dispatch_admitted=1 WHERE preview_id=$1 AND source_id=$2 AND owner=$3 AND request_sha256=$4 AND payload=$5 AND dispatch_admitted=0 AND state='prepared' AND original_receipt IS NULL AND original_unsent_receipt IS NULL").bind(op.preview_id.to_string()).bind(&op.source_id).bind(op.owner.locator()).bind(&op.request_sha256).bind(json_string(op)?).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.rows_affected();
            if n != 1 {
                return Err(Error::Conflict);
            }
            recheck(&mut locked, &target_permit).await?;
            recheck(&mut locked, &write).await?;
            true
        } else {
            return Err(Error::Conflict);
        };
        recheck(&mut locked, &read).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(HistoricalFileStagePermit {
            operation: actual,
            send,
        })
    }
    pub async fn close_historical_unsent_file(
        &self,
        authority: HistoricalAuthority<'_>,
        keys: &dyn KeyStore,
        key_id: KeyId,
        preview: Uuid,
        source: &str,
        input: CloseHistoricalUnsentFile,
    ) -> Result<HistoricalUnsentFileReceipt> {
        let identity = self.identity();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut locked = identity
            .lock_quota_in_transaction(&mut tx)
            .await
            .map_err(identity_error)?;
        let permit = authorize(&mut locked, &authority, key_id, ContentAction::Write).await?;
        let plan = load_plan(sqltx(&mut locked)?, keys, &authority.owner(), preview).await?;
        let (op, dispatch, closed, reserved) = original(sqltx(&mut locked)?, &plan, source).await?;
        if op.request_sha256 != input.expected_request_sha256 {
            return Err(Error::Conflict);
        }
        if let Some(receipt) = closed {
            let receipt = checked_closure(&receipt, &op)?;
            recheck(&mut locked, &permit).await?;

            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        if dispatch != 0 || op.state != "prepared" {
            return Err(Error::UncertainEffect);
        }
        let reserved =
            u64::try_from(reserved.ok_or(Error::Conflict)?).map_err(|_| Error::Conflict)?;
        if reserved < op.size {
            return Err(Error::Conflict);
        }
        let mut receipt = HistoricalUnsentFileReceipt {
            format: "historical-original-unsent-close/1".into(),
            original_operation: op.clone(),
            released_bytes: op.size,
            retained_reserved_bytes: reserved - op.size,
            original_charge_id: None,
            closed_at: now(),
            closure_sha256: String::new(),
        };
        let charge = if let HistoricalOwner::Organization {
            organization_id,
            creator_id,
            admission_project_id,
        } = &op.owner
        {
            let key = format!("hist:{}:file:{}:0", preview, hash(source.as_bytes()));
            let raw:String=sqlx::query_scalar("SELECT receipt FROM identity_storage_charges WHERE organization_id=$1 AND identity_id=$2 AND resource_id=$3 AND intent_key=$4 AND provenance_sha256=$5 AND bytes=$6").bind(organization_id.to_string()).bind(creator_id.to_string()).bind(admission_project_id.to_string()).bind(key).bind(&op.request_sha256).bind(reserved as i64).fetch_optional(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let charge: cowork_identity::StorageImportCharge = parse(&raw)?;
            receipt.original_charge_id = Some(charge.id);
            Some(charge)
        } else {
            None
        };
        receipt.closure_sha256 = canonical_hash(&receipt);
        let changed=sqlx::query("UPDATE historical_import_file_operations SET original_unsent_receipt=$1 WHERE preview_id=$2 AND source_id=$3 AND owner=$4 AND request_sha256=$5 AND payload=$6 AND dispatch_admitted=0 AND state='prepared' AND original_receipt IS NULL AND original_unsent_receipt IS NULL AND reserved_bytes=$7").bind(json_string(&receipt)?).bind(preview.to_string()).bind(source).bind(op.owner.locator()).bind(&op.request_sha256).bind(json_string(&op)?).bind(reserved as i64).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        if op.size > 0 {
            if let Some(charge) = charge {
                locked
                    .release_storage_in_transaction(
                        &charge,
                        op.size as i64,
                        &format!("m9-unsent:{preview}:{}", hash(source.as_bytes())),
                        &receipt.closure_sha256,
                    )
                    .await
                    .map_err(identity_error)?;
            }
            let limit = lock_limit(sqltx(&mut locked)?, key_id.instance).await?;
            let n=sqlx::query("UPDATE historical_import_limit SET reserved_bytes=reserved_bytes-$1 WHERE installation_id=$2 AND revision=$3 AND reserved_bytes=$4 AND reserved_bytes>=$1").bind(op.size as i64).bind(key_id.instance.to_string()).bind(limit.revision).bind(limit.reserved_bytes as i64).execute(&mut **sqltx(&mut locked)?).await.map_err(storage_error)?.rows_affected();
            if n != 1 {
                return Err(Error::Conflict);
            }
        }
        recheck(&mut locked, &permit).await?;

        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
