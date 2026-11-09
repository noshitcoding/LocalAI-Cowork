//! Current operator/project pools, independent of a device's asserted labels or pairing.
use super::*;
use crate::android_devices::{device_invalid, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};
async fn admin(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    org: Uuid,
) -> Result<()> {
    let actual = store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(team_service::identity_error)?;
    store
        .identity()
        .organization_in_transaction(tx, actor, org)
        .await
        .map_err(team_service::identity_error)?;
    store
        .identity()
        .require_session_action_in_transaction(tx, actor, ContentAction::Share)
        .await
        .map_err(team_service::identity_error)?;
    let role:Option<String>=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE identity_id=$1 AND organization_id=$2 AND active=1").bind(actor.identity_id().to_string()).bind(org.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if !actual.platform_admin && !role.is_some_and(|v| matches!(v.as_str(), "owner" | "admin")) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn policy(v: &Value) -> Result<()> {
    let o = v.as_object().ok_or_else(device_invalid)?;
    if o.keys()
        .any(|k| !matches!(k.as_str(), "required_capabilities" | "max_concurrent_runs"))
    {
        return Err(device_invalid());
    }
    let c = v["required_capabilities"]
        .as_array()
        .ok_or_else(device_invalid)?;
    if c.len() > 64
        || c.iter().any(|v| {
            v.as_str().is_none_or(|s| {
                s.is_empty()
                    || s.len() > 100
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        })
        || c.iter()
            .filter_map(Value::as_str)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != c.len()
        || !v["max_concurrent_runs"]
            .as_i64()
            .is_some_and(|n| (1..=16).contains(&n))
    {
        return Err(device_invalid());
    }
    Ok(())
}
fn view(r: &AnyRow) -> Result<Value> {
    Ok(
        json!({"id":r.try_get::<String,_>("id").map_err(storage_error)?,"organization_id":r.try_get::<String,_>("organization_id").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"active":r.try_get::<i64,_>("active").map_err(storage_error)?==1,"name":r.try_get::<String,_>("name").map_err(storage_error)?,"kind":"managed_windows","policy":parse::<Value>(&r.try_get::<String,_>("policy").map_err(storage_error)?)?}),
    )
}
pub(crate) async fn stamp(
    tx: &mut Transaction<'_, Any>,
    org: Uuid,
    project: Uuid,
    pool: Uuid,
) -> Result<Value> {
    let r=sqlx::query("SELECT p.revision,p.policy,m.revision AS mapping_revision FROM android_executor_pool_profiles p JOIN android_executor_pool_projects m ON m.pool_id=p.id JOIN tenant_projects t ON t.id=m.project_id JOIN identity_organizations o ON o.id=t.organization_id WHERE p.id=$1 AND p.organization_id=$2 AND m.organization_id=$2 AND m.project_id=$3 AND t.organization_id=$2 AND p.active=1 AND m.active=1 AND o.state='active'").bind(pool.to_string()).bind(org.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    let p: Value = parse(&r.try_get::<String, _>("policy").map_err(storage_error)?)?;
    policy(&p)?;
    Ok(
        json!({"format":1,"pool_id":pool,"organization_id":org,"project_id":project,"pool_revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"mapping_revision":r.try_get::<i64,_>("mapping_revision").map_err(storage_error)?,"policy_sha256":canonical_hash(&p),"policy":p}),
    )
}
pub(crate) async fn validate_run(tx: &mut Transaction<'_, Any>, r: &AnyRow) -> Result<()> {
    let v: Value = parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v["spec"]["executor_target"]["kind"] != "managed_windows_pool" {
        if v["spec"]["input"].get("executor_pool_access").is_some() {
            return Err(Error::Forbidden);
        }
        return Ok(());
    }
    let org = Uuid::parse_str(&r.try_get::<String, _>("team_id").map_err(storage_error)?)
        .map_err(storage_error)?;
    let project = Uuid::parse_str(
        &r.try_get::<String, _>("project_id")
            .map_err(storage_error)?,
    )
    .map_err(storage_error)?;
    let pool = Uuid::parse_str(
        v["spec"]["executor_target"]["pool_id"]
            .as_str()
            .ok_or_else(device_invalid)?,
    )
    .map_err(storage_error)?;
    let current = stamp(tx, org, project, pool).await?;
    if v["spec"]["input"]["executor_pool_access"] != current {
        return Err(Error::Forbidden);
    }
    let original:Option<String>=sqlx::query_scalar("SELECT stamp FROM android_executor_pool_runs WHERE run_id=$1 AND pool_id=$2 AND project_id=$3").bind(r.try_get::<String,_>("id").map_err(storage_error)?).bind(pool.to_string()).bind(project.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    if original.map(|s| parse::<Value>(&s)).transpose()?.as_ref() != Some(&current) {
        return Err(Error::Forbidden);
    }
    let running:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_executor_pool_runs a JOIN android_scoped_runs r ON r.id=a.run_id WHERE a.pool_id=$1 AND r.state IN('leased','running')").bind(pool.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let max = current["policy"]["max_concurrent_runs"]
        .as_i64()
        .ok_or_else(device_invalid)?;
    let state: String = r.try_get("state").map_err(storage_error)?;
    if running > max || (state == "queued" && running >= max) {
        return Err(Error::BudgetExceeded);
    }
    let registration: String = sqlx::query_scalar(
        "SELECT registration FROM android_devices WHERE id=$1 AND team_id=$2 AND revoked=0",
    )
    .bind(
        r.try_get::<String, _>("executor_id")
            .map_err(storage_error)?,
    )
    .bind(org.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::Forbidden)?;
    let actual: DeviceExecutorRegistration = parse(&registration)?;
    if actual.kind != "managed_windows"
        || actual.pool_id != Some(pool)
        || current["policy"]["required_capabilities"]
            .as_array()
            .ok_or_else(device_invalid)?
            .iter()
            .any(|c| {
                !actual
                    .capabilities
                    .iter()
                    .any(|a| Some(a.name.as_str()) == c.as_str())
            })
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn android_executor_pools(
        &self,
        actor: &Principal,
        org: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .organization_in_transaction(&mut tx, actor, org)
            .await
            .map_err(team_service::identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Read)
            .await
            .map_err(team_service::identity_error)?;
        let rows=sqlx::query("SELECT * FROM android_executor_pool_profiles WHERE organization_id=$1 AND id>$2 ORDER BY id LIMIT $3").bind(org.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for r in rows.iter().take(limit as usize) {
            items.push(view(r)?)
        }
        self.identity()
            .organization_in_transaction(&mut tx, actor, org)
            .await
            .map_err(team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"items":items,"next_cursor":if more{items.last().map(|v|v["id"].clone())}else{None}}),
        )
    }
    pub async fn android_executor_pool_write(
        &self,
        actor: &Principal,
        org: Uuid,
        id: Uuid,
        expected: i64,
        name: &str,
        active: bool,
        config: Value,
        request: Uuid,
    ) -> Result<Value> {
        policy(&config)?;
        if id.is_nil()
            || request.is_nil()
            || expected < 0
            || name.trim().is_empty()
            || name.len() > 200
            || name.chars().any(char::is_control)
        {
            return Err(device_invalid());
        }
        let sha = canonical_hash(
            &json!({"organization_id":org,"id":id,"expected_revision":expected,"name":name,"active":active,"policy":config,"request_id":request}),
        );
        let op = format!("pool-profile:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        admin(self, &mut tx, actor, org).await?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &op,
            "pool_profile",
            &sha,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        let old:Option<i64>=sqlx::query_scalar("SELECT revision FROM android_executor_pool_profiles WHERE id=$1 AND organization_id=$2").bind(id.to_string()).bind(org.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if old.unwrap_or(0) != expected {
            return Err(Error::Conflict);
        }
        let next = expected.checked_add(1).ok_or(Error::Conflict)?;
        if sqlx::query("INSERT INTO android_executor_pool_profiles(id,organization_id,owner_id,revision,active,name,kind,policy,created_at) VALUES($1,$2,$3,$4,$5,$6,'managed_windows',$7,$8) ON CONFLICT(id) DO UPDATE SET revision=$4,active=$5,name=$6,policy=$7 WHERE android_executor_pool_profiles.organization_id=$2 AND android_executor_pool_profiles.revision=$9").bind(id.to_string()).bind(org.to_string()).bind(actor.identity_id().to_string()).bind(next).bind(i64::from(active)).bind(name).bind(json_string(&config)?).bind(now).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        admin(self, &mut tx, actor, org).await?;
        let receipt = json!({"schema_version":2,"id":id,"organization_id":org,"revision":next,"active":active,"request_id":request,"request_sha256":sha});
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &op,
            "pool_profile",
            &sha,
            &receipt,
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn android_executor_pool_projects(
        &self,
        actor: &Principal,
        org: Uuid,
        pool: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        admin(self, &mut tx, actor, org).await?;
        let rows=sqlx::query("SELECT m.* FROM android_executor_pool_projects m JOIN android_executor_pool_profiles p ON p.id=m.pool_id WHERE p.id=$1 AND p.organization_id=$2 AND m.organization_id=$2 AND m.project_id>$3 ORDER BY m.project_id LIMIT $4").bind(pool.to_string()).bind(org.to_string()).bind(after.map(|p|p.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for r in rows.iter().take(limit as usize) {
            items.push(json!({"project_id":r.try_get::<String,_>("project_id").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"active":r.try_get::<i64,_>("active").map_err(storage_error)?==1}));
        }
        admin(self, &mut tx, actor, org).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"items":items,"next_cursor":if more{items.last().map(|v|v["project_id"].clone())}else{None}}),
        )
    }
    pub async fn android_executor_pool_project_write(
        &self,
        actor: &Principal,
        org: Uuid,
        pool: Uuid,
        project: Uuid,
        expected: i64,
        active: bool,
        request: Uuid,
    ) -> Result<Value> {
        if expected < 0 || request.is_nil() {
            return Err(device_invalid());
        }
        let sha = canonical_hash(
            &json!({"organization_id":org,"pool_id":pool,"project_id":project,"expected_revision":expected,"active":active,"request_id":request}),
        );
        let op = format!("pool-project:{request}");
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        admin(self, &mut tx, actor, org).await?;
        if let Some(v) = crate::android_administration_control::original(
            &mut tx,
            actor.identity_id(),
            &op,
            "pool_project",
            &sha,
        )
        .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        self.identity()
            .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Share)
            .await
            .map_err(team_service::identity_error)?;
        let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_executor_pool_profiles WHERE id=$1 AND organization_id=$2").bind(pool.to_string()).bind(org.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if exists != 1 {
            return Err(Error::NotFound);
        }
        let revision:Option<i64>=sqlx::query_scalar("SELECT revision FROM android_executor_pool_projects WHERE pool_id=$1 AND project_id=$2 AND organization_id=$3").bind(pool.to_string()).bind(project.to_string()).bind(org.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if revision.unwrap_or(0) != expected {
            return Err(Error::Conflict);
        }
        let next = expected.checked_add(1).ok_or(Error::Conflict)?;
        if sqlx::query("INSERT INTO android_executor_pool_projects(pool_id,project_id,organization_id,revision,active) VALUES($1,$2,$3,$4,$5) ON CONFLICT(pool_id,project_id) DO UPDATE SET revision=$4,active=$5 WHERE android_executor_pool_projects.revision=$6 AND android_executor_pool_projects.organization_id=$3").bind(pool.to_string()).bind(project.to_string()).bind(org.to_string()).bind(next).bind(i64::from(active)).bind(expected).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        self.identity()
            .authorize_content_in_transaction(&mut tx, actor, org, project, ContentAction::Share)
            .await
            .map_err(team_service::identity_error)?;
        admin(self, &mut tx, actor, org).await?;
        let receipt = json!({"pool_id":pool,"project_id":project,"organization_id":org,"revision":next,"active":active,"request_id":request,"request_sha256":sha});
        crate::android_administration_control::remember(
            &mut tx,
            actor.identity_id(),
            &op,
            "pool_project",
            &sha,
            &receipt,
            now,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
