//! Durable private push subscriptions and one-shot delivery attempts.
use super::*;
use crate::android_devices::{device_invalid, device_iso, device_writer};
use cowork_contracts::DevicePushSubscription;
use cowork_identity::Principal;
use team_service::identity_error;

pub struct AndroidPushAttempt {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub identity_id: Uuid,
    pub device_id: Uuid,
    pub revision: i64,
    pub sealed_token: String,
    pub payload: Value,
}
impl Store {
    pub async fn android_push_register<F>(
        &self,
        actor: &Principal,
        device: Uuid,
        digest: &str,
        seal: F,
    ) -> Result<DevicePushSubscription>
    where
        F: FnOnce(Uuid, i64) -> Result<String>,
    {
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(device_invalid());
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        let foreign: Option<String> = sqlx::query_scalar(
            "SELECT identity_id FROM android_push_subscriptions WHERE token_sha256=$1",
        )
        .bind(digest)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        if foreign.is_some_and(|id| id != actor.identity_id().to_string()) {
            return Err(Error::Forbidden);
        }
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .map(|s| s.to_string());
        let old=sqlx::query("SELECT id,revision,created_at,organization_id FROM android_push_subscriptions WHERE identity_id=$1 AND device_id=$2").bind(actor.identity_id().to_string()).bind(device.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let (Some(scope), Some(old)) = (&scope, &old) {
            if old
                .try_get::<Option<String>, _>("organization_id")
                .map_err(storage_error)?
                .as_ref()
                != Some(scope)
            {
                return Err(Error::Forbidden);
            }
        }
        let (id, revision, created) = if let Some(r) = old {
            (
                Uuid::parse_str(&r.try_get::<String, _>("id").map_err(storage_error)?)
                    .map_err(storage_error)?,
                r.try_get::<i64, _>("revision")
                    .map_err(storage_error)?
                    .checked_add(1)
                    .ok_or(Error::Conflict)?,
                r.try_get::<i64, _>("created_at").map_err(storage_error)?,
            )
        } else {
            (Uuid::new_v4(), 1, current)
        };
        let ciphertext = seal(id, revision)?;
        if ciphertext.len() > 16384 {
            return Err(device_invalid());
        }
        let sealed: cowork_secrets::SealedCredential = parse(&ciphertext)?;
        sealed.validate().map_err(|_| Error::NativeKeyUnavailable)?;
        crate::server_keys::cipher_writer(&mut tx, sealed.key_id()).await?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        sqlx::query("INSERT INTO android_push_subscriptions(id,identity_id,device_id,token_sha256,sealed_token,revision,created_at,active,organization_id) VALUES($1,$2,$3,$4,$5,$6,$7,1,$8) ON CONFLICT(identity_id,device_id) DO UPDATE SET token_sha256=excluded.token_sha256,sealed_token=excluded.sealed_token,revision=excluded.revision,active=1,organization_id=excluded.organization_id WHERE android_push_subscriptions.id=excluded.id")
            .bind(id.to_string()).bind(actor.identity_id().to_string()).bind(device.to_string()).bind(digest).bind(ciphertext).bind(revision).bind(created).bind(scope).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(DevicePushSubscription {
            schema_version: 2,
            id,
            device_id: device,
            provider: "fcm".into(),
            created_at: device_iso(created)?,
            last_success_at: None,
        })
    }
    pub async fn android_push_subscriptions(
        &self,
        actor: &Principal,
    ) -> Result<Vec<DevicePushSubscription>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .map(|s| s.to_string());
        let rows=sqlx::query("SELECT id,device_id,created_at,last_success_at FROM android_push_subscriptions WHERE identity_id=$1 AND active=1 AND ($2 IS NULL OR organization_id=$2) ORDER BY created_at LIMIT 100").bind(actor.identity_id().to_string()).bind(scope).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let values = rows
            .iter()
            .map(|r| {
                Ok(DevicePushSubscription {
                    schema_version: 2,
                    id: Uuid::parse_str(&r.try_get::<String, _>("id").map_err(storage_error)?)
                        .map_err(storage_error)?,
                    device_id: Uuid::parse_str(
                        &r.try_get::<String, _>("device_id").map_err(storage_error)?,
                    )
                    .map_err(storage_error)?,
                    provider: "fcm".into(),
                    created_at: device_iso(r.try_get("created_at").map_err(storage_error)?)?,
                    last_success_at: r
                        .try_get::<Option<i64>, _>("last_success_at")
                        .map_err(storage_error)?
                        .map(device_iso)
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_push_remove(&self, actor: &Principal, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        device_writer(&mut tx).await?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .map(|s| s.to_string());
        let n=sqlx::query("UPDATE android_push_subscriptions SET active=0,revision=revision+1 WHERE id=$1 AND identity_id=$2 AND active=1 AND ($3 IS NULL OR organization_id=$3)").bind(id.to_string()).bind(actor.identity_id().to_string()).bind(scope).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_push_queue_terminal_runs(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        // Notification content is deliberately generic. No prompt, response, title or artifact leaves the server.
        let rows=sqlx::query("SELECT s.id AS subscription_id,s.identity_id,s.revision,r.id AS run_id,r.team_id,r.project_id,r.status FROM android_push_subscriptions s JOIN (SELECT id,creator_id,team_id,project_id,status,created_at FROM team_runs UNION ALL SELECT id,creator_id,team_id,project_id,state AS status,created_at FROM android_scoped_runs) r ON r.creator_id=s.identity_id WHERE s.active=1 AND (s.organization_id IS NULL OR s.organization_id=r.team_id) AND r.status IN ('completed','failed','unknown','cancelled','interrupted','canceled') AND NOT EXISTS(SELECT 1 FROM android_push_outbox q WHERE q.subscription_id=s.id AND q.event_key='run:'||r.id||':'||r.status) ORDER BY r.created_at DESC LIMIT 200").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            let run: String = r.try_get("run_id").map_err(storage_error)?;
            let status: String = r.try_get("status").map_err(storage_error)?;
            let subscription: String = r.try_get("subscription_id").map_err(storage_error)?;
            let event = format!("run:{run}:{status}");
            let payload = json!({"schema_version":"2","kind":"run_event","run_id":run,"event_kind":status,"sequence":"0"});
            sqlx::query("INSERT INTO android_push_outbox(id,identity_id,subscription_id,subscription_revision,team_id,project_id,run_id,event_key,payload,state,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending',$10) ON CONFLICT(subscription_id,event_key) DO NOTHING")
                .bind(Uuid::new_v4().to_string()).bind(r.try_get::<String,_>("identity_id").map_err(storage_error)?).bind(subscription).bind(r.try_get::<i64,_>("revision").map_err(storage_error)?).bind(r.try_get::<Option<String>,_>("team_id").map_err(storage_error)?).bind(r.try_get::<String,_>("project_id").map_err(storage_error)?).bind(&run).bind(event).bind(serde_json::to_string(&payload).map_err(storage_error)?).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_push_claim(&self) -> Result<Option<AndroidPushAttempt>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let rows=sqlx::query("SELECT q.*,s.device_id,s.sealed_token,s.revision AS actual_revision,s.active FROM android_push_outbox q JOIN android_push_subscriptions s ON s.id=q.subscription_id WHERE q.state='pending' ORDER BY q.observed_at,q.id LIMIT 20").fetch_all(&mut *tx).await.map_err(storage_error)?;
        for r in rows {
            let id: String = r.try_get("id").map_err(storage_error)?;
            let team: Option<String> = r.try_get("team_id").map_err(storage_error)?;
            let project: Option<String> = r.try_get("project_id").map_err(storage_error)?;
            let identity: String = r.try_get("identity_id").map_err(storage_error)?;
            let revision: i64 = r.try_get("subscription_revision").map_err(storage_error)?;
            let allowed = if let (Some(team), Some(project)) = (&team, &project) {
                sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM identity_subjects i JOIN identity_memberships m ON m.identity_id=i.id JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_resources p ON p.organization_id=o.id JOIN identity_content_grants g ON g.membership_id=m.id AND g.resource_id=p.id WHERE i.id=$1 AND o.id=$2 AND p.id=$3 AND i.active=1 AND o.state='active' AND m.active=1 AND g.active=1 AND (g.permissions & 1)=1").bind(&identity).bind(team).bind(project).fetch_one(&mut *tx).await.map_err(storage_error)?==1
            } else if let Some(project) = project {
                sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM identity_subjects i JOIN android_personal_projects p ON p.owner_id=i.id WHERE i.id=$1 AND i.active=1 AND p.id=$2 AND p.deleted=0").bind(&identity).bind(project).fetch_one(&mut *tx).await.map_err(storage_error)?==1
            } else {
                false
            };
            if !allowed
                || r.try_get::<i64, _>("active").map_err(storage_error)? != 1
                || r.try_get::<i64, _>("actual_revision")
                    .map_err(storage_error)?
                    != revision
            {
                sqlx::query("UPDATE android_push_outbox SET state='revoked',observed_at=$1 WHERE id=$2 AND state='pending'").bind(current).bind(id).execute(&mut *tx).await.map_err(storage_error)?;
                continue;
            }
            let changed=sqlx::query("UPDATE android_push_outbox SET state='sending',observed_at=$1 WHERE id=$2 AND state='pending'").bind(current).bind(&id).execute(&mut *tx).await.map_err(storage_error)?;
            if changed.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
            let result = AndroidPushAttempt {
                id: Uuid::parse_str(&id).map_err(storage_error)?,
                subscription_id: Uuid::parse_str(
                    &r.try_get::<String, _>("subscription_id")
                        .map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                identity_id: Uuid::parse_str(&identity).map_err(storage_error)?,
                device_id: Uuid::parse_str(
                    &r.try_get::<String, _>("device_id").map_err(storage_error)?,
                )
                .map_err(storage_error)?,
                revision,
                sealed_token: r.try_get("sealed_token").map_err(storage_error)?,
                payload: parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?,
            };
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(result));
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    pub async fn android_push_finish(
        &self,
        attempt: &AndroidPushAttempt,
        accepted: bool,
        permanent: bool,
        code: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let state = if accepted {
            "provider_accepted"
        } else if permanent {
            "failed"
        } else {
            "unknown"
        };
        let n=sqlx::query("UPDATE android_push_outbox SET state=$1,observed_at=$2,error_code=$3 WHERE id=$4 AND subscription_id=$5 AND subscription_revision=$6 AND state='sending'").bind(state).bind(current).bind(code).bind(attempt.id.to_string()).bind(attempt.subscription_id.to_string()).bind(attempt.revision).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        if accepted {
            sqlx::query("UPDATE android_push_subscriptions SET last_success_at=$1 WHERE id=$2 AND revision=$3 AND active=1").bind(current).bind(attempt.subscription_id.to_string()).bind(attempt.revision).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
    /// The durable claim precedes provider I/O. Keep fresh subscription and ACL locks
    /// through the bounded send so a revocation cannot race external publication.
    /// A process loss after the claim remains sending/unknown and is never replayed.
    pub async fn android_push_deliver<F, Fut>(
        &self,
        attempt: &AndroidPushAttempt,
        send: F,
    ) -> Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = (bool, bool, &'static str)>,
    {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_push_outbox q JOIN android_push_subscriptions s ON s.id=q.subscription_id JOIN identity_subjects i ON i.id=q.identity_id JOIN identity_memberships m ON m.identity_id=i.id AND m.organization_id=q.team_id JOIN identity_organizations o ON o.id=m.organization_id JOIN identity_resources p ON p.id=q.project_id AND p.organization_id=o.id JOIN identity_content_grants g ON g.resource_id=p.id AND g.membership_id=m.id WHERE q.id=$1 AND q.subscription_id=$2 AND q.subscription_revision=$3 AND q.state='sending' AND s.revision=q.subscription_revision AND s.active=1 AND i.active=1 AND m.active=1 AND o.state='active' AND g.active=1 AND (g.permissions & 1)=1").bind(attempt.id.to_string()).bind(attempt.subscription_id.to_string()).bind(attempt.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let (accepted, permanent, code) = if count == 1 {
            send().await
        } else {
            (false, true, "subscription_authority_changed")
        };
        let state = if accepted {
            "provider_accepted"
        } else if permanent {
            "failed"
        } else {
            "unknown"
        };
        let n=sqlx::query("UPDATE android_push_outbox SET state=$1,observed_at=$2,error_code=$3 WHERE id=$4 AND state='sending'").bind(state).bind(current).bind(code).bind(attempt.id.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        if n.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        if accepted {
            sqlx::query("UPDATE android_push_subscriptions SET last_success_at=$1 WHERE id=$2 AND revision=$3 AND active=1").bind(current).bind(attempt.subscription_id.to_string()).bind(attempt.revision).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn android_push_status(&self, actor: &Principal) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let scope = self
            .identity()
            .session_organization_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?
            .map(|s| s.to_string());
        let rows=sqlx::query("SELECT id,state,observed_at,error_code FROM android_push_outbox WHERE identity_id=$1 AND ($2 IS NULL OR team_id=$2) ORDER BY observed_at DESC LIMIT 100").bind(actor.identity_id().to_string()).bind(scope).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let items=rows.iter().map(|r|Ok(json!({"id":r.try_get::<String,_>("id").map_err(storage_error)?,"state":r.try_get::<String,_>("state").map_err(storage_error)?,"observed_at":device_iso(r.try_get("observed_at").map_err(storage_error)?)?,"error_code":r.try_get::<Option<String>,_>("error_code").map_err(storage_error)?}))).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"items":items}))
    }
}
