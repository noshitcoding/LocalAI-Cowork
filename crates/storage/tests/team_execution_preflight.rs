//! Real transactional Team resolution; executor/key probes detect forbidden review effects.
use cowork_contracts::*;
use cowork_domain::{Error, Result};
use cowork_identity::{OrganizationRole, Permissions, Principal};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::{
    Store, WriteKnowledgeAccessPolicy,
    team_service::{
        LiteLlmExecutor, TeamExecution, TeamExecutionResult, TeamExecutor, TeamService,
    },
};
use cowork_workflow_contracts::{TeamExecutionBlocker, TeamExecutionKind, TeamExecutionRequest};
use futures_util::future::BoxFuture;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys(AtomicUsize);
impl KeyStore for Keys {
    fn load(&self, _: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Zeroizing::new([47; 32]))
    }
}
struct EffectProbe {
    executor: LiteLlmExecutor,
    calls: Arc<AtomicUsize>,
}
impl TeamExecutor for EffectProbe {
    fn supports(&self, plan: &TeamRunPlan) -> bool {
        self.executor.supports(plan)
    }
    fn execute(&self, _: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(Error::Forbidden) })
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    store: Store,
    service: TeamService,
    actor: Principal,
    team: Uuid,
    project: Uuid,
    order: TeamOrder,
    document: TeamDocument,
    keys: Arc<Keys>,
    key_id: KeyId,
    effects: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // A caller may supply only a separately created disposable PostgreSQL database.
        let store = if let Ok(url) = std::env::var("COWORK_TEAM_PREFLIGHT_DATABASE_URL") {
            assert!(
                url.rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with("cowork_team_review_")),
                "Use a new dedicated Team review qualification database"
            );
            Store::connect(&url).await.unwrap()
        } else {
            Store::sqlite(&directory.path().join("team-review.sqlite"))
                .await
                .unwrap()
        };
        let identity = store.identity();
        let bootstrap = "b".repeat(64);
        identity
            .initialize_credentials(&bootstrap, &"s".repeat(64))
            .await
            .unwrap();
        let (_, session) = identity
            .bootstrap(
                &bootstrap,
                "review-owner",
                "Review owner",
                "SyntheticPassword-2026!",
                "Review team",
            )
            .await
            .unwrap();
        let actor = identity.authenticate_session(&session.token).await.unwrap();
        let keys = Arc::new(Keys(AtomicUsize::new(0)));
        let effects = Arc::new(AtomicUsize::new(0));
        let key_id = KeyId {
            instance: Uuid::new_v4(),
            key: Uuid::new_v4(),
        };
        let service = TeamService::new(
            store.clone(),
            keys.clone(),
            key_id,
            Arc::new(EffectProbe {
                executor: LiteLlmExecutor::new(None).unwrap(),
                calls: effects.clone(),
            }),
        );
        let team = service.teams(&actor).await.unwrap()[0].id;
        let project = service
            .create_project(
                &actor,
                team,
                CreateTeamProject {
                    name: "Review project".into(),
                    idempotency_key: "review-project".into(),
                },
            )
            .await
            .unwrap()
            .id;
        service
            .configure_provider(
                &actor,
                team,
                ConfigureTeamProvider {
                    expected_revision: 0,
                    base_url: "http://127.0.0.1:9/v1".into(),
                    model: "review-model".into(),
                    api_key: "synthetic-key-never-in-review".into(),
                },
            )
            .await
            .unwrap();
        let order = service
            .create_order(
                &actor,
                team,
                project,
                CreateTeamOrder {
                    title: "Review order".into(),
                    prompt: "Original shared instruction".into(),
                    idempotency_key: "review-order".into(),
                },
            )
            .await
            .unwrap();
        let document = service
            .save_document(
                &actor,
                team,
                project,
                SaveTeamDocument {
                    provenance: None,
                    id: None,
                    expected_revision: 0,
                    kind: TeamDocumentKind::Skill,
                    name: "Frozen skill".into(),
                    content: "Synthetic Grüße".into(),
                    archived: false,
                },
            )
            .await
            .unwrap();
        keys.0.store(0, Ordering::SeqCst);
        Self {
            _directory: directory,
            store,
            service,
            actor,
            team,
            project,
            order,
            document,
            keys,
            key_id,
            effects,
        }
    }
    fn input(&self, key: &str) -> TeamExecutionRequest {
        TeamExecutionRequest {
            kind: TeamExecutionKind::Team,
            team_id: self.team,
            request: CreateTeamRun {
                order_id: self.order.id,
                prompt: "Review this task".into(),
                idempotency_key: key.into(),
                plan: TeamRunPlan {
                    skills: vec![self.document.id],
                    ..Default::default()
                },
            },
        }
    }
    fn no_effects(&self) {
        assert_eq!(
            self.keys.0.load(Ordering::SeqCst),
            0,
            "review opened a private credential"
        );
        assert_eq!(
            self.effects.load(Ordering::SeqCst),
            0,
            "review dispatched execution"
        );
    }
    async fn no_runs(&self) {
        assert!(
            self.service
                .runs(&self.actor, self.team, self.project)
                .await
                .unwrap()
                .is_empty()
        );
    }
    async fn other_actor(&self, name: &str) -> Principal {
        let identity = self.store.identity();
        identity
            .register_local_identity(name, name, "OtherSyntheticPassword-2026!")
            .await
            .unwrap();
        let session = identity
            .login(name, "OtherSyntheticPassword-2026!", 3_600_000)
            .await
            .unwrap();
        identity.authenticate_session(&session.token).await.unwrap()
    }
}

