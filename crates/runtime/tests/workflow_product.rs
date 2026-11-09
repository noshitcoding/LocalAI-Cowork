use cowork_contracts::*;
use cowork_domain::{Error, workflows::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use cowork_workflow_contracts::{RunPreflight, RunRequest, WorkflowRunKind, WorkflowRunRequest};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: Runtime,
    project: Project,
    input: WorkflowRunRequest,
    profile: ProviderProfile,
}
fn budgets() -> Budgets {
    Budgets {
        max_steps: 32,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 131_072,
    }
}
fn definition(profile: &ProviderProfile) -> WorkflowDefinition {
    let rights = WorkflowRights {
        current_workspace: false,
        tools: ["create_artifact", "list_artifacts", "read_artifact"]
            .into_iter()
            .map(String::from)
            .collect(),
        folders: Default::default(),
    };
    WorkflowDefinition {
        documents: Default::default(),
        iterations: Default::default(),
        subflows: Default::default(),
        controls: Default::default(),
        gates: Default::default(),
        dataflow: None,
        format_version: 1,
        name: "Create and verify a result".into(),
        mode: WorkflowMode::Sequential,
        failure: WorkflowFailure::Stop,
        rights: rights.clone(),
        budgets: budgets(),
        roles: vec![WorkflowRole {
            agent_config: None,
            model_binding: None,
            output: None,
            id: "writer".into(),
            instructions: "Create a useful result using only the selected tools.".into(),
            provider_id: profile.id.clone(),
            provider_revision: profile.revision,
            rights: rights.clone(),
            budgets: budgets(),
            delegate_to: Default::default(),
        }],
        steps: vec![WorkflowStep {
            execution: None,
            zero_context: false,
            id: "create".into(),
            role_id: "writer".into(),
            goal: "Produce the requested result.".into(),
            dependencies: vec![],
            rights,
            budgets: budgets(),
        }],
    }
}
async fn setup(context_tokens: u32) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&dir.path().join("product.db")).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Product workflow".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Create a deliverable".into(),
        })
        .await
        .unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "product-test".into(),
        name: "Product test model".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "test".into(),
        revision: 1,
        context_tokens,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let library = store
        .create_workflow(&project.id, "create-library", &definition(&profile))
        .await
        .unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap()
    .with_file_broker(cowork_broker::FileBroker::open(dir.path(), vec![]).unwrap());
    let input = WorkflowRunRequest {
        input_source: None,
        inputs: None,
        agent_config: None,
        native_tools: vec![],
        folders: vec![],
        mcp_tools: vec![],
        environment: None,
        kind: WorkflowRunKind::Workflow,
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: "start-workflow".into(),
        prompt: "Create a useful text artifact.".into(),
        workflow_id: library.id,
        version_id: library.versions[0].id.clone(),
        access_mode: AccessMode::ConfirmMutations,
        budgets: budgets(),
        expected_binding: None,
    };
    Fixture {
        _dir: dir,
        runtime,
        project,
        input,
        profile,
    }
}

