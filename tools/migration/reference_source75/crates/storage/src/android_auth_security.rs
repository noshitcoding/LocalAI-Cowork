//! Actual Android authentication factors and original replies, under one identity writer.
//! Cipher AAD binds the subject, factor revision or original operation. No plaintext
//! password, authenticator secret, recovery code or returned one-shot token is stored.
use super::*;
use crate::android_devices::{device_iso, device_writer};
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_identity::{Principal, SessionIssued};
use cowork_secrets::{
    CredentialScope, KeyId, KeyStore, SealedCredential, open_bearer, seal_bearer,
};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Any, Transaction};
use team_service::identity_error;
use zeroize::{Zeroize, Zeroizing};
const VERIFY_WINDOW: i64 = 12 * 60 * 60 * 1000;
const ENROLL_WINDOW: i64 = 10 * 60 * 1000;
const FAILURE_WINDOW: i64 = 5 * 60 * 1000;
const ROW_LIMIT: usize = 100_000;
#[path = "android_auth_invitations.rs"]
mod invitations;
#[path = "android_auth_management.rs"]
mod management;
#[path = "android_auth_sessions.rs"]
mod sessions;
#[path = "android_auth_transport.rs"]
pub(crate) mod transport;
fn invalid() -> Error {
    Error::Validation("android_auth_security_invalid".into())
}
fn one(n: u64) -> Result<()> {
    if n == 1 { Ok(()) } else { Err(Error::Conflict) }
}
fn uuid(v: &str) -> Result<Uuid> {
    Uuid::parse_str(v)
        .ok()
        .filter(|u| !u.is_nil() && u.to_string() == v)
        .ok_or_else(invalid)
}
fn digest(v: &str) -> String {
    format!("{:x}", Sha256::digest(v.as_bytes()))
}
fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut v = Zeroizing::new([0; N]);
    SystemRandom::new()
        .fill(&mut *v)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    Ok(v)
}
fn base32(v: &[u8]) -> String {
    const A: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut buf, mut bits) = (0u32, 0u8);
    let mut out = String::new();
    for b in v {
        buf = (buf << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(A[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}
fn normalize_recovery(v: &str) -> Option<Zeroizing<String>> {
    if v.len() > 128 {
        return None;
    }
    let value = Zeroizing::new(
        v.chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase(),
    );
    (value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b)))
    .then_some(value)
}
fn factor_scope(owner: Uuid, revision: i64) -> CredentialScope {
    CredentialScope {
        project_id: format!("android-auth-security/{owner}"),
        server_id: "totp".into(),
        reference: "totp-secret/v1".into(),
        revision,
        endpoint: "localai-cowork://android/auth/totp".into(),
    }
}
fn operation_scope(owner: Uuid, operation: Uuid) -> CredentialScope {
    CredentialScope {
        project_id: format!("android-auth-security/{owner}"),
        server_id: operation.to_string(),
        reference: "original-auth-operation/v1".into(),
        revision: 1,
        endpoint: "localai-cowork://android/auth/operations".into(),
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    owner_id: Uuid,
    session_id: Uuid,
    operation_id: Uuid,
    purpose: String,
    request: String,
    response: String,
}
impl Drop for Original {
    fn drop(&mut self) {
        self.request.zeroize();
        self.response.zeroize();
    }
}
async fn authority(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
) -> Result<cowork_identity::Identity> {
    let actual = store
        .identity()
        .validate_principal_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    store
        .identity()
        .require_global_session_in_transaction(tx, actor)
        .await
        .map_err(identity_error)?;
    Ok(actual)
}
async fn audit(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    action: &str,
    target: Uuid,
    current: i64,
) -> Result<()> {
    let sequence: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0)+1 FROM identity_audit")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    sqlx::query("INSERT INTO identity_audit(id,sequence,organization_id,actor_id,action,target_id,observed_at) VALUES($1,$2,NULL,$3,$4,$5,$6)").bind(id()).bind(sequence).bind(actor.identity_id().to_string()).bind(action).bind(target.to_string()).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
async fn replay(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    actor: &Principal,
    operation: Uuid,
    purpose: &str,
    request: &str,
) -> Result<Option<Value>> {
    let row=sqlx::query("SELECT session_id,purpose,scope,sealed FROM android_auth_operations WHERE owner_id=$1 AND operation_id=$2").bind(actor.identity_id().to_string()).bind(operation.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let Some(r) = row else { return Ok(None) };
    if r.try_get::<String, _>("session_id")
        .map_err(storage_error)?
        != actor.session_id().to_string()
        || r.try_get::<String, _>("purpose").map_err(storage_error)? != purpose
    {
        return Err(Error::Conflict);
    }
    let scope: CredentialScope = parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
    if serde_json::to_value(&scope).map_err(storage_error)?
        != serde_json::to_value(operation_scope(actor.identity_id(), operation))
            .map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    let sealed: SealedCredential =
        parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
    let b64 = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
    let bytes = Zeroizing::new(
        STANDARD
            .decode(b64.as_bytes())
            .map_err(|_| Error::Forbidden)?,
    );
    let old: Original = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
    if old.owner_id != actor.identity_id()
        || old.session_id != actor.session_id()
        || old.operation_id != operation
        || old.purpose != purpose
    {
        return Err(Error::Forbidden);
    }
    if old.request != request {
        return Err(Error::Conflict);
    }
    Ok(Some(parse(&old.response)?))
}
async fn remember(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    key: KeyId,
    actor: &Principal,
    operation: Uuid,
    purpose: &str,
    request: &str,
    response: &Value,
    current: i64,
) -> Result<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_operations WHERE owner_id=$1")
            .bind(actor.identity_id().to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if count >= 4096 {
        return Err(Error::BudgetExceeded);
    }
    let value = Original {
        owner_id: actor.identity_id(),
        session_id: actor.session_id(),
        operation_id: operation,
        purpose: purpose.into(),
        request: request.into(),
        response: serde_json::to_string(response).map_err(storage_error)?,
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&value).map_err(storage_error)?);
    let b64 = Zeroizing::new(STANDARD.encode(&*bytes));
    if b64.len() > 4096 {
        return Err(Error::BudgetExceeded);
    }
    let scope = operation_scope(actor.identity_id(), operation);
    crate::server_keys::cipher_writer(tx, key).await?;
    let sealed = seal_bearer(keys, key, &scope, &b64).map_err(|_| Error::NativeKeyUnavailable)?;
    sqlx::query("INSERT INTO android_auth_operations(owner_id,session_id,operation_id,purpose,scope,sealed,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(actor.identity_id().to_string()).bind(actor.session_id().to_string()).bind(operation.to_string()).bind(purpose).bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
#[derive(Clone, Copy)]
struct Proof {
    revision: i64,
    method: &'static str,
}
enum Verification {
    Accepted(Option<Proof>),
    Denied,
}
fn matching(secret: &[u8], code: &str, current: i64, last: i64) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let step = current / 30000;
    let mut accepted = None;
    for counter in [step - 1, step, step + 1] {
        if counter < 0 {
            continue;
        }
        let tag = hmac::sign(&key, &(counter as u64).to_be_bytes());
        let v = tag.as_ref();
        let p = usize::from(v[19] & 15);
        let n = ((u32::from(v[p] & 127) << 24)
            | (u32::from(v[p + 1]) << 16)
            | (u32::from(v[p + 2]) << 8)
            | u32::from(v[p + 3]))
            % 1_000_000;
        let expected = Zeroizing::new(format!("{n:06}"));
        let same = expected
            .as_bytes()
            .iter()
            .zip(code.as_bytes())
            .fold(0u8, |a, (x, y)| a | (*x ^ *y))
            == 0;
        if same && counter > last {
            accepted = Some(counter);
        }
    }
    accepted
}
async fn verify_factor(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    owner: Uuid,
    code: Option<&str>,
    current: i64,
    pending: Option<Uuid>,
) -> Result<Verification> {
    let row = sqlx::query("SELECT * FROM android_auth_totp WHERE owner_id=$1")
        .bind(owner.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?;
    let Some(r) = row else {
        return Ok(if pending.is_none() && code.is_none() {
            Verification::Accepted(None)
        } else {
            Verification::Denied
        });
    };
    let enabled = r.try_get::<i64, _>("enabled").map_err(storage_error)? == 1;
    if pending.is_none() && !enabled {
        return Ok(if code.is_none() {
            Verification::Accepted(None)
        } else {
            Verification::Denied
        });
    }
    if let Some(session) = pending {
        if enabled
            || r.try_get::<String, _>("enrollment_session_id")
                .map_err(storage_error)?
                != session.to_string()
            || r.try_get::<i64, _>("pending_expires_at")
                .map_err(storage_error)?
                <= current
        {
            return Ok(Verification::Denied);
        }
    }
    if r.try_get::<i64, _>("blocked_until")
        .map_err(storage_error)?
        > current
    {
        return Ok(Verification::Denied);
    }
    let revision: i64 = r.try_get("revision").map_err(storage_error)?;
    let supplied = code.unwrap_or("").trim();
    let mut proof = None;
    if supplied.len() == 6 && supplied.bytes().all(|b| b.is_ascii_digit()) {
        let scope: CredentialScope =
            parse(&r.try_get::<String, _>("scope").map_err(storage_error)?)?;
        if serde_json::to_value(&scope).map_err(storage_error)?
            != serde_json::to_value(factor_scope(owner, revision)).map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let sealed: SealedCredential =
            parse(&r.try_get::<String, _>("sealed").map_err(storage_error)?)?;
        let b64 = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let secret = Zeroizing::new(
            STANDARD
                .decode(b64.as_bytes())
                .map_err(|_| Error::Forbidden)?,
        );
        if secret.len() != 20 {
            return Err(Error::Forbidden);
        }
        let last: i64 = r.try_get("last_counter").map_err(storage_error)?;
        if let Some(counter) = matching(&secret, supplied, current, last) {
            one(sqlx::query("UPDATE android_auth_totp SET last_counter=$1,failed_attempts=0,failure_window_started_at=0,blocked_until=0 WHERE owner_id=$2 AND revision=$3 AND last_counter=$4").bind(counter).bind(owner.to_string()).bind(revision).bind(last).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
            proof = Some(Proof {
                revision,
                method: "totp",
            });
        }
    } else if enabled && pending.is_none() {
        if let Some(normal) = normalize_recovery(supplied) {
            let n=sqlx::query("UPDATE android_auth_recovery_codes SET used_at=$1 WHERE owner_id=$2 AND factor_revision=$3 AND digest=$4 AND used_at=0").bind(current).bind(owner.to_string()).bind(revision).bind(digest(&normal)).execute(&mut **tx).await.map_err(storage_error)?.rows_affected();
            if n == 1 {
                proof = Some(Proof {
                    revision,
                    method: "recovery",
                });
                sqlx::query("UPDATE android_auth_totp SET failed_attempts=0,failure_window_started_at=0,blocked_until=0 WHERE owner_id=$1 AND revision=$2").bind(owner.to_string()).bind(revision).execute(&mut **tx).await.map_err(storage_error)?;
            }
        }
    }
    if let Some(proof) = proof {
        return Ok(Verification::Accepted(Some(proof)));
    }
    let started: i64 = r
        .try_get("failure_window_started_at")
        .map_err(storage_error)?;
    let failures = if started > current.saturating_sub(FAILURE_WINDOW) {
        r.try_get::<i64, _>("failed_attempts")
            .map_err(storage_error)?
            .saturating_add(1)
    } else {
        1
    };
    sqlx::query("UPDATE android_auth_totp SET failed_attempts=$1,failure_window_started_at=$2,blocked_until=$3 WHERE owner_id=$4 AND revision=$5").bind(failures).bind(if failures==1{current}else{started}).bind(if failures>=5{current.saturating_add(FAILURE_WINDOW)}else{0}).bind(owner.to_string()).bind(revision).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(Verification::Denied)
}
async fn mark(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    proof: Proof,
    current: i64,
) -> Result<()> {
    // Pre-59 sessions have no trustworthy creation/device provenance. Record
    // only this actually verified factor and leave creation unknown (zero).
    sqlx::query("INSERT INTO android_auth_session_metadata(session_id,owner_id,device_id,created_at,last_used_at) VALUES($1,$2,'',0,$3) ON CONFLICT(session_id) DO NOTHING")
        .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(current).execute(&mut **tx).await.map_err(storage_error)?;
    one(sqlx::query("UPDATE android_auth_session_metadata SET verified_at=$1,factor_revision=$2,factor_method=$3,last_used_at=$1 WHERE session_id=$4 AND owner_id=$5 AND revoked_at=0").bind(current).bind(proof.revision).bind(proof.method).bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())
}
async fn assurance(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    actual: &cowork_identity::Identity,
    current: i64,
) -> Result<Value> {
    let required: i64 =
        sqlx::query_scalar("SELECT admin_required FROM android_auth_security_policy WHERE id=1")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let totp =
        sqlx::query("SELECT revision FROM android_auth_totp WHERE owner_id=$1 AND enabled=1")
            .bind(actor.identity_id().to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
    let passkeys: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM android_passkeys WHERE identity_id=$1")
            .bind(actor.identity_id().to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let meta=sqlx::query("SELECT created_at,verified_at,factor_revision,factor_method FROM android_auth_session_metadata WHERE session_id=$1 AND owner_id=$2 AND revoked_at=0").bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    let mut verified = false;
    let mut recent = false;
    if let Some(m) = meta {
        let created: i64 = m.try_get("created_at").map_err(storage_error)?;
        recent = created <= current && created > current.saturating_sub(ENROLL_WINDOW);
        let at: i64 = m.try_get("verified_at").map_err(storage_error)?;
        let revision: i64 = m.try_get("factor_revision").map_err(storage_error)?;
        let method: String = m.try_get("factor_method").map_err(storage_error)?;
        let factor = if method == "passkey" {
            let proven:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_passkey_proofs p JOIN android_passkeys k ON k.id=p.passkey_id AND k.identity_id=p.owner_id WHERE p.session_id=$1 AND p.owner_id=$2 AND p.credential_revision=$3 AND k.revision>=p.credential_revision AND p.verified_at=$4")
                .bind(actor.session_id().to_string()).bind(actor.identity_id().to_string()).bind(revision).bind(at).fetch_one(&mut **tx).await.map_err(storage_error)?;
            proven == 1
        } else {
            totp.as_ref()
                .is_some_and(|r| r.try_get::<i64, _>("revision").ok() == Some(revision))
                && matches!(method.as_str(), "totp" | "recovery")
        };
        verified = factor && at <= current && at > current.saturating_sub(VERIFY_WINDOW);
    }
    let required = required == 1 && actual.platform_admin;
    let enrolled = totp.is_some() || passkeys > 0;
    let allowed = !required || verified || (!enrolled && recent);
    Ok(
        json!({"required":required,"totp_enabled":totp.is_some(),"passkey_registered":passkeys>0,"session_verified":verified,"enrollment_allowed":allowed,"reauthentication_required":required&&!enrolled&&!allowed}),
    )
}
async fn recovery_codes(
    tx: &mut Transaction<'_, Any>,
    owner: Uuid,
    revision: i64,
) -> Result<Vec<String>> {
    sqlx::query("DELETE FROM android_auth_recovery_codes WHERE owner_id=$1")
        .bind(owner.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    let mut codes = Vec::new();
    for _ in 0..10 {
        let bytes = random::<25>()?;
        let raw = Zeroizing::new(base32(&*bytes));
        let code = raw
            .as_bytes()
            .chunks(4)
            .map(|v| std::str::from_utf8(v).map_err(|_| invalid()))
            .collect::<Result<Vec<_>>>()?
            .join("-");
        sqlx::query("INSERT INTO android_auth_recovery_codes(owner_id,factor_revision,digest) VALUES($1,$2,$3)").bind(owner.to_string()).bind(revision).bind(digest(&raw)).execute(&mut **tx).await.map_err(storage_error)?;
        codes.push(code);
    }
    Ok(codes)
}
impl Store {
    pub async fn android_security_login(
        &self,
        keys: &dyn KeyStore,
        email: &str,
        password: &str,
        device: &str,
        code: Option<&str>,
        ttl: i64,
    ) -> Result<SessionIssued> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let witness = self
            .identity()
            .verify_android_password_in_transaction(&mut tx, email, password)
            .await
            .map_err(identity_error)?;
        // Rotation owns ServerKeyGuard before rewrapping factor rows. Take the
        // same guard before any counter/recovery CAS, even without new ciphertext.
        crate::server_keys::maintenance_fence(&mut tx).await?;
        let current = device_writer(&mut tx).await?;
        let verified =
            match verify_factor(&mut tx, keys, witness.identity().id, code, current, None).await? {
                Verification::Accepted(v) => v,
                Verification::Denied => {
                    tx.commit().await.map_err(storage_error)?;
                    return Err(Error::Forbidden);
                }
            };
        let issued = self
            .identity()
            .issue_android_password_session_in_transaction(&mut tx, &witness, ttl)
            .await
            .map_err(identity_error)?;
        self.identity()
            .record_android_session_in_transaction(
                &mut tx,
                &issued,
                device,
                verified.map(|p| (p.revision, p.method)),
            )
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(issued)
    }
    pub async fn android_security_record_session(
        &self,
        issued: &SessionIssued,
        device: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        self.identity()
            .record_android_session_in_transaction(&mut tx, issued, device, None)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn android_security_status(&self, actor: &Principal) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let actual = authority(self, &mut tx, actor).await?;
        let mut value = assurance(&mut tx, actor, &actual, current).await?;
        let origin = self
            .identity()
            .android_oidc_origin_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        value["provider_verified"] = json!(origin.is_some());
        if let Some(origin) = origin {
            value["provider_verification_expires_at"] = json!(device_iso(origin.expires_at)?);
            if let Some(org) = origin.organization_id {
                value["oidc_organisation_id"] = json!(org);
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_security_totp_status(&self, actor: &Principal) -> Result<Value> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        authority(self, &mut tx, actor).await?;
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_auth_recovery_codes c JOIN android_auth_totp t ON t.owner_id=c.owner_id AND t.revision=c.factor_revision WHERE c.owner_id=$1 AND c.used_at=0 AND t.enabled=1").bind(actor.identity_id().to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let enabled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_auth_totp WHERE owner_id=$1 AND enabled=1",
        )
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(json!({"schema_version":2,"enabled":enabled==1,"unused_recovery_codes":count}))
    }
    pub async fn android_security_totp_setup(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        actor: &Principal,
        operation: Uuid,
    ) -> Result<Value> {
        if operation.is_nil() {
            return Err(invalid());
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current = device_writer(&mut tx).await?;
        let actual = authority(self, &mut tx, actor).await?;
        if let Some(v) = replay(&mut tx, keys, actor, operation, "totp_setup", "{}").await? {
            authority(self, &mut tx, actor).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        if assurance(&mut tx, actor, &actual, current).await?["enrollment_allowed"] != true {
            return Err(Error::Forbidden);
        }
        let old = sqlx::query("SELECT revision,enabled FROM android_auth_totp WHERE owner_id=$1")
            .bind(actor.identity_id().to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage_error)?;
        if old
            .as_ref()
            .is_some_and(|r| r.try_get::<i64, _>("enabled").ok() == Some(1))
        {
            return Err(Error::Conflict);
        }
        let revision = old
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        let secret = random::<20>()?;
        let encoded = Zeroizing::new(base32(&*secret));
        let b64 = Zeroizing::new(STANDARD.encode(&*secret));
        let scope = factor_scope(actor.identity_id(), revision);
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let sealed =
            seal_bearer(keys, key, &scope, &b64).map_err(|_| Error::NativeKeyUnavailable)?;
        let expires = current.checked_add(ENROLL_WINDOW).ok_or(Error::Conflict)?;
        sqlx::query("INSERT INTO android_auth_totp(owner_id,revision,scope,sealed,enabled,enrollment_session_id,pending_expires_at,last_counter,updated_at) VALUES($1,$2,$3,$4,0,$5,$6,-1,$7) ON CONFLICT(owner_id) DO UPDATE SET revision=excluded.revision,scope=excluded.scope,sealed=excluded.sealed,enabled=0,enrollment_session_id=excluded.enrollment_session_id,pending_expires_at=excluded.pending_expires_at,last_counter=-1,failed_attempts=0,failure_window_started_at=0,blocked_until=0,updated_at=excluded.updated_at")
            .bind(actor.identity_id().to_string()).bind(revision).bind(json_string(&scope)?).bind(json_string(&sealed)?).bind(actor.session_id().to_string()).bind(expires).bind(current).execute(&mut *tx).await.map_err(storage_error)?;
        let label = percent_encoding::utf8_percent_encode(
            &actual.username,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let value = json!({"schema_version":2,"secret":*encoded,"otpauth_uri":format!("otpauth://totp/LocalAI%20Cowork:{label}?secret={}&issuer=LocalAI%20Cowork&algorithm=SHA1&digits=6&period=30",*encoded),"expires_at":device_iso(expires)?});
        remember(
            &mut tx,
            keys,
            key,
            actor,
            operation,
            "totp_setup",
            "{}",
            &value,
            current,
        )
        .await?;
        audit(
            &mut tx,
            actor,
            "auth.totp.setup",
            actor.identity_id(),
            current,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    /// Enable, verify, rotate recovery codes and disable all preserve immutable
    /// original replies. A repeated old key never repeats factor consumption.
    pub async fn android_security_factor_operation(
        &self,
        keys: &dyn KeyStore,
        key: KeyId,
        actor: &Principal,
        operation: Uuid,
        purpose: &str,
        code: &str,
        password: Option<&str>,
    ) -> Result<Value> {
        if operation.is_nil()
            || !matches!(
                purpose,
                "totp_enable" | "assurance_verify" | "recovery_codes" | "totp_disable"
            )
            || code.is_empty()
            || code.len() > 128
        {
            return Err(invalid());
        }
        let key = crate::android_private_workspace::writer_cipher(keys, key)?;
        let request = Zeroizing::new(
            serde_json::to_string(&json!({"code":code,"password":password}))
                .map_err(storage_error)?,
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        device_writer(&mut tx).await?;
        let actual = authority(self, &mut tx, actor).await?;
        if let Some(v) = replay(&mut tx, keys, actor, operation, purpose, &request).await? {
            authority(self, &mut tx, actor).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(v);
        }
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        let current = device_writer(&mut tx).await?;
        if purpose == "totp_disable" {
            self.identity()
                .verify_current_password_in_transaction(
                    &mut tx,
                    actor,
                    password.ok_or_else(invalid)?,
                )
                .await
                .map_err(identity_error)?;
        }
        let pending = (purpose == "totp_enable").then_some(actor.session_id());
        let proof = match verify_factor(
            &mut tx,
            keys,
            actor.identity_id(),
            Some(code),
            current,
            pending,
        )
        .await?
        {
            Verification::Accepted(Some(p)) => p,
            _ => {
                tx.commit().await.map_err(storage_error)?;
                return Err(Error::Forbidden);
            }
        };
        let value = match purpose {
            "totp_enable" => {
                if assurance(&mut tx, actor, &actual, current).await?["enrollment_allowed"] != true
                {
                    return Err(Error::Forbidden);
                }
                one(sqlx::query("UPDATE android_auth_totp SET enabled=1,pending_expires_at=0,updated_at=$1 WHERE owner_id=$2 AND revision=$3 AND enabled=0 AND enrollment_session_id=$4").bind(current).bind(actor.identity_id().to_string()).bind(proof.revision).bind(actor.session_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
                mark(&mut tx, actor, proof, current).await?;
                json!({"schema_version":2,"recovery_codes":recovery_codes(&mut tx,actor.identity_id(),proof.revision).await?})
            }
            "recovery_codes" => {
                mark(&mut tx, actor, proof, current).await?;
                json!({"schema_version":2,"recovery_codes":recovery_codes(&mut tx,actor.identity_id(),proof.revision).await?})
            }
            "totp_disable" => {
                let required =
                    assurance(&mut tx, actor, &actual, current).await?["required"] == true;
                let passkeys: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM android_passkeys WHERE identity_id=$1",
                )
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
                if required && passkeys == 0 {
                    return Err(Error::Forbidden);
                }
                one(sqlx::query("UPDATE android_auth_totp SET enabled=0,pending_expires_at=0,updated_at=$1 WHERE owner_id=$2 AND revision=$3 AND enabled=1").bind(current).bind(actor.identity_id().to_string()).bind(proof.revision).execute(&mut *tx).await.map_err(storage_error)?.rows_affected())?;
                sqlx::query("DELETE FROM android_auth_recovery_codes WHERE owner_id=$1")
                    .bind(actor.identity_id().to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                sqlx::query("UPDATE android_auth_session_metadata SET verified_at=0,factor_revision=0,factor_method='' WHERE owner_id=$1 AND factor_method IN ('totp','recovery')").bind(actor.identity_id().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
                json!({"schema_version":2,"accepted":true})
            }
            _ => {
                mark(&mut tx, actor, proof, current).await?;
                assurance(&mut tx, actor, &actual, current).await?
            }
        };
        remember(
            &mut tx, keys, key, actor, operation, purpose, &request, &value, current,
        )
        .await?;
        audit(&mut tx, actor, purpose, actor.identity_id(), current).await?;
        authority(self, &mut tx, actor).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}

/// Rotation-only codec under the admitted global server-key writer. Original
/// AAD and business revisions survive. No request/password hash key is created.
pub(crate) async fn rewrap_helper(
    tx: &mut Transaction<'_, Any>,
    keys: &dyn KeyStore,
    target: KeyId,
) -> Result<u64> {
    // The admitted rotation owns the registry writer while its candidate is
    // intentionally not the current normal cipher writer yet.
    target.validate().map_err(|_| Error::NativeKeyUnavailable)?;
    keys.load(&target)
        .map_err(|_| Error::NativeKeyUnavailable)?;
    let mut count = 0;
    let factors=sqlx::query("SELECT owner_id,revision,scope,sealed FROM android_auth_totp ORDER BY owner_id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    if factors.len() > ROW_LIMIT {
        return Err(Error::BudgetExceeded);
    }
    for r in factors {
        let owner = uuid(&r.try_get::<String, _>("owner_id").map_err(storage_error)?)?;
        let revision: i64 = r.try_get("revision").map_err(storage_error)?;
        let scope_raw: String = r.try_get("scope").map_err(storage_error)?;
        let scope: CredentialScope = parse(&scope_raw)?;
        if serde_json::to_value(&scope).map_err(storage_error)?
            != serde_json::to_value(factor_scope(owner, revision)).map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let original: String = r.try_get("sealed").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            STANDARD
                .decode(plain.as_bytes())
                .map_err(|_| Error::Forbidden)?,
        );
        if bytes.len() != 20 {
            return Err(Error::Forbidden);
        }
        let next =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        one(sqlx::query("UPDATE android_auth_totp SET sealed=$1 WHERE owner_id=$2 AND revision=$3 AND scope=$4 AND sealed=$5").bind(json_string(&next)?).bind(owner.to_string()).bind(revision).bind(scope_raw).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    let operations=sqlx::query("SELECT owner_id,session_id,operation_id,purpose,scope,sealed FROM android_auth_operations ORDER BY owner_id,operation_id LIMIT 100001").fetch_all(&mut **tx).await.map_err(storage_error)?;
    if operations.len() > ROW_LIMIT {
        return Err(Error::BudgetExceeded);
    }
    for r in operations {
        let owner = uuid(&r.try_get::<String, _>("owner_id").map_err(storage_error)?)?;
        let operation = uuid(
            &r.try_get::<String, _>("operation_id")
                .map_err(storage_error)?,
        )?;
        let scope_raw: String = r.try_get("scope").map_err(storage_error)?;
        let scope: CredentialScope = parse(&scope_raw)?;
        if serde_json::to_value(&scope).map_err(storage_error)?
            != serde_json::to_value(operation_scope(owner, operation)).map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let original: String = r.try_get("sealed").map_err(storage_error)?;
        let sealed: SealedCredential = parse(&original)?;
        if sealed.key_id().instance != target.instance || sealed.key_id() == target {
            continue;
        }
        let plain = open_bearer(keys, &scope, &sealed).map_err(|_| Error::NativeKeyUnavailable)?;
        let bytes = Zeroizing::new(
            STANDARD
                .decode(plain.as_bytes())
                .map_err(|_| Error::Forbidden)?,
        );
        let value: Original = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
        if value.owner_id != owner
            || value.operation_id != operation
            || value.session_id.to_string()
                != r.try_get::<String, _>("session_id")
                    .map_err(storage_error)?
            || value.purpose != r.try_get::<String, _>("purpose").map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let next =
            seal_bearer(keys, target, &scope, &plain).map_err(|_| Error::NativeKeyUnavailable)?;
        one(sqlx::query("UPDATE android_auth_operations SET sealed=$1 WHERE owner_id=$2 AND session_id=$3 AND operation_id=$4 AND purpose=$5 AND scope=$6 AND sealed=$7").bind(json_string(&next)?).bind(owner.to_string()).bind(r.try_get::<String,_>("session_id").map_err(storage_error)?).bind(operation.to_string()).bind(r.try_get::<String,_>("purpose").map_err(storage_error)?).bind(scope_raw).bind(original).execute(&mut **tx).await.map_err(storage_error)?.rows_affected())?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Keys;
    impl KeyStore for Keys {
        fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
            Ok(Zeroizing::new(Sha256::digest(id.key.as_bytes()).into()))
        }
    }
    // Independent RFC 6238 Appendix B SHA-1 vector, reduced to the original
    // V1 six-digit wire. This fixture never claims deployed authenticator QA.
    fn code(secret: &[u8], millis: i64) -> String {
        let tag = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret),
            &((millis / 30000) as u64).to_be_bytes(),
        );
        let bytes = tag.as_ref();
        let offset = usize::from(bytes[19] & 15);
        let value = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
        format!("{:06}", value % 1_000_000)
    }
    #[tokio::test]
    async fn original_factor_and_invitation_receipts_survive_rewrap_and_revoke() {
        assert_eq!(
            matching(b"12345678901234567890", "287082", 59000, -1),
            Some(1)
        );
        assert_eq!(matching(b"12345678901234567890", "287082", 59000, 1), None);
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("android-auth.sqlite"))
            .await
            .unwrap();
        let identity = store.identity();
        let bootstrap = "b".repeat(64);
        identity
            .initialize_credentials(&bootstrap, &"s".repeat(64))
            .await
            .unwrap();
        let (_, issued) = identity
            .bootstrap(
                &bootstrap,
                "fixture-owner",
                "Fixture owner",
                "SyntheticPassword-2026!",
                "Fixture org",
            )
            .await
            .unwrap();
        let actor = identity.authenticate_session(&issued.token).await.unwrap();
        let keys = Keys;
        let key = KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        };
        let next = KeyId {
            instance: key.instance,
            key: Uuid::new_v4(),
        };
        let setup_op = Uuid::new_v4();
        let setup = store
            .android_security_totp_setup(&keys, key, &actor, setup_op)
            .await
            .unwrap();
        assert_eq!(
            store
                .android_security_totp_setup(&keys, key, &actor, setup_op)
                .await
                .unwrap(),
            setup
        );
        let mut tx = store.pool.begin().await.unwrap();
        let scope = factor_scope(actor.identity_id(), 1);
        let sealed: String =
            sqlx::query_scalar("SELECT sealed FROM android_auth_totp WHERE owner_id=$1")
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        let encoded = open_bearer(&keys, &scope, &serde_json::from_str(&sealed).unwrap()).unwrap();
        let secret = Zeroizing::new(STANDARD.decode(encoded.as_bytes()).unwrap());
        tx.commit().await.unwrap();
        let enable_code = code(&secret, now());
        let enable_op = Uuid::new_v4();
        let recovery = store
            .android_security_factor_operation(
                &keys,
                key,
                &actor,
                enable_op,
                "totp_enable",
                &enable_code,
                None,
            )
            .await
            .unwrap();
        assert_eq!(recovery["recovery_codes"].as_array().unwrap().len(), 10);
        assert_eq!(
            store
                .android_security_factor_operation(
                    &keys,
                    key,
                    &actor,
                    enable_op,
                    "totp_enable",
                    &enable_code,
                    None
                )
                .await
                .unwrap(),
            recovery
        );
        assert!(matches!(
            store
                .android_security_factor_operation(
                    &keys,
                    key,
                    &actor,
                    enable_op,
                    "totp_enable",
                    "000000",
                    None
                )
                .await,
            Err(Error::Conflict)
        ));
        assert!(
            store.android_security_status(&actor).await.unwrap()["session_verified"]
                .as_bool()
                .unwrap()
        );
        assert!(
            identity
                .login("fixture-owner", "SyntheticPassword-2026!", 3_600_000)
                .await
                .is_err()
        );
        let device = Uuid::new_v4();
        let recovery_code = recovery["recovery_codes"][0].as_str().unwrap();
        let login_op = Uuid::new_v4();
        let tokens = store
            .android_security_login_tokens(
                &keys,
                key,
                "fixture-owner",
                "SyntheticPassword-2026!",
                &device.to_string(),
                Some(recovery_code),
                3_600_000,
                login_op,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .android_security_login_tokens(
                    &keys,
                    key,
                    "fixture-owner",
                    "SyntheticPassword-2026!",
                    &device.to_string(),
                    Some(recovery_code),
                    3_600_000,
                    login_op
                )
                .await
                .unwrap(),
            tokens
        );
        assert!(
            store
                .android_security_login_tokens(
                    &keys,
                    key,
                    "fixture-owner",
                    "SyntheticPassword-2026!",
                    &device.to_string(),
                    Some(recovery_code),
                    3_600_000,
                    Uuid::new_v4()
                )
                .await
                .is_err()
        );
        let logged = identity
            .authenticate_session(tokens["access_token"].as_str().unwrap())
            .await
            .unwrap();
        assert!(
            store.android_security_status(&logged).await.unwrap()["session_verified"]
                .as_bool()
                .unwrap()
        );
        assert!(matches!(
            store
                .android_security_totp_setup(&keys, key, &logged, setup_op)
                .await,
            Err(Error::Conflict)
        ));
        // Only a still-live, actually privileged issuer may invite. Acceptance
        // creates a fresh unprivileged identity and no membership/private rows.
        let invite_op = Uuid::new_v4();
        let invitation = store
            .android_security_create_invitation(
                &keys,
                key,
                &actor,
                "guest@example.org",
                None,
                invite_op,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .android_security_create_invitation(
                    &keys,
                    key,
                    &actor,
                    "guest@example.org",
                    None,
                    invite_op
                )
                .await
                .unwrap(),
            invitation
        );
        let accept_op = Uuid::new_v4();
        let guest_device = Uuid::new_v4();
        let invitation_token = invitation["token"].as_str().unwrap();
        let guest = store
            .android_security_accept_invitation(
                &keys,
                key,
                invitation_token,
                "Fixture guest",
                "GuestSyntheticPassword-2026!",
                guest_device,
                3_600_000,
                accept_op,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .android_security_accept_invitation(
                    &keys,
                    key,
                    invitation_token,
                    "Fixture guest",
                    "GuestSyntheticPassword-2026!",
                    guest_device,
                    3_600_000,
                    accept_op
                )
                .await
                .unwrap(),
            guest
        );
        assert!(
            store
                .android_security_accept_invitation(
                    &keys,
                    key,
                    invitation_token,
                    "Fixture changed",
                    "GuestSyntheticPassword-2026!",
                    guest_device,
                    3_600_000,
                    accept_op
                )
                .await
                .is_err()
        );
        let guest_actor = identity
            .authenticate_session(guest["access_token"].as_str().unwrap())
            .await
            .unwrap();
        assert!(
            !identity
                .self_identity(&guest_actor)
                .await
                .unwrap()
                .platform_admin
        );
        assert!(
            identity
                .organizations(&guest_actor)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .android_security_create_invitation(
                    &keys,
                    key,
                    &guest_actor,
                    "third@example.org",
                    None,
                    Uuid::new_v4()
                )
                .await
                .is_err()
        );
        assert_eq!(
            store.android_security_email(&guest_actor).await.unwrap(),
            "guest@example.org"
        );
        let email_login = store
            .android_security_login_tokens(
                &keys,
                key,
                "guest@example.org",
                "GuestSyntheticPassword-2026!",
                &guest_device.to_string(),
                None,
                3_600_000,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        assert_eq!(email_login["user_id"], guest["user_id"]);
        assert!(
            store
                .android_security_create_invitation(
                    &keys,
                    key,
                    &actor,
                    "guest@example.org",
                    None,
                    Uuid::new_v4()
                )
                .await
                .is_err()
        );
        let email_subject = store
            .android_passkey_start_identity("guest@example.org")
            .await
            .unwrap();
        assert_eq!(email_subject.0, guest_actor.identity_id());
        assert!(
            store
                .android_passkey_start_identity("unknown@example.org")
                .await
                .is_err()
        );
        // Real ciphertext rotation, preserving the original operation response
        // and every business revision. The fixture has no installed KMS registry.
        let mut tx = store.pool.begin().await.unwrap();
        device_writer(&mut tx).await.unwrap();
        assert!(rewrap_helper(&mut tx, &keys, next).await.unwrap() >= 6);
        tx.commit().await.unwrap();
        assert_eq!(
            store
                .android_security_totp_setup(&keys, next, &actor, setup_op)
                .await
                .unwrap(),
            setup
        );
        assert_eq!(
            store
                .android_security_factor_operation(
                    &keys,
                    next,
                    &actor,
                    enable_op,
                    "totp_enable",
                    &enable_code,
                    None
                )
                .await
                .unwrap(),
            recovery
        );
        let mut tx = store.pool.begin().await.unwrap();
        let raw: String = sqlx::query_scalar(
            "SELECT sealed FROM android_auth_operations WHERE owner_id=$1 AND operation_id=$2",
        )
        .bind(actor.identity_id().to_string())
        .bind(enable_op.to_string())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(!raw.contains(recovery_code));
        assert!(!raw.contains("SyntheticPassword"));
        let changed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM android_auth_recovery_codes WHERE owner_id=$1 AND used_at>0",
        )
        .bind(actor.identity_id().to_string())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(changed, 1);
        let failures: i64 =
            sqlx::query_scalar("SELECT failed_attempts FROM android_auth_totp WHERE owner_id=$1")
                .bind(actor.identity_id().to_string())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(failures, 1);
        tx.commit().await.unwrap();
        store
            .android_security_revoke_session(&actor, logged.session_id())
            .await
            .unwrap();
        assert!(
            identity
                .authenticate_session(tokens["access_token"].as_str().unwrap())
                .await
                .is_err()
        );
        assert!(
            store
                .android_security_login_tokens(
                    &keys,
                    next,
                    "fixture-owner",
                    "SyntheticPassword-2026!",
                    &device.to_string(),
                    Some(recovery_code),
                    3_600_000,
                    login_op
                )
                .await
                .is_err()
        );
        store.close().await;
    }
}
