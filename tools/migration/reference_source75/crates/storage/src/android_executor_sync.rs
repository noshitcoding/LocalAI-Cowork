//! Device replication has its own current grant; pairing never becomes a human session.
use super::*;
use crate::android_device_registry::{AndroidAgentPrincipal, agent_current, device_owner};
use crate::android_devices::{device_invalid, device_writer};
use crate::android_wire_sync::{AndroidWireSyncChange, record};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};
async fn source(
    tx: &mut Transaction<'_, Any>,
    user: Uuid,
    project: Uuid,
    write: bool,
) -> Result<(Uuid, Value)> {
    let action = if write {
        ContentAction::Write
    } else {
        ContentAction::Read
    };
    if let Some(r) =
        sqlx::query("SELECT owner_id FROM android_personal_projects WHERE id=$1 AND deleted=0")
            .bind(project.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
    {
        let owner = Uuid::parse_str(&r.try_get::<String, _>("owner_id").map_err(storage_error)?)
            .map_err(storage_error)?;
        let stamp = crate::android_private_sharing::stamp(tx, user, project, action).await?;
        return Ok((
            owner,
            json!({"family":"personal","stamp":stamp,"source_owner_id":owner}),
        ));
    }
    let row=sqlx::query("SELECT p.organization_id,m.id AS member_id,m.revision AS member_revision,g.revision AS grant_revision,o.authorization_epoch FROM tenant_projects p JOIN identity_organizations o ON o.id=p.organization_id JOIN identity_memberships m ON m.organization_id=o.id JOIN identity_subjects i ON i.id=m.identity_id JOIN identity_content_grants g ON g.resource_id=p.id AND g.membership_id=m.id WHERE p.id=$1 AND COALESCE((SELECT archived FROM team_project_profiles f WHERE f.project_id=p.id),0)=0 AND o.state='active' AND m.identity_id=$2 AND m.active=1 AND i.active=1 AND g.active=1 AND (g.permissions & $3)=$3 AND ($4=0 OR m.role<>'viewer') AND NOT EXISTS(SELECT 1 FROM android_tenant_support_sources s WHERE s.resource_id=p.id AND s.membership_id=m.id)").bind(project.to_string()).bind(user.to_string()).bind(if write{3i64}else{1}).bind(i64::from(write)).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    Ok((
        user,
        json!({"family":"tenant","organization_id":row.try_get::<String,_>("organization_id").map_err(storage_error)?,"member_id":row.try_get::<String,_>("member_id").map_err(storage_error)?,"member_revision":row.try_get::<i64,_>("member_revision").map_err(storage_error)?,"grant_revision":row.try_get::<i64,_>("grant_revision").map_err(storage_error)?,"authorization_epoch":row.try_get::<i64,_>("authorization_epoch").map_err(storage_error)?}),
    ))
}
async fn permit(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    a: &AndroidAgentPrincipal,
    project: Uuid,
    write: bool,
    now: i64,
) -> Result<(Uuid, Value, AnyRow)> {
    agent_current(tx, a, now).await?;
    store
        .identity()
        .android_validate_executor_sync_source_in_transaction(tx, a.executor_id, project)
        .await
        .map_err(team_service::identity_error)?;
    let r=sqlx::query("SELECT g.*,s.revoked AS session_revoked,s.revision AS session_revision,s.expires_at AS session_expires,i.active AS issuer_active FROM android_executor_sync_grants g JOIN identity_sessions s ON s.id=g.issuer_session_id AND s.identity_id=g.issuer_id JOIN identity_subjects i ON i.id=g.issuer_id WHERE g.executor_id=$1 AND g.project_id=$2 AND g.owner_id=$3").bind(a.executor_id.to_string()).bind(project.to_string()).bind(a.owner_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let wanted = if write { 3 } else { 1 };
    if r.try_get::<i64, _>("active").map_err(storage_error)? != 1
        || r.try_get::<i64, _>("permissions").map_err(storage_error)? & wanted != wanted
        || r.try_get::<i64, _>("expires_at").map_err(storage_error)? <= now
        || r.try_get::<i64, _>("session_revoked")
            .map_err(storage_error)?
            != 0
        || r.try_get::<i64, _>("issuer_active")
            .map_err(storage_error)?
            != 1
        || r.try_get::<i64, _>("session_expires")
            .map_err(storage_error)?
            <= now
        || r.try_get::<i64, _>("session_revision")
            .map_err(storage_error)?
            != r.try_get::<i64, _>("issuer_session_revision")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let issuer = Uuid::parse_str(&r.try_get::<String, _>("issuer_id").map_err(storage_error)?)
        .map_err(storage_error)?;
    let (owner, current) = source(
        tx,
        issuer,
        project,
        r.try_get::<i64, _>("permissions").map_err(storage_error)? == 3,
    )
    .await?;
    if parse::<Value>(
        &r.try_get::<String, _>("source_authority")
            .map_err(storage_error)?,
    )? != current
        || r.try_get::<String, _>("source_owner_id")
            .map_err(storage_error)?
            != owner.to_string()
    {
        return Err(Error::Forbidden);
    }
    let scope: Option<String> = sqlx::query_scalar(
        "SELECT organization_id FROM identity_session_scopes WHERE session_id=$1",
    )
    .bind(
        r.try_get::<String, _>("issuer_session_id")
            .map_err(storage_error)?,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    if scope.is_some()
        && scope
            != r.try_get::<Option<String>, _>("organization_id")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let observed = device_writer(tx).await?;
    store
        .identity()
        .android_validate_executor_sync_source_in_transaction(tx, a.executor_id, project)
        .await
        .map_err(team_service::identity_error)?;
    agent_current(tx, a, observed).await?;
    Ok((owner, current, r))
}
fn safe(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().all(|(k, v)| {
            !matches!(
                k.to_ascii_lowercase().as_str(),
                "api_key"
                    | "apikey"
                    | "access_token"
                    | "refresh_token"
                    | "password"
                    | "secret"
                    | "client_secret"
                    | "authorization"
            ) && safe(v)
        }),
        Value::Array(a) => a.iter().all(safe),
        _ => true,
    }
}
impl Store {
    pub async fn android_executor_sync_grants(
        &self,
        actor: &Principal,
        executor: Uuid,
        project: Option<Uuid>,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_owner(self, &mut tx, actor, executor).await?;
        let rows=sqlx::query("SELECT project_id,permissions,revision,active,expires_at FROM android_executor_sync_grants WHERE executor_id=$1 AND owner_id=$2 AND ($3 IS NULL OR project_id=$3) AND project_id>$4 ORDER BY project_id LIMIT $5").bind(executor.to_string()).bind(actor.identity_id().to_string()).bind(project.map(|v|v.to_string())).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for r in rows.iter().take(limit as usize) {
            items.push(json!({"project_id":r.try_get::<String,_>("project_id").map_err(storage_error)?,"permissions":r.try_get::<i64,_>("permissions").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"active":r.try_get::<i64,_>("active").map_err(storage_error)?==1,"expires_at":r.try_get::<i64,_>("expires_at").map_err(storage_error)?}));
        }
        device_owner(self, &mut tx, actor, executor).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"executor_id":executor,"next_cursor":if more{items.last().map(|v|v["project_id"].clone())}else{None},"items":items}),
        )
    }
    pub async fn android_executor_sync_grant(
        &self,
        actor: &Principal,
        executor: Uuid,
        project: Uuid,
        expected: i64,
        write: bool,
        active: bool,
        expires: i64,
        request: Uuid,
    ) -> Result<Value> {
        if expected < 0 || request.is_nil() {
            return Err(device_invalid());
        }
        let sha = canonical_hash(
            &json!({"executor_id":executor,"project_id":project,"expected_revision":expected,"write":write,"active":active,"expires_at":expires,"request_id":request}),
        );
        let key = format!("executor-sync:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        device_owner(self, &mut tx, actor, executor).await?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Share)
            .await
            .map_err(team_service::identity_error)?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &key,
            "executor_sync",
            &sha,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        self.android_task_project_permit(&mut tx, actor, project, ContentAction::Share)
            .await?;
        let (owner, stamp) = source(&mut tx, actor.identity_id(), project, write).await?;
        let org = stamp["organization_id"].as_str().map(str::to_owned);
        let session=sqlx::query("SELECT revision,expires_at FROM identity_sessions WHERE id=$1 AND identity_id=$2 AND revoked=0 AND expires_at>$3").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(now).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if active
            && (expires <= now
                || expires > now + 7 * 24 * 60 * 60 * 1000
                || expires
                    > session
                        .try_get::<i64, _>("expires_at")
                        .map_err(storage_error)?)
        {
            return Err(device_invalid());
        }
        let rev:Option<i64>=sqlx::query_scalar("SELECT revision FROM android_executor_sync_grants WHERE executor_id=$1 AND project_id=$2").bind(executor.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if rev.unwrap_or(0) != expected {
            return Err(Error::Conflict);
        }
        let next = expected.checked_add(1).ok_or(Error::Conflict)?;
        if sqlx::query("INSERT INTO android_executor_sync_grants(executor_id,project_id,owner_id,issuer_id,source_owner_id,organization_id,issuer_session_id,issuer_session_revision,source_authority,permissions,revision,active,expires_at) VALUES($1,$2,$3,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(executor_id,project_id) DO UPDATE SET source_owner_id=$4,organization_id=$5,issuer_session_id=$6,issuer_session_revision=$7,source_authority=$8,permissions=$9,revision=$10,active=$11,expires_at=$12 WHERE android_executor_sync_grants.revision=$13 AND android_executor_sync_grants.owner_id=$3 AND android_executor_sync_grants.issuer_id=$3").bind(executor.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(owner.to_string()).bind(&org).bind(actor.session_id().to_string()).bind(session.try_get::<i64,_>("revision").map_err(storage_error)?).bind(json_string(&stamp)?).bind(if write{3i64}else{1}).bind(next).bind(i64::from(active)).bind(expires).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        self.android_task_project_permit(&mut tx, actor, project, ContentAction::Share)
            .await?;
        device_owner(self, &mut tx, actor, executor).await?;
        let receipt = json!({"executor_id":executor,"project_id":project,"organization_id":org,"revision":next,"active":active,"request_id":request,"request_sha256":sha});
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &key,
            "executor_sync",
            &sha,
            &receipt,
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_agent_sync_pull(
        &self,
        a: &AndroidAgentPrincipal,
        project: Uuid,
        after: i64,
        limit: u32,
    ) -> Result<Value> {
        if after < 0 || !(1..=1000).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let (owner, source, _) = permit(self, &mut tx, a, project, false, now).await?;
        let org = source["organization_id"].as_str().map(str::to_owned);
        let rows=sqlx::query("SELECT c.* FROM android_wire_sync_changes c WHERE c.project_id=$1 AND c.cursor>$2 AND ((c.team_id IS NULL AND c.owner_id=$3) OR (c.team_id=$4 AND c.owner_id=$5)) ORDER BY c.cursor LIMIT $6").bind(project.to_string()).bind(after).bind(owner.to_string()).bind(org).bind(a.owner_id.to_string()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut more = rows.len() > limit as usize;
        let mut items = Vec::new();
        let mut next = after;
        let mut size = 0usize;
        for r in rows.iter().take(limit as usize) {
            let raw: String = r.try_get("payload").map_err(storage_error)?;
            if raw.len() > 2 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            if !items.is_empty() && size + raw.len() > 3 * 1024 * 1024 {
                more = true;
                break;
            }
            size += raw.len();
            next = r.try_get("cursor").map_err(storage_error)?;
            let v: Value = parse(&raw)?;
            items.push(json!({"schema_version":2,"cursor":next,"entity_type":v["entity_type"],"entity_id":v["entity_id"],"revision":v["revision"],"operation":if v["tombstone"]==true{"delete"}else{"upsert"},"payload":v["payload"],"created_at":v["updated_at"]}));
        }
        let current = device_writer(&mut tx).await?;
        permit(self, &mut tx, a, project, false, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"changes":items,"next_cursor":next,"has_more":more}))
    }
    pub async fn android_agent_sync_push(
        &self,
        a: &AndroidAgentPrincipal,
        project: Uuid,
        changes: Vec<AndroidWireSyncChange>,
    ) -> Result<Value> {
        if changes.is_empty() || changes.len() > 100 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let (owner, authority, _) = permit(self, &mut tx, a, project, true, now).await?;
        let mut results = Vec::new();
        for c in changes {
            if c.schema_version != 2
                || c.device_id != a.executor_id
                || c.operation_id.is_nil()
                || c.base_revision < 0
                || !matches!(c.operation.as_str(), "upsert" | "delete")
                || !matches!(
                    c.entity_type.as_str(),
                    "project"
                        | "thread"
                        | "task"
                        | "message"
                        | "schedule"
                        | "run"
                        | "crew"
                        | "skill"
                        | "memory"
                        | "provider_profile"
                        | "secret_metadata"
                        | "mcp_metadata"
                )
                || (c.operation == "upsert") != c.payload.is_some()
            {
                return Err(device_invalid());
            }
            let at = chrono::DateTime::parse_from_rfc3339(&c.client_timestamp)
                .map_err(|_| device_invalid())?
                .timestamp_millis();
            if at > now + 300000 {
                return Err(device_invalid());
            }
            let payload = c.payload.clone().unwrap_or(Value::Null);
            if !payload.is_null()
                && (!payload.is_object()
                    || json_string(&payload)?.len() > 512 * 1024
                    || !safe(&payload)
                    || payload
                        .get("project_id")
                        .is_some_and(|v| v != &json!(project))
                    || payload
                        .get("team_id")
                        .is_some_and(|v| !v.is_null() && v != &authority["organization_id"]))
            {
                return Err(device_invalid());
            }
            if c.entity_type == "project" && c.entity_id != project {
                return Err(Error::Forbidden);
            }
            let sha = canonical_hash(&json!({"project_id":project,"change":c}));
            if let Some(r)=sqlx::query("SELECT request_sha256,receipt FROM android_wire_sync_receipts WHERE owner_id=$1 AND operation_id=$2").bind(a.owner_id.to_string()).bind(c.operation_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?{if r.try_get::<String,_>("request_sha256").map_err(storage_error)?!=sha{return Err(Error::Conflict)}results.push(parse::<Value>(&r.try_get::<String,_>("receipt").map_err(storage_error)?)?);continue;}
            let personal = authority["family"] == "personal";
            let source_owner = if personal
                && matches!(
                    c.entity_type.as_str(),
                    "project" | "thread" | "task" | "message"
                ) {
                owner
            } else {
                a.owner_id
            };
            if personal
                && matches!(
                    c.entity_type.as_str(),
                    "project" | "thread" | "task" | "message"
                )
            {
                let table = match c.entity_type.as_str() {
                    "project" => "android_personal_projects",
                    "thread" => "android_personal_threads",
                    "task" => "android_task_versions",
                    _ => "android_personal_messages",
                };
                let column = if c.entity_type == "project" {
                    "id"
                } else {
                    "project_id"
                };
                let found: Option<String> = sqlx::query_scalar(&format!(
                    "SELECT {column} FROM {table} WHERE id=$1 LIMIT 1"
                ))
                .bind(c.entity_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
                if found.is_some_and(|v| v != project.to_string()) {
                    return Err(Error::Conflict);
                }
                crate::android_shared_sync::seed_current(
                    &mut tx,
                    source_owner,
                    project,
                    &c.entity_type,
                    c.entity_id,
                    now,
                )
                .await?;
            }
            let old=sqlx::query("SELECT payload,revision,project_id FROM android_wire_sync_current WHERE owner_id=$1 AND entity_type=$2 AND entity_id=$3").bind(source_owner.to_string()).bind(&c.entity_type).bind(c.entity_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            if old.as_ref().is_some_and(|r| {
                r.try_get::<Option<String>, _>("project_id")
                    .ok()
                    .flatten()
                    .is_some_and(|p| p != project.to_string())
            }) {
                return Err(Error::Conflict);
            }
            let revision = old
                .as_ref()
                .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
                .transpose()?
                .unwrap_or(0);
            let receipt = if revision != c.base_revision {
                json!({"schema_version":2,"operation_id":c.operation_id,"status":"conflict","entity":old.as_ref().map(|r|parse::<Value>(&r.try_get::<String,_>("payload").map_err(storage_error)?)).transpose()?})
            } else {
                if personal
                    && matches!(
                        c.entity_type.as_str(),
                        "project" | "thread" | "task" | "message"
                    )
                {
                    crate::android_shared_sync::apply_content(
                        &mut tx,
                        a.owner_id,
                        source_owner,
                        project,
                        &c.entity_type,
                        c.entity_id,
                        &payload,
                        revision + 1,
                        c.operation == "delete",
                        now,
                    )
                    .await?;
                }
                let value = record(
                    &mut tx,
                    source_owner,
                    &c.entity_type,
                    c.entity_id,
                    authority["organization_id"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok()),
                    Some(project),
                    revision + 1,
                    &payload,
                    c.operation == "delete",
                    now,
                )
                .await?;
                json!({"schema_version":2,"operation_id":c.operation_id,"status":"applied","entity":value})
            };
            sqlx::query("INSERT INTO android_wire_sync_receipts(owner_id,operation_id,request_sha256,receipt) VALUES($1,$2,$3,$4)").bind(a.owner_id.to_string()).bind(c.operation_id.to_string()).bind(sha).bind(json_string(&receipt)?).execute(&mut *tx).await.map_err(storage_error)?;
            results.push(receipt);
            let current = device_writer(&mut tx).await?;
            permit(self, &mut tx, a, project, true, current).await?;
        }
        let response_bytes = results.iter().try_fold(0usize, |n, v| {
            n.checked_add(json_string(v)?.len())
                .ok_or(Error::BudgetExceeded)
        })?;
        if response_bytes > 3 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"results":results}))
    }
    pub async fn android_agent_sync_entities(
        &self,
        a: &AndroidAgentPrincipal,
        project: Uuid,
        kind: &str,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=1000).contains(&limit)
            || !matches!(
                kind,
                "project"
                    | "thread"
                    | "task"
                    | "message"
                    | "schedule"
                    | "run"
                    | "crew"
                    | "skill"
                    | "memory"
                    | "provider_profile"
                    | "secret_metadata"
                    | "mcp_metadata"
            )
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let (owner, source, _) = permit(self, &mut tx, a, project, false, now).await?;
        let org = source["organization_id"].as_str().map(str::to_owned);
        let rows=sqlx::query("SELECT c.* FROM android_wire_sync_current c WHERE c.project_id=$1 AND c.entity_type=$2 AND c.entity_id>$3 AND ((c.team_id IS NULL AND (c.owner_id=$4 OR c.owner_id=$5)) OR (c.team_id=$6 AND c.owner_id=$5)) ORDER BY c.entity_id LIMIT $7").bind(project.to_string()).bind(kind).bind(after.map(|u|u.to_string()).unwrap_or_default()).bind(owner.to_string()).bind(a.owner_id.to_string()).bind(org).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut more = rows.len() > limit as usize;
        let mut items = Vec::new();
        let mut size = 0usize;
        for r in rows.iter().take(limit as usize) {
            let raw: String = r.try_get("payload").map_err(storage_error)?;
            if raw.len() > 2 * 1024 * 1024 {
                return Err(Error::BudgetExceeded);
            }
            if !items.is_empty() && size + raw.len() > 3 * 1024 * 1024 {
                more = true;
                break;
            }
            size += raw.len();
            items.push(parse::<Value>(&raw)?);
        }
        let current = device_writer(&mut tx).await?;
        permit(self, &mut tx, a, project, false, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"next_after":if more{items.last().map(|v|v["entity_id"].clone())}else{None},"items":items,"has_more":more}),
        )
    }
    pub async fn android_current_sync_device(&self, actor: &Principal) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        let device:Option<String>=sqlx::query_scalar("SELECT device_id FROM android_wire_session_devices WHERE session_id=$1 AND owner_id=$2").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"owner_id":actor.identity_id(),"session_id":actor.session_id(),"device_id":device,"execution_grant":false}),
        )
    }
    pub async fn android_agent_sync_frame(
        &self,
        a: &AndroidAgentPrincipal,
        project: Uuid,
        cursor: i64,
    ) -> Result<Option<Value>> {
        if cursor <= 0 {
            return Err(device_invalid());
        }
        let page = self
            .android_agent_sync_pull(a, project, cursor - 1, 1)
            .await?;
        Ok(page["changes"]
            .as_array()
            .and_then(|v| v.first())
            .filter(|v| v["cursor"].as_i64() == Some(cursor))
            .cloned())
    }
    pub async fn android_sync_original(
        &self,
        actor: &Principal,
        operation: Uuid,
        sha: &str,
    ) -> Result<Value> {
        if sha.len() != 64
            || !sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        let r=sqlx::query("SELECT request_sha256,receipt FROM android_wire_sync_receipts WHERE owner_id=$1 AND operation_id=$2").bind(actor.identity_id().to_string()).bind(operation.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        if r.try_get::<String, _>("request_sha256")
            .map_err(storage_error)?
            != sha
        {
            return Err(Error::Conflict);
        }
        let receipt: Value = parse(&r.try_get::<String, _>("receipt").map_err(storage_error)?)?;
        let kind = receipt["entity"]["entity_type"].as_str();
        let id = receipt["entity"]["entity_id"].as_str();
        if let (Some(kind), Some(id)) = (kind, id) {
            let rows=sqlx::query("SELECT * FROM android_wire_sync_current WHERE entity_type=$1 AND entity_id=$2 LIMIT 2").bind(kind).bind(id).fetch_all(&mut *tx).await.map_err(storage_error)?;
            if rows.len() != 1 {
                return Err(Error::Forbidden);
            }
            let project: Option<String> = rows[0].try_get("project_id").map_err(storage_error)?;
            if let Some(p) = project {
                self.android_task_project_permit(
                    &mut tx,
                    actor,
                    Uuid::parse_str(&p).map_err(storage_error)?,
                    ContentAction::Read,
                )
                .await?;
            } else if rows[0]
                .try_get::<String, _>("owner_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
            {
                return Err(Error::Forbidden);
            }
        }
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"operation_id":operation,"request_sha256":sha,"receipt":receipt}))
    }
}
