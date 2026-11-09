//! V1 Android wire metadata; actual identity sessions and team ACLs remain authority.
use super::*;
use cowork_identity::{ContentAction, Principal};
use serde::{Deserialize, Serialize};
use team_service::identity_error;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthGrant {
    pub id: String,
    pub kind: String,
    pub device_id: String,
    pub expires_at: i64,
    pub payload: String,
}
async fn clock(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<i64> {
    sqlx::query("UPDATE android_compat_guard SET observed_at=observed_at WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let old: i64 = sqlx::query_scalar("SELECT observed_at FROM android_compat_guard WHERE id=1")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let current = now();
    if current < old {
        return Err(Error::Conflict);
    }
    sqlx::query("UPDATE android_compat_guard SET observed_at=$1 WHERE id=1")
        .bind(current)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(current)
}
fn token_hash(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Validation("Invalid compatibility grant".into()));
    }
    Ok(())
}
impl Store {
    pub async fn android_run_project_revision(&self, actor: &Principal, run: Uuid) -> Result<i64> {
        let (value, _, _) = self.android_run_request(actor, run).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(
                &mut tx,
                actor,
                value.team_id,
                value.project_id,
                ContentAction::Read,
            )
            .await
            .map_err(identity_error)?;
        let payload: String = sqlx::query_scalar(
            "SELECT binding FROM team_runs WHERE id=$1 AND team_id=$2 AND project_id=$3",
        )
        .bind(run.to_string())
        .bind(value.team_id.to_string())
        .bind(value.project_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let binding: Value = parse(&payload)?;
        if binding["run_id"].as_str() != Some(run.to_string().as_str())
            || binding["team_id"].as_str() != Some(value.team_id.to_string().as_str())
            || binding["project_id"].as_str() != Some(value.project_id.to_string().as_str())
            || binding["order_id"].as_str() != Some(value.order_id.to_string().as_str())
        {
            return Err(Error::Conflict);
        }
        let revision = binding["resource_revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or(Error::Conflict)?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(revision)
    }
    pub async fn android_bind_file(
        &self,
        actor: &Principal,
        run: Uuid,
        blob: &TeamBlob,
    ) -> Result<()> {
        let (value, _, _) = self.android_run_request(actor, run).await?;
        if blob.team_id != value.team_id || blob.project_id != value.project_id {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(
                &mut tx,
                actor,
                value.team_id,
                value.project_id,
                ContentAction::Write,
            )
            .await
            .map_err(identity_error)?;
        let actual: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM team_blobs WHERE id=$1 AND team_id=$2 AND project_id=$3",
        )
        .bind(blob.id.to_string())
        .bind(value.team_id.to_string())
        .bind(value.project_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if actual != 1 {
            return Err(Error::Forbidden);
        }
        sqlx::query("INSERT INTO android_run_files(run_id,blob_id) VALUES($1,$2) ON CONFLICT(run_id,blob_id) DO NOTHING").bind(run.to_string()).bind(blob.id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_run_files(&self, actor: &Principal, run: Uuid) -> Result<Vec<TeamBlob>> {
        let (value, _, _) = self.android_run_request(actor, run).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(
                &mut tx,
                actor,
                value.team_id,
                value.project_id,
                ContentAction::Read,
            )
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT b.payload FROM android_run_files f JOIN team_blobs b ON b.id=f.blob_id WHERE f.run_id=$1 AND b.team_id=$2 AND b.project_id=$3 ORDER BY b.id LIMIT 2001").bind(run.to_string()).bind(value.team_id.to_string()).bind(value.project_id.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if rows.len() > 2000 {
            return Err(Error::BudgetExceeded);
        }
        let blobs = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<TeamBlob>>>()?
            .into_iter()
            .filter(|b| b.state == TeamBlobState::Ready)
            .collect();
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(blobs)
    }
    pub async fn android_wire_operation(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        key: &str,
        digest: &str,
    ) -> Result<Option<Value>> {
        valid_text(key, 200)?;
        token_hash(digest)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        clock(&mut tx).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_wire_operations WHERE identity_id=$1")
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 100_000 {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO android_wire_operations(identity_id,team_id,project_id,operation_key,request_sha256,receipt) VALUES($1,$2,$3,$4,$5,NULL) ON CONFLICT(identity_id,operation_key) DO NOTHING").bind(actor.identity_id().to_string()).bind(team.to_string()).bind(project.to_string()).bind(key).bind(digest).execute(&mut *tx).await.map_err(storage_error)?;
        let row=sqlx::query("SELECT team_id,project_id,request_sha256,receipt FROM android_wire_operations WHERE identity_id=$1 AND operation_key=$2").bind(actor.identity_id().to_string()).bind(key).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != digest
            || row.try_get::<String, _>("team_id").map_err(storage_error)? != team.to_string()
            || row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
                != project.to_string()
        {
            return Err(Error::Conflict);
        }
        let receipt = row
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
            .map(|s| parse(&s))
            .transpose()?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_wire_finish(
        &self,
        actor: &Principal,
        team: Uuid,
        project: Uuid,
        key: &str,
        digest: &str,
        receipt: &Value,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        clock(&mut tx).await?;
        let payload = json_string(receipt)?;
        if payload.len() > 512 * 1024 {
            return Err(Error::Validation("Compatibility receipt limit".into()));
        }
        let changed=sqlx::query("UPDATE android_wire_operations SET receipt=$1 WHERE identity_id=$2 AND operation_key=$3 AND request_sha256=$4 AND team_id=$5 AND project_id=$6 AND receipt IS NULL").bind(&payload).bind(actor.identity_id().to_string()).bind(key).bind(digest).bind(team.to_string()).bind(project.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        let row:Option<String>=sqlx::query_scalar("SELECT receipt FROM android_wire_operations WHERE identity_id=$1 AND operation_key=$2 AND request_sha256=$3 AND team_id=$4 AND project_id=$5").bind(actor.identity_id().to_string()).bind(key).bind(digest).bind(team.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.flatten();
        if changed.rows_affected() == 0 && row.is_none() {
            return Err(Error::Conflict);
        }
        let result = parse(&row.ok_or(Error::Conflict)?)?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_creation_metadata(
        &self,
        actor: &Principal,
        team: Uuid,
        target: Uuid,
        action: &str,
    ) -> Result<(Uuid, i64)> {
        if !matches!(action, "organization_created" | "content_resource_created") {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        let row=sqlx::query("SELECT actor_id,observed_at FROM identity_audit WHERE organization_id=$1 AND target_id=$2 AND action=$3 ORDER BY sequence LIMIT 1").bind(team.to_string()).bind(target.to_string()).bind(action).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
        let owner = Uuid::parse_str(
            &row.try_get::<String, _>("actor_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let created = row.try_get("observed_at").map_err(storage_error)?;
        self.identity()
            .organization_in_transaction(&mut tx, actor, team)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok((owner, created))
    }
    pub async fn android_create_grant(&self, digest: &str, grant: &AndroidAuthGrant) -> Result<()> {
        token_hash(digest)?;
        if !matches!(grant.kind.as_str(), "code" | "refresh")
            || grant.device_id.is_empty()
            || grant.device_id.len() > 200
            || grant.payload.len() > 16_384
            || Uuid::parse_str(&grant.id).is_err()
        {
            return Err(Error::Validation("Invalid compatibility grant".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = clock(&mut tx).await?;
        if grant.expires_at <= current || grant.expires_at > current.saturating_add(86_400_000) {
            return Err(Error::Conflict);
        }
        let payload: Value = parse(&grant.payload)?;
        let sealed: cowork_secrets::SealedCredential =
            serde_json::from_value(payload["sealed"].clone())
                .map_err(|_| Error::NativeKeyUnavailable)?;
        crate::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
        sqlx::query("DELETE FROM android_auth_grants WHERE expires_at<=$1")
            .bind(current)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_grants")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if count >= 10_000 {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO android_auth_grants(id,kind,token_sha256,device_id,expires_at,consumed,payload) VALUES($1,$2,$3,$4,$5,0,$6)").bind(&grant.id).bind(&grant.kind).bind(digest).bind(&grant.device_id).bind(grant.expires_at).bind(&grant.payload).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_grant(&self, kind: &str, digest: &str) -> Result<AndroidAuthGrant> {
        token_hash(digest)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = clock(&mut tx).await?;
        let row=sqlx::query("SELECT id,kind,device_id,expires_at,payload FROM android_auth_grants WHERE token_sha256=$1 AND kind=$2 AND consumed=0 AND expires_at>$3").bind(digest).bind(kind).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let result = AndroidAuthGrant {
            id: row.try_get("id").map_err(storage_error)?,
            kind: row.try_get("kind").map_err(storage_error)?,
            device_id: row.try_get("device_id").map_err(storage_error)?,
            expires_at: row.try_get("expires_at").map_err(storage_error)?,
            payload: row.try_get("payload").map_err(storage_error)?,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_consume_grant(
        &self,
        grant: &AndroidAuthGrant,
        digest: &str,
    ) -> Result<()> {
        token_hash(digest)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = clock(&mut tx).await?;
        let changed=sqlx::query("UPDATE android_auth_grants SET consumed=1 WHERE id=$1 AND token_sha256=$2 AND kind=$3 AND device_id=$4 AND payload=$5 AND expires_at=$6 AND consumed=0 AND expires_at>$7").bind(&grant.id).bind(digest).bind(&grant.kind).bind(&grant.device_id).bind(&grant.payload).bind(grant.expires_at).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_project_team(&self, actor: &Principal, project: Uuid) -> Result<Uuid> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let team: String =
            sqlx::query_scalar("SELECT organization_id FROM tenant_projects WHERE id=$1")
                .bind(project.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let team = Uuid::parse_str(&team).map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(team)
    }
    pub async fn android_order(&self, actor: &Principal, order: Uuid) -> Result<TeamOrder> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let row = sqlx::query("SELECT team_id,project_id,payload FROM team_orders WHERE id=$1")
            .bind(order.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let team = Uuid::parse_str(&row.try_get::<String, _>("team_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        let value: TeamOrder = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if value.id != order || value.team_id != team || value.project_id != project {
            return Err(Error::Conflict);
        }
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_run_request(
        &self,
        actor: &Principal,
        run: Uuid,
    ) -> Result<(TeamRun, CreateTeamRun, Uuid)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let row = sqlx::query(
            "SELECT team_id,project_id,payload,request,creator_id FROM team_runs WHERE id=$1",
        )
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let team = Uuid::parse_str(&row.try_get::<String, _>("team_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let project = Uuid::parse_str(
            &row.try_get::<String, _>("project_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(&mut tx, actor, team, project, ContentAction::Read)
            .await
            .map_err(identity_error)?;
        let value: TeamRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let request: CreateTeamRun =
            parse(&row.try_get::<String, _>("request").map_err(storage_error)?)?;
        let creator = Uuid::parse_str(
            &row.try_get::<String, _>("creator_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?;
        if value.id != run
            || value.team_id != team
            || value.project_id != project
            || request.order_id != value.order_id
        {
            return Err(Error::Conflict);
        }
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok((value, request, creator))
    }
    /// Persist observed run-state snapshots, not an invented tool-event history.
    pub async fn android_observe_run(&self, actor: &Principal, run: &TeamRun) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(
                &mut tx,
                actor,
                run.team_id,
                run.project_id,
                ContentAction::Read,
            )
            .await
            .map_err(identity_error)?;
        clock(&mut tx).await?;
        let payload: String = sqlx::query_scalar(
            "SELECT payload FROM team_runs WHERE id=$1 AND team_id=$2 AND project_id=$3",
        )
        .bind(run.id.to_string())
        .bind(run.team_id.to_string())
        .bind(run.project_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let latest: TeamRun = parse(&payload)?;
        if latest.id != run.id
            || latest.team_id != run.team_id
            || latest.project_id != run.project_id
        {
            return Err(Error::Conflict);
        }
        let run = &latest;
        let snapshot = json_string(run)?;
        let digest = hash(snapshot.as_bytes());
        let prior: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_run_events WHERE run_id=$1 AND snapshot_sha256=$2",
        )
        .bind(run.id.to_string())
        .bind(&digest)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if prior == 0 {
            let seq: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM android_run_events WHERE run_id=$1",
            )
            .bind(run.id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if seq > 10_000 {
                return Err(Error::BudgetExceeded);
            }
            let state = match run.status {
                TeamRunStatus::Cancelled => "canceled",
                TeamRunStatus::Unknown => "interrupted",
                TeamRunStatus::Queued => "queued",
                TeamRunStatus::Running => "running",
                TeamRunStatus::Completed => "completed",
                TeamRunStatus::Failed => "failed",
            };
            let kind = if seq == 1 {
                "created"
            } else if state == "completed" {
                "completed"
            } else if state == "failed" {
                "failed"
            } else {
                "state_changed"
            };
            let event = json!({"schema_version":2,"run_id":run.id,"sequence":seq,"event_id":Uuid::new_v4(),"kind":kind,"payload":{"state":state,"response":run.response,"error":run.error,"observed_snapshot":true},"created_at":chrono::DateTime::from_timestamp_millis(run.updated_at).ok_or(Error::Conflict)?.to_rfc3339()});
            sqlx::query("INSERT INTO android_run_events(run_id,sequence,snapshot_sha256,payload) VALUES($1,$2,$3,$4)").bind(run.id.to_string()).bind(seq).bind(digest).bind(json_string(&event)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_run_events(
        &self,
        actor: &Principal,
        run: Uuid,
        after: i64,
    ) -> Result<Vec<Value>> {
        let (value, _, _) = self.android_run_request(actor, run).await?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let permit = self
            .identity()
            .authorize_content_in_transaction(
                &mut tx,
                actor,
                value.team_id,
                value.project_id,
                ContentAction::Read,
            )
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT payload FROM android_run_events WHERE run_id=$1 AND sequence>$2 ORDER BY sequence LIMIT 100").bind(run.to_string()).bind(after.max(0)).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let events = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<Value>>>()?;
        self.identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(events)
    }
}
