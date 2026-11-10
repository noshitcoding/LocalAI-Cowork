//! Short-lived, one-use desktop grants bound to the actual device lease and user ACL.
use super::*;
use crate::android_device_registry::{AndroidAgentPrincipal, agent_current};
use crate::android_device_runs::{
    device_lease_current, device_run_authority, device_run_permit, device_run_row,
};
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_identity::{ContentAction, Principal};
use sqlx::{Any, Transaction};
use team_service::identity_error;
pub struct AndroidDesktopAccess {
    pub run: Uuid,
    pub desktop: Uuid,
    pub executor: Uuid,
    pub credential: Uuid,
    pub credential_revision: i64,
    pub lease: Uuid,
    pub identity: Uuid,
    pub session: Uuid,
    pub session_revision: i64,
    pub control: bool,
    pub(crate) control_generation: Option<Uuid>,
}
pub(crate) async fn current_access(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    v: &AndroidDesktopAccess,
    current: i64,
) -> Result<()> {
    let r = device_run_row(tx, v.run).await?;
    device_run_authority(store, tx, &r, current).await?;
    if r.try_get::<String, _>("executor_id")
        .map_err(storage_error)?
        != v.executor.to_string()
        || r.try_get::<Option<String>, _>("credential_id")
            .map_err(storage_error)?
            != Some(v.credential.to_string())
        || r.try_get::<Option<i64>, _>("credential_revision")
            .map_err(storage_error)?
            != Some(v.credential_revision)
        || r.try_get::<Option<String>, _>("lease_token")
            .map_err(storage_error)?
            != Some(v.lease.to_string())
        || r.try_get::<Option<i64>, _>("lease_expires_at")
            .map_err(storage_error)?
            .is_none_or(|e| e <= current)
        || !matches!(
            r.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "running" | "leased"
        )
    {
        return Err(Error::Forbidden);
    }
    if let Some(team) = r
        .try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?
    {
        store
            .identity()
            .validate_retained_team_device_actor_in_transaction(
                tx,
                (v.identity, v.session, v.session_revision),
                Uuid::parse_str(&team).map_err(storage_error)?,
                Uuid::parse_str(
                    &r.try_get::<String, _>("project_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                if v.control {
                    ContentAction::Execute
                } else {
                    ContentAction::Read
                },
            )
            .await
            .map_err(identity_error)?;
    }
    let bit = if v.control { 4i64 } else { 1 };
    let count: i64 = if r
        .try_get::<Option<String>, _>("team_id")
        .map_err(storage_error)?
        .is_none()
    {
        sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN android_personal_projects p ON p.owner_id=i.id JOIN android_devices d ON d.identity_id=i.id JOIN android_device_credentials c ON c.executor_id=d.id WHERE i.id=$1 AND i.active=1 AND s.id=$2 AND s.revision=$3 AND s.revoked=0 AND s.expires_at>$4 AND NOT EXISTS(SELECT 1 FROM identity_session_scopes ss WHERE ss.session_id=s.id) AND p.id=$5 AND p.deleted=0 AND d.id=$6 AND d.team_id IS NULL AND d.revoked=0 AND d.execution_revision=$7 AND c.id=$8 AND c.revision=$9 AND c.revoked=0 AND c.expires_at>$4")
            .bind(v.identity.to_string()).bind(v.session.to_string()).bind(v.session_revision).bind(current).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).bind(v.executor.to_string()).bind(r.try_get::<i64,_>("executor_revision").map_err(storage_error)?).bind(v.credential.to_string()).bind(v.credential_revision).fetch_one(&mut **tx).await.map_err(storage_error)?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM identity_subjects i JOIN identity_sessions s ON s.identity_id=i.id JOIN identity_memberships m ON m.identity_id=i.id JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_content_grants g ON g.membership_id=m.id JOIN identity_resources p ON p.id=g.resource_id AND p.organization_id=o.id JOIN android_devices d ON d.id=$1 JOIN android_device_credentials c ON c.executor_id=d.id WHERE i.id=$2 AND i.active=1 AND s.id=$3 AND s.revision=$4 AND s.revoked=0 AND s.expires_at>$5 AND o.id=$6 AND o.state='active' AND m.active=1 AND (NOT $7=1 OR m.role<>'viewer') AND p.id=$8 AND g.active=1 AND (g.permissions & $9)=$9 AND d.revoked=0 AND d.execution_revision=$10 AND c.id=$11 AND c.revision=$12 AND c.revoked=0 AND c.expires_at>$5")
        .bind(v.executor.to_string()).bind(v.identity.to_string()).bind(v.session.to_string()).bind(v.session_revision).bind(current).bind(r.try_get::<String,_>("team_id").map_err(storage_error)?).bind(i64::from(v.control)).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).bind(bit).bind(r.try_get::<i64,_>("executor_revision").map_err(storage_error)?).bind(v.credential.to_string()).bind(v.credential_revision).fetch_one(&mut **tx).await.map_err(storage_error)?
    };
    if count != 1 {
        return Err(Error::Forbidden);
    }
    let d=sqlx::query("SELECT state,payload,controller_generation FROM android_scoped_desktop_sessions WHERE id=$1 AND run_id=$2 AND executor_id=$3 AND credential_id=$4 AND credential_revision=$5 AND lease_token=$6").bind(v.desktop.to_string()).bind(v.run.to_string()).bind(v.executor.to_string()).bind(v.credential.to_string()).bind(v.credential_revision).bind(v.lease.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
    if !matches!(
        d.try_get::<String, _>("state")
            .map_err(storage_error)?
            .as_str(),
        "starting" | "agent_controlled" | "user_controlled"
    ) {
        return Err(Error::Forbidden);
    }
    let payload: Value = parse(&d.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if v.control
        && (payload["controller_user_id"].as_str() != Some(v.identity.to_string().as_str())
            || d.try_get::<Option<String>, _>("controller_generation")
                .map_err(storage_error)?
                != v.control_generation.map(|id| id.to_string()))
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn android_desktop_start(
        &self,
        actor: &Principal,
        run: Uuid,
        width: u32,
        height: u32,
    ) -> Result<Value> {
        if !(320..=7680).contains(&width) || !(200..=4320).contains(&height) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        device_run_authority(self, &mut tx, &r, current).await?;
        if r.try_get::<String, _>("state").map_err(storage_error)? != "running"
            || r.try_get::<Option<i64>, _>("lease_expires_at")
                .map_err(storage_error)?
                .is_none_or(|e| e <= current)
        {
            return Err(Error::Conflict);
        }
        let registration:String=sqlx::query_scalar("SELECT registration FROM android_devices WHERE id=$1 AND revoked=0 AND execution_revision=$2").bind(r.try_get::<String,_>("executor_id").map_err(storage_error)?).bind(r.try_get::<i64,_>("executor_revision").map_err(storage_error)?).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let reg: DeviceExecutorRegistration = parse(&registration)?;
        if !reg
            .capabilities
            .iter()
            .any(|c| matches!(c.name.as_str(), "desktop.windows" | "desktop.linux"))
            || reg.personal_device_remote_control.as_deref() == Some("off")
        {
            return Err(Error::Forbidden);
        }
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_scoped_desktop_sessions WHERE run_id=$1 AND state IN('starting','agent_controlled','user_controlled')").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count >= 1 {
            return Err(Error::Conflict);
        }
        let id = Uuid::new_v4();
        let v = json!({"schema_version":2,"id":id,"run_id":run,"executor_id":reg.executor_id,"state":"starting","stream_protocol":"rfb.binary.v1","dimensions":{"width":width,"height":height,"scale_factor":1},"controller_user_id":null,"created_at":device_iso(current)?,"ended_at":null});
        sqlx::query("INSERT INTO android_scoped_desktop_sessions(id,run_id,executor_id,credential_id,credential_revision,lease_token,revision,state,payload,created_at) VALUES($1,$2,$3,$4,$5,$6,1,'starting',$7,$8)").bind(id.to_string()).bind(run.to_string()).bind(reg.executor_id.to_string()).bind(r.try_get::<Option<String>,_>("credential_id").map_err(storage_error)?.ok_or(Error::Conflict)?).bind(r.try_get::<Option<i64>,_>("credential_revision").map_err(storage_error)?.ok_or(Error::Conflict)?).bind(r.try_get::<Option<String>,_>("lease_token").map_err(storage_error)?.ok_or(Error::Conflict)?).bind(json_string(&v)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_desktops(&self, actor: &Principal, run: Uuid) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Read).await?;
        let rows=sqlx::query("SELECT payload FROM android_scoped_desktop_sessions WHERE run_id=$1 ORDER BY created_at LIMIT 100").bind(run.to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| parse(&r.try_get::<String, _>("payload").map_err(storage_error)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_desktop_stop(
        &self,
        actor: &Principal,
        run: Uuid,
        id: Uuid,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(self, &mut tx, actor, &r, ContentAction::Execute).await?;
        let raw: String = sqlx::query_scalar(
            "SELECT payload FROM android_scoped_desktop_sessions WHERE id=$1 AND run_id=$2",
        )
        .bind(id.to_string())
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let mut v: Value = parse(&raw)?;
        v["state"] = json!("ended");
        v["ended_at"] = json!(device_iso(current)?);
        v["controller_user_id"] = Value::Null;
        sqlx::query("UPDATE android_scoped_desktop_sessions SET state='ended',ended_at=$1,revision=revision+1,payload=$2 WHERE id=$3 AND run_id=$4").bind(current).bind(json_string(&v)?).bind(id.to_string()).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_reauthentication_grant(
        &self,
        actor: &Principal,
        token: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM identity_sessions WHERE id=$1 AND identity_id=$2",
        )
        .bind(actor.session_id().to_string())
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let expiry = current + 60_000;
        sqlx::query("INSERT INTO android_device_tickets(token_sha256,identity_id,session_id,session_revision,kind,payload,expires_at,consumed) VALUES($1,$2,$3,$4,'desktop_reauthentication','{}',$5,0)").bind(hash(token.as_bytes())).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(revision).bind(expiry).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"token":token,"purpose":"desktop_control","expires_at":device_iso(expiry)?}),
        )
    }
    pub async fn android_desktop_ticket(
        &self,
        actor: &Principal,
        run: Uuid,
        id: Uuid,
        control: bool,
        reauth: Option<&str>,
        token: &str,
    ) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let r = device_run_row(&mut tx, run).await?;
        device_run_permit(
            self,
            &mut tx,
            actor,
            &r,
            if control {
                ContentAction::Execute
            } else {
                ContentAction::Read
            },
        )
        .await?;
        device_run_authority(self, &mut tx, &r, current).await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1")
                .bind(actor.session_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if control {
            let grant = reauth.ok_or(Error::Forbidden)?;
            let n=sqlx::query("UPDATE android_device_tickets SET consumed=1 WHERE token_sha256=$1 AND kind='desktop_reauthentication' AND identity_id=$2 AND session_id=$3 AND session_revision=$4 AND consumed=0 AND expires_at>$5").bind(hash(grant.as_bytes())).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(revision).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
            if n.rows_affected() != 1 {
                return Err(Error::Forbidden);
            }
        }
        let d = sqlx::query(
            "SELECT payload,state FROM android_scoped_desktop_sessions WHERE id=$1 AND run_id=$2",
        )
        .bind(id.to_string())
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        if !matches!(
            d.try_get::<String, _>("state")
                .map_err(storage_error)?
                .as_str(),
            "starting" | "agent_controlled" | "user_controlled"
        ) {
            return Err(Error::Conflict);
        }
        let mut payload: Value = parse(&d.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let control_generation = control.then(Uuid::new_v4);
        if control {
            if payload["controller_user_id"]
                .as_str()
                .is_some_and(|v| v != actor.identity_id().to_string())
            {
                return Err(Error::Conflict);
            }
            payload["controller_user_id"] = json!(actor.identity_id());
            sqlx::query("UPDATE android_scoped_desktop_sessions SET payload=$1,revision=revision+1,controller_generation=$4 WHERE id=$2 AND run_id=$3").bind(json_string(&payload)?).bind(id.to_string()).bind(run.to_string()).bind(control_generation.map(|id|id.to_string())).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let expiry = current + 30_000;
        sqlx::query("INSERT INTO android_device_tickets(token_sha256,identity_id,session_id,session_revision,kind,run_id,desktop_id,payload,expires_at,consumed) VALUES($1,$2,$3,$4,'desktop_stream',$5,$6,$7,$8,0)").bind(hash(token.as_bytes())).bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(revision).bind(run.to_string()).bind(id.to_string()).bind(json_string(&json!({"control":control,"control_generation":control_generation}))?).bind(expiry).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"schema_version":2,"token":token,"control":control,"expires_at":device_iso(expiry)?}),
        )
    }
    pub async fn android_desktop_consume(
        &self,
        id: Uuid,
        token: &str,
    ) -> Result<AndroidDesktopAccess> {
        if token.len() > 200 {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let t=sqlx::query("SELECT * FROM android_device_tickets WHERE token_sha256=$1 AND desktop_id=$2 AND kind='desktop_stream' AND consumed=0 AND expires_at>$3").bind(hash(token.as_bytes())).bind(id.to_string()).bind(current).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let d =
            sqlx::query("SELECT * FROM android_scoped_desktop_sessions WHERE id=$1 AND run_id=$2")
                .bind(id.to_string())
                .bind(t.try_get::<String, _>("run_id").map_err(storage_error)?)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let uuid = |s: String| Uuid::parse_str(&s).map_err(storage_error);
        let payload: Value = parse(&t.try_get::<String, _>("payload").map_err(storage_error)?)?;
        let v = AndroidDesktopAccess {
            run: uuid(d.try_get("run_id").map_err(storage_error)?)?,
            desktop: id,
            executor: uuid(d.try_get("executor_id").map_err(storage_error)?)?,
            credential: uuid(d.try_get("credential_id").map_err(storage_error)?)?,
            credential_revision: d.try_get("credential_revision").map_err(storage_error)?,
            lease: uuid(d.try_get("lease_token").map_err(storage_error)?)?,
            identity: uuid(t.try_get("identity_id").map_err(storage_error)?)?,
            session: uuid(t.try_get("session_id").map_err(storage_error)?)?,
            session_revision: t.try_get("session_revision").map_err(storage_error)?,
            control: payload["control"].as_bool().ok_or(Error::Conflict)?,
            control_generation: payload["control_generation"]
                .as_str()
                .map(|id| Uuid::parse_str(id).map_err(storage_error))
                .transpose()?,
        };
        current_access(self, &mut tx, &v, current).await?;
        let n = sqlx::query(
            "UPDATE android_device_tickets SET consumed=1 WHERE token_sha256=$1 AND consumed=0",
        )
        .bind(hash(token.as_bytes()))
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_desktop_access_current(&self, v: &AndroidDesktopAccess) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        current_access(self, &mut tx, v, current).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_desktop_agent_current(
        &self,
        a: &AndroidAgentPrincipal,
        v: &AndroidDesktopAccess,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, current).await?;
        if a.executor_id != v.executor
            || a.credential_id != v.credential
            || a.credential_revision != v.credential_revision
        {
            return Err(Error::Forbidden);
        }
        device_lease_current(self, &mut tx, a, v.run, v.lease, current).await?;
        current_access(self, &mut tx, v, current).await?;
        let raw: String =
            sqlx::query_scalar("SELECT payload FROM android_scoped_desktop_sessions WHERE id=$1")
                .bind(v.desktop.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let mut payload: Value = parse(&raw)?;
        let state = if v.control || payload["controller_user_id"].as_str().is_some() {
            "user_controlled"
        } else {
            "agent_controlled"
        };
        payload["state"] = json!(state);
        sqlx::query("UPDATE android_scoped_desktop_sessions SET state=$1,payload=$2,revision=revision+1 WHERE id=$3").bind(state).bind(json_string(&payload)?).bind(v.desktop.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_desktop_stream_finished(&self, v: &AndroidDesktopAccess) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let row=sqlx::query("SELECT payload,controller_generation,state FROM android_scoped_desktop_sessions WHERE id=$1 AND run_id=$2").bind(v.desktop.to_string()).bind(v.run.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = row {
            let current = row
                .try_get::<Option<String>, _>("controller_generation")
                .map_err(storage_error)?;
            let mut payload: Value =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if v.control
                && current == v.control_generation.map(|id| id.to_string())
                && payload["controller_user_id"].as_str() == Some(v.identity.to_string().as_str())
            {
                payload["state"] = json!("agent_controlled");
                payload["controller_user_id"] = Value::Null;
                sqlx::query("UPDATE android_scoped_desktop_sessions SET state='agent_controlled',payload=$1,controller_generation=NULL,revision=revision+1 WHERE id=$2 AND state IN('starting','agent_controlled','user_controlled')").bind(json_string(&payload)?).bind(v.desktop.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
            }
        }
        tx.commit().await.map_err(storage_error)
    }
}
