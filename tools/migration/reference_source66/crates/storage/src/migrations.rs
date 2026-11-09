use super::*;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001.sql")),
    (2, include_str!("../migrations/0002.sql")),
    (3, include_str!("../migrations/0003.sql")),
    (4, include_str!("../migrations/0004.sql")),
    (5, include_str!("../migrations/0005.sql")),
    (6, include_str!("../migrations/0006.sql")),
    (7, include_str!("../migrations/0007.sql")),
    (8, include_str!("../migrations/0008.sql")),
    (9, include_str!("../migrations/0009.sql")),
    (10, include_str!("../migrations/0010.sql")),
    (11, include_str!("../migrations/0011.sql")),
    (12, include_str!("../migrations/0012.sql")),
    (13, include_str!("../migrations/0013.sql")),
    (14, include_str!("../migrations/0014.sql")),
    (15, include_str!("../migrations/0015.sql")),
    (16, include_str!("../migrations/0016.sql")),
    (17, include_str!("../migrations/0017.sql")),
    (18, include_str!("../migrations/0018.sql")),
    (19, include_str!("../migrations/0019.sql")),
    (20, include_str!("../migrations/0020.sql")),
    (21, include_str!("../migrations/0021.sql")),
    (22, include_str!("../migrations/0022.sql")),
    (23, include_str!("../migrations/0023.sql")),
    (24, include_str!("../migrations/0024.sql")),
    (25, include_str!("../migrations/0025.sql")),
    (26, include_str!("../migrations/0026.sql")),
    (27, include_str!("../migrations/0027.sql")),
    (28, include_str!("../migrations/0028.sql")),
    (29, include_str!("../migrations/0029.sql")),
    (30, include_str!("../migrations/0030.sql")),
    (31, include_str!("../migrations/0031.sql")),
    (32, include_str!("../migrations/0032.sql")),
    (33, include_str!("../migrations/0033.sql")),
    (34, include_str!("../migrations/0034.sql")),
    (35, include_str!("../migrations/0035.sql")),
    (36, include_str!("../migrations/0036.sql")),
    (37, include_str!("../migrations/0037.sql")),
    (38, include_str!("../migrations/0038.sql")),
    (39, include_str!("../migrations/0039.sql")),
    (40, include_str!("../migrations/0040.sql")),
    (41, include_str!("../migrations/0041.sql")),
    (42, include_str!("../migrations/0042.sql")),
    (43, include_str!("../migrations/0043.sql")),
    (44, include_str!("../migrations/0044.sql")),
    (45, include_str!("../migrations/0045.sql")),
    (46, include_str!("../migrations/0046.sql")),
    (47, include_str!("../migrations/0047.sql")),
    (48, include_str!("../migrations/0048.sql")),
    (49, include_str!("../migrations/0049.sql")),
    (50, include_str!("../migrations/0050.sql")),
    (51, include_str!("../migrations/0051.sql")),
    (52, include_str!("../migrations/0052.sql")),
    (53, include_str!("../migrations/0053.sql")),
    (54, include_str!("../migrations/0054.sql")),
    (55, include_str!("../migrations/0055.sql")),
    (56, include_str!("../migrations/0056.sql")),
    (57, include_str!("../migrations/0057.sql")),
    (58, include_str!("../migrations/0058.sql")),
    (59, include_str!("../migrations/0059.sql")),
    (60, include_str!("../migrations/0060.sql")),
    (61, include_str!("../migrations/0061.sql")),
    (62, include_str!("../migrations/0062.sql")),
    (63, include_str!("../migrations/0063.sql")),
    (64, include_str!("../migrations/0064.sql")),
    (65, include_str!("../migrations/0065.sql")),
    (66, include_str!("../migrations/0066.sql")),
];

