use cowork_contracts::{FolderGrant, ProviderProfile};
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};

fn definition() -> WorkflowDefinition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration.json"
    ))
    .unwrap()
}
fn grant(order: &str) -> FolderGrant {
    FolderGrant {
        id: format!("workspace:{order}"),
        project_id: "project".into(),
        name: "Managed workspace".into(),
        writable: true,
        revision: "a".repeat(64),
        resource_id: format!("existing-native-{order}"),
        order_id: Some(order.into()),
    }
}
fn scope(folder: Option<FolderGrant>) -> WorkflowScope {
    WorkflowScope {
        rights: WorkflowRights {
            folders: folder.iter().map(|f| f.id.clone()).collect(),
            ..Default::default()
        },
        workspace_folder: folder
            .as_ref()
            .and_then(|value| WorkflowWorkspaceBinding::from_grant(value).ok()),
        budgets: definition().budgets,
        max_parallel: 4,
    }
}
fn portable() -> WorkflowDefinition {
    let mut d = definition();
    d.rights.current_workspace = true;
    let call = d.iterations.get_mut("loop").unwrap();
    call.rights.current_workspace = true;
    call.definition.rights.current_workspace = true;
    d
}
fn derived(plan: &WorkflowPlan) -> WorkflowExecutionGraph<'_> {
    let mut p = WorkflowProgress::new(plan).unwrap();
    let capture = plan.iterations["loop"].capture_node.clone();
    let claim = p.claim(plan, 0, &capture, 100).unwrap();
    p.complete_control(
        plan,
        p.revision(),
        &claim,
        "capture",
        Some(&json!({"values":[1,2]})),
        100,
    )
    .unwrap();
    p.execution_graph(plan).unwrap()
}

#[test]
fn one_source_binds_each_confirmed_managed_grant_without_changing_the_source() {
    let mut d = portable();
    let body = &mut d.iterations.get_mut("loop").unwrap().definition;
    body.controls.clear();
    body.roles.push(serde_json::from_value(json!({"id":"worker","instructions":"Use the selected workspace.","provider_id":"model","provider_revision":1,"rights":{"current_workspace":true,"tools":[],"folders":[]},"budgets":body.budgets,"delegate_to":[]})).unwrap());
    body.steps.push(WorkflowStep {
        id: "echo".into(),
        role_id: "worker".into(),
        goal: "Use one item".into(),
        dependencies: vec![],
        rights: WorkflowRights {
            current_workspace: true,
            ..Default::default()
        },
        budgets: body.budgets.clone(),
        execution: None,
        zero_context: false,
    });
    let profiles:Vec<ProviderProfile>=vec![serde_json::from_value(json!({"id":"model","name":"Domain workspace fixture","kind":"ollama","endpoint":"http://127.0.0.1:11434","model":"frozen-model","revision":1,"context_tokens":2048,"credential_required":false,"credential_ready":true})).unwrap()];
    let original = serde_json::to_vec(&d).unwrap();
    let a = grant("a");
    let b = grant("b");
    let left = WorkflowPlan::compile(&d, &scope(Some(a.clone())), &profiles).unwrap();
    let right = WorkflowPlan::compile(&d, &scope(Some(b.clone())), &profiles).unwrap();
    assert_eq!(left.source_sha256, right.source_sha256);
    assert_eq!(left.source_sha256, canonical_hash(&d));
    assert_ne!(left.sha256, right.sha256);
    assert_eq!(
        left.scope.workspace_folder,
        Some(WorkflowWorkspaceBinding::from_grant(&a).unwrap())
    );
    assert_eq!(
        right.scope.workspace_folder,
        Some(WorkflowWorkspaceBinding::from_grant(&b).unwrap())
    );
    assert_eq!(
        left.resource_definition.as_deref().map(canonical_hash),
        Some(canonical_hash(&d))
    );
    for (plan, folder) in [(&left, &a), (&right, &b)] {
        plan.validate().unwrap();
        let graph = derived(plan);
        assert!(graph.nodes.values().all(|node| {
            !node.rights.current_workspace
                && node
                    .rights
                    .folders
                    .is_subset(&[folder.id.clone()].into_iter().collect())
        }));
        let models: Vec<_> = graph
            .nodes
            .values()
            .filter(|node| node.role_id.is_some())
            .collect();
        assert_eq!(models.len(), 2);
        assert!(
            models
                .iter()
                .all(|node| node.rights.folders == [folder.id.clone()].into_iter().collect())
        );
        assert!(!plan.definition.uses_current_workspace());
        let reopened: WorkflowPlan =
            serde_json::from_slice(&serde_json::to_vec(plan).unwrap()).unwrap();
        reopened.validate().unwrap();
    }
    assert_eq!(original, serde_json::to_vec(&d).unwrap());
}

#[test]
fn the_selector_requires_an_existing_admitted_managed_grant() {
    let d = portable();
    assert!(WorkflowPlan::compile(&d, &scope(None), &[]).is_err());
    let mut unavailable = scope(Some(grant("a")));
    unavailable.rights.folders.clear();
    assert!(WorkflowPlan::compile(&d, &unavailable, &[]).is_err());
    let mut external = grant("external");
    external.order_id = None;
    assert!(WorkflowPlan::compile(&d, &scope(Some(external)), &[]).is_err());
    let mut wildcard = scope(Some(grant("a")));
    wildcard.rights.current_workspace = true;
    assert!(WorkflowPlan::compile(&d, &wildcard, &[]).is_err());
}

