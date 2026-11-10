//! Persisted admission activation and operator-attested runtime coverage.
use super::*;
use cowork_contracts::enterprise_execution::ExecutionGenerationReceipt;

pub(super) async fn register(
    tx: &mut Transaction<'_, Postgres>,
    enabled: bool,
) -> anyhow::Result<()> {
    if !enabled {
        let configured:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM management_execution_activation) OR EXISTS(SELECT 1 FROM management_execution_ceiling_revisions)")
            .fetch_one(&mut **tx).await?;
        anyhow::ensure!(
            !configured,
            "execution admission is already configured; refusing a disabled management replica"
        );
        return Ok(());
    }
    // Serialize activation against instance creation, including older replicas.
    sqlx::query("LOCK TABLE management_instances IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut **tx)
        .await?;
    let uncovered:i64=sqlx::query_scalar("SELECT count(*) FROM management_instances i WHERE i.state<>'deleted' AND (NOT i.execution_required OR (i.state='ready' AND (i.execution_evidence IS NULL OR i.execution_evidence->>'active_epoch' IS DISTINCT FROM i.execution_epoch::text OR i.execution_evidence->'runtime'->>'version' IS DISTINCT FROM '1')))")
        .fetch_one(&mut **tx).await?;
    anyhow::ensure!(uncovered==0,"execution admission activation requires verified enrollment of {uncovered} existing instances");
    let unbound:i64=sqlx::query_scalar("SELECT count(*) FROM management_operations o JOIN management_instances i ON i.id=o.instance_id WHERE i.execution_required AND o.state IN ('running','reconciling') AND (NOT o.execution_runtime_required OR o.execution_binding IS NULL)")
        .fetch_one(&mut **tx).await?;
    anyhow::ensure!(
        unbound == 0,
        "execution admission activation has unbound operations requiring reconciliation"
    );
    sqlx::query("INSERT INTO management_execution_activation(singleton,contract_version) VALUES(true,1) ON CONFLICT DO NOTHING").execute(&mut **tx).await?;
    Ok(())
}

pub(super) fn operator_contract(required: bool, version: Option<u32>) -> Result<(), ApiError> {
    if required && version != Some(1) {
        return Err(ApiError::Unavailable(
            "execution admission requires provisioner contract version 1".into(),
        ));
    }
    Ok(())
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    row: &sqlx::postgres::PgRow,
    succeeded: bool,
    result: &Value,
) -> Result<(), ApiError> {
    if !row.try_get::<bool, _>("execution_runtime_required")? {
        return Ok(());
    }
    if row.try_get::<String, _>("action")? == "delete" && succeeded {
        // Deletion has its own scoped absence proof; it never claims a runtime.
        return Ok(());
    }
    let receipt: ExecutionGenerationReceipt =
        serde_json::from_value(result["execution"].clone())
            .map_err(|_| invalid("execution runtime receipt required"))?;
    match &receipt.runtime {
        Some(runtime) => {
            let image = runtime.server_image.split_once("@sha256:");
            if runtime.version != 1
                || runtime.manifest_sha256.len() != 64
                || !runtime
                    .manifest_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || !image.is_some_and(|(name, digest)| {
                    name.bytes()
                        .next()
                        .is_some_and(|b| b.is_ascii_alphanumeric())
                        && name.len() <= 512
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
                        && digest.len() == 64
                        && digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            {
                return Err(invalid("invalid execution runtime attestation"));
            }
            // The generation validator already matched the frozen binding and epoch.
            sqlx::query("UPDATE management_instances SET execution_evidence=$2 WHERE id=$1")
                .bind(row.try_get::<Uuid, _>("instance_id")?)
                .bind(&result["execution"])
                .execute(&mut **tx)
                .await?;
            append_audit(tx,row.try_get("organisation_id")?,row.try_get("actor_user_id")?,"instance.execution.runtime",row.try_get("instance_id")?,
                json!({"operation_id":row.try_get::<Uuid,_>("id")?,"execution":result["execution"]})).await?;
        }
        None if succeeded => {
            return Err(invalid(
                "verified admission runtime required for completion",
            ))
        }
        None => {
            // A failed initial provision must never manufacture coverage.
            sqlx::query("UPDATE management_instances SET execution_evidence=NULL WHERE id=$1")
                .bind(row.try_get::<Uuid, _>("instance_id")?)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}
