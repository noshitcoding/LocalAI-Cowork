//! Small synthetic HTTP/SQLite proof. No real llama.cpp/model qualification.
use super::*;
use cowork_contracts::{ProviderKind, ProviderProfile};
use cowork_model_accounting::{registry::*, tokenizer::*};
use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

fn synthetic(profile: &ProviderProfile) -> QualificationBundle {
    let vocabulary = BTreeMap::from([("[UNK]".to_string(), 0u32)]);
    let artifact=json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,"model":{"type":"WordLevel","vocab":vocabulary,"unk_token":"[UNK]"}}).to_string();
    let template = CompleteChatTemplate {
        format: 1,
        prefix: "synthetic ".into(),
        role_prefix: ["system", "user", "assistant", "tool"]
            .into_iter()
            .map(|r| (r.into(), format!("{r}: ")))
            .collect(),
        role_suffix: ["system", "user", "assistant", "tool"]
            .into_iter()
            .map(|r| (r.into(), " ".into()))
            .collect(),
        tools_prefix: "tools: ".into(),
        tools_suffix: " ".into(),
        output_prefix: "output: ".into(),
        output_suffix: " ".into(),
        generation_prompt: "assistant: ".into(),
    };
    let chat_template_artifact = serde_json::to_string(&template).unwrap();
    let inputs = [
        (
            vec![json!({"role":"user","content":"fixture 你好"})],
            json!([]),
            Value::Null,
        ),
        (
            vec![json!({"role":"tool","tool_call_id":"a","content":"confirmed"})],
            json!([{"name":"fixture"}]),
            Value::Null,
        ),
        (
            vec![json!({"role":"user","content":"structured"})],
            json!([]),
            json!({"type":"object"}),
        ),
    ];
    let reference_requests = inputs
        .into_iter()
        .map(|(messages, tools, output)| {
            let mut text = "synthetic ".to_string();
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
        issuer: "synthetic protocol fixture only".into(),
        reference_implementation:
            "independent explicit synthetic whitespace vectors; no model attestation".into(),
        deployment_sha256: sha256(b"synthetic-deployment"),
        reviewed_at: 1,
        reference_requests,
    };
    let qualification = TokenizerQualification {
        model: ModelIdentity {
            provider_id: profile.id.clone(),
            model_id: profile.model.clone(),
            model_sha256: sha256(b"synthetic-weights"),
            tokenizer_sha256: sha256(artifact.as_bytes()),
            vocabulary_sha256: digest(&vocabulary).unwrap(),
            chat_template_sha256: sha256(chat_template_artifact.as_bytes()),
            context_tokens: u64::from(profile.context_tokens),
        },
        engine: ENGINE.into(),
        reference_implementation: evidence.reference_implementation.clone(),
        parity_evidence_sha256: digest(&evidence).unwrap(),
        reference_encodings: evidence
            .reference_requests
            .iter()
            .map(|r| ReferenceEncoding {
                rendered_text: r.rendered_text.clone(),
                token_ids: r.token_ids.clone(),
            })
            .collect(),
    };
    QualificationBundle {
        format: 1,
        id: "synthetic-fixture".into(),
        version: 1,
        provider_sha256: config::canonical_digest(profile).unwrap(),
        wire_mode: RawWireMode::OpenaiCompletions,
        renderer: RENDERER.into(),
        tokenizer_artifact: artifact,
        chat_template_artifact,
        qualification,
        evidence,
    }
}

