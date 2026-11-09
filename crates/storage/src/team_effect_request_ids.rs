//! Project the original immutable wire ID without rewriting an intent or receipt.
use super::*;

pub(super) async fn backfill(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    // Bound the journal batch; ordinary effect requests are limited to 2 MiB.
    const MAX_INTENT: i64 = 2 * 1024 * 1024 + 8192;
    let oversized: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM team_effect_intents WHERE LENGTH(payload)>$1")
            .bind(MAX_INTENT)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if oversized != 0 {
        return Err(Error::Conflict);
    }
    let mut after = String::new();
    loop {
        let rows = sqlx::query("SELECT id,run_id,slot,attempt,sequence,kind,state,request_sha256,scope_sha256,payload FROM team_effect_intents WHERE id>$1 ORDER BY id LIMIT 8")
            .bind(&after).fetch_all(&mut **tx).await.map_err(storage_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let id: String = row.try_get("id").map_err(storage_error)?;
            let raw: String = row.try_get("payload").map_err(storage_error)?;
            if raw.len() > MAX_INTENT as usize {
                return Err(Error::Conflict);
            }
            let value: Value = parse(&raw)?;
            let view: TeamEffectView =
                serde_json::from_value(value.get("view").cloned().ok_or(Error::Conflict)?)
                    .map_err(storage_error)?;
            if view.id.to_string() != id
                || view.run_id.to_string()
                    != row.try_get::<String, _>("run_id").map_err(storage_error)?
                || view.slot != row.try_get::<String, _>("slot").map_err(storage_error)?
                || view.attempt != row.try_get::<i64, _>("attempt").map_err(storage_error)?
                || view.sequence != row.try_get::<i64, _>("sequence").map_err(storage_error)?
                || view.kind != row.try_get::<String, _>("kind").map_err(storage_error)?
                || view.state != row.try_get::<String, _>("state").map_err(storage_error)?
                || view.request_sha256
                    != row
                        .try_get::<String, _>("request_sha256")
                        .map_err(storage_error)?
                || view.scope_sha256
                    != row
                        .try_get::<String, _>("scope_sha256")
                        .map_err(storage_error)?
                || view.request_id.is_empty()
                || view.request_id.len() > 128
                || !view.request_id.bytes().all(|byte| byte.is_ascii_graphic())
                || view.attempt <= 0
                || view.sequence <= 0
                || value.get("request").is_none()
            {
                return Err(Error::Conflict);
            }
            let updated = sqlx::query(
                "UPDATE team_effect_intents SET request_id=$1 WHERE id=$2 AND request_id IS NULL",
            )
            .bind(&view.request_id)
            .bind(&id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
            if updated.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
            after = id;
        }
    }
    let missing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM team_effect_intents WHERE request_id IS NULL")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    if missing != 0 {
        return Err(Error::Conflict);
    }
    match tx.backend_name() {
        "SQLite" => {
            // SQLite cannot add a NOT NULL constraint after backfill without
            // rebuilding this referenced table. These guards enforce the same
            // invariant while preserving all foreign keys and original bytes.
            for statement in [
                "CREATE TRIGGER team_effect_request_required_insert BEFORE INSERT ON team_effect_intents WHEN NEW.request_id IS NULL OR LENGTH(NEW.request_id)=0 OR LENGTH(NEW.request_id)>128 BEGIN SELECT RAISE(ABORT,'team_effect_request_id_required'); END",
                "CREATE TRIGGER team_effect_request_required_update BEFORE UPDATE OF request_id ON team_effect_intents WHEN NEW.request_id IS NULL OR LENGTH(NEW.request_id)=0 OR LENGTH(NEW.request_id)>128 BEGIN SELECT RAISE(ABORT,'team_effect_request_id_required'); END",
            ] {
                sqlx::query(statement)
                    .execute(&mut **tx)
                    .await
                    .map_err(storage_error)?;
            }
        }
        "PostgreSQL" => {
            sqlx::query("ALTER TABLE team_effect_intents ALTER COLUMN request_id SET NOT NULL")
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
            sqlx::query("ALTER TABLE team_effect_intents ADD CONSTRAINT team_effect_request_required CHECK(LENGTH(request_id)>0 AND LENGTH(request_id)<=128)")
                .execute(&mut **tx).await.map_err(storage_error)?;
        }
        _ => return Err(Error::Conflict),
    }
    Ok(())
}

#[cfg(test)]
#[path = "team_effect_request_ids_tests.rs"]
mod tests;