async fn assert_no_order_workspace(fixture: &Fixture) {
    // Opening FileBroker creates its empty container directories before preflight.
    // A selected/created workspace would have a child, database row and grant.
    let container = fixture._dir.path().join("workspaces");
    assert!(container.is_dir());
    assert_eq!(std::fs::read_dir(container).unwrap().count(), 0);
    assert!(
        fixture
            .runtime
            .store
            .workspace(&fixture.input.order_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .runtime
            .files
            .as_ref()
            .unwrap()
            .grants(&fixture.project.id)
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn public_workflow_start_uses_shared_queue_and_exact_running_approval() {
    let mut fixture = setup(16_384).await;
    assert_no_order_workspace(&fixture).await;
    let flight = match fixture
        .runtime
        .preflight_request(&RunRequest::Workflow(fixture.input.clone()))
        .await
        .unwrap()
    {
        RunPreflight::Workflow(flight) => flight,
        RunPreflight::Single(_) => {
            panic!("workflow preflight must not select an ordinary provider")
        }
    };
    assert_eq!(flight.roles.len(), 1);
    assert_eq!(flight.roles[0].provider.id, fixture.profile.id);
    assert_eq!(flight.max_parallel, 1);
    assert_eq!(
        flight.allowed_tools,
        [
            "create_artifact",
            "list_artifacts",
            "read_artifact",
            "request_input"
        ]
        .map(str::to_owned)
        .to_vec()
    );
    assert_no_order_workspace(&fixture).await;
    fixture.input.expected_binding = Some(flight.workflow.clone());
    let run = fixture
        .runtime
        .create_run_request(&RunRequest::Workflow(fixture.input.clone()))
        .await
        .unwrap();
    assert_eq!(run.spec.workflow, Some(flight.workflow));
    assert!(run.spec.provider.is_none());
    assert!(run.spec.folders.is_empty());
    assert!(run.spec.mcp_tools.is_empty());
    assert_no_order_workspace(&fixture).await;
    let stop = CancellationToken::new();
    let worker = tokio::spawn(fixture.runtime.clone().serve(2, stop.clone()));
    let approval = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let detail = fixture.runtime.store.detail(&run.id).await.unwrap();
            if let Some(approval) = detail.approvals.into_iter().find(|a| a.decision.is_none()) {
                assert_eq!(detail.run.state, RunState::Running);
                return approval;
            }
            assert!(!matches!(
                detail.run.state,
                RunState::Failed | RunState::Interrupted
            ));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("workflow approval deadline");
    assert!(
        fixture
            .runtime
            .store
            .artifacts(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    let rejected = fixture
        .runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: "changed".into(),
                allow: true,
            },
        )
        .await;
    assert!(matches!(rejected, Err(Error::Conflict)));
    fixture
        .runtime
        .store
        .resolve_approval(
            &run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint.clone(),
                allow: true,
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let detail = fixture.runtime.store.detail(&run.id).await.unwrap();
            if detail.run.state == RunState::Completed {
                break;
            }
            assert!(!matches!(
                detail.run.state,
                RunState::Failed | RunState::Interrupted
            ));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("workflow completion deadline");
    stop.cancel();
    worker.await.unwrap();
    assert_no_order_workspace(&fixture).await;
    assert_eq!(
        fixture
            .runtime
            .store
            .artifacts(&fixture.input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn confirmed_workflow_replay_survives_revocation_and_removed_runtime_profiles() {
    let fixture = setup(16_384).await;
    let run = fixture
        .runtime
        .create_product_workflow_run(&fixture.input)
        .await
        .unwrap();
    fixture
        .runtime
        .store
        .revoke_workflow(&fixture.project.id, &fixture.input.workflow_id, 1, "revoke")
        .await
        .unwrap();
    fixture.runtime.providers.write().await.clear();
    assert_eq!(
        fixture
            .runtime
            .create_product_workflow_run(&fixture.input)
            .await
            .unwrap()
            .id,
        run.id
    );
    let mut changed = fixture.input.clone();
    changed.prompt.push_str(" changed");
    assert!(matches!(
        fixture.runtime.create_product_workflow_run(&changed).await,
        Err(Error::Conflict)
    ));
    let mut new_start = fixture.input.clone();
    new_start.idempotency_key = "another-start".into();
    new_start.base_revision = fixture
        .runtime
        .store
        .order(&new_start.order_id)
        .await
        .unwrap()
        .revision;
    assert!(matches!(
        fixture
            .runtime
            .create_product_workflow_run(&new_start)
            .await,
        Err(Error::Forbidden | Error::WorkflowRevoked)
    ));
    assert_eq!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn all_pinned_roles_are_checked_before_a_workflow_is_accepted() {
    let mut fixture = setup(16_384).await;
    let mut later = fixture.profile.clone();
    later.id = "later-credential-required".into();
    later.credential_required = true;
    // Stored readiness is insufficient: no real server credential is configured.
    fixture.runtime.store.insert_provider(&later).await.unwrap();
    fixture
        .runtime
        .providers
        .write()
        .await
        .push(ConfiguredProvider {
            profile: later.clone(),
            credential_env: None,
        });
    let mut definition = definition(&fixture.profile);
    let mut role = definition.roles[0].clone();
    role.id = "reviewer".into();
    role.provider_id = later.id;
    definition.roles.push(role);
    let mut step = definition.steps[0].clone();
    step.id = "review".into();
    step.role_id = "reviewer".into();
    definition.steps.push(step);
    let publication = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "add-reviewer",
            &definition,
        )
        .await
        .unwrap();
    fixture.input.version_id = publication.version_id.unwrap();
    assert!(
        matches!(fixture.runtime.create_product_workflow_run(&fixture.input).await, Err(Error::Provider(code)) if code == "credential_missing")
    );
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unsupported_tools_and_invalid_hierarchy_stay_library_data_without_start_authority() {
    let mut fixture = setup(16_384).await;
    let mut unsupported = definition(&fixture.profile);
    unsupported.roles[0]
        .rights
        .tools
        .insert("unsupported_tool_v99".into());
    let version = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "future-tool",
            &unsupported,
        )
        .await
        .unwrap();
    fixture.input.version_id = version.version_id.unwrap();
    assert!(
        fixture
            .runtime
            .preflight_workflow(&fixture.input)
            .await
            .is_err()
    );
    let mut hierarchy = definition(&fixture.profile);
    hierarchy.mode = WorkflowMode::Hierarchical {
        manager_step_id: Some("missing-manager".into()),
        max_parallel: 2,
        max_depth: 2,
        max_delegations: 2,
    };
    assert!(matches!(
        fixture
            .runtime
            .store
            .publish_workflow(
                &fixture.project.id,
                &fixture.input.workflow_id,
                2,
                "invalid-manager",
                &hierarchy,
            )
            .await,
        Err(Error::Validation(_))
    ));
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn initial_context_and_expected_plan_mismatch_prevent_acceptance() {
    let mut fixture = setup(16_384).await;
    let flight = fixture
        .runtime
        .preflight_workflow(&fixture.input)
        .await
        .unwrap();
    let mut binding = flight.workflow;
    binding.plan_sha256 = "0".repeat(64);
    fixture.input.expected_binding = Some(binding);
    assert!(matches!(
        fixture
            .runtime
            .create_product_workflow_run(&fixture.input)
            .await,
        Err(Error::Conflict)
    ));
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
    let mut fixture = setup(1024).await;
    let mut large = definition(&fixture.profile);
    large.roles[0].instructions = "x".repeat(6000);
    let version = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "large-context",
            &large,
        )
        .await
        .unwrap();
    fixture.input.version_id = version.version_id.unwrap();
    assert!(matches!(
        fixture
            .runtime
            .create_product_workflow_run(&fixture.input)
            .await,
        Err(Error::ContextLimit)
    ));
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workflow_product_preserves_all_four_access_modes() {
    for (mode, needs_approval, artifact_count) in [
        (AccessMode::ReadOnly, false, 0),
        (AccessMode::ConfirmMutations, true, 1),
        (AccessMode::ConfirmAll, true, 1),
        (AccessMode::Autonomous, false, 1),
    ] {
        let mut fixture = setup(16_384).await;
        fixture.input.access_mode = mode.clone();
        let run = fixture
            .runtime
            .create_product_workflow_run(&fixture.input)
            .await
            .unwrap();
        assert_eq!(run.spec.access_mode, mode);
        let stop = CancellationToken::new();
        let worker = tokio::spawn(fixture.runtime.clone().serve(1, stop.clone()));
        let count = tokio::time::timeout(Duration::from_secs(15), async {
            let mut count = 0;
            loop {
                let detail = fixture.runtime.store.detail(&run.id).await.unwrap();
                for approval in detail
                    .approvals
                    .into_iter()
                    .filter(|approval| approval.decision.is_none())
                {
                    assert!(
                        needs_approval,
                        "mode {mode:?} must not prompt for a denied or autonomous action"
                    );
                    assert_eq!(detail.run.state, RunState::Running);
                    fixture
                        .runtime
                        .store
                        .resolve_approval(
                            &run.id,
                            &approval.id,
                            ResolveApproval {
                                fingerprint: approval.fingerprint,
                                allow: true,
                            },
                        )
                        .await
                        .unwrap();
                    count += 1;
                }
                if detail.run.state == RunState::Completed {
                    return count;
                }
                assert!(
                    !matches!(detail.run.state, RunState::Failed | RunState::Interrupted),
                    "mode {mode:?}: {:?}",
                    detail.run.error_code
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("mode-specific workflow completion deadline");
        stop.cancel();
        worker.await.unwrap();
        assert_eq!(count, usize::from(needs_approval));
        assert_eq!(
            fixture
                .runtime
                .store
                .artifacts(&fixture.input.order_id)
                .await
                .unwrap()
                .len(),
            artifact_count
        );
    }
}

#[tokio::test]
async fn product_parallelism_is_capped_and_semantic_profile_drift_is_rejected() {
    let mut fixture = setup(16_384).await;
    let mut parallel = definition(&fixture.profile);
    parallel.mode = WorkflowMode::Parallel { max_parallel: 32 };
    let version = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "parallel-cap",
            &parallel,
        )
        .await
        .unwrap();
    fixture.input.version_id = version.version_id.unwrap();
    assert_eq!(
        fixture
            .runtime
            .preflight_workflow(&fixture.input)
            .await
            .unwrap()
            .max_parallel,
        4
    );
    fixture.runtime.providers.write().await[0].profile.model = "changed-test-model".into();
    assert!(
        matches!(fixture.runtime.create_product_workflow_run(&fixture.input).await, Err(Error::Provider(code)) if code == "profile_changed")
    );
    assert!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[derive(Clone)]
struct ProbeBarrier {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}
async fn barrier_catalog(
    axum::extract::State(barrier): axum::extract::State<ProbeBarrier>,
) -> axum::Json<serde_json::Value> {
    if barrier
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        == 0
    {
        barrier.entered.notify_one();
        barrier.release.notified().await;
        axum::Json(serde_json::json!({"data":[]}))
    } else {
        axum::Json(serde_json::json!({"data":[{"id":"test"}]}))
    }
}

#[tokio::test]
async fn concurrent_committed_workflow_replay_wins_over_late_provider_preflight_failure() {
    let barrier = ProbeBarrier {
        calls: Default::default(),
        entered: Default::default(),
        release: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let routes = axum::Router::new()
        .route("/models", axum::routing::get(barrier_catalog))
        .with_state(barrier.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    let mut fixture = setup(16_384).await;
    // Provider records are insert-only. Use a new identity and explicitly pin
    // its workflow version instead of pretending to replace the test profile.
    let mut profile = fixture.profile.clone();
    profile.id = "catalog-barrier-profile".into();
    profile.kind = ProviderKind::OpenaiCompatible;
    profile.endpoint = endpoint;
    fixture
        .runtime
        .store
        .insert_provider(&profile)
        .await
        .unwrap();
    let stored = fixture.runtime.store.providers().await.unwrap();
    let stored = stored
        .iter()
        .find(|stored| stored.id == profile.id)
        .unwrap();
    assert_eq!(
        cowork_domain::canonical_hash(stored),
        cowork_domain::canonical_hash(&profile)
    );
    fixture
        .runtime
        .providers
        .write()
        .await
        .push(ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        });
    let publication = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "catalog-barrier-profile-version",
            &definition(&profile),
        )
        .await
        .unwrap();
    fixture.input.version_id = publication.version_id.unwrap();
    let mut duplicate = {
        let runtime = fixture.runtime.clone();
        let input = fixture.input.clone();
        tokio::spawn(async move { runtime.create_product_workflow_run(&input).await })
    };
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("workflow returned before entering the catalog barrier: {result:?}"),
        entered = tokio::time::timeout(Duration::from_secs(10), barrier.entered.notified()) => {
            entered.expect("first probe must wait at the catalog barrier");
        }
    }
    // The second identical request receives a valid catalogue and commits while
    // the first has already observed no receipt and is still inside preflight.
    let accepted = fixture
        .runtime
        .create_product_workflow_run(&fixture.input)
        .await
        .unwrap();
    barrier.release.notify_one();
    let replay = duplicate.await.unwrap().unwrap();
    assert_eq!(replay.id, accepted.id);
    // Two competing preparations plus the winner's fresh confirmation check.
    // No completion request or failed preparation is automatically replayed.
    assert_eq!(barrier.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(
        fixture
            .runtime
            .store
            .runs(&fixture.input.order_id)
            .await
            .unwrap()
            .len(),
        1
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn workflow_start_copies_agent_context_and_narrows_shared_tools() {
    let mut fixture = setup(65_536).await;
    let mut copied = definition(&fixture.profile);
    copied.roles[0].instructions = format!("Copied long agent: {}", "x".repeat(20_000));
    copied.roles[0].agent_config = Some(EffectiveAgentConfig {
        instructions: copied.roles[0].instructions.clone(),
        ..Default::default()
    });
    let version = fixture
        .runtime
        .store
        .publish_workflow(
            &fixture.project.id,
            &fixture.input.workflow_id,
            1,
            "long-agent-snapshot",
            &copied,
        )
        .await
        .unwrap();
    fixture.input.version_id = version.version_id.unwrap();
    fixture.input.agent_config = Some(EffectiveAgentConfig {
        instructions: "Copied parent personality".into(),
        generation: GenerationSettings {
            temperature: Some(0.0),
            timeout_ms: Some(4000),
        },
        tool_filter: Some(vec!["read_artifact".into()]),
        ..Default::default()
    });
    let flight = fixture
        .runtime
        .preflight_workflow(&fixture.input)
        .await
        .unwrap();
    assert_eq!(flight.allowed_tools, vec!["read_artifact"]);
    let run = fixture
        .runtime
        .create_product_workflow_run(&fixture.input)
        .await
        .unwrap();
    let node = fixture
        .runtime
        .store
        .workflow_node_spec(&fixture.project.id, &run.id, "create")
        .await
        .unwrap();
    let snapshot = node.agent_config.unwrap();
    assert_eq!(snapshot.generation.temperature, Some(0.0));
    assert!(snapshot.instructions.contains("Copied parent personality"));
    assert!(snapshot.instructions.contains("Copied long agent"));
    assert_eq!(node.allowed_tools, vec!["read_artifact"]);
}
