//! Explicit retirement of a broken projection. The exact old SQL text is
//! archived atomically; a separate new chain never claims repaired provenance.
use crate::{
    IdentityError, Result,
    audit::{self, AuditAuthority},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_contracts::*;
use serde::Serialize;
use sqlx::{Any, Row, Transaction};
use uuid::Uuid;

#[derive(Serialize)]
struct OriginalHead {
    chain_id: String,
    sequence: i64,
    head_sha256: String,
}
#[derive(Serialize)]
struct OriginalRecord {
    chain_id: String,
    sequence: i64,
    event_id: String,
    payload: String,
    sha256: String,
}
#[derive(Serialize)]
struct OriginalSnapshot {
    format: &'static str,
    scope: String,
    head: OriginalHead,
    records: Vec<OriginalRecord>,
}
async fn snapshot(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
) -> Result<(AuditRecoveryPlan, Vec<u8>, Vec<String>)> {
    if sqlx::query("UPDATE audit_heads SET sequence=sequence WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?
        .rows_affected()
        != 1
    {
        return Err(IdentityError::NotFound);
    }
    let head=sqlx::query("SELECT chain_id,sequence,head_sha256,LENGTH(chain_id)+LENGTH(head_sha256) AS characters FROM audit_heads WHERE scope=$1").bind(scope).fetch_one(&mut **tx).await?;
    if head.try_get::<i64, _>("characters")? > 4096 {
        return Err(IdentityError::QuotaExceeded);
    }
    let count=sqlx::query("SELECT COUNT(*) AS total,COALESCE(SUM(LENGTH(payload)+LENGTH(event_id)+LENGTH(chain_id)+LENGTH(sha256)),0) AS characters FROM audit_records WHERE scope=$1").bind(scope).fetch_one(&mut **tx).await?;
    if count.try_get::<i64, _>("total")? > 100_000
        || count.try_get::<i64, _>("characters")? > 16 * 1024 * 1024
    {
        return Err(IdentityError::QuotaExceeded);
    }
    let original_head = OriginalHead {
        chain_id: head.try_get("chain_id")?,
        sequence: head.try_get("sequence")?,
        head_sha256: head.try_get("head_sha256")?,
    };
    let mut records = Vec::new();
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let rows = if let Some(after) = cursor {
            sqlx::query("SELECT chain_id,sequence,event_id,payload,sha256 FROM audit_records WHERE scope=$1 AND sequence>$2 ORDER BY sequence LIMIT 256").bind(scope).bind(after).fetch_all(&mut **tx).await?
        } else {
            sqlx::query("SELECT chain_id,sequence,event_id,payload,sha256 FROM audit_records WHERE scope=$1 ORDER BY sequence LIMIT 256").bind(scope).fetch_all(&mut **tx).await?
        };
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let r = OriginalRecord {
                chain_id: row.try_get("chain_id")?,
                sequence: row.try_get("sequence")?,
                event_id: row.try_get("event_id")?,
                payload: row.try_get("payload")?,
                sha256: row.try_get("sha256")?,
            };
            cursor = Some(r.sequence);
            ids.push(r.event_id.clone());
            records.push(r);
        }
    }
    if ids.len() as i64 != count.try_get::<i64, _>("total")? {
        return Err(IdentityError::Corrupt);
    }
    let (corrupt, complete) = match audit::verification(tx, scope).await {
        Ok(v) => (!v.valid, v.complete),
        Err(IdentityError::Corrupt) => (true, false),
        Err(e) => return Err(e),
    };
    let chain = Uuid::parse_str(&original_head.chain_id).ok();
    let prior = original_head.sequence;
    let original = serde_json::to_vec(&OriginalSnapshot {
        format: "cowork.audit.retired-original.v1",
        scope: scope.into(),
        head: original_head,
        records,
    })
    .map_err(|_| IdentityError::Corrupt)?;
    if original.len() > 64 * 1024 * 1024 {
        return Err(IdentityError::QuotaExceeded);
    }
    let plan = AuditRecoveryPlan {
        scope: scope.into(),
        snapshot_sha256: audit::hash(&original),
        prior_chain_id: chain,
        prior_sequence: prior.to_string(),
        records: ids.len() as i64,
        bytes: original.len() as i64,
        corrupt,
        verification_complete: complete,
        public_key_sha256: None,
    };
    Ok((plan, original, ids))
}
pub async fn plan(tx: &mut Transaction<'_, Any>, scope: &str) -> Result<AuditRecoveryPlan> {
    snapshot(tx, scope).await.map(|v| v.0)
}
fn decode_receipt(row: &sqlx::any::AnyRow) -> Result<AuditRecoveryReceipt> {
    let raw: String = row.try_get("receipt")?;
    if audit::hash(raw.as_bytes()) != row.try_get::<String, _>("receipt_sha256")? {
        return Err(IdentityError::Corrupt);
    }
    let out: AuditRecoveryReceipt =
        serde_json::from_str(&raw).map_err(|_| IdentityError::Corrupt)?;
    let mut unsigned = out.clone();
    unsigned.public_key.clear();
    unsigned.public_key_sha256.clear();
    unsigned.signature.clear();
    audit::verify_metadata(
        "cowork:audit:retirement:v1",
        &out.public_key,
        &out.public_key_sha256,
        &out.signature,
        &unsigned,
    )?;
    Ok(out)
}
pub async fn retire(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
    actor: Option<Uuid>,
    authority: &AuditAuthority,
    input: ConfirmAuditRecovery,
    observed_at: i64,
) -> Result<AuditRecoveryReceipt> {
    if input.request_id.is_nil()
        || !input.preserve_original_and_start_new_chain
        || input.snapshot_sha256.len() != 64
        || input
            .snapshot_sha256
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return Err(IdentityError::Validation);
    }
    let request_sha = audit::hash(&serde_json::to_vec(&input).map_err(|_| IdentityError::Corrupt)?);
    // Fence before receipt lookup, including replay; concurrent new appends must
    // wait until the original archive and new head are committed together.
    if sqlx::query("UPDATE audit_heads SET sequence=sequence WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?
        .rows_affected()
        != 1
    {
        return Err(IdentityError::NotFound);
    }
    if let Some(row) =
        sqlx::query("SELECT * FROM audit_retired_chains WHERE scope=$1 AND request_id=$2")
            .bind(scope)
            .bind(input.request_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
    {
        if row.try_get::<String, _>("request_sha256")? != request_sha {
            return Err(IdentityError::Conflict);
        }
        let out = decode_receipt(&row)?;
        if out.scope != scope
            || out.original.scope != scope
            || out.request_id != input.request_id
            || out.original.snapshot_sha256 != input.snapshot_sha256
        {
            return Err(IdentityError::Corrupt);
        }
        let raw: String = row.try_get("original_base64")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| IdentityError::Corrupt)?;
        if bytes.len() > 64 * 1024 * 1024
            || audit::hash(&bytes) != out.original.snapshot_sha256
            || out.original.snapshot_sha256 != row.try_get::<String, _>("original_sha256")?
        {
            return Err(IdentityError::Corrupt);
        }
        return Ok(out);
    }
    crate::audit_origin::admission(tx, authority).await?;
    let (mut original, bytes, ids) = snapshot(tx, scope).await?;
    original.public_key_sha256 = Some(authority.public_key_sha256()?);
    if original.snapshot_sha256 != input.snapshot_sha256 {
        return Err(IdentityError::Conflict);
    }
    if !original.corrupt {
        return Err(IdentityError::Validation);
    }
    let count=sqlx::query("SELECT COUNT(*) AS total,COALESCE(SUM(LENGTH(original_base64)),0) AS bytes FROM audit_retired_chains WHERE scope=$1").bind(scope).fetch_one(&mut **tx).await?;
    if count.try_get::<i64, _>("total")? >= 8
        || count
            .try_get::<i64, _>("bytes")?
            .saturating_add((bytes.len() as i64) * 4 / 3 + 8)
            > 512 * 1024 * 1024
    {
        return Err(IdentityError::QuotaExceeded);
    }
    let mut receipt = AuditRecoveryReceipt {
        format: "cowork.audit.retirement.v1".into(),
        request_id: input.request_id,
        scope: scope.into(),
        captured_at: observed_at,
        original,
        new_chain_id: Uuid::new_v4(),
        actor_id: actor,
        public_key: String::new(),
        public_key_sha256: String::new(),
        signature: String::new(),
    };
    let (public, pin, signature) =
        authority.sign_metadata("cowork:audit:retirement:v1", &receipt)?;
    receipt.public_key = public;
    receipt.public_key_sha256 = pin;
    receipt.signature = signature;
    let raw = serde_json::to_string(&receipt).map_err(|_| IdentityError::Corrupt)?;
    sqlx::query("INSERT INTO audit_retired_chains(scope,request_id,request_sha256,original_base64,original_sha256,receipt,receipt_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(scope).bind(input.request_id.to_string()).bind(request_sha).bind(URL_SAFE_NO_PAD.encode(&bytes)).bind(&receipt.original.snapshot_sha256).bind(&raw).bind(audit::hash(raw.as_bytes())).execute(&mut **tx).await?;
    for id in ids {
        sqlx::query("INSERT INTO audit_retired_sources(scope,event_id) VALUES($1,$2) ON CONFLICT(scope,event_id) DO NOTHING").bind(scope).bind(id).execute(&mut **tx).await?;
    }
    // Only this confirmed, digest-bound operation retires current projection
    // rows. Their exact strings and columns remain in the original archive.
    sqlx::query("DELETE FROM audit_records WHERE scope=$1")
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE audit_heads SET chain_id=$1,sequence=0,head_sha256=$2 WHERE scope=$3")
        .bind(receipt.new_chain_id.to_string())
        .bind("0".repeat(64))
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    audit::append(
        tx,
        scope,
        audit::MetadataEvent {
            event_id: &format!("chain-retirement:{}", receipt.request_id),
            action: "history_chain_retired",
            actor_id: actor.map(|a| a.to_string()),
            target_id: None,
            observed_at,
            origin: AuditOrigin::PreservedArchive,
            source_sha256: Some(receipt.original.snapshot_sha256.clone()),
        },
    )
    .await?;
    Ok(receipt)
}
pub async fn receipts(
    tx: &mut Transaction<'_, Any>,
    scope: &str,
) -> Result<Vec<AuditRecoveryReceipt>> {
    let rows=sqlx::query("SELECT receipt,receipt_sha256 FROM audit_retired_chains WHERE scope=$1 ORDER BY request_id LIMIT 9").bind(scope).fetch_all(&mut **tx).await?;
    if rows.len() > 8 {
        return Err(IdentityError::Corrupt);
    }
    rows.iter()
        .map(|row| {
            let receipt = decode_receipt(row)?;
            if receipt.scope != scope
                || receipt.original.scope != scope
                || !receipt.original.corrupt
                || receipt.original.public_key_sha256.as_deref()
                    != Some(receipt.public_key_sha256.as_str())
            {
                return Err(IdentityError::Corrupt);
            }
            Ok(receipt)
        })
        .collect()
}
pub async fn original(tx: &mut Transaction<'_, Any>, scope: &str, id: Uuid) -> Result<String> {
    let row=sqlx::query("SELECT original_base64,original_sha256 FROM audit_retired_chains WHERE scope=$1 AND request_id=$2").bind(scope).bind(id.to_string()).fetch_optional(&mut **tx).await?.ok_or(IdentityError::NotFound)?;
    let raw: String = row.try_get("original_base64")?;
    if raw.len() > 64 * 1024 * 1024 * 4 / 3 + 8 {
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
            Ok(Zeroizing::new([19; 32]))
        }
    }
    #[tokio::test]
    async fn retirement_preserves_exact_corrupt_sql_and_original_receipts() {
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
        let authority = AuditAuthority::new(
            Arc::new(Keys),
            KeyId {
                instance: Uuid::from_u128(91),
                key: Uuid::from_u128(92),
            },
        )
        .unwrap();
        audit::append(
            &mut tx,
            "personal",
            audit::MetadataEvent {
                event_id: "original-event",
                action: "run_created",
                actor_id: None,
                target_id: None,
                observed_at: 1,
                origin: AuditOrigin::NativeV2,
                source_sha256: None,
            },
        )
        .await
        .unwrap();
        let archive_input = CaptureAuditArchive {
            request_id: Uuid::from_u128(93),
            after_sequence: 0,
            through_sequence: None,
            limit: 500,
        };
        let old_archive = audit::capture(&mut tx, "personal", &authority, archive_input.clone(), 2)
            .await
            .unwrap();
        let malformed = "  original corrupt SQL text: \"\\\n Ω  ";
        sqlx::query("UPDATE audit_records SET payload=$1 WHERE scope='personal'")
            .bind(malformed)
            .execute(&mut *tx)
            .await
            .unwrap();
        let original_plan = plan(&mut tx, "personal").await.unwrap();
        assert!(original_plan.corrupt);
        let input = ConfirmAuditRecovery {
            request_id: Uuid::from_u128(94),
            snapshot_sha256: original_plan.snapshot_sha256.clone(),
            preserve_original_and_start_new_chain: true,
        };
        let mut changed = input.clone();
        changed.snapshot_sha256 = "f".repeat(64);
        assert!(matches!(
            retire(&mut tx, "personal", None, &authority, changed, 3).await,
            Err(IdentityError::Conflict)
        ));
        let receipt = retire(&mut tx, "personal", None, &authority, input.clone(), 3)
            .await
            .unwrap();
        assert_ne!(Some(receipt.new_chain_id), receipt.original.prior_chain_id);
        let exact = URL_SAFE_NO_PAD
            .decode(
                original(&mut tx, "personal", input.request_id)
                    .await
                    .unwrap(),
            )
            .unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&exact).unwrap();
        assert_eq!(snapshot["records"][0]["payload"].as_str(), Some(malformed));
        assert_eq!(audit::hash(&exact), original_plan.snapshot_sha256);
        assert_eq!(
            serde_json::to_value(&receipt).unwrap(),
            serde_json::to_value(
                retire(&mut tx, "personal", None, &authority, input, 4)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(
            serde_json::to_value(&old_archive).unwrap(),
            serde_json::to_value(
                audit::capture(&mut tx, "personal", &authority, archive_input, 5)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        let check = audit::verification(&mut tx, "personal").await.unwrap();
        assert!(check.valid && check.complete);
        assert_eq!(check.head_sequence, 1);
        assert_eq!(receipts(&mut tx, "personal").await.unwrap().len(), 1);
        tx.commit().await.unwrap();
        pool.close().await;
    }
}
