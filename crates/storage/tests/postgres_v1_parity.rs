use cowork_contracts::*;
use cowork_storage::{Store, library_config::LibraryOwner};
use sqlx::{Row, any::AnyPoolOptions};

/// Uses only a disposable database created for this test run. It first creates
/// all guarded schemas, then removes this task's additive storage tables to
/// reconstruct the81 baseline with actual existing content.
#[tokio::test]
#[ignore = "Requires isolated cowork_parity database in COWORK_TEST_POSTGRES"]
async fn postgres_existing81_data_upgrades_and_new_workspace_features_roundtrip() {
    let url = std::env::var("COWORK_TEST_POSTGRES").expect("isolated PostgreSQL URL");
    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        database, "cowork_parity",
        "This fixture may only mutate its dedicated database"
    );
    let store = Store::connect(&url).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Existing81project".into(),
        })
        .await
        .unwrap();
    let chat = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Existing chat".into(),
        })
        .await
        .unwrap();
    let chat = store
        .save_draft(
            &chat.id,
            SaveDraft {
                base_revision: 1,
                draft: "Älterer Entwurf bleibt erhalten".into(),
            },
        )
        .await
        .unwrap();
    let personality = store
        .library_write_personality(
            LibraryOwner::Local,
            None,
            &WritePersonality {
                expected_revision: 0,
                idempotency_key: "old-personality".into(),
                value: PersonalityDefinition {
                    name: "Existing reviewer".into(),
                    system_prompt: "Existing instructions".into(),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    store.close().await;
    let mut migrations =
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .map(|path| {
                (
                    path.file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .parse::<i64>()
                        .unwrap(),
                    path,
                )
            })
            .filter(|(version, _)| *version > 81)
            .collect::<Vec<_>>();
    migrations.sort_by_key(|(version, _)| *version);
    for (_, path) in migrations.iter().rev() {
        let sql = std::fs::read_to_string(path).unwrap();
        let statements = sql.split(';').collect::<Vec<_>>();
        for statement in statements.into_iter().rev() {
            let statement = statement
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join(" ");
            let statement = statement.trim();
            let table = statement
                .strip_prefix("CREATE TABLE IF NOT EXISTS ")
                .or_else(|| statement.strip_prefix("CREATE TABLE "));
            if let Some(table) = table {
                let name = table
                    .split(|c: char| c == '(' || c.is_whitespace())
                    .next()
                    .unwrap();
                assert!(
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                );
                sqlx::query(&format!("DROP TABLE IF EXISTS {name}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            } else {
                assert!(
                    statement.is_empty()
                        || statement.starts_with("CREATE INDEX")
                        || statement.starts_with("CREATE UNIQUE INDEX")
                        || statement.starts_with("INSERT INTO "),
                    "Fixture must explicitly account for non-table migration: {statement}"
                );
            }
        }
    }
    sqlx::query("DELETE FROM schema_migrations WHERE version>81")
        .execute(&pool)
        .await
        .unwrap();
    let upgraded = Store::connect(&url).await.unwrap();
    assert_eq!(upgraded.order(&chat.id).await.unwrap().draft, chat.draft);
    assert!(
        upgraded
            .library_personalities(LibraryOwner::Local)
            .await
            .unwrap()
            .iter()
            .any(|value| value.id == personality.id)
    );
    let independent = upgraded.independent_chat_space().await.unwrap();
    assert!(
        !upgraded
            .projects()
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == independent.id)
    );
    let newchat = upgraded
        .create_order(CreateOrder {
            project_id: independent.id.clone(),
            title: "No project".into(),
        })
        .await
        .unwrap();
    assert!(
        upgraded
            .orders(&independent.id)
            .await
            .unwrap()
            .iter()
            .any(|value| value.id == newchat.id)
    );
    let removal = DeleteLibraryEntry {
        expected_revision: newchat.revision,
        idempotency_key: "postgres-chat-delete".into(),
    };
    let removed = upgraded
        .delete_native_chat(&newchat.id, &removal)
        .await
        .unwrap();
    assert!(removed.deleted);
    assert_eq!(removed.revision, newchat.revision + 1);
    assert!(
        upgraded
            .delete_native_chat(&newchat.id, &removal)
            .await
            .unwrap()
            .deleted
    );
    assert!(
        !upgraded
            .orders(&independent.id)
            .await
            .unwrap()
            .iter()
            .any(|value| value.id == newchat.id)
    );
    assert!(
        !upgraded
            .archived_chats(&independent.id)
            .await
            .unwrap()
            .iter()
            .any(|value| value.order.id == newchat.id)
    );
    assert_eq!(
        upgraded.order(&newchat.id).await.unwrap().title,
        newchat.title
    );
    assert!(upgraded.native_chat_available(&newchat.id).await.is_err());
    let p = upgraded.project_settings(&project.id).await.unwrap();
    let deletion = DeleteLibraryEntry {
        expected_revision: p.revision,
        idempotency_key: "postgres-delete".into(),
    };
    assert!(
        upgraded
            .delete_native_project(&project.id, &deletion)
            .await
            .unwrap()
            .deleted
    );
    assert_eq!(
        upgraded.order(&chat.id).await.unwrap().project_id,
        independent.id
    );
    assert!(
        upgraded
            .delete_native_project(&project.id, &deletion)
            .await
            .unwrap()
            .deleted
    );
    let versions = sqlx::query("SELECT version FROM schema_migrations ORDER BY version")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        versions.last().unwrap().get::<i64, _>("version"),
        migrations.last().unwrap().0
    );
    upgraded.close().await;
    pool.close().await;
}
