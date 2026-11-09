//! A signing epoch distinct from the data-key epoch. A stale live authority
//! cannot publish a new signature after a private operator transition.
use crate::{
    IdentityError, Result,
    audit::{self, AuditAuthority},
    audit_checkpoints as cp,
};
use cowork_contracts::*;
use sqlx::{Any, Row, Transaction};
use uuid::Uuid;
async fn table_exists(tx: &mut Transaction<'_, Any>, name: &str) -> Result<bool> {
    let count:i64=sqlx::query_scalar(match tx.backend_name(){"SQLite"=>"SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=$1","PostgreSQL"=>"SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name=$1",_=>return Err(IdentityError::Corrupt)}).bind(name).fetch_one(&mut **tx).await?;
    if count > 1 {
        return Err(IdentityError::Corrupt);
    }
    Ok(count == 1)
}
async fn legacy_admission(tx: &mut Transaction<'_, Any>, authority: &AuditAuthority) -> Result<()> {
    if authority.epoch() != 0 {
        return Err(IdentityError::Corrupt);
    }
    if table_exists(tx, "server_audit_rotation_plans").await?
        && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM server_audit_rotation_plans")
            .fetch_one(&mut **tx)
            .await?
            != 0
    {
        return Err(IdentityError::Corrupt);
    }
    if table_exists(tx, "server_key_registry").await? {
        if let Some(raw) =
            sqlx::query_scalar::<_, String>("SELECT payload FROM server_key_registry WHERE id=1")
                .fetch_optional(&mut **tx)
                .await?
        {
            if raw.len() > 4 * 1024 * 1024 {
                return Err(IdentityError::Corrupt);
            }
            let v: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| IdentityError::Corrupt)?;
            let stable = &v["stable"];
            if !v["pending"].is_null()
                || !stable["audit_epoch"].is_null()
                || !stable["operator_audit"].is_null()
                || stable["audit"]["instance"].as_str()
                    != Some(authority.key_id().instance.to_string().as_str())
                || stable["audit"]["key"].as_str()
                    != Some(authority.key_id().key.to_string().as_str())
            {
                return Err(IdentityError::Conflict);
            }
        }
    }
    Ok(())
}

