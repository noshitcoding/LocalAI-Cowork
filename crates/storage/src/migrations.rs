use super::*;
#[path = "team_effect_request_ids.rs"]
mod team_effect_request_ids;

pub(super) const MIGRATIONS: &[(i64, &str)] = &[
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
    (67, include_str!("../migrations/0067.sql")),
    (68, include_str!("../migrations/0068.sql")),
    (69, include_str!("../migrations/0069.sql")),
    (70, include_str!("../migrations/0070.sql")),
    (71, include_str!("../migrations/0071.sql")),
    (72, include_str!("../migrations/0072.sql")),
    (73, include_str!("../migrations/0073.sql")),
    (74, include_str!("../migrations/0074.sql")),
    (75, include_str!("../migrations/0075.sql")),
    (76, include_str!("../migrations/0076.sql")),
    (77, include_str!("../migrations/0077.sql")),
    (78, include_str!("../migrations/0078.sql")),
    (79, include_str!("../migrations/0079.sql")),
    (80, include_str!("../migrations/0080.sql")),
    (81, include_str!("../migrations/0081.sql")),
    (82, include_str!("../migrations/0082.sql")),
    (83, include_str!("../migrations/0083.sql")),
    (84, include_str!("../migrations/0084.sql")),
    (85, include_str!("../migrations/0085.sql")),
    (86, include_str!("../migrations/0086.sql")),
    (87, include_str!("../migrations/0087.sql")),
    (95, include_str!("../migrations/0095.sql")),
    (96, include_str!("../migrations/0096.sql")),
    (97, include_str!("../migrations/0097.sql")),
    (98, include_str!("../migrations/0098.sql")),
    (99, include_str!("../migrations/0099.sql")),
    (100, include_str!("../migrations/0100.sql")),
    (101, include_str!("../migrations/0101.sql")),
    (102, include_str!("../migrations/0102.sql")),
    (103, include_str!("../migrations/0103.sql")),
    (104, include_str!("../migrations/0104.sql")),
    (105, include_str!("../migrations/0105.sql")),
    (106, include_str!("../migrations/0106.sql")),
    (107, include_str!("../migrations/0107.sql")),
    (108, include_str!("../migrations/0108.sql")),
    (109, include_str!("../migrations/0109.sql")),
    (110, include_str!("../migrations/0110.sql")),
    (111, include_str!("../migrations/0111.sql")),
    (112, include_str!("../migrations/0112.sql")),
    (113, include_str!("../migrations/0113.sql")),
    (114, include_str!("../migrations/0114.sql")),
    (115, include_str!("../migrations/0115.sql")),
    (116, include_str!("../migrations/0116.sql")),
];

// A running development build applied this exact initial automation schema before
// task_id was added to 0101. Keep its receipt intact; 0102 upgrades both variants.
const AUTOMATION_INITIAL_SCHEMA: &str =
    include_str!("../migration-fixtures/automation-initial.sql");
const OWNER_EXECUTION_INITIAL_SCHEMA: &str =
    include_str!("../migration-fixtures/owner-execution-initial.sql");

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
        let known_initial = match version {
            101 => checksum == hash(AUTOMATION_INITIAL_SCHEMA.as_bytes()),
            104 => checksum == hash(OWNER_EXECUTION_INITIAL_SCHEMA.as_bytes()),
            _ => false,
        };
        if version != *expected || (checksum != hash(sql.as_bytes()) && !known_initial) {
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
        if *version == 80 {
            crate::personal_provider_credentials::backfill_versions(tx).await?;
        }
        if *version == 102 {
            backfill_automation_task_bindings(tx).await?;
        }
        if *version == 105 {
            backfill_owner_execution_retirement(tx).await?;
        }
        if *version == 106 {
            team_effect_request_ids::backfill(tx).await?;
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

async fn backfill_owner_execution_retirement(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
) -> Result<()> {
    let checksum: String =
        sqlx::query_scalar("SELECT checksum FROM schema_migrations WHERE version=104")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let retired = if checksum == hash(OWNER_EXECUTION_INITIAL_SCHEMA.as_bytes()) {
        "0"
    } else {
        "retired"
    };
    let query = format!(
        "INSERT INTO owner_execution_plans_v2(id,project_id,order_id,idempotency_key,request_sha256,plan_sha256,payload,run_id,receipt,retired) SELECT id,project_id,order_id,idempotency_key,request_sha256,plan_sha256,payload,run_id,receipt,{retired} FROM owner_execution_plans"
    );
    sqlx::query(&query)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    for statement in [
        "DROP TABLE owner_execution_plans",
        "ALTER TABLE owner_execution_plans_v2 RENAME TO owner_execution_plans",
        "CREATE INDEX owner_execution_plans_order ON owner_execution_plans(order_id,id)",
    ] {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

async fn backfill_automation_task_bindings(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
) -> Result<()> {
    let checksum: String =
        sqlx::query_scalar("SELECT checksum FROM schema_migrations WHERE version=101")
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let json = |column: &str, key: &str| match tx.backend_name() {
        "SQLite" => format!("json_extract(e.{column},'$.{key}')"),
        "PostgreSQL" => format!("(CAST(e.{column} AS JSONB)->>'{key}')"),
        _ => unreachable!("Store supports SQLite and PostgreSQL only"),
    };
    let task = json("ingress_receipt", "task_id");
    // Use the immutable ingress task, never a trigger which may have been retargeted.
    // Invalid or inconsistent receipts fail the NOT NULL/FK constraint transactionally.
    let mut conditions = vec![
        format!("{task}={}", json("payload", "task_id")),
        format!(
            "EXISTS(SELECT 1 FROM native_workspace_tasks t WHERE t.id={task} AND t.project_id=e.project_id)"
        ),
    ];
    for column in ["ingress_receipt", "payload"] {
        for (field, binding) in [
            ("id", "id"),
            ("project_id", "project_id"),
            ("trigger_id", "trigger_id"),
        ] {
            conditions.push(format!("{}=e.{binding}", json(column, field)));
        }
    }
    if checksum != hash(AUTOMATION_INITIAL_SCHEMA.as_bytes()) {
        conditions.push(format!("e.task_id={task}"));
    }
    let query = format!(
        "INSERT INTO automation_events_v2(sequence,id,project_id,trigger_id,task_id,source,event_key,request_hash,state,received_at,run_key,run_id,run_request,run_request_hash,payload,ingress_receipt) SELECT e.sequence,e.id,e.project_id,e.trigger_id,CASE WHEN {} THEN {task} ELSE NULL END,e.source,e.event_key,e.request_hash,e.state,e.received_at,e.run_key,e.run_id,e.run_request,e.run_request_hash,e.payload,e.ingress_receipt FROM automation_events e",
        conditions.join(" AND ")
    );
    sqlx::query(&query)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    for statement in [
        "DROP TABLE automation_events",
        "ALTER TABLE automation_events_v2 RENAME TO automation_events",
        "CREATE INDEX automation_events_queue ON automation_events(state,sequence)",
        "CREATE INDEX automation_events_history ON automation_events(project_id,trigger_id,sequence)",
    ] {
        sqlx::query(statement)
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
