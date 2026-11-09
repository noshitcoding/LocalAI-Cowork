//! Synthetic loopback protocol/store proof. This qualifies no real model.
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::{canonical_hash, terminal};
use cowork_model_accounting::{digest, registry::*, sha256, tokenizer::*};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::{Store, model_money::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn qualification(profile: &ProviderProfile) -> QualificationBundle {
    let vocab: BTreeMap<String, u32> = BTreeMap::from([("[UNK]".into(), 0)]);
    let artifact=json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":vocab,"unk_token":"[UNK]"}}).to_string();
    let prefixes = BTreeMap::from([
        ("system".into(), "system: ".into()),
        ("user".into(), "user: ".into()),
        ("assistant".into(), "assistant: ".into()),
        ("tool".into(), "tool: ".into()),
    ]);
    let suffixes = BTreeMap::from([
        ("system".into(), " ".into()),
        ("user".into(), " ".into()),
        ("assistant".into(), " ".into()),
        ("tool".into(), " ".into()),
    ]);
    let template = CompleteChatTemplate {
        format: 1,
        prefix: "synthetic fixture ".into(),
        role_prefix: prefixes,
        role_suffix: suffixes,
        tools_prefix: "tools: ".into(),
        tools_suffix: " ".into(),
        output_prefix: "output: ".into(),
        output_suffix: " ".into(),
        generation_prompt: "assistant: ".into(),
    };
    let chat_template_artifact = serde_json::to_string(&template).unwrap();
    let inputs = [
        (
            vec![
                json!({"role":"system","content":"fixture"}),
                json!({"role":"user","content":"Hello 你好"}),
            ],
            json!([]),
            Value::Null,
        ),
        (
            vec![
                json!({"role":"assistant","content":"create","tool_calls":[{"id":"artifact-call","type":"function","function":{"name":"create_artifact","arguments":"{\"name\":\"a.txt\",\"content\":\"fixture\"}"}}]}),
                json!({"role":"tool","tool_call_id":"artifact-call","name":"create_artifact","content":"confirmed"}),
            ],
            json!([{"type":"function","function":{"name":"create_artifact","parameters":{"type":"object"}}}]),
            Value::Null,
        ),
        (
            vec![json!({"role":"user","content":"Structured final"})],
            json!([]),
            json!({"name":"final","schema":{"type":"object","properties":{"result":{"type":"string"}},"required":["result"],"additionalProperties":false}}),
        ),
    ];
    // Independent explicit synthetic whitespace vectors, not an attestation
    // about any deployed model or a self-qualification product mechanism.
    let requests: Vec<_> = inputs
        .into_iter()
        .map(|(messages, tools, output)| {
            let mut text = String::from("synthetic fixture ");
            for m in &messages {
                text += &format!("{}: {} ", m["role"].as_str().unwrap(), m);
            }
            text += &format!("tools: {TOOL_PROTOCOL}{tools} output: {output} assistant: ");
            let token_ids = vec![0; text.split_whitespace().count()];
            ReferenceRequest {
                messages,
                tools,
                output,
                rendered_text: text,
                token_ids,
            }
        })
        .collect();
    let evidence = DeploymentEvidence {
        issuer: "explicit synthetic test fixture".into(),
        reference_implementation: "synthetic whitespace reference; no real model qualification"
            .into(),
        deployment_sha256: sha256(b"synthetic-deployment"),
        reviewed_at: 1,
        reference_requests: requests,
    };
    let identity = ModelIdentity {
        provider_id: profile.id.clone(),
        model_id: profile.model.clone(),
        model_sha256: sha256(b"synthetic-weights"),
        tokenizer_sha256: sha256(artifact.as_bytes()),
        vocabulary_sha256: digest(&vocab).unwrap(),
        chat_template_sha256: sha256(chat_template_artifact.as_bytes()),
        context_tokens: u64::from(profile.context_tokens),
    };
    let qualification = TokenizerQualification {
        model: identity,
        engine: ENGINE.into(),
        reference_implementation: evidence.reference_implementation.clone(),
        parity_evidence_sha256: digest(&evidence).unwrap(),
        reference_encodings: evidence
            .reference_requests
            .iter()
            .map(|s| ReferenceEncoding {
                rendered_text: s.rendered_text.clone(),
                token_ids: s.token_ids.clone(),
            })
            .collect(),
    };
    QualificationBundle {
        format: 1,
        id: "synthetic-operator-model".into(),
        version: 1,
        provider_sha256: canonical_hash(profile),
        wire_mode: RawWireMode::OpenaiCompletions,
        renderer: RENDERER.into(),
        tokenizer_artifact: artifact,
        chat_template_artifact,
        qualification,
        evidence,
    }
}

#[derive(Clone)]
struct Server {
    store: Store,
    descriptor: Value,
    deployment: String,
    run: Arc<Mutex<String>>,
    seen: Arc<Mutex<Vec<Value>>>,
}
async fn completion(
    State(s): State<Server>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ([(String, String); 2], String) {
    assert_eq!(
        headers["x-cowork-deployment-sha256"].to_str().unwrap(),
        s.deployment
    );
    let run = s.run.lock().unwrap().clone();
    let stored = s.store.stored_run(&run).await.unwrap();
    let registered = s
        .store
        .registered_model(&stored.record.spec)
        .await
        .unwrap()
        .unwrap();
    let prepared = cowork_runtime::provider::prepare_registered(
        &stored.record.spec,
        &stored.checkpoint.messages,
        false,
        Some(&registered),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(prepared.payload()).unwrap(),
        body
    );
    let attempt = stored.record.model_attempts.last().unwrap();
    let expected = prepared.exact_reserved_tokens().unwrap();
    assert_eq!(attempt.reserved_tokens, expected);
    assert_eq!(attempt.state, ModelAttemptState::InFlight);
    let count = body["prompt"].as_str().unwrap().split_whitespace().count() as u64;
    let ledger = s.store.monetary_ledger().await.unwrap();
    let reservation = ledger
        .reservation(&monetary_attempt_id(&run, &attempt.id))
        .unwrap();
    assert_eq!(reservation.spec.input_upper_bound, count);
    assert_eq!(reservation.receipt.reserved_units, expected);
    let response = if stored
        .checkpoint
        .messages
        .iter()
        .any(|m| m["role"] == "tool")
    {
        "Confirmed final answer".into()
    } else {
        json!({"content":"Create deliverable","tools":[{"id":"artifact-call","name":"create_artifact","arguments":{"name":"qualified.txt","content":"confirmed fixture artifact"}}]}).to_string()
    };
    s.seen.lock().unwrap().push(body);
    let stream = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"text":response,"finish_reason":"stop"}],"usage":{"prompt_tokens":count,"completion_tokens":5,"cost":"0.000123","currency":"EUR"}})
    );
    (
        [
            ("x-cowork-deployment-sha256".into(), s.deployment),
            ("content-type".into(), "text/event-stream".into()),
        ],
        stream,
    )
}

