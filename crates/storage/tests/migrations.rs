use cowork_contracts::*;
use cowork_domain::{Checkpoint, Error, StoredRun, hash};
use cowork_storage::Store;
use serde_json::json;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};

async fn raw(path: &std::path::Path) -> SqlitePool {
    SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn schema113_upgrades_with_empty_observational_completion_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained113.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Retained before original completion custody".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Existing order and draft".into(),
        })
        .await
        .unwrap();
    store.close().await;
    let pool = raw(&path).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_deliveries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TABLE owner_managed_result_deliveries")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM schema_migrations WHERE version=116")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_versions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TABLE owner_managed_result_versions")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM schema_migrations WHERE version=115")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_completion_receipts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TABLE owner_managed_completion_receipts")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM schema_migrations WHERE version=114")
        .execute(&pool)
        .await
        .unwrap();
    let before: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,checksum FROM schema_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(before.last().unwrap().0, 113);
    pool.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.order(&order.id).await.unwrap().title, order.title);
    store.close().await;
    let pool = raw(&path).await;
    let after: Vec<(i64, String)> = sqlx::query_as(
        "SELECT version,checksum FROM schema_migrations WHERE version<=113 ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        116
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_completion_receipts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_versions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    pool.close().await;
}

#[tokio::test]
async fn existing_preview_schema_upgrades_without_changing_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-preview.sqlite3");
    let pool = raw(&path).await;
    let schema = include_str!("../migrations/0001.sql");
    for statement in schema.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES(1,$1)")
        .bind(hash(schema.as_bytes()))
        .execute(&pool)
        .await
        .unwrap();
    let p = Project {
        id: "p".into(),
        name: "Existing private data".into(),
        revision: 1,
        created_at: 1,
    };
    let o = Order {
        id: "o".into(),
        project_id: p.id.clone(),
        title: "Erhaltener Äpfel-Titel".into(),
        draft: "Unfinished draft".into(),
        revision: 3,
        created_at: 1,
    };
    sqlx::query("INSERT INTO projects(id,payload) VALUES($1,$2)")
        .bind(&p.id)
        .bind(serde_json::to_string(&p).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO orders(id,project_id,revision,payload) VALUES($1,$2,$3,$4)")
        .bind(&o.id)
        .bind(&p.id)
        .bind(o.revision)
        .bind(serde_json::to_string(&o).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let record = RunRecord {
        model_attempts: vec![],
        id: "r".into(),
        spec: RunSpec {
            input_workspace: None,
            agent_config: None,
            model_binding: None,
            output: None,
            workflow: None,
            mcp_tools: vec![],
            environment: None,
            folders: vec![],
            order_id: o.id.clone(),
            project_id: p.id.clone(),
            prompt: "legacy prompt".into(),
            provider: Some(ProviderProfile {
                generation: Default::default(),
                id: "test".into(),
                name: "Fixture".into(),
                kind: ProviderKind::Test,
                endpoint: String::new(),
                model: "test".into(),
                revision: 1,
                context_tokens: 32768,
                credential_required: false,
                credential_ready: true,
                capabilities: Default::default(),
            }),
            file_action: None,
            access_mode: AccessMode::ConfirmMutations,
            executor: "local-v2".into(),
            policy_revision: 1,
            allowed_tools: vec![],
            budgets: Budgets::default(),
        },
        state: RunState::Completed,
        revision: 1,
        last_sequence: 3,
        steps: 2,
        created_at: 1,
        updated_at: 5,
        started_at: Some(2),
        error_code: None,
        usage: Usage {
            input_tokens: Some(12),
            output_tokens: Some(3),
            ..Default::default()
        },
    };
    let stored = StoredRun {
        record,
        checkpoint: Checkpoint::default(),
        lease: None,
    };
    sqlx::query("INSERT INTO runs(id,order_id,idempotency_key,request_hash,revision,state,payload) VALUES('r','o','legacy','legacy',1,'completed',$1)")
        .bind(serde_json::to_string(&stored).unwrap()).execute(&pool).await.unwrap();
    let mut original_events = vec![];
    for (sequence, kind, payload) in [
        (1, "model_started", json!({"step":1})),
        (
            2,
            "model_completed",
            json!({"usage":{"input_tokens":12,"output_tokens":3,"cost":null,"price_source_version":null}}),
        ),
        (3, "model_started", json!({"step":2})),
    ] {
        let event = RunEvent {
            schema_version: 4,
            id: format!("old-{sequence}"),
            run_id: "r".into(),
            sequence,
            kind: kind.into(),
            payload,
            created_at: sequence,
        };
        let bytes = serde_json::to_string(&event).unwrap();
        original_events.push(bytes.clone());
        sqlx::query("INSERT INTO events(id,run_id,sequence,payload) VALUES($1,'r',$2,$3)")
            .bind(&event.id)
            .bind(sequence)
            .bind(bytes)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (id, role) in [("a", "user"), ("b", "assistant")] {
        let message = Message {
            id: id.into(),
            order_id: o.id.clone(),
            run_id: "r".into(),
            role: role.into(),
            content: format!("Erhaltene Nachricht {id}"),
            created_at: 4,
        };
        sqlx::query(
            "INSERT INTO messages(id,run_id,order_id,created_at,payload) VALUES($1,'r','o',4,$2)",
        )
        .bind(id)
        .bind(serde_json::to_string(&message).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    let detail = store.detail("r").await.unwrap();
    assert_eq!(detail.run.model_attempts.len(), 2);
    assert_eq!(
        detail.run.model_attempts[0].state,
        ModelAttemptState::Measured
    );
    assert_eq!(
        detail.run.model_attempts[1].state,
        ModelAttemptState::Interrupted
    );
    assert_eq!(detail.token_budget.charged_tokens, 32783);
    assert_eq!(detail.token_budget.unconfirmed_tokens, 32768);
    assert_eq!(detail.run.usage.input_tokens, Some(12));
    assert!(detail.run.usage.incomplete);
    assert_eq!(detail.run.state, RunState::Completed);
    assert!(
        detail
            .run
            .model_attempts
            .iter()
            .all(|a| a.request_sha256.is_none())
    );
    assert_eq!(store.order("o").await.unwrap().draft, "Unfinished draft");
    assert_eq!(
        store.search("äPFEL", None, 0, 10).await.unwrap().items[0].order_id,
        "o"
    );
    assert!(store.history("o").await.unwrap().inherited.is_none());
    let messages = store.messages("o").await.unwrap();
    assert_eq!(
        messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        store
            .search("Nachricht", None, 0, 10)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    store.close().await;
    let pool = raw(&path).await;
    let events: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM events WHERE run_id='r' ORDER BY sequence")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(events, original_events);
    let versions = sqlx::query("SELECT version FROM schema_migrations ORDER BY version")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        versions
            .iter()
            .map(|r| r.get::<i64, _>("version"))
            .collect::<Vec<_>>(),
        {
            let mut versions = std::fs::read_dir(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
            )
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .map(|path| {
                path.file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .parse::<i64>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
            versions.sort();
            versions
        }
    );
    pool.close().await;
    Store::sqlite(&path).await.unwrap().close().await;
}

#[tokio::test]
async fn changed_or_future_schema_is_rejected_without_rewriting_it() {
    for future in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.sqlite3");
        Store::sqlite(&path).await.unwrap().close().await;
        let pool = raw(&path).await;
        if future {
            sqlx::query("INSERT INTO schema_migrations(version,checksum) VALUES(999,'future')")
                .execute(&pool)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE schema_migrations SET checksum='tampered' WHERE version=1")
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
        assert!(matches!(
            Store::sqlite(&path).await,
            Err(Error::Validation(_))
        ));
        let pool = raw(&path).await;
        let value: String =
            sqlx::query_scalar("SELECT checksum FROM schema_migrations WHERE version=$1")
                .bind(if future { 999 } else { 1 })
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(value, if future { "future" } else { "tampered" });
        pool.close().await;
    }
}

async fn remove_post_105_migrations_from_fixture(pool: &sqlx::AnyPool) {
    // These fixtures reconstruct 101/104 from a fresh current database. Remove
    // the later wire projection as well as its receipt, so they are genuine
    // historical prefixes rather than incomplete modern schemas.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(pool)
            .await
            .unwrap(),
        116
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_deliveries")
            .fetch_one(pool)
            .await
            .unwrap(),
        0,
        "Historical fixture must preserve any original delivery intent"
    );
    for statement in [
        "DROP TABLE owner_managed_result_deliveries",
        "DELETE FROM schema_migrations WHERE version=116",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_versions")
            .fetch_one(pool)
            .await
            .unwrap(),
        0,
        "Historical fixture must preserve any original result custody"
    );
    for statement in [
        "DROP TABLE owner_managed_result_versions",
        "DELETE FROM schema_migrations WHERE version=115",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_completion_receipts")
            .fetch_one(pool)
            .await
            .unwrap(),
        0,
        "Historical fixture must preserve any original signed completion history"
    );
    for statement in [
        "DROP TABLE owner_managed_completion_receipts",
        "DELETE FROM schema_migrations WHERE version=114",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_input_transfers")
            .fetch_one(pool)
            .await
            .unwrap(),
        0,
        "Historical fixture must preserve any current transfer custody"
    );
    for statement in [
        "DROP TABLE owner_managed_input_transfers",
        "DELETE FROM schema_migrations WHERE version=113",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    for table in [
        "owner_run_workspaces",
        "owner_execution_plans_v2",
        "broker_controller",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap(),
            0,
            "Historical fixtures must not discard current controller/plan/runspace custody"
        );
    }
    for statement in [
        "DROP TABLE owner_run_workspaces",
        "DROP TABLE owner_execution_plans_v2",
        "DELETE FROM schema_migrations WHERE version=112",
        "DROP TABLE broker_controller",
        "DELETE FROM schema_migrations WHERE version=111",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_execution_inputs")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    for statement in [
        "DROP TABLE owner_execution_inputs",
        "DELETE FROM schema_migrations WHERE version=110",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_timer_wakes")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    for statement in [
        "DROP TABLE workflow_timer_wakes",
        "DELETE FROM schema_migrations WHERE version=109",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_execution_plan_targets")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    for statement in [
        "DROP TABLE owner_execution_plan_targets",
        "DELETE FROM schema_migrations WHERE version=108",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_targets")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    for statement in [
        "DROP TABLE execution_target_revisions",
        "DROP TABLE execution_targets",
        "DROP TABLE execution_target_registry_guard",
        "DELETE FROM schema_migrations WHERE version=107",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM team_effect_intents")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    let backend = {
        let connection = pool.acquire().await.unwrap();
        connection.backend_name().to_owned()
    };
    if backend == "SQLite" {
        for statement in [
            "DROP TRIGGER team_effect_request_required_insert",
            "DROP TRIGGER team_effect_request_required_update",
        ] {
            sqlx::query(statement).execute(pool).await.unwrap();
        }
    }
    for statement in [
        "DROP INDEX team_effect_request_scope",
        "ALTER TABLE team_effect_intents DROP COLUMN request_id",
        "DELETE FROM schema_migrations WHERE version=106",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
}

async fn automation_upgrade_fixture(
    path: &std::path::Path,
    initial: bool,
    corrupt: bool,
) -> (String, String) {
    let store = Store::sqlite(path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Retained automation".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Retained chat".into(),
        })
        .await
        .unwrap();
    store.close().await;
    sqlx::any::install_default_drivers();
    let encoded = percent_encoding::utf8_percent_encode(
        path.to_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    );
    let pool = sqlx::AnyPool::connect(&format!("sqlite:{encoded}?mode=rwc"))
        .await
        .unwrap();
    automation_upgrade_fixture_on(pool, project, order, initial, corrupt).await
}

async fn automation_upgrade_fixture_on(
    pool: sqlx::AnyPool,
    project: Project,
    order: Order,
    initial: bool,
    corrupt: bool,
) -> (String, String) {
    remove_post_105_migrations_from_fixture(&pool).await;
    // Reconstruct the historical prefix, not a ledger with a hole in its sequence.
    for statement in [
        "DROP TABLE owner_execution_plans",
        "DROP TABLE automation_file_watches",
        "DELETE FROM schema_migrations WHERE version>=102",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    let schema = if initial {
        sqlx::query("ALTER TABLE automation_events DROP COLUMN task_id")
            .execute(&pool)
            .await
            .unwrap();
        include_str!("../migration-fixtures/automation-initial.sql")
    } else {
        include_str!("../migrations/0101.sql")
    };
    let checksum = hash(schema.as_bytes());
    sqlx::query("UPDATE schema_migrations SET checksum=$1 WHERE version=101")
        .bind(&checksum)
        .execute(&pool)
        .await
        .unwrap();
    for id in ["original-task", "retargeted-task"] {
        sqlx::query("INSERT INTO native_workspace_tasks(id,project_id,order_id,revision,archived,payload) VALUES($1,$2,$3,1,0,'{}')")
            .bind(id).bind(&project.id).bind(&order.id).execute(&pool).await.unwrap();
    }
    // The trigger has since been retargeted. The receipt must retain the original task.
    sqlx::query("INSERT INTO automation_triggers(id,project_id,task_id,revision,enabled,payload,task_snapshot) VALUES('trigger',$1,'retargeted-task',2,1,'{}','{}')")
        .bind(&project.id).execute(&pool).await.unwrap();
    let event = AutomationEvent {
        workflow_input: None,
        id: "retained-event".into(),
        project_id: project.id.clone(),
        trigger_id: "trigger".into(),
        trigger_revision: 1,
        task_id: "original-task".into(),
        task_revision: 1,
        event_id: "delivery".into(),
        source: AutomationEventSource::Manual,
        data: json!({"preserved":"Grüße"}),
        state: AutomationEventState::Queued,
        run_id: None,
        reason: None,
        received_at: 1,
        updated_at: 1,
    };
    let payload = serde_json::to_string(&event).unwrap();
    let mut receipt = serde_json::to_value(&event).unwrap();
    if corrupt {
        receipt["task_id"] = json!("retargeted-task");
    }
    let receipt = if corrupt {
        serde_json::to_string(&receipt).unwrap()
    } else {
        payload.clone()
    };
    let extra_column = if initial { "" } else { ",task_id" };
    let extra_value = if initial { "" } else { ",'original-task'" };
    sqlx::query(&format!("INSERT INTO automation_events(sequence,id,project_id,trigger_id,source,event_key,request_hash,state,received_at,run_key,payload,ingress_receipt{extra_column}) VALUES(1,'retained-event',$1,'trigger','manual','delivery','request','queued',1,'original-run-key',$2,$3{extra_value})"))
        .bind(&project.id).bind(&payload).bind(receipt).execute(&pool).await.unwrap();
    pool.close().await;
    (checksum, payload)
}

#[tokio::test]
#[ignore = "Requires a new disposable cowork_migration_102_* database in COWORK_TEST_POSTGRES"]
async fn postgres_automation_initial_schema_preserves_frozen_task_and_receipts() {
    let url = std::env::var("COWORK_TEST_POSTGRES").expect("dedicated PostgreSQL URL");
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.strip_prefix("cowork_migration_102_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        "This fixture may only mutate its explicitly named disposable database"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let store = Store::connect(&url).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Retained PostgreSQL automation".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Retained chat".into(),
        })
        .await
        .unwrap();
    store.close().await;
    let (checksum, payload) =
        automation_upgrade_fixture_on(pool, project, order, true, false).await;
    Store::connect(&url).await.unwrap().close().await;
    Store::connect(&url).await.unwrap().close().await;
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let row = sqlx::query(
        "SELECT task_id,payload,ingress_receipt FROM automation_events WHERE id='retained-event'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("task_id"), "original-task");
    assert_eq!(row.get::<String, _>("payload"), payload);
    assert_eq!(row.get::<String, _>("ingress_receipt"), payload);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT checksum FROM schema_migrations WHERE version=101")
            .fetch_one(&pool)
            .await
            .unwrap(),
        checksum
    );
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT is_nullable::text FROM information_schema.columns WHERE table_schema='public' AND table_name='automation_events' AND column_name='task_id'").fetch_one(&pool).await.unwrap(),"NO");
    pool.close().await;
}

#[tokio::test]
async fn automation_initial_and_interim_schemas_upgrade_without_rebinding_events() {
    for initial in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retained-automation.sqlite3");
        let (checksum, payload) = automation_upgrade_fixture(&path, initial, false).await;
        Store::sqlite(&path).await.unwrap().close().await;
        Store::sqlite(&path).await.unwrap().close().await;
        let pool = raw(&path).await;
        let row = sqlx::query("SELECT task_id,payload,ingress_receipt,run_key FROM automation_events WHERE id='retained-event'").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("task_id"), "original-task");
        assert_eq!(row.get::<String, _>("payload"), payload);
        assert_eq!(row.get::<String, _>("ingress_receipt"), payload);
        assert_eq!(row.get::<String, _>("run_key"), "original-run-key");
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT checksum FROM schema_migrations WHERE version=101"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            checksum
        );
        let columns = sqlx::query("PRAGMA table_info(automation_events)")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            columns
                .iter()
                .find(|row| row.get::<String, _>("name") == "task_id")
                .unwrap()
                .get::<i64, _>("notnull"),
            1
        );
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
        pool.close().await;
    }
}

#[tokio::test]
async fn automation_upgrade_rejects_inconsistent_receipts_without_rewriting_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inconsistent-automation.sqlite3");
    let (checksum, payload) = automation_upgrade_fixture(&path, true, true).await;
    assert!(Store::sqlite(&path).await.is_err());
    let pool = raw(&path).await;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT checksum FROM schema_migrations WHERE version=101")
            .fetch_one(&pool)
            .await
            .unwrap(),
        checksum
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT payload FROM automation_events WHERE id='retained-event'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        payload
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM schema_migrations WHERE version=102")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query(
        "UPDATE schema_migrations SET checksum='unknown-automation-draft' WHERE version=101",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    assert!(matches!(
        Store::sqlite(&path).await,
        Err(Error::Validation(_))
    ));
}

