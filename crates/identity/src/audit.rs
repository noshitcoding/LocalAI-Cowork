//! Scoped metadata ledger and portable signed snapshots. No prompt, tool argument,
//! credential, native path, or model response is admitted into this projection.
use crate::{IdentityError, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_contracts::*;
use cowork_secrets::{KeyId, KeyStore};
use ring::{
    hmac,
    signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Any, Row, Transaction};
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const FORMAT: &str = "cowork.audit.page.v1";
pub const SIGNED_FORMAT: &str = "cowork.audit.signed.v1";
const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_CHECK: i64 = 100_000;
const MAX_ORIGINAL: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct AuditAuthority {
    keys: Arc<dyn KeyStore>,
    key_id: KeyId,
    epoch: i64,
}
impl AuditAuthority {
    pub fn new(keys: Arc<dyn KeyStore>, key_id: KeyId) -> Result<Self> {
        key_id.validate().map_err(|_| IdentityError::Corrupt)?;
        let key_id = keys
            .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::AuditSigning)
            .map_err(|_| IdentityError::Corrupt)?;
        let epoch = keys
            .audit_signing_epoch(&key_id)
            .map_err(|_| IdentityError::Corrupt)?;
        let value = Self {
            keys,
            key_id,
            epoch,
        };
        value.signer()?;
        Ok(value)
    }
    /// This original slot signs installation operator journals across all audit
    /// key epochs. It never implicitly follows the new page-signing key.
    pub fn new_operator(keys: Arc<dyn KeyStore>, key_id: KeyId) -> Result<Self> {
        let key_id = keys
            .key_for_purpose(&key_id, cowork_secrets::KeyPurpose::OperatorSigning)
            .map_err(|_| IdentityError::Corrupt)?;
        let value = Self {
            keys,
            key_id,
            epoch: 0,
        };
        value.signer()?;
        Ok(value)
    }
    /// Explicit proved candidate authority, used only by private rotation code.
    /// It performs no key discovery, creation, purpose-slot adoption or SQL write.
    pub fn rotation_candidate(
        keys: Arc<dyn KeyStore>,
        proof: &cowork_secrets::KeyProof,
    ) -> Result<Self> {
        cowork_secrets::verify_key(keys.as_ref(), proof).map_err(|_| IdentityError::Corrupt)?;
        let key_id = proof.key;
        let epoch = keys
            .audit_signing_epoch(&key_id)
            .map_err(|_| IdentityError::Corrupt)?;
        let value = Self {
            keys,
            key_id,
            epoch,
        };
        value.signer()?;
        Ok(value)
    }
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }
    pub fn epoch(&self) -> i64 {
        self.epoch
    }
    pub fn sign_origin_rotation(
        &self,
        payload: &impl Serialize,
    ) -> Result<(String, String, String)> {
        self.sign_metadata("cowork:audit:origin-rotation:v1", payload)
    }
    fn signer(&self) -> Result<Ed25519KeyPair> {
        let master = self
            .keys
            .load(&self.key_id)
            .map_err(|_| IdentityError::Corrupt)?;
        let input = encoded(&("cowork:audit:key:v1", self.key_id.instance, self.key_id.key))?;
        let seed = Zeroizing::new(
            hmac::sign(
                &hmac::Key::new(hmac::HMAC_SHA256, master.as_ref()),
                input.as_bytes(),
            )
            .as_ref()
            .to_vec(),
        );
        Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| IdentityError::Corrupt)
    }
    pub fn public_key_sha256(&self) -> Result<String> {
        Ok(hash(self.signer()?.public_key().as_ref()))
    }
    pub fn public_key(&self) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(self.signer()?.public_key().as_ref()))
    }
    /// Fixed operator-key history domain. Never signs an arbitrary caller-selected domain.
    pub fn sign_server_key_operation(
        &self,
        payload: &impl Serialize,
    ) -> Result<(String, String, String)> {
        self.sign_metadata("cowork:server:key-operation:v1", payload)
    }
    pub(crate) fn sign_metadata(
        &self,
        domain: &str,
        payload: &impl Serialize,
    ) -> Result<(String, String, String)> {
        let signer = self.signer()?;
        let public = URL_SAFE_NO_PAD.encode(signer.public_key().as_ref());
        let pin = hash(signer.public_key().as_ref());
        let message = encoded(&(domain, &public, &pin, payload))?;
        Ok((
            public,
            pin,
            URL_SAFE_NO_PAD.encode(signer.sign(message.as_bytes()).as_ref()),
        ))
    }
    pub(crate) fn sign(
        &self,
        request_id: Uuid,
        captured_at: i64,
        page: AuditPage,
    ) -> Result<SignedAuditArchive> {
        let signer = self.signer()?;
        let mut value = SignedAuditArchive {
            format: SIGNED_FORMAT.into(),
            request_id,
            captured_at,
            public_key: URL_SAFE_NO_PAD.encode(signer.public_key().as_ref()),
            public_key_sha256: hash(signer.public_key().as_ref()),
            page,
            signature: String::new(),
        };
        value.signature = URL_SAFE_NO_PAD.encode(signer.sign(&signature_bytes(&value)?).as_ref());
        Ok(value)
    }
}
fn encoded(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| IdentityError::Corrupt)
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn scope_valid(scope: &str) -> bool {
    scope == "personal"
        || ["account:", "organization:"].iter().any(|prefix| {
            scope.strip_prefix(prefix).is_some_and(|v| {
                Uuid::parse_str(v).is_ok_and(|id| !id.is_nil() && id.to_string() == v)
            })
        })
}
pub(crate) async fn writer_fence(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<()> {
    head(tx, scope).await.map(|_| ())
}
pub(crate) fn verify_metadata(
    domain: &str,
    public: &str,
    pin: &str,
    signature: &str,
    payload: &impl Serialize,
) -> Result<()> {
    let key = URL_SAFE_NO_PAD
        .decode(public)
        .map_err(|_| IdentityError::Corrupt)?;
    let sig = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| IdentityError::Corrupt)?;
    if key.len() != 32 || sig.len() != 64 || hash(&key) != pin {
        return Err(IdentityError::Corrupt);
    }
    let message = encoded(&(domain, public, pin, payload))?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(message.as_bytes(), &sig)
        .map_err(|_| IdentityError::Corrupt)
}
fn signature_bytes(value: &SignedAuditArchive) -> Result<Vec<u8>> {
    Ok(encoded(&(
        "cowork:audit:signature:v1",
        &value.format,
        value.request_id,
        value.captured_at,
        &value.public_key,
        &value.public_key_sha256,
        &value.page,
    ))?
    .into_bytes())
}
fn record_hash(scope: &str, chain: Uuid, record: &AuditRecord) -> Result<String> {
    Ok(hash(
        encoded(&(
            "cowork:audit:event:v1",
            scope,
            chain,
            record.sequence,
            &record.event_id,
            &record.action,
            &record.actor_id,
            &record.target_id,
            record.observed_at,
            &record.origin,
            &record.source_sha256,
            &record.previous_sha256,
        ))?
        .as_bytes(),
    ))
}
fn record_valid(record: &AuditRecord) -> bool {
    record.sequence > 0
        && record.observed_at > 0
        && !record.event_id.is_empty()
        && record.event_id.len() <= 256
        && !record.event_id.chars().any(char::is_control)
        && !record.action.is_empty()
        && record.action.len() <= 100
        && record
            .action
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:-".contains(&b))
        && [&record.actor_id, &record.target_id]
            .iter()
            .all(|id| id.as_ref().is_none_or(|v| Uuid::parse_str(v).is_ok()))
        && digest(&record.previous_sha256)
        && digest(&record.sha256)
        && record.source_sha256.as_ref().is_none_or(|v| digest(v))
}
pub fn verify_archive(value: &SignedAuditArchive, expected_public_key_sha256: &str) -> Result<()> {
    if value.format != SIGNED_FORMAT
        || value.request_id.is_nil()
        || value.captured_at <= 0
        || !digest(expected_public_key_sha256)
        || value.public_key_sha256 != expected_public_key_sha256
    {
        return Err(IdentityError::Validation);
    }
    let public = URL_SAFE_NO_PAD
        .decode(&value.public_key)
        .map_err(|_| IdentityError::Validation)?;
    let signature = URL_SAFE_NO_PAD
        .decode(&value.signature)
        .map_err(|_| IdentityError::Validation)?;
    if public.len() != 32 || signature.len() != 64 || hash(&public) != expected_public_key_sha256 {
        return Err(IdentityError::Validation);
    }
    verify_page(&value.page)?;
    UnparsedPublicKey::new(&ED25519, public)
        .verify(&signature_bytes(value)?, &signature)
        .map_err(|_| IdentityError::Validation)
}
pub fn verify_bundle(bundle: &AuditArchiveBundle, expected_public_key_sha256: &str) -> Result<()> {
    if bundle.format != "cowork.audit.bundle.v1"
        || bundle.pages.is_empty()
        || bundle.pages.len() > 200
    {
        return Err(IdentityError::Validation);
    }
    let first = &bundle.pages[0].page;
    let mut after = first.after_sequence;
    let mut previous = first.previous_sha256.clone();
    for archive in &bundle.pages {
        verify_archive(archive, expected_public_key_sha256)?;
        let page = &archive.page;
        if page.scope != first.scope
            || page.chain_id != first.chain_id
            || page.through_sequence != first.through_sequence
            || page.through_sha256 != first.through_sha256
            || page.after_sequence != after
            || page.previous_sha256 != previous
        {
            return Err(IdentityError::Validation);
        }
        after = page.next_sequence;
        if let Some(event) = page.events.last() {
            previous = event.sha256.clone();
        }
    }
    Ok(())
}
pub fn verify_page(page: &AuditPage) -> Result<()> {
    if page.format != FORMAT
        || !scope_valid(&page.scope)
        || page.chain_id.is_nil()
        || page.after_sequence < 0
        || page.through_sequence < page.after_sequence
        || page.events.len() > 500
        || !digest(&page.previous_sha256)
        || !digest(&page.through_sha256)
        || (page.after_sequence == 0 && page.previous_sha256 != ZERO)
    {
        return Err(IdentityError::Validation);
    }
    let mut sequence = page.after_sequence;
    let mut previous = page.previous_sha256.clone();
    for event in &page.events {
        sequence = sequence.checked_add(1).ok_or(IdentityError::Validation)?;
        if !record_valid(event)
            || event.sequence != sequence
            || event.previous_sha256 != previous
            || event.sha256 != record_hash(&page.scope, page.chain_id, event)?
        {
            return Err(IdentityError::Validation);
        }
        previous = event.sha256.clone();
    }
    if page.next_sequence != sequence
        || sequence > page.through_sequence
        || page.complete != (sequence == page.through_sequence)
        || (page.complete && previous != page.through_sha256)
        || (!page.complete && page.events.is_empty())
    {
        return Err(IdentityError::Validation);
    }
    Ok(())
}

