use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_domain::Error;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Keys(KeyId);
impl KeyStore for Keys {
    fn load(&self, id: &KeyId) -> cowork_secrets::Result<Zeroizing<[u8; 32]>> {
        if id != &self.0 {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(Zeroizing::new([17; 32]))
    }
}

#[tokio::test]
async fn operator_profiles_still_require_the_exact_current_profile() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let profile = ProviderProfile {
        id: "operator-fixture".into(),
        name: "Operator configured".into(),
        kind: ProviderKind::Test,
        endpoint: "".into(),
        model: "fixture".into(),
        revision: 2,
        context_tokens: 32768,
        credential_required: false,
        credential_ready: true,
        generation: Default::default(),
        capabilities: Default::default(),
    };
    store.insert_provider(&profile).await.unwrap();
    let runtime = Runtime::new(
        store,
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        true,
    )
    .unwrap();
    assert!(
        runtime
            .personal_provider_credential(&profile)
            .await
            .unwrap()
            .is_none()
    );
    let mut stale = profile.clone();
    stale.revision = 1;
    assert!(matches!(
        runtime.personal_provider_credential(&stale).await,
        Err(Error::Conflict)
    ));
    let mut changed = profile.clone();
    changed.name = "Different full profile".into();
    assert!(matches!(
        runtime.personal_provider_credential(&changed).await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn already_accepted_run_executes_only_original_endpoint_model_and_key_after_edit() {
    type Captured = Arc<Mutex<Vec<(String, Value)>>>;
    async fn chat(
        State(captured): State<Captured>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> String {
        captured.lock().unwrap().push((
            headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            body,
        ));
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"content":"Original model completed"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4}})
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let captured: Captured = Default::default();
    let app = Router::new()
        .route(
            "/models",
            get(|| async { Json(json!({"data":[{"id":"initial-model"}]})) }),
        )
        .route("/chat/completions", post(chat))
        .with_state(captured.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let keys = Arc::new(Keys(KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    }));
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    let initial = runtime
        .configure_personal_provider(&ConfigurePersonalProvider {
            idempotency_key: "original-connection".into(),
            profile: CreateProvider {
                name: "Original".into(),
                kind: ProviderKind::OpenaiCompatible,
                endpoint,
                model: "initial-model".into(),
                context_tokens: 32768,
                capabilities: Default::default(),
            },
            api_key: Some("old-synthetic-key".into()),
            explicitly_allow_no_key: false,
        })
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Frozen credentials".into(),
        })
        .await
        .unwrap();
    let order = store
        .create_order(CreateOrder {
            project_id: project.id,
            title: "New Chat".into(),
        })
        .await
        .unwrap();
    let run = runtime
        .create_run(&CreateRun {
            order_id: order.id,
            base_revision: order.revision,
            idempotency_key: "accepted-before-edit".into(),
            prompt: "Complete using the original connection".into(),
            provider_id: initial.id.clone(),
            provider_revision: 1,
            access_mode: AccessMode::Autonomous,
            budgets: Budgets::default(),
            folders: vec![],
            native_tools: vec![],
            mcp_tools: vec![],
            environment: None,
            output: None,
            model_binding: None,
            agent_config: None,
        })
        .await
        .unwrap();
    runtime
        .update_provider_settings(
            &initial.id,
            &UpdateProviderSettings {
                idempotency_key: "later-settings".into(),
                expected_revision: 1,
                profile: CreateProvider {
                    name: "Changed later".into(),
                    kind: ProviderKind::OpenaiCompatible,
                    endpoint: "https://must-never-be-called.invalid/v1".into(),
                    model: "later-model".into(),
                    context_tokens: 65536,
                    capabilities: Default::default(),
                },
                generation: GenerationSettings {
                    temperature: Some(2.0),
                    timeout_ms: Some(1000),
                },
                api_key: Some("new-synthetic-key".into()),
                remove_key: false,
            },
        )
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let result = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let detail = store.detail(&run.id).await.unwrap();
            if cowork_domain::terminal(&detail.run.state) {
                break detail;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    stop.cancel();
    worker.await.unwrap();
    server.abort();
    let _ = server.await;
    let detail = result.unwrap();
    assert_eq!(detail.run.state, RunState::Completed);
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "Bearer old-synthetic-key");
    assert_eq!(requests[0].1["model"], "initial-model");
    assert!(requests[0].1.get("temperature").is_none());
    assert_eq!(detail.run.spec.provider.as_ref().unwrap().revision, 1);
}

