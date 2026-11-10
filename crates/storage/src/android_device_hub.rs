//! Durable connection fencing and one-use routing grants. These grants carry no
//! user or device credential and are never a substitute for current authority.
use super::*;
use crate::android_desktop::{AndroidDesktopAccess, current_access};
use crate::android_device_registry::{AndroidAgentPrincipal, agent_current};
use crate::android_devices::{device_invalid, device_writer};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidHubOwner {
    pub executor: Uuid,
    pub node: Uuid,
    pub boot: Uuid,
    pub nonce: Uuid,
    pub epoch: i64,
    pub origin: Option<String>,
}
pub struct AndroidHubForward {
    pub owner: AndroidHubOwner,
    pub access: AndroidDesktopAccess,
    pub agent: Option<AndroidAgentPrincipal>,
    pub stream: Option<Uuid>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    run: Uuid,
    desktop: Uuid,
    executor: Uuid,
    credential: Uuid,
    credential_revision: i64,
    lease: Uuid,
    identity: Uuid,
    session: Uuid,
    session_revision: i64,
    control: bool,
    control_generation: Option<Uuid>,
}
impl From<&AndroidDesktopAccess> for Access {
    fn from(v: &AndroidDesktopAccess) -> Self {
        Self {
            run: v.run,
            desktop: v.desktop,
            executor: v.executor,
            credential: v.credential,
            credential_revision: v.credential_revision,
            lease: v.lease,
            identity: v.identity,
            session: v.session,
            session_revision: v.session_revision,
            control: v.control,
            control_generation: v.control_generation,
        }
    }
}
impl From<Access> for AndroidDesktopAccess {
    fn from(v: Access) -> Self {
        Self {
            run: v.run,
            desktop: v.desktop,
            executor: v.executor,
            credential: v.credential,
            credential_revision: v.credential_revision,
            lease: v.lease,
            identity: v.identity,
            session: v.session,
            session_revision: v.session_revision,
            control: v.control,
            control_generation: v.control_generation,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Agent {
    executor_id: Uuid,
    credential_id: Uuid,
    credential_revision: i64,
    owner_id: Uuid,
    device_revision: i64,
}
impl From<&AndroidAgentPrincipal> for Agent {
    fn from(a: &AndroidAgentPrincipal) -> Self {
        Self {
            executor_id: a.executor_id,
            credential_id: a.credential_id,
            credential_revision: a.credential_revision,
            owner_id: a.owner_id,
            device_revision: a.device_revision,
        }
    }
}
impl From<Agent> for AndroidAgentPrincipal {
    fn from(a: Agent) -> Self {
        Self {
            executor_id: a.executor_id,
            credential_id: a.credential_id,
            credential_revision: a.credential_revision,
            owner_id: a.owner_id,
            device_revision: a.device_revision,
        }
    }
}
fn owner(r: &sqlx::any::AnyRow) -> Result<AndroidHubOwner> {
    let uuid = |name: &str| -> Result<Uuid> {
        Uuid::parse_str(&r.try_get::<String, _>(name).map_err(storage_error)?)
            .map_err(storage_error)
    };
    Ok(AndroidHubOwner {
        executor: uuid("executor_id")?,
        node: uuid("node_id")?,
        boot: uuid("boot_nonce")?,
        nonce: uuid("connection_nonce")?,
        epoch: r.try_get("epoch").map_err(storage_error)?,
        origin: r.try_get("origin").map_err(storage_error)?,
    })
}
async fn current(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    expected: &AndroidHubOwner,
    at: i64,
) -> Result<()> {
    let r =
        sqlx::query("SELECT * FROM android_hub_connections WHERE executor_id=$1 AND expires_at>$2")
            .bind(expected.executor.to_string())
            .bind(at)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Conflict)?;
    if owner(&r)? != *expected {
        return Err(Error::Conflict);
    }
    let a = AndroidAgentPrincipal {
        executor_id: expected.executor,
        credential_id: Uuid::parse_str(
            &r.try_get::<String, _>("credential_id")
                .map_err(storage_error)?,
        )
        .map_err(storage_error)?,
        credential_revision: r.try_get("credential_revision").map_err(storage_error)?,
        device_revision: r.try_get("device_revision").map_err(storage_error)?,
        owner_id: Uuid::parse_str(&r.try_get::<String, _>("owner_id").map_err(storage_error)?)
            .map_err(storage_error)?,
    };
    agent_current(tx, &a, at).await?;
    Ok(())
}
async fn for_access(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    access: &AndroidDesktopAccess,
    at: i64,
) -> Result<AndroidHubOwner> {
    current_access(store, tx, access, at).await?;
    let r = sqlx::query("SELECT * FROM android_hub_connections WHERE executor_id=$1")
        .bind(access.executor.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Conflict)?;
    let v = owner(&r)?;
    current(tx, &v, at).await?;
    if r.try_get::<String, _>("credential_id")
        .map_err(storage_error)?
        != access.credential.to_string()
        || r.try_get::<i64, _>("credential_revision")
            .map_err(storage_error)?
            != access.credential_revision
    {
        return Err(Error::Forbidden);
    }
    Ok(v)
}
fn agent_matches(a: &AndroidAgentPrincipal, v: &AndroidDesktopAccess) -> Result<()> {
    if a.executor_id != v.executor
        || a.credential_id != v.credential
        || a.credential_revision != v.credential_revision
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
impl Store {
    pub async fn android_hub_claim(
        &self,
        a: &AndroidAgentPrincipal,
        node: Uuid,
        boot: Uuid,
        origin: Option<&str>,
    ) -> Result<AndroidHubOwner> {
        if node.is_nil() || boot.is_nil() {
            return Err(device_invalid());
        }
        if let Some(s) = origin {
            let u = reqwest::Url::parse(s).map_err(|_| device_invalid())?;
            if u.scheme() != "https"
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || u.path() != "/"
                || s.len() > 1024
            {
                return Err(device_invalid());
            }
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, at).await?;
        let old = sqlx::query("SELECT * FROM android_hub_connections WHERE executor_id=$1")
            .bind(a.executor_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        let epoch = if let Some(r) = old {
            let previous = owner(&r)?;
            match current(&mut tx, &previous, at).await {
                Ok(()) => return Err(Error::Conflict),
                Err(Error::Conflict | Error::Forbidden | Error::NotFound) => {}
                Err(e) => return Err(e),
            }
            r.try_get::<i64, _>("epoch")
                .map_err(storage_error)?
                .checked_add(1)
                .ok_or(Error::Conflict)?
        } else {
            1
        };
        let v = AndroidHubOwner {
            executor: a.executor_id,
            node,
            boot,
            nonce: Uuid::new_v4(),
            epoch,
            origin: origin.map(str::to_owned),
        };
        sqlx::query("INSERT INTO android_hub_connections(executor_id,node_id,boot_nonce,connection_nonce,epoch,origin,credential_id,credential_revision,device_revision,owner_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(executor_id) DO UPDATE SET node_id=excluded.node_id,boot_nonce=excluded.boot_nonce,connection_nonce=excluded.connection_nonce,epoch=excluded.epoch,origin=excluded.origin,credential_id=excluded.credential_id,credential_revision=excluded.credential_revision,device_revision=excluded.device_revision,owner_id=excluded.owner_id,expires_at=excluded.expires_at")
            .bind(a.executor_id.to_string()).bind(node.to_string()).bind(boot.to_string()).bind(v.nonce.to_string()).bind(epoch).bind(&v.origin).bind(a.credential_id.to_string()).bind(a.credential_revision).bind(a.device_revision).bind(a.owner_id.to_string()).bind(at+20_000).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(v)
    }
    pub async fn android_hub_current(&self, v: &AndroidHubOwner) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        current(&mut tx, v, at).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_hub_renew(
        &self,
        a: &AndroidAgentPrincipal,
        v: &AndroidHubOwner,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, at).await?;
        current(&mut tx, v, at).await?;
        if a.executor_id != v.executor {
            return Err(Error::Forbidden);
        }
        let same:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_hub_connections WHERE executor_id=$1 AND connection_nonce=$2 AND epoch=$3 AND credential_id=$4 AND credential_revision=$5 AND device_revision=$6 AND owner_id=$7")
            .bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(a.credential_id.to_string()).bind(a.credential_revision).bind(a.device_revision).bind(a.owner_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if same != 1 {
            return Err(Error::Forbidden);
        }
        sqlx::query("UPDATE android_hub_connections SET expires_at=$1 WHERE executor_id=$2 AND connection_nonce=$3 AND epoch=$4").bind(at+20_000).bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_hub_release(&self, v: &AndroidHubOwner) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        sqlx::query("UPDATE android_hub_connections SET expires_at=0 WHERE executor_id=$1 AND connection_nonce=$2 AND epoch=$3 AND node_id=$4 AND boot_nonce=$5").bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(v.node.to_string()).bind(v.boot.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_hub_owner(&self, v: &AndroidDesktopAccess) -> Result<AndroidHubOwner> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        let owner = for_access(self, &mut tx, v, at).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(owner)
    }
    pub async fn android_hub_stream_register(
        &self,
        v: &AndroidHubOwner,
        a: &AndroidDesktopAccess,
        id: Uuid,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        if for_access(self, &mut tx, a, at).await? != *v {
            return Err(Error::Conflict);
        }
        sqlx::query("DELETE FROM android_hub_streams WHERE expires_at<=$1")
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM android_hub_streams WHERE executor_id=$1")
                .bind(v.executor.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if n >= 32 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO android_hub_streams(id,executor_id,connection_nonce,epoch,access,expires_at) VALUES($1,$2,$3,$4,$5,$6)").bind(id.to_string()).bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(json_string(&Access::from(a))?).bind(at+120_000).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_hub_stream_remove(&self, v: &AndroidHubOwner, id: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM android_hub_streams WHERE id=$1 AND connection_nonce=$2 AND epoch=$3",
        )
        .bind(id.to_string())
        .bind(v.nonce.to_string())
        .bind(v.epoch)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_hub_agent_stream(
        &self,
        a: &AndroidAgentPrincipal,
        id: Uuid,
    ) -> Result<(AndroidHubOwner, AndroidDesktopAccess)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        agent_current(&mut tx, a, at).await?;
        let r = sqlx::query("SELECT * FROM android_hub_streams WHERE id=$1 AND expires_at>$2")
            .bind(id.to_string())
            .bind(at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::Forbidden)?;
        let access: AndroidDesktopAccess =
            parse::<Access>(&r.try_get::<String, _>("access").map_err(storage_error)?)?.into();
        agent_matches(a, &access)?;
        let v = for_access(self, &mut tx, &access, at).await?;
        if r.try_get::<String, _>("connection_nonce")
            .map_err(storage_error)?
            != v.nonce.to_string()
            || r.try_get::<i64, _>("epoch").map_err(storage_error)? != v.epoch
        {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok((v, access))
    }
    pub async fn android_hub_forward(
        &self,
        v: &AndroidHubOwner,
        a: &AndroidDesktopAccess,
        agent: Option<&AndroidAgentPrincipal>,
        stream: Option<Uuid>,
        token: &str,
    ) -> Result<()> {
        if token.len() < 40 || token.len() > 200 || agent.is_some() != stream.is_some() {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        if for_access(self, &mut tx, a, at).await? != *v {
            return Err(Error::Conflict);
        }
        if let Some(agent) = agent {
            agent_current(&mut tx, agent, at).await?;
            agent_matches(agent, a)?;
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_hub_streams WHERE id=$1 AND executor_id=$2 AND connection_nonce=$3 AND epoch=$4 AND expires_at>$5").bind(stream.unwrap().to_string()).bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(at).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count != 1 {
                return Err(Error::Conflict);
            }
        }
        sqlx::query("DELETE FROM android_hub_forwards WHERE expires_at<$1")
            .bind(at - 86_400_000)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_hub_forwards WHERE expires_at>$1 AND consumed=0",
        )
        .bind(at)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 4096 {
            return Err(Error::Conflict);
        }
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM android_hub_forwards")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if retained >= 100_000 {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO android_hub_forwards(token_sha256,executor_id,connection_nonce,epoch,kind,access,agent,stream_id,expires_at,consumed) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,0)").bind(hash(token.as_bytes())).bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(if agent.is_some(){"agent"}else{"desktop"}).bind(json_string(&Access::from(a))?).bind(agent.map(|a|json_string(&Agent::from(a))).transpose()?).bind(stream.map(|v|v.to_string())).bind(at+30_000).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_hub_take_forward(
        &self,
        node: Uuid,
        boot: Uuid,
        token: &str,
        kind: &str,
    ) -> Result<AndroidHubForward> {
        if token.len() < 40 || token.len() > 200 || !matches!(kind, "agent" | "desktop") {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let at = device_writer(&mut tx).await?;
        let r=sqlx::query("SELECT * FROM android_hub_forwards WHERE token_sha256=$1 AND kind=$2 AND consumed=0 AND expires_at>$3").bind(hash(token.as_bytes())).bind(kind).bind(at).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let access: AndroidDesktopAccess =
            parse::<Access>(&r.try_get::<String, _>("access").map_err(storage_error)?)?.into();
        let v = for_access(self, &mut tx, &access, at).await?;
        if v.node != node
            || v.boot != boot
            || r.try_get::<String, _>("connection_nonce")
                .map_err(storage_error)?
                != v.nonce.to_string()
            || r.try_get::<i64, _>("epoch").map_err(storage_error)? != v.epoch
        {
            return Err(Error::Conflict);
        }
        let agent = r
            .try_get::<Option<String>, _>("agent")
            .map_err(storage_error)?
            .map(|s| parse::<Agent>(&s).map(AndroidAgentPrincipal::from))
            .transpose()?;
        let stream = r
            .try_get::<Option<String>, _>("stream_id")
            .map_err(storage_error)?
            .map(|s| Uuid::parse_str(&s).map_err(storage_error))
            .transpose()?;
        if let Some(a) = &agent {
            agent_current(&mut tx, a, at).await?;
            agent_matches(a, &access)?;
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_hub_streams WHERE id=$1 AND executor_id=$2 AND connection_nonce=$3 AND epoch=$4 AND expires_at>$5").bind(stream.ok_or(Error::Conflict)?.to_string()).bind(v.executor.to_string()).bind(v.nonce.to_string()).bind(v.epoch).bind(at).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if count != 1 {
                return Err(Error::Conflict);
            }
        }
        let n = sqlx::query(
            "UPDATE android_hub_forwards SET consumed=1 WHERE token_sha256=$1 AND consumed=0",
        )
        .bind(hash(token.as_bytes()))
        .execute(&mut *tx)
        .await
        .map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(AndroidHubForward {
            owner: v,
            access,
            agent,
            stream,
        })
    }
}