#[tokio::test]
async fn qualified_dispatch_money_archive_and_revoked_import_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("context.db");
    let store = Store::sqlite(&path).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "synthetic-raw".into(),
        name: "Synthetic protocol only".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        model: "synthetic-wordlevel".into(),
        revision: 1,
        context_tokens: 16384,
        credential_required: false,
        credential_ready: true,
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let bundle = qualification(&profile);
    let import = ImportModelQualification {
        idempotency_key: "operator-import".into(),
        qualification_bundle: json!(bundle),
    };
    let registry = store.import_model_qualification(&import).await.unwrap();
    let descriptor = json!({"format":1,"provider_sha256":bundle.provider_sha256,"deployment_sha256":bundle.evidence.deployment_sha256,"model_sha256":bundle.qualification.model.model_sha256,"tokenizer_sha256":bundle.qualification.model.tokenizer_sha256,"chat_template_sha256":bundle.qualification.model.chat_template_sha256});
    let server = Server {
        store: store.clone(),
        descriptor,
        deployment: bundle.evidence.deployment_sha256.clone(),
        run: Default::default(),
        seen: Default::default(),
    };
    let app = Router::new()
        .route(
            "/models",
            get(|| async { Json(json!({"data":[{"id":"synthetic-wordlevel"}]})) }),
        )
        .route(
            "/.well-known/cowork-model-qualification",
            get(|State(s): State<Server>| async move { Json(s.descriptor) }),
        )
        .route("/completions", post(completion))
        .with_state(server.clone());
    let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let currency = Currency {
        code: "EUR".into(),
        scale: 6,
    };
    store
        .configure_personal_money(&PersonalMoneyConfiguration {
            format_version: 1,
            installation: PoolPolicy {
                scope: PoolScope::Installation("fixture".into()),
                version: 1,
                currency: currency.clone(),
                limit_units: 1_000_000,
            },
            project_limit_units: 1_000_000,
            run_limit_units: 1_000_000,
            pool_policy_version: 1,
            deployments: vec![PersonalMoneyDeployment {
                deployment: OperatorModelDeployment {
                    id: "fixture-deployment".into(),
                    provider: profile.clone(),
                    model_sha256: bundle.qualification.model.model_sha256.clone(),
                    price_id: "fixture-price".into(),
                    price_version: 1,
                    token_mode: MonetaryTokenMode::ExactArtifactRenderer,
                },
                price: PriceVersion {
                    id: "fixture-price".into(),
                    version: 1,
                    provider_id: profile.id.clone(),
                    model_sha256: bundle.qualification.model.model_sha256.clone(),
                    currency,
                    input_units_per_million: 1_000_000,
                    output_units_per_million: 1_000_000,
                    cached_input_units_per_million: None,
                    source_sha256: sha256(b"synthetic-price"),
                },
            }],
        })
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Fixture".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "Fixture".into(),
        })
        .await
        .unwrap();
    let runtime = Runtime::new(
        store.clone(),
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        false,
    )
    .unwrap();
    let mut input = CreateRun {
        agent_config: None,
        order_id: order.id.clone(),
        base_revision: order.revision,
        idempotency_key: "run".into(),
        prompt: "Create the fixture artifact".into(),
        provider_id: profile.id.clone(),
        provider_revision: 1,
        access_mode: AccessMode::Autonomous,
        budgets: Budgets {
            max_steps: 6,
            max_seconds: 30,
            max_output_tokens: 128,
            max_total_tokens: 4096,
        },
        folders: vec![],
        mcp_tools: vec![],
        native_tools: vec![],
        environment: None,
        output: None,
        model_binding: Some(registry.binding.clone()),
    };
    let run = runtime.create_run(&input).await.unwrap();
    *server.run.lock().unwrap() = run.id.clone();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let finished = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let d = store.detail(&run.id).await.unwrap();
            if terminal(&d.run.state) {
                break d;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    stop.cancel();
    worker.await.unwrap();
    assert_eq!(finished.run.state, RunState::Completed);
    assert_eq!(finished.artifacts.len(), 1);
    assert_eq!(server.seen.lock().unwrap().len(), 2);
    let events = store.events(&run.id, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "qualified_model_context"
                && e.payload["independently_verified"] == true)
            .count(),
        2
    );
    assert!(events.iter().any(|e| e.kind == "model_completed"
        && e.payload["received_cost_metadata"]["body_usage"]["cost"] == "0.000123"));
    input.base_revision = store.order(&order.id).await.unwrap().revision;
    input.idempotency_key = "next".into();
    input.prompt = "Continue with a brief reference".into();
    let view = runtime
        .preview_model_context(&PreviewModelContext {
            request: input.clone(),
        })
        .await
        .unwrap();
    assert_eq!(view.count_mode, "exact_artifact_renderer");
    let removable = view
        .groups
        .iter()
        .filter(|g| !g.required)
        .map(|g| g.id.clone())
        .collect::<Vec<_>>();
    assert!(!removable.is_empty());
    let compact = CompactModelContext {
        request: input.clone(),
        idempotency_key: "compact-once".into(),
        snapshot_sha256: view.snapshot_sha256,
        remove_groups: removable,
        summary: Some("The fixture artifact was created.".into()),
    };
    let (a, b) = tokio::join!(
        runtime.compact_model_context(&compact),
        runtime.compact_model_context(&compact)
    );
    let receipt = a.unwrap();
    assert_eq!(b.unwrap().id, receipt.id);
    let archive = store.context_archive(&order.id, &receipt.id).await.unwrap();
    assert_eq!(canonical_hash(&archive.original), receipt.original_sha256);
    assert_eq!(canonical_hash(&archive.retained), receipt.retained_sha256);
    let retained = runtime
        .preview_model_context(&PreviewModelContext {
            request: input.clone(),
        })
        .await
        .unwrap();
    assert_eq!(retained.archive_id.as_deref(), Some(receipt.id.as_str()));
    assert!(retained.groups.iter().all(|g| g.required));
    assert_eq!(
        store
            .detail(&run.id)
            .await
            .unwrap()
            .run
            .model_attempts
            .len(),
        2
    );
    store
        .revoke_model_qualification(&registry.binding)
        .await
        .unwrap();
    assert!(runtime.create_run(&input).await.is_err());
    assert_eq!(
        canonical_hash(&store.import_model_qualification(&import).await.unwrap()),
        canonical_hash(&registry)
    );
    let mut new_import = import.clone();
    new_import.idempotency_key = "different-intent".into();
    assert!(store.import_model_qualification(&new_import).await.is_err());
    runtime.store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    assert_eq!(
        reopened.context_archives(&order.id, 0).await.unwrap().len(),
        1
    );
    assert_eq!(
        reopened
            .context_archive(&order.id, &receipt.id)
            .await
            .unwrap()
            .receipt
            .id,
        receipt.id
    );
    reopened.close().await;
    server_task.abort();
    assert!(server_task.await.unwrap_err().is_cancelled());
}