#[tokio::test]
async fn accepted_profiles_keep_exact_credentials_through_edits_restart_and_lost_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credential-history.sqlite");
    let store = Store::sqlite(&path).await.unwrap();
    let keys = Arc::new(Keys(KeyId {
        instance: Uuid::new_v4(),
        key: Uuid::new_v4(),
    }));
    let runtime = Runtime::new(store.clone(), vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    let create = ConfigurePersonalProvider {
        idempotency_key: "create-owner-profile".into(),
        profile: CreateProvider {
            name: "Initial".into(),
            kind: ProviderKind::OpenaiCompatible,
            endpoint: "https://first.invalid/v1".into(),
            model: "first-model".into(),
            context_tokens: 32768,
            capabilities: Default::default(),
        },
        api_key: Some("first-synthetic-secret".into()),
        explicitly_allow_no_key: false,
    };
    let initial = runtime.configure_personal_provider(&create).await.unwrap();
    let edited = runtime
        .update_provider_settings(
            &initial.id,
            &UpdateProviderSettings {
                idempotency_key: "edit-owner-profile".into(),
                expected_revision: 1,
                profile: CreateProvider {
                    name: "Edited".into(),
                    kind: ProviderKind::OpenaiCompatible,
                    endpoint: "https://second.invalid/v1".into(),
                    model: "second-model".into(),
                    context_tokens: 65536,
                    capabilities: Default::default(),
                },
                generation: GenerationSettings {
                    temperature: Some(0.0),
                    timeout_ms: Some(5000),
                },
                api_key: Some("second-synthetic-secret".into()),
                remove_key: false,
            },
        )
        .await
        .unwrap()
        .profile;
    assert_eq!(
        runtime
            .personal_provider_credential(&initial)
            .await
            .unwrap()
            .as_deref(),
        Some("first-synthetic-secret")
    );
    assert_eq!(
        runtime
            .personal_provider_credential(&edited)
            .await
            .unwrap()
            .as_deref(),
        Some("second-synthetic-secret")
    );
    let mut substituted = initial.clone();
    substituted.endpoint = edited.endpoint.clone();
    assert!(matches!(
        runtime.personal_provider_credential(&substituted).await,
        Err(Error::Conflict)
    ));
    let mut unknown_revision = initial.clone();
    unknown_revision.revision = 55;
    assert!(matches!(
        runtime
            .personal_provider_credential(&unknown_revision)
            .await,
        Err(Error::Conflict)
    ));
    // A lost creation ACK returns the original metadata without rolling back
    // the current connection or dispatching any provider HTTP request.
    assert_eq!(
        serde_json::to_value(runtime.configure_personal_provider(&create).await.unwrap()).unwrap(),
        serde_json::to_value(&initial).unwrap()
    );
    assert_eq!(runtime.profiles().await[0].revision, 2);
    drop(runtime);
    store.close().await;
    let reopened = Store::sqlite(&path).await.unwrap();
    let restarted = Runtime::new(reopened.clone(), vec![], false)
        .unwrap()
        .with_personal_provider_keys(keys.clone(), keys.0);
    assert_eq!(
        restarted
            .personal_provider_credential(&initial)
            .await
            .unwrap()
            .as_deref(),
        Some("first-synthetic-secret")
    );
    let bound = reopened
        .personal_provider_credential(&initial)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !serde_json::to_string(&bound)
            .unwrap()
            .contains("first-synthetic-secret")
    );
    assert_eq!(
        restarted
            .configure_personal_provider(&create)
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(restarted.profiles().await[0].revision, 2);
    restarted
        .delete_provider_settings(
            &initial.id,
            &DeleteProviderSettings {
                idempotency_key: "retire-profile".into(),
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        restarted.personal_provider_credential(&initial).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        restarted.personal_provider_credential(&edited).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(
        serde_json::to_value(
            restarted
                .configure_personal_provider(&create)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&initial).unwrap()
    );
    assert!(restarted.profiles().await.is_empty());
    assert!(!reopened.server_key_has_existing_ciphertext().await.unwrap());
}
