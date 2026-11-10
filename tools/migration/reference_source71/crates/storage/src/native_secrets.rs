use super::*;
use cowork_secrets::{KeyId, KeyProof};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretState {
    pub instance: Uuid,
    pub revision: i64,
    pub active: Option<KeyProof>,
    pub pending_operation: Option<String>,
}
impl NativeSecretState {
    fn validate(&self) -> Result<()> {
        if self.instance.is_nil()
            || self.revision < 0
            || self
                .active
                .as_ref()
                .is_some_and(|p| p.validate().is_err() || p.key.instance != self.instance)
            || self
                .pending_operation
                .as_ref()
                .is_some_and(|s| Uuid::parse_str(s).is_err())
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeKeyOperation {
    pub id: String,
    pub request_key: String,
    pub base_revision: i64,
    pub key: KeyId,
    pub state: NativeSecretsSetupState,
    pub proof: Option<KeyProof>,
    pub created_at: i64,
    pub updated_at: i64,
}
impl NativeKeyOperation {
    pub(super) fn validate(&self) -> Result<()> {
        valid_text(&self.request_key, 128)?;
        if Uuid::parse_str(&self.id).is_err()
            || self.key.validate().is_err()
            || !(0..=i64::MAX - 2).contains(&self.base_revision)
            || self.created_at <= 0
            || self.updated_at < self.created_at
        {
            return Err(Error::Forbidden);
        }
        match self.state {
            NativeSecretsSetupState::Activated => {
                if self
                    .proof
                    .as_ref()
                    .is_none_or(|p| p.validate().is_err() || p.key != self.key)
                {
                    return Err(Error::Forbidden);
                }
            }
            _ => {
                if self.proof.is_some() {
                    return Err(Error::Forbidden);
                }
            }
        }
        Ok(())
    }
    fn request_hash(&self) -> String {
        canonical_hash(&json!([
            "native_key_setup",
            self.request_key,
            self.base_revision
        ]))
    }
    pub fn reserved_revision(&self) -> i64 {
        self.base_revision + 1
    }
}
/// Native creation keeps this writer fence until the actual OS call returns,
/// including when the original HTTP caller times out or is canceled.
pub struct NativeKeyCreationGuard {
    tx: sqlx::Transaction<'static, sqlx::Any>,
}
impl NativeKeyCreationGuard {
    pub async fn finish(self) -> Result<()> {
        self.tx.commit().await.map_err(storage_error)
    }
}
/// A creation opportunity is returned once, only after its reservation commits.
/// It cannot be cloned or deserialized. Replay and reconcile never return one.
pub struct NativeKeyCreationPermit {
    operation: NativeKeyOperation,
}
impl NativeKeyCreationPermit {
    pub fn operation(&self) -> &NativeKeyOperation {
        &self.operation
    }
    pub fn into_operation(self) -> NativeKeyOperation {
        self.operation
    }
}
pub enum NativeKeyAdmission {
    Create(NativeKeyCreationPermit),
    Existing(NativeKeyOperation),
}
fn checked_state(row: &sqlx::any::AnyRow) -> Result<NativeSecretState> {
    let state: NativeSecretState =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    state.validate()?;
    if row.try_get::<String, _>("id").map_err(storage_error)? != "local" {
        return Err(Error::Forbidden);
    }
    if state.revision != row.try_get::<i64, _>("revision").map_err(storage_error)? {
        return Err(Error::Forbidden);
    }
    Ok(state)
}
pub(super) fn checked_operation(row: &sqlx::any::AnyRow) -> Result<NativeKeyOperation> {
    let operation: NativeKeyOperation =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    operation.validate()?;
    if operation.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || operation.request_key
            != row
                .try_get::<String, _>("request_key")
                .map_err(storage_error)?
        || operation.request_hash()
            != row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(operation)
}
impl Store {
    pub async fn begin_native_key_creation(
        &self,
        operation: &NativeKeyOperation,
    ) -> Result<NativeKeyCreationGuard> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = lock_state(&mut tx).await?;
        let row = sqlx::query(
            "SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE id=$1",
        )
        .bind(&operation.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let saved = checked_operation(&row)?;
        if saved != *operation
            || saved.state != NativeSecretsSetupState::Pending
            || state.instance != saved.key.instance
            || state.pending_operation.as_deref() != Some(saved.id.as_str())
            || state.revision != saved.reserved_revision()
        {
            return Err(Error::Conflict);
        }
        crate::native_key_retirement::allow_cipher(
            &mut tx,
            &saved.key.cipher_id().map_err(|_| Error::Forbidden)?,
        )
        .await?;
        Ok(NativeKeyCreationGuard { tx })
    }
    /// Trusted native-only recovery inventory; never a public run/export projection.
    pub async fn native_recovery_inventory(&self) -> Result<(NativeSecretState, Vec<KeyProof>)> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = lock_state(&mut tx).await?;
        if state.pending_operation.is_some() || state.active.is_none() {
            return Err(Error::Conflict);
        }
        let required = crate::native_key_retirement::required_keys(&mut tx, &state).await?;
        let mut proofs =
            crate::native_key_retirement::required_proofs(&mut tx, &state, &required).await?;
        let active = state.active.clone().ok_or(Error::Conflict)?;
        if proofs
            .insert(active.key.key, active.clone())
            .is_some_and(|old| old != active)
        {
            return Err(Error::Forbidden);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok((state, proofs.into_values().collect()))
    }
    /// Rewrap private ciphertext atomically. All runtime records and authorities
    /// retain their identities/revisions; no remote session action is sent.
    pub async fn reencrypt_native_secrets(
        &self,
        expected: i64,
        keys: &dyn cowork_secrets::KeyStore,
    ) -> Result<Value> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = lock_state(&mut tx).await?;
        if state.revision != expected || state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let active = state.active.as_ref().ok_or(Error::NativeKeyUnavailable)?;
        crate::historical_imports::product::native_quiescence(&mut tx, state.instance).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE state NOT IN ('completed','failed','canceled','expired','interrupted')")
            .fetch_one(&mut *tx).await.map_err(storage_error)?;
        if count != 0 {
            return Err(Error::Validation(
                "Finish or cancel active runs before re-encryption".into(),
            ));
        }
        let material = cowork_secrets::load_verified_key(keys, active)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let target = cowork_mcp::SessionCipher::new(
            &active.key.cipher_id().map_err(|_| Error::Forbidden)?,
            material,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let rows = sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials")
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut credentials = 0;
        for row in &rows {
            let mut record = crate::mcp_credentials::checked(row)?;
            if record.revoked {
                continue;
            }
            let old = record.sealed_for(&record.scope)?;
            if old.key_id() == active.key {
                continue;
            }
            let bearer = cowork_secrets::open_bearer(keys, &record.scope, old)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            record.sealed = Some(
                cowork_secrets::seal_bearer(keys, active.key, &record.scope, &bearer)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            );
            if sqlx::query("UPDATE mcp_credentials SET payload=$1 WHERE project_id=$2 AND reference=$3 AND revision=$4 AND payload=$5")
                .bind(json_string(&record)?).bind(&record.scope.project_id).bind(&record.scope.reference).bind(record.revision)
                .bind(row.try_get::<String,_>("payload").map_err(storage_error)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
            credentials += 1;
        }
        let rows = sqlx::query("SELECT id,run_id,server_id,revision,payload FROM mcp_connections")
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let mut sessions = 0;
        for row in &rows {
            let original: String = row.try_get("payload").map_err(storage_error)?;
            let run_id: String = row.try_get("run_id").map_err(storage_error)?;
            let stored: StoredRun = parse(
                &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                    .bind(&run_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?,
            )?;
            let mut connection = crate::mcp_connections::checked(row, &stored.record)?;
            let Some(sealed) = connection.sealed.as_ref() else {
                continue;
            };
            let key = cowork_secrets::KeyId::from_cipher_id(sealed.key_id())
                .map_err(|_| Error::Forbidden)?;
            if key == active.key {
                continue;
            }
            if matches!(
                connection.state,
                crate::McpConnectionState::Busy { .. } | crate::McpConnectionState::Connecting
            ) {
                return Err(Error::Conflict);
            }
            let scope = connection.scope(
                &stored.record,
                connection.checkpoint_revision.ok_or(Error::Forbidden)?,
            )?;
            let source = cowork_mcp::SessionCipher::new(
                sealed.key_id(),
                keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?,
            )
            .map_err(|_| Error::NativeKeyUnavailable)?;
            connection.sealed = Some(
                source
                    .rewrap(sealed, &scope, &target)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            );
            if sqlx::query(
                "UPDATE mcp_connections SET payload=$1 WHERE id=$2 AND revision=$3 AND payload=$4",
            )
            .bind(json_string(&connection)?)
            .bind(&connection.id)
            .bind(connection.revision)
            .bind(original)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
                != 1
            {
                return Err(Error::Conflict);
            }
            sessions += 1;
        }
        let auth_transports = crate::native_auth_transport::rewrap_helper(&mut tx, keys, active.key).await?;
        let historical_imports = crate::historical_imports::product::native_rewrap_helper(&mut tx, keys, active.key).await?;
        let oauth_tasks=crate::mcp_oauth::rewrap(&mut tx,keys,active.key,"native").await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"state":"rewrapped","credential_count":credentials,"session_count":sessions,"auth_transport_count":auth_transports,"historical_import_count":historical_imports,"mcp_oauth_task_count":oauth_tasks,"revision":expected,"remote_requests":0}),
        )
    }
    pub fn is_local_sqlite(&self) -> bool {
        self.sqlite
    }
    pub async fn native_secret_state(&self) -> Result<Option<NativeSecretState>> {
        let row =
            sqlx::query("SELECT id,revision,payload FROM native_secret_state WHERE id='local'")
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        row.as_ref().map(checked_state).transpose()
    }
    pub async fn native_key_request(&self, key: &str) -> Result<Option<NativeKeyOperation>> {
        valid_text(key, 128)?;
        let row=sqlx::query("SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE request_key=$1")
            .bind(key).fetch_optional(&self.pool).await.map_err(storage_error)?;
        row.as_ref().map(checked_operation).transpose()
    }
    pub async fn native_key_operation(&self, operation: &str) -> Result<NativeKeyOperation> {
        let row = sqlx::query(
            "SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE id=$1",
        )
        .bind(operation)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        checked_operation(&row)
    }
    pub async fn begin_native_key_setup(
        &self,
        input: &NativeSecretsSetup,
    ) -> Result<NativeKeyAdmission> {
        valid_text(&input.idempotency_key, 128)?;
        if !(0..=i64::MAX - 2).contains(&input.base_revision) {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let empty = NativeSecretState {
            instance: Uuid::new_v4(),
            revision: 0,
            active: None,
            pending_operation: None,
        };
        sqlx::query("INSERT INTO native_secret_state(id,revision,payload) VALUES('local',0,$1) ON CONFLICT(id) DO NOTHING")
            .bind(json_string(&empty)?).execute(&mut *tx).await.map_err(storage_error)?;
        let mut state = lock_state(&mut tx).await?;
        let previous=sqlx::query("SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE request_key=$1")
            .bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = previous {
            let operation = checked_operation(&row)?;
            if operation.base_revision != input.base_revision
                || operation.key.instance != state.instance
            {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(NativeKeyAdmission::Existing(operation));
        }
        if state.revision != input.base_revision || state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let timestamp = now();
        let operation = NativeKeyOperation {
            id: id(),
            request_key: input.idempotency_key.clone(),
            base_revision: state.revision,
            key: KeyId {
                instance: state.instance,
                key: Uuid::new_v4(),
            },
            state: NativeSecretsSetupState::Pending,
            proof: None,
            created_at: timestamp,
            updated_at: timestamp,
        };
        operation.validate()?;
        state.pending_operation = Some(operation.id.clone());
        state.revision += 1;
        write_state(&mut tx, &state).await?;
        sqlx::query("INSERT INTO native_key_operations(id,request_key,request_sha256,payload) VALUES($1,$2,$3,$4)")
            .bind(&operation.id).bind(&operation.request_key).bind(operation.request_hash()).bind(json_string(&operation)?)
            .execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(NativeKeyAdmission::Create(NativeKeyCreationPermit {
            operation,
        }))
    }
    /// Caller has independently verified this key proof in the native store.
    /// Replays return the operation receipt without reselecting an older active key.
    pub async fn finish_native_key_setup(
        &self,
        input: &NativeSecretsResolution,
        proof: Option<&KeyProof>,
    ) -> Result<NativeKeyOperation> {
        if input.base_revision <= 0 || input.base_revision == i64::MAX {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut state = lock_state(&mut tx).await?;
        let row = sqlx::query(
            "SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE id=$1",
        )
        .bind(&input.operation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let mut operation = checked_operation(&row)?;
        if operation.reserved_revision() != input.base_revision
            || operation.key.instance != state.instance
        {
            return Err(Error::Conflict);
        }
        if proof.is_some_and(|p| p.validate().is_err() || p.key != operation.key) {
            return Err(Error::Forbidden);
        }
        let outcome = if proof.is_some() {
            NativeSecretsSetupState::Activated
        } else {
            NativeSecretsSetupState::Abandoned
        };
        if operation.state != NativeSecretsSetupState::Pending {
            if operation.state != outcome || operation.proof.as_ref() != proof {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(operation);
        }
        if state.revision != input.base_revision
            || state.pending_operation.as_deref() != Some(operation.id.as_str())
        {
            return Err(Error::Conflict);
        }
        if proof.is_some() {
            crate::native_key_retirement::allow_cipher(
                &mut tx,
                &operation.key.cipher_id().map_err(|_| Error::Forbidden)?,
            )
            .await?;
        }
        operation.state = outcome;
        operation.proof = proof.cloned();
        operation.updated_at = now();
        operation.validate()?;
        if let Some(proof) = proof {
            state.active = Some(proof.clone());
        }
        state.pending_operation = None;
        state.revision += 1;
        write_state(&mut tx, &state).await?;
        sqlx::query("UPDATE native_key_operations SET payload=$1 WHERE id=$2")
            .bind(json_string(&operation)?)
            .bind(&operation.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(operation)
    }
}
pub(super) async fn lock_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
) -> Result<NativeSecretState> {
    if sqlx::query("UPDATE native_secret_state SET revision=revision WHERE id='local'")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let row = sqlx::query("SELECT id,revision,payload FROM native_secret_state WHERE id='local'")
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    checked_state(&row)
}
async fn write_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    state: &NativeSecretState,
) -> Result<()> {
    state.validate()?;
    sqlx::query("UPDATE native_secret_state SET revision=$1,payload=$2 WHERE id='local'")
        .bind(state.revision)
        .bind(json_string(state)?)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
