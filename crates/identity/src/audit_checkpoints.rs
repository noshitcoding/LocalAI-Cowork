//! Independent audit witnesses. Frozen requests and original signed receipts
//! survive response loss; no renderer-supplied file establishes its own trust.
use crate::{
    IdentityError, Result,
    audit::{self, AuditAuthority},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_contracts::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Any, Row, Transaction};
use uuid::Uuid;

const MAX_REQUEST: usize = 64 * 1024 * 1024;
const MAX_CHECKPOINT: usize = 16 * 1024;
const MAX_REQUESTS: i64 = 512;
pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| IdentityError::Corrupt)
}
pub fn encode_original_bytes(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn valid_scope(scope: &str) -> bool {
    scope == "personal"
        || ["account:", "organization:"].iter().any(|p| {
            scope.strip_prefix(p).is_some_and(|v| {
                Uuid::parse_str(v).is_ok_and(|id| !id.is_nil() && id.to_string() == v)
            })
        })
}
pub(crate) async fn fence(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<()> {
    if !valid_scope(scope) {
        return Err(IdentityError::Validation);
    }
    sqlx::query("INSERT INTO audit_heads(scope,chain_id,sequence,head_sha256) VALUES($1,$2,0,$3) ON CONFLICT(scope) DO NOTHING").bind(scope).bind(Uuid::new_v4().to_string()).bind("0".repeat(64)).execute(&mut **tx).await?;
    sqlx::query("UPDATE audit_heads SET sequence=sequence WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn decode<T: DeserializeOwned>(row: &sqlx::any::AnyRow) -> Result<T> {
    let raw: String = row.try_get("payload")?;
    if raw.len() > MAX_REQUEST
        || audit::hash(raw.as_bytes()) != row.try_get::<String, _>("payload_sha256")?
    {
        return Err(IdentityError::Corrupt);
    }
    serde_json::from_str(&raw).map_err(|_| IdentityError::Corrupt)
}
pub fn verify_binding(
    binding: &AuditWitnessBinding,
    origin_pin: &str,
    witness_pin: &str,
) -> Result<()> {
    if binding.format != "cowork.audit.witness-binding.v1"
        || binding.binding_id.is_nil()
        || binding.origin_id.is_nil()
        || !valid_scope(&binding.scope)
        || binding.configured_at <= 0
        || !digest(origin_pin)
        || !digest(witness_pin)
        || origin_pin == witness_pin
        || binding.origin_public_key_sha256 != origin_pin
        || binding.witness_public_key_sha256 != witness_pin
    {
        return Err(IdentityError::Validation);
    }
    let mut unsigned = binding.clone();
    unsigned.public_key.clear();
    unsigned.signature.clear();
    audit::verify_metadata(
        "cowork:audit:witness-binding:v1",
        &binding.public_key,
        origin_pin,
        &binding.signature,
        &unsigned,
    )
}
pub fn verify_request(
    request: &AuditCheckpointRequest,
    origin_pin: &str,
    witness_pin: &str,
) -> Result<()> {
    verify_binding(&request.binding, origin_pin, witness_pin)?;
    if request.format != "cowork.audit.checkpoint-request.v1"
        || request.request_id.is_nil()
        || request.captured_at <= 0
        || request.public_key_sha256 != origin_pin
        || request
            .previous_checkpoint_sha256
            .as_ref()
            .is_some_and(|v| !digest(v))
    {
        return Err(IdentityError::Validation);
    }
    audit::verify_bundle(&request.archive, origin_pin)?;
    let first = &request.archive.pages[0].page;
    if first.scope != request.binding.scope
        || first.after_sequence != 0
        || !request
            .archive
            .pages
            .last()
            .is_some_and(|p| p.page.complete)
        || canonical(request)?.len() > MAX_REQUEST
    {
        return Err(IdentityError::Validation);
    }
    let mut unsigned = request.clone();
    unsigned.public_key.clear();
    unsigned.public_key_sha256.clear();
    unsigned.signature.clear();
    audit::verify_metadata(
        "cowork:audit:checkpoint-request:v1",
        &request.public_key,
        origin_pin,
        &request.signature,
        &unsigned,
    )
}
pub fn verify_checkpoint(value: &SignedAuditCheckpoint, witness_pin: &str) -> Result<()> {
    if value.format != "cowork.audit.independent-checkpoint.v1"
        || value.request_id.is_nil()
        || value.binding_id.is_nil()
        || value.origin_id.is_nil()
        || value.chain_id.is_nil()
        || !valid_scope(&value.scope)
        || value.head_sequence < 0
        || value.witness_sequence <= 0
        || value.witnessed_at <= 0
        || !digest(&value.head_sha256)
        || !digest(&value.candidate_sha256)
        || !digest(&value.origin_public_key_sha256)
        || !digest(witness_pin)
        || value.public_key_sha256 != witness_pin
        || value
            .previous_checkpoint_sha256
            .as_ref()
            .is_some_and(|v| !digest(v))
        || (value.witness_sequence == 1) != value.previous_checkpoint_sha256.is_none()
        || (value.head_sequence == 0 && value.head_sha256 != "0".repeat(64))
    {
        return Err(IdentityError::Validation);
    }
    let mut unsigned = value.clone();
    unsigned.public_key.clear();
    unsigned.public_key_sha256.clear();
    unsigned.signature.clear();
    audit::verify_metadata(
        "cowork:audit:independent-checkpoint:v1",
        &value.public_key,
        witness_pin,
        &value.signature,
        &unsigned,
    )
}
pub fn verify_checkpoint_request_binding(
    receipt: &SignedAuditCheckpoint,
    request: &AuditCheckpointRequest,
) -> Result<()> {
    verify_request(
        request,
        &request.binding.origin_public_key_sha256,
        &request.binding.witness_public_key_sha256,
    )?;
    verify_checkpoint(receipt, &request.binding.witness_public_key_sha256)?;
    let head = &request.archive.pages[0].page;
    if receipt.request_id != request.request_id
        || receipt.binding_id != request.binding.binding_id
        || receipt.origin_id != request.binding.origin_id
        || receipt.scope != request.binding.scope
        || receipt.origin_public_key_sha256 != request.binding.origin_public_key_sha256
        || receipt.candidate_sha256 != audit::hash(&canonical(request)?)
        || receipt.chain_id != head.chain_id
        || receipt.head_sequence != head.through_sequence
        || receipt.head_sha256 != head.through_sha256
        || receipt.previous_checkpoint_sha256 != request.previous_checkpoint_sha256
    {
        return Err(IdentityError::Validation);
    }
    Ok(())
}
/// The caller must retain this original receipt outside the product database.
/// The independent witness checks continuity against its own latest receipt.
pub fn witness(
    request: &AuditCheckpointRequest,
    previous: Option<&SignedAuditCheckpoint>,
    origin_pin: &str,
    signer: &Ed25519KeyPair,
    witnessed_at: i64,
    permit_new_chain: Option<Uuid>,
) -> Result<SignedAuditCheckpoint> {
    witness_with_transition(
        request,
        previous,
        origin_pin,
        signer,
        witnessed_at,
        permit_new_chain,
        None,
    )
}
pub fn witness_with_transition(
    request: &AuditCheckpointRequest,
    previous: Option<&SignedAuditCheckpoint>,
    origin_pin: &str,
    signer: &Ed25519KeyPair,
    witnessed_at: i64,
    permit_new_chain: Option<Uuid>,
    trust_transition: Option<&SignedAuditTrustTransition>,
) -> Result<SignedAuditCheckpoint> {
    let witness_pin = audit::hash(signer.public_key().as_ref());
    verify_request(request, origin_pin, &witness_pin)?;
    let head = &request.archive.pages[0].page;
    let mut transition = false;
    let sequence = if let Some(previous) = previous {
        if previous.binding_id != request.binding.binding_id {
            let proof = trust_transition.ok_or(IdentityError::Conflict)?;
            let a = &proof.proposal.previous_binding;
            let b = &proof.proposal.next_binding;
            crate::audit_trust::verify_transition(
                proof,
                &a.origin_public_key_sha256,
                origin_pin,
                &a.witness_public_key_sha256,
                &witness_pin,
            )?;
            if canonical(&proof.proposal.previous_checkpoint)? != canonical(previous)?
                || canonical(b)? != canonical(&request.binding)?
                || witnessed_at < proof.accepted_at
            {
                return Err(IdentityError::Conflict);
            }
        } else {
            verify_checkpoint(previous, &witness_pin)?;
        }
        if previous.origin_id != request.binding.origin_id
            || previous.scope != request.binding.scope
            || request.previous_checkpoint_sha256.as_deref()
                != Some(audit::hash(&canonical(previous)?).as_str())
        {
            return Err(IdentityError::Conflict);
        }
        if previous.chain_id != head.chain_id {
            if permit_new_chain != Some(head.chain_id) {
                return Err(IdentityError::Conflict);
            }
            transition = true;
        } else {
            if head.through_sequence < previous.head_sequence {
                return Err(IdentityError::Conflict);
            }
            let unchanged = previous.head_sequence == 0 && previous.head_sha256 == "0".repeat(64)
                || request
                    .archive
                    .pages
                    .iter()
                    .flat_map(|p| &p.page.events)
                    .any(|e| {
                        e.sequence == previous.head_sequence && e.sha256 == previous.head_sha256
                    });
            if !unchanged {
                return Err(IdentityError::Conflict);
            }
        }
        previous
            .witness_sequence
            .checked_add(1)
            .ok_or(IdentityError::QuotaExceeded)?
    } else {
        if request.previous_checkpoint_sha256.is_some() || permit_new_chain.is_some() {
            return Err(IdentityError::Conflict);
        }
        1
    };
    let mut receipt = SignedAuditCheckpoint {
        format: "cowork.audit.independent-checkpoint.v1".into(),
        request_id: request.request_id,
        binding_id: request.binding.binding_id,
        origin_id: request.binding.origin_id,
        scope: request.binding.scope.clone(),
        candidate_sha256: audit::hash(&canonical(request)?),
        origin_public_key_sha256: origin_pin.into(),
        chain_id: head.chain_id,
        head_sequence: head.through_sequence,
        head_sha256: head.through_sha256.clone(),
        witnessed_at,
        witness_sequence: sequence,
        previous_checkpoint_sha256: request.previous_checkpoint_sha256.clone(),
        operator_chain_transition: transition,
        public_key: String::new(),
        public_key_sha256: String::new(),
        signature: String::new(),
    };
    let public = URL_SAFE_NO_PAD.encode(signer.public_key().as_ref());
    let message = canonical(&(
        "cowork:audit:independent-checkpoint:v1",
        &public,
        &witness_pin,
        &receipt,
    ))?;
    receipt.signature = URL_SAFE_NO_PAD.encode(signer.sign(&message).as_ref());
    receipt.public_key = public;
    receipt.public_key_sha256 = witness_pin;
    verify_checkpoint_request_binding(&receipt, request)?;
    Ok(receipt)
}
pub(crate) async fn binding(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
) -> Result<Option<AuditWitnessBinding>> {
    let Some(row) =
        sqlx::query("SELECT payload,payload_sha256 FROM audit_witness_bindings WHERE scope=$1")
            .bind(scope)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Ok(None);
    };
    let value: AuditWitnessBinding = decode(&row)?;
    verify_binding(
        &value,
        &value.origin_public_key_sha256,
        &value.witness_public_key_sha256,
    )
    .map_err(|_| IdentityError::Corrupt)?;
    if value.scope != scope {
        return Err(IdentityError::Corrupt);
    }
    Ok(Some(value))
}
pub(crate) async fn latest(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    binding: Option<&AuditWitnessBinding>,
) -> Result<Option<(SignedAuditCheckpoint, String)>> {
    let Some(row) = sqlx::query("SELECT i.payload,i.payload_sha256,h.checkpoint_sha256 FROM audit_checkpoint_heads h JOIN audit_checkpoint_imports i ON i.scope=h.scope AND i.candidate_request_id=h.candidate_request_id WHERE h.scope=$1").bind(scope).fetch_optional(&mut **tx).await? else {return Ok(None)};
    let value: SignedAuditCheckpoint = decode(&row)?;
    binding.ok_or(IdentityError::Corrupt)?;
    let pin = binding_for(tx, scope, value.binding_id)
        .await?
        .ok_or(IdentityError::Corrupt)?;
    verify_checkpoint(&value, &pin.witness_public_key_sha256)
        .map_err(|_| IdentityError::Corrupt)?;
    let hash = audit::hash(&canonical(&value)?);
    if hash != row.try_get::<String, _>("checkpoint_sha256")?
        || value.scope != scope
        || value.binding_id != pin.binding_id
        || value.origin_id != pin.origin_id
        || value.origin_public_key_sha256 != pin.origin_public_key_sha256
    {
        return Err(IdentityError::Corrupt);
    }
    Ok(Some((value, hash)))
}
/// Private installation code holds all scoped heads before this metadata read.
/// HTTP callers continue to authorize through IdentityStore, never this helper.
pub async fn operator_latest(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    pin: &AuditWitnessBinding,
) -> Result<Option<(SignedAuditCheckpoint, String)>> {
    latest(tx, scope, Some(pin)).await
}
pub(crate) async fn binding_for(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    id: Uuid,
) -> Result<Option<AuditWitnessBinding>> {
    if let Some(p) = binding(tx, scope).await?
        && p.binding_id == id
    {
        return Ok(Some(p));
    }
    let Some(row)=sqlx::query("SELECT payload,payload_sha256 FROM audit_witness_binding_history WHERE scope=$1 AND binding_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await? else{return Ok(None)};
    let p: AuditWitnessBinding = decode(&row)?;
    verify_binding(
        &p,
        &p.origin_public_key_sha256,
        &p.witness_public_key_sha256,
    )?;
    if p.binding_id != id || p.scope != scope {
        return Err(IdentityError::Corrupt);
    }
    Ok(Some(p))
}
pub async fn configure(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    actor: Option<Uuid>,
    authority: &AuditAuthority,
    input: ConfigureAuditWitness,
    observed_at: i64,
) -> Result<AuditWitnessBinding> {
    if input.request_id.is_nil()
        || input.origin_id.is_nil()
        || !input.independently_confirmed
        || !digest(&input.origin_public_key_sha256)
        || !digest(&input.witness_public_key_sha256)
        || input.origin_public_key_sha256 == input.witness_public_key_sha256
    {
        return Err(IdentityError::Validation);
    }
    fence(tx, scope).await?;
    let request_hash = audit::hash(&canonical(&input)?);
    if let Some(row)=sqlx::query("SELECT request_sha256,payload,payload_sha256 FROM audit_witness_binding_history WHERE scope=$1 AND request_id=$2").bind(scope).bind(input.request_id.to_string()).fetch_optional(&mut **tx).await?{if row.try_get::<String,_>("request_sha256")?!=request_hash{return Err(IdentityError::Conflict);}let original:AuditWitnessBinding=decode(&row)?;verify_binding(&original,&input.origin_public_key_sha256,&input.witness_public_key_sha256)?;return Ok(original);}
    if let Some(row) = sqlx::query("SELECT * FROM audit_witness_bindings WHERE scope=$1")
        .bind(scope)
        .fetch_optional(&mut **tx)
        .await?
    {
        if row.try_get::<String, _>("request_id")? != input.request_id.to_string()
            || row.try_get::<String, _>("request_sha256")? != request_hash
        {
            return Err(IdentityError::Conflict);
        }
        return binding(tx, scope).await?.ok_or(IdentityError::Corrupt);
    }
    if authority.public_key_sha256()? != input.origin_public_key_sha256 {
        return Err(IdentityError::Conflict);
    }
    crate::audit_origin::admission(tx, authority).await?;
    let mut value = AuditWitnessBinding {
        format: "cowork.audit.witness-binding.v1".into(),
        binding_id: input.request_id,
        origin_id: input.origin_id,
        scope: scope.into(),
        origin_public_key_sha256: input.origin_public_key_sha256,
        witness_public_key_sha256: input.witness_public_key_sha256,
        configured_at: observed_at,
        actor_id: actor,
        public_key: String::new(),
        signature: String::new(),
    };
    let (public, _, signature) =
        authority.sign_metadata("cowork:audit:witness-binding:v1", &value)?;
    value.public_key = public;
    value.signature = signature;
    let raw = canonical(&value)?;
    sqlx::query("INSERT INTO audit_witness_bindings(scope,request_id,request_sha256,payload,payload_sha256) VALUES($1,$2,$3,$4,$5)").bind(scope).bind(input.request_id.to_string()).bind(request_hash).bind(String::from_utf8(raw.clone()).map_err(|_|IdentityError::Corrupt)?).bind(audit::hash(&raw)).execute(&mut **tx).await?;
    Ok(value)
}
pub async fn capture(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    authority: &AuditAuthority,
    input: CaptureAuditCheckpoint,
    observed_at: i64,
) -> Result<AuditCheckpointRequest> {
    if input.request_id.is_nil()
        || input.binding_id.is_nil()
        || input
            .previous_checkpoint_sha256
            .as_ref()
            .is_some_and(|s| !digest(s))
    {
        return Err(IdentityError::Validation);
    }
    fence(tx, scope).await?;
    let pin = binding(tx, scope).await?.ok_or(IdentityError::NotFound)?;
    let request_hash = audit::hash(&canonical(&input)?);
    if let Some(row) =
        sqlx::query("SELECT * FROM audit_checkpoint_requests WHERE scope=$1 AND request_id=$2")
            .bind(scope)
            .bind(input.request_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
    {
        if row.try_get::<String, _>("request_sha256")? != request_hash {
            return Err(IdentityError::Conflict);
        }
        let value: AuditCheckpointRequest = decode(&row)?;
        let original_pin = binding_for(tx, scope, value.binding.binding_id)
            .await?
            .ok_or(IdentityError::Corrupt)?;
        if canonical(&original_pin)? != canonical(&value.binding)? {
            return Err(IdentityError::Corrupt);
        }
        verify_request(
            &value,
            &original_pin.origin_public_key_sha256,
            &original_pin.witness_public_key_sha256,
        )
        .map_err(|_| IdentityError::Corrupt)?;
        if value.request_id != input.request_id
            || value.binding.binding_id != input.binding_id
            || value.previous_checkpoint_sha256 != input.previous_checkpoint_sha256
        {
            return Err(IdentityError::Corrupt);
        }
        return Ok(value);
    }
    let previous = latest(tx, scope, Some(&pin)).await?;
    if input.binding_id != pin.binding_id
        || previous.as_ref().map(|v| v.1.clone()) != input.previous_checkpoint_sha256
        || authority.public_key_sha256()? != pin.origin_public_key_sha256
    {
        return Err(IdentityError::Conflict);
    }
    if crate::audit_trust::pending(tx, scope).await?.is_some() {
        return Err(IdentityError::Conflict);
    }
    crate::audit_origin::admission(tx, authority).await?;
    let count=sqlx::query("SELECT COUNT(*) AS total,COALESCE(SUM(LENGTH(payload)),0) AS bytes,COALESCE(SUM(CASE WHEN completed=0 THEN 1 ELSE 0 END),0) AS pending FROM audit_checkpoint_requests WHERE scope=$1").bind(scope).fetch_one(&mut **tx).await?;
    if count.try_get::<i64, _>("pending")? != 0 {
        return Err(IdentityError::Conflict);
    }
    if count.try_get::<i64, _>("total")? >= MAX_REQUESTS {
        return Err(IdentityError::QuotaExceeded);
    }
    let check = audit::verification(tx, scope).await?;
    if !check.valid {
        return Err(IdentityError::Corrupt);
    }
    if !check.complete {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut pages = Vec::new();
    let mut after = 0;
    loop {
        let page = audit::page(tx, scope, after, Some(check.head_sequence), 500).await?;
        after = page.next_sequence;
        let complete = page.complete;
        pages.push(authority.sign(Uuid::new_v4(), observed_at, page)?);
        if complete {
            break;
        }
        if pages.len() >= 200 {
            return Err(IdentityError::QuotaExceeded);
        }
    }
    let mut value = AuditCheckpointRequest {
        format: "cowork.audit.checkpoint-request.v1".into(),
        request_id: input.request_id,
        binding: pin,
        captured_at: observed_at,
        previous_checkpoint_sha256: input.previous_checkpoint_sha256,
        archive: AuditArchiveBundle {
            format: "cowork.audit.bundle.v1".into(),
            pages,
        },
        public_key: String::new(),
        public_key_sha256: String::new(),
        signature: String::new(),
    };
    let (public, pin, signature) =
        authority.sign_metadata("cowork:audit:checkpoint-request:v1", &value)?;
    value.public_key = public;
    value.public_key_sha256 = pin;
    value.signature = signature;
    let raw = canonical(&value)?;
    if raw.len() > MAX_REQUEST
        || count
            .try_get::<i64, _>("bytes")?
            .saturating_add(raw.len() as i64)
            > 512 * 1024 * 1024
    {
        return Err(IdentityError::QuotaExceeded);
    }
    verify_request(
        &value,
        &value.binding.origin_public_key_sha256,
        &value.binding.witness_public_key_sha256,
    )?;
    sqlx::query("INSERT INTO audit_checkpoint_requests(scope,request_id,request_sha256,payload,payload_sha256,completed) VALUES($1,$2,$3,$4,$5,0)").bind(scope).bind(value.request_id.to_string()).bind(request_hash).bind(String::from_utf8(raw.clone()).map_err(|_|IdentityError::Corrupt)?).bind(audit::hash(&raw)).execute(&mut **tx).await?;
    Ok(value)
}
pub async fn request(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    id: Uuid,
) -> Result<AuditCheckpointRequest> {
    fence(tx, scope).await?;
    let row=sqlx::query("SELECT payload,payload_sha256 FROM audit_checkpoint_requests WHERE scope=$1 AND request_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let value: AuditCheckpointRequest = decode(&row)?;
    let pin = binding_for(tx, scope, value.binding.binding_id)
        .await?
        .ok_or(IdentityError::Corrupt)?;
    if canonical(&pin)? != canonical(&value.binding)? {
        return Err(IdentityError::Corrupt);
    }
    verify_request(
        &value,
        &pin.origin_public_key_sha256,
        &pin.witness_public_key_sha256,
    )
    .map_err(|_| IdentityError::Corrupt)?;
    if value.request_id != id || value.binding.scope != scope {
        return Err(IdentityError::Corrupt);
    }
    Ok(value)
}
pub async fn import(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: ImportAuditCheckpoint,
) -> Result<SignedAuditCheckpoint> {
    if input.request_id.is_nil()
        || input.candidate_request_id.is_nil()
        || input.checkpoint_bytes_base64.len() > MAX_CHECKPOINT * 4 / 3 + 8
    {
        return Err(IdentityError::Validation);
    }
    fence(tx, scope).await?;
    let request_hash = audit::hash(&canonical(&input)?);
    let original = URL_SAFE_NO_PAD
        .decode(&input.checkpoint_bytes_base64)
        .map_err(|_| IdentityError::Validation)?;
    if original.len() > MAX_CHECKPOINT {
        return Err(IdentityError::Validation);
    }
    let receipt: SignedAuditCheckpoint =
        serde_json::from_slice(&original).map_err(|_| IdentityError::Validation)?;
    let candidate = request(tx, scope, input.candidate_request_id).await?;
    verify_checkpoint_request_binding(&receipt, &candidate)?;
    if let Some(row) =
        sqlx::query("SELECT * FROM audit_checkpoint_imports WHERE scope=$1 AND request_id=$2")
            .bind(scope)
            .bind(input.request_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
    {
        if row.try_get::<String, _>("request_sha256")? != request_hash {
            return Err(IdentityError::Conflict);
        }
        let value: SignedAuditCheckpoint = decode(&row)?;
        verify_checkpoint_request_binding(&value, &candidate)
            .map_err(|_| IdentityError::Corrupt)?;
        if row.try_get::<String, _>("original_sha256")? != audit::hash(&original)
            || row.try_get::<String, _>("original_base64")? != input.checkpoint_bytes_base64
        {
            return Err(IdentityError::Corrupt);
        }
        return Ok(value);
    }
    let pin = binding(tx, scope).await?.ok_or(IdentityError::NotFound)?;
    let previous = latest(tx, scope, Some(&pin)).await?;
    if canonical(&pin)? != canonical(&candidate.binding)?
        || crate::audit_trust::pending(tx, scope).await?.is_some()
    {
        return Err(IdentityError::Conflict);
    }
    if let Some((old, _)) = &previous
        && old.binding_id != receipt.binding_id
    {
        let proof = crate::audit_trust::for_binding(tx, scope, receipt.binding_id)
            .await?
            .ok_or(IdentityError::Conflict)?;
        if canonical(&proof.proposal.previous_checkpoint)? != canonical(old)?
            || canonical(&proof.proposal.next_binding)? != canonical(&candidate.binding)?
            || receipt.witnessed_at < proof.accepted_at
        {
            return Err(IdentityError::Conflict);
        }
    }
    if receipt.previous_checkpoint_sha256 != previous.as_ref().map(|v| v.1.clone())
        || receipt.witness_sequence != previous.as_ref().map_or(1, |v| v.0.witness_sequence + 1)
    {
        return Err(IdentityError::Conflict);
    }
    if let Some((previous, _)) = &previous
        && (previous.chain_id == receipt.chain_id
            && (receipt.head_sequence < previous.head_sequence
                || receipt.operator_chain_transition)
            || previous.chain_id != receipt.chain_id && !receipt.operator_chain_transition)
    {
        return Err(IdentityError::Validation);
    }
    let raw = canonical(&receipt)?;
    let sha = audit::hash(&raw);
    sqlx::query("INSERT INTO audit_checkpoint_imports(scope,request_id,request_sha256,candidate_request_id,original_base64,original_sha256,payload,payload_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(scope).bind(input.request_id.to_string()).bind(request_hash).bind(input.candidate_request_id.to_string()).bind(input.checkpoint_bytes_base64).bind(audit::hash(&original)).bind(String::from_utf8(raw).map_err(|_|IdentityError::Corrupt)?).bind(&sha).execute(&mut **tx).await?;
    sqlx::query("UPDATE audit_checkpoint_requests SET completed=1 WHERE scope=$1 AND request_id=$2 AND completed=0").bind(scope).bind(input.candidate_request_id.to_string()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO audit_checkpoint_heads(scope,checkpoint_sha256,candidate_request_id) VALUES($1,$2,$3) ON CONFLICT(scope) DO UPDATE SET checkpoint_sha256=excluded.checkpoint_sha256,candidate_request_id=excluded.candidate_request_id").bind(scope).bind(sha).bind(input.candidate_request_id.to_string()).execute(&mut **tx).await?;
    Ok(receipt)
}
pub async fn status(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<AuditCheckpointStatus> {
    fence(tx, scope).await?;
    let pin = binding(tx, scope).await?;
    let current = latest(tx, scope, pin.as_ref()).await?;
    let pending: Option<String> = sqlx::query_scalar(
        "SELECT request_id FROM audit_checkpoint_requests WHERE scope=$1 AND completed=0",
    )
    .bind(scope)
    .fetch_optional(&mut **tx)
    .await?;
    let head = sqlx::query("SELECT chain_id,sequence,head_sha256 FROM audit_heads WHERE scope=$1")
        .bind(scope)
        .fetch_one(&mut **tx)
        .await?;
    let matches = current.as_ref().is_some_and(|(r, _)| {
        head.try_get::<String, _>("chain_id").ok().as_deref()
            == Some(r.chain_id.to_string().as_str())
    });
    let prefix = if let Some((r, _)) = &current {
        if matches && head.try_get::<i64, _>("sequence")? >= r.head_sequence {
            let page = audit::page(tx, scope, r.head_sequence, Some(r.head_sequence), 1).await;
            page.is_ok_and(|p| p.previous_sha256 == r.head_sha256)
        } else {
            false
        }
    } else {
        false
    };
    let prefix = prefix
        && audit::verification(tx, scope)
            .await
            .is_ok_and(|v| v.valid && v.complete);
    Ok(AuditCheckpointStatus {
        binding: pin,
        latest: current.as_ref().map(|v| v.0.clone()),
        latest_sha256: current.map(|v| v.1),
        pending_request_id: pending
            .map(|id| Uuid::parse_str(&id).map_err(|_| IdentityError::Corrupt))
            .transpose()?,
        current_chain_matches: matches,
        current_prefix_matches: prefix,
        trust_transitions: crate::audit_trust::history(tx, scope).await?,
        pending_transition_request_id: crate::audit_trust::pending(tx, scope).await?,
    })
}
pub async fn history(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    after: i64,
    limit: i64,
) -> Result<Vec<SignedAuditCheckpoint>> {
    if after < 0 || !(1..=100).contains(&limit) {
        return Err(IdentityError::Validation);
    }
    fence(tx, scope).await?;
    let rows=sqlx::query("SELECT payload,payload_sha256 FROM audit_checkpoint_imports WHERE scope=$1 ORDER BY candidate_request_id").bind(scope).fetch_all(&mut **tx).await?;
    if rows.len() > MAX_REQUESTS as usize {
        return Err(IdentityError::Corrupt);
    }
    let mut values = Vec::new();
    for row in rows {
        let v: SignedAuditCheckpoint = decode(&row)?;
        let pin = binding_for(tx, scope, v.binding_id)
            .await?
            .ok_or(IdentityError::Corrupt)?;
        verify_checkpoint(&v, &pin.witness_public_key_sha256)
            .map_err(|_| IdentityError::Corrupt)?;
        if v.scope != scope {
            return Err(IdentityError::Corrupt);
        }
        values.push(v);
    }
    values.sort_by_key(|v| v.witness_sequence);
    Ok(values
        .into_iter()
        .filter(|v| v.witness_sequence > after)
        .take(limit as usize)
        .collect())
}
pub async fn original(tx: &mut Transaction<'_, Any>, scope: &str, id: Uuid) -> Result<String> {
    fence(tx, scope).await?;
    let row=sqlx::query("SELECT original_base64,original_sha256 FROM audit_checkpoint_imports WHERE scope=$1 AND candidate_request_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let raw: String = row.try_get("original_base64")?;
    if raw.len() > MAX_CHECKPOINT * 4 / 3 + 8 {
        return Err(IdentityError::Corrupt);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(&raw)
        .map_err(|_| IdentityError::Corrupt)?;
    if audit::hash(&bytes) != row.try_get::<String, _>("original_sha256")? {
        return Err(IdentityError::Corrupt);
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_secrets::{KeyId, KeyStore};
    use std::sync::Arc;
    use zeroize::Zeroizing;
    struct Keys;
    impl KeyStore for Keys {
        fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
            Ok(Zeroizing::new([71; 32]))
        }
    }
    #[tokio::test]
    async fn independent_witness_rejects_a_resigned_database_fork_and_retains_original_operations()
    {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("CREATE TABLE identity_audit(id TEXT,sequence BIGINT)")
            .execute(&mut *tx)
            .await
            .unwrap();
        for statement in [
            include_str!("audit_schema.sql"),
            include_str!("audit_recovery_schema.sql"),
            include_str!("audit_checkpoint_schema.sql"),
            include_str!("audit_trust_schema.sql"),
        ]
        .join("\n")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        {
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
        }
        let authority = AuditAuthority::new(
            Arc::new(Keys),
            KeyId {
                instance: Uuid::from_u128(121),
                key: Uuid::from_u128(122),
            },
        )
        .unwrap();
        let origin = authority.public_key_sha256().unwrap();
        let external = Ed25519KeyPair::from_seed_unchecked(&[72; 32]).unwrap();
        let external_pin = audit::hash(external.public_key().as_ref());
        let configured = ConfigureAuditWitness {
            request_id: Uuid::from_u128(123),
            origin_id: Uuid::from_u128(124),
            origin_public_key_sha256: origin.clone(),
            witness_public_key_sha256: external_pin.clone(),
            independently_confirmed: true,
        };
        let binding = configure(&mut tx, "personal", None, &authority, configured.clone(), 1)
            .await
            .unwrap();
        assert_eq!(
            canonical(&binding).unwrap(),
            canonical(
                &configure(&mut tx, "personal", None, &authority, configured, 2)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        audit::append(
            &mut tx,
            "personal",
            audit::MetadataEvent {
                event_id: "first",
                action: "run_created",
                actor_id: None,
                target_id: None,
                observed_at: 2,
                origin: AuditOrigin::NativeV2,
                source_sha256: None,
            },
        )
        .await
        .unwrap();
        let input = CaptureAuditCheckpoint {
            request_id: Uuid::from_u128(125),
            binding_id: binding.binding_id,
            previous_checkpoint_sha256: None,
        };
        let candidate = capture(&mut tx, "personal", &authority, input.clone(), 3)
            .await
            .unwrap();
        audit::append(
            &mut tx,
            "personal",
            audit::MetadataEvent {
                event_id: "second",
                action: "run_completed",
                actor_id: None,
                target_id: None,
                observed_at: 4,
                origin: AuditOrigin::NativeV2,
                source_sha256: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            canonical(&candidate).unwrap(),
            canonical(
                &capture(&mut tx, "personal", &authority, input, 5)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        let receipt = witness(&candidate, None, &origin, &external, 6, None).unwrap();
        assert!(verify_checkpoint(&receipt, &origin).is_err());
        let original = serde_json::to_vec_pretty(&receipt).unwrap();
        let importing = ImportAuditCheckpoint {
            request_id: Uuid::from_u128(126),
            candidate_request_id: candidate.request_id,
            checkpoint_bytes_base64: URL_SAFE_NO_PAD.encode(&original),
        };
        let accepted = import(&mut tx, "personal", importing.clone())
            .await
            .unwrap();
        assert_eq!(canonical(&receipt).unwrap(), canonical(&accepted).unwrap());
        assert_eq!(
            original,
            URL_SAFE_NO_PAD
                .decode(
                    super::original(&mut tx, "personal", candidate.request_id)
                        .await
                        .unwrap()
                )
                .unwrap()
        );
        assert_eq!(
            canonical(&accepted).unwrap(),
            canonical(&import(&mut tx, "personal", importing).await.unwrap()).unwrap()
        );
        let status = status(&mut tx, "personal").await.unwrap();
        assert!(status.current_chain_matches && status.current_prefix_matches);
        let next = capture(
            &mut tx,
            "personal",
            &authority,
            CaptureAuditCheckpoint {
                request_id: Uuid::from_u128(127),
                binding_id: binding.binding_id,
                previous_checkpoint_sha256: status.latest_sha256,
            },
            7,
        )
        .await
        .unwrap();
        let next_receipt = witness(&next, Some(&receipt), &origin, &external, 8, None).unwrap();
        assert_eq!(next_receipt.witness_sequence, 2);
        // A database attacker with the origin signer can produce a valid new
        // chain proof and signatures. The separately retained old witness head
        // still refuses its changed prefix.
        let mut fork = next.clone();
        fork.request_id = Uuid::from_u128(128);
        let page = &mut fork.archive.pages[0].page;
        let mut previous = "0".repeat(64);
        for (i, event) in page.events.iter_mut().enumerate() {
            if i == 0 {
                event.action = "run_failed".into();
            }
            event.previous_sha256 = previous;
            event.sha256 = audit::hash(
                &canonical(&(
                    "cowork:audit:event:v1",
                    &page.scope,
                    page.chain_id,
                    event.sequence,
                    &event.event_id,
                    &event.action,
                    &event.actor_id,
                    &event.target_id,
                    event.observed_at,
                    &event.origin,
                    &event.source_sha256,
                    &event.previous_sha256,
                ))
                .unwrap(),
            );
            previous = event.sha256.clone();
        }
        page.through_sha256 = previous;
        let page = page.clone();
        fork.archive.pages[0] = authority.sign(Uuid::from_u128(129), 7, page).unwrap();
        fork.public_key.clear();
        fork.public_key_sha256.clear();
        fork.signature.clear();
        let (public, pin, signature) = authority
            .sign_metadata("cowork:audit:checkpoint-request:v1", &fork)
            .unwrap();
        fork.public_key = public;
        fork.public_key_sha256 = pin;
        fork.signature = signature;
        verify_request(&fork, &origin, &external_pin).unwrap();
        assert!(matches!(
            witness(&fork, Some(&receipt), &origin, &external, 8, None),
            Err(IdentityError::Conflict)
        ));
        assert!(matches!(
            witness(&candidate, Some(&receipt), &origin, &external, 8, None),
            Err(IdentityError::Conflict)
        ));
        import(
            &mut tx,
            "personal",
            ImportAuditCheckpoint {
                request_id: Uuid::from_u128(130),
                candidate_request_id: next.request_id,
                checkpoint_bytes_base64: URL_SAFE_NO_PAD.encode(canonical(&next_receipt).unwrap()),
            },
        )
        .await
        .unwrap();
        let recipient = Ed25519KeyPair::from_seed_unchecked(&[73; 32]).unwrap();
        let recipient_pin = audit::hash(recipient.public_key().as_ref());
        let intent = PrepareAuditTrustTransition {
            request_id: Uuid::from_u128(131),
            binding_id: binding.binding_id,
            previous_checkpoint_sha256: audit::hash(&canonical(&next_receipt).unwrap()),
            new_witness_public_key_sha256: recipient_pin.clone(),
            independently_confirmed: true,
        };
        let proposal =
            crate::audit_trust::prepare(&mut tx, "personal", None, &authority, intent.clone(), 9)
                .await
                .unwrap();
        assert_eq!(
            canonical(&proposal).unwrap(),
            canonical(
                &crate::audit_trust::prepare(&mut tx, "personal", None, &authority, intent, 99)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        assert!(
            capture(
                &mut tx,
                "personal",
                &authority,
                CaptureAuditCheckpoint {
                    request_id: Uuid::from_u128(132),
                    binding_id: binding.binding_id,
                    previous_checkpoint_sha256: Some(audit::hash(
                        &canonical(&next_receipt).unwrap()
                    ))
                },
                10
            )
            .await
            .is_err()
        );
        let transition = crate::audit_trust::sign_transition(
            proposal.clone(),
            &origin,
            &origin,
            &external,
            &recipient,
            10,
        )
        .unwrap();
        let mut tampered = transition.clone();
        tampered.proposal.previous_checkpoint.head_sha256 = "b".repeat(64);
        assert!(
            crate::audit_trust::verify_transition(
                &tampered,
                &origin,
                &origin,
                &external_pin,
                &recipient_pin
            )
            .is_err()
        );
        let transition_bytes = serde_json::to_vec_pretty(&transition).unwrap();
        let accepting = ImportAuditTrustTransition {
            request_id: Uuid::from_u128(133),
            proposal_request_id: proposal.request_id,
            original_bytes_base64: URL_SAFE_NO_PAD.encode(&transition_bytes),
            independently_confirmed: true,
        };
        crate::audit_trust::import(&mut tx, "personal", accepting.clone())
            .await
            .unwrap();
        crate::audit_trust::import(&mut tx, "personal", accepting)
            .await
            .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(
                    crate::audit_trust::original(&mut tx, "personal", proposal.request_id)
                        .await
                        .unwrap()
                )
                .unwrap(),
            transition_bytes
        );
        assert_eq!(
            canonical(
                &request(&mut tx, "personal", candidate.request_id)
                    .await
                    .unwrap()
            )
            .unwrap(),
            canonical(&candidate).unwrap()
        );
        assert_eq!(history(&mut tx, "personal", 0, 100).await.unwrap().len(), 2);
        let continued = capture(
            &mut tx,
            "personal",
            &authority,
            CaptureAuditCheckpoint {
                request_id: Uuid::from_u128(134),
                binding_id: proposal.next_binding.binding_id,
                previous_checkpoint_sha256: Some(audit::hash(&canonical(&next_receipt).unwrap())),
            },
            11,
        )
        .await
        .unwrap();
        assert!(
            witness(
                &continued,
                Some(&next_receipt),
                &origin,
                &recipient,
                12,
                None
            )
            .is_err()
        );
        let continued_receipt = witness_with_transition(
            &continued,
            Some(&next_receipt),
            &origin,
            &recipient,
            12,
            None,
            Some(&transition),
        )
        .unwrap();
        assert_eq!(continued_receipt.witness_sequence, 3);
        import(
            &mut tx,
            "personal",
            ImportAuditCheckpoint {
                request_id: Uuid::from_u128(135),
                candidate_request_id: continued.request_id,
                checkpoint_bytes_base64: URL_SAFE_NO_PAD
                    .encode(canonical(&continued_receipt).unwrap()),
            },
        )
        .await
        .unwrap();
        assert_eq!(history(&mut tx, "personal", 0, 100).await.unwrap().len(), 3);
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(
                    super::original(&mut tx, "personal", candidate.request_id)
                        .await
                        .unwrap()
                )
                .unwrap(),
            original
        );
        tx.commit().await.unwrap();
        pool.close().await;
    }
}
