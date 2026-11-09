//! Durable tombstones precede native deletion and reject stale ciphertext writes.
use super::*;
use cowork_secrets::{KeyBackup, KeyId, KeyProof};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeKeyRetirement {
    pub key: KeyId,
    pub proof: Option<KeyProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphan_operation: Option<NativeKeyOperation>,
    pub request_key: String,
    pub expected_revision: i64,
    pub state: String,
}
/// Keep the native write gate owned by the OS operation, even if its caller
/// times out or drops the request. Retirement cannot pass a still-running restore.
pub struct NativeKeyRestoreGuard {
    tx: sqlx::Transaction<'static, sqlx::Any>,
    instance: Uuid,
    proofs: Vec<KeyProof>,
}
impl NativeKeyRestoreGuard {
    pub fn instance(&self) -> Uuid {
        self.instance
    }
    pub fn proofs(&self) -> &[KeyProof] {
        &self.proofs
    }
    pub async fn finish(self) -> Result<()> {
        self.tx.commit().await.map_err(storage_error)
    }
}
impl NativeKeyRetirement {
    fn digest(&self) -> String {
        if let Some(original) = &self.orphan_operation {
            return canonical_hash(&json!([
                "native_orphan_key_retire",
                self.key,
                self.expected_revision,
                self.request_key,
                original
            ]));
        }
        canonical_hash(&json!([
            "native_key_retire",
            self.key,
            self.expected_revision,
            self.request_key
        ]))
    }
    fn checked(row: &sqlx::any::AnyRow) -> Result<Self> {
        let value: Self = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if let Some(proof) = &value.proof {
            proof.validate().map_err(|_| Error::Forbidden)?;
            if value.key != proof.key {
                return Err(Error::Forbidden);
            }
        } else if value.orphan_operation.is_none() {
            return Err(Error::Forbidden);
        }
        if let Some(original) = &value.orphan_operation {
            original.validate()?;
            if original.key != value.key
                || original.proof.is_some()
                || original.state != NativeSecretsSetupState::Abandoned
            {
                return Err(Error::Forbidden);
            }
        }
        if value.expected_revision < 0
            || !matches!(value.state.as_str(), "pending" | "removed")
            || value.key.cipher_id().map_err(|_| Error::Forbidden)?
                != row
                    .try_get::<String, _>("cipher_id")
                    .map_err(storage_error)?
            || value.request_key
                != row
                    .try_get::<String, _>("request_key")
                    .map_err(storage_error)?
            || value.digest()
                != row
                    .try_get::<String, _>("request_sha256")
                    .map_err(storage_error)?
            || value.state != row.try_get::<String, _>("state").map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        Ok(value)
    }
}
pub(super) async fn allow_cipher(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    cipher: &str,
) -> Result<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM native_key_retirements WHERE cipher_id=$1")
            .bind(cipher)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count != 0 {
        return Err(Error::NativeKeyUnavailable);
    }
    Ok(())
}
pub(super) async fn required_keys(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    state: &NativeSecretState,
) -> Result<BTreeSet<Uuid>> {
    let mut keys = BTreeSet::new();
    if let Some(active) = &state.active {
        keys.insert(active.key.key);
    }
    for row in sqlx::query("SELECT project_id,reference,revision,payload FROM mcp_credentials")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let record = crate::mcp_credentials::checked(&row)?;
        if let Some(sealed) = record.sealed {
            let key = sealed.key_id();
            if key.instance != state.instance {
                return Err(Error::Forbidden);
            }
            keys.insert(key.key);
        }
    }
    for row in sqlx::query("SELECT id,run_id,server_id,revision,payload FROM mcp_connections")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let run: String = row.try_get("run_id").map_err(storage_error)?;
        let stored: StoredRun = parse(
            &sqlx::query_scalar::<_, String>("SELECT payload FROM runs WHERE id=$1")
                .bind(run)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?,
        )?;
        let record = crate::mcp_connections::checked(&row, &stored.record)?;
        if let Some(sealed) = record.sealed {
            let key = KeyId::from_cipher_id(sealed.key_id()).map_err(|_| Error::Forbidden)?;
            if key.instance != state.instance {
                return Err(Error::Forbidden);
            }
            keys.insert(key.key);
        }
    }
    for key in crate::native_auth_transport::required_keys(tx, state.instance).await? {
        keys.insert(key.key);
    }
    keys.extend(crate::historical_imports::product::required_native_keys(tx, state.instance).await?);
    for key in crate::mcp_oauth::required_keys(tx,"native").await? {
        if key.instance!=state.instance{return Err(Error::Forbidden);}keys.insert(key.key);
    }
    if keys.len() > 1024 {
        return Err(Error::Validation(
            "More than 1024 keys are currently required by ciphertext; re-encrypt before backup"
                .into(),
        ));
    }
    for key in &keys {
        allow_cipher(
            tx,
            &KeyId {
                instance: state.instance,
                key: *key,
            }
            .cipher_id()
            .map_err(|_| Error::Forbidden)?,
        )
        .await?;
    }
    Ok(keys)
}
pub(super) async fn required_proofs(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    state: &NativeSecretState,
    required: &BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, KeyProof>> {
    let mut proofs = BTreeMap::new();
    let mut cursor = String::new();
    loop {
        let rows=sqlx::query("SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE id>$1 ORDER BY id LIMIT 256")
            .bind(&cursor).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let operation = crate::native_secrets::checked_operation(row)?;
            cursor = operation.id;
            if operation.key.instance != state.instance {
                return Err(Error::Forbidden);
            }
            if let Some(proof) = operation.proof {
                if required.contains(&proof.key.key)
                    && proofs
                        .insert(proof.key.key, proof.clone())
                        .is_some_and(|old| old != proof)
                {
                    return Err(Error::Forbidden);
                }
            }
        }
    }
    let active = state.active.as_ref().ok_or(Error::NativeKeyUnavailable)?;
    if proofs
        .insert(active.key.key, active.clone())
        .is_some_and(|old| old != *active)
    {
        return Err(Error::Forbidden);
    }
    if required.iter().any(|key| !proofs.contains_key(key)) {
        return Err(Error::NativeKeyUnavailable);
    }
    Ok(proofs)
}
impl Store {
    /// A selected older backup may include known historical keys which are no
    /// longer referenced. Validate those exact proofs, rather than requiring it
    /// to equal today's minimal backup inventory. Retired/foreign keys fail first.
    pub async fn begin_native_key_restore(
        &self,
        expected: i64,
        backup: &KeyBackup,
    ) -> Result<NativeKeyRestoreGuard> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        if backup.format != 1
            || backup.proofs.is_empty()
            || backup.proofs.len() > 1024
            || backup.ciphertext.len() != backup.proofs.len() * 32 + 16
        {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        if state.revision != expected
            || state.instance != backup.instance
            || state.pending_operation.is_some()
        {
            return Err(Error::Conflict);
        }
        let mut wanted = required_keys(&mut tx, &state).await?;
        let mut previous = None;
        for proof in &backup.proofs {
            proof.validate().map_err(|_| Error::Forbidden)?;
            if proof.key.instance != state.instance
                || previous.is_some_and(|id| id >= proof.key.key)
            {
                return Err(Error::Forbidden);
            }
            previous = Some(proof.key.key);
            allow_cipher(
                &mut tx,
                &proof.key.cipher_id().map_err(|_| Error::Forbidden)?,
            )
            .await?;
            wanted.insert(proof.key.key);
        }
        let known = required_proofs(&mut tx, &state, &wanted).await?;
        if backup
            .proofs
            .iter()
            .any(|proof| known.get(&proof.key.key) != Some(proof))
        {
            return Err(Error::Forbidden);
        }
        Ok(NativeKeyRestoreGuard {
            tx,
            instance: state.instance,
            proofs: backup.proofs.clone(),
        })
    }
    pub async fn native_key_cleanup_view(&self) -> Result<Value> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        if state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let required = required_keys(&mut tx, &state).await?;
        let mut candidates = Vec::new();
        let mut cursor = String::new();
        let mut remaining = 0u64;
        loop {
            let rows=sqlx::query("SELECT o.id,o.request_key,o.request_sha256,o.payload FROM native_key_operations o WHERE o.id>$1 ORDER BY o.id LIMIT 256")
                .bind(&cursor).fetch_all(&mut *tx).await.map_err(storage_error)?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                let operation = crate::native_secrets::checked_operation(row)?;
                cursor = operation.id.clone();
                if operation.key.instance != state.instance {
                    return Err(Error::Forbidden);
                }
                if operation.proof.is_none()
                    && operation.state != NativeSecretsSetupState::Abandoned
                {
                    continue;
                }
                if required.contains(&operation.key.key) {
                    continue;
                }
                let retired=sqlx::query("SELECT cipher_id,request_key,request_sha256,payload,state FROM native_key_retirements WHERE cipher_id=$1")
                    .bind(operation.key.cipher_id().map_err(|_|Error::Forbidden)?).fetch_optional(&mut *tx).await.map_err(storage_error)?;
                let retirement = retired
                    .as_ref()
                    .map(NativeKeyRetirement::checked)
                    .transpose()?;
                if retirement.as_ref().is_some_and(|r| r.state == "removed") {
                    continue;
                }
                remaining = remaining.checked_add(1).ok_or(Error::Conflict)?;
                if candidates.len() < 100 {
                    candidates.push(json!({"key":operation.key,"orphan_operation_id":if operation.proof.is_none(){Some(&operation.id)}else{None},"retirement":retirement}));
                }
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(
            json!({"revision":state.revision,"required_key_count":required.len(),"candidates":candidates,"remaining":remaining}),
        )
    }
    pub async fn begin_native_key_retirement(
        &self,
        expected: i64,
        key: KeyId,
        request: &str,
    ) -> Result<NativeKeyRetirement> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        valid_text(request, 128)?;
        key.validate().map_err(|_| Error::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        if state.instance != key.instance {
            return Err(Error::Forbidden);
        }
        let required = required_keys(&mut tx, &state).await?;
        if required.contains(&key.key) {
            return Err(Error::Forbidden);
        }
        let cipher = key.cipher_id().map_err(|_| Error::Forbidden)?;
        if let Some(row)=sqlx::query("SELECT cipher_id,request_key,request_sha256,payload,state FROM native_key_retirements WHERE cipher_id=$1 OR request_key=$2")
            .bind(&cipher).bind(request).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            let old=NativeKeyRetirement::checked(&row)?;
            if old.key!=key || old.request_key!=request || old.expected_revision!=expected || old.orphan_operation.is_some(){return Err(Error::Conflict);}
            tx.commit().await.map_err(storage_error)?;return Ok(old);
        }
        if state.revision != expected || state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let mut requested = BTreeSet::new();
        requested.insert(key.key);
        let proof = required_proofs(&mut tx, &state, &requested)
            .await?
            .remove(&key.key)
            .ok_or(Error::NotFound)?;
        let value = NativeKeyRetirement {
            key,
            proof: Some(proof),
            orphan_operation: None,
            request_key: request.into(),
            expected_revision: expected,
            state: "pending".into(),
        };
        sqlx::query("INSERT INTO native_key_retirements(cipher_id,request_key,request_sha256,payload,state,created_at,updated_at) VALUES($1,$2,$3,$4,'pending',$5,$5)")
            .bind(cipher).bind(request).bind(value.digest()).bind(json_string(&value)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// An observed OS proof is used only to remove this abandoned reservation.
    /// It never becomes an activated setup proof or execution authority.
    pub async fn begin_native_orphan_retirement(
        &self,
        expected: i64,
        operation_id: &str,
        request: &str,
        observed: Option<&KeyProof>,
    ) -> Result<NativeKeyRetirement> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        valid_text(request, 128)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let state = crate::native_secrets::lock_state(&mut tx).await?;
        let row = sqlx::query(
            "SELECT id,request_key,request_sha256,payload FROM native_key_operations WHERE id=$1",
        )
        .bind(operation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
        let original = crate::native_secrets::checked_operation(&row)?;
        if original.state != NativeSecretsSetupState::Abandoned
            || original.proof.is_some()
            || original.key.instance != state.instance
        {
            return Err(Error::Forbidden);
        }
        if observed.is_some_and(|p| p.key != original.key || p.validate().is_err()) {
            return Err(Error::Forbidden);
        }
        let required = required_keys(&mut tx, &state).await?;
        if required.contains(&original.key.key) {
            return Err(Error::Forbidden);
        }
        let cipher = original.key.cipher_id().map_err(|_| Error::Forbidden)?;
        if let Some(row) = sqlx::query("SELECT cipher_id,request_key,request_sha256,payload,state FROM native_key_retirements WHERE cipher_id=$1 OR request_key=$2")
            .bind(&cipher).bind(request).fetch_optional(&mut *tx).await.map_err(storage_error)? {
            let old = NativeKeyRetirement::checked(&row)?;
            if old.orphan_operation.as_ref() != Some(&original) || old.expected_revision != expected || old.request_key != request {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        if state.revision != expected || state.pending_operation.is_some() {
            return Err(Error::Conflict);
        }
        let value = NativeKeyRetirement {
            key: original.key,
            proof: observed.cloned(),
            orphan_operation: Some(original),
            request_key: request.into(),
            expected_revision: expected,
            state: "pending".into(),
        };
        sqlx::query("INSERT INTO native_key_retirements(cipher_id,request_key,request_sha256,payload,state,created_at,updated_at) VALUES($1,$2,$3,$4,'pending',$5,$5)")
            .bind(cipher).bind(request).bind(value.digest()).bind(json_string(&value)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn finish_native_key_retirement(
        &self,
        receipt: &NativeKeyRetirement,
    ) -> Result<NativeKeyRetirement> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        crate::native_secrets::lock_state(&mut tx).await?;
        let row=sqlx::query("SELECT cipher_id,request_key,request_sha256,payload,state FROM native_key_retirements WHERE cipher_id=$1")
            .bind(receipt.key.cipher_id().map_err(|_|Error::Forbidden)?).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let mut saved = NativeKeyRetirement::checked(&row)?;
        if saved.digest() != receipt.digest()
            || saved.proof != receipt.proof
            || saved.orphan_operation != receipt.orphan_operation
        {
            return Err(Error::Conflict);
        }
        if saved.state != "removed" {
            saved.state = "removed".into();
            sqlx::query("UPDATE native_key_retirements SET state='removed',payload=$1,updated_at=$2 WHERE cipher_id=$3 AND state='pending'")
            .bind(json_string(&saved)?).bind(now()).bind(saved.key.cipher_id().map_err(|_|Error::Forbidden)?).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(saved)
    }
}
