//! Explicit local-model compatibility qualification after shared Team resolution.
//! This is the existing Team dispatch path, not common PlanV2/runner admission.
use cowork_contracts::*;
use cowork_domain::Result;
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    Store,
    team_service::{
        LiteLlmExecutor, TeamExecution, TeamExecutionResult, TeamExecutor, TeamService,
    },
};
use cowork_workflow_contracts::{TeamExecutionKind, TeamExecutionRequest};
use futures_util::future::BoxFuture;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys(AtomicUsize);
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Zeroizing::new([53; 32]))
    }
}
struct RealDispatch {
    inner: LiteLlmExecutor,
    calls: Arc<AtomicUsize>,
}
impl TeamExecutor for RealDispatch {
    fn supports(&self, plan: &TeamRunPlan) -> bool {
        self.inner.supports(plan)
    }
    fn supports_recovery(&self) -> bool {
        self.inner.supports_recovery()
    }
    fn execute(&self, execution: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let result = self.inner.execute(execution).await;
            if let Err(cowork_domain::Error::Storage(source)) = &result
                && let Some(error) = source.downcast_ref::<sqlx::Error>()
            {
                match error {
                    sqlx::Error::Database(db) => eprintln!(
                        "synthetic SQLite model test storage error: code={:?}; {}",
                        db.code(),
                        db.message()
                    ),
                    sqlx::Error::RowNotFound => {
                        eprintln!("synthetic SQLite model test storage error: row_not_found")
                    }
                    sqlx::Error::ColumnDecode { index, .. } => eprintln!(
                        "synthetic SQLite model test storage error: column_decode {index}"
                    ),
                    sqlx::Error::ColumnNotFound(name) => eprintln!(
                        "synthetic SQLite model test storage error: missing_column {name}"
                    ),
                    _ => eprintln!("synthetic SQLite model test storage error: other_sqlx"),
                }
            }
            result
        })
    }
}

#[tokio::test]
#[ignore = "requires explicitly enabled local Ollama qwen3:4b; sends synthetic data only"]
async fn shared_resolution_preserves_real_local_model_skill_context() {
    assert_eq!(std::env::var("COWORK_TEAM_REAL_MODEL").as_deref(), Ok("1"));
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temporary.path().join("real-team-model.sqlite"))
        .await
        .unwrap();
    let identity = store.identity();
    let bootstrap = "b".repeat(64);
    identity
        .initialize_credentials(&bootstrap, &"s".repeat(64))
        .await
        .unwrap();
    let (_, session) = identity
        .bootstrap(
            &bootstrap,
            "real-review-owner",
            "Synthetic model qualification",
            "SyntheticPassword-2026!",
            "Synthetic local team",
        )
        .await
        .unwrap();
    let actor = identity.authenticate_session(&session.token).await.unwrap();
    let keys = Arc::new(Keys(AtomicUsize::new(0)));
    let calls = Arc::new(AtomicUsize::new(0));
    let service = TeamService::new(
        store.clone(),
        keys.clone(),
        KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        },
        Arc::new(RealDispatch {
            inner: LiteLlmExecutor::new(None).unwrap(),
            calls: calls.clone(),
        }),
    );
    let team = service.teams(&actor).await.unwrap()[0].id;
    let project = service
        .create_project(
            &actor,
            team,
            CreateTeamProject {
                name: "Synthetic model context".into(),
                idempotency_key: "model-project".into(),
            },
        )
        .await
        .unwrap();
    service
        .configure_provider(
            &actor,
            team,
            ConfigureTeamProvider {
                expected_revision: 0,
                base_url: "http://127.0.0.1:11434/v1".into(),
                model: "qwen3:4b".into(),
                api_key: "synthetic-local-model-key".into(),
            },
        )
        .await
        .unwrap();
    let order=service.create_order(&actor,team,project.id,CreateTeamOrder{title:"Synthetic context proof".into(),prompt:"Follow the selected skill and return its proof value without explanation. Do not use tools.".into(),idempotency_key:"model-order".into()}).await.unwrap();
    let nonce = format!("TEAM-REAL-{}", Uuid::new_v4().simple());
    let document = service
        .save_document(
            &actor,
            team,
            project.id,
            SaveTeamDocument {
                provenance: None,
                id: None,
                expected_revision: 0,
                kind: TeamDocumentKind::Skill,
                name: "Synthetic proof skill".into(),
                content: format!("Return exactly this proof value: {nonce}"),
                archived: false,
            },
        )
        .await
        .unwrap();
    let input = TeamExecutionRequest {
        kind: TeamExecutionKind::Team,
        team_id: team,
        request: CreateTeamRun {
            order_id: order.id,
            prompt: "Perform the task using the selected skill.".into(),
            idempotency_key: "real-model-run".into(),
            plan: TeamRunPlan {
                skills: vec![document.id],
                ..Default::default()
            },
        },
    };
    keys.0.store(0, Ordering::SeqCst);
    let review = service.preflight_execution(&actor, &input).await.unwrap();
    assert!(!review.ready); // no fabricated common runner qualification
    assert_eq!(keys.0.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        service
            .runs(&actor, team, project.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        review.documents[0].content_sha256,
        format!("{:x}", Sha256::digest(document.content.as_bytes()))
    );
    let initial = service
        .create_run(&actor, team, input.request.clone())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(120), service.poll_once())
            .await
            .unwrap()
            .unwrap()
    );
    let final_run = service.run(&actor, team, initial.id).await.unwrap();
    assert_eq!(
        final_run.status,
        TeamRunStatus::Completed,
        "{:?}",
        final_run.error
    );
    let response = final_run.response.as_deref().unwrap();
    assert!(
        response.contains(&nonce),
        "real model did not return the value available only in the frozen skill"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(keys.0.load(Ordering::SeqCst) > 0);
    let repeat = service
        .create_run(&actor, team, input.request.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&initial).unwrap(),
        serde_json::to_value(repeat).unwrap()
    );
    service
        .update_order(
            &actor,
            team,
            order.id,
            UpdateTeamOrder {
                expected_revision: order.revision,
                title: order.title.clone(),
                prompt: order.prompt.clone(),
                archived: true,
            },
        )
        .await
        .unwrap();
    let member = service.self_member(&actor, team).await.unwrap();
    identity
        .grant_content(
            &actor,
            team,
            project.id,
            member.id,
            1,
            cowork_identity::Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    let recovered = service
        .create_run(&actor, team, input.request)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&initial).unwrap(),
        serde_json::to_value(recovered).unwrap()
    );
    assert!(!service.poll_once().await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    println!(
        "{}",
        json!({"passed":true,"real_local_model":true,"model":"qwen3:4b","dispatches":1,
        "frozen_skill_context_verified":true,"source_sha256":review.documents[0].content_sha256,
        "response_sha256":format!("{:x}",Sha256::digest(response.as_bytes())),"original_receipt_replayed":true,
        "original_receipt_after_order_archive_and_execute_revocation":true,
        "review_opened_credentials":false,"common_admission_qualified":false,"scope":"existing Team dispatch compatibility after shared resolver extraction"})
    );
}
