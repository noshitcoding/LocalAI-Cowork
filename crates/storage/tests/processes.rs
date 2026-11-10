use cowork_contracts::*;
use cowork_domain::{environments::*, processes::*, *};
use cowork_storage::{Change, ProcessAdmission, ProcessResolution, Store, id, now};
use serde_json::json;
use std::collections::BTreeMap;

struct Fixture {
    run: RunRecord,
    token: String,
    tool: ToolCall,
    observation: EnvironmentObservation,
}
async fn setup(store: &Store, mode: AccessMode) -> Fixture {
    let project = store
        .create_project(CreateProject {
            name: "Synthetic process qualification".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Process journal".into(),
        })
        .await
        .unwrap();
    let definition = EnvironmentDefinition {
        format_version: 1,
        network: None,
        artifact: EnvironmentArtifact::LinuxImage {
            reference: format!("test.invalid/fixture@sha256:{}", "a".repeat(64)),
        },
        programs: BTreeMap::from([("fixture".into(), "bin/fixture".into())]),
        packages: vec![],
        limits: EnvironmentLimits {
            cpu_millis: 1000,
            memory_bytes: 256 * 1024 * 1024,
            processes: 32,
            scratch_bytes: 64 * 1024 * 1024,
        },
    };
    let mut env = store
        .create_environment(&project.id, "Synthetic adapter only", "create")
        .await
        .unwrap();
    let receipt = store
        .change_environment(
            &project.id,
            &env.id,
            1,
            "stage",
            &EnvironmentChange::Stage {
                definition: definition.clone(),
            },
        )
        .await
        .unwrap();
    let version = receipt.version_id.unwrap();
    let q = EnvironmentQualification {
        runner_id: "test-runner".into(),
        runner_generation: 1,
        adapter_build_sha256: "b".repeat(64),
        artifact_sha256: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(&definition),
        report_sha256: "c".repeat(64),
        capabilities: definition.required_capabilities(),
        verified_at: now(),
    };
    store
        .change_environment(
            &project.id,
            &env.id,
            2,
            "qualified",
            &EnvironmentChange::Qualify {
                version_id: version.clone(),
                qualification: q.clone(),
            },
        )
        .await
        .unwrap();
    let observation = EnvironmentObservation {
        runner_id: q.runner_id.clone(),
        runner_generation: q.runner_generation,
        adapter_build_sha256: q.adapter_build_sha256.clone(),
        artifact_sha256: q.artifact_sha256.clone(),
        definition_sha256: q.definition_sha256.clone(),
        capabilities: q.capabilities.clone(),
        observed_at: now(),
    };
    store
        .change_environment(
            &project.id,
            &env.id,
            3,
            "active",
            &EnvironmentChange::Activate {
                version_id: version,
                observation: observation.clone(),
            },
        )
        .await
        .unwrap();
    env = store.environment(&project.id, &env.id).await.unwrap();
    let pin = env.pin(&observation, now()).unwrap();
    let folder = FolderGrant {
        id: format!("workspace:{}", order.id),
        project_id: project.id.clone(),
        name: "Own working directory".into(),
        writable: true,
        revision: "fixture".into(),
        resource_id: id(),
        order_id: Some(order.id.clone()),
    };
    let spec = RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: Some(pin.reference()),
        order_id: order.id.clone(),
        project_id: project.id,
        prompt: "Synthetic process only".into(),
        provider: Some(ProviderProfile {
            generation: Default::default(),
            id: "fixture".into(),
            name: "Synthetic provider (unused)".into(),
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
        access_mode: mode.clone(),
        executor: q.runner_id,
        policy_revision: 1,
        allowed_tools: vec!["execute_program".into()],
        budgets: Budgets::default(),
        folders: vec![folder],
    };
    let input = CreateRun {
        agent_config: None,
        model_binding: None,
        output: None,
        native_tools: vec![],
        environment: None,
        mcp_tools: vec![],
        order_id: order.id,
        base_revision: order.revision,
        idempotency_key: id(),
        prompt: spec.prompt.clone(),
        provider_id: "fixture".into(),
        provider_revision: 1,
        access_mode: mode,
        budgets: Budgets::default(),
        folders: vec![],
    };
    let run = store.create_run(&input, spec).await.unwrap();
    // No production runtime claims these synthetic invocations or trusts these reports.
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(claim.record.id, run.id);
    let token = claim.lease.unwrap().token;
    let tool = ToolCall {
        id: id(),
        name: "execute_program".into(),
        arguments: json!(ProcessInvocation {
            program: "fixture".into(),
            arguments: vec!["exact quoted argument".into()],
            timeout_seconds: 10
        }),
    };
    store
        .mutate(&run.id, Some(&token), |r| {
            r.checkpoint.pending_tools = vec![tool.clone()];
            Ok(Change::default())
        })
        .await
        .unwrap();
    Fixture {
        run,
        token,
        tool,
        observation,
    }
}
fn capture(bytes: &[u8]) -> ProcessCapture {
    ProcessCapture {
        bytes: bytes.into(),
        sha256: hash(bytes),
        total_bytes: bytes.len() as u64,
        truncated: false,
    }
}
fn result() -> ProcessResult {
    ProcessResult {
        termination: ProcessTermination::Exited,
        process_id: Some(123),
        exit_code: Some(37),
        signal: None,
        stdout: capture(&[0xff, 0, 1, 2]),
        stderr: capture(b"synthetic diagnostic"),
        duration_ms: 10,
        output_complete: true,
    }
}
async fn expire(store: &Store, f: &Fixture) {
    store
        .mutate(&f.run.id, None, |r| {
            r.lease.as_mut().unwrap().until = 0;
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert_eq!(store.recover_expired_leases().await.unwrap(), 1);
}
async fn approve(store: &Store, f: &mut Fixture) {
    let approval = Approval {
        id: id(),
        run_id: f.run.id.clone(),
        tool: f.tool.clone(),
        target: f.run.spec.executor.clone(),
        policy_revision: f.run.spec.policy_revision,
        fingerprint: approval_fingerprint(&f.run, &f.tool),
        decision: None,
    };
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.approvals.push(approval.clone());
            r.transition(RunState::WaitingApproval)?;
            r.lease = None;
            Ok(Change::event(
                "approval_requested",
                json!({"approval":approval}),
            ))
        })
        .await
        .unwrap();
    store
        .resolve_approval(
            &f.run.id,
            &approval.id,
            ResolveApproval {
                fingerprint: approval.fingerprint,
                allow: true,
            },
        )
        .await
        .unwrap();
    f.token = store.claim().await.unwrap().unwrap().lease.unwrap().token;
}
async fn shared_contract(store: &Store) {
    for mode in [
        AccessMode::ReadOnly,
        AccessMode::ConfirmMutations,
        AccessMode::ConfirmAll,
        AccessMode::Autonomous,
    ] {
        let mut f = setup(store, mode.clone()).await;
        if mode != AccessMode::Autonomous {
            assert!(matches!(
                store
                    .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
                    .await,
                Err(Error::Forbidden)
            ));
            assert!(store.process_attempts(&f.run.id).await.unwrap().is_empty());
            if mode == AccessMode::ReadOnly {
                store.cancel(&f.run.id).await.unwrap();
                store
                    .mutate(&f.run.id, Some(&f.token), |r| {
                        r.transition(RunState::Canceled)?;
                        r.lease = None;
                        Ok(Change::event("run_canceled", json!({})))
                    })
                    .await
                    .unwrap();
                continue;
            }
            approve(store, &mut f).await;
        }
        let (a, b) = tokio::join!(
            store.admit_process(&f.run.id, &f.token, &f.tool, &f.observation),
            store.admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
        );
        let mut permits = 0;
        let mut attempt = None;
        for admission in [a.unwrap(), b.unwrap()] {
            match admission {
                ProcessAdmission::Launch(permit) => {
                    permits += 1;
                    attempt = Some(permit.into_attempt());
                }
                ProcessAdmission::Existing(_) => {}
            }
        }
        assert_eq!(permits, 1);
        let attempt = attempt.unwrap();
        assert_eq!(attempt.environment, f.run.spec.environment.clone().unwrap());
        let before = store.events(&f.run.id, 0).await.unwrap().len();
        assert!(matches!(
            store
                .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
                .await
                .unwrap(),
            ProcessAdmission::Existing(_)
        ));
        assert_eq!(store.events(&f.run.id, 0).await.unwrap().len(), before);
        let mut changed = f.tool.clone();
        changed.arguments["arguments"] = json!(["changed"]);
        assert!(matches!(
            store
                .admit_process(&f.run.id, &f.token, &changed, &f.observation)
                .await,
            Err(Error::Conflict)
        ));
        assert!(matches!(
            store
                .finish_process(&f.run.id, "stale", &f.tool.id, result())
                .await,
            Err(Error::StaleLease)
        ));
        let (a, b) = tokio::join!(
            store.finish_process(&f.run.id, &f.token, &f.tool.id, result()),
            store.finish_process(&f.run.id, &f.token, &f.tool.id, result())
        );
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(store.stored_run(&f.run.id).await.unwrap().record.steps, 1);
        assert!(
            store
                .stored_run(&f.run.id)
                .await
                .unwrap()
                .checkpoint
                .in_flight
                .is_none()
        );
        let completed = store.process_attempts(&f.run.id).await.unwrap();
        assert_eq!(completed.len(), 1);
        let archive = store.export_order(&f.run.spec.order_id).await.unwrap();
        assert_eq!(archive.format_version, 13);
        assert_eq!(archive.data.runs[0].process_attempts, completed);
        assert_eq!(archive.sha256, canonical_hash(&archive.data));
        let public = serde_json::to_string(&archive).unwrap();
        assert!(!public.contains("lease_sha256"));
        assert!(!public.contains(&f.token));
        assert_eq!(
            store
                .events(&f.run.id, 0)
                .await
                .unwrap()
                .iter()
                .filter(|e| e.kind == "process_completed")
                .count(),
            1
        );
        store.cancel(&f.run.id).await.unwrap();
        store
            .mutate(&f.run.id, Some(&f.token), |r| {
                r.transition(RunState::Canceled)?;
                r.lease = None;
                Ok(Change::event("run_canceled", json!({})))
            })
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn sqlite_process_policy_idempotence_completion_and_export_contract() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("state.sqlite3"))
        .await
        .unwrap();
    shared_contract(&store).await;
}
#[tokio::test]
#[ignore = "Requires a dedicated PostgreSQL database in COWORK_TEST_POSTGRES"]
async fn postgres_process_contract() {
    let store = Store::connect(&std::env::var("COWORK_TEST_POSTGRES").unwrap())
        .await
        .unwrap();
    shared_contract(&store).await;
    store.close().await;
}
#[tokio::test]
async fn immutable_spec_revocation_and_unknown_program_deny_before_admission() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("state.sqlite3"))
        .await
        .unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    let before = store.stored_run(&f.run.id).await.unwrap();
    assert!(
        store
            .mutate(&f.run.id, Some(&f.token), |r| {
                r.record.spec.environment = None;
                Ok(Change::default())
            })
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap()),
        canonical_hash(&before)
    );
    let mut tool = f.tool.clone();
    tool.arguments["program"] = json!("unregistered");
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools[0] = tool.clone();
            Ok(Change::default())
        })
        .await
        .unwrap();
    assert!(
        store
            .admit_process(&f.run.id, &f.token, &tool, &f.observation)
            .await
            .is_err()
    );
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_none()
    );
    store
        .mutate(&f.run.id, Some(&f.token), |r| {
            r.checkpoint.pending_tools[0] = f.tool.clone();
            Ok(Change::default())
        })
        .await
        .unwrap();
    let mut lost = f.observation.clone();
    lost.capabilities.clear();
    assert!(
        store
            .admit_process(&f.run.id, &f.token, &f.tool, &lost)
            .await
            .is_err()
    );
    let binding = f.run.spec.environment.as_ref().unwrap();
    let env = store
        .environment(&f.run.spec.project_id, &binding.environment_id)
        .await
        .unwrap();
    store
        .change_environment(
            &env.project_id,
            &env.id,
            env.revision,
            "revoked",
            &EnvironmentChange::Revoke,
        )
        .await
        .unwrap();
    assert!(
        store
            .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
            .await
            .is_err()
    );
    assert!(store.process_attempts(&f.run.id).await.unwrap().is_empty());
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_none()
    );
}
#[tokio::test]
async fn canceled_process_keeps_writer_until_observed_termination_and_can_settle_after_revocation()
{
    let temp = tempfile::tempdir().unwrap();
    let store = Store::sqlite(&temp.path().join("state.sqlite3"))
        .await
        .unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    assert!(matches!(
        store
            .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
            .await
            .unwrap(),
        ProcessAdmission::Launch(_)
    ));
    assert!(matches!(
        store
            .mutate(&f.run.id, Some(&f.token), |r| {
                r.checkpoint.in_flight = None;
                Ok(Change::default())
            })
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(matches!(
        store
            .mutate(&f.run.id, Some(&f.token), |r| {
                r.checkpoint.pending_tools.clear();
                Ok(Change::default())
            })
            .await,
        Err(Error::UncertainEffect)
    ));
    assert!(matches!(
        store
            .mutate(&f.run.id, Some(&f.token), |r| {
                r.transition(RunState::Failed)?;
                Ok(Change::event("run_failed", json!({})))
            })
            .await,
        Err(Error::UncertainEffect)
    ));
    let order = store.order(&f.run.spec.order_id).await.unwrap();
    let waiting = store
        .create_run(
            &CreateRun {
                agent_config: None,
                model_binding: None,
                output: None,
                native_tools: vec![],
                environment: None,
                mcp_tools: vec![],
                order_id: order.id,
                base_revision: order.revision,
                idempotency_key: id(),
                prompt: f.run.spec.prompt.clone(),
                provider_id: "fixture".into(),
                provider_revision: 1,
                access_mode: AccessMode::Autonomous,
                budgets: f.run.spec.budgets.clone(),
                folders: f.run.spec.folders.clone(),
            },
            f.run.spec.clone(),
        )
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    assert_eq!(
        store.cancel(&f.run.id).await.unwrap().state,
        RunState::CancelRequested
    );
    assert!(store.claim().await.unwrap().is_none());
    let env = store
        .environment(
            &f.run.spec.project_id,
            &f.run.spec.environment.as_ref().unwrap().environment_id,
        )
        .await
        .unwrap();
    store
        .change_environment(
            &env.project_id,
            &env.id,
            env.revision,
            "revoked",
            &EnvironmentChange::Revoke,
        )
        .await
        .unwrap();
    let mut done = result();
    done.termination = ProcessTermination::Canceled;
    store
        .finish_process(&f.run.id, &f.token, &f.tool.id, done)
        .await
        .unwrap();
    let stored = store.stored_run(&f.run.id).await.unwrap();
    assert_eq!(stored.record.state, RunState::Canceled);
    assert!(stored.lease.is_none());
    assert!(stored.checkpoint.in_flight.is_none());
    assert_eq!(store.claim().await.unwrap().unwrap().record.id, waiting.id);
}
#[tokio::test]
async fn failed_admission_and_settlement_transactions_never_publish_partial_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER fail_process_insert BEFORE INSERT ON process_attempts BEGIN SELECT RAISE(ABORT,'synthetic commit failure'); END").execute(&pool).await.unwrap();
    let before = canonical_hash(&store.stored_run(&f.run.id).await.unwrap());
    let events = store.events(&f.run.id, 0).await.unwrap().len();
    assert!(
        store
            .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
            .await
            .is_err()
    );
    assert_eq!(
        canonical_hash(&store.stored_run(&f.run.id).await.unwrap()),
        before
    );
    assert_eq!(store.events(&f.run.id, 0).await.unwrap().len(), events);
    sqlx::query("DROP TRIGGER fail_process_insert")
        .execute(&pool)
        .await
        .unwrap();
    store
        .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_process_update BEFORE UPDATE ON process_attempts BEGIN SELECT RAISE(ABORT,'synthetic result failure'); END").execute(&pool).await.unwrap();
    assert!(
        store
            .finish_process(&f.run.id, &f.token, &f.tool.id, result())
            .await
            .is_err()
    );
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_some()
    );
    assert_eq!(
        store.process_attempts(&f.run.id).await.unwrap()[0].state,
        ProcessAttemptState::LaunchCommitted
    );
    assert!(matches!(
        store
            .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
            .await
            .unwrap(),
        ProcessAdmission::Existing(_)
    ));
    expire(&store, &f).await;
    assert!(store.resume(&f.run.id).await.is_err());
    assert!(store.cancel(&f.run.id).await.is_err());
    pool.close().await;
}
#[tokio::test]
async fn corrupted_process_indexes_or_frozen_bindings_are_rejected_without_repair() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    store
        .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let original: String =
        sqlx::query_scalar("SELECT payload FROM process_attempts WHERE run_id=$1")
            .bind(&f.run.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before = canonical_hash(&store.stored_run(&f.run.id).await.unwrap());
    for (pointer, value) in [
        ("/record/id", json!("changed-id")),
        ("/record/run_id", json!("foreign-run")),
        ("/record/project_id", json!("foreign-project")),
        ("/record/tool/id", json!("changed-call")),
        ("/record/environment/version_id", json!("other-version")),
        (
            "/record/environment/qualification_sha256",
            json!("f".repeat(64)),
        ),
        ("/record/workspace_resource_id", json!("foreign-workspace")),
        ("/record/request_sha256", json!("f".repeat(64))),
        ("/record/revision", json!(7)),
    ] {
        let mut altered: serde_json::Value = serde_json::from_str(&original).unwrap();
        *altered.pointer_mut(pointer).unwrap() = value;
        let corrupted = altered.to_string();
        sqlx::query("UPDATE process_attempts SET payload=$1 WHERE run_id=$2")
            .bind(&corrupted)
            .bind(&f.run.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.detail(&f.run.id).await.is_err(), "{pointer}");
        assert!(
            store.export_order(&f.run.spec.order_id).await.is_err(),
            "{pointer}"
        );
        assert!(
            store
                .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
                .await
                .is_err(),
            "{pointer}"
        );
        let retained: String =
            sqlx::query_scalar("SELECT payload FROM process_attempts WHERE run_id=$1")
                .bind(&f.run.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(retained, corrupted);
        assert_eq!(
            canonical_hash(&store.stored_run(&f.run.id).await.unwrap()),
            before
        );
    }
    sqlx::query("UPDATE process_attempts SET payload=$1 WHERE run_id=$2")
        .bind(&original)
        .bind(&f.run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .detail(&f.run.id)
            .await
            .unwrap()
            .process_attempts
            .len(),
        1
    );
    pool.close().await;
}
#[tokio::test]
async fn uncertain_admission_requires_exact_reconciliation_and_explicit_resume() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite3");
    let store = Store::sqlite(&path).await.unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    let permit = match store
        .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
        .await
        .unwrap()
    {
        ProcessAdmission::Launch(p) => p,
        _ => panic!("first admission must grant one permit"),
    };
    let attempt = permit.into_attempt();
    expire(&store, &f).await;
    assert!(matches!(
        store
            .finish_process(&f.run.id, &f.token, &f.tool.id, result())
            .await,
        Err(Error::StaleLease)
    ));
    let resolution = ProcessResolution::NotStarted {
        evidence_sha256: hash(b"synthetic runner evidence: launch never occurred"),
    };
    assert!(
        store
            .reconcile_process(&f.run.id, &f.tool.id, &"0".repeat(64), resolution.clone())
            .await
            .is_err()
    );
    assert!(
        store
            .stored_run(&f.run.id)
            .await
            .unwrap()
            .checkpoint
            .in_flight
            .is_some()
    );
    let reconciled = store
        .reconcile_process(
            &f.run.id,
            &f.tool.id,
            &attempt.request_sha256,
            resolution.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reconcile_process(&f.run.id, &f.tool.id, &attempt.request_sha256, resolution)
            .await
            .unwrap(),
        reconciled
    );
    assert_eq!(
        store.stored_run(&f.run.id).await.unwrap().record.state,
        RunState::Interrupted
    );
    store.resume(&f.run.id).await.unwrap();
    let next = store.claim().await.unwrap().unwrap();
    let token = next.lease.unwrap().token;
    assert!(next.checkpoint.pending_tools.is_empty());
    assert!(matches!(
        store
            .admit_process(&f.run.id, &token, &f.tool, &f.observation)
            .await
            .unwrap(),
        ProcessAdmission::Existing(_)
    ));
    assert_eq!(store.process_attempts(&f.run.id).await.unwrap().len(), 1);
    store.close().await;
    let store = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        store.process_attempts(&f.run.id).await.unwrap()[0],
        reconciled
    );
}
#[test]
fn process_arguments_and_output_evidence_are_bounded_and_checked() {
    let mut tool = ToolCall {
        id: "call".into(),
        name: "execute_program".into(),
        arguments: json!(ProcessInvocation {
            program: "fixture".into(),
            arguments: vec![],
            timeout_seconds: 10
        }),
    };
    process_invocation(&tool).unwrap();
    tool.arguments["environment"] = json!({"PATH":"unexpected"});
    assert!(process_invocation(&tool).is_err());
    let good = result();
    validate_process_result(&good).unwrap();
    let mut bad = good.clone();
    bad.stdout.bytes[0] = 3;
    assert!(validate_process_result(&bad).is_err());
    let mut bad = good.clone();
    bad.stdout = capture(&vec![0; 65_537]);
    assert!(validate_process_result(&bad).is_err());
    let mut bad = good.clone();
    bad.stdout.total_bytes = 20;
    assert!(validate_process_result(&bad).is_err());
    let mut bad = good.clone();
    bad.process_id = None;
    assert!(validate_process_result(&bad).is_err());
    let mut bad = good;
    bad.signal = Some(9);
    assert!(validate_process_result(&bad).is_err());
}

