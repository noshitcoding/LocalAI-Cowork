use cowork_contracts::ProviderProfile;
use cowork_domain::{canonical_hash, workflows::*};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

fn model_definition() -> WorkflowDefinition {
    let mut d: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-iteration.json"
    ))
    .unwrap();
    let body = &mut d.iterations.get_mut("loop").unwrap().definition;
    body.controls.clear();
    body.roles.push(serde_json::from_value(json!({"id":"worker","instructions":"Use only the explicit amount.","provider_id":"model","provider_revision":1,"rights":{"tools":[],"folders":[]},"budgets":body.budgets,"delegate_to":[]})).unwrap());
    body.steps.push(WorkflowStep {
        id: "echo".into(),
        role_id: "worker".into(),
        goal: "Return one amount".into(),
        dependencies: vec![],
        rights: WorkflowRights::default(),
        budgets: body.budgets.clone(),
        execution: None,
        zero_context: false,
    });
    d
}

#[test]
fn sixteen_model_entries_complete_and_reopen_with_one_ordered_original_ledger() {
    let d = model_definition();
    let profile: ProviderProfile=serde_json::from_value(json!({"id":"model","name":"Domain scale fixture","kind":"ollama","endpoint":"http://127.0.0.1:11434","model":"frozen-model","revision":1,"context_tokens":2048,"credential_required":false,"credential_ready":true})).unwrap();
    let plan = WorkflowPlan::compile(
        &d,
        &WorkflowScope {
            workspace_folder: None,
            rights: d.rights.clone(),
            budgets: d.budgets.clone(),
            max_parallel: 4,
        },
        &[profile],
    )
    .unwrap();
    let immutable = serde_json::to_vec(&plan).unwrap();
    let input = json!({"values":(0..16).collect::<Vec<_>>()});
    let started = Instant::now();
    macro_rules! measured {
        ($label:expr,$expression:expr) => {{
            let sample = Instant::now();
            let result = $expression;
            eprintln!("stage={} ms={}", $label, sample.elapsed().as_millis());
            result
        }};
    }
    let mut p = WorkflowProgress::new(&plan).unwrap();
    let mut models = 0;
    for _ in 0..128 {
        let ready = measured!("ready", p.ready_nodes(&plan).unwrap());
        let Some(node) = ready.last() else { break };
        let node = node.clone();
        let graph = measured!("graph", p.execution_graph(&plan).unwrap());
        let native = p.requires_native_in(&graph, &node).unwrap();
        let frame = if native {
            Value::Null
        } else {
            let completed: BTreeMap<_, _> = p
                .nodes()
                .iter()
                .filter_map(|(id, node)| {
                    if let WorkflowNodeState::Completed { output, .. } = &node.state {
                        Some((id.clone(), output.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            graph
                .definition
                .resolve_step_data(&node, Some(&input), &completed)
                .unwrap()
        };
        let claim = measured!("claim", p.claim(&plan, p.revision(), &node, 100).unwrap());
        let attempt = format!("action-{}", p.revision());
        if native {
            p.complete_control(&plan, p.revision(), &claim, &attempt, Some(&input), 100)
                .unwrap();
        } else {
            p.admit(
                &plan,
                p.revision(),
                &claim,
                &attempt,
                WorkflowOperation::Model {
                    request_sha256: canonical_hash(&frame),
                    reserved_tokens: None,
                },
                100,
            )
            .unwrap();
            p.settle(
                &plan,
                p.revision(),
                &claim,
                &attempt,
                WorkflowOutcome::Measured {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                100,
            )
            .unwrap();
            p.complete(
                &plan,
                p.revision(),
                &claim,
                &json!({"amount":frame["amount"]}).to_string(),
                100,
            )
            .unwrap();
            models += 1;
            eprintln!(
                "models={} total_ms={}",
                models,
                started.elapsed().as_millis()
            );
        }
    }
    assert_eq!(models, 16);
    assert_eq!(p.state(), WorkflowState::Completed);
    assert_eq!(p.charged_tokens(), 32);
    assert_eq!(p.attempts().len(), 34);
    assert_eq!(p.iteration_manifests()["loop"].items.len(), 16);
    let WorkflowNodeState::Completed { output, .. } = &p.nodes()["after"].state else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({"results":(0..16).collect::<Vec<_>>()})
    );
    let reopened: WorkflowProgress =
        serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
    reopened.validate(&plan).unwrap();
    assert_eq!(canonical_hash(&p), canonical_hash(&reopened));
    assert_eq!(immutable, serde_json::to_vec(&plan).unwrap());
    eprintln!(
        "sixteen_models_complete_reopen_ms={}",
        started.elapsed().as_millis()
    );
}