struct ClockFenceProbe {
    pool: sqlx::AnyPool,
    calls: Arc<AtomicUsize>,
    allowed: Arc<AtomicUsize>,
}
impl TeamExecutor for ClockFenceProbe {
    fn supports(&self, _: &TeamRunPlan) -> bool {
        true
    }
    fn execute(&self, execution: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        Box::pin(async move {
            execution.authority.revalidate().await?;
            self.calls.fetch_add(1, Ordering::SeqCst);
            let floor: i64 =
                sqlx::query_scalar("SELECT utc_highwater FROM identity_installation WHERE id=1")
                    .fetch_one(&self.pool)
                    .await
                    .unwrap();
            sqlx::query("UPDATE identity_installation SET utc_highwater=$1 WHERE id=1")
                .bind(floor + 3_600_000)
                .execute(&self.pool)
                .await
                .unwrap();
            let admitted = execution.authority.revalidate().await.is_ok();
            // Restore only this disposable fixture so the worker can record the result.
            sqlx::query("UPDATE identity_installation SET utc_highwater=$1 WHERE id=1")
                .bind(floor)
                .execute(&self.pool)
                .await
                .unwrap();
            if admitted {
                self.allowed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(TeamExecutionResult {
                response: "synthetic clock fence probe".into(),
            })
        })
    }
}

#[tokio::test]
async fn blocks_effect_when_identity_clock_would_move_backwards() {
    let f = Fixture::new().await;
    let url = std::env::var("COWORK_TEAM_PREFLIGHT_DATABASE_URL").unwrap_or_else(|_| {
        let path = f._directory.path().join("team-review.sqlite");
        format!(
            "sqlite:{}?mode=rw",
            percent_encoding::utf8_percent_encode(
                path.to_str().unwrap(),
                percent_encoding::NON_ALPHANUMERIC
            )
        )
    });
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let allowed = Arc::new(AtomicUsize::new(0));
    let service = TeamService::new(
        f.store.clone(),
        f.keys.clone(),
        f.key_id,
        Arc::new(ClockFenceProbe {
            pool: pool.clone(),
            calls: calls.clone(),
            allowed: allowed.clone(),
        }),
    );
    service
        .create_run(&f.actor, f.team, f.input("clock-effect").request)
        .await
        .unwrap();
    service.poll_once().await.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "The admitted effect path must actually reach the fence probe"
    );
    assert_eq!(
        allowed.load(Ordering::SeqCst),
        0,
        "A retained Team binding must use the identity UTC high-water fence before an effect"
    );
    pool.close().await;
    f.store.close().await;
}

