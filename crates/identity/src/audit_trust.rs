//! Explicit cross-signed trust changes. Original bindings, requests and imported
//! byte strings remain immutable; neither an embedded pin nor SQL sets trust.
use crate::{
    IdentityError, Result,
    audit::{self, AuditAuthority},
    audit_checkpoints as cp,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_contracts::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sqlx::{Any, Row, Transaction};
use uuid::Uuid;
const MAX_PROOF: usize = 65536;
const MAX_TRANSITIONS: i64 = 128;
fn unsigned_proposal(p: &AuditTrustTransitionProposal) -> AuditTrustTransitionProposal {
    let mut p = p.clone();
    p.previous_origin_signature.clear();
    p.next_origin_signature.clear();
    p
}
fn unsigned_receipt(p: &SignedAuditTrustTransition) -> SignedAuditTrustTransition {
    let mut p = p.clone();
    p.previous_witness_public_key.clear();
    p.next_witness_public_key.clear();
    p.previous_witness_signature.clear();
    p.next_witness_signature.clear();
    p
}
pub fn verify_proposal(
    p: &AuditTrustTransitionProposal,
    old_origin: &str,
    new_origin: &str,
    old_witness: &str,
    new_witness: &str,
) -> Result<()> {
    cp::verify_binding(&p.previous_binding, old_origin, old_witness)?;
    cp::verify_binding(&p.next_binding, new_origin, new_witness)?;
    cp::verify_checkpoint(&p.previous_checkpoint, old_witness)?;
    let a = &p.previous_binding;
    let b = &p.next_binding;
    let h = &p.previous_checkpoint;
    if p.format != "cowork.audit.trust-proposal.v1"
        || p.request_id.is_nil()
        || p.request_id != b.binding_id
        || a.binding_id == b.binding_id
        || a.origin_id != b.origin_id
        || a.scope != b.scope
        || p.prepared_at <= 0
        || p.prepared_at < a.configured_at
        || p.prepared_at < h.witnessed_at
        || b.configured_at != p.prepared_at
        || h.binding_id != a.binding_id
        || h.origin_id != a.origin_id
        || h.scope != a.scope
        || h.origin_public_key_sha256 != old_origin
        || cp::canonical(p)?.len() > MAX_PROOF
        || (old_origin == new_origin && old_witness == new_witness)
    {
        return Err(IdentityError::Validation);
    }
    let unsigned = unsigned_proposal(p);
    audit::verify_metadata(
        "cowork:audit:trust-origin-transition:v1",
        &a.public_key,
        old_origin,
        &p.previous_origin_signature,
        &unsigned,
    )?;
    audit::verify_metadata(
        "cowork:audit:trust-origin-transition:v1",
        &b.public_key,
        new_origin,
        &p.next_origin_signature,
        &unsigned,
    )
}
pub fn verify_transition(
    p: &SignedAuditTrustTransition,
    old_origin: &str,
    new_origin: &str,
    old_witness: &str,
    new_witness: &str,
) -> Result<()> {
    verify_proposal(
        &p.proposal,
        old_origin,
        new_origin,
        old_witness,
        new_witness,
    )?;
    if p.format != "cowork.audit.trust-transition.v1"
        || p.accepted_at < p.proposal.prepared_at
        || cp::canonical(p)?.len() > MAX_PROOF
    {
        return Err(IdentityError::Validation);
    }
    let unsigned = unsigned_receipt(p);
    audit::verify_metadata(
        "cowork:audit:trust-witness-transition:v1",
        &p.previous_witness_public_key,
        old_witness,
        &p.previous_witness_signature,
        &unsigned,
    )?;
    audit::verify_metadata(
        "cowork:audit:trust-witness-transition:v1",
        &p.next_witness_public_key,
        new_witness,
        &p.next_witness_signature,
        &unsigned,
    )
}
/// Both signers are opened by explicit independent native operators. This pure
/// operation cannot create, replace or discover a key or modify a policy.
pub fn sign_transition(
    proposal: AuditTrustTransitionProposal,
    old_origin: &str,
    new_origin: &str,
    old: &Ed25519KeyPair,
    new: &Ed25519KeyPair,
    accepted_at: i64,
) -> Result<SignedAuditTrustTransition> {
    let old_pin = audit::hash(old.public_key().as_ref());
    let new_pin = audit::hash(new.public_key().as_ref());
    verify_proposal(&proposal, old_origin, new_origin, &old_pin, &new_pin)?;
    let mut p = SignedAuditTrustTransition {
        format: "cowork.audit.trust-transition.v1".into(),
        proposal,
        accepted_at,
        previous_witness_public_key: String::new(),
        next_witness_public_key: String::new(),
        previous_witness_signature: String::new(),
        next_witness_signature: String::new(),
    };
    let old_public = URL_SAFE_NO_PAD.encode(old.public_key().as_ref());
    let new_public = URL_SAFE_NO_PAD.encode(new.public_key().as_ref());
    p.previous_witness_signature = URL_SAFE_NO_PAD.encode(
        old.sign(&cp::canonical(&(
            "cowork:audit:trust-witness-transition:v1",
            &old_public,
            &old_pin,
            &unsigned_receipt(&p),
        ))?)
        .as_ref(),
    );
    p.next_witness_signature = URL_SAFE_NO_PAD.encode(
        new.sign(&cp::canonical(&(
            "cowork:audit:trust-witness-transition:v1",
            &new_public,
            &new_pin,
            &unsigned_receipt(&p),
        ))?)
        .as_ref(),
    );
    p.previous_witness_public_key = old_public;
    p.next_witness_public_key = new_public;
    verify_transition(&p, old_origin, new_origin, &old_pin, &new_pin)?;
    Ok(p)
}
pub(crate) fn self_verify(p: &SignedAuditTrustTransition) -> Result<()> {
    let a = &p.proposal.previous_binding;
    let b = &p.proposal.next_binding;
    verify_transition(
        p,
        &a.origin_public_key_sha256,
        &b.origin_public_key_sha256,
        &a.witness_public_key_sha256,
        &b.witness_public_key_sha256,
    )
}
pub async fn pending(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<Option<Uuid>> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT request_id FROM audit_trust_transition_requests WHERE scope=$1 AND completed=0",
    )
    .bind(scope)
    .fetch_optional(&mut **tx)
    .await?;
    value
        .map(|v| Uuid::parse_str(&v).map_err(|_| IdentityError::Corrupt))
        .transpose()
}
fn decode<T: serde::de::DeserializeOwned>(r: &sqlx::any::AnyRow) -> Result<T> {
    let raw: String = r.try_get("payload")?;
    if raw.len() > MAX_PROOF
        || audit::hash(raw.as_bytes()) != r.try_get::<String, _>("payload_sha256")?
    {
        return Err(IdentityError::Corrupt);
    }
    serde_json::from_str(&raw).map_err(|_| IdentityError::Corrupt)
}
pub async fn history(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
) -> Result<Vec<SignedAuditTrustTransition>> {
    let rows=sqlx::query("SELECT payload,payload_sha256 FROM audit_trust_transition_imports WHERE scope=$1 LIMIT 129").bind(scope).fetch_all(&mut **tx).await?;
    if rows.len() > MAX_TRANSITIONS as usize {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut values = Vec::new();
    for row in rows {
        let p: SignedAuditTrustTransition = decode(&row)?;
        self_verify(&p)?;
        if p.proposal.previous_binding.scope != scope {
            return Err(IdentityError::Corrupt);
        }
        values.push(p);
    }
    // Order by actual original predecessor identities, never mutable timestamps.
    let mut result = Vec::new();
    if !values.is_empty() {
        let roots: Vec<usize> = values
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                !values.iter().any(|q| {
                    q.proposal.next_binding.binding_id == p.proposal.previous_binding.binding_id
                })
            })
            .map(|(i, _)| i)
            .collect();
        if roots.len() != 1 {
            return Err(IdentityError::Corrupt);
        }
        let mut id = values[roots[0]].proposal.previous_binding.binding_id;
        while !values.is_empty() {
            let next: Vec<usize> = values
                .iter()
                .enumerate()
                .filter(|(_, p)| p.proposal.previous_binding.binding_id == id)
                .map(|(i, _)| i)
                .collect();
            if next.len() != 1 {
                return Err(IdentityError::Corrupt);
            }
            let p = values.remove(next[0]);
            if result
                .last()
                .is_some_and(|last: &SignedAuditTrustTransition| {
                    cp::canonical(&last.proposal.next_binding).ok()
                        != cp::canonical(&p.proposal.previous_binding).ok()
                })
            {
                return Err(IdentityError::Corrupt);
            }
            id = p.proposal.next_binding.binding_id;
            result.push(p);
        }
    }
    Ok(result)
}
pub async fn for_binding(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    id: Uuid,
) -> Result<Option<SignedAuditTrustTransition>> {
    Ok(history(tx, scope)
        .await?
        .into_iter()
        .find(|p| p.proposal.next_binding.binding_id == id))
}
pub async fn proposal(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    id: Uuid,
) -> Result<AuditTrustTransitionProposal> {
    cp::fence(tx, scope).await?;
    let row=sqlx::query("SELECT payload,payload_sha256 FROM audit_trust_transition_requests WHERE scope=$1 AND request_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let p: AuditTrustTransitionProposal = decode(&row)?;
    let a = &p.previous_binding;
    let b = &p.next_binding;
    verify_proposal(
        &p,
        &a.origin_public_key_sha256,
        &b.origin_public_key_sha256,
        &a.witness_public_key_sha256,
        &b.witness_public_key_sha256,
    )?;
    if p.request_id != id || a.scope != scope {
        return Err(IdentityError::Corrupt);
    }
    Ok(p)
}
/// The public API supports a new independent witness with the SAME origin key.
/// An origin-key change requires separate proved private operator authorities.
pub async fn prepare(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    actor: Option<Uuid>,
    authority: &AuditAuthority,
    input: PrepareAuditTrustTransition,
    observed_at: i64,
) -> Result<AuditTrustTransitionProposal> {
    if sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_trust_transition_requests WHERE scope=$1 AND request_id=$2",
    )
    .bind(scope)
    .bind(input.request_id.to_string())
    .fetch_one(&mut **tx)
    .await?
        == 0
    {
        cp::fence(tx, scope).await?;
        crate::audit_origin::admission(tx, authority).await?;
    }
    prepare_with_authorities(tx, scope, actor, authority, authority, input, observed_at).await
}
pub async fn prepare_with_authorities(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    actor: Option<Uuid>,
    old: &AuditAuthority,
    new: &AuditAuthority,
    input: PrepareAuditTrustTransition,
    observed_at: i64,
) -> Result<AuditTrustTransitionProposal> {
    if !input.independently_confirmed
        || input.request_id.is_nil()
        || input.binding_id.is_nil()
        || !cp::digest(&input.previous_checkpoint_sha256)
        || !cp::digest(&input.new_witness_public_key_sha256)
    {
        return Err(IdentityError::Validation);
    }
    cp::fence(tx, scope).await?;
    let input_sha = audit::hash(&cp::canonical(&input)?);
    if let Some(row) = sqlx::query(
        "SELECT input_sha256 FROM audit_trust_transition_requests WHERE scope=$1 AND request_id=$2",
    )
    .bind(scope)
    .bind(input.request_id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    {
        if row.try_get::<String, _>("input_sha256")? != input_sha {
            return Err(IdentityError::Conflict);
        }
        return proposal(tx, scope, input.request_id).await;
    }
    let previous = cp::binding(tx, scope)
        .await?
        .ok_or(IdentityError::NotFound)?;
    let checkpoint = cp::latest(tx, scope, Some(&previous))
        .await?
        .ok_or(IdentityError::NotFound)?;
    if previous.binding_id != input.binding_id
        || previous.origin_public_key_sha256 != old.public_key_sha256()?
        || checkpoint.1 != input.previous_checkpoint_sha256
        || pending(tx, scope).await?.is_some()
        || observed_at < checkpoint.0.witnessed_at
    {
        return Err(IdentityError::Conflict);
    }
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_checkpoint_requests WHERE scope=$1 AND completed=0",
    )
    .bind(scope)
    .fetch_one(&mut **tx)
    .await?;
    if unfinished != 0 {
        return Err(IdentityError::Conflict);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_trust_transition_requests WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?;
    if count >= MAX_TRANSITIONS {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut binding = AuditWitnessBinding {
        format: "cowork.audit.witness-binding.v1".into(),
        binding_id: input.request_id,
        origin_id: previous.origin_id,
        scope: scope.into(),
        origin_public_key_sha256: new.public_key_sha256()?,
        witness_public_key_sha256: input.new_witness_public_key_sha256,
        configured_at: observed_at,
        actor_id: actor,
        public_key: String::new(),
        signature: String::new(),
    };
    let (public, _, signature) = new.sign_metadata("cowork:audit:witness-binding:v1", &binding)?;
    binding.public_key = public;
    binding.signature = signature;
    let mut p = AuditTrustTransitionProposal {
        format: "cowork.audit.trust-proposal.v1".into(),
        request_id: input.request_id,
        previous_binding: previous,
        next_binding: binding,
        previous_checkpoint: checkpoint.0,
        prepared_at: observed_at,
        previous_origin_signature: String::new(),
        next_origin_signature: String::new(),
    };
    p.previous_origin_signature = old
        .sign_metadata(
            "cowork:audit:trust-origin-transition:v1",
            &unsigned_proposal(&p),
        )?
        .2;
    p.next_origin_signature = new
        .sign_metadata(
            "cowork:audit:trust-origin-transition:v1",
            &unsigned_proposal(&p),
        )?
        .2;
    let a = &p.previous_binding;
    let b = &p.next_binding;
    verify_proposal(
        &p,
        &a.origin_public_key_sha256,
        &b.origin_public_key_sha256,
        &a.witness_public_key_sha256,
        &b.witness_public_key_sha256,
    )?;
    let raw = cp::canonical(&p)?;
    sqlx::query("INSERT INTO audit_trust_transition_requests(scope,request_id,input_sha256,payload,payload_sha256,completed) VALUES($1,$2,$3,$4,$5,0)").bind(scope).bind(input.request_id.to_string()).bind(input_sha).bind(String::from_utf8(raw.clone()).map_err(|_|IdentityError::Corrupt)?).bind(audit::hash(&raw)).execute(&mut **tx).await?;
    Ok(p)
}
pub async fn import(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: ImportAuditTrustTransition,
) -> Result<SignedAuditTrustTransition> {
    import_inner(tx, scope, input, false).await
}
pub async fn import_operator(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: ImportAuditTrustTransition,
) -> Result<SignedAuditTrustTransition> {
    import_inner(tx, scope, input, true).await
}
async fn import_inner(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: ImportAuditTrustTransition,
    operator: bool,
) -> Result<SignedAuditTrustTransition> {
    if input.request_id.is_nil()
        || input.proposal_request_id.is_nil()
        || !input.independently_confirmed
        || input.original_bytes_base64.len() > MAX_PROOF * 4 / 3 + 8
    {
        return Err(IdentityError::Validation);
    }
    cp::fence(tx, scope).await?;
    let input_sha = audit::hash(&cp::canonical(&input)?);
    let original = URL_SAFE_NO_PAD
        .decode(&input.original_bytes_base64)
        .map_err(|_| IdentityError::Validation)?;
    if original.len() > MAX_PROOF {
        return Err(IdentityError::Validation);
    }
    let receipt: SignedAuditTrustTransition =
        serde_json::from_slice(&original).map_err(|_| IdentityError::Validation)?;
    self_verify(&receipt)?;
    let p = proposal(tx, scope, input.proposal_request_id).await?;
    if cp::canonical(&receipt.proposal)? != cp::canonical(&p)? {
        return Err(IdentityError::Conflict);
    }
    if let Some(row) =
        sqlx::query("SELECT * FROM audit_trust_transition_imports WHERE scope=$1 AND request_id=$2")
            .bind(scope)
            .bind(input.request_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
    {
        if row.try_get::<String, _>("input_sha256")? != input_sha
            || row.try_get::<String, _>("original_base64")? != input.original_bytes_base64
            || row.try_get::<String, _>("original_sha256")? != audit::hash(&original)
        {
            return Err(IdentityError::Conflict);
        }
        let value: SignedAuditTrustTransition = decode(&row)?;
        self_verify(&value)?;
        if cp::canonical(&value)? != cp::canonical(&receipt)? {
            return Err(IdentityError::Corrupt);
        }
        return Ok(value);
    }
    if !operator {
        if receipt.proposal.previous_binding.origin_public_key_sha256
            != receipt.proposal.next_binding.origin_public_key_sha256
        {
            return Err(IdentityError::Forbidden);
        }
        if let Some((_, _, _, pin, pending)) = crate::audit_origin::installed(tx).await?
            && (pending.is_some() || pin != receipt.proposal.next_binding.origin_public_key_sha256)
        {
            return Err(IdentityError::Conflict);
        }
    }
    let current = cp::binding(tx, scope)
        .await?
        .ok_or(IdentityError::NotFound)?;
    let head = cp::latest(tx, scope, Some(&current))
        .await?
        .ok_or(IdentityError::NotFound)?;
    if cp::canonical(&current)? != cp::canonical(&p.previous_binding)?
        || cp::canonical(&head.0)? != cp::canonical(&p.previous_checkpoint)?
        || pending(tx, scope).await? != Some(input.proposal_request_id)
    {
        return Err(IdentityError::Conflict);
    }
    let old = sqlx::query("SELECT * FROM audit_witness_bindings WHERE scope=$1")
        .bind(scope)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO audit_witness_binding_history(scope,binding_id,request_id,request_sha256,payload,payload_sha256) VALUES($1,$2,$3,$4,$5,$6)").bind(scope).bind(current.binding_id.to_string()).bind(old.try_get::<String,_>("request_id")?).bind(old.try_get::<String,_>("request_sha256")?).bind(old.try_get::<String,_>("payload")?).bind(old.try_get::<String,_>("payload_sha256")?).execute(&mut **tx).await?;
    let next = cp::canonical(&p.next_binding)?;
    if sqlx::query("UPDATE audit_witness_bindings SET request_id=$1,request_sha256=$2,payload=$3,payload_sha256=$4 WHERE scope=$5 AND payload=$6").bind(p.next_binding.binding_id.to_string()).bind(audit::hash(&cp::canonical(&p)?)).bind(String::from_utf8(next.clone()).map_err(|_|IdentityError::Corrupt)?).bind(audit::hash(&next)).bind(scope).bind(old.try_get::<String,_>("payload")?).execute(&mut **tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict);}
    let raw = cp::canonical(&receipt)?;
    sqlx::query("INSERT INTO audit_trust_transition_imports(scope,request_id,input_sha256,proposal_request_id,original_base64,original_sha256,payload,payload_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(scope).bind(input.request_id.to_string()).bind(input_sha).bind(input.proposal_request_id.to_string()).bind(input.original_bytes_base64).bind(audit::hash(&original)).bind(String::from_utf8(raw.clone()).map_err(|_|IdentityError::Corrupt)?).bind(audit::hash(&raw)).execute(&mut **tx).await?;
    if sqlx::query("UPDATE audit_trust_transition_requests SET completed=1 WHERE scope=$1 AND request_id=$2 AND completed=0").bind(scope).bind(input.proposal_request_id.to_string()).execute(&mut **tx).await?.rows_affected()!=1{return Err(IdentityError::Conflict);}
    Ok(receipt)
}
pub async fn original(tx: &mut Transaction<'_, Any>, scope: &str, id: Uuid) -> Result<String> {
    cp::fence(tx, scope).await?;
    let row=sqlx::query("SELECT original_base64,original_sha256 FROM audit_trust_transition_imports WHERE scope=$1 AND proposal_request_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let raw: String = row.try_get("original_base64")?;
    if raw.len() > MAX_PROOF * 4 / 3 + 8 {
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
