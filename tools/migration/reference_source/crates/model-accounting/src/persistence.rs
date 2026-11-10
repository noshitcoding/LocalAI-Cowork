//! Portable SQLite/PostgreSQL ledger mutation inside the caller's transaction.
//! This module never commits or starts a second transaction on the caller's behalf.
use crate::money::MonetaryLedger;
use crate::{Error, Result, nonempty, sha256, valid_hash};
use serde::{Deserialize, Serialize};
use sqlx::{Any, Row, Transaction};

pub const LEDGER_ID: &str = "model-accounting-global-v1";
const FORMAT: i64 = 1;

fn storage(error: sqlx::Error) -> Error {
    Error::Storage(error.to_string())
}

/// Caller runs this under its central storage migration lock/transaction.
pub async fn migrate(tx: &mut Transaction<'_, Any>) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS accounting_ledgers (id TEXT PRIMARY KEY, format_version BIGINT NOT NULL, revision BIGINT NOT NULL, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)")
        .execute(&mut **tx).await.map_err(storage)?;
    sqlx::query("CREATE TABLE IF NOT EXISTS accounting_operations (id TEXT PRIMARY KEY, ledger_id TEXT NOT NULL, format_version BIGINT NOT NULL, kind TEXT NOT NULL, request_sha256 TEXT NOT NULL, accepted_ledger_revision BIGINT NOT NULL, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)")
        .execute(&mut **tx).await.map_err(storage)?;
    let payload = serde_json::to_string(&MonetaryLedger::default())
        .map_err(|e| Error::Invalid(e.to_string()))?;
    sqlx::query("INSERT INTO accounting_ledgers(id,format_version,revision,payload_sha256,payload) VALUES($1,$2,0,$3,$4) ON CONFLICT(id) DO NOTHING")
        .bind(LEDGER_ID).bind(FORMAT).bind(sha256(payload.as_bytes())).bind(payload)
        .execute(&mut **tx).await.map_err(storage)?;
    load_locked(tx).await?;
    Ok(())
}

/// A writer lock is acquired before reading any ledger balance. Keep this lock
/// before run/attempt/clock locks in the integrating storage lock-order contract.
pub async fn load_locked(tx: &mut Transaction<'_, Any>) -> Result<LockedLedger> {
    if sqlx::query("UPDATE accounting_ledgers SET revision=revision WHERE id=$1")
        .bind(LEDGER_ID)
        .execute(&mut **tx)
        .await
        .map_err(storage)?
        .rows_affected()
        != 1
    {
        return Err(Error::Conflict);
    }
    let (ledger, original_revision, original_payload_hash) = read_checked(tx).await?;
    Ok(LockedLedger {
        ledger,
        original_revision,
        original_payload_hash,
    })
}

/// Pure preflight/history read. It grants neither a write lock nor IO authority;
/// actual admission must reload with load_locked in its execution transaction.
pub async fn snapshot(tx: &mut Transaction<'_, Any>) -> Result<MonetaryLedger> {
    Ok(read_checked(tx).await?.0)
}

async fn read_checked(tx: &mut Transaction<'_, Any>) -> Result<(MonetaryLedger, i64, String)> {
    let row = sqlx::query(
        "SELECT format_version,revision,payload_sha256,payload FROM accounting_ledgers WHERE id=$1",
    )
    .bind(LEDGER_ID)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let format: i64 = row.try_get("format_version").map_err(storage)?;
    let revision: i64 = row.try_get("revision").map_err(storage)?;
    let hash: String = row.try_get("payload_sha256").map_err(storage)?;
    let payload: String = row.try_get("payload").map_err(storage)?;
    if format != FORMAT || revision < 0 || sha256(payload.as_bytes()) != hash {
        return Err(Error::Conflict);
    }
    let ledger: MonetaryLedger =
        serde_json::from_str(&payload).map_err(|e| Error::Invalid(e.to_string()))?;
    ledger.validate()?;
    if ledger.revision() != revision as u64 {
        return Err(Error::Conflict);
    }
    Ok((ledger, revision, hash))
}

pub struct LockedLedger {
    pub ledger: MonetaryLedger,
    original_revision: i64,
    original_payload_hash: String,
}