struct ApprovalFenceProbe {
    requested: Mutex<Option<tokio::sync::oneshot::Sender<TeamToolApproval>>>,
    proceed: Arc<tokio::sync::Notify>,
    claims: Arc<AtomicUsize>,
    allowed: Arc<AtomicUsize>,
}
impl TeamExecutor for ApprovalFenceProbe {
    fn supports(&self, _: &TeamRunPlan) -> bool {
        true
    }
    fn execute(&self, execution: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        Box::pin(async move {
            let sha = "a".repeat(64);
            let approval = execution
                .authority
                .request_tool_approval(
                    "office",
                    json!({"probe":"no real tool operation"}),
                    sha.clone(),
                )
                .await?;
            self.requested
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(approval.clone())
                .unwrap();
            self.proceed.notified().await;
            self.claims.fetch_add(1, Ordering::SeqCst);
            if execution
                .authority
                .claim_tool_approval(approval.id, &sha)
                .await
                .is_ok()
            {
                execution
                    .authority
                    .revalidate_tool_approval(approval.id)
                    .await?;
                self.allowed.fetch_add(1, Ordering::SeqCst);
                execution
                    .authority
                    .finish_tool_approval(approval.id, None, "completed", None)
                    .await?;
            }
            Ok(TeamExecutionResult {
                response: "synthetic approval fence probe".into(),
            })
        })
    }
}

async fn approval_fence_case(restrict_approver: bool) {
    let f = Fixture::new().await;
    let url = std::env::var("COWORK_TEAM_PREFLIGHT_DATABASE_URL").unwrap_or_else(|_| {
        let path = f._directory.path().join("team-review.sqlite");
        format!(
            "sqlite:{}?mode=rw",
            percent_encoding::utf8_percent_encode(
                path.to_str().unwrap(),
                percent_encoding::NON_ALPHANUMERIC
            )
        )
    });
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let issued = f
        .store
        .identity()
        .login("review-owner", "SyntheticPassword-2026!", 3_600_000)
        .await
        .unwrap();
    let approver = f
        .store
        .identity()
        .authenticate_session(&issued.token)
        .await
        .unwrap();
    assert_ne!(approver.session_id(), f.actor.session_id());
    let (requested, receive) = tokio::sync::oneshot::channel();
    let proceed = Arc::new(tokio::sync::Notify::new());
    let claims = Arc::new(AtomicUsize::new(0));
    let allowed = Arc::new(AtomicUsize::new(0));
    let service = TeamService::new(
        f.store.clone(),
        f.keys.clone(),
        f.key_id,
        Arc::new(ApprovalFenceProbe {
            requested: Mutex::new(Some(requested)),
            proceed: proceed.clone(),
            claims: claims.clone(),
            allowed: allowed.clone(),
        }),
    );
    let run = service
        .create_run(&f.actor, f.team, f.input("approval-effect").request)
        .await
        .unwrap();
    let worker_service = service.clone();
    let worker = tokio::spawn(async move { worker_service.poll_once().await });
    let approval = tokio::time::timeout(std::time::Duration::from_secs(15), receive)
        .await
        .unwrap()
        .unwrap();
    service
        .approve_tool(
            &approver,
            f.team,
            run.id,
            approval.id,
            ApproveTeamTool {
                approved: true,
                request_sha256: approval.request_sha256.clone(),
            },
        )
        .await
        .unwrap();
    if restrict_approver {
        sqlx::query("INSERT INTO identity_session_scopes(session_id,organization_id,permissions) VALUES($1,$2,1)")
            .bind(approver.session_id().to_string()).bind(f.team.to_string()).execute(&pool).await.unwrap();
    }
    // The creator is still fully authorized; only the distinct approving
    // session's new ceiling may reject this claim.
    service
        .preflight_execution(&f.actor, &f.input("creator-still-authorized"))
        .await
        .unwrap();
    proceed.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(15), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(claims.load(Ordering::SeqCst), 1);
    assert_eq!(
        allowed.load(Ordering::SeqCst),
        usize::from(!restrict_approver)
    );
    pool.close().await;
    f.store.close().await;
}

#[tokio::test]
async fn approving_session_read_only_ceiling_blocks_a_later_tool_claim() {
    approval_fence_case(true).await;
}

#[tokio::test]
async fn unchanged_approving_session_can_claim_and_complete_its_exact_tool_approval() {
    approval_fence_case(false).await;
}

struct UnsupportedExecutor;
impl TeamExecutor for UnsupportedExecutor {
    fn supports(&self, _: &TeamRunPlan) -> bool {
        false
    }
    fn execute(&self, _: TeamExecution) -> BoxFuture<'_, Result<TeamExecutionResult>> {
        panic!("Original receipt recovery must not dispatch execution")
    }
}