struct Head {
    chain: Uuid,
    sequence: i64,
    hash: String,
}
async fn head(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<Head> {
    if !scope_valid(scope) {
        return Err(IdentityError::Validation);
    }
    sqlx::query("INSERT INTO audit_heads(scope,chain_id,sequence,head_sha256) VALUES($1,$2,0,$3) ON CONFLICT(scope) DO NOTHING")
        .bind(scope).bind(Uuid::new_v4().to_string()).bind(ZERO).execute(&mut **tx).await?;
    // Acquire the actual scoped SQL writer lock; all readers use this lock too.
    sqlx::query("UPDATE audit_heads SET sequence=sequence WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query("SELECT chain_id,sequence,head_sha256 FROM audit_heads WHERE scope=$1")
        .bind(scope)
        .fetch_one(&mut **tx)
        .await?;
    let value = Head {
        chain: Uuid::parse_str(&row.try_get::<String, _>("chain_id")?)
            .map_err(|_| IdentityError::Corrupt)?,
        sequence: row.try_get("sequence")?,
        hash: row.try_get("head_sha256")?,
    };
    if value.chain.is_nil()
        || value.sequence < 0
        || !digest(&value.hash)
        || (value.sequence == 0 && value.hash != ZERO)
    {
        return Err(IdentityError::Corrupt);
    }
    Ok(value)
}
async fn record(tx: &mut Transaction<'_, Any>, scope: &str, sequence: i64) -> Result<AuditRecord> {
    let row = sqlx::query("SELECT payload,sha256,chain_id FROM audit_records WHERE scope=$1 AND sequence=$2 AND chain_id=(SELECT chain_id FROM audit_heads WHERE scope=$1)").bind(scope).bind(sequence).fetch_optional(&mut **tx).await?.ok_or(IdentityError::Corrupt)?;
    let value: AuditRecord = serde_json::from_str(&row.try_get::<String, _>("payload")?)
        .map_err(|_| IdentityError::Corrupt)?;
    let chain = Uuid::parse_str(&row.try_get::<String, _>("chain_id")?)
        .map_err(|_| IdentityError::Corrupt)?;
    if !record_valid(&value)
        || value.sequence != sequence
        || value.sha256 != row.try_get::<String, _>("sha256")?
        || value.sha256 != record_hash(scope, chain, &value)?
    {
        return Err(IdentityError::Corrupt);
    }
    Ok(value)
}
pub struct MetadataEvent<'a> {
    pub event_id: &'a str,
    pub action: &'a str,
    pub actor_id: Option<String>,
    pub target_id: Option<String>,
    pub observed_at: i64,
    pub origin: AuditOrigin,
    pub source_sha256: Option<String>,
}
/// Must be called in the same transaction as its durable source event, after
/// identity/product locks. A failure rolls back both the source and projection.
pub async fn append(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: MetadataEvent<'_>,
) -> Result<i64> {
    let current = head(tx, scope).await?;
    if sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_retired_sources WHERE scope=$1 AND event_id=$2",
    )
    .bind(scope)
    .bind(input.event_id)
    .fetch_one(&mut **tx)
    .await?
        != 0
    {
        return Err(IdentityError::Conflict);
    }
    if let Some(row) =
        sqlx::query("SELECT sequence FROM audit_records WHERE scope=$1 AND event_id=$2")
            .bind(scope)
            .bind(input.event_id)
            .fetch_optional(&mut **tx)
            .await?
    {
        let sequence = row.try_get("sequence")?;
        let existing = record(tx, scope, sequence).await?;
        if existing.action != input.action
            || existing.actor_id != input.actor_id
            || existing.target_id != input.target_id
            || existing.observed_at != input.observed_at
            || existing.origin != input.origin
            || existing.source_sha256 != input.source_sha256
        {
            return Err(IdentityError::Conflict);
        }
        return Ok(sequence);
    }
    if current.sequence > 0 && record(tx, scope, current.sequence).await?.sha256 != current.hash {
        return Err(IdentityError::Corrupt);
    }
    let mut value = AuditRecord {
        sequence: current
            .sequence
            .checked_add(1)
            .ok_or(IdentityError::Corrupt)?,
        event_id: input.event_id.into(),
        action: input.action.into(),
        actor_id: input.actor_id,
        target_id: input.target_id,
        observed_at: input.observed_at,
        origin: input.origin,
        source_sha256: input.source_sha256,
        previous_sha256: current.hash,
        sha256: ZERO.into(),
    };
    if !record_valid(&value) {
        return Err(IdentityError::Validation);
    }
    value.sha256 = record_hash(scope, current.chain, &value)?;
    sqlx::query("INSERT INTO audit_records(scope,chain_id,sequence,event_id,payload,sha256) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(scope).bind(current.chain.to_string()).bind(value.sequence).bind(&value.event_id).bind(encoded(&value)?).bind(&value.sha256).execute(&mut **tx).await?;
    sqlx::query("UPDATE audit_heads SET sequence=$1,head_sha256=$2 WHERE scope=$3 AND sequence=$4")
        .bind(value.sequence)
        .bind(&value.sha256)
        .bind(scope)
        .bind(current.sequence)
        .execute(&mut **tx)
        .await?;
    Ok(value.sequence)
}
pub async fn page(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    after: i64,
    through: Option<i64>,
    limit: i64,
) -> Result<AuditPage> {
    if after < 0 || !(1..=500).contains(&limit) {
        return Err(IdentityError::Validation);
    }
    let current = head(tx, scope).await?;
    let through = through.unwrap_or(current.sequence);
    if through < after || through > current.sequence {
        return Err(IdentityError::Validation);
    }
    let previous = if after == 0 {
        ZERO.into()
    } else {
        record(tx, scope, after).await?.sha256
    };
    let through_hash = if through == 0 {
        ZERO.into()
    } else {
        record(tx, scope, through).await?.sha256
    };
    if through == current.sequence && through_hash != current.hash {
        return Err(IdentityError::Corrupt);
    }
    let rows = sqlx::query("SELECT sequence FROM audit_records WHERE scope=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT $4")
        .bind(scope).bind(after).bind(through).bind(limit).fetch_all(&mut **tx).await?;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        events.push(record(tx, scope, row.try_get("sequence")?).await?);
    }
    let next = events.last().map_or(after, |e| e.sequence);
    let value = AuditPage {
        format: FORMAT.into(),
        scope: scope.into(),
        chain_id: current.chain,
        after_sequence: after,
        previous_sha256: previous,
        through_sequence: through,
        through_sha256: through_hash,
        events,
        next_sequence: next,
        complete: next == through,
    };
    verify_page(&value).map_err(|_| IdentityError::Corrupt)?;
    Ok(value)
}
pub async fn capture(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    authority: &AuditAuthority,
    input: CaptureAuditArchive,
    observed_at: i64,
) -> Result<SignedAuditArchive> {
    if input.request_id.is_nil() {
        return Err(IdentityError::Validation);
    }
    let request_hash = hash(encoded(&input)?.as_bytes());
    head(tx, scope).await?;
    if let Some(row) = sqlx::query("SELECT request_sha256,payload FROM audit_archive_receipts WHERE scope=$1 AND request_id=$2")
        .bind(scope).bind(input.request_id.to_string()).fetch_optional(&mut **tx).await? {
        if row.try_get::<String,_>("request_sha256")? != request_hash { return Err(IdentityError::Conflict); }
        let original: SignedAuditArchive = serde_json::from_str(&row.try_get::<String,_>("payload")?).map_err(|_| IdentityError::Corrupt)?;
        verify_archive(&original,&original.public_key_sha256).map_err(|_| IdentityError::Corrupt)?;
        return Ok(original);
    }
    crate::audit_origin::admission(tx, authority).await?;
    let value = authority.sign(
        input.request_id,
        observed_at,
        page(
            tx,
            scope,
            input.after_sequence,
            input.through_sequence,
            input.limit,
        )
        .await?,
    )?;
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_archive_receipts WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?;
    if receipts >= 4096 {
        return Err(IdentityError::QuotaExceeded);
    }
    sqlx::query("INSERT INTO audit_archive_receipts(scope,request_id,request_sha256,payload) VALUES($1,$2,$3,$4)")
        .bind(scope).bind(input.request_id.to_string()).bind(request_hash).bind(encoded(&value)?).execute(&mut **tx).await?;
    Ok(value)
}
pub async fn verification(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<AuditVerification> {
    let current = head(tx, scope).await?;
    let mut value = AuditVerification {
        scope: scope.into(),
        chain_id: current.chain,
        valid: true,
        complete: false,
        checked_events: 0,
        head_sequence: current.sequence,
        head_sha256: current.hash.clone(),
        first_problem_sequence: None,
        legacy_unsigned_events: 0,
        preserved_archives: 0,
    };
    let mut previous = ZERO.to_owned();
    let through = current.sequence.min(MAX_CHECK);
    // Bounded verification, constant page memory. Never deletes or repairs bytes.
    while value.valid && value.checked_events < through {
        let rows = sqlx::query("SELECT sequence,chain_id,payload,sha256 FROM audit_records WHERE scope=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT 256")
            .bind(scope).bind(value.checked_events).bind(through).fetch_all(&mut **tx).await?;
        if rows.is_empty() {
            value.valid = false;
            value.first_problem_sequence = Some(value.checked_events + 1);
            break;
        }
        for row in rows {
            let sequence = value.checked_events + 1;
            let decoded =
                serde_json::from_str::<AuditRecord>(&row.try_get::<String, _>("payload")?);
            let good = match decoded {
                Ok(event)
                    if row.try_get::<i64, _>("sequence")? == sequence
                        && row.try_get::<String, _>("chain_id")? == current.chain.to_string()
                        && record_valid(&event)
                        && event.sequence == sequence
                        && event.previous_sha256 == previous
                        && event.sha256 == row.try_get::<String, _>("sha256")?
                        && event.sha256 == record_hash(scope, current.chain, &event)? =>
                {
                    previous = event.sha256;
                    value.checked_events += 1;
                    if event.origin == AuditOrigin::LegacyUnsigned {
                        value.legacy_unsigned_events += 1;
                    }
                    true
                }
                _ => false,
            };
            if !good {
                value.valid = false;
                value.first_problem_sequence = Some(sequence);
                break;
            }
        }
    }
    if value.valid && value.checked_events == current.sequence {
        value.complete = true;
        if previous != current.hash {
            value.valid = false;
            value.first_problem_sequence = Some(current.sequence);
        }
    }
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_records WHERE scope=$1")
        .bind(scope)
        .fetch_one(&mut **tx)
        .await?;
    if rows != current.sequence {
        value.valid = false;
        value.complete = false;
        value.first_problem_sequence = Some(value.checked_events.saturating_add(1));
    }
    value.preserved_archives =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_preserved_history WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?;
    Ok(value)
}
pub async fn preserve(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    input: PreserveAuditHistory,
    observed_at: i64,
) -> Result<PreservedAuditHistory> {
    if input.request_id.is_nil() || input.bytes_base64.len() > MAX_ORIGINAL * 4 / 3 + 8 {
        return Err(IdentityError::Validation);
    }
    let original = URL_SAFE_NO_PAD
        .decode(&input.bytes_base64)
        .map_err(|_| IdentityError::Validation)?;
    if original.is_empty() || original.len() > MAX_ORIGINAL {
        return Err(IdentityError::Validation);
    }
    let content = hash(&original);
    let request_hash = hash(encoded(&input)?.as_bytes());
    let current = head(tx, scope).await?;
    if let Some(row) = sqlx::query("SELECT request_sha256,receipt FROM audit_preserved_history WHERE scope=$1 AND request_id=$2")
        .bind(scope).bind(input.request_id.to_string()).fetch_optional(&mut **tx).await? {
        if row.try_get::<String,_>("request_sha256")? != request_hash { return Err(IdentityError::Conflict); }
        return serde_json::from_str(&row.try_get::<String,_>("receipt")?).map_err(|_| IdentityError::Corrupt);
    }
    let (trust, chain, public) = match serde_json::from_slice::<SignedAuditArchive>(&original) {
        Ok(archive) => {
            let expected = input
                .expected_public_key_sha256
                .as_deref()
                .ok_or(IdentityError::Validation)?;
            verify_archive(&archive, expected)?;
            (
                "verified_v2_signature",
                Some(archive.page.chain_id),
                Some(archive.public_key_sha256),
            )
        }
        Err(_) => {
            if let Ok(bundle) = serde_json::from_slice::<AuditArchiveBundle>(&original) {
                let expected = input
                    .expected_public_key_sha256
                    .as_deref()
                    .ok_or(IdentityError::Validation)?;
                verify_bundle(&bundle, expected)?;
                (
                    "verified_v2_signature",
                    Some(bundle.pages[0].page.chain_id),
                    Some(expected.to_owned()),
                )
            } else {
                if input.expected_public_key_sha256.is_some() {
                    return Err(IdentityError::Validation);
                }
                // V1 HMAC keys are separate native secrets. Original signatures are
                // preserved byte-for-byte, but we never assert they were verified.
                ("legacy_unverified", None, None)
            }
        }
    };
    let used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(LENGTH(original_base64)),0) FROM audit_preserved_history WHERE scope=$1").bind(scope).fetch_one(&mut **tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_preserved_history WHERE scope=$1")
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?;
    if count >= 64 || used.saturating_add(input.bytes_base64.len() as i64) > 358 * 1024 * 1024 {
        return Err(IdentityError::QuotaExceeded);
    }
    let sequence = append(
        tx,
        scope,
        MetadataEvent {
            event_id: &format!("archive:{}", input.request_id),
            action: "history_archive_preserved",
            actor_id: None,
            target_id: None,
            observed_at,
            origin: AuditOrigin::PreservedArchive,
            source_sha256: Some(content.clone()),
        },
    )
    .await?;
    let value = PreservedAuditHistory {
        request_id: input.request_id,
        content_sha256: content.clone(),
        bytes: original.len() as i64,
        trust: trust.into(),
        original_chain_id: chain,
        public_key_sha256: public,
        anchor_sequence: sequence,
        anchor_chain_id: Some(current.chain),
    };
    sqlx::query("INSERT INTO audit_preserved_history(scope,request_id,request_sha256,content_sha256,original_base64,receipt) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(scope).bind(input.request_id.to_string()).bind(request_hash).bind(content).bind(input.bytes_base64).bind(encoded(&value)?).execute(&mut **tx).await?;
    Ok(value)
}
pub async fn history(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    request_id: Uuid,
) -> Result<String> {
    let row = sqlx::query("SELECT original_base64,content_sha256 FROM audit_preserved_history WHERE scope=$1 AND request_id=$2").bind(scope).bind(request_id.to_string())
        .fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let value: String = row.try_get("original_base64")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(&value)
        .map_err(|_| IdentityError::Corrupt)?;
    if bytes.len() > MAX_ORIGINAL || hash(&bytes) != row.try_get::<String, _>("content_sha256")? {
        return Err(IdentityError::Corrupt);
    }
    Ok(value)
}
pub async fn history_receipts(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
) -> Result<Vec<PreservedAuditHistory>> {
    let rows = sqlx::query(
        "SELECT receipt FROM audit_preserved_history WHERE scope=$1 ORDER BY request_id LIMIT 1000",
    )
    .bind(scope)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            serde_json::from_str(&row.try_get::<String, _>("receipt")?)
                .map_err(|_| IdentityError::Corrupt)
        })
        .collect()
}
pub async fn insights(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    authority: Option<&AuditAuthority>,
) -> Result<AuditInsights> {
    let current = head(tx, scope).await?;
    let verification = verification(tx, scope).await?;
    let recent = if verification.valid {
        page(tx, scope, current.sequence.saturating_sub(20), None, 20)
            .await?
            .events
    } else {
        Vec::new()
    };
    let mut categories = std::collections::BTreeMap::<String, i64>::new();
    if verification.valid {
        for row in sqlx::query(
            "SELECT payload FROM audit_records WHERE scope=$1 ORDER BY sequence DESC LIMIT 10000",
        )
        .bind(scope)
        .fetch_all(&mut **tx)
        .await?
        {
            let event: AuditRecord = serde_json::from_str(&row.try_get::<String, _>("payload")?)
                .map_err(|_| IdentityError::Corrupt)?;
            // Category totals have an explicit bounded 10k-event window.
            *categories.entry(event.action).or_default() += 1;
        }
    }
    Ok(AuditInsights {
        scope: scope.into(),
        total_events: current.sequence,
        projects: None,
        orders: None,
        runs: None,
        messages: None,
        knowledge_documents: None,
        skills: None,
        categories: categories
            .into_iter()
            .map(|(category, count)| AuditCount { category, count })
            .collect(),
        run_states: Vec::new(),
        recent_events: recent,
        verification,
        public_key_sha256: authority
            .map(AuditAuthority::public_key_sha256)
            .transpose()?,
        execution: None,
        retired_chains: crate::audit_recovery::receipts(tx, scope).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Keys;
    impl KeyStore for Keys {
        fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
            Ok(Zeroizing::new([3; 32]))
        }
    }
    fn authority() -> AuditAuthority {
        AuditAuthority::new(
            Arc::new(Keys),
            KeyId {
                instance: Uuid::from_u128(1),
                key: Uuid::from_u128(2),
            },
        )
        .unwrap()
    }
    fn empty() -> AuditPage {
        AuditPage {
            format: FORMAT.into(),
            scope: "personal".into(),
            chain_id: Uuid::from_u128(3),
            after_sequence: 0,
            previous_sha256: ZERO.into(),
            through_sequence: 0,
            through_sha256: ZERO.into(),
            events: vec![],
            next_sequence: 0,
            complete: true,
        }
    }
    #[test]
    fn signature_checks_pin_and_every_snapshot_field() {
        let a = authority();
        let mut value = a.sign(Uuid::from_u128(4), 1, empty()).unwrap();
        verify_archive(&value, &a.public_key_sha256().unwrap()).unwrap();
        assert!(verify_archive(&value, ZERO).is_err());
        value.captured_at += 1;
        assert!(verify_archive(&value, &a.public_key_sha256().unwrap()).is_err());
    }
    #[test]
    fn altered_event_and_false_complete_page_are_rejected() {
        let mut page = empty();
        page.complete = false;
        assert!(verify_page(&page).is_err());
        page.complete = true;
        let mut event = AuditRecord {
            sequence: 1,
            event_id: "e".into(),
            action: "run_event".into(),
            actor_id: None,
            target_id: None,
            observed_at: 1,
            origin: AuditOrigin::NativeV2,
            source_sha256: None,
            previous_sha256: ZERO.into(),
            sha256: ZERO.into(),
        };
        event.sha256 = record_hash(&page.scope, page.chain_id, &event).unwrap();
        page.through_sequence = 1;
        page.next_sequence = 1;
        page.through_sha256 = event.sha256.clone();
        page.events.push(event);
        verify_page(&page).unwrap();
        page.events[0].action = "run_completed".into();
        assert!(verify_page(&page).is_err());
    }
    #[tokio::test]
    async fn sql_receipts_original_bytes_and_tamper_report_are_durable() {
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
        ]
        .join("\n")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        {
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
        }
        let event = || MetadataEvent {
            event_id: "event-1",
            action: "run_created",
            actor_id: None,
            target_id: None,
            observed_at: 1,
            origin: AuditOrigin::NativeV2,
            source_sha256: None,
        };
        assert_eq!(append(&mut tx, "personal", event()).await.unwrap(), 1);
        assert_eq!(append(&mut tx, "personal", event()).await.unwrap(), 1);
        let original = b"original-v1-signature\n\xff";
        let history_id = Uuid::from_u128(7);
        let receipt = preserve(
            &mut tx,
            "personal",
            PreserveAuditHistory {
                request_id: history_id,
                bytes_base64: URL_SAFE_NO_PAD.encode(original),
                expected_public_key_sha256: None,
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(receipt.trust, "legacy_unverified");
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(history(&mut tx, "personal", history_id).await.unwrap())
                .unwrap(),
            original
        );
        let input = CaptureAuditArchive {
            request_id: Uuid::from_u128(8),
            after_sequence: 0,
            through_sequence: None,
            limit: 500,
        };
        let signed = capture(&mut tx, "personal", &authority(), input.clone(), 3)
            .await
            .unwrap();
        verify_archive(&signed, &authority().public_key_sha256().unwrap()).unwrap();
        let replay = capture(&mut tx, "personal", &authority(), input.clone(), 4)
            .await
            .unwrap();
        assert_eq!(encoded(&signed).unwrap(), encoded(&replay).unwrap());
        let mut changed = input;
        changed.limit = 1;
        assert!(matches!(
            capture(&mut tx, "personal", &authority(), changed, 4).await,
            Err(IdentityError::Conflict)
        ));
        assert!(verification(&mut tx, "personal").await.unwrap().valid);
        sqlx::query("UPDATE audit_records SET payload='unparsed-original-data' WHERE scope='personal' AND sequence=1").execute(&mut *tx).await.unwrap();
        let report = verification(&mut tx, "personal").await.unwrap();
        assert!(!report.valid);
        assert_eq!(report.first_problem_sequence, Some(1));
        let redacted = insights(&mut tx, "personal", None).await.unwrap();
        assert!(redacted.recent_events.is_empty());
        assert!(redacted.categories.is_empty());
        assert!(
            !encoded(&redacted)
                .unwrap()
                .contains("unparsed-original-data")
        );
        verify_archive(&signed, &authority().public_key_sha256().unwrap()).unwrap();
        tx.commit().await.unwrap();
        pool.close().await;
    }
}