impl LockedLedger {
    /// Persist with the caller's model attempt, run revision and event before the
    /// same transaction commits. This method never returns an IO permit itself.
    pub async fn persist(self, tx: &mut Transaction<'_, Any>) -> Result<()> {
        self.ledger.validate()?;
        let revision = i64::try_from(self.ledger.revision()).map_err(|_| Error::Overflow)?;
        if revision < self.original_revision {
            return Err(Error::Conflict);
        }
        let payload =
            serde_json::to_string(&self.ledger).map_err(|e| Error::Invalid(e.to_string()))?;
        let hash = sha256(payload.as_bytes());
        if revision == self.original_revision && hash != self.original_payload_hash {
            return Err(Error::Conflict);
        }
        if sqlx::query("UPDATE accounting_ledgers SET revision=$1,payload_sha256=$2,payload=$3 WHERE id=$4 AND format_version=$5 AND revision=$6 AND payload_sha256=$7")
            .bind(revision).bind(hash).bind(payload).bind(LEDGER_ID).bind(FORMAT).bind(self.original_revision).bind(self.original_payload_hash)
            .execute(&mut **tx).await.map_err(storage)?.rows_affected() != 1
        { return Err(Error::StaleRevision); }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingOperationKind {
    PriceRegistration,
    PoolActivation,
    Admission,
    Uncertainty,
    Settlement,
    Compaction,
}

impl AccountingOperationKind {
    fn name(&self) -> &'static str {
        match self {
            Self::PriceRegistration => "price_registration",
            Self::PoolActivation => "pool_activation",
            Self::Admission => "admission",
            Self::Uncertainty => "uncertainty",
            Self::Settlement => "settlement",
            Self::Compaction => "compaction",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingOperation {
    pub id: String,
    pub kind: AccountingOperationKind,
    pub request_sha256: String,
    pub accepted_ledger_revision: u64,
    pub receipt: serde_json::Value,
}

impl AccountingOperation {
    fn validate(&self) -> Result<()> {
        if !nonempty(&self.id)
            || !valid_hash(&self.request_sha256)
            || self.accepted_ledger_revision == 0
        {
            return Err(Error::Invalid(
                "invalid immutable accounting operation".into(),
            ));
        }
        Ok(())
    }
}

/// Read-only receipts never authorize another model request.
pub async fn operation(
    tx: &mut Transaction<'_, Any>,
    id: &str,
) -> Result<Option<AccountingOperation>> {
    let Some(row)=sqlx::query("SELECT ledger_id,format_version,kind,request_sha256,accepted_ledger_revision,payload_sha256,payload FROM accounting_operations WHERE id=$1")
        .bind(id).fetch_optional(&mut **tx).await.map_err(storage)? else {return Ok(None);};
    let payload: String = row.try_get("payload").map_err(storage)?;
    let hash: String = row.try_get("payload_sha256").map_err(storage)?;
    if row.try_get::<String, _>("ledger_id").map_err(storage)? != LEDGER_ID
        || row.try_get::<i64, _>("format_version").map_err(storage)? != FORMAT
        || sha256(payload.as_bytes()) != hash
    {
        return Err(Error::Conflict);
    }
    let value: AccountingOperation =
        serde_json::from_str(&payload).map_err(|e| Error::Invalid(e.to_string()))?;
    value.validate()?;
    if value.id != id
        || row.try_get::<String, _>("kind").map_err(storage)? != value.kind.name()
        || row
            .try_get::<String, _>("request_sha256")
            .map_err(storage)?
            != value.request_sha256
        || row
            .try_get::<i64, _>("accepted_ledger_revision")
            .map_err(storage)?
            != i64::try_from(value.accepted_ledger_revision).map_err(|_| Error::Overflow)?
    {
        return Err(Error::Conflict);
    }
    Ok(Some(value))
}

/// Call only while holding load_locked's lock. Immutable exact replay returns
/// false; a conflicting ID cannot change an old receipt or grant dispatch.
pub async fn record_operation(
    tx: &mut Transaction<'_, Any>,
    value: &AccountingOperation,
) -> Result<bool> {
    value.validate()?;
    if let Some(old) = operation(tx, &value.id).await? {
        return if old == *value {
            Ok(false)
        } else {
            Err(Error::Conflict)
        };
    }
    let row_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM accounting_ledgers WHERE id=$1")
            .bind(LEDGER_ID)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
    let revision = i64::try_from(value.accepted_ledger_revision).map_err(|_| Error::Overflow)?;
    if revision > row_revision {
        return Err(Error::Conflict);
    }
    let payload = serde_json::to_string(value).map_err(|e| Error::Invalid(e.to_string()))?;
    sqlx::query("INSERT INTO accounting_operations(id,ledger_id,format_version,kind,request_sha256,accepted_ledger_revision,payload_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(&value.id).bind(LEDGER_ID).bind(FORMAT).bind(value.kind.name()).bind(&value.request_sha256).bind(revision).bind(sha256(payload.as_bytes())).bind(payload)
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(true)
}
