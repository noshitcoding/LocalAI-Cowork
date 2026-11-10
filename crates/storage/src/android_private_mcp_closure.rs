//! Positive original stdio-session closure, separate from tool outcome or current authority.
use super::*;

pub struct PrivateMcpClosure {
    run: Uuid,
    entity: Uuid,
    revision: i64,
    source: Value,
    source_version: String,
}

async fn original_source(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    c: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let checkpoint: Value = parse(
        &c.try_get::<Option<String>, _>("checkpoint")
            .map_err(storage_error)?
            .ok_or(Error::UncertainEffect)?,
    )?;
    if checkpoint["transport"] != "stdio" {
        return Err(Error::UncertainEffect);
    }
    let entity = id(c, "entity_id")?;
    // Authenticate original catalog ciphertext before preserving the original source row.
    let definitions: Value = serde_json::from_slice(&workspace::open_value(
        keys,
        r,
        &format!("mcp-catalog:{entity}"),
        &parse(
            &c.try_get::<String, _>("definitions")
                .map_err(storage_error)?,
        )?,
    )?)
    .map_err(storage_error)?;
    if Catalog::from_definitions(
        serde_json::from_value(definitions.clone()).map_err(storage_error)?,
    )
    .map_err(|_| Error::Forbidden)?
    .sha256()
        != c.try_get::<String, _>("catalog_sha")
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let nonce = crate::android_personal::wire_uuid(&checkpoint["isolated"], "nonce")?;
    let container=sqlx::query("SELECT * FROM android_private_containers WHERE id=$1 AND run_id=$2 AND owner_id=$3 AND project_id=$4 AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL").bind(nonce.to_string()).bind(id(r,"id")?.to_string()).bind(id(r,"owner_id")?.to_string()).bind(id(r,"project_id")?.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let binding: Value = parse(
        &container
            .try_get::<String, _>("original_binding")
            .map_err(storage_error)?,
    )?;
    if checkpoint["isolated"]["container_id"]
        != json!(
            container
                .try_get::<Option<String>, _>("container_id")
                .map_err(storage_error)?
        )
        || checkpoint["isolated"]["binding_sha256"] != canonical_hash(&binding)
    {
        return Err(Error::Forbidden);
    }
    Ok(
        json!({"format":"android-private-mcp-source/1","run":id(r,"id")?,"owner":id(r,"owner_id")?,"project":id(r,"project_id")?,"entity":entity,"revision":c.try_get::<i64,_>("revision").map_err(storage_error)?,"original_lease":id(&container,"lease_id")?,"binding_sha256":c.try_get::<String,_>("binding_sha").map_err(storage_error)?,"catalog_sha256":c.try_get::<String,_>("catalog_sha").map_err(storage_error)?,"definitions":definitions,"protocol":c.try_get::<String,_>("protocol").map_err(storage_error)?,"checkpoint":checkpoint,"container_nonce":nonce,"container_binding":binding,"container_creation_sha256":container.try_get::<String,_>("creation_sha").map_err(storage_error)?}),
    )
}

async fn witness(tx: &mut Transaction<'_, Any>, source: &Value) -> Result<Value> {
    let c=sqlx::query("SELECT * FROM android_private_containers WHERE id=$1 AND run_id=$2 AND lease_id=$3 AND owner_id=$4 AND project_id=$5 AND original_binding=$6 AND creation_sha=$7 AND create_dispatch_admitted=1 AND original_unsent_receipt IS NULL AND state='removed'").bind(source["container_nonce"].as_str().ok_or(Error::Forbidden)?).bind(source["run"].as_str().ok_or(Error::Forbidden)?).bind(source["original_lease"].as_str().ok_or(Error::Forbidden)?).bind(source["owner"].as_str().ok_or(Error::Forbidden)?).bind(source["project"].as_str().ok_or(Error::Forbidden)?).bind(json_string(&source["container_binding"])?).bind(source["container_creation_sha256"].as_str().ok_or(Error::Forbidden)?).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
    let w: Value = parse(
        &c.try_get::<Option<String>, _>("termination_witness")
            .map_err(storage_error)?
            .ok_or(Error::UncertainEffect)?,
    )?;
    if w["removed"] != true
        || w["nonce"] != source["container_nonce"]
        || w["engine_id"] != source["container_binding"]["engine_id"]
        || w["original_id"] != source["checkpoint"]["isolated"]["container_id"]
        || w["stop"]["nonce"] != w["nonce"]
        || w["stop"]["engine_id"] != w["engine_id"]
        || w["stop"]["original_id"] != w["original_id"]
    {
        return Err(Error::UncertainEffect);
    }
    for value in [&w, &w["stop"]] {
        if value["running"] != false
            || value["paused"] != false
            || value["restarting"] != false
            || value["pid"] != 0
        {
            return Err(Error::UncertainEffect);
        }
    }
    Ok(w)
}

pub(crate) async fn verified(
    tx: &mut Transaction<'_, Any>,
    r: &AnyRow,
    c: &AnyRow,
    keys: &dyn KeyStore,
) -> Result<Value> {
    let closure=sqlx::query("SELECT * FROM android_private_mcp_closures WHERE run_id=$1 AND entity_id=$2 AND session_revision=$3 AND state='closed'").bind(id(r,"id")?.to_string()).bind(id(c,"entity_id")?.to_string()).bind(c.try_get::<i64,_>("revision").map_err(storage_error)?).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::UncertainEffect)?;
    let source: Value = serde_json::from_slice(
        &workspace::version_bytes(
            tx,
            r,
            &closure
                .try_get::<String, _>("source_version_id")
                .map_err(storage_error)?,
            keys,
        )
        .await?,
    )
    .map_err(storage_error)?;
    if source != original_source(tx, r, c, keys).await?
        || source["original_lease"]
            != closure
                .try_get::<String, _>("original_lease_id")
                .map_err(storage_error)?
        || source["checkpoint"]["connection_id"]
            != closure
                .try_get::<String, _>("connection_id")
                .map_err(storage_error)?
        || source["checkpoint"]["epoch"]
            != closure
                .try_get::<i64, _>("connection_epoch")
                .map_err(storage_error)?
        || source["binding_sha256"]
            != closure
                .try_get::<String, _>("binding_sha256")
                .map_err(storage_error)?
        || canonical_hash(&source["checkpoint"])
            != closure
                .try_get::<String, _>("checkpoint_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let receipt: Value = serde_json::from_slice(
        &workspace::version_bytes(
            tx,
            r,
            &closure
                .try_get::<Option<String>, _>("receipt_version_id")
                .map_err(storage_error)?
                .ok_or(Error::UncertainEffect)?,
            keys,
        )
        .await?,
    )
    .map_err(storage_error)?;
    if receipt["format"] != "android-private-mcp-closure/1"
        || receipt["source_sha256"] != canonical_hash(&source)
        || receipt["physical_witness"] != witness(tx, &source).await?
        || receipt["original_effects_settled"] != false
    {
        return Err(Error::Forbidden);
    }
    Ok(receipt)
}