pub async fn installed(
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<(Uuid, Uuid, i64, String, Option<Uuid>)>> {
    let exists:i64=sqlx::query_scalar(match tx.backend_name(){"SQLite"=>"SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name='audit_origin_epoch'","PostgreSQL"=>"SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name='audit_origin_epoch'",_=>return Err(IdentityError::Corrupt)}).fetch_one(&mut **tx).await?;
    if exists == 0 {
        if table_exists(tx, "audit_origin_guard").await? {
            return Err(IdentityError::Corrupt);
        }
        return Ok(None);
    }
    if exists != 1 {
        return Err(IdentityError::Corrupt);
    }
    // A permanent registry guard exists even before the first epoch row.
    // Otherwise UPDATE of an absent epoch would not fence a legacy capture.
    // No ServerKeyGuard is acquired after this shared signing guard.
    let guard = if tx.backend_name() == "PostgreSQL" {
        "SELECT id FROM audit_origin_guard WHERE id=1 FOR SHARE"
    } else {
        if sqlx::query("UPDATE audit_origin_guard SET id=id WHERE id=1")
            .execute(&mut **tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(IdentityError::Corrupt);
        }
        "SELECT id FROM audit_origin_guard WHERE id=1"
    };
    if sqlx::query_scalar::<_, i64>(guard)
        .fetch_optional(&mut **tx)
        .await?
        != Some(1)
    {
        return Err(IdentityError::Corrupt);
    }
    // FOR SHARE, not FOR KEY SHARE: non-key epoch updates must conflict. SQLite
    // takes its real writer lock as well.
    if tx.backend_name() == "SQLite" {
        sqlx::query("UPDATE audit_origin_epoch SET epoch=epoch WHERE id=1")
            .execute(&mut **tx)
            .await?;
    }
    let query = if tx.backend_name() == "PostgreSQL" {
        "SELECT * FROM audit_origin_epoch WHERE id=1 FOR SHARE"
    } else {
        "SELECT * FROM audit_origin_epoch WHERE id=1"
    };
    let Some(r) = sqlx::query(query).fetch_optional(&mut **tx).await? else {
        return Ok(None);
    };
    let instance = Uuid::parse_str(&r.try_get::<String, _>("instance_id")?)
        .map_err(|_| IdentityError::Corrupt)?;
    let key =
        Uuid::parse_str(&r.try_get::<String, _>("key_id")?).map_err(|_| IdentityError::Corrupt)?;
    let epoch: i64 = r.try_get("epoch")?;
    let pin: String = r.try_get("public_key_sha256")?;
    let pending = r
        .try_get::<Option<String>, _>("pending_request_id")?
        .map(|v| Uuid::parse_str(&v).map_err(|_| IdentityError::Corrupt))
        .transpose()?;
    if instance.is_nil()
        || key.is_nil()
        || epoch < 0
        || !cp::digest(&pin)
        || pending.is_some_and(|id| id.is_nil())
    {
        return Err(IdentityError::Corrupt);
    }
    Ok(Some((instance, key, epoch, pin, pending)))
}
pub async fn admission(tx: &mut Transaction<'_, Any>, authority: &AuditAuthority) -> Result<()> {
    let current = installed(tx).await?;
    if let Some((instance, key, epoch, pin, pending)) = current {
        let id = authority.key_id();
        if pending.is_some()
            || instance != id.instance
            || key != id.key
            || epoch != authority.epoch()
            || pin != authority.public_key_sha256()?
        {
            return Err(IdentityError::Conflict);
        }
    } else {
        legacy_admission(tx, authority).await?;
    }
    Ok(())
}
pub fn verify_plan(p: &AuditOriginRotationPlan, old_pin: &str, new_pin: &str) -> Result<()> {
    if p.format != "cowork.audit.origin-rotation.v1"
        || p.request_id.is_nil()
        || p.instance_id.is_nil()
        || p.original_audit_key.is_nil()
        || p.next_audit_key.is_nil()
        || p.original_audit_key == p.next_audit_key
        || p.original_epoch < 0
        || p.next_epoch
            != p.original_epoch
                .checked_add(1)
                .ok_or(IdentityError::Corrupt)?
        || p.prepared_at <= 0
        || !cp::digest(&p.original_registry_sha256)
        || p.previous_public_key_sha256 != old_pin
        || p.next_public_key_sha256 != new_pin
        || old_pin == new_pin
        || p.proposals.len() > 128
        || cp::canonical(p)?.len() > 4 * 1024 * 1024
    {
        return Err(IdentityError::Validation);
    }
    let mut scopes = std::collections::BTreeSet::new();
    for proposal in &p.proposals {
        let a = &proposal.previous_binding;
        let b = &proposal.next_binding;
        crate::audit_trust::verify_proposal(
            proposal,
            old_pin,
            new_pin,
            &a.witness_public_key_sha256,
            &b.witness_public_key_sha256,
        )?;
        if a.witness_public_key_sha256 != b.witness_public_key_sha256
            || !scopes.insert(a.scope.clone())
            || proposal.prepared_at < p.prepared_at
        {
            return Err(IdentityError::Validation);
        }
    }
    let mut unsigned = p.clone();
    unsigned.previous_origin_signature.clear();
    unsigned.next_origin_signature.clear();
    audit::verify_metadata(
        "cowork:audit:origin-rotation:v1",
        &p.previous_public_key,
        old_pin,
        &p.previous_origin_signature,
        &unsigned,
    )?;
    audit::verify_metadata(
        "cowork:audit:origin-rotation:v1",
        &p.next_public_key,
        new_pin,
        &p.next_origin_signature,
        &unsigned,
    )
}
pub fn sign_plan(
    mut p: AuditOriginRotationPlan,
    old: &AuditAuthority,
    new: &AuditAuthority,
) -> Result<AuditOriginRotationPlan> {
    p.previous_public_key = old.public_key()?;
    p.previous_public_key_sha256 = old.public_key_sha256()?;
    p.next_public_key = new.public_key()?;
    p.next_public_key_sha256 = new.public_key_sha256()?;
    p.previous_origin_signature.clear();
    p.next_origin_signature.clear();
    p.previous_origin_signature = old.sign_origin_rotation(&p)?.2;
    let mut unsigned = p.clone();
    unsigned.previous_origin_signature.clear();
    p.next_origin_signature = new.sign_origin_rotation(&unsigned)?.2;
    verify_plan(&p, &p.previous_public_key_sha256, &p.next_public_key_sha256)?;
    Ok(p)
}
/// Called after locking all current scoped heads in a stable sorted order. The
/// head set is reread only after exclusive epoch ownership, closing new scopes.
pub async fn lock_heads(tx: &mut Transaction<'_, Any>) -> Result<Vec<String>> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT scope FROM audit_heads ORDER BY scope LIMIT 100001")
            .fetch_all(&mut **tx)
            .await?;
    if ids.len() > 100000 {
        return Err(IdentityError::QuotaExceeded);
    }
    for id in &ids {
        cp::fence(tx, id).await?;
    }
    Ok(ids)
}
pub async fn exclusive(tx: &mut Transaction<'_, Any>, heads: &[String]) -> Result<()> {
    if sqlx::query("UPDATE audit_origin_guard SET id=id WHERE id=1")
        .execute(&mut **tx)
        .await?
        .rows_affected()
        != 1
    {
        return Err(IdentityError::Corrupt);
    }
    sqlx::query("UPDATE audit_origin_epoch SET epoch=epoch WHERE id=1")
        .execute(&mut **tx)
        .await?;
    let actual: Vec<String> =
        sqlx::query_scalar("SELECT scope FROM audit_heads ORDER BY scope LIMIT 100001")
            .fetch_all(&mut **tx)
            .await?;
    if actual != heads {
        return Err(IdentityError::Conflict);
    }
    Ok(())
}