async fn owner_execution_upgrade_fixture(
    pool: &sqlx::AnyPool,
    project: &Project,
    order: &Order,
    initial: bool,
) -> String {
    remove_post_105_migrations_from_fixture(pool).await;
    sqlx::query("DROP TABLE owner_execution_plans")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM schema_migrations WHERE version=105")
        .execute(pool)
        .await
        .unwrap();
    let schema = if initial {
        include_str!("../migration-fixtures/owner-execution-initial.sql")
    } else {
        include_str!("../migrations/0104.sql")
    };
    for statement in schema.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    let checksum = hash(schema.as_bytes());
    sqlx::query("UPDATE schema_migrations SET checksum=$1 WHERE version=104")
        .bind(&checksum)
        .execute(pool)
        .await
        .unwrap();
    let extra_column = if initial { "" } else { ",retired" };
    let extra_value = if initial { "" } else { ",1" };
    sqlx::query(&format!("INSERT INTO owner_execution_plans(id,project_id,order_id,idempotency_key,request_sha256,plan_sha256,payload{extra_column}) VALUES('original-plan',$1,$2,'original-request','request-hash','plan-hash','retained-payload'{extra_value})"))
        .bind(&project.id).bind(&order.id).execute(pool).await.unwrap();
    checksum
}

#[tokio::test]
async fn owner_execution_initial_and_retirement_schemas_upgrade_without_rewriting_receipts() {
    for initial in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retained-plans.sqlite3");
        let store = Store::sqlite(&path).await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Retained execution".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Retained plan".into(),
            })
            .await
            .unwrap();
        store.close().await;
        let encoded = percent_encoding::utf8_percent_encode(
            path.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        let pool = sqlx::AnyPool::connect(&format!("sqlite:{encoded}?mode=rwc"))
            .await
            .unwrap();
        let checksum = owner_execution_upgrade_fixture(&pool, &project, &order, initial).await;
        pool.close().await;
        Store::sqlite(&path).await.unwrap().close().await;
        Store::sqlite(&path).await.unwrap().close().await;
        let pool = raw(&path).await;
        let row=sqlx::query("SELECT payload,request_sha256,plan_sha256,retired,run_id,receipt FROM owner_execution_plans WHERE id='original-plan'").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("payload"), "retained-payload");
        assert_eq!(row.get::<String, _>("request_sha256"), "request-hash");
        assert_eq!(row.get::<String, _>("plan_sha256"), "plan-hash");
        assert_eq!(row.get::<i64, _>("retired"), if initial { 0 } else { 1 });
        assert!(
            row.get::<Option<String>, _>("run_id").is_none()
                && row.get::<Option<String>, _>("receipt").is_none()
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT checksum FROM schema_migrations WHERE version=104"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            checksum
        );
        assert!(
            sqlx::query("UPDATE owner_execution_plans SET retired=2 WHERE id='original-plan'")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query(
            "UPDATE schema_migrations SET checksum='unknown-execution-draft' WHERE version=104",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        assert!(matches!(
            Store::sqlite(&path).await,
            Err(Error::Validation(_))
        ));
    }
}

