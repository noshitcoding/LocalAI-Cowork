//! Explicit V2 device pairing. An executor credential is not a user or owner session.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction, any::AnyRow};
use team_service::identity_error;

pub struct AndroidAgentPrincipal {
    pub executor_id: Uuid,
    pub credential_id: Uuid,
    pub credential_revision: i64,
    pub(crate) owner_id: Uuid,
    pub(crate) device_revision: i64,
}
fn registration(v: &DeviceExecutorRegistration) -> Result<()> {
    if v.schema_version != 2
        || v.protocol_version != 2
        || v.executor_id.is_nil()
        || v.display_name.trim().is_empty()
        || v.display_name.len() > 200
        || v.display_name.chars().any(char::is_control)
        || !(1..=16).contains(&v.max_concurrent_runs)
        || v.capabilities.len() > 64
        || v.labels.len() > 32
    {
        return Err(device_invalid());
    }
    match v.kind.as_str() {
        "personal_device" if v.pool_id.is_none() => (),
        "managed_windows" if v.pool_id.is_some() => (),
        _ => return Err(device_invalid()),
    }
    if v.personal_device_remote_control
        .as_deref()
        .is_some_and(|s| !matches!(s, "off" | "confirm_each_session" | "unattended"))
    {
        return Err(device_invalid());
    }
    let mut names = std::collections::BTreeSet::new();
    for c in &v.capabilities {
        if !names.insert(&c.name)
            || c.schema_version != 2
            || c.name.is_empty()
            || c.name.len() > 100
            || !c
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || c.version.len() > 100
            || json_string(&c.attributes)?.len() > 8192
        {
            return Err(device_invalid());
        }
    }
    if json_string(v)?.len() > 65536 {
        return Err(device_invalid());
    }
    Ok(())
}
pub(crate) async fn agent_current(
    tx: &mut Transaction<'_, Any>,
    a: &AndroidAgentPrincipal,
    current: i64,
) -> Result<DeviceExecutorRegistration> {
    let row=sqlx::query("SELECT d.registration,d.identity_id,d.execution_revision AS revision,d.revoked,c.revision AS credential_revision,c.revoked AS credential_revoked,c.expires_at,i.active FROM android_devices d JOIN android_device_credentials c ON c.executor_id=d.id JOIN identity_subjects i ON i.id=d.identity_id WHERE d.id=$1 AND c.id=$2 AND c.identity_id=d.identity_id")
        .bind(a.executor_id.to_string()).bind(a.credential_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
    if row
        .try_get::<String, _>("identity_id")
        .map_err(storage_error)?
        != a.owner_id.to_string()
        || row.try_get::<i64, _>("revision").map_err(storage_error)? != a.device_revision
        || row
            .try_get::<i64, _>("credential_revision")
            .map_err(storage_error)?
            != a.credential_revision
        || row.try_get::<i64, _>("revoked").map_err(storage_error)? != 0
        || row
            .try_get::<i64, _>("credential_revoked")
            .map_err(storage_error)?
            != 0
        || row.try_get::<i64, _>("active").map_err(storage_error)? != 1
        || row.try_get::<i64, _>("expires_at").map_err(storage_error)? <= current
    {
        return Err(Error::Forbidden);
    }
    parse(
        &row.try_get::<String, _>("registration")
            .map_err(storage_error)?,
    )
}
fn wire(r: &AnyRow, current: i64, active: i64) -> Result<Value> {
    Ok(
        json!({"registration":parse::<Value>(&r.try_get::<String,_>("registration").map_err(storage_error)?)?,"online":r.try_get::<i64,_>("revoked").map_err(storage_error)?==0&&r.try_get::<i64,_>("last_seen_at").map_err(storage_error)?+30_000>current,"draining":r.try_get::<i64,_>("draining").map_err(storage_error)?==1,"active_runs":active,"last_seen_at":device_iso(r.try_get("last_seen_at").map_err(storage_error)?)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"revoked":r.try_get::<i64,_>("revoked").map_err(storage_error)?==1,"schema_version":2,"team_id":r.try_get::<Option<String>,_>("team_id").map_err(storage_error)?}),
    )
}
pub(crate) async fn device_owner(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    id: Uuid,
) -> Result<AnyRow> {
    store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    let row = sqlx::query("SELECT * FROM android_devices WHERE id=$1 AND identity_id=$2")
        .bind(id.to_string())
        .bind(actor.identity_id().to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    if let Some(team) = row
        .try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?
    {
        store
            .identity()
            .organization_in_transaction(tx, actor, Uuid::parse_str(&team).map_err(storage_error)?)
            .await
            .map_err(identity_error)?;
    } else {
        store
            .identity()
            .require_global_session_in_transaction(tx, actor)
            .await
            .map_err(identity_error)?;
    }
    Ok(row)
}
impl Store {
    pub async fn android_pair_device(
        &self,
        actor: &Principal,
        input: PairDeviceExecutor,
    ) -> Result<Value> {
        registration(&input.registration)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let mut r = input.registration;
        if r.owner_user_id.is_some_and(|id| id != actor.identity_id()) {
            return Err(Error::Forbidden);
        }
        r.owner_user_id = Some(actor.identity_id());
        let pairing_digest = hash(json_string(&(&r, input.team_id))?.as_bytes());
        if let Some(team) = input.team_id {
            self.identity()
                .organization_in_transaction(&mut tx, actor, team)
                .await
                .map_err(identity_error)?;
            if r.kind == "managed_windows" {
                let role:String=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE organization_id=$1 AND identity_id=$2 AND active=1").bind(team.to_string()).bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if !matches!(role.as_str(), "owner" | "admin") {
                    return Err(Error::Forbidden);
                }
            }
        } else {
            self.identity()
                .require_global_session_in_transaction(&mut tx, actor)
                .await
                .map_err(identity_error)?;
            if r.kind != "personal_device" {
                return Err(device_invalid());
            }
        }
        let old = sqlx::query("SELECT * FROM android_devices WHERE id=$1")
            .bind(r.executor_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        if let Some(old) = old {
            if old
                .try_get::<String, _>("identity_id")
                .map_err(storage_error)?
                != actor.identity_id().to_string()
                || old
                    .try_get::<String, _>("pairing_request_sha256")
                    .map_err(storage_error)?
                    != pairing_digest
                || old
                    .try_get::<Option<String>, _>("team_id")
                    .map_err(storage_error)?
                    != input.team_id.map(|v| v.to_string())
            {
                return Err(Error::Conflict);
            }
            let result = wire(&old, current, 0)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(result);
        }
        sqlx::query("INSERT INTO android_devices(id,identity_id,team_id,registration,pairing_request_sha256,revision,execution_revision,draining,revoked,last_seen_at) VALUES($1,$2,$3,$4,$5,1,1,0,0,0)").bind(r.executor_id.to_string()).bind(actor.identity_id().to_string()).bind(input.team_id.map(|t|t.to_string())).bind(json_string(&r)?).bind(pairing_digest).execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM android_devices WHERE id=$1")
            .bind(r.executor_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = wire(&row, current, 0)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_devices(&self, actor: &Principal) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT d.*,(SELECT COUNT(*) FROM android_scoped_runs r WHERE r.executor_id=d.id AND r.state IN('leased','running')) AS active_runs FROM android_devices d WHERE ($2 IS NULL OR d.team_id=$2) AND (d.identity_id=$1 OR EXISTS(SELECT 1 FROM identity_memberships m JOIN identity_organizations o ON o.id=m.organization_id WHERE m.organization_id=d.team_id AND m.identity_id=$1 AND m.active=1 AND o.state='active')) ORDER BY d.id LIMIT 200").bind(actor.identity_id().to_string()).bind(scope.map(|s|s.to_string())).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| wire(r, now(), r.try_get("active_runs").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_update_device(
        &self,
        actor: &Principal,
        id: Uuid,
        v: UpdateDeviceExecutor,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_owner(self, &mut tx, actor, id).await?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(identity_error)?;
        device_writer(&mut tx).await?;
        let n=sqlx::query("UPDATE android_devices SET draining=$1,revoked=$2,revision=revision+1,execution_revision=execution_revision+CASE WHEN revoked<>$2 THEN 1 ELSE 0 END WHERE id=$3 AND identity_id=$4 AND revision=$5").bind(i64::from(v.draining)).bind(i64::from(v.revoked)).bind(id.to_string()).bind(actor.identity_id().to_string()).bind(v.expected_revision).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_create_device_credential(
        &self,
        actor: &Principal,
        id: Uuid,
        label: &str,
        expires: Option<i64>,
        token: &str,
    ) -> Result<Value> {
        if label.trim().is_empty() || label.len() > 100 || token.len() < 40 || token.len() > 200 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let r = device_owner(self, &mut tx, actor, id).await?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        if r.try_get::<i64, _>("revoked").map_err(storage_error)? != 0 {
            return Err(Error::Forbidden);
        }
        let expiry = expires.unwrap_or(current + 90 * 86400_000);
        if expiry <= current || expiry > current + 366 * 86400_000 {
            return Err(device_invalid());
        }
        let credential = Uuid::new_v4();
        sqlx::query("INSERT INTO android_device_credentials(id,executor_id,identity_id,token_sha256,label,revision,expires_at,revoked) VALUES($1,$2,$3,$4,$5,1,$6,0)").bind(credential.to_string()).bind(id.to_string()).bind(actor.identity_id().to_string()).bind(hash(token.as_bytes())).bind(label.trim()).bind(expiry).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"credential_id":credential,"executor_id":id,"token":token,"expires_at":device_iso(expiry)?}),
        )
    }
    pub async fn android_device_credentials(
        &self,
        actor: &Principal,
        id: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_owner(self, &mut tx, actor, id).await?;
        let rows=sqlx::query("SELECT id,label,revision,expires_at,revoked FROM android_device_credentials WHERE executor_id=$1 AND identity_id=$2 ORDER BY id LIMIT 100").bind(id.to_string()).bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values=rows.iter().map(|r|Ok(json!({"id":r.try_get::<String,_>("id").map_err(storage_error)?,"executor_id":id,"label":r.try_get::<String,_>("label").map_err(storage_error)?,"revision":r.try_get::<i64,_>("revision").map_err(storage_error)?,"expires_at":device_iso(r.try_get("expires_at").map_err(storage_error)?)?,"revoked":r.try_get::<i64,_>("revoked").map_err(storage_error)?==1}))).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_revoke_device_credential(
        &self,
        actor: &Principal,
        id: Uuid,
        credential: Uuid,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_owner(self, &mut tx, actor, id).await?;
        self.identity()
            .require_session_action_in_transaction(&mut tx, actor, ContentAction::Write)
            .await
            .map_err(identity_error)?;
        device_writer(&mut tx).await?;
        let n=sqlx::query("UPDATE android_device_credentials SET revoked=1,revision=revision+1 WHERE id=$1 AND executor_id=$2 AND identity_id=$3 AND revoked=0").bind(credential.to_string()).bind(id.to_string()).bind(actor.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_agent_authenticate(
        &self,
        id: Uuid,
        token: &str,
    ) -> Result<AndroidAgentPrincipal> {
        if token.len() < 40 || token.len() > 200 {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let r=sqlx::query("SELECT c.id,c.identity_id,c.revision,d.execution_revision AS device_revision FROM android_device_credentials c JOIN android_devices d ON d.id=c.executor_id WHERE c.executor_id=$1 AND c.token_sha256=$2 AND c.revoked=0 AND d.revoked=0 AND c.expires_at>$3").bind(id.to_string()).bind(hash(token.as_bytes())).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let a = AndroidAgentPrincipal {
            executor_id: id,
            credential_id: Uuid::parse_str(&r.try_get::<String, _>("id").map_err(storage_error)?)
                .map_err(storage_error)?,
            credential_revision: r.try_get("revision").map_err(storage_error)?,
            owner_id: Uuid::parse_str(
                &r.try_get::<String, _>("identity_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?,
            device_revision: r.try_get("device_revision").map_err(storage_error)?,
        };
        agent_current(&mut tx, &a, current).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(a)
    }
    pub async fn android_agent_register(
        &self,
        a: &AndroidAgentPrincipal,
        v: DeviceExecutorRegistration,
    ) -> Result<Value> {
        registration(&v)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let actual = agent_current(&mut tx, a, current).await?;
        if v.executor_id != actual.executor_id
            || v.kind != actual.kind
            || v.pool_id != actual.pool_id
            || v.owner_user_id
                .is_some_and(|id| Some(id) != actual.owner_user_id)
            || v.personal_device_remote_control != actual.personal_device_remote_control
            || v.max_concurrent_runs > actual.max_concurrent_runs
            || v.capabilities.len() != actual.capabilities.len()
            || v.capabilities
                .iter()
                .any(|v| !actual.capabilities.iter().any(|c| c.name == v.name))
        {
            return Err(Error::Conflict);
        }
        let mut observed = v;
        observed.owner_user_id = actual.owner_user_id;
        sqlx::query("UPDATE android_devices SET last_seen_at=$1,registration=$2 WHERE id=$3 AND execution_revision=$4").bind(current).bind(json_string(&observed)?).bind(a.executor_id.to_string()).bind(a.device_revision).execute(&mut *tx).await.map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM android_devices WHERE id=$1")
            .bind(a.executor_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = wire(&row, current, 0)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_agent_heartbeat(
        &self,
        a: &AndroidAgentPrincipal,
        protocol: u16,
        active: &[Uuid],
    ) -> Result<Value> {
        if protocol != 2 || active.len() > 16 {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, current).await?;
        for id in active {
            let n:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_runs WHERE id=$1 AND executor_id=$2 AND credential_id=$3 AND credential_revision=$4 AND state IN('leased','running') AND lease_expires_at>$5").bind(id.to_string()).bind(a.executor_id.to_string()).bind(a.credential_id.to_string()).bind(a.credential_revision).bind(current).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if n != 1 {
                return Err(Error::Conflict);
            }
        }
        sqlx::query("UPDATE android_devices SET last_seen_at=$1 WHERE id=$2")
            .bind(current)
            .bind(a.executor_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let row = sqlx::query("SELECT * FROM android_devices WHERE id=$1")
            .bind(a.executor_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let result = wire(&row, current, active.len() as i64)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