#[tokio::test]
async fn readable_original_receipt_survives_execute_grant_revocation() {
    let f = Fixture::new().await;
    let input = f.input("readable-receipt").request;
    let original = f
        .service
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    let member = f.service.self_member(&f.actor, f.team).await.unwrap();
    f.store
        .identity()
        .grant_content(
            &f.actor,
            f.team,
            f.project,
            member.id,
            1,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        f.service
            .run(&f.actor, f.team, original.id)
            .await
            .unwrap()
            .id,
        original.id
    );
    let recovered = f
        .service
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let mut changed = input.clone();
    changed.prompt.push_str(" changed intent");
    assert!(matches!(
        f.service.create_run(&f.actor, f.team, changed).await,
        Err(Error::Conflict)
    ));
    let mut new = input;
    new.idempotency_key = "a-new-admission-is-still-denied".into();
    assert!(f.service.create_run(&f.actor, f.team, new).await.is_err());
    assert_eq!(f.keys.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
    f.store.close().await;
}

#[tokio::test]
async fn original_receipt_survives_executor_unavailability_and_order_archival() {
    let f = Fixture::new().await;
    let input = f.input("archived-receipt").request;
    let original = f
        .service
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    let unavailable = TeamService::new(
        f.store.clone(),
        f.keys.clone(),
        f.key_id,
        Arc::new(UnsupportedExecutor),
    );
    let recovered = unavailable
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    f.service
        .update_order(
            &f.actor,
            f.team,
            f.order.id,
            UpdateTeamOrder {
                expected_revision: f.order.revision,
                title: f.order.title.clone(),
                prompt: f.order.prompt.clone(),
                archived: true,
            },
        )
        .await
        .unwrap();
    let recovered = f
        .service
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let mut new = input;
    new.idempotency_key = "archived-order-new-admission-is-denied".into();
    assert!(f.service.create_run(&f.actor, f.team, new).await.is_err());
    assert_eq!(f.keys.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
    f.store.close().await;
}

#[tokio::test]
async fn original_creation_key_does_not_bypass_current_read_authority() {
    let f = Fixture::new().await;
    let input = f.input("unreadable-receipt").request;
    f.service
        .create_run(&f.actor, f.team, input.clone())
        .await
        .unwrap();
    let member = f.service.self_member(&f.actor, f.team).await.unwrap();
    f.store
        .identity()
        .grant_content(
            &f.actor,
            f.team,
            f.project,
            member.id,
            1,
            Permissions::read_only(),
            false,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.service.create_run(&f.actor, f.team, input).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(f.keys.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
    f.store.close().await;
}

#[tokio::test]
async fn resolves_current_team_facts_without_run_keys_or_model_effects() {
    let f = Fixture::new().await;
    let input = f.input("preflight");
    let review = f
        .service
        .preflight_execution(&f.actor, &input)
        .await
        .unwrap();
    let member = f.service.self_member(&f.actor, f.team).await.unwrap();
    let team = f
        .store
        .identity()
        .organization(&f.actor, f.team)
        .await
        .unwrap();
    assert!(!review.ready);
    assert_eq!(
        review.blockers,
        vec![TeamExecutionBlocker::UnboundExecutionTarget]
    );
    assert_eq!(review.actor.identity_id, f.actor.identity_id());
    assert_eq!(review.actor.session_id, f.actor.session_id());
    assert_eq!(review.actor.organization_id, f.team);
    assert_eq!(review.actor.organization_epoch, team.authorization_epoch);
    assert_eq!(review.actor.membership_id, member.id);
    assert_eq!(review.actor.membership_revision, member.revision);
    assert_eq!(review.actor.project_id, f.project);
    assert!(
        review.actor.session_revision > 0
            && review.actor.resource_revision > 0
            && review.actor.grant_revision > 0
    );
    assert_eq!(review.project_revision, 1);
    assert_eq!(review.order_revision, f.order.revision);
    assert_eq!(review.order_prompt, f.order.prompt);
    assert_eq!(review.model.provider_revision, 1);
    assert_eq!(review.model.model, "review-model");
    assert_eq!(review.model.base_url, "http://127.0.0.1:9/v1");
    assert_eq!(review.documents.len(), 1);
    assert_eq!(review.documents[0].id, f.document.id);
    assert_eq!(review.documents[0].revision, f.document.revision);
    assert_eq!(
        review.documents[0].content_sha256,
        format!("{:x}", Sha256::digest(f.document.content.as_bytes()))
    );
    let encoded = serde_json::to_string(&review).unwrap();
    assert!(!encoded.contains("synthetic-key-never-in-review"));
    assert!(!encoded.contains("sealed_key") && !encoded.contains("api_key"));
    assert_eq!(
        serde_json::to_value(&review.request).unwrap(),
        serde_json::to_value(&input.request).unwrap()
    );
    let repeat = f
        .service
        .preflight_execution(&f.actor, &input)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(review).unwrap(),
        serde_json::to_value(repeat).unwrap()
    );
    f.no_runs().await;
    f.no_effects();
    f.store.close().await;
}

#[tokio::test]
async fn refreshes_exact_project_provider_order_and_document_versions() {
    let f = Fixture::new().await;
    let input = f.input("versions");
    let before = f
        .service
        .preflight_execution(&f.actor, &input)
        .await
        .unwrap();
    f.service
        .update_project(
            &f.actor,
            f.team,
            f.project,
            UpdateTeamProject {
                expected_revision: 1,
                name: "Updated review project".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    f.service
        .configure_provider(
            &f.actor,
            f.team,
            ConfigureTeamProvider {
                expected_revision: 1,
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "updated-model".into(),
                api_key: "different-synthetic-key".into(),
            },
        )
        .await
        .unwrap();
    let updated = f
        .service
        .save_document(
            &f.actor,
            f.team,
            f.project,
            SaveTeamDocument {
                provenance: None,
                id: Some(f.document.id),
                expected_revision: 1,
                kind: TeamDocumentKind::Skill,
                name: "Updated skill".into(),
                content: "Changed evidence".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    f.service
        .update_order(
            &f.actor,
            f.team,
            f.order.id,
            UpdateTeamOrder {
                expected_revision: 1,
                title: "Updated order".into(),
                prompt: "Changed instruction".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    f.keys.0.store(0, Ordering::SeqCst);
    let after = f
        .service
        .preflight_execution(&f.actor, &input)
        .await
        .unwrap();
    assert_eq!(before.project_revision, 1);
    assert_eq!(after.project_revision, 2);
    assert!(after.actor.resource_revision > before.actor.resource_revision);
    assert_eq!(before.model.provider_revision, 1);
    assert_eq!(after.model.provider_revision, 2);
    assert_eq!(after.model.model, "updated-model");
    assert_eq!(after.order_revision, 2);
    assert_eq!(after.order_prompt, "Changed instruction");
    assert_eq!(after.documents[0].revision, updated.revision);
    assert_ne!(
        before.documents[0].content_sha256,
        after.documents[0].content_sha256
    );
    f.service
        .save_document(
            &f.actor,
            f.team,
            f.project,
            SaveTeamDocument {
                provenance: None,
                id: Some(updated.id),
                expected_revision: updated.revision,
                kind: updated.kind,
                name: updated.name,
                content: updated.content,
                archived: true,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        f.service.preflight_execution(&f.actor, &input).await,
        Err(Error::Conflict)
    ));
    f.no_runs().await;
    f.no_effects();
    f.store.close().await;
}

#[tokio::test]
async fn rejects_foreign_viewer_and_revoked_session_before_resolution_effects() {
    let f = Fixture::new().await;
    let input = f.input("authority");
    let other = f.other_actor("review-viewer").await;
    assert!(f.service.preflight_execution(&other, &input).await.is_err());
    let member = f
        .store
        .identity()
        .add_member(
            &f.actor,
            f.team,
            other.identity_id(),
            OrganizationRole::Viewer,
        )
        .await
        .unwrap();
    f.store
        .identity()
        .grant_content(
            &f.actor,
            f.team,
            f.project,
            member.id,
            0,
            Permissions::read_only(),
            true,
        )
        .await
        .unwrap();
    assert!(matches!(
        f.service.preflight_execution(&other, &input).await,
        Err(Error::Forbidden)
    ));
    f.no_runs().await;
    f.store.identity().logout(&f.actor).await.unwrap();
    assert!(
        f.service
            .preflight_execution(&f.actor, &input)
            .await
            .is_err()
    );
    assert!(
        f.service
            .create_run(&f.actor, f.team, input.request)
            .await
            .is_err()
    );
    f.no_effects();
    f.store.close().await;
}

#[tokio::test]
async fn shares_knowledge_policy_and_explicit_personal_memory_selection() {
    let f = Fixture::new().await;
    let memory = Uuid::new_v4();
    f.store
        .actor_context_mutate(
            &f.actor,
            "memories",
            memory,
            &json!({
        "request_id":Uuid::new_v4(),"expected_revision":0,"value":{"project_id":null,
        "key":"review_memory","content":"Private synthetic review note","category":"knowledge",
        "confidence":null,"enabled":true}}),
            false,
        )
        .await
        .unwrap();
    let knowledge = f
        .service
        .save_document(
            &f.actor,
            f.team,
            f.project,
            SaveTeamDocument {
                provenance: None,
                id: None,
                expected_revision: 0,
                kind: TeamDocumentKind::Knowledge,
                name: "Review knowledge".into(),
                content: "Shared synthetic evidence".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    let mut input = f.input("knowledge");
    input.request.plan.knowledge.push(knowledge.id);
    input.request.plan.personal_knowledge = true;
    let policy = |revision, enabled| WriteKnowledgeAccessPolicy {
        expected_revision: revision,
        private_projects: false,
        team_projects: enabled,
        enabled_users: vec![],
        enabled_projects: vec![],
        personal_team_runs: Some(enabled),
        enabled_personal_team_users: Some(vec![]),
    };
    f.store
        .write_knowledge_access_policy(&f.actor, &policy(0, false))
        .await
        .unwrap();
    assert!(
        matches!(f.service.preflight_execution(&f.actor, &input).await,
        Err(Error::Validation(message)) if message=="knowledge_base_disabled")
    );
    f.store
        .write_knowledge_access_policy(&f.actor, &policy(1, true))
        .await
        .unwrap();
    let review = f
        .service
        .preflight_execution(&f.actor, &input)
        .await
        .unwrap();
    assert_eq!(review.documents.len(), 2);
    assert_eq!(review.personal_knowledge.len(), 1);
    assert_eq!(review.personal_knowledge[0].id, memory);
    assert!(review.personal_knowledge[0].revision > 0);
    input.request.plan.personal_knowledge = false;
    assert!(
        f.service
            .preflight_execution(&f.actor, &input)
            .await
            .unwrap()
            .personal_knowledge
            .is_empty()
    );
    f.store
        .write_knowledge_access_policy(&f.actor, &policy(2, false))
        .await
        .unwrap();
    assert!(
        f.service
            .preflight_execution(&f.actor, &input)
            .await
            .is_err()
    );
    f.no_runs().await;
    f.no_effects();
    f.store.close().await;
}

#[tokio::test]
async fn existing_admission_keeps_original_receipts_and_project_revision_gates() {
    let f = Fixture::new().await;
    let input = f.input("original-receipt");
    let initial = f
        .service
        .create_run(&f.actor, f.team, input.request.clone())
        .await
        .unwrap();
    f.service
        .cancel_run(&f.actor, f.team, initial.id)
        .await
        .unwrap();
    let replay = f
        .service
        .create_run(&f.actor, f.team, input.request.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&initial).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
    assert_eq!(
        f.service
            .runs(&f.actor, f.team, f.project)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut changed = input.request;
    changed.prompt.push_str(" changed");
    assert!(matches!(
        f.service.create_run(&f.actor, f.team, changed).await,
        Err(Error::Conflict)
    ));
    let fresh = f.input("wrong-revision");
    assert!(matches!(
        f.service
            .create_run_at_project_revision(&f.actor, f.team, fresh.request, 99)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        f.service
            .runs(&f.actor, f.team, f.project)
            .await
            .unwrap()
            .len(),
        1
    );
    let scheduled = f.input("team-schedule:forged");
    assert!(matches!(
        f.service.preflight_execution(&f.actor, &scheduled).await,
        Err(Error::Forbidden)
    ));
    f.no_effects();
    f.store.close().await;
}
