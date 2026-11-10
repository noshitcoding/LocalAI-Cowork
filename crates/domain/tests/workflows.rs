use cowork_contracts::{Budgets, ProviderKind, ProviderProfile};
use cowork_domain::{Error, canonical_hash, workflows::*};
use std::collections::BTreeSet;

fn rights(tools: &[&str], folders: &[&str]) -> WorkflowRights {
    WorkflowRights {
        current_workspace: false,
        tools: tools.iter().map(|s| s.to_string()).collect(),
        folders: folders.iter().map(|s| s.to_string()).collect(),
    }
}
fn limits() -> Budgets {
    Budgets {
        max_steps: 8,
        max_seconds: 60,
        max_output_tokens: 256,
        max_total_tokens: 8192,
    }
}
fn fixture() -> (WorkflowDefinition, WorkflowScope, Vec<ProviderProfile>) {
    let all = rights(&["read", "write"], &["project", "private"]);
    let role = WorkflowRole {
        agent_config: None,
        model_binding: None,
        output: None,
        id: "author".into(),
        instructions: "Produce the requested result".into(),
        provider_id: "local".into(),
        provider_revision: 3,
        rights: all.clone(),
        budgets: limits(),
        delegate_to: BTreeSet::new(),
    };
    let step = |id: &str, deps: &[&str]| WorkflowStep {
        execution: None,
        zero_context: false,
        id: id.into(),
        role_id: "author".into(),
        goal: format!("Result for {id}"),
        dependencies: deps.iter().map(|s| s.to_string()).collect(),
        rights: all.clone(),
        budgets: limits(),
    };
    (
        WorkflowDefinition {
            documents: Default::default(),
            iterations: Default::default(),
            subflows: Default::default(),
            controls: Default::default(),
            gates: Default::default(),
            dataflow: None,
            format_version: 1,
            name: "Create and review".into(),
            mode: WorkflowMode::Parallel { max_parallel: 3 },
            failure: WorkflowFailure::ContinueIndependent,
            rights: all.clone(),
            budgets: limits(),
            roles: vec![role],
            steps: vec![step("a", &[]), step("b", &[]), step("c", &["a", "b"])],
        },
        WorkflowScope {
            workspace_folder: None,
            rights: rights(&["read"], &["project"]),
            budgets: limits(),
            max_parallel: 2,
        },
        vec![ProviderProfile {
            generation: Default::default(),
            id: "local".into(),
            name: "Local model".into(),
            kind: ProviderKind::Ollama,
            endpoint: "http://127.0.0.1:11434".into(),
            model: "explicit-model".into(),
            revision: 3,
            context_tokens: 2048,
            credential_required: false,
            credential_ready: true,
            capabilities: Default::default(),
        }],
    )
}
fn compile() -> WorkflowPlan {
    let (d, s, p) = fixture();
    WorkflowPlan::compile(&d, &s, &p).unwrap()
}

#[test]
fn crew_review_preserves_all_64_tasks_without_relaxing_generic_workflows() {
    let (mut definition, scope, providers) = fixture();
    let role = format!("crew-role-{:040x}", 1);
    definition.roles[0].id = role.clone();
    let template = definition.steps[0].clone();
    definition.steps = (0..64)
        .map(|index| WorkflowStep {
            execution: None,
            zero_context: false,
            id: format!("crew-task-{index:040x}"),
            role_id: role.clone(),
            dependencies: vec![],
            ..template.clone()
        })
        .collect();
    let dependencies = definition
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();
    definition.steps.push(WorkflowStep {
        execution: None,
        zero_context: false,
        id: "crew-manager-review".into(),
        role_id: role.clone(),
        dependencies,
        ..template
    });
    definition.mode = WorkflowMode::Hierarchical {
        manager_step_id: Some("crew-manager-review".into()),
        max_parallel: 2,
        max_depth: 4,
        max_delegations: 8,
    };
    assert!(definition.validate_definition().is_err());
    assert!(WorkflowPlan::compile(&definition, &scope, &providers).is_err());
    let plan = WorkflowPlan::compile_crew_review(&definition, &scope, &providers).unwrap();
    assert_eq!(plan.nodes.len(), 65);
    let bytes = serde_json::to_vec(&plan).unwrap();
    let restored: WorkflowPlan = serde_json::from_slice(&bytes).unwrap();
    restored.validate().unwrap();
    let mut unmarked = restored;
    unmarked.crew_review = None;
    assert!(unmarked.validate().is_err());
    definition.steps[64].dependencies.pop();
    assert!(WorkflowPlan::compile_crew_review(&definition, &scope, &providers).is_err());
    assert!(
        serde_json::to_value(compile())
            .unwrap()
            .get("crew_review")
            .is_none()
    );
}

