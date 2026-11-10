use super::*;
fn record(raw: &[u8]) -> Result<Value> {
    if raw.len() > 4096 {
        return Err(invalid());
    }
    let r: Value = serde_json::from_slice(raw).map_err(|_| invalid())?;
    let nonce = uuid_value(&r["nonce"])?;
    if r["format"] != 2 || r["name"] != format!("cowork-team-{nonce}") {
        return Err(Error::UncertainEffect);
    }
    Ok(r)
}
impl TeamExecutionAuthority {
    pub async fn admit_container_host(
        &self,
        approval: Uuid,
        raw: &[u8],
    ) -> Result<Option<TeamHostAdmissionRef>> {
        let mut source = record(raw)?;
        if source.get("trusted_host_admission").is_some()
            || !source["container_id"].is_null()
            || uuid_value(&source["team"])? != self.team_id()
            || uuid_value(&source["order"])? != self.order_id()
            || uuid_value(&source["run"])? != self.run_id()
        {
            return Err(Error::Conflict);
        }
        let nonce = uuid_value(&source["nonce"])?;
        let host: TeamOriginalHostBinding =
            serde_json::from_value(source["host"].clone()).map_err(|_| invalid())?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        crate::server_keys::server_admission(&mut tx, self.key_id).await?;
        approval_current(self, &mut tx, approval).await?;
        let approved:TeamToolApproval=parse(&sqlx::query_scalar::<_,String>("SELECT payload FROM team_tool_approvals WHERE id=$1 AND run_id=$2 AND lease_owner=$3").bind(approval.to_string()).bind(self.run_id().to_string()).bind(&self.lease_owner).fetch_one(&mut*tx).await.map_err(storage_error)?)?;
        let expected = serde_json::json!({"container_name":source["name"],"container_nonce":source["nonce"],"image":source["image"],"team_id":source["team"],"order_id":source["order"],"run_id":source["run"],"github":source["github"]});
        if approved.action["container"] != expected || !sha(&approved.request_sha256) {
            return Err(Error::Conflict);
        }
        let rows=sqlx::query("SELECT * FROM team_host_trust_profiles WHERE host_id=$1 AND engine_id=$2 ORDER BY active DESC,id LIMIT 65").bind(host.host_id.to_string()).bind(&host.engine_id).fetch_all(&mut*tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            sqlx::query("INSERT INTO team_container_host_admissions(nonce,run_id,team_id,order_id,host_id,engine_id,approval_id,approval_request_sha256,original_lease_owner,original_binding,source_record,source_sha256,original_created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(nonce.to_string()).bind(self.run_id().to_string()).bind(self.team_id().to_string()).bind(self.order_id().to_string()).bind(host.host_id.to_string()).bind(&host.engine_id).bind(approval.to_string()).bind(&approved.request_sha256).bind(&self.lease_owner).bind(json_string(&self.binding)?).bind(json_string(&source)?).bind(digest(&source)?).bind(db_now(&self.store,&mut tx).await?).execute(&mut*tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        }
        if rows.len() > 64 {
            return Err(Error::BudgetExceeded);
        }
        let row = rows
            .iter()
            .find(|r| r.try_get::<i64, _>("active").ok() == Some(1))
            .ok_or(Error::Forbidden)?;
        let p = registry::profile(row)?;
        if p.team_id != self.team_id()
            || p.binding != host
            || row
                .try_get::<Option<i64>, _>("retired_at")
                .map_err(storage_error)?
                .is_some()
        {
            return Err(Error::Forbidden);
        }
        let reference = TeamHostAdmissionRef {
            profile_id: p.id,
            profile_revision: row.try_get("revision").map_err(storage_error)?,
            profile_sha256: digest(&p)?,
        };
        source["trusted_host_admission"] =
            serde_json::to_value(&reference).map_err(storage_error)?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT source_sha256 FROM team_container_host_admissions WHERE nonce=$1",
        )
        .bind(nonce.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        if existing.is_some() {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO team_container_host_admissions(nonce,run_id,team_id,order_id,host_id,engine_id,profile_id,profile_revision,profile_sha256,approval_id,approval_request_sha256,original_lease_owner,original_binding,source_record,source_sha256,original_created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)").bind(nonce.to_string()).bind(self.run_id().to_string()).bind(self.team_id().to_string()).bind(self.order_id().to_string()).bind(host.host_id.to_string()).bind(&host.engine_id).bind(p.id.to_string()).bind(reference.profile_revision).bind(&reference.profile_sha256).bind(approval.to_string()).bind(&approved.request_sha256).bind(&self.lease_owner).bind(json_string(&self.binding)?).bind(json_string(&source)?).bind(digest(&source)?).bind(db_now(&self.store,&mut tx).await?).execute(&mut*tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(reference))
    }
    pub async fn dispatch_container_host(&self, approval: Uuid, nonce: Uuid) -> Result<()> {
        self.host_gate(approval, nonce, true, false).await
    }
    pub async fn start_container_host(&self, approval: Uuid, nonce: Uuid) -> Result<()> {
        self.host_gate(approval, nonce, false, true).await
    }
    pub async fn revalidate_container_host(&self, nonce: Uuid) -> Result<()> {
        self.host_gate(Uuid::nil(), nonce, false, false).await
    }
    async fn host_gate(
        &self,
        approval: Uuid,
        nonce: Uuid,
        dispatch: bool,
        start: bool,
    ) -> Result<()> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        crate::server_keys::server_admission(&mut tx, self.key_id).await?;
        if !approval.is_nil() {
            approval_current(self, &mut tx, approval).await?;
        }
        let Some(r) = sqlx::query("SELECT * FROM team_container_host_admissions WHERE nonce=$1")
            .bind(nonce.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
        else {
            return Err(Error::Conflict);
        };
        if r.try_get::<String, _>("run_id").map_err(storage_error)? != self.run_id().to_string()
            || r.try_get::<String, _>("original_binding")
                .map_err(storage_error)?
                != json_string(&self.binding)?
            || r.try_get::<String, _>("original_lease_owner")
                .map_err(storage_error)?
                != self.lease_owner
            || r.try_get::<Option<String>, _>("retired_receipt_sha256")
                .map_err(storage_error)?
                .is_some()
            || !approval.is_nil()
                && r.try_get::<String, _>("approval_id")
                    .map_err(storage_error)?
                    != approval.to_string()
        {
            return Err(Error::Forbidden);
        }
        if let Some(id) = r
            .try_get::<Option<String>, _>("profile_id")
            .map_err(storage_error)?
        {
            let p = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            registry::profile(&p)?;
            if p.try_get::<i64, _>("active").map_err(storage_error)? != 1
                || p.try_get::<Option<i64>, _>("retired_at")
                    .map_err(storage_error)?
                    .is_some()
                || p.try_get::<i64, _>("revision").map_err(storage_error)?
                    != r.try_get::<i64, _>("profile_revision")
                        .map_err(storage_error)?
                || p.try_get::<String, _>("profile_sha256")
                    .map_err(storage_error)?
                    != r.try_get::<String, _>("profile_sha256")
                        .map_err(storage_error)?
            {
                return Err(Error::Forbidden);
            }
        } else {
            let registered: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM team_host_trust_profiles WHERE host_id=$1 AND engine_id=$2",
            )
            .bind(r.try_get::<String, _>("host_id").map_err(storage_error)?)
            .bind(r.try_get::<String, _>("engine_id").map_err(storage_error)?)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if registered != 0 {
                return Err(Error::Forbidden);
            }
        }
        if dispatch {
            let changed=sqlx::query("UPDATE team_container_host_admissions SET dispatch_admitted=1 WHERE nonce=$1 AND dispatch_admitted=0 AND received_container_id IS NULL AND retired_receipt_sha256 IS NULL").bind(nonce.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
            if changed.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
        } else if r
            .try_get::<i64, _>("dispatch_admitted")
            .map_err(storage_error)?
            != 1
            || start
                && r.try_get::<Option<String>, _>("received_container_id")
                    .map_err(storage_error)?
                    .is_none()
        {
            return Err(Error::Conflict);
        }
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        if !approval.is_nil() {
            approval_current(self, &mut tx, approval).await?;
        }
        tx.commit().await.map_err(storage_error)
    }
    /// Original Create observation persists even if the current session/approver was revoked during IO.
    /// It does not authorize Start, which has a separate fresh gate.
    pub async fn receive_container_host(&self, nonce: Uuid, raw: &[u8]) -> Result<()> {
        let actual = record(raw)?;
        let cid = actual["container_id"]
            .as_str()
            .ok_or(Error::UncertainEffect)?;
        if !sha(cid) || uuid_value(&actual["nonce"])? != nonce {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        registry::lock(&self.store, &mut tx).await?;
        let Some(r) = sqlx::query("SELECT * FROM team_container_host_admissions WHERE nonce=$1")
            .bind(nonce.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
        else {
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        };
        if r.try_get::<String, _>("run_id").map_err(storage_error)? != self.run_id().to_string()
            || r.try_get::<String, _>("original_binding")
                .map_err(storage_error)?
                != json_string(&self.binding)?
            || r.try_get::<String, _>("original_lease_owner")
                .map_err(storage_error)?
                != self.lease_owner
            || r.try_get::<i64, _>("dispatch_admitted")
                .map_err(storage_error)?
                != 1
        {
            return Err(Error::Conflict);
        }
        let mut original: Value = parse(
            &r.try_get::<String, _>("source_record")
                .map_err(storage_error)?,
        )?;
        if digest(&original)?
            != r.try_get::<String, _>("source_sha256")
                .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        original["container_id"] = Value::String(cid.to_owned());
        if original != actual {
            return Err(Error::Conflict);
        }
        if let Some(old) = r
            .try_get::<Option<String>, _>("received_record_sha256")
            .map_err(storage_error)?
        {
            if old != bytes_sha(raw) {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        let changed=sqlx::query("UPDATE team_container_host_admissions SET received_container_id=$1,received_record=$2,received_record_sha256=$3 WHERE nonce=$4 AND dispatch_admitted=1 AND received_record_sha256 IS NULL AND source_sha256=$5").bind(cid).bind(STANDARD.encode(raw)).bind(bytes_sha(raw)).bind(nonce.to_string()).bind(r.try_get::<String,_>("source_sha256").map_err(storage_error)?).execute(&mut*tx).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn container_host_removed(&self, nonce: Uuid, raw: &[u8]) -> Result<()> {
        let actual = record(raw)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        registry::lock(&self.store, &mut tx).await?;
        let row = sqlx::query(
            "SELECT * FROM team_container_host_admissions WHERE nonce=$1 AND run_id=$2",
        )
        .bind(nonce.to_string())
        .bind(self.run_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if row
            .try_get::<String, _>("original_binding")
            .map_err(storage_error)?
            != json_string(&self.binding)?
            || row
                .try_get::<String, _>("original_lease_owner")
                .map_err(storage_error)?
                != self.lease_owner
            || row
                .try_get::<Option<String>, _>("received_record_sha256")
                .map_err(storage_error)?
                .as_deref()
                != Some(bytes_sha(raw).as_str())
            || actual["nonce"] != nonce.to_string()
        {
            return Err(Error::Conflict);
        }
        let receipt = serde_json::json!({"format":"localai-team-original-container-removed/1","nonce":nonce,"run_id":self.run_id(),"host_id":row.try_get::<String,_>("host_id").map_err(storage_error)?,"engine_id":row.try_get::<String,_>("engine_id").map_err(storage_error)?,"original_record_sha256":bytes_sha(raw),"container_id":actual["container_id"],"container_removed":true,"host_retired":false,"effects_settled":false});
        if let Some(old) = row
            .try_get::<Option<String>, _>("normal_removed_receipt")
            .map_err(storage_error)?
        {
            if old != json_string(&receipt)? {
                return Err(Error::Conflict);
            }
        } else {
            sqlx::query("UPDATE team_container_host_admissions SET normal_removed_receipt=$1 WHERE nonce=$2 AND normal_removed_receipt IS NULL AND received_record_sha256=$3").bind(json_string(&receipt)?).bind(nonce.to_string()).bind(bytes_sha(raw)).execute(&mut*tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
}
async fn approval_current(
    a: &TeamExecutionAuthority,
    tx: &mut Transaction<'_, Any>,
    id: Uuid,
) -> Result<()> {
    let r = sqlx::query(
        "SELECT * FROM team_tool_approvals WHERE id=$1 AND run_id=$2 AND lease_owner=$3",
    )
    .bind(id.to_string())
    .bind(a.run_id().to_string())
    .bind(&a.lease_owner)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let v: TeamToolApproval = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v.state != "executing" {
        return Err(Error::Forbidden);
    }
    validate_approver(&a.store, tx, &r, &v).await
}