#[derive(Clone)]
struct Backend {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    count: Arc<AtomicUsize>,
    reject: bool,
}
async fn tokenize(Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["add_special"], false);
    assert_eq!(body["parse_special"], true);
    Json(json!({"tokens":vec![0u32;body["content"].as_str().unwrap().split_whitespace().count()]}))
}
async fn upstream(State(state): State<Backend>, Json(body): Json<Value>) -> (StatusCode, String) {
    state.count.fetch_add(1, Ordering::SeqCst);
    assert_eq!(body["cache_prompt"], false);
    assert!(body["prompt"].as_array().unwrap().iter().all(Value::is_u64));
    state.entered.notify_one();
    state.release.notified().await;
    if state.reject {
        return (
            StatusCode::BAD_REQUEST,
            "original failed receive bytes".into(),
        );
    }
    let input = body["prompt"].as_array().unwrap().len();
    (
        StatusCode::OK,
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"index":0,"text":"original answer","finish_reason":"stop"}],"usage":{"prompt_tokens":input,"completion_tokens":2,"total_tokens":input+2}})
        ),
    )
}
async fn gateway(
    temp: &std::path::Path,
    backend: Backend,
) -> (Gateway, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/tokenize", post(tokenize))
        .route("/v1/completions", post(upstream))
        .with_state(backend);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let profile = ProviderProfile {
        generation: Default::default(),
        id: "synthetic-provider".into(),
        name: "fixture".into(),
        kind: ProviderKind::OpenaiCompatible,
        endpoint: "http://127.0.0.1:39999".into(),
        model: "synthetic".into(),
        revision: 1,
        context_tokens: 1024,
        credential_required: true,
        credential_ready: true,
        capabilities: Default::default(),
    };
    let bundle = synthetic(&profile);
    let model = RegisteredModel::load(bundle.clone()).unwrap();
    let backend_directory = temp.join("backend");
    std::fs::create_dir(&backend_directory).unwrap();
    let loaded = config::Loaded {
        config: config::Config {
            format: 1,
            listen: "127.0.0.1:39999".parse().unwrap(),
            profile,
            bundle,
            deployment: config::Deployment {
                format: 1,
                backend: "llama_cpp_token_ids_v1".into(),
                supervisor: "cowork-executor-owned-v1".into(),
                server: "fixture".into(),
                gateway_sha256: sha256(b"fixture"),
                runtime_files: BTreeMap::new(),
                model_sha256: sha256(b"synthetic-weights"),
                model_id: "synthetic".into(),
                context_tokens: 1024,
                tokenizer_sha256: model.identity().tokenizer_sha256.clone(),
                chat_template_sha256: model.identity().chat_template_sha256.clone(),
            },
            backend_directory,
            model_file: temp.join("unused-fixture-model"),
            data_directory: temp.into(),
            bearer_file: temp.join("unused-fixture-token"),
        },
        model,
        token: "a".repeat(64),
        held_files: vec![],
        _held_directories: vec![],
    };
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(temp.join("fixture.sqlite"))
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE guard(id INTEGER PRIMARY KEY,revision INTEGER);INSERT INTO guard VALUES(1,0);CREATE TABLE requests(id TEXT PRIMARY KEY,request_sha256 TEXT,state TEXT,input_ids TEXT,receipt TEXT,response BLOB,raw_response BLOB NOT NULL DEFAULT X'')").execute(&pool).await.unwrap();
    (
        Gateway {
            loaded: Arc::new(loaded),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            upstream: format!("http://{address}"),
            backend_key: "synthetic".into(),
            backend_owner: Arc::new(owner::BackendOwner::capture(std::process::id()).unwrap()),
            backend_address: address,
            pool,
            alive: Arc::new(AtomicBool::new(true)),
            serial: Arc::new(tokio::sync::Semaphore::new(1)),
        },
        task,
    )
}
fn request(g: &Gateway) -> (HeaderMap, Bytes) {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", g.loaded.token).parse().unwrap(),
    );
    headers.insert("x-cowork-model-attempt", "b".repeat(64).parse().unwrap());
    headers.insert(
        "x-cowork-deployment-sha256",
        g.loaded
            .config
            .bundle
            .evidence
            .deployment_sha256
            .parse()
            .unwrap(),
    );
    (headers,Bytes::from(json!({"model":"synthetic","prompt":"explicit synthetic prompt","stream":true,"stream_options":{"include_usage":true},"max_tokens":32}).to_string()))
}
#[tokio::test]
async fn original_send_survives_caller_loss_and_retains_failed_bytes() {
    for reject in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let backend = Backend {
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            count: Arc::new(AtomicUsize::new(0)),
            reject,
        };
        let (g, server) = gateway(temp.path(), backend.clone()).await;
        let (headers, body) = request(&g);
        let caller = tokio::spawn(completion(State(g.clone()), headers.clone(), body.clone()));
        tokio::time::timeout(Duration::from_secs(5), backend.entered.notified())
            .await
            .unwrap();
        caller.abort();
        let _ = caller.await;
        backend.release.notify_one();
        let _settled =
            tokio::time::timeout(Duration::from_secs(5), g.serial.clone().acquire_owned())
                .await
                .unwrap()
                .unwrap();
        drop(_settled);
        let row = sqlx::query("SELECT state,raw_response FROM requests")
            .fetch_one(&g.pool)
            .await
            .unwrap();
        let raw = row.get::<Vec<u8>, _>("raw_response");
        if reject {
            assert_eq!(row.get::<String, _>("state"), "unknown");
            assert_eq!(raw, b"original failed receive bytes");
            assert!(matches!(
                completion(State(g.clone()), headers.clone(), body.clone()).await,
                Err((StatusCode::CONFLICT, "retained_unconfirmed_request"))
            ));
        } else {
            assert_eq!(row.get::<String, _>("state"), "completed");
            let response = completion(State(g.clone()), headers.clone(), body.clone())
                .await
                .unwrap();
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 4_000_000)
                    .await
                    .unwrap()
                    .as_ref(),
                raw
            );
        }
        let changed=Bytes::from(json!({"model":"synthetic","prompt":"changed","stream":true,"stream_options":{"include_usage":true},"max_tokens":32}).to_string());
        assert!(matches!(
            completion(State(g.clone()), headers, changed).await,
            Err((StatusCode::CONFLICT, "attempt_changed"))
        ));
        assert_eq!(backend.count.load(Ordering::SeqCst), 1);
        g.pool.close().await;
        server.abort();
        let _ = server.await;
    }
}
#[test]
fn exact_terminal_stream_refuses_incomplete_and_duplicate_usage() {
    let terminal = json!({"choices":[{"index":0,"text":"yes","finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}});
    let complete = format!("data: {terminal}\n\ndata: [DONE]\n\n");
    assert!(completed_stream(complete.as_bytes(), 3, 32));
    assert!(!completed_stream(
        format!("data: {terminal}\n\n").as_bytes(),
        3,
        32
    ));
    assert!(!completed_stream(
        format!("data: {terminal}\n\ndata: {terminal}\n\ndata: [DONE]\n\n").as_bytes(),
        3,
        32
    ));
    assert!(!completed_stream(complete.as_bytes(), 4, 32));
}
