use super::*;
use sqlx::any::AnyPoolOptions;

#[derive(Clone, Copy)]
enum Corruption {
    None,
    MissingWire,
    WrongIdentity,
    DuplicateWire,
}
struct Original {
    id: String,
    wire: String,
    payload: String,
    receipt: String,
}

async fn pool(postgres: bool) -> sqlx::AnyPool {
    sqlx::any::install_default_drivers();
    let url = if postgres {
        let url = std::env::var("COWORK_TEST_POSTGRES")
            .expect("separate disposable PostgreSQL database required");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert!(
            parsed.host_str() == Some("127.0.0.1")
                && parsed.path().starts_with("/cowork_effect_request_")
        );
        url
    } else {
        "sqlite::memory:".into()
    };
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    if postgres {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema()",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "use a newly created test database");
    }
    sqlx::query("CREATE TABLE team_runs(id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    for statement in include_str!("../migrations/0038.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool
}
async fn seed(pool: &sqlx::AnyPool, corruption: Corruption) -> Vec<Original> {
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO team_runs(id) VALUES($1)")
        .bind(run.to_string())
        .execute(pool)
        .await
        .unwrap();
    let mut saved = Vec::new();
    let shared = Uuid::new_v4().to_string();
    // More than one backfill batch, including every JSON request shape.
    for n in 0..17 {
        let request = match n % 3 {
            0 => Value::Null,
            1 => json!(["synthetic", n]),
            _ => json!({"proof":n}),
        };
        let scope = hash(b"original-scope");
        let wire = if matches!(corruption, Corruption::DuplicateWire) {
            shared.clone()
        } else {
            Uuid::new_v4().to_string()
        };
        let view = TeamEffectView {
            id: Uuid::new_v4(),
            run_id: run,
            slot: format!("original/model/{n}"),
            attempt: 1,
            sequence: n + 1,
            kind: "model".into(),
            state: "completed".into(),
            request_sha256: hash(
                serde_json::to_string(&(&scope, "model", &request))
                    .unwrap()
                    .as_bytes(),
            ),
            request_id: wire.clone(),
            scope_sha256: scope.clone(),
            provider_revision: 1,
            created_at: 1,
        };
        let mut value = json!({"view":view,"request":request});
        if n == 0 {
            match corruption {
                Corruption::MissingWire => {
                    value["view"].as_object_mut().unwrap().remove("request_id");
                }
                Corruption::WrongIdentity => value["view"]["id"] = json!(Uuid::new_v4()),
                _ => {}
            }
        }
        let raw = serde_json::to_string_pretty(&value).unwrap();
        sqlx::query("INSERT INTO team_effect_intents(id,run_id,slot,attempt,sequence,kind,state,request_sha256,scope_sha256,dispatch_owner,permit_token,payload) VALUES($1,$2,$3,1,$4,'model','completed',$5,$6,'fixture-owner','fixture-permit',$7)")
            .bind(view.id.to_string()).bind(run.to_string()).bind(&view.slot).bind(view.sequence).bind(&view.request_sha256).bind(scope).bind(&raw).execute(pool).await.unwrap();
        let receipt=serde_json::to_string_pretty(&json!({"effect_id":view.id,"request_id":wire,"outcome":"completed","result":"Synthetic Grüße"})).unwrap();
        sqlx::query("INSERT INTO team_effect_receipts(effect_id,outcome,source,payload,result) VALUES($1,'completed','executor',$2,'original-result')")
            .bind(view.id.to_string()).bind(&receipt).execute(pool).await.unwrap();
        saved.push(Original {
            id: view.id.to_string(),
            wire,
            payload: raw,
            receipt,
        });
    }
    saved
}
async fn project(tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<()> {
    for statement in include_str!("../migrations/0106.sql")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    backfill(tx).await
}
async fn unchanged(pool: &sqlx::AnyPool, saved: &[Original], projected: bool) {
    for original in saved {
        let row=sqlx::query("SELECT i.payload,r.payload AS receipt,r.result FROM team_effect_intents i JOIN team_effect_receipts r ON r.effect_id=i.id WHERE i.id=$1")
            .bind(&original.id).fetch_one(pool).await.unwrap();
        assert_eq!(
            row.try_get::<String, _>("payload").unwrap(),
            original.payload
        );
        assert_eq!(
            row.try_get::<String, _>("receipt").unwrap(),
            original.receipt
        );
        assert_eq!(
            row.try_get::<String, _>("result").unwrap(),
            "original-result"
        );
        if projected {
            let wire: String =
                sqlx::query_scalar("SELECT request_id FROM team_effect_intents WHERE id=$1")
                    .bind(&original.id)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            assert_eq!(wire, original.wire);
        }
    }
}
async fn positive(postgres: bool) {
    let pool = pool(postgres).await;
    let saved = seed(&pool, Corruption::None).await;
    let mut tx = pool.begin().await.unwrap();
    project(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    unchanged(&pool, &saved, true).await;
    let too_long = "x".repeat(129);
    for value in [None, Some(""), Some(too_long.as_str())] {
        assert!(
            sqlx::query("UPDATE team_effect_intents SET request_id=$1 WHERE id=$2")
                .bind(value)
                .bind(&saved[0].id)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("UPDATE team_effect_intents SET request_id=$1 WHERE id=$2")
            .bind(&saved[0].wire)
            .bind(&saved[1].id)
            .execute(&pool)
            .await
            .is_err()
    );
    // An old writer omitting the new wire field must fail before any dispatch.
    assert!(sqlx::query("INSERT INTO team_effect_intents(id,run_id,slot,attempt,sequence,kind,state,request_sha256,scope_sha256,dispatch_owner,permit_token,payload) SELECT $1,run_id,'omitted-wire',1,99,kind,state,request_sha256,scope_sha256,dispatch_owner,permit_token,payload FROM team_effect_intents WHERE id=$2")
        .bind(Uuid::new_v4().to_string()).bind(&saved[0].id).execute(&pool).await.is_err());
    unchanged(&pool, &saved, true).await;
    pool.close().await;
}
async fn negative(postgres: bool, case: Corruption) {
    let pool = pool(postgres).await;
    let saved = seed(&pool, case).await;
    let mut tx = pool.begin().await.unwrap();
    assert!(project(&mut tx).await.is_err());
    tx.rollback().await.unwrap();
    unchanged(&pool, &saved, false).await;
    assert!(
        sqlx::query("SELECT request_id FROM team_effect_intents")
            .fetch_all(&pool)
            .await
            .is_err(),
        "failed migration committed a partial schema"
    );
    pool.close().await;
}
#[tokio::test]
async fn sqlite_keeps_original_intents_receipts_and_requires_unique_wire_ids() {
    positive(false).await;
}
#[tokio::test]
async fn sqlite_rejects_missing_identity_mismatch_and_duplicate_wire_ids() {
    for case in [
        Corruption::MissingWire,
        Corruption::WrongIdentity,
        Corruption::DuplicateWire,
    ] {
        negative(false, case).await;
    }
}
#[tokio::test]
#[ignore = "requires a separately created disposable PostgreSQL database"]
async fn postgres_keeps_original_intents_receipts_and_requires_unique_wire_ids() {
    positive(true).await;
}
#[tokio::test]
#[ignore = "requires a separately created disposable PostgreSQL database"]
async fn postgres_rejects_duplicate_wire_ids_without_partial_schema() {
    negative(true, Corruption::DuplicateWire).await;
}