pub(crate) async fn quiescent(tx: &mut Transaction<'_, Any>, run: Uuid) -> Result<()> {
    for query in [
        "SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND state!='received'",
        "SELECT COUNT(*) FROM mcp_async_tasks WHERE owner_kind='personal' AND run_id=$1 AND (pending_operation_id IS NOT NULL OR sealed_final_result IS NULL OR state IN('working','input_required','unknown','expired'))",
        "SELECT COUNT(*) FROM mcp_task_interactions i JOIN mcp_async_tasks t ON t.id=i.task_record_id WHERE t.owner_kind='personal' AND t.run_id=$1 AND i.state NOT IN('responded','expired')",
    ] {
        let n: i64 = sqlx::query_scalar(query)
            .bind(run.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if n != 0 {
            return Err(Error::UncertainEffect);
        }
    }
    Ok(())
}

impl Store {
    #[expect(
        clippy::too_many_arguments,
        reason = "android_private_mcp_prepare_close: Keep original run/lease/entity revision, connection epoch and closure key custody explicit"
    )]
    pub async fn android_private_mcp_prepare_close(
        &self,
        run: Uuid,
        lease: Uuid,
        entity: Uuid,
        revision: i64,
        connection_id: &str,
        epoch: u64,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<PrivateMcpClosure> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = row(&mut tx, run).await?;
        let c=sqlx::query("SELECT * FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND revision=$3 AND state IN('ready','admitted','closed')").bind(run.to_string()).bind(entity.to_string()).bind(revision).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let source = original_source(&mut tx, &r, &c, keys).await?;
        if source["original_lease"] != lease.to_string()
            || source["checkpoint"]["connection_id"] != connection_id
            || source["checkpoint"]["epoch"] != epoch
        {
            return Err(Error::Forbidden);
        }
        let old=sqlx::query("SELECT source_version_id FROM android_private_mcp_closures WHERE run_id=$1 AND entity_id=$2 AND session_revision=$3").bind(run.to_string()).bind(entity.to_string()).bind(revision).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let version = if let Some(old) = old {
            let version: String = old.try_get("source_version_id").map_err(storage_error)?;
            let original: Value = serde_json::from_slice(
                &workspace::version_bytes(&mut tx, &r, &version, keys).await?,
            )
            .map_err(storage_error)?;
            if original != source {
                return Err(Error::Conflict);
            }
            version
        } else {
            let version = workspace::add_version(
                &mut tx,
                &r,
                &format!("mcp-history/{entity}/{revision}/source.json"),
                "artifact",
                json_string(&source)?.as_bytes(),
                keys,
                key_id,
                current,
            )
            .await?;
            sqlx::query("INSERT INTO android_private_mcp_closures(run_id,entity_id,session_revision,original_lease_id,connection_id,connection_epoch,binding_sha256,checkpoint_sha256,source_version_id,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'prepared',$10)").bind(run.to_string()).bind(entity.to_string()).bind(revision).bind(lease.to_string()).bind(connection_id).bind(i64::try_from(epoch).map_err(|_|Error::Forbidden)?).bind(source["binding_sha256"].as_str().ok_or(Error::Forbidden)?).bind(canonical_hash(&source["checkpoint"])).bind(&version).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
            version
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(PrivateMcpClosure {
            run,
            entity,
            revision,
            source,
            source_version: version,
        })
    }
    pub async fn android_private_mcp_finish_close(
        &self,
        permit: PrivateMcpClosure,
        keys: &dyn KeyStore,
        key_id: KeyId,
    ) -> Result<Value> {
        let key_id = workspace::writer_cipher(keys, key_id)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r = row(&mut tx, permit.run).await?;
        let c=sqlx::query("SELECT * FROM android_private_mcp_sessions WHERE run_id=$1 AND entity_id=$2 AND revision=$3 AND state IN('ready','admitted','closed')").bind(permit.run.to_string()).bind(permit.entity.to_string()).bind(permit.revision).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        if original_source(&mut tx, &r, &c, keys).await? != permit.source {
            return Err(Error::Conflict);
        }
        if c.try_get::<String, _>("state").map_err(storage_error)? == "closed" {
            let receipt = verified(&mut tx, &r, &c, keys).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let physical = witness(&mut tx, &permit.source).await?;
        let receipt = json!({"format":"android-private-mcp-closure/1","source_sha256":canonical_hash(&permit.source),"physical_witness":physical,"original_effects_settled":false,"closed_at":current});
        let version = workspace::add_version(
            &mut tx,
            &r,
            &format!(
                "mcp-history/{}/{}/receipt.json",
                permit.entity, permit.revision
            ),
            "artifact",
            json_string(&receipt)?.as_bytes(),
            keys,
            key_id,
            current,
        )
        .await?;
        if sqlx::query("UPDATE android_private_mcp_closures SET receipt_version_id=$1,state='closed' WHERE run_id=$2 AND entity_id=$3 AND session_revision=$4 AND state='prepared' AND source_version_id=$5 AND receipt_version_id IS NULL").bind(version).bind(permit.run.to_string()).bind(permit.entity.to_string()).bind(permit.revision).bind(permit.source_version).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        if sqlx::query("UPDATE android_private_mcp_sessions SET state='closed' WHERE run_id=$1 AND entity_id=$2 AND revision=$3 AND state IN('ready','admitted') AND checkpoint=$4").bind(permit.run.to_string()).bind(permit.entity.to_string()).bind(permit.revision).bind(json_string(&permit.source["checkpoint"])?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub(crate) async fn android_private_mcp_verify_closures(
        &self,
        tx: &mut Transaction<'_, Any>,
        r: &AnyRow,
        keys: &dyn KeyStore,
    ) -> Result<()> {
        let rows=sqlx::query("SELECT * FROM android_private_mcp_sessions WHERE run_id=$1 AND state='closed' ORDER BY entity_id LIMIT 129").bind(id(r,"id")?.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.len() > 128 {
            return Err(Error::BudgetExceeded);
        }
        for c in rows {
            verified(tx, r, &c, keys).await?;
        }
        Ok(())
    }
}
