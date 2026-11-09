//! A single SQL statement projects the designated personal installation only.
use super::*;
use crate::model_money::{PersonalMoneyConfiguration, PoolPolicy, PoolScope, ReservationState};
use cowork_model_accounting::persistence;
use serde::de::DeserializeOwned;
use sqlx::any::AnyRow;

// READ COMMITTED is deliberately sufficient: configuration, installation and
// ledger come from one statement snapshot, including during atomic activation.
const SNAPSHOT_SQL: &str = "SELECT ledger.format_version AS ledger_format_version, ledger.revision AS ledger_revision, ledger.payload_sha256 AS ledger_payload_sha256, ledger.payload AS ledger_payload, configuration.id AS configuration_id, configuration.payload_sha256 AS configuration_payload_sha256, configuration.payload AS configuration_payload, installation.id AS installation_id, installation.payload_sha256 AS installation_payload_sha256, installation.payload AS installation_payload FROM accounting_ledgers AS ledger LEFT JOIN accounting_personal_configuration AS configuration ON configuration.id=$2 LEFT JOIN accounting_personal_installation AS installation ON installation.id=$3 WHERE ledger.id=$1";

fn optional_payload<T: DeserializeOwned>(row: &AnyRow, prefix: &str) -> Result<Option<T>> {
    let id: Option<String> = row
        .try_get(format!("{prefix}_id").as_str())
        .map_err(storage_error)?;
    let payload: Option<String> = row
        .try_get(format!("{prefix}_payload").as_str())
        .map_err(storage_error)?;
    let digest: Option<String> = row
        .try_get(format!("{prefix}_payload_sha256").as_str())
        .map_err(storage_error)?;
    match (id, payload, digest) {
        (None, None, None) => Ok(None),
        (Some(_), Some(payload), Some(digest)) if hash(payload.as_bytes()) == digest => {
            serde_json::from_str(&payload)
                .map(Some)
                .map_err(|_| Error::Conflict)
        }
        _ => Err(Error::Conflict),
    }
}

impl Store {
    /// Read-only status, never a preflight promise or execution authorization.
    pub async fn personal_money_status(&self) -> Result<PersonalMoneyStatus> {
        let row = sqlx::query(SNAPSHOT_SQL)
            .bind(persistence::LEDGER_ID)
            .bind("personal-money-configuration-v1")
            .bind("personal-installation-v1")
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        let ledger = persistence::decode_snapshot(
            row.try_get("ledger_format_version")
                .map_err(storage_error)?,
            row.try_get("ledger_revision").map_err(storage_error)?,
            &row.try_get::<String, _>("ledger_payload_sha256")
                .map_err(storage_error)?,
            &row.try_get::<String, _>("ledger_payload")
                .map_err(storage_error)?,
        )
        .map_err(|_| Error::Conflict)?;
        let config: Option<PersonalMoneyConfiguration> = optional_payload(&row, "configuration")?;
        let installation: Option<PoolPolicy> = optional_payload(&row, "installation")?;
        let Some(installation) = installation else {
            return if config.is_none() && !ledger.has_active_installation() {
                Ok(PersonalMoneyStatus::NotConfigured {})
            } else {
                Err(Error::Conflict)
            };
        };
        if !matches!(&installation.scope, PoolScope::Installation(id) if !id.trim().is_empty())
            || ledger
                .active_policy(&installation.scope)
                .map_err(|_| Error::Conflict)?
                != &installation
        {
            return Err(Error::Conflict);
        }
        if let Some(config) = &config {
            config.validate().map_err(|_| Error::Conflict)?;
            if config.installation != installation {
                return Err(Error::Conflict);
            }
        }
        let (mut confirmed, mut pending, mut unknown) = (0u64, 0u64, 0u64);
        let (mut confirmed_attempts, mut pending_attempts, mut unknown_attempts) =
            (0u64, 0u64, 0u64);
        let mut overrun = false;
        for reservation in ledger.reservations().values().filter(|reservation| {
            reservation
                .spec
                .pools
                .iter()
                .any(|pool| pool.scope == installation.scope)
        }) {
            let charge = reservation.charged_units().map_err(|_| Error::Conflict)?;
            let (amount, count) = match &reservation.state {
                ReservationState::Settled { .. } if reservation.complete_usage() => {
                    (&mut confirmed, &mut confirmed_attempts)
                }
                ReservationState::Held => (&mut pending, &mut pending_attempts),
                _ => (&mut unknown, &mut unknown_attempts),
            };
            *amount = amount.checked_add(charge).ok_or(Error::Conflict)?;
            *count = count.checked_add(1).ok_or(Error::Conflict)?;
            overrun |= reservation.overrun();
        }
        let charged = confirmed
            .checked_add(pending)
            .and_then(|value| value.checked_add(unknown))
            .ok_or(Error::Conflict)?;
        if charged
            != ledger
                .charged_to(&installation.scope)
                .map_err(|_| Error::Conflict)?
        {
            return Err(Error::Conflict);
        }
        let remaining = installation.limit_units.saturating_sub(charged);
        let status = PersonalMoneyPoolStatus {
            currency: PersonalMoneyCurrency {
                code: installation.currency.code,
                scale: installation.currency.scale,
            },
            policy_version: installation.version.into(),
            limit_units: installation.limit_units.into(),
            charged_units: charged.into(),
            confirmed_units: confirmed.into(),
            pending_units: pending.into(),
            unknown_units: unknown.into(),
            remaining_units: remaining.into(),
            confirmed_attempts: confirmed_attempts.into(),
            pending_attempts: pending_attempts.into(),
            unknown_attempts: unknown_attempts.into(),
            overrun,
            limit_exceeded: charged > installation.limit_units,
            no_headroom: remaining == 0,
        };
        Ok(match config {
            Some(config) => PersonalMoneyStatus::OperatorConfiguration {
                installation: status,
                defaults: PersonalMoneyDefaults {
                    project_limit_units: config.project_limit_units.into(),
                    run_limit_units: config.run_limit_units.into(),
                    pool_policy_version: config.pool_policy_version.into(),
                },
            },
            None => PersonalMoneyStatus::LegacyManual {
                installation: status,
            },
        })
    }
}