#[test]
fn tool_sampling_requires_the_original_pending_mcp_claim() {
    let (mut definition, mut scope, providers) = fixture();
    definition.rights.tools.insert("mcp_original".into());
    definition.roles[0]
        .rights
        .tools
        .insert("mcp_original".into());
    definition.steps[0]
        .rights
        .tools
        .insert("mcp_original".into());
    scope.rights.tools.insert("mcp_original".into());
    let plan = WorkflowPlan::compile(&definition, &scope, &providers).unwrap();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress.claim(&plan, progress.revision(), "a", 1).unwrap();
    progress
        .admit(
            &plan,
            progress.revision(),
            &claim,
            "original-tool",
            WorkflowOperation::Tool {
                tool_id: "mcp_original".into(),
                arguments_sha256: "a".repeat(64),
            },
            2,
        )
        .unwrap();
    let before = canonical_hash(&progress);
    assert!(
        progress
            .admit(
                &plan,
                progress.revision(),
                &claim,
                "ordinary-model",
                WorkflowOperation::Model {
                    request_sha256: "b".repeat(64),
                    reserved_tokens: None,
                },
                3
            )
            .is_err()
    );
    assert!(
        progress
            .admit_tool_sampling(
                &plan,
                progress.revision(),
                &claim,
                "sample",
                WorkflowSamplingRequest {
                    parent_tool_id: "different-tool",
                    request_sha256: &"b".repeat(64),
                    reserved_tokens: None
                },
                3
            )
            .is_err()
    );
    assert_eq!(canonical_hash(&progress), before);
    assert!(matches!(
        progress
            .admit_tool_sampling(
                &plan,
                progress.revision(),
                &claim,
                "sample",
                WorkflowSamplingRequest {
                    parent_tool_id: "original-tool",
                    request_sha256: &"b".repeat(64),
                    reserved_tokens: None
                },
                3
            )
            .unwrap(),
        WorkflowAdmission::Dispatch(_)
    ));
    assert_eq!(progress.attempts()["sample"].reserved_tokens, 2048);
    assert!(matches!(
        progress
            .admit_tool_sampling(
                &plan,
                progress.revision(),
                &claim,
                "sample",
                WorkflowSamplingRequest {
                    parent_tool_id: "original-tool",
                    request_sha256: &"b".repeat(64),
                    reserved_tokens: None
                },
                3
            )
            .unwrap(),
        WorkflowAdmission::Existing(_)
    ));
    progress.validate(&plan).unwrap();
    let bytes = serde_json::to_vec(&progress).unwrap();
    let mut restored: WorkflowProgress = serde_json::from_slice(&bytes).unwrap();
    restored.validate(&plan).unwrap();
    restored
        .settle_response(
            &plan,
            restored.revision(),
            &claim,
            "sample",
            WorkflowResponseEvidence {
                outcome: WorkflowOutcome::Measured {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                sha256: "c".repeat(64),
            },
            4,
        )
        .unwrap();
    assert_eq!(restored.attempts()["sample"].charged_tokens(), 15);
    assert!(restored.attempts()["original-tool"].outcome.is_none());
    restored
        .settle(
            &plan,
            restored.revision(),
            &claim,
            "original-tool",
            WorkflowOutcome::Succeeded,
            5,
        )
        .unwrap();
    restored.validate(&plan).unwrap();
}

#[test]
fn tool_sampling_consumes_the_existing_graph_step_limit() {
    let (mut definition, mut scope, providers) = fixture();
    definition.budgets.max_steps = 1;
    scope.budgets.max_steps = 1;
    definition.rights.tools.insert("mcp_original".into());
    definition.roles[0]
        .rights
        .tools
        .insert("mcp_original".into());
    definition.steps[0]
        .rights
        .tools
        .insert("mcp_original".into());
    scope.rights.tools.insert("mcp_original".into());
    let plan = WorkflowPlan::compile(&definition, &scope, &providers).unwrap();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress.claim(&plan, progress.revision(), "a", 1).unwrap();
    progress
        .admit(
            &plan,
            progress.revision(),
            &claim,
            "original-tool",
            WorkflowOperation::Tool {
                tool_id: "mcp_original".into(),
                arguments_sha256: "a".repeat(64),
            },
            2,
        )
        .unwrap();
    let before = canonical_hash(&progress);
    assert!(matches!(
        progress.admit_tool_sampling(
            &plan,
            progress.revision(),
            &claim,
            "sample",
            WorkflowSamplingRequest {
                parent_tool_id: "original-tool",
                request_sha256: &"b".repeat(64),
                reserved_tokens: None
            },
            3
        ),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(canonical_hash(&progress), before);
}
fn hierarchy(depth: u16, delegations: u16) -> WorkflowPlan {
    let (mut d, s, p) = fixture();
    d.mode = WorkflowMode::Hierarchical {
        manager_step_id: None,
        max_parallel: 2,
        max_depth: depth,
        max_delegations: delegations,
    };
    d.roles[0].delegate_to.insert("author".into());
    d.steps.truncate(1);
    WorkflowPlan::compile(&d, &s, &p).unwrap()
}
fn request() -> WorkflowDelegation {
    WorkflowDelegation {
        role_id: "author".into(),
        goal: "Create a bounded part".into(),
        rights: rights(&["read", "write", "admin"], &["project", "private"]),
        budgets: limits(),
    }
}
fn model() -> WorkflowOperation {
    WorkflowOperation::Model {
        reserved_tokens: None,
        request_sha256: "a".repeat(64),
    }
}
fn tool(name: &str) -> WorkflowOperation {
    WorkflowOperation::Tool {
        tool_id: name.into(),
        arguments_sha256: "b".repeat(64),
    }
}
fn measured() -> WorkflowOutcome {
    WorkflowOutcome::Measured {
        input_tokens: 256,
        output_tokens: 256,
    }
}
fn claim(state: &mut WorkflowProgress, plan: &WorkflowPlan, node: &str) -> WorkflowClaim {
    state.claim(plan, state.revision(), node, 100).unwrap()
}
fn admit(
    state: &mut WorkflowProgress,
    plan: &WorkflowPlan,
    lease: &WorkflowClaim,
    id: &str,
    op: WorkflowOperation,
) {
    let WorkflowAdmission::Dispatch(permit) = state
        .admit(plan, state.revision(), lease, id, op.clone(), 100)
        .unwrap()
    else {
        panic!("new action must have exactly one dispatch permit")
    };
    assert_eq!(permit.id(), id);
    assert_eq!(permit.into_dispatch(), (id.into(), lease.clone(), op));
}
fn finish(state: &mut WorkflowProgress, plan: &WorkflowPlan, lease: &WorkflowClaim) {
    state
        .complete(plan, state.revision(), lease, "Result", 100)
        .unwrap();
}
fn snapshot(state: &WorkflowProgress) -> String {
    serde_json::to_string(state).unwrap()
}

#[test]
fn compiler_freezes_explicit_provider_and_intersects_all_rights() {
    let plan = compile();
    plan.validate().unwrap();
    assert_eq!(plan.max_parallel, 2);
    assert_eq!(plan.nodes["a"].rights, rights(&["read"], &["project"]));
    assert_eq!(plan.roles["author"].provider.model, "explicit-model");
    let (mut d, s, p) = fixture();
    d.roles[0].provider_revision = 2;
    assert!(matches!(
        WorkflowPlan::compile(&d, &s, &p),
        Err(Error::Conflict)
    ));
    d.roles[0].provider_id = "deleted".into();
    assert!(matches!(
        WorkflowPlan::compile(&d, &s, &p),
        Err(Error::Provider(_))
    ));
}
#[test]
fn invalid_graphs_and_reserved_child_ids_are_rejected() {
    let (d, s, p) = fixture();
    let mut cycle = d.clone();
    cycle.steps[0].dependencies.push("c".into());
    assert!(WorkflowPlan::compile(&cycle, &s, &p).is_err());
    let mut missing = d.clone();
    missing.steps[0].dependencies.push("missing".into());
    assert!(WorkflowPlan::compile(&missing, &s, &p).is_err());
    let mut duplicate = d.clone();
    duplicate.steps[2].dependencies.push("a".into());
    assert!(WorkflowPlan::compile(&duplicate, &s, &p).is_err());
    let mut reserved = d.clone();
    reserved.steps[0].id = "delegated-1".into();
    assert!(WorkflowPlan::compile(&reserved, &s, &p).is_err());
    let mut self_cycle = d;
    self_cycle.steps[0].dependencies.push("a".into());
    assert!(WorkflowPlan::compile(&self_cycle, &s, &p).is_err());
}
#[test]
fn invalid_profile_and_delegation_configuration_are_rejected() {
    let (mut d, mut s, mut p) = fixture();
    p[0].credential_ready = false;
    assert!(WorkflowPlan::compile(&d, &s, &p).is_err());
    p[0].credential_ready = true;
    d.roles[0].delegate_to.insert("author".into());
    assert!(WorkflowPlan::compile(&d, &s, &p).is_err());
    d.mode = WorkflowMode::Hierarchical {
        manager_step_id: None,
        max_parallel: 2,
        max_depth: 2,
        max_delegations: 4,
    };
    d.roles[0].delegate_to.insert("missing".into());
    assert!(WorkflowPlan::compile(&d, &s, &p).is_err());
    d.roles[0].delegate_to.remove("missing");
    d.budgets.max_output_tokens = 2048;
    d.roles[0].budgets.max_output_tokens = 2048;
    // A stricter authorized scope remains valid. Enlarging it exposes the invalid model limit.
    WorkflowPlan::compile(&d, &s, &p).unwrap();
    s.budgets.max_output_tokens = 2048;
    assert!(matches!(
        WorkflowPlan::compile(&d, &s, &p),
        Err(Error::BudgetConfiguration)
    ));
}
#[test]
fn sequential_order_is_explicit_and_backwards_dependencies_fail() {
    let (mut d, s, p) = fixture();
    d.mode = WorkflowMode::Sequential;
    let plan = WorkflowPlan::compile(&d, &s, &p).unwrap();
    let state = WorkflowProgress::new(&plan).unwrap();
    assert_eq!(state.ready_nodes(&plan).unwrap(), vec!["a"]);
    assert!(plan.nodes["b"].dependencies.contains("a"));
    d.steps[0].dependencies.push("b".into());
    assert!(WorkflowPlan::compile(&d, &s, &p).is_err());
}
#[test]
fn parallel_dependencies_and_capacity_survive_checkpoint_roundtrip() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    assert_eq!(state.ready_nodes(&plan).unwrap(), vec!["a", "b"]);
    let a = claim(&mut state, &plan, "a");
    let b = claim(&mut state, &plan, "b");
    assert!(state.claim(&plan, state.revision(), "c", 100).is_err());
    finish(&mut state, &plan, &a);
    let mut state: WorkflowProgress = serde_json::from_str(&snapshot(&state)).unwrap();
    state.validate(&plan).unwrap();
    assert!(state.ready_nodes(&plan).unwrap().is_empty());
    finish(&mut state, &plan, &b);
    assert_eq!(state.ready_nodes(&plan).unwrap(), vec!["c"]);
    let c = claim(&mut state, &plan, "c");
    finish(&mut state, &plan, &c);
    assert_eq!(state.state(), WorkflowState::Completed);
}
#[test]
fn independent_nodes_cannot_exceed_parallel_limit() {
    let (mut d, s, p) = fixture();
    d.steps[2].dependencies.clear();
    let plan = WorkflowPlan::compile(&d, &s, &p).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    claim(&mut state, &plan, "a");
    claim(&mut state, &plan, "b");
    let before = snapshot(&state);
    assert!(matches!(
        state.claim(&plan, state.revision(), "c", 100),
        Err(Error::WorkspaceBusy)
    ));
    assert_eq!(snapshot(&state), before);
}
#[test]
fn shared_token_reservations_prevent_parallel_budget_multiplication() {
    let (mut d, mut s, p) = fixture();
    d.steps[2].dependencies.clear();
    d.budgets.max_total_tokens = 4096;
    s.max_parallel = 3;
    let plan = WorkflowPlan::compile(&d, &s, &p).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let b = claim(&mut state, &plan, "b");
    let c = claim(&mut state, &plan, "c");
    admit(&mut state, &plan, &a, "m1", model());
    admit(&mut state, &plan, &b, "m2", model());
    assert_eq!(state.charged_tokens(), 4096);
    let before = snapshot(&state);
    assert!(matches!(
        state.admit(&plan, state.revision(), &c, "m3", model(), 100),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(snapshot(&state), before);
    state
        .settle(&plan, state.revision(), &a, "m1", measured(), 100)
        .unwrap();
    assert!(matches!(
        state.admit(&plan, state.revision(), &c, "m3", model(), 100),
        Err(Error::BudgetExceeded)
    ));
    state
        .settle(&plan, state.revision(), &b, "m2", measured(), 100)
        .unwrap();
    admit(&mut state, &plan, &c, "m3", model());
    assert_eq!(state.charged_tokens(), 3072);
}
#[test]
fn incomplete_usage_holds_full_context_and_overrun_interrupts() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    admit(&mut state, &plan, &a, "m1", model());
    state
        .settle(
            &plan,
            state.revision(),
            &a,
            "m1",
            WorkflowOutcome::Unmeasured {
                input_tokens: Some(50),
                output_tokens: None,
            },
            100,
        )
        .unwrap();
    assert_eq!(state.charged_tokens(), 2048);
    admit(&mut state, &plan, &a, "m2", model());
    state
        .settle(
            &plan,
            state.revision(),
            &a,
            "m2",
            WorkflowOutcome::Measured {
                input_tokens: 3000,
                output_tokens: 50,
            },
            100,
        )
        .unwrap();
    assert_eq!(state.charged_tokens(), 5098);
    assert_eq!(state.state(), WorkflowState::Interrupted);
    assert!(state.ready_nodes(&plan).unwrap().is_empty());
}
#[test]
fn admission_replay_returns_receipt_without_dispatch_or_budget_increment() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    admit(&mut state, &plan, &a, "m1", model());
    let before = snapshot(&state);
    let mut state: WorkflowProgress = serde_json::from_str(&before).unwrap();
    assert!(matches!(
        state.admit(&plan, 0, &a, "m1", model(), 100),
        Ok(WorkflowAdmission::Existing(_))
    ));
    assert_eq!(snapshot(&state), before);
    assert!(matches!(
        state.admit(&plan, state.revision(), &a, "m1", tool("read"), 100),
        Err(Error::Conflict)
    ));
    state
        .settle(&plan, state.revision(), &a, "m1", measured(), 100)
        .unwrap();
    let before = snapshot(&state);
    state.settle(&plan, 0, &a, "m1", measured(), 100).unwrap();
    assert_eq!(snapshot(&state), before);
}
#[test]
fn denied_tools_and_stale_claims_never_change_checkpoint() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let before = snapshot(&state);
    assert!(matches!(
        state.admit(&plan, state.revision(), &a, "write", tool("write"), 100),
        Err(Error::Forbidden)
    ));
    assert_eq!(snapshot(&state), before);
    let mut stale = a.clone();
    stale.token = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        state.complete(&plan, state.revision(), &stale, "false result", 100),
        Err(Error::StaleLease)
    ));
    assert_eq!(snapshot(&state), before);
    assert!(matches!(
        state.complete(&plan, 0, &a, "false result", 100),
        Err(Error::Conflict)
    ));
    assert_eq!(snapshot(&state), before);
}
#[test]
fn uncertain_tool_outcome_is_not_replayed_after_coordinator_recovery() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    admit(&mut state, &plan, &a, "t1", tool("read"));
    assert!(matches!(
        state.complete(&plan, state.revision(), &a, "unconfirmed", 100),
        Err(Error::UncertainEffect)
    ));
    state.recover(&plan, state.revision(), 101).unwrap();
    assert_eq!(state.state(), WorkflowState::Interrupted);
    assert_eq!(state.attempts().len(), 1);
    assert!(state.attempts()["t1"].outcome.is_none());
    assert!(matches!(
        state.admit(&plan, 0, &a, "t1", tool("read"), 101),
        Ok(WorkflowAdmission::Existing(_))
    ));
    assert!(matches!(
        state.settle(
            &plan,
            state.revision(),
            &a,
            "t1",
            WorkflowOutcome::Succeeded,
            101
        ),
        Err(Error::StaleLease)
    ));
    assert!(state.ready_nodes(&plan).unwrap().is_empty());
}
#[test]
fn uncertainty_blocks_other_agents_new_actions_but_allows_admitted_receipts() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let b = claim(&mut state, &plan, "b");
    admit(&mut state, &plan, &a, "t1", tool("read"));
    admit(&mut state, &plan, &b, "m1", model());
    state
        .settle(
            &plan,
            state.revision(),
            &a,
            "t1",
            WorkflowOutcome::Uncertain,
            100,
        )
        .unwrap();
    state
        .settle(&plan, state.revision(), &b, "m1", measured(), 100)
        .unwrap();
    assert!(matches!(
        state.admit(&plan, state.revision(), &b, "m2", model(), 100),
        Err(Error::UncertainEffect)
    ));
    finish(&mut state, &plan, &b);
    assert_eq!(state.state(), WorkflowState::Interrupted);
}
#[test]
fn continue_independent_blocks_dependency_chain_and_preserves_other_branch() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    state
        .fail(&plan, state.revision(), &a, "provider_unavailable", 100)
        .unwrap();
    assert_eq!(state.nodes()["c"].state, WorkflowNodeState::Blocked);
    assert_eq!(state.ready_nodes(&plan).unwrap(), vec!["b"]);
    let b = claim(&mut state, &plan, "b");
    finish(&mut state, &plan, &b);
    assert_eq!(state.state(), WorkflowState::Failed);
}
#[test]
fn stop_policy_denies_new_actions_and_settles_already_admitted_actions() {
    let (mut d, s, p) = fixture();
    d.failure = WorkflowFailure::Stop;
    let plan = WorkflowPlan::compile(&d, &s, &p).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let b = claim(&mut state, &plan, "b");
    admit(&mut state, &plan, &b, "m1", model());
    state
        .fail(&plan, state.revision(), &a, "test_failure", 100)
        .unwrap();
    state
        .settle(&plan, state.revision(), &b, "m1", measured(), 100)
        .unwrap();
    assert!(
        state
            .admit(&plan, state.revision(), &b, "m2", model(), 100)
            .is_err()
    );
    finish(&mut state, &plan, &b);
    assert_eq!(state.state(), WorkflowState::Failed);
}
#[test]
fn hierarchical_delegation_intersects_rights_and_reclaims_parent_with_new_token() {
    let plan = hierarchy(2, 4);
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let children = state
        .delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), request()],
            100,
        )
        .unwrap();
    let before = snapshot(&state);
    assert_eq!(
        state
            .delegate(&plan, 0, &a, "d1", vec![request(), request()], 100)
            .unwrap(),
        children
    );
    assert_eq!(snapshot(&state), before);
    assert_eq!(
        state.nodes()[&children[0]].plan.rights,
        rights(&["read"], &["project"])
    );
    let first = claim(&mut state, &plan, &children[0]);
    let second = claim(&mut state, &plan, &children[1]);
    finish(&mut state, &plan, &first);
    finish(&mut state, &plan, &second);
    let resumed = claim(&mut state, &plan, "a");
    assert_ne!(resumed.token, a.token);
    assert!(matches!(
        state.complete(&plan, state.revision(), &a, "stale", 100),
        Err(Error::StaleLease)
    ));
    finish(&mut state, &plan, &resumed);
    assert_eq!(state.state(), WorkflowState::Completed);
}
#[test]
fn delegation_receipt_conflicts_and_limits_are_atomic() {
    let plan = hierarchy(1, 2);
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let before = snapshot(&state);
    let mut forbidden = request();
    forbidden.role_id = "admin".into();
    assert!(
        state
            .delegate(
                &plan,
                state.revision(),
                &a,
                "denied",
                vec![request(), forbidden],
                100
            )
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    let children = state
        .delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), request()],
            100,
        )
        .unwrap();
    let before = snapshot(&state);
    let mut changed = request();
    changed.goal = "Different scope".into();
    assert!(matches!(
        state.delegate(&plan, state.revision(), &a, "d1", vec![changed], 100),
        Err(Error::Conflict)
    ));
    assert_eq!(snapshot(&state), before);
    let child = claim(&mut state, &plan, &children[0]);
    let before = snapshot(&state);
    assert!(matches!(
        state.delegate(&plan, state.revision(), &child, "d2", vec![request()], 100),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(snapshot(&state), before);
}
#[test]
fn descendants_share_their_parent_step_budget() {
    let mut plan = hierarchy(2, 4);
    let mut d = plan.definition.clone();
    d.steps[0].budgets.max_steps = 1;
    plan = WorkflowPlan::compile(&d, &plan.scope, &plan.profiles).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let children = state
        .delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), request()],
            100,
        )
        .unwrap();
    let first = claim(&mut state, &plan, &children[0]);
    let second = claim(&mut state, &plan, &children[1]);
    admit(&mut state, &plan, &first, "t1", tool("read"));
    assert!(matches!(
        state.admit(&plan, state.revision(), &second, "t2", tool("read"), 100),
        Err(Error::BudgetExceeded)
    ));
}
#[test]
fn failed_child_blocks_waiting_parent_and_pending_siblings() {
    let plan = hierarchy(2, 4);
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let children = state
        .delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), request()],
            100,
        )
        .unwrap();
    let child = claim(&mut state, &plan, &children[0]);
    state
        .fail(&plan, state.revision(), &child, "test_failure", 100)
        .unwrap();
    assert_eq!(state.nodes()["a"].state, WorkflowNodeState::Blocked);
    assert_eq!(
        state.nodes()[&children[1]].state,
        WorkflowNodeState::Blocked
    );
    assert_eq!(state.state(), WorkflowState::Failed);
}
#[test]
fn monotonic_clock_and_original_deadlines_cannot_be_reset() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let before = snapshot(&state);
    assert!(
        state
            .admit(&plan, state.revision(), &a, "rollback", model(), 99)
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    assert!(matches!(
        state.admit(&plan, state.revision(), &a, "expired", model(), 160),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(snapshot(&state), before);
    finish(&mut state, &plan, &a);
    assert!(matches!(
        state.claim(&plan, state.revision(), "b", 160),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(state.nodes()["b"].state, WorkflowNodeState::Pending);
}
#[test]
fn forged_checkpoint_and_frozen_plan_are_rejected_before_mutation() {
    let plan = compile();
    let state = WorkflowProgress::new(&plan).unwrap();
    let mut value = serde_json::to_value(&state).unwrap();
    value["nodes"]["a"]["plan"]["rights"]["tools"] = serde_json::json!(["admin"]);
    let mut forged: WorkflowProgress = serde_json::from_value(value).unwrap();
    let before = snapshot(&forged);
    assert!(forged.claim(&plan, 0, "a", 100).is_err());
    assert_eq!(snapshot(&forged), before);
    let mut altered = plan.clone();
    altered
        .nodes
        .get_mut("a")
        .unwrap()
        .rights
        .tools
        .insert("admin".into());
    altered.sha256.clear();
    altered.sha256 = canonical_hash(&altered);
    assert!(altered.validate().is_err());
    let mut value = serde_json::to_value(&state).unwrap();
    value["revision"] = 99.into();
    let forged: WorkflowProgress = serde_json::from_value(value).unwrap();
    assert!(forged.validate(&plan).is_err());
}

#[test]
fn delegated_models_share_parent_tokens_even_when_global_budget_has_capacity() {
    let old = hierarchy(2, 4);
    let mut d = old.definition.clone();
    d.steps[0].budgets.max_total_tokens = 4096;
    let plan = WorkflowPlan::compile(&d, &old.scope, &old.profiles).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let children = state
        .delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), request()],
            100,
        )
        .unwrap();
    let first = claim(&mut state, &plan, &children[0]);
    let second = claim(&mut state, &plan, &children[1]);
    admit(&mut state, &plan, &first, "m1", model());
    admit(&mut state, &plan, &second, "m2", model());
    state
        .settle(
            &plan,
            state.revision(),
            &first,
            "m1",
            WorkflowOutcome::Unmeasured {
                input_tokens: None,
                output_tokens: None,
            },
            100,
        )
        .unwrap();
    assert_eq!(state.charged_tokens(), 4096);
    assert!(state.charged_tokens() < plan.budgets.max_total_tokens);
    assert!(matches!(
        state.admit(&plan, state.revision(), &first, "m3", model(), 100),
        Err(Error::BudgetExceeded)
    ));
}
#[test]
fn coordinator_cannot_delegate_to_an_existing_but_unapproved_role() {
    let old = hierarchy(2, 4);
    let mut d = old.definition.clone();
    let mut reviewer = d.roles[0].clone();
    reviewer.id = "reviewer".into();
    reviewer.delegate_to.clear();
    d.roles.push(reviewer);
    let plan = WorkflowPlan::compile(&d, &old.scope, &old.profiles).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let mut review = request();
    review.role_id = "reviewer".into();
    let before = snapshot(&state);
    assert!(matches!(
        state.delegate(
            &plan,
            state.revision(),
            &a,
            "d1",
            vec![request(), review],
            100
        ),
        Err(Error::Forbidden)
    ));
    assert_eq!(snapshot(&state), before);
}
#[test]
fn a_resumed_parent_retains_its_initial_deadline() {
    let plan = hierarchy(2, 4);
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let children = state
        .delegate(&plan, state.revision(), &a, "d1", vec![request()], 100)
        .unwrap();
    let child = state
        .claim(&plan, state.revision(), &children[0], 150)
        .unwrap();
    state
        .complete(&plan, state.revision(), &child, "child result", 159)
        .unwrap();
    let before = snapshot(&state);
    assert!(matches!(
        state.claim(&plan, state.revision(), "a", 160),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(snapshot(&state), before);
}
#[test]
fn outputs_and_model_usage_are_validated_before_settlement_or_completion() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    admit(&mut state, &plan, &a, "m1", model());
    let before = snapshot(&state);
    assert!(
        state
            .settle(
                &plan,
                state.revision(),
                &a,
                "m1",
                WorkflowOutcome::Succeeded,
                100
            )
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    assert!(
        state
            .settle(
                &plan,
                state.revision(),
                &a,
                "m1",
                WorkflowOutcome::Measured {
                    input_tokens: u64::MAX,
                    output_tokens: 1
                },
                100
            )
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    state
        .settle(&plan, state.revision(), &a, "m1", measured(), 100)
        .unwrap();
    let before = snapshot(&state);
    assert!(
        state
            .complete(&plan, state.revision(), &a, &"x".repeat(65_537), 100)
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    state.settle(&plan, 0, &a, "m1", measured(), 100).unwrap();
    assert_eq!(snapshot(&state), before);
}

#[test]
fn atomic_database_tool_result_is_one_replay_bound_budgeted_decision() {
    let plan = compile();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let before = state.revision();
    state
        .record_tool_result(
            &plan,
            before,
            &a,
            "db-tool",
            "read",
            &"b".repeat(64),
            &"c".repeat(64),
            true,
            100,
        )
        .unwrap();
    assert_eq!(state.revision(), before + 1);
    assert_eq!(state.attempts().len(), 1);
    assert_eq!(
        state.attempts()["db-tool"].outcome,
        Some(WorkflowOutcome::Succeeded)
    );
    assert_eq!(state.charged_tokens(), 0);
    let saved = snapshot(&state);
    state
        .record_tool_result(
            &plan,
            0,
            &a,
            "db-tool",
            "read",
            &"b".repeat(64),
            &"c".repeat(64),
            true,
            0,
        )
        .unwrap();
    assert_eq!(snapshot(&state), saved);
    assert!(
        state
            .record_tool_result(
                &plan,
                state.revision(),
                &a,
                "db-tool",
                "read",
                &"b".repeat(64),
                &"d".repeat(64),
                true,
                100
            )
            .is_err()
    );
    let mut restored: WorkflowProgress = serde_json::from_str(&saved).unwrap();
    restored.validate(&plan).unwrap();
    assert!(
        restored
            .settle(
                &plan,
                restored.revision(),
                &a,
                "db-tool",
                WorkflowOutcome::Failed,
                100
            )
            .is_err()
    );
    let mut altered = serde_json::to_value(&restored).unwrap();
    altered["attempts"]["db-tool"]["response_sha256"] = serde_json::json!("d".repeat(64));
    assert!(
        serde_json::from_value::<WorkflowProgress>(altered)
            .unwrap()
            .validate(&plan)
            .is_err()
    );
}
#[test]
fn atomic_database_tool_result_rejects_rights_budget_and_bad_evidence_without_changes() {
    // Compile a one-step shared budget so the second tool cannot be admitted.
    let (mut d, mut scope, providers) = fixture();
    d.budgets.max_steps = 1;
    scope.budgets.max_steps = 1;
    let plan = WorkflowPlan::compile(&d, &scope, &providers).unwrap();
    let mut state = WorkflowProgress::new(&plan).unwrap();
    let a = claim(&mut state, &plan, "a");
    let before = snapshot(&state);
    assert!(
        state
            .record_tool_result(
                &plan,
                state.revision(),
                &a,
                "denied",
                "write",
                &"b".repeat(64),
                &"c".repeat(64),
                true,
                100
            )
            .is_err()
    );
    assert!(
        state
            .record_tool_result(
                &plan,
                state.revision(),
                &a,
                "invalid",
                "read",
                &"b".repeat(64),
                "bad",
                true,
                100
            )
            .is_err()
    );
    assert_eq!(snapshot(&state), before);
    state
        .record_tool_result(
            &plan,
            state.revision(),
            &a,
            "first",
            "read",
            &"b".repeat(64),
            &"c".repeat(64),
            false,
            100,
        )
        .unwrap();
    let before = snapshot(&state);
    assert!(matches!(
        state.record_tool_result(
            &plan,
            state.revision(),
            &a,
            "second",
            "read",
            &"b".repeat(64),
            &"c".repeat(64),
            true,
            100
        ),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(snapshot(&state), before);
}

fn input_plan() -> WorkflowPlan {
    let (mut definition, mut scope, profiles) = fixture();
    definition.rights.tools.insert("request_input".into());
    for role in &mut definition.roles {
        role.rights.tools.insert("request_input".into());
    }
    for step in &mut definition.steps {
        step.rights.tools.insert("request_input".into());
    }
    scope.rights.tools.insert("request_input".into());
    WorkflowPlan::compile(&definition, &scope, &profiles).unwrap()
}
#[test]
fn human_input_reserves_one_step_and_only_typed_answer_can_settle_it() {
    let plan = input_plan();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress.claim(&plan, 0, "a", 0).unwrap();
    let args = canonical_hash(&serde_json::json!({"question":"Which source?"}));
    progress
        .open_input(&plan, progress.revision(), &claim, "question-a", &args, 1)
        .unwrap();
    let waiting_revision = progress.revision();
    assert_eq!(progress.attempts().len(), 1);
    assert_eq!(progress.charged_tokens(), 0);
    progress
        .open_input(&plan, 0, &claim, "question-a", &args, 1)
        .unwrap();
    assert_eq!(progress.revision(), waiting_revision);
    assert!(matches!(
        progress.settle(
            &plan,
            waiting_revision,
            &claim,
            "question-a",
            WorkflowOutcome::Succeeded,
            2
        ),
        Err(Error::Forbidden)
    ));
    let evidence = WorkflowInputAnswerEvidence {
        receipt_sha256: canonical_hash(&serde_json::json!({"answer":"  exact\r\nAntwort  "})),
        confirmed_sequence: 7,
        answer_key_sha256: canonical_hash(&"explicit-intent"),
    };
    progress
        .answer_input(&plan, waiting_revision, &claim, "question-a", &evidence, 2)
        .unwrap();
    let answered_revision = progress.revision();
    progress
        .answer_input(&plan, 0, &claim, "question-a", &evidence, 2)
        .unwrap();
    assert_eq!(progress.revision(), answered_revision);
    assert_eq!(progress.attempts().len(), 1);
    assert_eq!(
        progress.input_checkpoints()["question-a"].confirmed_sequence,
        Some(7)
    );
    let mut changed = evidence.clone();
    changed.confirmed_sequence += 1;
    assert!(matches!(
        progress.answer_input(&plan, answered_revision, &claim, "question-a", &changed, 3),
        Err(Error::Conflict)
    ));
    progress.validate(&plan).unwrap();
}
#[test]
fn input_checkpoint_is_replayed_and_cannot_revive_an_interrupted_claim() {
    let plan = input_plan();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let claim = progress.claim(&plan, 0, "a", 0).unwrap();
    let args = canonical_hash(&"question");
    progress
        .open_input(&plan, progress.revision(), &claim, "question-a", &args, 1)
        .unwrap();
    let mut corrupted = serde_json::to_value(&progress).unwrap();
    corrupted["inputs"]["question-a"]["arguments_sha256"] =
        serde_json::json!(canonical_hash(&"another"));
    let corrupted: WorkflowProgress = serde_json::from_value(corrupted).unwrap();
    assert!(matches!(corrupted.validate(&plan), Err(Error::Forbidden)));
    progress.recover(&plan, progress.revision(), 2).unwrap();
    let evidence = WorkflowInputAnswerEvidence {
        receipt_sha256: canonical_hash(&"receipt"),
        confirmed_sequence: 9,
        answer_key_sha256: canonical_hash(&"key"),
    };
    assert!(matches!(
        progress.answer_input(
            &plan,
            progress.revision(),
            &claim,
            "question-a",
            &evidence,
            3
        ),
        Err(Error::StaleLease)
    ));
    assert_eq!(progress.state(), WorkflowState::Interrupted);
    assert_eq!(progress.input_checkpoints().len(), 1);
    progress.validate(&plan).unwrap();
}
#[test]
fn parallel_inputs_do_not_free_steps_or_extend_the_original_deadline() {
    let plan = input_plan();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let a = progress.claim(&plan, 0, "a", 0).unwrap();
    let b = progress.claim(&plan, progress.revision(), "b", 0).unwrap();
    let args = canonical_hash(&"question");
    progress
        .open_input(&plan, progress.revision(), &a, "question-a", &args, 1)
        .unwrap();
    progress
        .open_input(&plan, progress.revision(), &b, "question-b", &args, 1)
        .unwrap();
    assert_eq!(progress.input_checkpoints().len(), 2);
    assert_eq!(progress.attempts().len(), 2);
    let before = canonical_hash(&progress);
    let evidence = WorkflowInputAnswerEvidence {
        receipt_sha256: canonical_hash(&"receipt"),
        confirmed_sequence: 9,
        answer_key_sha256: canonical_hash(&"key"),
    };
    assert!(matches!(
        progress.answer_input(&plan, progress.revision(), &a, "question-a", &evidence, 60),
        Err(Error::BudgetExceeded)
    ));
    assert_eq!(canonical_hash(&progress), before);
}

#[test]
fn known_human_wait_failure_closes_only_the_wait_and_preserves_unknown_model_evidence() {
    let plan = input_plan();
    let mut progress = WorkflowProgress::new(&plan).unwrap();
    let a = progress.claim(&plan, 0, "a", 0).unwrap();
    let b = progress.claim(&plan, progress.revision(), "b", 0).unwrap();
    progress
        .open_input(
            &plan,
            progress.revision(),
            &a,
            "question-a",
            &canonical_hash(&"question"),
            1,
        )
        .unwrap();
    progress
        .admit(
            &plan,
            progress.revision(),
            &b,
            "pending-provider",
            WorkflowOperation::Model {
                reserved_tokens: None,
                request_sha256: canonical_hash(&"request"),
            },
            1,
        )
        .unwrap();
    progress
        .fail(&plan, progress.revision(), &a, "budget_exceeded", 60)
        .unwrap();
    assert_eq!(
        progress.input_checkpoints()["question-a"]
            .closed_reason
            .as_deref(),
        Some("budget_exceeded")
    );
    assert_eq!(
        progress.attempts()["question-a"].outcome,
        Some(WorkflowOutcome::Failed)
    );
    assert_eq!(progress.attempts()["pending-provider"].outcome, None);
    assert!(matches!(
        progress.fail(&plan, progress.revision(), &b, "budget_exceeded", 60),
        Err(Error::UncertainEffect)
    ));
    progress.validate(&plan).unwrap();
}