#[test]
fn enclosing_rights_still_bound_every_list_and_body() {
    let mut d = portable();
    d.iterations
        .get_mut("loop")
        .unwrap()
        .rights
        .current_workspace = false;
    let plan = WorkflowPlan::compile(&d, &scope(Some(grant("a"))), &[]).unwrap();
    let graph = derived(&plan);
    assert!(
        graph
            .nodes
            .values()
            .filter(|node| node.parent.is_some() || node.id == "loop")
            .all(|node| node.rights.folders.is_empty())
    );
}

#[test]
fn logical_workspace_selection_is_separate_from_the_enclosing_physical_authority() {
    let d = portable();
    let folder = grant("a");
    let plan = WorkflowPlan::compile(&d, &scope(Some(folder.clone())), &[]).unwrap();
    let mut changed = folder;
    changed.resource_id = "different-existing-native-resource".into();
    changed.revision = "b".repeat(64);
    let next = WorkflowPlan::compile(&d, &scope(Some(changed)), &[]).unwrap();
    assert_eq!(plan.source_sha256, next.source_sha256);
    assert_eq!(plan.sha256, next.sha256);
    assert_eq!(plan.scope.workspace_folder, next.scope.workspace_folder);
    let mut both = scope(Some(grant("a")));
    both.rights.folders.insert(grant("b").id);
    let admitted = WorkflowPlan::compile(&d, &both, &[]).unwrap();
    let mut forged = serde_json::to_value(&admitted).unwrap();
    forged["scope"]["workspace_folder"] =
        serde_json::to_value(WorkflowWorkspaceBinding::from_grant(&grant("b")).unwrap()).unwrap();
    let mut hashable = forged.clone();
    hashable["sha256"] = json!("");
    forged["sha256"] = json!(canonical_hash(&hashable));
    // A full recompilation may bind a new trusted scope. Rehashing only the
    // binding while keeping the derived body plans is never sufficient.
    assert!(
        serde_json::from_value::<WorkflowPlan>(forged)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn old_concrete_folder_ids_are_preserved_and_never_implicitly_ported() {
    let mut d = definition();
    d.rights.folders.insert("workspace:old-order".into());
    let mut available = scope(None);
    available.rights.folders.insert(grant("new-order").id);
    let plan = WorkflowPlan::compile(&d, &available, &[]).unwrap();
    assert!(plan.resource_definition.is_none());
    assert!(plan.scope.workspace_folder.is_none());
    assert_eq!(plan.definition.rights.folders, d.rights.folders);
    assert!(
        plan.nodes
            .values()
            .all(|node| node.rights.folders.is_empty())
    );
    let text = serde_json::to_string(&plan).unwrap();
    assert!(!text.contains("current_workspace"));
    assert!(!text.contains("workspace_folder"));
    assert!(!text.contains("resource_definition"));
    let old: WorkflowPlan = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-plan-before-composition.json"
    ))
    .unwrap();
    old.validate().unwrap();
    assert_eq!(
        old.sha256,
        "843bb38c743928c7b5d24804bd53251114620784b7955fd275495cf95ef0a800"
    );
}

#[test]
fn the_original_selector_cannot_be_removed_or_replaced_by_an_unmarked_definition() {
    let d = portable();
    let plan = WorkflowPlan::compile(&d, &scope(Some(grant("a"))), &[]).unwrap();
    for source in [Value::Null, serde_json::to_value(definition()).unwrap()] {
        let mut edited = serde_json::to_value(&plan).unwrap();
        edited["resource_definition"] = source;
        let mut hashable = edited.clone();
        hashable["sha256"] = json!("");
        edited["sha256"] = json!(canonical_hash(&hashable));
        assert!(
            serde_json::from_value::<WorkflowPlan>(edited)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[test]
fn nested_frozen_subflow_selectors_are_resolved_without_changing_the_library_tree() {
    let mut d: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-subflow.json"
    ))
    .unwrap();
    fn select(d: &mut WorkflowDefinition) {
        d.rights.current_workspace = true;
        for role in &mut d.roles {
            role.rights.current_workspace = true;
        }
        for step in &mut d.steps {
            step.rights.current_workspace = true;
        }
        for call in d.subflows.values_mut() {
            call.rights.current_workspace = true;
            select(&mut call.definition);
        }
    }
    select(&mut d);
    let original = canonical_hash(&d);
    let plan = WorkflowPlan::compile(&d, &scope(Some(grant("nested"))), &[]).unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.source_sha256, original);
    assert!(!plan.definition.uses_current_workspace());
    assert!(!plan.composition_scopes.is_empty());
    assert_eq!(canonical_hash(&d), original);
    assert_eq!(
        plan.resource_definition
            .as_ref()
            .map(|value| canonical_hash(value.as_ref())),
        Some(original)
    );
}
