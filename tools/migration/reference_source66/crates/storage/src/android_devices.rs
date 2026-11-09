//! Fresh V2 identity-bound Android credentials, challenge transactions and devices.
use super::*;
use cowork_contracts::DevicePasskeyRecord;
use cowork_identity::{Principal, SessionIssued};
use team_service::identity_error;

pub struct AndroidPasskeyState {
    pub id: Uuid,
    pub identity: Uuid,
    pub identity_revision: i64,
    pub kind: String,
    pub device_id: Option<Uuid>,
    pub state: Value,
    pub binding: Value,
    pub expires_at: i64,
}
pub struct AndroidStoredPasskey {
    pub id: Uuid,
    pub credential_id: String,
    pub credential: Value,
    pub revision: i64,
}
/// Internal cryptographic verifier result. No Deserialize or HTTP representation.
pub enum AndroidVerifiedPasskey {
    Registration {
        credential_id: String,
        credential: Value,
        label: String,
    },
    Authentication {
        id: Uuid,
        credential: Value,
    },
}
pub enum AndroidPasskeyFinished {
    Registered(DevicePasskeyRecord),
    Authenticated {
        issued: SessionIssued,
        device_id: Uuid,
        binding: Value,
    },
}
pub(crate) fn device_invalid() -> Error {
    Error::Validation("Invalid device request".into())
}
pub(crate) fn device_iso(value: i64) -> Result<String> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|v| v.to_rfc3339())
        .ok_or(Error::Conflict)
}
pub(crate) async fn device_writer(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<i64> {
    // All identity mutators share this lock. Sample after an actual lock wait.
    sqlx::query("UPDATE identity_installation SET revision=revision WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    sqlx::query("UPDATE android_device_guard SET observed_at=observed_at WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    // All replicas share the database clock. Host clock skew must neither
    // extend another node's ownership nor spuriously trip its high-water fence.
    let query = match tx.backend_name() {
        "SQLite" => "SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)",
        "PostgreSQL" => {
            "SELECT CAST(FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000) AS BIGINT)"
        }
        _ => {
            return Err(Error::Validation(
                "Unsupported device authority clock".into(),
            ));
        }
    };
    let current: i64 = sqlx::query_scalar(query)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let old: i64 = sqlx::query_scalar("SELECT observed_at FROM android_device_guard WHERE id=1")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    if current <= 0 || current < old {
        return Err(Error::Conflict);
    }
    sqlx::query("UPDATE android_device_guard SET observed_at=$1 WHERE id=1")
        .bind(current)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(current)
}
async fn stored_passkeys(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    identity: Uuid,
) -> Result<Vec<AndroidStoredPasskey>> {
    let rows=sqlx::query("SELECT id,credential_id,credential,revision FROM android_passkeys WHERE identity_id=$1 ORDER BY id LIMIT 33").bind(identity.to_string()).fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 32 {
        return Err(Error::Conflict);
    }
    rows.iter()
        .map(|r| {
            Ok(AndroidStoredPasskey {
                id: Uuid::parse_str(&r.try_get::<String, _>("id").map_err(storage_error)?)
                    .map_err(storage_error)?,
                credential_id: r.try_get("credential_id").map_err(storage_error)?,
                credential: parse(
                    &r.try_get::<String, _>("credential")
                        .map_err(storage_error)?,
                )?,
                revision: r.try_get("revision").map_err(storage_error)?,
            })
        })
        .collect()
}
impl Store {
    pub async fn android_passkey_start_identity(
        &self,
        name: &str,
    ) -> Result<(Uuid, i64, String, String, Vec<AndroidStoredPasskey>)> {
        if name.len() > 200 {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let actual = self
            .identity()
            .android_subject_for_login_in_transaction(&mut tx, name.trim())
            .await
            .map_err(identity_error)?;
        let id = actual.id;
        let keys = stored_passkeys(&mut tx, id).await?;
        let value = (
            id,
            actual.revision,
            actual.username,
            actual.display_name,
            keys,
        );
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_passkey_registration_identity(
        &self,
        actor: &Principal,
    ) -> Result<(Uuid, i64, String, String, Vec<AndroidStoredPasskey>)> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let identity = self
            .identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        device_writer(&mut tx).await?;
        let keys = stored_passkeys(&mut tx, identity.id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok((
            identity.id,
            identity.revision,
            identity.username,
            identity.display_name,
            keys,
        ))
    }
    pub async fn android_passkey_create_challenge(
        &self,
        actor: Option<&Principal>,
        value: &AndroidPasskeyState,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.android_passkey_create_challenge_in_transaction(&mut tx, actor, value).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub(crate) async fn android_passkey_create_challenge_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        actor: Option<&Principal>,
        value: &AndroidPasskeyState,
    ) -> Result<()> {
        if !matches!(
            value.kind.as_str(),
            "registration" | "authentication" | "native_authentication"
        ) || serde_json::to_vec(&value.state)
            .map_err(storage_error)?
            .len()
            > 256 * 1024
            || serde_json::to_vec(&value.binding)
                .map_err(storage_error)?
                .len()
                > 4096
        {
            return Err(device_invalid());
        }
        if let Some(a) = actor {
            self.identity()
                .require_global_session_in_transaction(tx, a)
                .await
                .map_err(identity_error)?;
            if self
                .identity()
                .validate_principal_in_transaction(tx, a)
                .await
                .map_err(identity_error)?
                .id
                != value.identity
            {
                return Err(Error::Forbidden);
            }
        }
        let current = device_writer(tx).await?;
        if value.expires_at <= current
            || value.expires_at > current + 300_000
            || (value.kind == "registration") != actor.is_some()
        {
            return Err(device_invalid());
        }
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM identity_subjects WHERE id=$1 AND active=1 AND revision=$2",
        )
        .bind(value.identity.to_string())
        .bind(value.identity_revision)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if active != 1 {
            return Err(Error::Forbidden);
        }
        sqlx::query("DELETE FROM android_passkey_challenges WHERE expires_at<=$1")
            .bind(current)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_passkey_challenges WHERE identity_id=$1 AND consumed=0",
        )
        .bind(value.identity.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if count >= 8 {
            return Err(Error::BudgetExceeded);
        }
        let session_revision: Option<i64> = if let Some(a) = actor {
            Some(
                sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1")
                    .bind(a.session_id().to_string())
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?,
            )
        } else {
            None
        };
        sqlx::query("INSERT INTO android_passkey_challenges(id,identity_id,identity_revision,kind,actor_session_id,actor_session_revision,device_id,state,binding,expires_at,consumed) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,0)")
            .bind(value.id.to_string()).bind(value.identity.to_string()).bind(value.identity_revision).bind(&value.kind).bind(actor.map(|a|a.session_id().to_string())).bind(session_revision).bind(value.device_id.map(|id|id.to_string())).bind(serde_json::to_string(&value.state).map_err(storage_error)?).bind(serde_json::to_string(&value.binding).map_err(storage_error)?).bind(value.expires_at).execute(&mut **tx).await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_passkey_finish<F>(
        &self,
        actor: Option<&Principal>,
        id: Uuid,
        kind: &str,
        verify: F,
    ) -> Result<AndroidPasskeyFinished>
    where
        F: FnOnce(&AndroidPasskeyState, &[AndroidStoredPasskey]) -> Result<AndroidVerifiedPasskey>,
    {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let result = self
            .android_passkey_finish_in_transaction(&mut tx, actor, id, kind, verify)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub(crate) async fn android_passkey_finish_in_transaction<F>(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        actor: Option<&Principal>,
        id: Uuid,
        kind: &str,
        verify: F,
    ) -> Result<AndroidPasskeyFinished>
    where
        F: FnOnce(&AndroidPasskeyState, &[AndroidStoredPasskey]) -> Result<AndroidVerifiedPasskey>,
    {
        if let Some(a) = actor {
            self.identity()
                .require_global_session_in_transaction(tx, a)
                .await
                .map_err(identity_error)?;
            self.identity()
                .validate_principal_in_transaction(tx, a)
                .await
                .map_err(identity_error)?;
        }
        let current = device_writer(tx).await?;
        let r=sqlx::query("SELECT c.*,i.revision AS current_identity_revision FROM android_passkey_challenges c JOIN identity_subjects i ON i.id=c.identity_id WHERE c.id=$1 AND c.kind=$2 AND c.consumed=0 AND c.expires_at>$3 AND i.active=1").bind(id.to_string()).bind(kind).bind(current).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let value = AndroidPasskeyState {
            id,
            identity: Uuid::parse_str(
                &r.try_get::<String, _>("identity_id")
                    .map_err(storage_error)?,
            )
            .map_err(storage_error)?,
            identity_revision: r.try_get("identity_revision").map_err(storage_error)?,
            kind: kind.into(),
            device_id: r
                .try_get::<Option<String>, _>("device_id")
                .map_err(storage_error)?
                .map(|s| Uuid::parse_str(&s).map_err(storage_error))
                .transpose()?,
            state: parse(&r.try_get::<String, _>("state").map_err(storage_error)?)?,
            binding: parse(&r.try_get::<String, _>("binding").map_err(storage_error)?)?,
            expires_at: r.try_get("expires_at").map_err(storage_error)?,
        };
        if value.identity_revision
            != r.try_get::<i64, _>("current_identity_revision")
                .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        if let Some(a) = actor {
            let actual: i64 =
                sqlx::query_scalar("SELECT revision FROM identity_sessions WHERE id=$1")
                    .bind(a.session_id().to_string())
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?;
            if value.identity != a.identity_id()
                || r.try_get::<Option<String>, _>("actor_session_id")
                    .map_err(storage_error)?
                    .as_deref()
                    != Some(a.session_id().to_string().as_str())
                || r.try_get::<Option<i64>, _>("actor_session_revision")
                    .map_err(storage_error)?
                    != Some(actual)
            {
                return Err(Error::Forbidden);
            }
        } else if kind == "registration" {
            return Err(Error::Forbidden);
        }
        let keys = stored_passkeys(tx, value.identity).await?;
        let verified = verify(&value, &keys)?;
        let changed=sqlx::query("UPDATE android_passkey_challenges SET consumed=1 WHERE id=$1 AND consumed=0 AND expires_at>$2").bind(id.to_string()).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Forbidden);
        }
        let result = match verified {
            AndroidVerifiedPasskey::Registration {
                credential_id,
                credential,
                label,
            } => {
                if kind != "registration"
                    || keys.len() >= 32
                    || credential_id.is_empty()
                    || credential_id.len() > 2048
                    || label.trim().is_empty()
                    || label.len() > 200
                {
                    return Err(device_invalid());
                }
                let key = Uuid::new_v4();
                let text = serde_json::to_string(&credential).map_err(storage_error)?;
                if text.len() > 128 * 1024 {
                    return Err(device_invalid());
                }
                sqlx::query("INSERT INTO android_passkeys(id,identity_id,credential_id,label,credential,revision,created_at) VALUES($1,$2,$3,$4,$5,1,$6)").bind(key.to_string()).bind(value.identity.to_string()).bind(credential_id).bind(label.trim()).bind(text).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
                AndroidPasskeyFinished::Registered(DevicePasskeyRecord {
                    schema_version: 2,
                    id: key,
                    label: label.trim().into(),
                    created_at: device_iso(current)?,
                    last_used_at: None,
                })
            }
            AndroidVerifiedPasskey::Authentication {
                id: key,
                credential,
            } => {
                if !matches!(kind, "authentication" | "native_authentication") {
                    return Err(Error::Forbidden);
                }
                let original = keys.iter().find(|k| k.id == key).ok_or(Error::Forbidden)?;
                let text = serde_json::to_string(&credential).map_err(storage_error)?;
                if text.len() > 128 * 1024 {
                    return Err(device_invalid());
                }
                let changed=sqlx::query("UPDATE android_passkeys SET credential=$1,revision=revision+1,last_used_at=$2 WHERE id=$3 AND identity_id=$4 AND revision=$5").bind(text).bind(current).bind(key.to_string()).bind(value.identity.to_string()).bind(original.revision).execute(&mut **tx).await.map_err(storage_error)?;
                if changed.rows_affected() != 1 {
                    return Err(Error::Conflict);
                }
                let issued = self
                    .identity()
                    .issue_verified_session_in_transaction(tx, value.identity, 28_800_000)
                    .await
                    .map_err(identity_error)?;
                // This arm is reached only after the actual cryptographic
                // verifier and original challenge/credential CAS above.
                let session = self
                    .identity()
                    .record_android_session_in_transaction(
                        tx,
                        &issued,
                        &value.device_id.ok_or(Error::Forbidden)?.to_string(),
                        Some((
                            original.revision.checked_add(1).ok_or(Error::Conflict)?,
                            "passkey",
                        )),
                    )
                    .await
                    .map_err(identity_error)?;
                sqlx::query("INSERT INTO android_auth_passkey_proofs(session_id,owner_id,passkey_id,credential_revision,verified_at) VALUES($1,$2,$3,$4,$5)")
                    .bind(session.to_string()).bind(value.identity.to_string()).bind(key.to_string()).bind(original.revision.checked_add(1).ok_or(Error::Conflict)?).bind(current)
                    .execute(&mut **tx).await.map_err(storage_error)?;
                AndroidPasskeyFinished::Authenticated {
                    issued,
                    device_id: value.device_id.ok_or(Error::Forbidden)?,
                    binding: value.binding,
                }
            }
        };
        Ok(result)
    }
    pub async fn android_passkeys(&self, actor: &Principal) -> Result<Vec<DevicePasskeyRecord>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let rows=sqlx::query("SELECT id,label,created_at,last_used_at FROM android_passkeys WHERE identity_id=$1 ORDER BY created_at,id").bind(actor.identity_id().to_string()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let result = rows
            .iter()
            .map(|r| {
                Ok(DevicePasskeyRecord {
                    schema_version: 2,
                    id: Uuid::parse_str(&r.try_get::<String, _>("id").map_err(storage_error)?)
                        .map_err(storage_error)?,
                    label: r.try_get("label").map_err(storage_error)?,
                    created_at: device_iso(r.try_get("created_at").map_err(storage_error)?)?,
                    last_used_at: r
                        .try_get::<Option<i64>, _>("last_used_at")
                        .map_err(storage_error)?
                        .map(device_iso)
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_remove_passkey(&self, actor: &Principal, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .require_global_session_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        device_writer(&mut tx).await?;
        let changed = sqlx::query("DELETE FROM android_passkeys WHERE id=$1 AND identity_id=$2")
            .bind(id.to_string())
            .bind(actor.identity_id().to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage_error)
    }
}
