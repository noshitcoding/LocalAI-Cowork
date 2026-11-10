use axum::{Router, response::IntoResponse, routing::post};
use cowork_contracts::*;
use cowork_runtime::provider;
use serde_json::json;
use tokio::sync::mpsc;

struct CapturedRequest {
    path: String,
    headers: axum::http::HeaderMap,
    payload: Vec<u8>,
}

struct CaptureServer {
    url: String,
    requests: mpsc::Receiver<CapturedRequest>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl CaptureServer {
    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for CaptureServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn capture_server(
    path: &str,
    status: axum::http::StatusCode,
    body: &'static str,
    redirect: Option<String>,
) -> CaptureServer {
    let (capture, requests) = mpsc::channel(4);
    let app = Router::new().route(
        path,
        post(
            move |headers: axum::http::HeaderMap,
                  axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
                  bytes: axum::body::Bytes| {
                let capture = capture.clone();
                let redirect = redirect.clone();
                async move {
                    capture
                        .send(CapturedRequest {
                            path: uri.path().into(),
                            headers,
                            payload: bytes.to_vec(),
                        })
                        .await
                        .unwrap();
                    let mut response =
                        (status, [("content-type", "text/event-stream")], body).into_response();
                    if let Some(location) = redirect {
                        response
                            .headers_mut()
                            .insert(axum::http::header::LOCATION, location.parse().unwrap());
                    }
                    response
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    CaptureServer {
        url,
        requests,
        shutdown: Some(shutdown),
        task: Some(task),
    }
}
#[test]
fn context_guard_includes_tool_definitions_and_fingerprints_frozen_limits() {
    let mut request = spec("http://127.0.0.1:1".into(), ProviderKind::OpenaiCompatible);
    request.provider.as_mut().unwrap().context_tokens = 1024;
    request.budgets.max_output_tokens = 1;
    let messages = vec![json!({"role":"user","content":"x"})];
    let before = provider::request_fingerprint(&request, &messages).unwrap();
    request.budgets.max_output_tokens = 2;
    assert_ne!(
        provider::request_fingerprint(&request, &messages).unwrap(),
        before
    );
    request.allowed_tools = vec!["write_text_file".into(), "restore_file".into()];
    assert!(matches!(
        provider::request_fingerprint(&request, &messages),
        Err(cowork_domain::Error::ContextLimit)
    ));
}

#[test]
fn duplicate_tool_ids_cannot_reuse_a_previous_approval() {
    let call = ToolCall {
        id: "one-action".into(),
        name: "create_artifact".into(),
        arguments: json!({"content":"x"}),
    };
    assert!(provider::validate_tool_ids(&[], &[call.clone(), call.clone()]).is_err());
    let history = vec![
        json!({"role":"assistant","tool_calls":[{"id":"one-action","function":{"name":"create_artifact"}}]}),
    ];
    assert!(provider::validate_tool_ids(&history, &[call]).is_err());
}

#[tokio::test]
async fn codex_requires_an_exact_opaque_account_transport_after_responses_preparation() {
    let endpoint = "codex://11111111-1111-4111-8111-111111111111/3?reasoning_effort=high";
    let mut request = spec(endpoint.into(), ProviderKind::Codex);
    request.agent_config = Some(EffectiveAgentConfig {
        generation: GenerationSettings {
            temperature: Some(0.0),
            timeout_ms: Some(3000),
        },
        ..Default::default()
    });
    let messages = vec![
        json!({"role":"system","content":"Frozen user rules"}),
        json!({"role":"user","content":"One bounded request"}),
    ];
    let prepared = provider::prepare(&request, &messages, false).unwrap();
    let body: serde_json::Value = serde_json::from_slice(prepared.payload()).unwrap();
    assert_eq!(body["instructions"], "Frozen user rules\n");
    assert!(body.get("messages").is_none());
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_output_tokens"], request.budgets.max_output_tokens);
    let (tx, _rx) = mpsc::channel(4);
    assert!(
        matches!(provider::complete_prepared(prepared,None,tx).await,Err(cowork_domain::Error::Provider(code)) if code=="codex_transport_required")
    );
    for invalid in [
        "codex://11111111-1111-4111-8111-111111111111/0",
        "codex://11111111-1111-4111-8111-111111111111/03",
        "codex://11111111-1111-4111-8111-111111111111/3?token=secret",
        "https://chatgpt.com/backend-api/codex/responses",
    ] {
        request.provider.as_mut().unwrap().endpoint = invalid.into();
        assert!(provider::prepare(&request, &messages, false).is_err());
    }
}

fn spec(endpoint: String, kind: ProviderKind) -> RunSpec {
    RunSpec {
        input_workspace: None,
        agent_config: None,
        model_binding: None,
        output: None,
        workflow: None,
        mcp_tools: vec![],
        environment: None,
        folders: vec![],
        order_id: "o".into(),
        project_id: "p".into(),
        prompt: "p".into(),
        provider: Some(ProviderProfile {
            generation: Default::default(),
            id: "p".into(),
            name: "fixture".into(),
            kind,
            endpoint,
            model: "m".into(),
            revision: 1,
            context_tokens: 32768,
            credential_required: false,
            credential_ready: true,
            capabilities: Default::default(),
        }),
        file_action: None,
        access_mode: AccessMode::ConfirmMutations,
        executor: "local-v2".into(),
        policy_revision: 1,
        allowed_tools: vec![],
        budgets: Budgets::default(),
    }
}
async fn server(body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().route(
        "/chat/completions",
        post(
            move || async move { ([("content-type", "text/event-stream")], body).into_response() },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), task)
}
#[tokio::test]
async fn streaming_reassembles_arguments_and_usage_without_guessing_costs() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hi \",\"tool_calls\":[{\"index\":0,\"id\":\"call-a\",\"function\":{\"name\":\"create_artifact\",\"arguments\":\"{\\\"name\\\":\\\"a\\\",\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"there\",\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"content\\\":\\\"value\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4}}\n\n",
        "data: [DONE]\n\n"
    );
    let (url, task) = server(body).await;
    let (tx, mut rx) = mpsc::channel(32);
    let reply = provider::complete(
        &spec(url, ProviderKind::OpenaiCompatible),
        &[json!({"role":"user","content":"hi"})],
        None,
        tx,
        false,
    )
    .await
    .unwrap();
    assert_eq!(reply.content, "Hi there");
    assert_eq!(
        reply.tools[0].arguments,
        json!({"name":"a","content":"value"})
    );
    assert_eq!(reply.usage.input_tokens, Some(12));
    assert!(reply.usage.cost.is_none());
    assert_eq!(rx.recv().await.unwrap(), "Hi ");
    assert_eq!(rx.recv().await.unwrap(), "there");
    assert_eq!(rx.recv().await, None);
    task.abort();
}
#[tokio::test]
async fn a_truncated_stream_cannot_authorize_a_tool() {
    let (url, task) =
        server("data: {\"choices\":[{\"delta\":{\"content\":\"unfinished\"}}]}\n\n").await;
    let (tx, _rx) = mpsc::channel(32);
    assert!(
        provider::complete(
            &spec(url, ProviderKind::OpenaiCompatible),
            &[],
            None,
            tx,
            false
        )
        .await
        .is_err()
    );
    task.abort();
}
#[tokio::test]
async fn ollama_ndjson_and_structured_tool_arguments() {
    let app=Router::new().route("/api/chat",post(||async{(
        [("content-type","application/x-ndjson")],
        concat!("{\"message\":{\"content\":\"\",\"thinking\":\"Private provider reasoning\"},\"done\":false}\n",
            "{\"message\":{\"content\":\"Ready\",\"tool_calls\":[{\"function\":{\"name\":\"request_input\",\"arguments\":{\"question\":\"Which document?\"}}}]},\"done\":false}\n",
            "{\"message\":{\"content\":\"\"},\"done\":true,\"prompt_eval_count\":7,\"eval_count\":3}\n"))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (tx, mut rx) = mpsc::channel(32);
    let reply = provider::complete(
        &spec(format!("http://{addr}"), ProviderKind::Ollama),
        &[],
        None,
        tx,
        false,
    )
    .await
    .unwrap();
    assert_eq!(reply.tools[0].name, "request_input");
    assert_eq!(reply.usage.output_tokens, Some(3));
    assert_eq!(reply.content, "Ready");
    assert_eq!(rx.recv().await.as_deref(), Some("Ready"));
    assert_eq!(rx.recv().await, None);
    task.abort();
}

#[tokio::test]
async fn openai_chat_sends_one_initial_system_message_without_rewriting_history() {
    let response = "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
    let mut fixture = capture_server(
        "/chat/completions",
        axum::http::StatusCode::OK,
        response,
        None,
    )
    .await;
    let request = spec(fixture.url.clone(), ProviderKind::OpenaiCompatible);
    let messages = vec![
        json!({"role":"system","content":"Base rules: ü 🙂"}),
        json!({"role":"system","content":"Project rules"}),
        json!({"role":"user","content":"Start"}),
        json!({"role":"assistant","content":"Plan"}),
        json!({"role":"system","cowork_phase":"private-marker","content":"Execute the reviewed plan"}),
        json!({"role":"assistant","tool_calls":[{"id":"call-1","type":"function","function":{"name":"list_artifacts","arguments":"{}"}}]}),
        json!({"role":"tool","tool_call_id":"call-1","content":"[]"}),
    ];
    let original = messages.clone();
    let prepared = provider::prepare(&request, &messages, false).unwrap();
    prepared.assert_binding(&request, &original).unwrap();
    let (tx, _rx) = mpsc::channel(4);
    assert_eq!(
        provider::complete_prepared(prepared, None, tx)
            .await
            .unwrap()
            .content,
        "OK"
    );
    let wire: serde_json::Value =
        serde_json::from_slice(&fixture.requests.recv().await.unwrap().payload).unwrap();
    let sent = wire["messages"].as_array().unwrap();
    assert_eq!(
        sent[0],
        json!({"role":"system","content":"Base rules: ü 🙂\n\nProject rules\n\nExecute the reviewed plan"})
    );
    assert_eq!(sent.iter().filter(|m| m["role"] == "system").count(), 1);
    assert_eq!(
        &sent[1..],
        &[
            original[2].clone(),
            original[3].clone(),
            original[5].clone(),
            original[6].clone()
        ]
    );
    assert_eq!(messages, original);
    fixture.stop().await;
}

#[test]
fn openai_hybrid_planning_instructions_join_the_initial_system_message() {
    let mut request = spec("http://127.0.0.1:1".into(), ProviderKind::OpenaiCompatible);
    request.agent_config = Some(EffectiveAgentConfig {
        execution_mode: Some(AgentExecutionMode::Hybrid),
        ..Default::default()
    });
    request.allowed_tools = vec!["request_input".into()];
    let messages = vec![
        json!({"role":"system","content":"Base rules"}),
        json!({"role":"user","content":"Plan this task"}),
    ];
    let prepared = provider::prepare(&request, &messages, false).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(prepared.payload()).unwrap();
    assert_eq!(wire["messages"].as_array().unwrap().len(), 2);
    assert!(
        wire["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("Base rules\n\nPlanning phase.")
    );
    assert_eq!(wire["messages"][1], messages[1]);
    assert_eq!(wire["tools"], json!([]));
}

#[tokio::test]
async fn rejected_chat_parameters_have_a_specific_safe_error_code() {
    let mut fixture = capture_server(
        "/chat/completions",
        axum::http::StatusCode::BAD_REQUEST,
        "{\"error\":{\"message\":\"System message must be at the beginning. private-secret\"}}",
        None,
    )
    .await;
    let request = spec(fixture.url.clone(), ProviderKind::OpenaiCompatible);
    let (tx, _rx) = mpsc::channel(4);
    let error = provider::complete(&request, &[], Some("private-secret".into()), tx, false)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "provider_request_rejected");
    assert!(!error.to_string().contains("private-secret"));
    fixture.requests.recv().await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn prepared_openai_and_ollama_send_exact_frozen_unicode_and_tool_bytes() {
    for kind in [ProviderKind::OpenaiCompatible, ProviderKind::Ollama] {
        let ollama = matches!(kind, ProviderKind::Ollama);
        let path = if ollama {
            "/api/chat"
        } else {
            "/chat/completions"
        };
        let response = if ollama {
            "{\"message\":{\"content\":\"accepted\"},\"done\":true}\n"
        } else {
            "data: {\"choices\":[{\"delta\":{\"content\":\"accepted\"}}]}\n\ndata: [DONE]\n\n"
        };
        let mut fixture = capture_server(path, axum::http::StatusCode::OK, response, None).await;
        let mut request = spec(format!("{}/", fixture.url), kind);
        request.allowed_tools = vec!["request_input".into()];
        request.provider.as_mut().unwrap().credential_required = true;
        let arguments = json!({"question":"Grüße 🙂\n\"quoted\" \\slash\u{0}","nested":{"value":[1,true,null]}});
        let mut messages = vec![
            json!({"role":"user","content":"漢字 é🙂\n\"quoted\" \\slash\u{0}"}),
            json!({"role":"assistant","tool_calls":[{"id":"frozen-call","type":"function","function":{"name":"request_input","arguments":arguments.to_string()}}]}),
            json!({"role":"tool","tool_call_id":"frozen-call","content":"result with ü and \t tab"}),
        ];
        let original_messages = messages.clone();
        let prepared = provider::prepare(&request, &messages, false).unwrap();
        prepared.assert_binding(&request, &messages).unwrap();
        assert_eq!(
            prepared.logical_fingerprint(),
            provider::request_fingerprint(&request, &messages).unwrap()
        );
        let bytes = prepared.payload().to_vec();
        let wire_digest = prepared.wire_sha256().to_owned();
        let mut expected_messages = original_messages.clone();
        if ollama {
            expected_messages[1]["tool_calls"][0]["function"]["arguments"] = arguments;
        }
        let expected = if ollama {
            json!({"model":"m","messages":expected_messages,"stream":true,"tools":provider::tool_catalog(&request),"options":{"num_predict":request.budgets.max_output_tokens}})
        } else {
            json!({"model":"m","messages":expected_messages,"stream":true,"stream_options":{"include_usage":true},"tools":provider::tool_catalog(&request),"max_tokens":request.budgets.max_output_tokens})
        };
        assert_eq!(bytes, serde_json::to_vec(&expected).unwrap());
        assert_eq!(
            wire_digest,
            cowork_domain::canonical_hash(
                &json!({"format":1,"provider_kind":request.provider.as_ref().unwrap().kind,"endpoint":format!("{}{path}",fixture.url),"payload_sha256":cowork_domain::hash(&bytes)})
            )
        );
        assert_eq!(
            messages, original_messages,
            "preparation must not rewrite caller history"
        );

        // Every subsequent caller mutation must be rejected by the binding
        // check, while dispatch continues to use the original sealed payload.
        request.provider.as_mut().unwrap().endpoint = "http://127.0.0.1:1".into();
        request.provider.as_mut().unwrap().model = "changed-model".into();
        request.allowed_tools.clear();
        request.budgets.max_seconds = 0;
        messages[0]["content"] = json!("changed after admission");
        assert!(matches!(
            prepared.assert_binding(&request, &messages),
            Err(cowork_domain::Error::Conflict)
        ));
        assert_eq!(prepared.payload(), bytes);
        assert_eq!(prepared.wire_sha256(), wire_digest);
        let credential = "fixture-key-only-in-authorization-header";
        let (deltas, mut received) = mpsc::channel(4);
        let reply = provider::complete_prepared(prepared, Some(credential.into()), deltas)
            .await
            .unwrap();
        assert_eq!(reply.content, "accepted");
        assert_eq!(received.recv().await.unwrap(), "accepted");
        let sent = fixture.requests.recv().await.unwrap();
        assert_eq!(sent.path, path);
        assert_eq!(sent.headers["content-type"], "application/json");
        assert_eq!(
            sent.headers["authorization"],
            format!("Bearer {credential}")
        );
        assert_eq!(sent.payload, bytes);
        assert!(
            !String::from_utf8(sent.payload)
                .unwrap()
                .contains(credential)
        );
        fixture.stop().await;
    }
}

#[test]
fn prepared_binding_includes_full_spec_and_preserves_legacy_logical_format() {
    let request = spec("http://127.0.0.1:1".into(), ProviderKind::OpenaiCompatible);
    let messages = vec![json!({"role":"user","content":"binding"})];
    let prepared = provider::prepare(&request, &messages, false).unwrap();
    let old_logical = cowork_domain::canonical_hash(
        &json!({"format":1,"provider":request.provider,"messages":messages,"tools":provider::tool_catalog(&request),"max_output_tokens":request.budgets.max_output_tokens}),
    );
    assert_eq!(prepared.logical_fingerprint(), old_logical);
    let mut changes = Vec::new();
    let mut changed = request.clone();
    changed.budgets.max_seconds += 1;
    changes.push(changed);
    let mut changed = request.clone();
    changed.access_mode = AccessMode::Autonomous;
    changes.push(changed);
    let mut changed = request.clone();
    changed.policy_revision += 1;
    changes.push(changed);
    let mut changed = request.clone();
    changed.project_id = "another-project".into();
    changes.push(changed);
    let mut changed = request.clone();
    changed.executor = "another-executor".into();
    changes.push(changed);
    let mut changed = request.clone();
    changed.prompt = "different retained prompt".into();
    changes.push(changed);
    for changed in changes {
        // These changes are intentionally outside the old logical digest.
        assert_eq!(
            provider::request_fingerprint(&changed, &messages).unwrap(),
            old_logical
        );
        assert!(matches!(
            prepared.assert_binding(&changed, &messages),
            Err(cowork_domain::Error::Conflict)
        ));
    }
    assert!(matches!(
        prepared.assert_binding(&request, &[json!({"role":"user","content":"different"})]),
        Err(cowork_domain::Error::Conflict)
    ));
    let mut changed = request.clone();
    changed.provider.as_mut().unwrap().endpoint = "http://127.0.0.1:2".into();
    let changed = provider::prepare(&changed, &messages, false).unwrap();
    assert_ne!(prepared.wire_sha256(), changed.wire_sha256());
    assert_eq!(prepared.payload(), changed.payload());
}

#[tokio::test]
async fn failed_preparation_never_contacts_the_provider() {
    let mut fixture = capture_server("/api/chat", axum::http::StatusCode::OK, "", None).await;
    let request = spec(fixture.url.clone(), ProviderKind::Ollama);
    let invalid = vec![
        json!({"role":"assistant","tool_calls":[{"function":{"name":"request_input","arguments":"not valid JSON"}}]}),
    ];
    assert!(provider::prepare(&request, &invalid, false).is_err());
    let (deltas, _) = mpsc::channel(4);
    assert!(
        provider::complete(&request, &invalid, None, deltas, false)
            .await
            .is_err()
    );
    let oversized = vec![json!({"role":"user","content":"x".repeat(100_000)})];
    assert!(matches!(
        provider::prepare(&request, &oversized, false),
        Err(cowork_domain::Error::ContextLimit)
    ));
    let mut forbidden = request.clone();
    forbidden.provider.as_mut().unwrap().kind = ProviderKind::Test;
    assert!(matches!(
        provider::prepare(&forbidden, &[], false),
        Err(cowork_domain::Error::Forbidden)
    ));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            fixture.requests.recv()
        )
        .await
        .is_err()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn prepared_credentials_and_redirects_fail_without_second_request() {
    let mut recipient =
        capture_server("/chat/completions", axum::http::StatusCode::OK, "", None).await;
    let mut fixture = capture_server(
        "/chat/completions",
        axum::http::StatusCode::TEMPORARY_REDIRECT,
        "",
        Some(format!("{}/chat/completions", recipient.url)),
    )
    .await;
    let mut request = spec(fixture.url.clone(), ProviderKind::OpenaiCompatible);
    request.provider.as_mut().unwrap().credential_required = true;
    let prepared = provider::prepare(&request, &[], false).unwrap();
    let (deltas, _) = mpsc::channel(4);
    assert!(
        matches!(provider::complete_prepared(prepared, None, deltas).await, Err(cowork_domain::Error::Provider(code)) if code == "credential_missing")
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            fixture.requests.recv()
        )
        .await
        .is_err()
    );
    let prepared = provider::prepare(&request, &[], false).unwrap();
    let (deltas, _) = mpsc::channel(4);
    assert!(
        matches!(provider::complete_prepared(prepared, Some("private-credential".into()), deltas).await, Err(cowork_domain::Error::Provider(code)) if code == "http_307")
    );
    assert_eq!(
        fixture.requests.recv().await.unwrap().headers["authorization"],
        "Bearer private-credential"
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            recipient.requests.recv()
        )
        .await
        .is_err()
    );
    fixture.stop().await;
    recipient.stop().await;
}

#[tokio::test]
async fn prepared_test_provider_uses_only_its_frozen_history_and_profile() {
    let mut request = spec("ignored".into(), ProviderKind::Test);
    let mut messages = vec![json!({"role":"tool","content":"original result"})];
    let prepared = provider::prepare(&request, &messages, true).unwrap();
    request.provider.as_mut().unwrap().kind = ProviderKind::OpenaiCompatible;
    messages.clear();
    assert!(prepared.assert_binding(&request, &messages).is_err());
    let (deltas, _) = mpsc::channel(4);
    let reply = provider::complete_prepared(prepared, None, deltas)
        .await
        .unwrap();
    assert!(reply.tools.is_empty());
    assert!(reply.content.contains("Deterministic test completed"));
}

#[test]
fn insecure_remote_endpoints_and_embedded_credentials_are_rejected() {
    for endpoint in [
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com?api_key=secret",
    ] {
        assert!(
            provider::validate_profile(
                spec(endpoint.into(), ProviderKind::OpenaiCompatible)
                    .provider
                    .as_ref()
                    .unwrap(),
                false
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn generation_parameters_reach_each_wire_adapter_and_change_the_binding() {
    for kind in [ProviderKind::OpenaiCompatible, ProviderKind::Ollama] {
        let ollama = matches!(kind, ProviderKind::Ollama);
        let mut request = spec("http://127.0.0.1:1".into(), kind);
        request.provider.as_mut().unwrap().generation = GenerationSettings {
            temperature: Some(0.8),
            timeout_ms: Some(5000),
        };
        let base = provider::prepare(&request, &[], false).unwrap();
        let value: serde_json::Value = serde_json::from_slice(base.payload()).unwrap();
        assert_eq!(
            if ollama {
                &value["options"]["temperature"]
            } else {
                &value["temperature"]
            },
            &json!(0.8)
        );
        request.agent_config = Some(EffectiveAgentConfig {
            generation: GenerationSettings {
                temperature: Some(0.0),
                timeout_ms: Some(1000),
            },
            ..Default::default()
        });
        let copied = provider::prepare(&request, &[], false).unwrap();
        let value: serde_json::Value = serde_json::from_slice(copied.payload()).unwrap();
        assert_eq!(
            if ollama {
                &value["options"]["temperature"]
            } else {
                &value["temperature"]
            },
            &json!(0.0)
        );
        assert_ne!(base.logical_fingerprint(), copied.logical_fingerprint());
        assert!(base.assert_binding(&request, &[]).is_err());
    }
}

#[tokio::test]
async fn frozen_request_timeout_aborts_a_real_unresponsive_provider() {
    let app = Router::new().route(
        "/chat/completions",
        post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            "data: [DONE]\n\n"
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut request = spec(url, ProviderKind::OpenaiCompatible);
    request.agent_config = Some(EffectiveAgentConfig {
        generation: GenerationSettings {
            temperature: None,
            timeout_ms: Some(1000),
        },
        ..Default::default()
    });
    let (tx, _rx) = mpsc::channel(4);
    let started = std::time::Instant::now();
    let result = provider::complete(&request, &[], None, tx, false).await;
    assert!(matches!(result, Err(cowork_domain::Error::Provider(_))));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    task.abort();
}