pub(super) async fn apply(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version BIGINT PRIMARY KEY, checksum TEXT NOT NULL)")
        .execute(&mut **tx).await.map_err(storage_error)?;
    let existing = sqlx::query("SELECT version,checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    for (index, row) in existing.iter().enumerate() {
        let version: i64 = row.try_get("version").map_err(storage_error)?;
        let checksum: String = row.try_get("checksum").map_err(storage_error)?;
        let Some((expected, sql)) = MIGRATIONS.get(index) else {
            return Err(Error::Validation(
                "Database schema is newer than this application".into(),
            ));
        };
        if version != *expected || checksum != hash(sql.as_bytes()) {
            return Err(Error::Validation(
                "Database migration sequence or checksum mismatch".into(),
            ));
        }
    }
    for (version, sql) in MIGRATIONS.iter().skip(existing.len()) {
        for statement in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            sqlx::query(statement)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        if *version == 2 {
            backfill_history(tx).await?;
        }
        if *version == 9 {
            backfill_accounting(tx).await?;
        }
        if *version == 17 {
            workflows::product::refresh_all_attention(tx).await?;
        }
        if *version == 40 {
            crate::knowledge_provenance::backfill_personal(tx).await?;
            crate::knowledge_provenance::backfill_team(tx).await?;
        }
        if *version == 66 {
            crate::historical_imports::product::backfill_key_references(tx).await?;
        }
        sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES($1,$2)")
            .bind(version)
            .bind(hash(sql.as_bytes()))
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

async fn backfill_accounting(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    for row in sqlx::query("SELECT id,payload FROM runs")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let run_id: String = row.try_get("id").map_err(storage_error)?;
        let mut stored: StoredRun =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if !stored.record.model_attempts.is_empty() {
            return Err(Error::Validation("Unversioned model accounting".into()));
        }
        let Some(provider) = &stored.record.spec.provider else {
            continue;
        };
        let reserved = provider.context_tokens as u64;
        for event in sqlx::query("SELECT payload FROM events WHERE run_id=$1 ORDER BY sequence")
            .bind(&run_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(storage_error)?
        {
            let event: RunEvent = parse(
                &event
                    .try_get::<String, _>("payload")
                    .map_err(storage_error)?,
            )?;
            if event.kind == "model_started" {
                interrupt_models(&mut stored.record, event.created_at);
                stored.record.model_attempts.push(ModelAttempt {
                    id: format!("legacy:{}", event.id),
                    step: event.payload["step"].as_u64().unwrap_or(0) as u32,
                    request_sha256: None,
                    reserved_tokens: reserved,
                    state: ModelAttemptState::InFlight,
                    usage: Usage::default(),
                    started_at: event.created_at,
                    finished_at: None,
                    limit_exceeded: false,
                });
            } else if event.kind == "model_completed" {
                let usage: Usage = serde_json::from_value(event.payload["usage"].clone())
                    .map_err(storage_error)?;
                if let Some(attempt) = stored
                    .record
                    .model_attempts
                    .iter_mut()
                    .rev()
                    .find(|a| a.state == ModelAttemptState::InFlight)
                {
                    attempt.state = if usage.input_tokens.is_some() && usage.output_tokens.is_some()
                    {
                        ModelAttemptState::Measured
                    } else {
                        ModelAttemptState::Unmeasured
                    };
                    attempt.usage = usage;
                    attempt.finished_at = Some(event.created_at);
                    attempt.limit_exceeded = measured_tokens(&attempt.usage) > reserved
                        || attempt.usage.output_tokens.is_some_and(|n| {
                            n > stored.record.spec.budgets.max_output_tokens as u64
                        });
                }
            }
        }
        let updated = stored.record.updated_at;
        interrupt_models(&mut stored.record, updated);
        if stored.record.model_attempts.is_empty() && stored.record.steps > 0 {
            stored.record.model_attempts.push(ModelAttempt {
                id: format!("legacy:{run_id}"),
                step: 0,
                request_sha256: None,
                reserved_tokens: reserved,
                state: ModelAttemptState::Unmeasured,
                usage: stored.record.usage.clone(),
                started_at: stored.record.created_at,
                finished_at: Some(stored.record.updated_at),
                limit_exceeded: false,
            });
        }
        stored.record.usage.incomplete |= stored
            .record
            .model_attempts
            .iter()
            .any(|a| a.state != ModelAttemptState::Measured);
        sqlx::query("UPDATE runs SET payload=$1 WHERE id=$2")
            .bind(json_string(&stored)?)
            .bind(run_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

async fn backfill_history(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    let orders = sqlx::query("SELECT payload FROM orders ORDER BY id")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
    for row in orders {
        let order: Order = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        history::index_document(
            tx,
            &order.id,
            &order.id,
            "order",
            &order.title,
            order.created_at,
        )
        .await?;
        let messages =
            sqlx::query("SELECT payload FROM messages WHERE order_id=$1 ORDER BY created_at,id")
                .bind(&order.id)
                .fetch_all(&mut **tx)
                .await
                .map_err(storage_error)?;
        for (index, row) in messages.into_iter().enumerate() {
            let m: Message = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            sqlx::query("UPDATE messages SET position=$1 WHERE id=$2")
                .bind(index as i64 + 1)
                .bind(&m.id)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
            history::index_document(tx, &m.id, &m.order_id, "message", &m.content, m.created_at)
                .await?;
        }
    }
    for row in sqlx::query("SELECT payload FROM artifacts")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let a: Artifact = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        history::index_document(
            tx,
            &a.id,
            &a.order_id,
            "artifact",
            &format!("{}\n{}", a.name, a.content),
            a.created_at,
        )
        .await?;
    }
    for row in sqlx::query("SELECT payload FROM runs")
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?
    {
        let r: StoredRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        sqlx::query("UPDATE runs SET updated_at=$1 WHERE id=$2")
            .bind(r.record.updated_at)
            .bind(r.record.id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    sqlx::query("CREATE UNIQUE INDEX messages_position ON messages(order_id,position)")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    Ok(())
}