#[tokio::test]
#[ignore = "Requires a new disposable cowork_migration_102_* database in COWORK_TEST_POSTGRES"]
async fn postgres_owner_execution_initial_schema_keeps_plans_and_checksums() {
    let url = std::env::var("COWORK_TEST_POSTGRES").expect("dedicated PostgreSQL URL");
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.strip_prefix("cowork_migration_102_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let store = Store::connect(&url).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Retained PostgreSQL plan".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Retained execution".into(),
        })
        .await
        .unwrap();
    store.close().await;
    let checksum = owner_execution_upgrade_fixture(&pool, &project, &order, true).await;
    pool.close().await;
    Store::connect(&url).await.unwrap().close().await;
    Store::connect(&url).await.unwrap().close().await;
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let row =
        sqlx::query("SELECT payload,retired FROM owner_execution_plans WHERE id='original-plan'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>("payload"), "retained-payload");
    assert_eq!(row.get::<i64, _>("retired"), 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT checksum FROM schema_migrations WHERE version=104")
            .fetch_one(&pool)
            .await
            .unwrap(),
        checksum
    );
    pool.close().await;
}

async fn retained_schema115_tables(
    pool: &SqlitePool,
) -> std::collections::BTreeMap<String, (String, Vec<String>)> {
    let mut result = std::collections::BTreeMap::new();
    let tables: Vec<(String,String)> = sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(pool).await.unwrap();
    for (name, definition) in tables {
        if name == "owner_managed_result_deliveries" {
            continue;
        }
        let identifier = format!("\"{}\"", name.replace('"', "\"\""));
        let columns = sqlx::query(&format!("PRAGMA table_info({identifier})"))
            .fetch_all(pool)
            .await
            .unwrap();
        let quoted = columns
            .iter()
            .map(|row| {
                let name: String = row.try_get("name").unwrap();
                format!("quote(\"{}\")", name.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join("||'|'||");
        assert!(!quoted.is_empty());
        let suffix = if name == "schema_migrations" {
            " WHERE version<=115"
        } else {
            ""
        };
        let mut rows: Vec<String> =
            sqlx::query_scalar(&format!("SELECT {quoted} FROM {identifier}{suffix}"))
                .fetch_all(pool)
                .await
                .unwrap();
        rows.sort();
        result.insert(name, (definition, rows));
    }
    result
}

#[tokio::test]
async fn schema115_adds_delivery_journal_without_changing_existing_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained115.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Retained115 delivery upgrade".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Keep existing order and all history".into(),
        })
        .await
        .unwrap();
    store.close().await;
    let pool = raw(&path).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_deliveries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TABLE owner_managed_result_deliveries")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM schema_migrations WHERE version=116")
        .execute(&pool)
        .await
        .unwrap();
    let before = retained_schema115_tables(&pool).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        115
    );
    pool.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(store.order(&order.id).await.unwrap().title, order.title);
    store.close().await;
    let pool = raw(&path).await;
    let after = retained_schema115_tables(&pool).await;
    assert_eq!(
        before, after,
        "Schema115 definitions, rows and original receipts must remain identical"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        116
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM owner_managed_result_deliveries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    pool.close().await;
}
