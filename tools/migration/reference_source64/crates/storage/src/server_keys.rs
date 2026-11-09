//! Installation-only server key custody and original operator journals. No HTTP
//! authority, native-key adoption, implicit signer changes, or automatic recovery.
use super::*;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cowork_identity::audit::AuditAuthority;
use cowork_secrets::kms::{StableKeyPurposes, VaultTransitProfile, WrappedServerKey};
use cowork_secrets::{
    CredentialScope, KeyId, KeyProof, KeyStore, SealedCredential, open_bearer, seal_bearer,
    verify_key,
};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};
use std::{collections::BTreeSet, sync::Arc};
use zeroize::Zeroizing;

const MAX_KEYS: usize = 1024;
const MAX_OPERATIONS: i64 = 512;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyRecord {
    pub proof: KeyProof,
    pub wrapped: Option<WrappedServerKey>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyRegistry {
    pub format: u32,
    pub revision: i64,
    pub instance_id: Uuid,
    pub active: KeyId,
    pub stable: StableKeyPurposes,
    pub profile: Option<VaultTransitProfile>,
    pub keys: Vec<ServerKeyRecord>,
    pub pending: Option<Uuid>,
}
impl ServerKeyRegistry {
    pub fn validate(&self) -> Result<()> {
        if self.format != 1
            || self.revision <= 0
            || self.instance_id.is_nil()
            || self.active.instance != self.instance_id
            || self.keys.is_empty()
            || self.keys.len() > MAX_KEYS
        {
            return Err(Error::Conflict);
        }
        let mut seen = BTreeSet::new();
        for record in &self.keys {
            record
                .proof
                .validate()
                .map_err(|_| Error::NativeKeyUnavailable)?;
            if record.proof.key.instance != self.instance_id
                || !seen.insert(record.proof.key.cipher_id().map_err(|_| Error::Conflict)?)
            {
                return Err(Error::Conflict);
            }
            match (&self.profile, &record.wrapped) {
                (Some(profile), Some(wrapped)) if wrapped.proof == record.proof => wrapped
                    .validate(profile)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
                (None, None) if self.keys.len() == 1 => {}
                _ => return Err(Error::Conflict),
            }
        }
        for key in [
            self.active,
            self.stable.audit,
            self.stable.offline,
            self.stable.request_fingerprint,
            self.stable.operator_audit.unwrap_or(self.stable.audit),
        ] {
            if key.instance != self.instance_id
                || !seen.contains(&key.cipher_id().map_err(|_| Error::Conflict)?)
            {
                return Err(Error::Conflict);
            }
        }
        if self.pending.is_some_and(|id| id.is_nil()) {
            return Err(Error::Conflict);
        }
        if self.stable.audit_epoch.is_some_and(|epoch| epoch <= 0)
            || self.stable.audit_epoch.is_some() != self.stable.operator_audit.is_some()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(json_string(self)?.as_bytes()))
    }
    pub fn envelopes(&self) -> Result<Vec<WrappedServerKey>> {
        self.validate()?;
        self.keys
            .iter()
            .map(|r| r.wrapped.clone().ok_or(Error::NativeKeyUnavailable))
            .collect()
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerKeyOperationKind {
    ImportKms,
    RotateData,
    RotateAuditSigner,
    RewrapKms,
    RetireLegacyFile,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LegacyMasterFileProof {
    pub sha256: String,
    pub file_identity: [u64; 2],
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyRequest {
    pub format: u32,
    pub request_id: Uuid,
    pub expected_revision: i64,
    pub instance_id: Uuid,
    pub kind: ServerKeyOperationKind,
    pub profile: VaultTransitProfile,
    pub wrapping_version: u32,
    #[serde(default)]
    pub legacy_file: Option<LegacyMasterFileProof>,
}
impl ServerKeyRequest {
    fn sha256(&self) -> Result<String> {
        self.profile
            .validate()
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if self.format != 1
            || self.request_id.is_nil()
            || self.instance_id.is_nil()
            || self.expected_revision <= 0
            || self.wrapping_version == 0
        {
            return Err(Error::Conflict);
        }
        match (&self.legacy_file, self.kind) {
            (Some(proof), ServerKeyOperationKind::RetireLegacyFile)
                if proof.sha256.len() == 64
                    && proof
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) => {}
            (
                None,
                ServerKeyOperationKind::ImportKms
                | ServerKeyOperationKind::RotateData
                | ServerKeyOperationKind::RotateAuditSigner
                | ServerKeyOperationKind::RewrapKms,
            ) => {}
            _ => return Err(Error::Conflict),
        }
        Ok(hash(json_string(self)?.as_bytes()))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyOperation {
    pub format: u32,
    pub request: ServerKeyRequest,
    pub source: ServerKeyRegistry,
    pub source_sha256: String,
    pub prepared_at: i64,
    pub candidate_proof: Option<KeyProof>,
    pub envelope_id: Option<Uuid>,
    pub prepared_material: Option<SealedCredential>,
    pub prepared_material_sha256: Option<String>,
    pub original_intent_mac: String,
    pub confirmed_envelopes_mac: Option<String>,
    pub confirmed_envelopes: Vec<WrappedServerKey>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyReceiptPayload {
    pub format: u32,
    pub request: ServerKeyRequest,
    pub original_registry_sha256: String,
    pub result_registry_sha256: String,
    pub original_revision: i64,
    pub result_revision: i64,
    pub original_active: KeyId,
    pub result_active: KeyId,
    pub stable: StableKeyPurposes,
    pub encrypted_rows_rewrapped: u64,
    pub result: String,
    pub completed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_origin_plan_sha256: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedServerKeyReceipt {
    pub payload: ServerKeyReceiptPayload,
    pub public_key: String,
    pub public_key_sha256: String,
    pub signature: String,
}
impl SignedServerKeyReceipt {
    /// Verification requires the independently retained original operator signing pin.
    pub fn verify(&self, original_origin_pin: &str) -> Result<()> {
        use ring::signature::{ED25519, UnparsedPublicKey};
        if original_origin_pin.len() != 64
            || self.public_key_sha256 != original_origin_pin
            || self.payload.format != 1
            || self.payload.completed_at <= 0
        {
            return Err(Error::NativeKeyUnavailable);
        }
        let public = URL_SAFE_NO_PAD
            .decode(&self.public_key)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let signature = URL_SAFE_NO_PAD
            .decode(&self.signature)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        if public.len() != 32 || signature.len() != 64 || hash(&public) != original_origin_pin {
            return Err(Error::NativeKeyUnavailable);
        }
        let bytes = json_string(&(
            "cowork:server:key-operation:v1",
            &self.public_key,
            &self.public_key_sha256,
            &self.payload,
        ))?;
        UnparsedPublicKey::new(&ED25519, &public)
            .verify(bytes.as_bytes(), &signature)
            .map_err(|_| Error::NativeKeyUnavailable)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerKeyOperationStatus {
    pub operation: ServerKeyOperation,
    pub state: String,
    pub receipt: Option<SignedServerKeyReceipt>,
}

async fn guard(tx: &mut Transaction<'_, Any>) -> Result<()> {
    if sqlx::query("UPDATE server_key_guard SET id=id WHERE id=1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn current_in(tx: &mut Transaction<'_, Any>) -> Result<Option<ServerKeyRegistry>> {
    guard(tx).await?;
    let row = sqlx::query("SELECT revision,payload FROM server_key_registry WHERE id=1")
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    row.map(|r| {
        let registry: ServerKeyRegistry =
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        registry.validate()?;
        if registry.revision != r.try_get::<i64, _>("revision").map_err(storage_error)? {
            return Err(Error::Conflict);
        }
        Ok(registry)
    })
    .transpose()
}
/// Held in the SAME transaction as ciphertext publication, after current actor
/// authorization. A stale runtime can never publish under a retired data epoch.
/// Distinct native instances keep their existing native lifecycle fences.
pub async fn cipher_writer(tx: &mut Transaction<'_, Any>, key_id: KeyId) -> Result<()> {
    key_id.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    if let Some(registry) = current_in(tx).await? {
        if key_id.instance == registry.instance_id
            && (registry.pending.is_some() || key_id != registry.active)
        {
            return Err(Error::NativeKeyUnavailable);
        }
    }
    Ok(())
}
/// A server-only admission fence. This never accepts an unrelated native key.
pub async fn server_admission(tx: &mut Transaction<'_, Any>, key_id: KeyId) -> Result<()> {
    if let Some(registry) = current_in(tx).await? {
        if registry.pending.is_some() || registry.active != key_id {
            return Err(Error::NativeKeyUnavailable);
        }
    }
    Ok(())
}
pub async fn maintenance_fence(tx: &mut Transaction<'_, Any>) -> Result<()> {
    if current_in(tx)
        .await?
        .is_some_and(|registry| registry.pending.is_some())
    {
        return Err(Error::NativeKeyUnavailable);
    }
    Ok(())
}
async fn sample(tx: &mut Transaction<'_, Any>) -> Result<i64> {
    let query = match tx.backend_name() {
        "SQLite" => "SELECT CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)",
        "PostgreSQL" => {
            "SELECT CAST(FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000) AS BIGINT)"
        }
        _ => return Err(Error::Conflict),
    };
    let time: i64 = sqlx::query_scalar(query)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let latest: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(completed_at),0) FROM server_key_operations")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if time <= 0 || time < latest {
        return Err(Error::Conflict);
    }
    Ok(time)
}
fn seed_scope(op: &ServerKeyOperation) -> CredentialScope {
    CredentialScope {
        project_id: op.source.instance_id.to_string(),
        server_id: "server-key-operator/v1".into(),
        reference: op.request.request_id.to_string(),
        revision: op.request.expected_revision,
        endpoint: format!(
            "installation://server-key-candidate/{}",
            hash(json_string(&op.request).unwrap_or_default().as_bytes())
        ),
    }
}
async fn completion_sample(tx: &mut Transaction<'_, Any>, prepared_at: i64) -> Result<i64> {
    let current = sample(tx).await?;
    if current < prepared_at {
        return Err(Error::Conflict);
    }
    Ok(current)
}
impl ServerKeyOperation {
    fn original_intent_bytes(&self) -> Result<Vec<u8>> {
        Ok(json_string(&(
            "cowork:server:key-operator-intent:v1",
            self.format,
            &self.request,
            &self.source_sha256,
            self.prepared_at,
            &self.candidate_proof,
            self.envelope_id,
            &self.prepared_material_sha256,
        ))?
        .into_bytes())
    }
    fn sign_original_intent(&self, keys: &dyn KeyStore) -> Result<String> {
        let key = keys
            .load(&self.source.stable.request_fingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        Ok(URL_SAFE_NO_PAD.encode(
            ring::hmac::sign(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()),
                &self.original_intent_bytes()?,
            )
            .as_ref(),
        ))
    }
    fn confirmed_envelope_bytes(&self) -> Result<Vec<u8>> {
        Ok(json_string(&(
            "cowork:server:kms-original-confirmation:v1",
            &self.original_intent_mac,
            &self.confirmed_envelopes,
        ))?
        .into_bytes())
    }
    fn sign_confirmed_envelopes(&self, keys: &dyn KeyStore) -> Result<String> {
        let key = keys
            .load(&self.source.stable.request_fingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        Ok(URL_SAFE_NO_PAD.encode(
            ring::hmac::sign(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()),
                &self.confirmed_envelope_bytes()?,
            )
            .as_ref(),
        ))
    }
    pub fn verify_original_intent(&self, keys: &dyn KeyStore) -> Result<()> {
        if self.source.sha256()? != self.source_sha256
            || self.request.expected_revision != self.source.revision
            || self.request.instance_id != self.source.instance_id
        {
            return Err(Error::Conflict);
        }
        let key = keys
            .load(&self.source.stable.request_fingerprint)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let tag = URL_SAFE_NO_PAD
            .decode(&self.original_intent_mac)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        ring::hmac::verify(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()),
            &self.original_intent_bytes()?,
            &tag,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        if let Some(sealed) = &self.prepared_material {
            if Some(hash(json_string(sealed)?.as_bytes())) != self.prepared_material_sha256 {
                return Err(Error::Conflict);
            }
        }
        match (
            &self.confirmed_envelopes_mac,
            self.confirmed_envelopes.is_empty(),
        ) {
            (Some(encoded), false) => {
                let tag = URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| Error::NativeKeyUnavailable)?;
                ring::hmac::verify(
                    &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_ref()),
                    &self.confirmed_envelope_bytes()?,
                    &tag,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?;
            }
            (None, true) => {}
            _ => return Err(Error::Conflict),
        }
        Ok(())
    }
    pub fn candidate_material(&self, keys: &dyn KeyStore) -> Result<Zeroizing<[u8; 32]>> {
        self.verify_original_intent(keys)?;
        let proof = self.candidate_proof.as_ref().ok_or(Error::Conflict)?;
        if self.request.kind == ServerKeyOperationKind::ImportKms {
            return cowork_secrets::load_verified_key(keys, proof)
                .map_err(|_| Error::NativeKeyUnavailable);
        }
        let encoded = open_bearer(
            keys,
            &seed_scope(self),
            self.prepared_material.as_ref().ok_or(Error::Conflict)?,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| Error::NativeKeyUnavailable)?,
        );
        let mut result = Zeroizing::new([0u8; 32]);
        if bytes.len() != 32 {
            return Err(Error::NativeKeyUnavailable);
        }
        result.copy_from_slice(&bytes);
        Ok(result)
    }
    pub fn confirmed_registry(&self) -> Result<ServerKeyRegistry> {
        if self.format != 1
            || self.source.sha256()? != self.source_sha256
            || self.request.instance_id != self.source.instance_id
            || self.request.expected_revision != self.source.revision
        {
            return Err(Error::Conflict);
        }
        let mut next = self.source.clone();
        next.profile = Some(self.request.profile.clone());
        next.pending = None;
        next.revision = next.revision.checked_add(2).ok_or(Error::Conflict)?;
        match self.request.kind {
            ServerKeyOperationKind::ImportKms => {
                if self.source.profile.is_some() || self.confirmed_envelopes.len() != 1 {
                    return Err(Error::Conflict);
                }
                let wrapped = &self.confirmed_envelopes[0];
                if Some(&wrapped.proof) != self.candidate_proof.as_ref()
                    || Some(wrapped.envelope_id) != self.envelope_id
                    || wrapped.proof != next.keys[0].proof
                {
                    return Err(Error::Conflict);
                }
                next.keys[0].wrapped = Some(wrapped.clone());
            }
            ServerKeyOperationKind::RotateData | ServerKeyOperationKind::RotateAuditSigner => {
                if self.source.profile.as_ref() != Some(&self.request.profile)
                    || self.confirmed_envelopes.len() != 1
                    || next.keys.len() >= MAX_KEYS
                {
                    return Err(Error::Conflict);
                }
                let wrapped = &self.confirmed_envelopes[0];
                if Some(&wrapped.proof) != self.candidate_proof.as_ref()
                    || Some(wrapped.envelope_id) != self.envelope_id
                    || wrapped.proof.key.instance != next.instance_id
                    || next.keys.iter().any(|r| r.proof.key == wrapped.proof.key)
                {
                    return Err(Error::Conflict);
                }
                if self.request.kind == ServerKeyOperationKind::RotateData {
                    next.active = wrapped.proof.key;
                } else {
                    next.stable.operator_audit = Some(
                        self.source
                            .stable
                            .operator_audit
                            .unwrap_or(self.source.stable.audit),
                    );
                    next.stable.audit = wrapped.proof.key;
                    next.stable.audit_epoch = Some(
                        self.source
                            .stable
                            .audit_epoch
                            .unwrap_or(0)
                            .checked_add(1)
                            .ok_or(Error::Conflict)?,
                    );
                }
                next.keys.push(ServerKeyRecord {
                    proof: wrapped.proof.clone(),
                    wrapped: Some(wrapped.clone()),
                });
            }
            ServerKeyOperationKind::RewrapKms => {
                if self.source.profile.as_ref() != Some(&self.request.profile)
                    || self.confirmed_envelopes.len() != next.keys.len()
                {
                    return Err(Error::Conflict);
                }
                for (record, wrapped) in next.keys.iter_mut().zip(&self.confirmed_envelopes) {
                    let old = record.wrapped.as_ref().ok_or(Error::Conflict)?;
                    if wrapped.proof != record.proof
                        || wrapped.envelope_id != old.envelope_id
                        || wrapped.context_base64 != old.context_base64
                        || wrapped.wrapping_version <= old.wrapping_version
                    {
                        return Err(Error::Conflict);
                    }
                    record.wrapped = Some(wrapped.clone());
                }
            }
            ServerKeyOperationKind::RetireLegacyFile => {
                if self.source.profile.as_ref() != Some(&self.request.profile)
                    || !self.confirmed_envelopes.is_empty()
                    || self.request.legacy_file.is_none()
                    || self.envelope_id.is_some()
                    || self.prepared_material.is_some()
                {
                    return Err(Error::Conflict);
                }
            }
        }
        for wrapped in &self.confirmed_envelopes {
            wrapped
                .validate(&self.request.profile)
                .map_err(|_| Error::NativeKeyUnavailable)?;
            if wrapped.wrapping_version != self.request.wrapping_version {
                return Err(Error::Conflict);
            }
        }
        next.validate()?;
        Ok(next)
    }
}
async fn operation_in(
    tx: &mut Transaction<'_, Any>,
    request: Uuid,
) -> Result<Option<ServerKeyOperationStatus>> {
    let row = sqlx::query(
        "SELECT input_sha256,payload,state,receipt FROM server_key_operations WHERE request_id=$1",
    )
    .bind(request.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    row.map(|r| {
        let operation: ServerKeyOperation =
            parse(&r.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if operation.request.request_id != request
            || operation.request.sha256()?
                != r.try_get::<String, _>("input_sha256")
                    .map_err(storage_error)?
            || operation.source.sha256()? != operation.source_sha256
        {
            return Err(Error::Conflict);
        }
        let state = r.try_get::<String, _>("state").map_err(storage_error)?;
        let receipt: Option<SignedServerKeyReceipt> = r
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
            .map(|s| parse(&s))
            .transpose()?;
        if let Some(value) = &receipt {
            value.verify(&value.public_key_sha256)?;
            if value.payload.request != operation.request
                || value.payload.original_registry_sha256 != operation.source_sha256
                || value.payload.original_revision != operation.source.revision
                || value.payload.original_active != operation.source.active
                || value.payload.stable
                    != if state == "complete" {
                        operation.confirmed_registry()?.stable
                    } else {
                        operation.source.stable.clone()
                    }
                || (state == "complete"
                    && operation.request.kind == ServerKeyOperationKind::RotateAuditSigner)
                    != value.payload.audit_origin_plan_sha256.is_some()
                || (state == "complete"
                    && !matches!(
                        value.payload.result.as_str(),
                        "committed" | "legacy_original_file_observed_absent"
                    ))
                || (state == "cancelled" && value.payload.result != "cancelled_before_activation")
                || !matches!(state.as_str(), "complete" | "cancelled")
            {
                return Err(Error::Conflict);
            }
        } else if matches!(state.as_str(), "complete" | "cancelled") {
            return Err(Error::Conflict);
        }
        Ok(ServerKeyOperationStatus {
            operation,
            state,
            receipt,
        })
    })
    .transpose()
}
fn pending_matches(current: &ServerKeyRegistry, op: &ServerKeyOperation) -> Result<()> {
    let mut expected = op.source.clone();
    expected.pending = Some(op.request.request_id);
    expected.revision = expected.revision.checked_add(1).ok_or(Error::Conflict)?;
    if current != &expected {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn replace_registry(
    tx: &mut Transaction<'_, Any>,
    original: &ServerKeyRegistry,
    next: &ServerKeyRegistry,
) -> Result<()> {
    next.validate()?;
    if sqlx::query("UPDATE server_key_registry SET revision=$1,payload=$2 WHERE id=1 AND revision=$3 AND payload=$4").bind(next.revision).bind(json_string(next)?).bind(original.revision).bind(json_string(original)?).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
    Ok(())
}
impl Store {
    pub async fn server_key_registry(&self) -> Result<Option<ServerKeyRegistry>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = current_in(&mut tx).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn server_key_has_existing_ciphertext(&self) -> Result<bool> {
        for table in [
            "team_provider",
            "team_mcp_servers",
            "team_github",
            "personal_provider_credentials",
            "android_personal_provider_secrets",
            "android_private_snapshot_chunks",
            "android_private_loop_checkpoints",
            "android_private_loop_steps",
            "android_private_loop_versions",
            "android_private_mcp_bindings",
            "android_private_mcp_sessions",
            "android_push_subscriptions",
            "operator_device_configuration",
            "android_auth_totp",
            "android_auth_operations",
            "android_auth_transport_operations",
            "android_private_github_bindings",
            "audit_records",
            "audit_archive_receipts",
            "audit_witness_bindings",
            "audit_origin_epoch",
            "server_audit_rotation_plans",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&self.pool)
                .await
                .map_err(storage_error)?;
            if count != 0 {
                return Ok(true);
            }
        }
        let imports: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM historical_import_chunks WHERE owner LIKE 'org:%'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(storage_error)?;
        if imports != 0 {
            return Ok(true);
        }
        Ok(false)
    }
    pub async fn initialize_server_key_registry(
        &self,
        keys: &dyn KeyStore,
        id: KeyId,
    ) -> Result<ServerKeyRegistry> {
        let proof =
            cowork_secrets::prove_key(keys, &id).map_err(|_| Error::NativeKeyUnavailable)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if let Some(original) = current_in(&mut tx).await? {
            if original.profile.is_some()
                || original.active != id
                || original.pending.is_some()
                || original.keys.len() != 1
                || original.keys[0].proof != proof
            {
                return Err(Error::NativeKeyUnavailable);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(original);
        }
        let value = ServerKeyRegistry {
            format: 1,
            revision: 1,
            instance_id: id.instance,
            active: id,
            stable: StableKeyPurposes {
                audit: id,
                offline: id,
                request_fingerprint: id,
                operator_audit: None,
                audit_epoch: None,
            },
            profile: None,
            keys: vec![ServerKeyRecord {
                proof,
                wrapped: None,
            }],
            pending: None,
        };
        value.validate()?;
        sqlx::query("INSERT INTO server_key_registry(id,revision,payload) VALUES(1,1,$1)")
            .bind(json_string(&value)?)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Provider waits must finish before this final epoch check. No key profile,
    /// pending operation, or signer slot is adopted merely because it decrypts.
    pub async fn confirm_server_key_snapshot(&self, original: &ServerKeyRegistry) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        if original.pending.is_some() || current_in(&mut tx).await?.as_ref() != Some(original) {
            return Err(Error::NativeKeyUnavailable);
        }
        tx.commit().await.map_err(storage_error)
    }
    pub async fn server_key_operation(
        &self,
        request: Uuid,
    ) -> Result<Option<ServerKeyOperationStatus>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        guard(&mut tx).await?;
        let value = operation_in(&mut tx, request).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Same original request wins after reply loss; new random candidate inputs
    /// are ignored only when the immutable original request identity matches.
    pub async fn prepare_server_key_operation(
        &self,
        request: ServerKeyRequest,
        source: &ServerKeyRegistry,
        keys: &dyn KeyStore,
        candidate: Option<KeyProof>,
        material: Option<Zeroizing<[u8; 32]>>,
    ) -> Result<ServerKeyOperationStatus> {
        let input = request.sha256()?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let original = current_in(&mut tx)
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        if let Some(retained) = operation_in(&mut tx, request.request_id).await? {
            if retained.operation.request != request {
                return Err(Error::Conflict);
            }
            retained.operation.verify_original_intent(keys)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(retained);
        }
        if original != *source
            || original.pending.is_some()
            || source.instance_id != request.instance_id
            || source.revision != request.expected_revision
        {
            return Err(Error::Conflict);
        }
        quiescent(&mut tx).await?;
        for record in &source.keys {
            verify_key(keys, &record.proof).map_err(|_| Error::NativeKeyUnavailable)?;
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_key_operations")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if count >= MAX_OPERATIONS {
            return Err(Error::BudgetExceeded);
        }
        let mut operation = ServerKeyOperation {
            format: 1,
            request: request.clone(),
            source: source.clone(),
            source_sha256: source.sha256()?,
            prepared_at: sample(&mut tx).await?,
            candidate_proof: candidate,
            envelope_id: None,
            prepared_material: None,
            prepared_material_sha256: None,
            original_intent_mac: String::new(),
            confirmed_envelopes_mac: None,
            confirmed_envelopes: Vec::new(),
        };
        match request.kind {
            ServerKeyOperationKind::ImportKms => {
                if source.profile.is_some()
                    || material.is_some()
                    || operation.candidate_proof.as_ref() != Some(&source.keys[0].proof)
                {
                    return Err(Error::Conflict);
                }
                operation.envelope_id = Some(Uuid::new_v4());
            }
            ServerKeyOperationKind::RotateData | ServerKeyOperationKind::RotateAuditSigner => {
                if source.profile.as_ref() != Some(&request.profile)
                    || source.keys.len() >= MAX_KEYS
                {
                    return Err(Error::Conflict);
                }
                let proof = operation.candidate_proof.as_ref().ok_or(Error::Conflict)?;
                if proof.key.instance != source.instance_id
                    || source.keys.iter().any(|r| r.proof.key == proof.key)
                {
                    return Err(Error::Conflict);
                }
                let material = material.ok_or(Error::Conflict)?;
                operation.envelope_id = Some(Uuid::new_v4());
                operation.prepared_material = Some(
                    seal_bearer(
                        keys,
                        source.active,
                        &seed_scope(&operation),
                        &Zeroizing::new(STANDARD.encode(material.as_ref())),
                    )
                    .map_err(|_| Error::NativeKeyUnavailable)?,
                );
            }
            ServerKeyOperationKind::RewrapKms => {
                if source.profile.as_ref() != Some(&request.profile)
                    || operation.candidate_proof.is_some()
                    || material.is_some()
                    || source.keys.iter().any(|r| {
                        r.wrapped
                            .as_ref()
                            .is_none_or(|w| w.wrapping_version >= request.wrapping_version)
                    })
                {
                    return Err(Error::Conflict);
                }
            }
            ServerKeyOperationKind::RetireLegacyFile => {
                if source.profile.as_ref() != Some(&request.profile)
                    || material.is_some()
                    || operation.candidate_proof.as_ref() != Some(&source.keys[0].proof)
                {
                    return Err(Error::Conflict);
                }
            }
        }
        operation.prepared_material_sha256 = operation
            .prepared_material
            .as_ref()
            .map(|sealed| json_string(sealed).map(|raw| hash(raw.as_bytes())))
            .transpose()?;
        operation.original_intent_mac = operation.sign_original_intent(keys)?;
        let mut next = source.clone();
        next.pending = Some(request.request_id);
        next.revision = next.revision.checked_add(1).ok_or(Error::Conflict)?;
        sqlx::query("INSERT INTO server_key_operations(request_id,input_sha256,payload,state,receipt,completed_at) VALUES($1,$2,$3,'prepared',NULL,0)").bind(request.request_id.to_string()).bind(input).bind(json_string(&operation)?).execute(&mut *tx).await.map_err(storage_error)?;
        replace_registry(&mut tx, source, &next).await?;
        if request.kind == ServerKeyOperationKind::RotateAuditSigner {
            prepare_audit_epoch(&mut tx, &operation, keys).await?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(ServerKeyOperationStatus {
            operation,
            state: "prepared".into(),
            receipt: None,
        })
    }
    pub async fn record_server_key_candidate(
        &self,
        original: &ServerKeyOperation,
        envelopes: Vec<WrappedServerKey>,
        keys: &dyn KeyStore,
    ) -> Result<ServerKeyOperationStatus> {
        if original.request.kind == ServerKeyOperationKind::RetireLegacyFile {
            return Err(Error::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = current_in(&mut tx)
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let retained = operation_in(&mut tx, original.request.request_id)
            .await?
            .ok_or(Error::NotFound)?;
        retained.operation.verify_original_intent(keys)?;
        original.verify_original_intent(keys)?;
        if retained.operation.original_intent_mac != original.original_intent_mac {
            return Err(Error::Conflict);
        }
        if retained.operation.request != original.request
            || retained.operation.source_sha256 != original.source_sha256
        {
            return Err(Error::Conflict);
        }
        if retained.state != "prepared" {
            tx.commit().await.map_err(storage_error)?;
            return Ok(retained);
        }
        pending_matches(&current, &retained.operation)?;
        let mut next = retained.operation.clone();
        next.confirmed_envelopes = envelopes;
        next.confirmed_registry()?;
        next.confirmed_envelopes_mac = Some(next.sign_confirmed_envelopes(keys)?);
        // The provisional capsule is not a permanent bridge from an old master
        // to future data keys. Its authenticated digest remains in the journal.
        next.prepared_material = None;
        if sqlx::query("UPDATE server_key_operations SET payload=$1,state='confirmed' WHERE request_id=$2 AND state='prepared' AND payload=$3").bind(json_string(&next)?).bind(next.request.request_id.to_string()).bind(json_string(&retained.operation)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(ServerKeyOperationStatus {
            operation: next,
            state: "confirmed".into(),
            receipt: None,
        })
    }
    /// All cipher families and the active epoch commit in one SQL transaction.
    /// Only encryption envelopes change; immutable business bytes, old MACs,
    /// signer slots and original receipt identities are never rewritten.
    pub async fn commit_server_key_operation(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
        operator_rewrap: impl FnOnce(&dyn KeyStore, KeyId, i64, &str) -> Result<String>,
    ) -> Result<SignedServerKeyReceipt> {
        self.commit_server_key_operation_inner(request, keys, operator_rewrap, None)
            .await
    }
    pub async fn commit_server_audit_rotation(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
        new_origin_pin: &str,
        approvals: Vec<ImportAuditTrustTransition>,
    ) -> Result<SignedServerKeyReceipt> {
        self.commit_server_key_operation_inner(
            request,
            keys,
            rewrap_operator_configuration,
            Some((new_origin_pin.to_owned(), approvals)),
        )
        .await
    }
    async fn commit_server_key_operation_inner(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
        operator_rewrap: impl FnOnce(&dyn KeyStore, KeyId, i64, &str) -> Result<String>,
        audit_approvals: Option<(String, Vec<ImportAuditTrustTransition>)>,
    ) -> Result<SignedServerKeyReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = current_in(&mut tx)
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let retained = operation_in(&mut tx, request)
            .await?
            .ok_or(Error::NotFound)?;
        retained.operation.verify_original_intent(keys.as_ref())?;
        if let Some(receipt) = retained.receipt {
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        if retained.state != "confirmed" {
            return Err(Error::Conflict);
        }
        if retained.operation.request.kind == ServerKeyOperationKind::RetireLegacyFile {
            return Err(Error::Conflict);
        }
        pending_matches(&current, &retained.operation)?;
        quiescent(&mut tx).await?;
        let next = retained.operation.confirmed_registry()?;
        for record in &next.keys {
            verify_key(keys.as_ref(), &record.proof).map_err(|_| Error::NativeKeyUnavailable)?;
        }
        let audit_plan_sha256 = if retained.operation.request.kind
            == ServerKeyOperationKind::RotateAuditSigner
        {
            let (pin, approvals) = audit_approvals.ok_or(Error::Conflict)?;
            Some(
                activate_audit_epoch(&mut tx, &retained.operation, keys.clone(), &pin, approvals)
                    .await?,
            )
        } else {
            if audit_approvals.is_some() {
                return Err(Error::Conflict);
            }
            None
        };
        let mut rows = 0u64;
        let mut archive_rows = 0u64;
        if retained.operation.request.kind == ServerKeyOperationKind::RotateData {
            ciphertext_preflight(&mut tx).await?;
            rows = rows
                .checked_add(
                    super::team_service::rewrap_server_secrets(&mut tx, keys.as_ref(), next.active)
                        .await?,
                )
                .ok_or(Error::BudgetExceeded)?;
            rows = rows
                .checked_add(rewrap_personal(&mut tx, keys.as_ref(), next.active).await?)
                .ok_or(Error::BudgetExceeded)?;
            rows = rows
                .checked_add(
                    super::android_auth_security::rewrap_helper(
                        &mut tx,
                        keys.as_ref(),
                        next.active,
                    )
                    .await?,
                )
                .ok_or(Error::BudgetExceeded)?;
            rows = rows
                .checked_add(
                    super::android_private_mcp::rewrap_android_server_secrets(
                        &mut tx,
                        keys.as_ref(),
                        next.active,
                    )
                    .await?,
                )
                .ok_or(Error::BudgetExceeded)?;
            rows = rows
                .checked_add(
                    super::android_private_github::rewrap(&mut tx, keys.as_ref(), next.active)
                        .await?,
                )
                .ok_or(Error::BudgetExceeded)?;
            rows = rows
                .checked_add(
                    super::android_auth_security::transport::rewrap_helper(
                        &mut tx,
                        keys.as_ref(),
                        next.active,
                    )
                    .await?,
                )
                .ok_or(Error::BudgetExceeded)?;
            archive_rows = super::historical_imports::product::rewrap_helper(
                &mut tx,
                keys.as_ref(),
                next.active,
            )
            .await?;
            rows = rows
                .checked_add(archive_rows)
                .ok_or(Error::BudgetExceeded)?;
            if let Some(row) = sqlx::query(
                "SELECT revision,payload FROM operator_device_configuration WHERE id='devices-v1'",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?
            {
                let revision: i64 = row.try_get("revision").map_err(storage_error)?;
                let original: String = row.try_get("payload").map_err(storage_error)?;
                let rewritten = operator_rewrap(keys.as_ref(), next.active, revision, &original)?;
                if rewritten != original {
                    if sqlx::query("UPDATE operator_device_configuration SET payload=$1 WHERE id='devices-v1' AND revision=$2 AND payload=$3").bind(rewritten).bind(revision).bind(original).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
                    rows = rows.checked_add(1).ok_or(Error::BudgetExceeded)?;
                }
            }
        }
        if rows.checked_sub(archive_rows).ok_or(Error::Conflict)? > 100_000 {
            return Err(Error::BudgetExceeded);
        }
        let payload = ServerKeyReceiptPayload {
            format: 1,
            request: retained.operation.request.clone(),
            original_registry_sha256: retained.operation.source_sha256.clone(),
            result_registry_sha256: next.sha256()?,
            original_revision: retained.operation.source.revision,
            result_revision: next.revision,
            original_active: retained.operation.source.active,
            result_active: next.active,
            stable: next.stable.clone(),
            encrypted_rows_rewrapped: rows,
            result: "committed".into(),
            completed_at: completion_sample(&mut tx, retained.operation.prepared_at).await?,
            audit_origin_plan_sha256: audit_plan_sha256,
        };
        let authority = AuditAuthority::new_operator(keys, next.active)
            .map_err(super::team_service::identity_error)?;
        let (public_key, public_key_sha256, signature) = authority
            .sign_server_key_operation(&payload)
            .map_err(super::team_service::identity_error)?;
        let receipt = SignedServerKeyReceipt {
            payload,
            public_key,
            public_key_sha256,
            signature,
        };
        replace_registry(&mut tx, &current, &next).await?;
        if sqlx::query("UPDATE server_key_operations SET state='complete',receipt=$1,completed_at=$2 WHERE request_id=$3 AND state='confirmed' AND payload=$4").bind(json_string(&receipt)?).bind(receipt.payload.completed_at).bind(request.to_string()).bind(json_string(&retained.operation)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn cancel_server_key_operation(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
    ) -> Result<SignedServerKeyReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = current_in(&mut tx)
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let retained = operation_in(&mut tx, request)
            .await?
            .ok_or(Error::NotFound)?;
        retained.operation.verify_original_intent(keys.as_ref())?;
        if let Some(receipt) = retained.receipt {
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        if retained.operation.request.kind == ServerKeyOperationKind::RetireLegacyFile {
            return Err(Error::Conflict);
        }
        pending_matches(&current, &retained.operation)?;
        if retained.operation.request.kind == ServerKeyOperationKind::RotateAuditSigner {
            cancel_audit_epoch(&mut tx, &retained.operation, keys.clone()).await?;
        }
        let mut next = retained.operation.source.clone();
        next.revision = current.revision.checked_add(1).ok_or(Error::Conflict)?;
        let authority = AuditAuthority::new_operator(keys, next.active)
            .map_err(super::team_service::identity_error)?;
        let payload = ServerKeyReceiptPayload {
            format: 1,
            request: retained.operation.request.clone(),
            original_registry_sha256: retained.operation.source_sha256,
            result_registry_sha256: next.sha256()?,
            original_revision: retained.operation.source.revision,
            result_revision: next.revision,
            original_active: next.active,
            result_active: next.active,
            stable: next.stable.clone(),
            encrypted_rows_rewrapped: 0,
            result: "cancelled_before_activation".into(),
            completed_at: completion_sample(&mut tx, retained.operation.prepared_at).await?,
            audit_origin_plan_sha256: None,
        };
        let (public_key, public_key_sha256, signature) = authority
            .sign_server_key_operation(&payload)
            .map_err(super::team_service::identity_error)?;
        let receipt = SignedServerKeyReceipt {
            payload,
            public_key,
            public_key_sha256,
            signature,
        };
        replace_registry(&mut tx, &current, &next).await?;
        if sqlx::query("UPDATE server_key_operations SET state='cancelled',receipt=$1,completed_at=$2 WHERE request_id=$3 AND state IN('prepared','confirmed')").bind(json_string(&receipt)?).bind(receipt.payload.completed_at).bind(request.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    /// The fixed native file operation runs under this SQL writer fence. A
    /// filesystem acknowledgement lost before SQL commit leaves the original
    /// prepared operation intact; explicit reconciliation observes the same
    /// original path/proof, never replaces it or adopts a different file.
    pub async fn retire_server_original_file(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
        remove: impl FnOnce(&LegacyMasterFileProof) -> Result<()>,
    ) -> Result<SignedServerKeyReceipt> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = current_in(&mut tx)
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let retained = operation_in(&mut tx, request)
            .await?
            .ok_or(Error::NotFound)?;
        retained.operation.verify_original_intent(keys.as_ref())?;
        if let Some(receipt) = retained.receipt {
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        if retained.state != "prepared"
            || retained.operation.request.kind != ServerKeyOperationKind::RetireLegacyFile
        {
            return Err(Error::Conflict);
        }
        pending_matches(&current, &retained.operation)?;
        quiescent(&mut tx).await?;
        let next = retained.operation.confirmed_registry()?;
        for record in &next.keys {
            verify_key(keys.as_ref(), &record.proof).map_err(|_| Error::NativeKeyUnavailable)?;
        }
        let payload = ServerKeyReceiptPayload {
            format: 1,
            request: retained.operation.request.clone(),
            original_registry_sha256: retained.operation.source_sha256.clone(),
            result_registry_sha256: next.sha256()?,
            original_revision: retained.operation.source.revision,
            result_revision: next.revision,
            original_active: next.active,
            result_active: next.active,
            stable: next.stable.clone(),
            encrypted_rows_rewrapped: 0,
            result: "legacy_original_file_observed_absent".into(),
            completed_at: completion_sample(&mut tx, retained.operation.prepared_at).await?,
            audit_origin_plan_sha256: None,
        };
        let authority = AuditAuthority::new_operator(keys, next.active)
            .map_err(super::team_service::identity_error)?;
        let (public_key, public_key_sha256, signature) = authority
            .sign_server_key_operation(&payload)
            .map_err(super::team_service::identity_error)?;
        let receipt = SignedServerKeyReceipt {
            payload,
            public_key,
            public_key_sha256,
            signature,
        };
        remove(
            retained
                .operation
                .request
                .legacy_file
                .as_ref()
                .ok_or(Error::Conflict)?,
        )?;
        replace_registry(&mut tx, &current, &next).await?;
        if sqlx::query("UPDATE server_key_operations SET state='complete',receipt=$1,completed_at=$2 WHERE request_id=$3 AND state='prepared' AND payload=$4").bind(json_string(&receipt)?).bind(receipt.payload.completed_at).bind(request.to_string()).bind(json_string(&retained.operation)?).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
async fn quiescent(tx: &mut Transaction<'_, Any>) -> Result<()> {
    // An expired lease or wall-clock guess is never closure evidence.
    for query in [
        "SELECT COUNT(*) FROM team_runs WHERE status IN('running','unknown')",
        "SELECT COUNT(*) FROM android_private_server_runs WHERE state IN('running','waiting_for_approval','waiting_for_input','unknown')",
        "SELECT COUNT(*) FROM android_private_loop_steps WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_mcp_sessions WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_browser_sessions WHERE state!='closed'",
        "SELECT COUNT(*) FROM android_private_browser_requests WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_terminal_sessions WHERE state!='closed'",
        "SELECT COUNT(*) FROM android_private_terminal_inputs WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_github_sessions WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM android_private_github_requests WHERE state IN('admitted','uncertain')",
        "SELECT COUNT(*) FROM historical_import_file_operations WHERE owner LIKE 'org:%' AND state IN('prepared','uncertain')",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if count != 0 {
            return Err(Error::UncertainEffect);
        }
    }
    let containers_migration: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations WHERE version=55")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if containers_migration == 1 {
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_private_containers WHERE state!='removed'",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
        if open != 0 {
            return Err(Error::UncertainEffect);
        }
    } else if containers_migration != 0 {
        return Err(Error::Conflict);
    }
    Ok(())
}
struct AuditSnapshot {
    id: KeyId,
    epoch: i64,
    material: Zeroizing<[u8; 32]>,
}
impl KeyStore for AuditSnapshot {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new(*self.material))
    }
    fn audit_signing_epoch(&self, id: &KeyId) -> cowork_secrets::Result<i64> {
        if id != &self.id {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(self.epoch)
    }
}
fn original_audit(keys: &dyn KeyStore, op: &ServerKeyOperation) -> Result<AuditAuthority> {
    let proof = &op
        .source
        .keys
        .iter()
        .find(|r| r.proof.key == op.source.stable.audit)
        .ok_or(Error::Conflict)?
        .proof;
    let material =
        cowork_secrets::load_verified_key(keys, proof).map_err(|_| Error::NativeKeyUnavailable)?;
    AuditAuthority::new(
        Arc::new(AuditSnapshot {
            id: proof.key,
            epoch: op.source.stable.audit_epoch.unwrap_or(0),
            material,
        }),
        proof.key,
    )
    .map_err(super::team_service::identity_error)
}
async fn audit_bindings(tx: &mut Transaction<'_, Any>) -> Result<Vec<AuditWitnessBinding>> {
    let rows = sqlx::query(
        "SELECT scope,payload,payload_sha256 FROM audit_witness_bindings ORDER BY scope LIMIT 129",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    if rows.len() > 128 {
        return Err(Error::BudgetExceeded);
    }
    let mut result = Vec::new();
    for row in rows {
        let raw: String = row.try_get("payload").map_err(storage_error)?;
        if raw.len() > 16384
            || hash(raw.as_bytes())
                != row
                    .try_get::<String, _>("payload_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let p: AuditWitnessBinding = parse(&raw)?;
        cowork_identity::audit_checkpoints::verify_binding(
            &p,
            &p.origin_public_key_sha256,
            &p.witness_public_key_sha256,
        )
        .map_err(super::team_service::identity_error)?;
        if p.scope != row.try_get::<String, _>("scope").map_err(storage_error)? {
            return Err(Error::Conflict);
        }
        result.push(p);
    }
    Ok(result)
}
async fn bound_audit_ready(
    tx: &mut Transaction<'_, Any>,
    binding: &AuditWitnessBinding,
    old_pin: &str,
) -> Result<()> {
    if binding.origin_public_key_sha256 != old_pin
        || cowork_identity::audit_trust::pending(tx, &binding.scope)
            .await
            .map_err(super::team_service::identity_error)?
            .is_some()
    {
        return Err(Error::Conflict);
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_checkpoint_requests WHERE scope=$1 AND completed=0",
    )
    .bind(&binding.scope)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    if pending != 0 {
        return Err(Error::Conflict);
    }
    let previous = cowork_identity::audit_checkpoints::operator_latest(tx, &binding.scope, binding)
        .await
        .map_err(super::team_service::identity_error)?
        .ok_or(Error::Conflict)?;
    if previous.0.binding_id != binding.binding_id {
        return Err(Error::Conflict);
    }
    let checked = cowork_identity::audit::verification(tx, &binding.scope)
        .await
        .map_err(super::team_service::identity_error)?;
    if !checked.valid || !checked.complete || checked.chain_id != previous.0.chain_id {
        return Err(Error::Conflict);
    }
    let prefix = cowork_identity::audit::page(
        tx,
        &binding.scope,
        previous.0.head_sequence,
        Some(previous.0.head_sequence),
        1,
    )
    .await
    .map_err(super::team_service::identity_error)?;
    if prefix.previous_sha256 != previous.0.head_sha256 {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn prepare_audit_epoch(
    tx: &mut Transaction<'_, Any>,
    op: &ServerKeyOperation,
    keys: &dyn KeyStore,
) -> Result<()> {
    let authority = original_audit(keys, op)?;
    let heads = cowork_identity::audit_origin::lock_heads(tx)
        .await
        .map_err(super::team_service::identity_error)?;
    cowork_identity::audit_origin::exclusive(tx, &heads)
        .await
        .map_err(super::team_service::identity_error)?;
    if cowork_identity::audit_origin::installed(tx)
        .await
        .map_err(super::team_service::identity_error)?
        .is_none()
    {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_audit_rotation_plans")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        if count != 0
            || op.source.stable.audit_epoch.is_some()
            || op.source.stable.operator_audit.is_some()
        {
            return Err(Error::Conflict);
        }
        sqlx::query("INSERT INTO audit_origin_epoch(id,instance_id,key_id,epoch,public_key_sha256,pending_request_id) VALUES(1,$1,$2,0,$3,NULL)").bind(op.source.instance_id.to_string()).bind(op.source.stable.audit.key.to_string()).bind(authority.public_key_sha256().map_err(super::team_service::identity_error)?).execute(&mut **tx).await.map_err(storage_error)?;
    }
    cowork_identity::audit_origin::admission(tx, &authority)
        .await
        .map_err(super::team_service::identity_error)?;
    for b in audit_bindings(tx).await? {
        bound_audit_ready(
            tx,
            &b,
            &authority
                .public_key_sha256()
                .map_err(super::team_service::identity_error)?,
        )
        .await?;
    }
    if sqlx::query("UPDATE audit_origin_epoch SET pending_request_id=$1 WHERE id=1 AND pending_request_id IS NULL").bind(op.request.request_id.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    Ok(())
}
async fn audit_epoch_matches(
    tx: &mut Transaction<'_, Any>,
    op: &ServerKeyOperation,
    keys: &dyn KeyStore,
) -> Result<AuditAuthority> {
    let old = original_audit(keys, op)?;
    let row = cowork_identity::audit_origin::installed(tx)
        .await
        .map_err(super::team_service::identity_error)?
        .ok_or(Error::Conflict)?;
    if row.0 != op.source.instance_id
        || row.1 != op.source.stable.audit.key
        || row.2 != op.source.stable.audit_epoch.unwrap_or(0)
        || row.3
            != old
                .public_key_sha256()
                .map_err(super::team_service::identity_error)?
        || row.4 != Some(op.request.request_id)
    {
        return Err(Error::Conflict);
    }
    Ok(old)
}
async fn audit_plan_in(
    tx: &mut Transaction<'_, Any>,
    request: Uuid,
) -> Result<Option<AuditOriginRotationPlan>> {
    let Some(row) = sqlx::query(
        "SELECT payload,payload_sha256 FROM server_audit_rotation_plans WHERE request_id=$1",
    )
    .bind(request.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let raw: String = row.try_get("payload").map_err(storage_error)?;
    if raw.len() > 4 * 1024 * 1024
        || hash(raw.as_bytes())
            != row
                .try_get::<String, _>("payload_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    let plan: AuditOriginRotationPlan = parse(&raw)?;
    cowork_identity::audit_origin::verify_plan(
        &plan,
        &plan.previous_public_key_sha256,
        &plan.next_public_key_sha256,
    )
    .map_err(super::team_service::identity_error)?;
    if plan.request_id != request {
        return Err(Error::Conflict);
    }
    Ok(Some(plan))
}
fn audit_plan_matches(
    plan: &AuditOriginRotationPlan,
    op: &ServerKeyOperation,
    next: &ServerKeyRegistry,
) -> Result<()> {
    if plan.request_id != op.request.request_id
        || plan.instance_id != op.source.instance_id
        || plan.original_registry_sha256 != op.source_sha256
        || plan.original_audit_key != op.source.stable.audit.key
        || plan.next_audit_key != next.stable.audit.key
        || plan.original_epoch != op.source.stable.audit_epoch.unwrap_or(0)
        || Some(plan.next_epoch) != next.stable.audit_epoch
        || plan.prepared_at != op.prepared_at
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
impl Store {
    pub async fn server_audit_rotation_plan(
        &self,
        request: Uuid,
    ) -> Result<Option<AuditOriginRotationPlan>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        guard(&mut tx).await?;
        let p = audit_plan_in(&mut tx, request).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(p)
    }
    pub async fn confirm_server_audit_snapshot(
        &self,
        keys: Arc<dyn KeyStore>,
        active: KeyId,
    ) -> Result<()> {
        let authority =
            AuditAuthority::new(keys, active).map_err(super::team_service::identity_error)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        cowork_identity::audit_origin::admission(&mut tx, &authority)
            .await
            .map_err(super::team_service::identity_error)?;
        tx.commit().await.map_err(storage_error)
    }
    /// All old external checkpoints must be imported before this original
    /// private plan is published. No fresh signatures are admitted meanwhile.
    pub async fn stage_server_audit_rotation(
        &self,
        request: Uuid,
        keys: Arc<dyn KeyStore>,
    ) -> Result<AuditOriginRotationPlan> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = current_in(&mut tx).await?.ok_or(Error::Conflict)?;
        let retained = operation_in(&mut tx, request)
            .await?
            .ok_or(Error::NotFound)?;
        let op = &retained.operation;
        op.verify_original_intent(keys.as_ref())?;
        if op.request.kind != ServerKeyOperationKind::RotateAuditSigner {
            return Err(Error::Conflict);
        }
        let next = op.confirmed_registry()?;
        let old = original_audit(keys.as_ref(), op)?;
        let new = AuditAuthority::rotation_candidate(
            keys.clone(),
            op.candidate_proof.as_ref().ok_or(Error::Conflict)?,
        )
        .map_err(super::team_service::identity_error)?;
        if let Some(plan) = audit_plan_in(&mut tx, request).await? {
            cowork_identity::audit_origin::verify_plan(
                &plan,
                &old.public_key_sha256()
                    .map_err(super::team_service::identity_error)?,
                &new.public_key_sha256()
                    .map_err(super::team_service::identity_error)?,
            )
            .map_err(super::team_service::identity_error)?;
            audit_plan_matches(&plan, op, &next)?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(plan);
        }
        if retained.state != "confirmed" {
            return Err(Error::Conflict);
        }
        pending_matches(&current, op)?;
        quiescent(&mut tx).await?;
        let heads = cowork_identity::audit_origin::lock_heads(&mut tx)
            .await
            .map_err(super::team_service::identity_error)?;
        cowork_identity::audit_origin::exclusive(&mut tx, &heads)
            .await
            .map_err(super::team_service::identity_error)?;
        audit_epoch_matches(&mut tx, op, keys.as_ref()).await?;
        let mut proposals = Vec::new();
        let now = completion_sample(&mut tx, op.prepared_at).await?;
        for b in audit_bindings(&mut tx).await? {
            bound_audit_ready(
                &mut tx,
                &b,
                &old.public_key_sha256()
                    .map_err(super::team_service::identity_error)?,
            )
            .await?;
            let previous =
                cowork_identity::audit_checkpoints::operator_latest(&mut tx, &b.scope, &b)
                    .await
                    .map_err(super::team_service::identity_error)?
                    .ok_or(Error::Conflict)?;
            let input = PrepareAuditTrustTransition {
                request_id: Uuid::new_v4(),
                binding_id: b.binding_id,
                previous_checkpoint_sha256: previous.1,
                new_witness_public_key_sha256: b.witness_public_key_sha256.clone(),
                independently_confirmed: true,
            };
            proposals.push(
                cowork_identity::audit_trust::prepare_with_authorities(
                    &mut tx, &b.scope, None, &old, &new, input, now,
                )
                .await
                .map_err(super::team_service::identity_error)?,
            );
        }
        let p = AuditOriginRotationPlan {
            format: "cowork.audit.origin-rotation.v1".into(),
            request_id: request,
            instance_id: op.source.instance_id,
            original_registry_sha256: op.source_sha256.clone(),
            original_audit_key: op.source.stable.audit.key,
            next_audit_key: next.stable.audit.key,
            original_epoch: op.source.stable.audit_epoch.unwrap_or(0),
            next_epoch: next.stable.audit_epoch.ok_or(Error::Conflict)?,
            prepared_at: op.prepared_at,
            proposals,
            previous_public_key: String::new(),
            previous_public_key_sha256: String::new(),
            next_public_key: String::new(),
            next_public_key_sha256: String::new(),
            previous_origin_signature: String::new(),
            next_origin_signature: String::new(),
        };
        let plan = cowork_identity::audit_origin::sign_plan(p, &old, &new)
            .map_err(super::team_service::identity_error)?;
        let raw = json_string(&plan)?;
        sqlx::query("INSERT INTO server_audit_rotation_plans(request_id,payload,payload_sha256) VALUES($1,$2,$3)").bind(request.to_string()).bind(&raw).bind(hash(raw.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(plan)
    }
}
async fn activate_audit_epoch(
    tx: &mut Transaction<'_, Any>,
    op: &ServerKeyOperation,
    keys: Arc<dyn KeyStore>,
    confirmed_pin: &str,
    approvals: Vec<ImportAuditTrustTransition>,
) -> Result<String> {
    let plan = audit_plan_in(tx, op.request.request_id)
        .await?
        .ok_or(Error::Conflict)?;
    let heads = cowork_identity::audit_origin::lock_heads(tx)
        .await
        .map_err(super::team_service::identity_error)?;
    cowork_identity::audit_origin::exclusive(tx, &heads)
        .await
        .map_err(super::team_service::identity_error)?;
    let old = audit_epoch_matches(tx, op, keys.as_ref()).await?;
    let new = AuditAuthority::rotation_candidate(
        keys.clone(),
        op.candidate_proof.as_ref().ok_or(Error::Conflict)?,
    )
    .map_err(super::team_service::identity_error)?;
    cowork_identity::audit_origin::verify_plan(
        &plan,
        &old.public_key_sha256()
            .map_err(super::team_service::identity_error)?,
        &new.public_key_sha256()
            .map_err(super::team_service::identity_error)?,
    )
    .map_err(super::team_service::identity_error)?;
    audit_plan_matches(&plan, op, &op.confirmed_registry()?)?;
    if confirmed_pin != plan.next_public_key_sha256 || approvals.len() != plan.proposals.len() {
        return Err(Error::Conflict);
    }
    let bindings = audit_bindings(tx).await?;
    if bindings.len() != plan.proposals.len() {
        return Err(Error::Conflict);
    }
    let mut seen = BTreeSet::new();
    for p in &plan.proposals {
        let current = bindings
            .iter()
            .find(|b| b.scope == p.previous_binding.scope)
            .ok_or(Error::Conflict)?;
        if json_string(current)? != json_string(&p.previous_binding)? {
            return Err(Error::Conflict);
        }
        let input = approvals
            .iter()
            .find(|a| a.proposal_request_id == p.request_id)
            .ok_or(Error::Conflict)?;
        if input.request_id != p.request_id || !seen.insert(input.proposal_request_id) {
            return Err(Error::Conflict);
        }
        cowork_identity::audit_trust::import_operator(tx, &current.scope, input.clone())
            .await
            .map_err(super::team_service::identity_error)?;
    }
    if sqlx::query("UPDATE audit_origin_epoch SET key_id=$1,epoch=$2,public_key_sha256=$3,pending_request_id=NULL WHERE id=1 AND instance_id=$4 AND key_id=$5 AND epoch=$6 AND pending_request_id=$7").bind(plan.next_audit_key.to_string()).bind(plan.next_epoch).bind(&plan.next_public_key_sha256).bind(plan.instance_id.to_string()).bind(plan.original_audit_key.to_string()).bind(plan.original_epoch).bind(plan.request_id.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    Ok(hash(json_string(&plan)?.as_bytes()))
}
async fn cancel_audit_epoch(
    tx: &mut Transaction<'_, Any>,
    op: &ServerKeyOperation,
    keys: Arc<dyn KeyStore>,
) -> Result<()> {
    if audit_plan_in(tx, op.request.request_id).await?.is_some() {
        return Err(Error::UncertainEffect);
    }
    let heads = cowork_identity::audit_origin::lock_heads(tx)
        .await
        .map_err(super::team_service::identity_error)?;
    cowork_identity::audit_origin::exclusive(tx, &heads)
        .await
        .map_err(super::team_service::identity_error)?;
    audit_epoch_matches(tx, op, keys.as_ref()).await?;
    if sqlx::query("UPDATE audit_origin_epoch SET pending_request_id=NULL WHERE id=1 AND pending_request_id=$1").bind(op.request.request_id.to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict);}
    Ok(())
}
async fn ciphertext_preflight(tx: &mut Transaction<'_, Any>) -> Result<()> {
    let mut total_rows = 0u64;
    let mut total_bytes = 0u64;
    for (table, columns) in [
        ("team_provider", &["sealed_key"][..]),
        ("team_mcp_servers", &["sealed_config", "sealed_session"][..]),
        ("team_github", &["sealed_config"][..]),
        ("personal_provider_credentials", &["payload"][..]),
        ("android_personal_provider_secrets", &["sealed"][..]),
        ("android_private_snapshot_chunks", &["sealed"][..]),
        ("android_private_loop_checkpoints", &["sealed"][..]),
        ("android_private_loop_steps", &["intent", "receipt"][..]),
        ("android_private_loop_versions", &["sealed"][..]),
        ("android_private_mcp_bindings", &["sealed"][..]),
        (
            "android_private_mcp_sessions",
            &["definitions", "checkpoint"][..],
        ),
        ("android_push_subscriptions", &["sealed_token"][..]),
        ("operator_device_configuration", &["payload"][..]),
        ("android_auth_totp", &["sealed"][..]),
        ("android_auth_operations", &["sealed"][..]),
        ("android_auth_transport_operations", &["sealed"][..]),
        ("android_private_github_bindings", &["sealed"][..]),
    ] {
        let expressions = columns
            .iter()
            .map(|column| match tx.backend_name() {
                "SQLite" => Ok(format!("LENGTH(CAST(COALESCE({column},'') AS BLOB))")),
                "PostgreSQL" => Ok(format!("OCTET_LENGTH(COALESCE({column},''))")),
                _ => Err(Error::Conflict),
            })
            .collect::<Result<Vec<_>>>()?;
        let sql = format!(
            "SELECT COUNT(*) AS rows,CAST(COALESCE(SUM({}),0) AS BIGINT) AS bytes FROM {table}",
            expressions.join("+")
        );
        let row = sqlx::query(&sql)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
        let rows = u64::try_from(row.try_get::<i64, _>("rows").map_err(storage_error)?)
            .map_err(|_| Error::Conflict)?;
        let bytes = u64::try_from(row.try_get::<i64, _>("bytes").map_err(storage_error)?)
            .map_err(|_| Error::Conflict)?;
        total_rows = total_rows.checked_add(rows).ok_or(Error::BudgetExceeded)?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or(Error::BudgetExceeded)?;
        if total_rows > 100_000 || total_bytes > 512 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
    }
    // Archive ciphertext is processed in bounded batches, independent of the
    // ordinary secret budget. Do not reduce the admitted historical intake size.
    let length = match tx.backend_name() {
        "SQLite" => "LENGTH(CAST(sealed AS BLOB))",
        "PostgreSQL" => "OCTET_LENGTH(sealed)",
        _ => return Err(Error::Conflict),
    };
    let row=sqlx::query(&format!("SELECT CAST(COALESCE(MAX({length}),0) AS BIGINT) AS encoded,CAST(COALESCE(MIN(size),0) AS BIGINT) AS smallest,CAST(COALESCE(MAX(size),0) AS BIGINT) AS largest FROM historical_import_chunks WHERE owner LIKE 'org:%'" )).fetch_one(&mut **tx).await.map_err(storage_error)?;
    let encoded = u64::try_from(row.try_get::<i64, _>("encoded").map_err(storage_error)?)
        .map_err(|_| Error::Conflict)?;
    let smallest = row.try_get::<i64, _>("smallest").map_err(storage_error)?;
    let largest = u64::try_from(row.try_get::<i64, _>("largest").map_err(storage_error)?)
        .map_err(|_| Error::Conflict)?;
    if smallest < 0 || encoded > 2 * 1024 * 1024 || largest > 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorDeviceEnvelope {
    digest: String,
    parts: Vec<SealedCredential>,
}
/// Authenticates every exact original settings chunk, preserving its plaintext,
/// digest, business revision and installation AAD. No settings are applied here.
pub fn rewrap_operator_configuration(
    keys: &dyn KeyStore,
    target: KeyId,
    revision: i64,
    original: &str,
) -> Result<String> {
    let envelope: OperatorDeviceEnvelope = parse(original)?;
    if envelope.parts.is_empty()
        || envelope.parts.len() > 22
        || envelope.digest.len() != 64
        || revision <= 0
    {
        return Err(Error::NativeKeyUnavailable);
    }
    let scope = |id: KeyId, index: usize| CredentialScope {
        project_id: id.instance.to_string(),
        server_id: "operator-devices".into(),
        reference: format!(
            "device-settings:{}:{}:{index}",
            envelope.digest,
            envelope.parts.len()
        ),
        revision,
        endpoint: "installation://devices/v1".into(),
    };
    let mut plaintext = Zeroizing::new(Vec::new());
    let mut parts = Vec::new();
    for (index, part) in envelope.parts.iter().enumerate() {
        if part.key_id().instance != target.instance {
            return Err(Error::NativeKeyUnavailable);
        }
        let encoded = open_bearer(keys, &scope(part.key_id(), index), part)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded.as_bytes())
                .map_err(|_| Error::NativeKeyUnavailable)?,
        );
        if bytes.len() > 3000 || plaintext.len() + bytes.len() > 65536 {
            return Err(Error::BudgetExceeded);
        }
        plaintext.extend_from_slice(&bytes);
        parts.push(
            seal_bearer(keys, target, &scope(target, index), &encoded)
                .map_err(|_| Error::NativeKeyUnavailable)?,
        );
    }
    let settings: Value =
        serde_json::from_slice(&plaintext).map_err(|_| Error::NativeKeyUnavailable)?;
    if hash(&plaintext) != envelope.digest || settings["revision"].as_i64() != Some(revision) {
        return Err(Error::NativeKeyUnavailable);
    }
    json_string(&OperatorDeviceEnvelope {
        digest: envelope.digest,
        parts,
    })
}
async fn rewrap_personal(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    use super::personal_provider_credentials::PersonalProviderCredential;
    let rows=sqlx::query("SELECT provider_id,payload FROM personal_provider_credentials ORDER BY provider_id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    if rows.len() > 100_000 {
        return Err(Error::BudgetExceeded);
    }
    let mut count = 0;
    for row in rows {
        let original: String = row.try_get("payload").map_err(storage_error)?;
        let mut bound: PersonalProviderCredential = parse(&original)?;
        if let Some(sealed) = &bound.sealed {
            if sealed.key_id().instance != target.instance || sealed.key_id() == target {
                continue;
            }
            let plain =
                open_bearer(keys, &bound.scope, sealed).map_err(|_| Error::NativeKeyUnavailable)?;
            bound.sealed = Some(
                seal_bearer(keys, target, &bound.scope, &plain)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            );
            if sqlx::query("UPDATE personal_provider_credentials SET payload=$1 WHERE provider_id=$2 AND payload=$3").bind(json_string(&bound)?).bind(row.try_get::<String,_>("provider_id").map_err(storage_error)?).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixtureKeys {
        original: KeyId,
        next: KeyId,
        audit_next: KeyId,
    }
    struct EpochKeys {
        keys: Arc<dyn KeyStore>,
        stable: StableKeyPurposes,
    }
    impl KeyStore for EpochKeys {
        fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
            self.keys.load(id)
        }
        fn key_for_purpose(
            &self,
            id: &KeyId,
            purpose: cowork_secrets::KeyPurpose,
        ) -> cowork_secrets::Result<KeyId> {
            self.load(id)?;
            use cowork_secrets::KeyPurpose::*;
            Ok(match purpose {
                DataEncryption => *id,
                AuditSigning => self.stable.audit,
                OfflineSigning => self.stable.offline,
                RequestFingerprint => self.stable.request_fingerprint,
                OperatorSigning => self.stable.operator_audit.unwrap_or(self.stable.audit),
            })
        }
        fn audit_signing_epoch(&self, id: &KeyId) -> cowork_secrets::Result<i64> {
            self.load(id)?;
            Ok(self.stable.audit_epoch.unwrap_or(0))
        }
    }
    impl KeyStore for FixtureKeys {
        fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
            if *id == self.original {
                Ok(Zeroizing::new([7; 32]))
            } else if *id == self.next {
                Ok(Zeroizing::new([13; 32]))
            } else if *id == self.audit_next {
                Ok(Zeroizing::new([19; 32]))
            } else {
                Err(cowork_secrets::Error::Missing)
            }
        }
        fn key_for_purpose(
            &self,
            id: &KeyId,
            purpose: cowork_secrets::KeyPurpose,
        ) -> cowork_secrets::Result<KeyId> {
            self.load(id)?;
            Ok(if purpose == cowork_secrets::KeyPurpose::DataEncryption {
                *id
            } else {
                self.original
            })
        }
    }
    // Metadata fixture only: no simulated provider acknowledgement or cloud RPC.
    // The integration adapter independently confirms real ciphertext by unwrap.
    fn envelope(
        profile: &VaultTransitProfile,
        id: Uuid,
        proof: KeyProof,
        version: u32,
    ) -> WrappedServerKey {
        let sha = profile.sha256().unwrap();
        WrappedServerKey {
            format: 1,
            envelope_id: id,
            provider_sha256: sha.clone(),
            context_base64: STANDARD.encode(
                serde_json::to_vec(&("cowork:server:kms-envelope:v1", &sha, id, &proof)).unwrap(),
            ),
            proof,
            ciphertext: format!("vault:v{version}:{}", STANDARD.encode([0u8; 48])),
            wrapping_version: version,
        }
    }
    #[tokio::test]
    async fn atomic_rotation_preserves_original_signers_receipts_and_fences_old_writers() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&directory.path().join("fixture.sqlite3"))
            .await
            .unwrap();
        let instance = Uuid::new_v4();
        let old = KeyId {
            instance,
            key: Uuid::new_v4(),
        };
        let next = KeyId {
            instance,
            key: Uuid::new_v4(),
        };
        let audit_next = KeyId {
            instance,
            key: Uuid::new_v4(),
        };
        let keys: Arc<dyn KeyStore> = Arc::new(FixtureKeys {
            original: old,
            next,
            audit_next,
        });
        let source = store
            .initialize_server_key_registry(keys.as_ref(), old)
            .await
            .unwrap();
        let origin = AuditAuthority::new(keys.clone(), old)
            .unwrap()
            .public_key_sha256()
            .unwrap();
        let profile = ProviderProfile {
            id: Uuid::new_v4().to_string(),
            name: "Fixture".into(),
            kind: ProviderKind::OpenaiCompatible,
            endpoint: "https://fixture.invalid/v1".into(),
            model: "fixture".into(),
            revision: 1,
            context_tokens: 32768,
            credential_required: true,
            credential_ready: true,
        };
        let scope = super::super::personal_provider_credentials::personal_provider_scope(&profile);
        let credential = super::super::personal_provider_credentials::PersonalProviderCredential {
            profile_sha256: canonical_hash(&profile),
            scope: scope.clone(),
            sealed: Some(seal_bearer(keys.as_ref(), old, &scope, "fixture-secret").unwrap()),
            allow_no_key: false,
        };
        store
            .create_personal_provider_with_credential(
                "original-request",
                &"a".repeat(64),
                &profile,
                &credential,
            )
            .await
            .unwrap();
        let original_payload: String = sqlx::query_scalar(
            "SELECT payload FROM personal_provider_credentials WHERE provider_id=$1",
        )
        .bind(&profile.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let request_row:String=sqlx::query_scalar("SELECT request_sha256 FROM personal_provider_requests WHERE request_key='original-request'").fetch_one(&store.pool).await.unwrap();
        let vault = VaultTransitProfile {
            format: 1,
            provider_id: Uuid::new_v4(),
            endpoint: "https://kms.invalid/".into(),
            mount: "transit".into(),
            key_name: "server-master".into(),
            namespace: None,
        };
        let import = ServerKeyRequest {
            format: 1,
            request_id: Uuid::new_v4(),
            expected_revision: 1,
            instance_id: instance,
            kind: ServerKeyOperationKind::ImportKms,
            profile: vault.clone(),
            wrapping_version: 1,
            legacy_file: None,
        };
        // Actual migrated 0055 table and foreign keys, with metadata fixtures
        // only: logical completion/expired lease do not prove container removal.
        let mut tx = store.pool.begin().await.unwrap();
        for query in [
            "INSERT INTO identity_subjects(id,username,display_name,active,platform_admin,revision,password_salt,password_hash) VALUES('rotation-owner','rotation-owner','Fixture',1,0,1,'fixture','fixture')",
            "INSERT INTO identity_sessions(id,identity_id,token_sha256,expires_at,revoked,revision) VALUES('rotation-session','rotation-owner','rotation-session-sha',0,0,1)",
            "INSERT INTO android_personal_projects(id,owner_id,revision,payload,deleted) VALUES('rotation-project','rotation-owner',1,'{}',0)",
            "INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted) VALUES('rotation-thread','rotation-project','rotation-owner',1,'{}',0)",
            "INSERT INTO android_personal_provider_profiles(id,owner_id,revision,payload,deleted) VALUES('rotation-provider','rotation-owner',1,'{}',0)",
            "INSERT INTO android_private_server_runs(id,owner_id,session_id,session_revision,project_id,project_revision,thread_id,provider_id,creation_key,request_sha,initial_receipt,payload,state,lease_until,created_at) VALUES('rotation-run','rotation-owner','rotation-session',1,'rotation-project',1,'rotation-thread','rotation-provider','fixture','fixture','{}','{}','completed',0,0)",
            "INSERT INTO android_private_containers(id,run_id,owner_id,project_id,invocation_id,lease_id,original_binding,creation_sha,state,revision,created_at) VALUES('rotation-container','rotation-run','rotation-owner','rotation-project','fixture','expired','{}','fixture','prepared',1,0)",
        ] {
            sqlx::query(query).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        for state in ["prepared", "uncertain", "terminated"] {
            sqlx::query(
                "UPDATE android_private_containers SET state=$1 WHERE id='rotation-container'",
            )
            .bind(state)
            .execute(&store.pool)
            .await
            .unwrap();
            assert!(matches!(
                store
                    .prepare_server_key_operation(
                        import.clone(),
                        &source,
                        keys.as_ref(),
                        Some(source.keys[0].proof.clone()),
                        None
                    )
                    .await,
                Err(Error::UncertainEffect)
            ));
            assert!(store.server_key_registry().await.unwrap().unwrap() == source);
            assert!(
                store
                    .server_key_operation(import.request_id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        sqlx::query(
            "UPDATE android_private_containers SET state='removed' WHERE id='rotation-container'",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        // Actual 0060 metadata/FK fixtures: a physically closed browser
        // does not erase an unknown original request effect.
        for q in [
            "INSERT INTO android_private_loop_versions(id,run_id,path,kind,digest,size,sealed,created_at) VALUES('rotation-browser-baseline','rotation-run','fixture','artifact','fixture',0,'{}',0)",
            "INSERT INTO android_private_browser_sessions(id,run_id,owner_id,project_id,lease_id,container_nonce,baseline_version,last_sequence,state,revision,created_at) VALUES('rotation-browser','rotation-run','rotation-owner','rotation-project','expired','rotation-container','rotation-browser-baseline',0,'admitted',1,0)",
            "INSERT INTO android_private_browser_requests(session_id,action_sequence,request_id,request_sha,intent_version,state,created_at) VALUES('rotation-browser',1,'fixture','fixture','rotation-browser-baseline','admitted',0)",
        ] {
            sqlx::query(q).execute(&store.pool).await.unwrap();
        }
        for state in ["admitted", "ready", "uncertain", "closed"] {
            sqlx::query(
                "UPDATE android_private_browser_sessions SET state=$1 WHERE id='rotation-browser'",
            )
            .bind(state)
            .execute(&store.pool)
            .await
            .unwrap();
            assert!(matches!(
                store
                    .prepare_server_key_operation(
                        import.clone(),
                        &source,
                        keys.as_ref(),
                        Some(source.keys[0].proof.clone()),
                        None
                    )
                    .await,
                Err(Error::UncertainEffect)
            ));
        }
        sqlx::query("UPDATE android_private_browser_requests SET state='received' WHERE session_id='rotation-browser'").execute(&store.pool).await.unwrap();
        // Remove only synthetic metadata before exercising real cipher rewrap.
        for q in [
            "DELETE FROM android_private_browser_requests WHERE session_id='rotation-browser'",
            "DELETE FROM android_private_browser_sessions WHERE id='rotation-browser'",
            "DELETE FROM android_private_loop_versions WHERE id='rotation-browser-baseline'",
        ] {
            sqlx::query(q).execute(&store.pool).await.unwrap();
        }
        let prepared = store
            .prepare_server_key_operation(
                import.clone(),
                &source,
                keys.as_ref(),
                Some(source.keys[0].proof.clone()),
                None,
            )
            .await
            .unwrap();
        let mut tx = store.pool.begin().await.unwrap();
        assert!(cipher_writer(&mut tx, old).await.is_err());
        tx.rollback().await.unwrap();
        let original_envelope = envelope(
            &vault,
            prepared.operation.envelope_id.unwrap(),
            source.keys[0].proof.clone(),
            1,
        );
        store
            .record_server_key_candidate(
                &prepared.operation,
                vec![original_envelope],
                keys.as_ref(),
            )
            .await
            .unwrap();
        let receipt = store
            .commit_server_key_operation(
                import.request_id,
                keys.clone(),
                rewrap_operator_configuration,
            )
            .await
            .unwrap();
        receipt.verify(&origin).unwrap();
        let original_receipt = json_string(&receipt).unwrap();
        assert_eq!(
            json_string(
                &store
                    .commit_server_key_operation(
                        import.request_id,
                        keys.clone(),
                        rewrap_operator_configuration
                    )
                    .await
                    .unwrap()
            )
            .unwrap(),
            original_receipt
        );
        let imported = store.server_key_registry().await.unwrap().unwrap();
        assert_eq!(imported.revision, 3);
        let rotate = ServerKeyRequest {
            format: 1,
            request_id: Uuid::new_v4(),
            expected_revision: 3,
            instance_id: instance,
            kind: ServerKeyOperationKind::RotateData,
            profile: vault.clone(),
            wrapping_version: 1,
            legacy_file: None,
        };
        let next_proof = cowork_secrets::prove_key(keys.as_ref(), &next).unwrap();
        let prepared = store
            .prepare_server_key_operation(
                rotate.clone(),
                &imported,
                keys.as_ref(),
                Some(next_proof.clone()),
                Some(Zeroizing::new([13; 32])),
            )
            .await
            .unwrap();
        assert_eq!(
            *prepared
                .operation
                .candidate_material(keys.as_ref())
                .unwrap(),
            [13; 32]
        );
        let mut substituted = prepared.operation.clone();
        substituted.envelope_id = Some(Uuid::new_v4());
        assert!(substituted.verify_original_intent(keys.as_ref()).is_err());
        let confirmed = store
            .record_server_key_candidate(
                &prepared.operation,
                vec![envelope(
                    &vault,
                    prepared.operation.envelope_id.unwrap(),
                    next_proof,
                    1,
                )],
                keys.as_ref(),
            )
            .await
            .unwrap();
        assert!(confirmed.operation.prepared_material.is_none());
        assert!(confirmed.operation.prepared_material_sha256.is_some());
        let mut substituted_ack = confirmed.operation.clone();
        substituted_ack.confirmed_envelopes[0].ciphertext =
            format!("vault:v1:{}", STANDARD.encode([1u8; 48]));
        assert!(
            substituted_ack
                .verify_original_intent(keys.as_ref())
                .is_err()
        );
        let mut corrupted: Value = serde_json::from_str(&original_payload).unwrap();
        corrupted["sealed"]["ciphertext"][0] =
            json!(corrupted["sealed"]["ciphertext"][0].as_u64().unwrap() ^ 1);
        sqlx::query("UPDATE personal_provider_credentials SET payload=$1 WHERE provider_id=$2")
            .bind(json_string(&corrupted).unwrap())
            .bind(&profile.id)
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(
            store
                .commit_server_key_operation(
                    rotate.request_id,
                    keys.clone(),
                    rewrap_operator_configuration
                )
                .await
                .is_err()
        );
        let pending = store.server_key_registry().await.unwrap().unwrap();
        assert_eq!(pending.active, old);
        assert_eq!(pending.pending, Some(rotate.request_id));
        assert_eq!(
            store
                .server_key_operation(rotate.request_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            "confirmed"
        );
        sqlx::query("UPDATE personal_provider_credentials SET payload=$1 WHERE provider_id=$2")
            .bind(&original_payload)
            .bind(&profile.id)
            .execute(&store.pool)
            .await
            .unwrap();
        let rotated = store
            .commit_server_key_operation(
                rotate.request_id,
                keys.clone(),
                rewrap_operator_configuration,
            )
            .await
            .unwrap();
        rotated.verify(&origin).unwrap();
        assert_eq!(rotated.payload.encrypted_rows_rewrapped, 1);
        let current = store.server_key_registry().await.unwrap().unwrap();
        assert_eq!(current.active, next);
        assert_eq!(current.stable.audit, old);
        assert_eq!(current.stable.request_fingerprint, old);
        let actual = store
            .personal_provider_credential(&profile)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.sealed.as_ref().unwrap().key_id(), next);
        assert_eq!(
            &*open_bearer(keys.as_ref(), &scope, actual.sealed.as_ref().unwrap()).unwrap(),
            "fixture-secret"
        );
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT request_sha256 FROM personal_provider_requests WHERE request_key='original-request'").fetch_one(&store.pool).await.unwrap(),request_row);
        assert_eq!(
            AuditAuthority::new(keys.clone(), next)
                .unwrap()
                .public_key_sha256()
                .unwrap(),
            origin
        );
        let mut tx = store.pool.begin().await.unwrap();
        assert!(cipher_writer(&mut tx, old).await.is_err());
        assert!(cipher_writer(&mut tx, next).await.is_ok());
        tx.rollback().await.unwrap();
        let replay = store
            .prepare_server_key_operation(rotate.clone(), &imported, keys.as_ref(), None, None)
            .await
            .unwrap();
        assert_eq!(
            json_string(&replay.receipt.unwrap()).unwrap(),
            json_string(&rotated).unwrap()
        );
        let mut altered = rotate.clone();
        altered.wrapping_version = 2;
        assert!(
            store
                .prepare_server_key_operation(altered, &imported, keys.as_ref(), None, None)
                .await
                .is_err()
        );
        let rewrap = ServerKeyRequest {
            format: 1,
            request_id: Uuid::new_v4(),
            expected_revision: current.revision,
            instance_id: instance,
            kind: ServerKeyOperationKind::RewrapKms,
            profile: vault.clone(),
            wrapping_version: 2,
            legacy_file: None,
        };
        let data_before = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM personal_provider_credentials WHERE provider_id=$1",
        )
        .bind(&profile.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let prepared = store
            .prepare_server_key_operation(rewrap.clone(), &current, keys.as_ref(), None, None)
            .await
            .unwrap();
        let custody = current
            .keys
            .iter()
            .map(|r| {
                let old = r.wrapped.as_ref().unwrap();
                envelope(&vault, old.envelope_id, r.proof.clone(), 2)
            })
            .collect();
        store
            .record_server_key_candidate(&prepared.operation, custody, keys.as_ref())
            .await
            .unwrap();
        let wrapped = store
            .commit_server_key_operation(
                rewrap.request_id,
                keys.clone(),
                rewrap_operator_configuration,
            )
            .await
            .unwrap();
        wrapped.verify(&origin).unwrap();
        assert_eq!(wrapped.payload.encrypted_rows_rewrapped, 0);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT payload FROM personal_provider_credentials WHERE provider_id=$1"
            )
            .bind(&profile.id)
            .fetch_one(&store.pool)
            .await
            .unwrap(),
            data_before
        );
        assert_eq!(
            json_string(
                &store
                    .server_key_operation(import.request_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .receipt
                    .unwrap()
            )
            .unwrap(),
            original_receipt
        );
        // A distinct audit epoch leaves the active data key and original
        // operator signer untouched. The witness proof is pure Ring signing;
        // no physical witness installation or Vault acknowledgement is implied.
        use cowork_identity::{audit as audit_crypto, audit_checkpoints as cp, audit_trust};
        use ring::signature::{Ed25519KeyPair, KeyPair};
        let authority = AuditAuthority::new(keys.clone(), next).unwrap();
        let archive_input = CaptureAuditArchive {
            request_id: Uuid::new_v4(),
            after_sequence: 0,
            through_sequence: None,
            limit: 500,
        };
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_origin_guard WHERE id=1")
                .fetch_one(&store.pool)
                .await
                .unwrap(),
            1
        );
        sqlx::query("DELETE FROM audit_origin_guard WHERE id=1")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(
            store
                .personal_audit_capture(&authority, archive_input.clone())
                .await
                .is_err()
        );
        sqlx::query("INSERT INTO audit_origin_guard(id) VALUES(1)")
            .execute(&store.pool)
            .await
            .unwrap();
        let archive = store
            .personal_audit_capture(&authority, archive_input.clone())
            .await
            .unwrap();
        let witness = Ed25519KeyPair::from_seed_unchecked(&[25; 32]).unwrap();
        let witness_pin = audit_crypto::hash(witness.public_key().as_ref());
        let binding = store
            .personal_audit_witness(
                &authority,
                ConfigureAuditWitness {
                    request_id: Uuid::new_v4(),
                    origin_id: Uuid::new_v4(),
                    origin_public_key_sha256: origin.clone(),
                    witness_public_key_sha256: witness_pin.clone(),
                    independently_confirmed: true,
                },
            )
            .await
            .unwrap();
        let checkpoint_input = CaptureAuditCheckpoint {
            request_id: Uuid::new_v4(),
            binding_id: binding.binding_id,
            previous_checkpoint_sha256: None,
        };
        let checkpoint = store
            .personal_audit_checkpoint_capture(&authority, checkpoint_input.clone())
            .await
            .unwrap();
        let checkpoint_receipt = cp::witness(
            &checkpoint,
            None,
            &origin,
            &witness,
            checkpoint.captured_at + 1,
            None,
        )
        .unwrap();
        let checkpoint_original =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec_pretty(&checkpoint_receipt).unwrap());
        store
            .personal_audit_checkpoint_import(ImportAuditCheckpoint {
                request_id: Uuid::new_v4(),
                candidate_request_id: checkpoint.request_id,
                checkpoint_bytes_base64: checkpoint_original.clone(),
            })
            .await
            .unwrap();
        let audit_source = store.server_key_registry().await.unwrap().unwrap();
        let change = ServerKeyRequest {
            format: 1,
            request_id: Uuid::new_v4(),
            expected_revision: audit_source.revision,
            instance_id: instance,
            kind: ServerKeyOperationKind::RotateAuditSigner,
            profile: vault.clone(),
            wrapping_version: 2,
            legacy_file: None,
        };
        let audit_proof = cowork_secrets::prove_key(keys.as_ref(), &audit_next).unwrap();
        let prepared = store
            .prepare_server_key_operation(
                change.clone(),
                &audit_source,
                keys.as_ref(),
                Some(audit_proof.clone()),
                Some(Zeroizing::new([19; 32])),
            )
            .await
            .unwrap();
        let fresh_archive = CaptureAuditArchive {
            request_id: Uuid::new_v4(),
            ..archive_input.clone()
        };
        assert!(
            store
                .personal_audit_capture(&authority, fresh_archive.clone())
                .await
                .is_err()
        );
        assert_eq!(
            cp::canonical(
                &store
                    .personal_audit_capture(&authority, archive_input.clone())
                    .await
                    .unwrap()
            )
            .unwrap(),
            cp::canonical(&archive).unwrap()
        );
        let confirmed = store
            .record_server_key_candidate(
                &prepared.operation,
                vec![envelope(
                    &vault,
                    prepared.operation.envelope_id.unwrap(),
                    audit_proof,
                    2,
                )],
                keys.as_ref(),
            )
            .await
            .unwrap();
        let audit_next_registry = confirmed.operation.confirmed_registry().unwrap();
        let epoch_keys: Arc<dyn KeyStore> = Arc::new(EpochKeys {
            keys: keys.clone(),
            stable: audit_next_registry.stable.clone(),
        });
        let plan = store
            .stage_server_audit_rotation(change.request_id, epoch_keys.clone())
            .await
            .unwrap();
        assert_eq!(plan.proposals.len(), 1);
        assert_eq!(plan.original_epoch, 0);
        assert_eq!(plan.next_epoch, 1);
        let next_pin = plan.next_public_key_sha256.clone();
        cowork_identity::audit_origin::verify_plan(&plan, &origin, &next_pin).unwrap();
        assert_eq!(
            cp::canonical(
                &store
                    .stage_server_audit_rotation(change.request_id, epoch_keys.clone())
                    .await
                    .unwrap()
            )
            .unwrap(),
            cp::canonical(&plan).unwrap()
        );
        assert!(matches!(
            store
                .cancel_server_key_operation(change.request_id, epoch_keys.clone())
                .await,
            Err(Error::UncertainEffect)
        ));
        let transition = audit_trust::sign_transition(
            plan.proposals[0].clone(),
            &origin,
            &next_pin,
            &witness,
            &witness,
            plan.proposals[0].prepared_at,
        )
        .unwrap();
        let original_transition =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec_pretty(&transition).unwrap());
        let approval = ImportAuditTrustTransition {
            request_id: transition.proposal.request_id,
            proposal_request_id: transition.proposal.request_id,
            original_bytes_base64: original_transition.clone(),
            independently_confirmed: true,
        };
        assert!(
            store
                .personal_audit_trust_import(approval.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .commit_server_audit_rotation(
                    change.request_id,
                    epoch_keys.clone(),
                    &"0".repeat(64),
                    vec![approval.clone()]
                )
                .await
                .is_err()
        );
        let mut forged = transition.clone();
        forged.next_witness_signature = URL_SAFE_NO_PAD.encode([0u8; 64]);
        let forged_input = ImportAuditTrustTransition {
            original_bytes_base64: URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap()),
            ..approval.clone()
        };
        assert!(
            store
                .commit_server_audit_rotation(
                    change.request_id,
                    epoch_keys.clone(),
                    &next_pin,
                    vec![forged_input]
                )
                .await
                .is_err()
        );
        assert_eq!(
            store
                .personal_audit_checkpoint_status()
                .await
                .unwrap()
                .binding
                .unwrap()
                .binding_id,
            binding.binding_id
        );
        assert_eq!(
            store.server_key_registry().await.unwrap().unwrap().pending,
            Some(change.request_id)
        );
        let activated = store
            .commit_server_audit_rotation(
                change.request_id,
                epoch_keys.clone(),
                &next_pin,
                vec![approval.clone()],
            )
            .await
            .unwrap();
        activated.verify(&origin).unwrap();
        assert_eq!(activated.payload.encrypted_rows_rewrapped, 0);
        assert_eq!(
            activated.payload.audit_origin_plan_sha256,
            Some(audit_crypto::hash(&cp::canonical(&plan).unwrap()))
        );
        let after = store.server_key_registry().await.unwrap().unwrap();
        assert_eq!(after.active, next);
        assert_eq!(after.stable.audit, audit_next);
        assert_eq!(after.stable.operator_audit, Some(old));
        assert_eq!(after.stable.request_fingerprint, old);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT payload FROM personal_provider_credentials WHERE provider_id=$1"
            )
            .bind(&profile.id)
            .fetch_one(&store.pool)
            .await
            .unwrap(),
            data_before
        );
        assert!(
            store
                .confirm_server_audit_snapshot(keys.clone(), next)
                .await
                .is_err()
        );
        store
            .confirm_server_audit_snapshot(epoch_keys.clone(), next)
            .await
            .unwrap();
        assert!(
            store
                .personal_audit_capture(&authority, fresh_archive.clone())
                .await
                .is_err()
        );
        assert_eq!(
            cp::canonical(
                &store
                    .personal_audit_capture(&authority, archive_input)
                    .await
                    .unwrap()
            )
            .unwrap(),
            cp::canonical(&archive).unwrap()
        );
        let new_authority = AuditAuthority::new(epoch_keys.clone(), next).unwrap();
        assert_eq!(new_authority.public_key_sha256().unwrap(), next_pin);
        let new_archive = store
            .personal_audit_capture(&new_authority, fresh_archive)
            .await
            .unwrap();
        audit_crypto::verify_archive(&new_archive, &next_pin).unwrap();
        audit_crypto::verify_archive(&archive, &origin).unwrap();
        let current_binding = store
            .personal_audit_checkpoint_status()
            .await
            .unwrap()
            .binding
            .unwrap();
        let new_checkpoint = store
            .personal_audit_checkpoint_capture(
                &new_authority,
                CaptureAuditCheckpoint {
                    request_id: Uuid::new_v4(),
                    binding_id: current_binding.binding_id,
                    previous_checkpoint_sha256: Some(audit_crypto::hash(
                        &cp::canonical(&checkpoint_receipt).unwrap(),
                    )),
                },
            )
            .await
            .unwrap();
        let new_receipt = cp::witness_with_transition(
            &new_checkpoint,
            Some(&checkpoint_receipt),
            &next_pin,
            &witness,
            new_checkpoint.captured_at + 1,
            None,
            Some(&transition),
        )
        .unwrap();
        assert_eq!(new_receipt.witness_sequence, 2);
        store
            .personal_audit_checkpoint_import(ImportAuditCheckpoint {
                request_id: Uuid::new_v4(),
                candidate_request_id: new_checkpoint.request_id,
                checkpoint_bytes_base64: URL_SAFE_NO_PAD
                    .encode(serde_json::to_vec_pretty(&new_receipt).unwrap()),
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .personal_audit_checkpoint_original(checkpoint.request_id)
                .await
                .unwrap(),
            checkpoint_original
        );
        assert_eq!(
            store
                .personal_audit_trust_original(approval.proposal_request_id)
                .await
                .unwrap(),
            original_transition
        );
        assert_eq!(
            cp::canonical(
                &store
                    .personal_audit_checkpoint_capture(&authority, checkpoint_input)
                    .await
                    .unwrap()
            )
            .unwrap(),
            cp::canonical(&checkpoint).unwrap()
        );
        assert_eq!(
            AuditAuthority::new_operator(epoch_keys, next)
                .unwrap()
                .public_key_sha256()
                .unwrap(),
            origin
        );
        assert_eq!(
            json_string(
                &store
                    .server_key_operation(import.request_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .receipt
                    .unwrap()
            )
            .unwrap(),
            original_receipt
        );
        store.close().await;
    }
}
