//! Explicit operator engine registration; a restart never reactivates or replaces an old slot.
use super::*;
fn checked(value: &Value) -> Result<(String, String, Uuid, Option<String>)> {
    if value["format"] != "android-private-workflow-engine/1" {
        return Err(device_invalid());
    }
    let kind = value["engine"]
        .as_str()
        .filter(|v| matches!(*v, "workflow" | "crew"))
        .ok_or_else(device_invalid)?
        .to_owned();
    let instance = crate::android_personal::wire_uuid(value, "instance_id")?;
    let runner = value
        .get("runner_binding_sha256")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_owned)
                .ok_or_else(device_invalid)
        })
        .transpose()?;
    if kind == "crew"
        && (runner.is_none()
            || value["crewai_version"].as_str().is_none_or(str::is_empty)
            || value["crewai_source_sha256"]
                .as_str()
                .is_none_or(|v| v.len() != 64)
            || value["bridge_sha256"]
                .as_str()
                .is_none_or(|v| v.len() != 64)
            || value["image"]
                .as_str()
                .is_none_or(|v| !v.contains("@sha256:")))
    {
        return Err(device_invalid());
    }
    let slot = canonical_hash(
        &json!({"format":"android-private-workflow-engine-slot/1","engine":kind,"instance_id":instance,"host_id":value.get("host_id"),"engine_id":value.get("engine_id")}),
    );
    Ok((slot, kind, instance, runner))
}
fn stamp(row: &AnyRow) -> Result<Value> {
    Ok(
        json!({"format":"android-private-workflow-engine-stamp/1","slot":row.try_get::<String,_>("slot").map_err(storage_error)?,"engine":row.try_get::<String,_>("engine").map_err(storage_error)?,"instance_id":row.try_get::<String,_>("instance_id").map_err(storage_error)?,"revision":row.try_get::<i64,_>("revision").map_err(storage_error)?,"config_sha256":row.try_get::<String,_>("config_sha256").map_err(storage_error)?,"runner_binding_sha256":row.try_get::<Option<String>,_>("runner_binding_sha256").map_err(storage_error)?}),
    )
}
pub(crate) async fn validate(tx: &mut Transaction<'_, Any>, original: &Value) -> Result<()> {
    let slot = original["slot"].as_str().ok_or(Error::Forbidden)?;
    let r =
        sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1 AND active=1")
            .bind(slot)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
    if stamp(&r)? != *original {
        return Err(Error::Forbidden);
    }
    let body: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if canonical_hash(&body)
        != r.try_get::<String, _>("config_sha256")
            .map_err(storage_error)?
        || checked(&body)?.0 != slot
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn current(
    tx: &mut Transaction<'_, Any>,
    kind: &str,
    runner: Option<&Value>,
) -> Result<Value> {
    let sha = runner.map(canonical_hash);
    let rows=sqlx::query("SELECT * FROM android_private_workflow_engines WHERE engine=$1 AND active=1 AND ($2 IS NULL OR runner_binding_sha256=$2) ORDER BY slot LIMIT 2").bind(kind).bind(sha).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() != 1 {
        return Err(Error::Validation(
            "private_workflow_engine_not_available".into(),
        ));
    }
    let s = stamp(&rows[0])?;
    validate(tx, &s).await?;
    Ok(s)
}
impl Store {
    pub async fn android_workflow_engine_register(&self, config: &Value) -> Result<Value> {
        let (slot, kind, instance, runner) = checked(config)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let old = sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1")
            .bind(&slot)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(row) = old {
            let view = json!({"stamp":stamp(&row)?,"active":row.try_get::<i64,_>("active").map_err(storage_error)?==1,"configured_matches":row.try_get::<String,_>("config_sha256").map_err(storage_error)?==canonical_hash(config)});
            tx.commit().await.map_err(storage_error)?;
            return Ok(view);
        }
        sqlx::query("INSERT INTO android_private_workflow_engines(slot,engine,instance_id,runner_binding_sha256,revision,config_sha256,active,payload) VALUES($1,$2,$3,$4,1,$5,1,$6)").bind(&slot).bind(kind).bind(instance.to_string()).bind(runner).bind(canonical_hash(config)).bind(json_string(config)?).execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1")
            .bind(slot)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let view = json!({"stamp":stamp(&row)?,"active":true,"configured_matches":true});
        tx.commit().await.map_err(storage_error)?;
        Ok(view)
    }
    pub async fn android_workflow_engine_inspect(&self, config: &Value) -> Result<Value> {
        let (slot, _, _, _) = checked(config)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let r = sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1")
            .bind(slot)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let out = json!({"stamp":stamp(&r)?,"active":r.try_get::<i64,_>("active").map_err(storage_error)?==1,"configured_matches":r.try_get::<String,_>("config_sha256").map_err(storage_error)?==canonical_hash(config)});
        tx.commit().await.map_err(storage_error)?;
        Ok(out)
    }
    pub async fn android_workflow_engine_change(
        &self,
        config: &Value,
        expected: i64,
        request: Uuid,
        active: bool,
    ) -> Result<Value> {
        if expected <= 0 || request.is_nil() {
            return Err(device_invalid());
        }
        let (slot, kind, instance, runner) = checked(config)?;
        let body = json!({"format":"android-private-workflow-engine-change/1","config":config,"expected_revision":expected,"request_id":request,"active":active});
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        crate::server_keys::maintenance_fence(&mut tx).await?;
        if let Some(old)=sqlx::query("SELECT request_sha256,original_receipt FROM android_private_workflow_engine_operations WHERE slot=$1 AND request_id=$2").bind(&slot).bind(request.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if old.try_get::<String,_>("request_sha256").map_err(storage_error)?!=canonical_hash(&body){return Err(Error::Conflict)}let value:Value=parse(&old.try_get::<String,_>("original_receipt").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(value)}
        let changed=sqlx::query("UPDATE android_private_workflow_engines SET revision=revision+1,active=$1,config_sha256=CASE WHEN $1=1 THEN $2 ELSE config_sha256 END,payload=CASE WHEN $1=1 THEN $3 ELSE payload END,runner_binding_sha256=CASE WHEN $1=1 THEN $4 ELSE runner_binding_sha256 END WHERE slot=$5 AND revision=$6 AND instance_id=$7 AND engine=$8").bind(i64::from(active)).bind(canonical_hash(config)).bind(json_string(config)?).bind(runner).bind(&slot).bind(expected).bind(instance.to_string()).bind(kind).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
        if changed != 1 {
            return Err(Error::Conflict);
        }
        let r = sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1")
            .bind(&slot)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let receipt = json!({"format":"android-private-workflow-engine-receipt/1","request_id":request,"request_sha256":canonical_hash(&body),"original_revision":expected,"active":active,"stamp":stamp(&r)?});
        sqlx::query("INSERT INTO android_private_workflow_engine_operations(slot,request_id,expected_revision,request_sha256,original_receipt) VALUES($1,$2,$3,$4,$5)").bind(slot).bind(request.to_string()).bind(expected).bind(canonical_hash(&body)).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_workflow_engine_admission(
        &self,
        a: &Principal,
        project: Uuid,
        config: &Value,
    ) -> Result<Value> {
        let (slot, _, _, _) = checked(config)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.android_personal_permit(&mut tx, a, project, ContentAction::Execute)
            .await?;
        let r=sqlx::query("SELECT * FROM android_private_workflow_engines WHERE slot=$1 AND active=1 AND config_sha256=$2").bind(slot).bind(canonical_hash(config)).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let stamp = stamp(&r)?;
        validate(&mut tx, &stamp).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(stamp)
    }
}