#[test]
#[ignore = "Synthetic adapter command invoked only through the real process supervisor"]
fn process_effect_fixture() {
    use std::io::Write;
    let root = std::path::PathBuf::from(std::env::var_os("COWORK_PROCESS_EFFECT").unwrap());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(root.join("effect"))
        .unwrap();
    file.write_all(b"once\n").unwrap();
    file.sync_all().unwrap();
}
fn supervised_effect(root: &std::path::Path) -> ProcessResult {
    let launch = cowork_executor::TrustedLaunch {
        program: std::env::current_exe().unwrap(),
        arguments: vec![
            "--exact".into(),
            "process_effect_fixture".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        directory: root.into(),
        environment: BTreeMap::from([(
            "COWORK_PROCESS_EFFECT".into(),
            root.to_str().unwrap().into(),
        )]),
        timeout: std::time::Duration::from_secs(10),
        capture_bytes_per_stream: 4096,
    };
    let done =
        cowork_executor::supervise(&launch, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert_eq!(done.exit_code, Some(0));
    ProcessResult {
        termination: ProcessTermination::Exited,
        process_id: done.process_id,
        exit_code: done.exit_code,
        signal: done.signal,
        stdout: capture(&done.stdout.bytes),
        stderr: capture(&done.stderr.bytes),
        duration_ms: done.duration_ms,
        output_complete: done.output_complete,
    }
}
#[tokio::test]
#[ignore = "Synthetic worker killed by hard_crash_after_effect_does_not_replay_process"]
async fn process_crash_fixture() {
    use std::io::Write;
    let root = std::path::PathBuf::from(std::env::var_os("COWORK_PROCESS_CRASH").unwrap());
    let store = Store::sqlite(&root.join("state.sqlite3")).await.unwrap();
    let f = setup(&store, AccessMode::Autonomous).await;
    let permit = match store
        .admit_process(&f.run.id, &f.token, &f.tool, &f.observation)
        .await
        .unwrap()
    {
        ProcessAdmission::Launch(p) => p,
        _ => panic!("no first permit"),
    };
    let attempt = permit.into_attempt();
    let done = supervised_effect(&root);
    // Actual external effect happened; simulate losing the DB completion afterwards.
    let mut marker = std::fs::File::create(root.join("after-effect.json")).unwrap();
    marker
        .write_all(
            &serde_json::to_vec(
                &json!({"run":f.run,"tool":f.tool,"attempt":attempt,"result":done}),
            )
            .unwrap(),
        )
        .unwrap();
    marker.sync_all().unwrap();
    drop(marker);
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}
#[test]
fn hard_crash_after_effect_does_not_replay_process() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "process_crash_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("COWORK_PROCESS_CRASH", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let until = Instant::now() + Duration::from_secs(15);
    let value: serde_json::Value = loop {
        if let Ok(bytes) = std::fs::read(temp.path().join("after-effect.json"))
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            break value;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before effect"
        );
        assert!(Instant::now() < until, "fixture timed out");
        std::thread::sleep(Duration::from_millis(10));
    };
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = Store::sqlite(&temp.path().join("state.sqlite3"))
            .await
            .unwrap();
        let run: RunRecord = serde_json::from_value(value["run"].clone()).unwrap();
        let tool: ToolCall = serde_json::from_value(value["tool"].clone()).unwrap();
        let attempt: ProcessAttempt = serde_json::from_value(value["attempt"].clone()).unwrap();
        let result: ProcessResult = serde_json::from_value(value["result"].clone()).unwrap();
        store
            .mutate(&run.id, None, |r| {
                r.lease.as_mut().unwrap().until = 0;
                Ok(Change::default())
            })
            .await
            .unwrap();
        store.recover_expired_leases().await.unwrap();
        assert!(store.resume(&run.id).await.is_err());
        assert_eq!(store.process_attempts(&run.id).await.unwrap()[0], attempt);
        let proof = canonical_hash(&value);
        store
            .reconcile_process(
                &run.id,
                &tool.id,
                &attempt.request_sha256,
                ProcessResolution::Completed {
                    result,
                    evidence_sha256: proof,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store.stored_run(&run.id).await.unwrap().record.state,
            RunState::Interrupted
        );
        store.resume(&run.id).await.unwrap();
        assert!(
            store
                .stored_run(&run.id)
                .await
                .unwrap()
                .checkpoint
                .pending_tools
                .is_empty()
        );
        assert_eq!(
            std::fs::read(temp.path().join("effect")).unwrap(),
            b"once\n"
        );
        let archive = store.export_order(&run.spec.order_id).await.unwrap();
        assert_eq!(archive.data.runs[0].process_attempts.len(), 1);
        assert_eq!(archive.sha256, canonical_hash(&archive.data));
        store.close().await;
    });
}
