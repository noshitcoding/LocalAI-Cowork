//! Codex supplies account refresh/auth only. The harness sees constant input and
//! constant synthetic output; all real model content stays in the V2 tool loop.
use super::*;
use crate::codex_accounts::{CodexBroker, Process};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;
pub const UPSTREAM: &str = "https://chatgpt.com/backend-api/codex/responses";
pub struct CodexTransport {
    broker: Arc<CodexBroker>,
    namespace: String,
    account: String,
    profile_sha: String,
}
impl CodexTransport {
    pub fn assert_profile(&self, profile: &ProviderProfile) -> Result<()> {
        if canonical_hash(profile) != self.profile_sha
            || !matches!(profile.kind, ProviderKind::Codex)
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
pub async fn transport(
    store: &cowork_storage::Store,
    broker: Arc<CodexBroker>,
    namespace: String,
    profile: &ProviderProfile,
) -> Result<CodexTransport> {
    let account = store
        .codex_model_profile_account(&namespace, profile)
        .await?;
    Ok(CodexTransport {
        broker,
        namespace,
        account: account.account_id,
        profile_sha: canonical_hash(profile),
    })
}
pub fn responses_body(
    spec: &RunSpec,
    messages: &[Value],
    tools: &Value,
    planning: bool,
) -> Result<Value> {
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    let locator = parse_codex_profile_endpoint(&profile.endpoint).ok_or(Error::Forbidden)?;
    if spec.model_binding.is_some() {
        return Err(Error::Validation(
            "codex_raw_model_binding_unsupported".into(),
        ));
    }
    let mut instructions = String::new();
    let mut input = Vec::new();
    for message in messages {
        let role = message["role"].as_str().ok_or(Error::Forbidden)?;
        if let Some(continuation) = message
            .get("cowork_provider_continuation")
            .filter(|v| !v.is_null())
        {
            if role != "assistant" {
                return Err(Error::Forbidden);
            }
            validate_continuation(spec, continuation)?;
            input.extend(
                continuation["output"]
                    .as_array()
                    .ok_or(Error::Forbidden)?
                    .iter()
                    .cloned(),
            );
            continue;
        }
        if role == "system" || role == "developer" {
            let content = message["content"].as_str().ok_or(Error::Forbidden)?;
            instructions.push_str(content);
            instructions.push('\n');
            continue;
        }
        if role == "tool" {
            input.push(json!({"type":"function_call_output","call_id":message["tool_call_id"],"output":message["content"]}));
            continue;
        }
        if !matches!(role, "user" | "assistant") {
            return Err(Error::Forbidden);
        }
        if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()) {
            input.push(json!({"type":"message","role":role,"content":[{"type":if role=="assistant"{"output_text"}else{"input_text"},"text":text}]}));
        } else if let (Some(parts), "user") = (message["content"].as_array(), role) {
            // Tool images reach a vision model as one user message of text and image parts.
            let content = parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("text") => Ok(json!({"type":"input_text","text":part["text"]})),
                    Some("image_url") => {
                        Ok(json!({"type":"input_image","image_url":part["image_url"]["url"]}))
                    }
                    _ => Err(Error::Forbidden),
                })
                .collect::<Result<Vec<_>>>()?;
            input.push(json!({"type":"message","role":"user","content":content}));
        }
        if let Some(calls) = message["tool_calls"].as_array() {
            for call in calls {
                input.push(json!({"type":"function_call","call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"]}));
            }
        }
    }
    let mut catalog = Vec::new();
    for tool in tools.as_array().ok_or(Error::Forbidden)? {
        let f = &tool["function"];
        if tool["type"] != "function" || !f["name"].is_string() || !f["parameters"].is_object() {
            return Err(Error::Forbidden);
        }
        catalog.push(json!({"type":"function","name":f["name"],"description":f["description"],"parameters":f["parameters"],"strict":false}));
    }
    let mut body = json!({"model":profile.model,"instructions":instructions,"input":input,"tools":catalog,"tool_choice":"auto","parallel_tool_calls":false,"store":false,"stream":true,"include":["reasoning.encrypted_content"],"max_output_tokens":spec.budgets.max_output_tokens});
    if let Some(effort) = locator.reasoning_effort {
        body["reasoning"] = json!({"effort":effort});
    }
    if let Some(temperature) = spec
        .agent_config
        .as_ref()
        .and_then(|c| c.generation.temperature)
        .or(profile.generation.temperature)
    {
        body["temperature"] = json!(temperature);
    }
    if let Some(output) = spec.output.as_ref().filter(|_| !planning) {
        let format = cowork_domain::structured_output::openai_format(output)?;
        body["text"]["format"] = format["json_schema"].clone();
        body["text"]["format"]["type"] = json!("json_schema");
    }
    if serde_json::to_vec(&body)
        .map_err(storage_error)?
        .len()
        .saturating_add(spec.budgets.max_output_tokens as usize)
        > profile.context_tokens as usize
    {
        return Err(Error::ContextLimit);
    }
    Ok(body)
}
struct StopProcess(Arc<Process>);
impl Drop for StopProcess {
    fn drop(&mut self) {
        self.0.stop();
    }
}
async fn headers(
    socket: &mut tokio::net::TcpStream,
    path: &str,
) -> Result<Vec<(String, Zeroizing<String>)>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let end = loop {
        if bytes.len() > 65536 {
            return Err(Error::Forbidden);
        }
        let mut chunk = [0u8; 4096];
        let n = socket
            .read(&mut chunk)
            .await
            .map_err(|_| Error::UncertainEffect)?;
        if n == 0 {
            return Err(Error::UncertainEffect);
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(pos) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| Error::Forbidden)?;
    let mut lines = text.split("\r\n");
    let expected = format!("POST {path} HTTP/1.1");
    if lines.next() != Some(expected.as_str()) {
        return Err(Error::Forbidden);
    }
    let mut result: Vec<(String, Zeroizing<String>)> = Vec::new();
    let mut length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(Error::Forbidden)?;
        let name = name.to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            if length.is_some() {
                return Err(Error::Forbidden);
            }
            length = Some(value.parse::<usize>().map_err(|_| Error::Forbidden)?);
        } else if name == "transfer-encoding" {
            return Err(Error::Forbidden);
        } else if matches!(
            name.as_str(),
            "authorization" | "chatgpt-account-id" | "openai-beta" | "originator"
        ) {
            if result.iter().any(|(n, _)| n == &name) {
                return Err(Error::Forbidden);
            }
            result.push((name, Zeroizing::new(value.to_owned())));
        }
    }
    let length = length.ok_or(Error::Forbidden)?;
    if length > 2 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let mut remaining = length.saturating_sub(bytes.len() - end);
    let mut discard = [0u8; 8192];
    while remaining > 0 {
        let max = remaining.min(discard.len());
        let n = socket
            .read(&mut discard[..max])
            .await
            .map_err(|_| Error::UncertainEffect)?;
        if n == 0 {
            return Err(Error::UncertainEffect);
        }
        remaining -= n;
    }
    if !result
        .iter()
        .any(|(n, v)| n == "authorization" && v.starts_with("Bearer ") && v.len() > 10)
    {
        return Err(Error::Provider("codex_account_sign_in_required".into()));
    }
    Ok(result)
}
fn synthetic() -> Vec<u8> {
    let response = json!({"id":"resp_cowork_auth_ack","object":"response","status":"completed","model":"gateway","output":[{"type":"message","id":"msg_cowork_auth_ack","role":"assistant","status":"completed","content":[{"type":"output_text","text":"OK","annotations":[]}]}],"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}});
    let events = [
        json!({"type":"response.created","response":{"id":"resp_cowork_auth_ack","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":response["output"][0]}),
        json!({"type":"response.completed","response":response}),
    ];
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect::<String>()
        .into_bytes()
}
pub async fn complete(
    access: CodexTransport,
    spec: &RunSpec,
    payload: Vec<u8>,
    deltas: mpsc::Sender<String>,
    observer: ProviderReceiveObserver,
) -> Result<Reply> {
    complete_at(access, spec, payload, deltas, observer, UPSTREAM).await
}
async fn complete_at(
    access: CodexTransport,
    spec: &RunSpec,
    payload: Vec<u8>,
    deltas: mpsc::Sender<String>,
    observer: ProviderReceiveObserver,
    upstream: &str,
) -> Result<Reply> {
    let profile = spec.provider.as_ref().ok_or(Error::Forbidden)?;
    access.assert_profile(profile)?;
    let timeout = Duration::from_millis(
        spec.agent_config
            .as_ref()
            .and_then(|c| c.generation.timeout_ms)
            .or(profile.generation.timeout_ms)
            .unwrap_or(spec.budgets.max_seconds.saturating_mul(1000))
            .min(spec.budgets.max_seconds.saturating_mul(1000)),
    );
    tokio::time::timeout(timeout,async move{
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|_|Error::Provider("codex_gateway_unavailable".into()))?;
        let path=format!("/cowork-auth-{}/responses",uuid::Uuid::new_v4());let gateway=format!("http://{}{}",listener.local_addr().map_err(|_|Error::Forbidden)?,path.strip_suffix("/responses").ok_or(Error::Forbidden)?);
        let broker=access.broker;let namespace=access.namespace;let account=access.account;let model=profile.model.clone();
        let process=tokio::task::spawn_blocking(move||broker.gateway_process(&namespace,&account,&gateway)).await.map_err(|_|Error::UncertainEffect)??;let process=Arc::new(process);let _stop=StopProcess(process.clone());
        let starting=process.clone();let setup=tokio::task::spawn_blocking(move||{let thread=starting.request("thread/start",Some(json!({"model":model,"ephemeral":true,"approvalPolicy":"never","sandbox":"read-only","baseInstructions":"Reply OK. Never use tools.","developerInstructions":"Authentication transport only. Acknowledge with OK."})))?;let id=thread["thread"]["id"].as_str().ok_or(Error::Forbidden)?;starting.request("turn/start",Some(json!({"threadId":id,"input":[{"type":"text","text":"Constant authentication transport request. Reply OK."}]})))?;Ok::<_,Error>(())});
        let (mut socket,_)=listener.accept().await.map_err(|_|Error::UncertainEffect)?;drop(listener);let forwarded=headers(&mut socket,&path).await?;
        // No harness body or tools are ever forwarded. This exact payload was
        // serialized and durably admitted by V2 before the auth harness starts.
        let client=Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(timeout).build().map_err(|_|Error::Forbidden)?;let mut request=client.post(upstream).header("content-type","application/json").body(payload);for (name,value) in forwarded.iter(){request=request.header(name,value.as_str());}
        let response=request.send().await.map_err(|_|Error::UncertainEffect)?;if !response.status().is_success(){return Err(Error::Provider(format!("codex_upstream_http_{}_not_replayed",response.status().as_u16())));}
        let mut stream=response.bytes_stream();let mut buffer=Vec::new();let mut completed=None;let mut streamed=String::new();let mut output_bytes=0usize;
        while let Some(chunk)=stream.next().await {let chunk=chunk.map_err(|_|Error::UncertainEffect)?;observer.observe_chunk(&chunk,false);buffer.extend_from_slice(&chunk);if observer.0.lock().expect("receive observer poisoned").bytes.len()>16*1024*1024||buffer.len()>2*1024*1024{return Err(Error::BudgetExceeded);}while let Some(end)=buffer.iter().position(|b|*b==b'\n'){let line=buffer.drain(..=end).collect::<Vec<_>>();let raw=std::str::from_utf8(&line).map_err(|_|Error::Forbidden)?.trim();let Some(data)=raw.strip_prefix("data:").map(str::trim).filter(|data|*data!="[DONE]")else{continue;};let event:Value=serde_json::from_str(data).map_err(|_|Error::Forbidden)?;match event["type"].as_str(){Some("response.output_text.delta")=>{let text=event["delta"].as_str().ok_or(Error::Forbidden)?;output_bytes=output_bytes.saturating_add(text.len());if output_bytes>spec.budgets.max_output_tokens as usize{return Err(Error::BudgetExceeded);}streamed.push_str(text);deltas.send(text.into()).await.map_err(|_|Error::UncertainEffect)?;},Some("response.function_call_arguments.delta")=>{output_bytes=output_bytes.saturating_add(event["delta"].as_str().ok_or(Error::Forbidden)?.len());if output_bytes>spec.budgets.max_output_tokens as usize{return Err(Error::BudgetExceeded);}},Some("response.completed")=>{if completed.is_some(){return Err(Error::Forbidden);}completed=Some(event["response"].clone());},Some("error"|"response.failed"|"response.incomplete")=>return Err(Error::Provider("codex_model_response_incomplete".into())),_=>{}}}}
        let response=completed.ok_or_else(||Error::Provider("codex_response_stream_incomplete".into()))?;let reply=parse_reply(spec,&response,&streamed,&observer)?;
        let wire=synthetic();let head=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",wire.len());socket.write_all(head.as_bytes()).await.map_err(|_|Error::UncertainEffect)?;socket.write_all(&wire).await.map_err(|_|Error::UncertainEffect)?;let _=socket.shutdown().await;let _=setup.await;Ok(reply)
    }).await.map_err(|_|Error::UncertainEffect)?
}
fn parse_reply(
    spec: &RunSpec,
    response: &Value,
    streamed: &str,
    observer: &ProviderReceiveObserver,
) -> Result<Reply> {
    if response["status"] != "completed" {
        return Err(Error::Provider("codex_response_incomplete".into()));
    }
    let mut content = String::new();
    let mut tools = Vec::new();
    let continuation = json!({"format":"codex-responses-continuation/1","profile_sha256":canonical_hash(spec.provider.as_ref().ok_or(Error::Forbidden)?),"output":response["output"]});
    validate_continuation(spec, &continuation)?;
    for item in response["output"].as_array().ok_or(Error::Forbidden)? {
        match item["type"].as_str() {
            Some("message") => {
                for part in item["content"].as_array().ok_or(Error::Forbidden)? {
                    if part["type"] == "output_text" {
                        content.push_str(part["text"].as_str().ok_or(Error::Forbidden)?);
                    }
                }
            }
            Some("function_call") => {
                let id = item["call_id"].as_str().ok_or(Error::Forbidden)?.to_owned();
                let name = item["name"].as_str().ok_or(Error::Forbidden)?.to_owned();
                if !spec.allowed_tools.contains(&name) {
                    return Err(Error::Forbidden);
                }
                let arguments: Value =
                    serde_json::from_str(item["arguments"].as_str().ok_or(Error::Forbidden)?)
                        .map_err(|_| Error::Forbidden)?;
                if !arguments.is_object() || id.is_empty() || id.len() > 200 {
                    return Err(Error::Forbidden);
                }
                tools.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            Some("reasoning") => {}
            _ => return Err(Error::Provider("codex_builtin_output_forbidden".into())),
        }
    }
    if (!streamed.is_empty() && content != streamed) || tools.len() > 16 {
        return Err(Error::Forbidden);
    }
    validate_tool_ids(&[], &tools)?;
    let bytes = tools
        .iter()
        .try_fold(content.len(), |total, tool| {
            serde_json::to_vec(&tool.arguments).map(|v| total.saturating_add(v.len()))
        })
        .map_err(storage_error)?;
    if bytes > spec.budgets.max_output_tokens as usize {
        return Err(Error::BudgetExceeded);
    }
    let usage = &response["usage"];
    let input = usage["input_tokens"].as_u64();
    let output = usage["output_tokens"].as_u64();
    if output.is_some_and(|n| n > u64::from(spec.budgets.max_output_tokens)) {
        return Err(Error::BudgetExceeded);
    }
    let normalized = json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":usage["total_tokens"],"prompt_tokens_details":usage["input_tokens_details"],"completion_tokens_details":usage["output_tokens_details"]});
    let mut observed = observer.0.lock().expect("receive observer poisoned");
    observed.eof = true;
    observed.completed_stream = true;
    observed.completed_usage = Some(normalized.clone());
    observed.usage.push(normalized.clone());
    let received = observed
        .scope
        .clone()
        .map(|scope| {
            cowork_storage::model_money::CompletedModelReceipt::from_completed_stream(
                scope,
                &hash(&observed.bytes),
                normalized,
                false,
            )
        })
        .transpose()
        .map_err(|error| Error::Storage(Box::new(error)))?;
    Ok(Reply {
        provider_continuation: Some(continuation),
        content,
        tools,
        usage: Usage {
            input_tokens: input,
            output_tokens: output,
            incomplete: input.is_none() || output.is_none(),
            ..Default::default()
        },
        received_cost_metadata: None,
        received,
    })
}
pub fn validate_continuation(spec: &RunSpec, value: &Value) -> Result<()> {
    if value["format"] != "codex-responses-continuation/1"
        || value["profile_sha256"]
            != canonical_hash(spec.provider.as_ref().ok_or(Error::Forbidden)?)
        || value.as_object().is_none_or(|o| o.len() != 3)
        || serde_json::to_vec(value).map_err(storage_error)?.len() > 512 * 1024
    {
        return Err(Error::Forbidden);
    }
    let output = value["output"].as_array().ok_or(Error::Forbidden)?;
    if output.len() > 128
        || output.iter().any(|item| {
            !matches!(
                item["type"].as_str(),
                Some("reasoning" | "message" | "function_call")
            ) || item["type"] == "message" && item["role"] != "assistant"
        })
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_spec() -> RunSpec {
        let profile = ProviderProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Fixture".into(),
            kind: ProviderKind::Codex,
            endpoint: codex_profile_endpoint(&CodexProfileLocator {
                account_id: uuid::Uuid::new_v4().to_string(),
                account_revision: 1,
                reasoning_effort: Some("low".into()),
            })
            .unwrap(),
            model: "gpt-5.4".into(),
            context_tokens: 32768,
            revision: 1,
            credential_required: false,
            credential_ready: true,
            generation: Default::default(),
            capabilities: Default::default(),
        };
        serde_json::from_value(json!({"project_id":"fixture","order_id":"fixture","prompt":"PRIVATE_V2_PROMPT","provider":profile,"access_mode":"confirm_all","executor":"local-v2","policy_revision":1,"allowed_tools":["create_artifact"],"budgets":Budgets::default(),"folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null})).unwrap()
    }
    #[test]
    fn responses_use_only_v2_messages_tools_and_frozen_model_options() {
        let spec = fixture_spec();
        let messages = vec![
            json!({"role":"system","content":"Original role rules"}),
            json!({"role":"user","content":"PRIVATE_V2_PROMPT"}),
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","function":{"name":"create_artifact","arguments":"{\"name\":\"x\",\"content\":\"y\"}"}}]}),
            json!({"role":"tool","tool_call_id":"call_1","content":"Approved V2 result"}),
        ];
        let body = responses_body(&spec, &messages, &tool_catalog(&spec), false).unwrap();
        assert_eq!(body["instructions"], "Original role rules\n");
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["max_output_tokens"], spec.budgets.max_output_tokens);
        assert!(
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["name"] == "create_artifact")
        );
        let blocked = json!({"status":"completed","output":[{"type":"local_shell_call","action":{"command":"unauthorized"}}]});
        assert!(
            parse_reply(
                &spec,
                &blocked,
                "",
                &ProviderReceiveObserver::evidence_only()
            )
            .is_err()
        );
        let synthetic = String::from_utf8(synthetic()).unwrap();
        assert!(!synthetic.contains("PRIVATE_V2_PROMPT"));
        assert!(!synthetic.contains("create_artifact"));
        assert!(synthetic.contains("OK"));
    }
    #[tokio::test]
    #[ignore = "Requires explicit pinned COWORK_TEST_CODEX_MANIFEST; uses dummy auth and loopback only"]
    async fn pinned_auth_gateway_routes_real_v2_response_without_exposing_it_to_codex() {
        let manifest =
            std::env::var("COWORK_TEST_CODEX_MANIFEST").expect("explicit pinned manifest");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("codex-runtime.json"),
            serde_json::to_vec(&json!({"manifest":manifest})).unwrap(),
        )
        .unwrap();
        let mut broker = CodexBroker::new(dir.path().to_path_buf());
        broker.fixture_auth = true;
        let broker = Arc::new(broker);
        assert!(broker.status().available);
        let spec = fixture_spec();
        let profile = spec.provider.as_ref().unwrap();
        let account = parse_codex_profile_endpoint(&profile.endpoint)
            .unwrap()
            .account_id;
        let access = CodexTransport {
            broker: broker.clone(),
            namespace: "installation-owner".into(),
            account: account.clone(),
            profile_sha: canonical_hash(profile),
        };
        let body = responses_body(
            &spec,
            &[json!({"role":"user","content":"PRIVATE_V2_PROMPT"})],
            &tool_catalog(&spec),
            false,
        )
        .unwrap();
        let expected = body.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let app=axum::Router::new().route("/responses",axum::routing::post(move|headers:axum::http::HeaderMap,axum::Json(body):axum::Json<Value>|{let calls=seen.clone();let expected=expected.clone();async move{
            assert_eq!(headers["authorization"],"Bearer synthetic-gateway-fixture-key");let ordinal=calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            let output=if ordinal==0{assert_eq!(body,expected);json!([{"type":"reasoning","id":"rs_fixture","encrypted_content":"opaque-fixture-reasoning","summary":[]},{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"PRIVATE_V2_REPLY"}]},{"type":"function_call","call_id":"call_actual","name":"create_artifact","arguments":"{\"name\":\"approved.txt\",\"content\":\"actual\"}"}])}
            else{assert_eq!(ordinal,1);assert_eq!(body["input"][1]["encrypted_content"],"opaque-fixture-reasoning");assert_eq!(body["input"][2]["phase"],"commentary");assert_eq!(body["input"][3]["call_id"],"call_actual");assert_eq!(body["input"][4]["type"],"function_call_output");assert_eq!(body["input"][4]["output"],"Original reviewed fixture result");json!([{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Completed after durable tool receipt"}]}])};
            let response=json!({"id":format!("resp_actual_{ordinal}"),"status":"completed","output":output,"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}});([(axum::http::header::CONTENT_TYPE,"text/event-stream")],format!("event: response.completed\ndata: {}\n\n",json!({"type":"response.completed","response":response})))}}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tx, _rx) = mpsc::channel(16);
        let observer = ProviderReceiveObserver::evidence_only();
        let reply = complete_at(
            access,
            &spec,
            serde_json::to_vec(&body).unwrap(),
            tx,
            observer.clone(),
            &endpoint,
        )
        .await
        .unwrap();
        assert_eq!(reply.content, "PRIVATE_V2_REPLY");
        assert_eq!(reply.tools.len(), 1);
        assert_eq!(reply.tools[0].name, "create_artifact");
        assert_eq!(reply.usage.output_tokens, Some(20));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(observer.received_evidence().unwrap().unwrap()["completed_stream"] == true);
        let path = dir.path().join("accepted-v2.sqlite");
        let store = cowork_storage::Store::sqlite(&path).await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Codex fixture".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Tool continuation".into(),
            })
            .await
            .unwrap();
        let mut persisted_spec = spec.clone();
        persisted_spec.project_id = project.id;
        persisted_spec.order_id = order.id.clone();
        let create:CreateRun=serde_json::from_value(json!({"order_id":order.id,"base_revision":1,"idempotency_key":"codex-real-fixture","prompt":persisted_spec.prompt,"provider_id":profile.id,"provider_revision":1,"access_mode":"confirm_all"})).unwrap();
        let run = store.create_run(&create, persisted_spec).await.unwrap();
        store.mutate(&run.id,None,|run|{run.checkpoint.messages=vec![json!({"role":"user","content":"PRIVATE_V2_PROMPT"}),json!({"role":"assistant","content":reply.content,"tool_calls":[{"id":"call_actual","type":"function","function":{"name":"create_artifact","arguments":"{\"name\":\"approved.txt\",\"content\":\"actual\"}"}}],"cowork_provider_continuation":reply.provider_continuation}),json!({"role":"tool","tool_call_id":"call_actual","content":"Original reviewed fixture result"})];Ok(cowork_storage::Change::event("fixture_original_tool_receipt",json!({}))) }).await.unwrap();
        store.close().await;
        let reopened = cowork_storage::Store::sqlite(&path).await.unwrap();
        let restored = reopened.stored_run(&run.id).await.unwrap();
        let body = responses_body(
            &restored.record.spec,
            &restored.checkpoint.messages,
            &tool_catalog(&restored.record.spec),
            false,
        )
        .unwrap();
        let access = CodexTransport {
            broker: broker.clone(),
            namespace: "installation-owner".into(),
            account: account.clone(),
            profile_sha: canonical_hash(profile),
        };
        let (tx, _rx) = mpsc::channel(16);
        let second = complete_at(
            access,
            &restored.record.spec,
            serde_json::to_vec(&body).unwrap(),
            tx,
            ProviderReceiveObserver::evidence_only(),
            &endpoint,
        )
        .await
        .unwrap();
        assert_eq!(second.content, "Completed after durable tool receipt");
        assert!(second.tools.is_empty());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let mut changed = restored.record.spec.clone();
        changed.provider.as_mut().unwrap().model = "foreign-model".into();
        assert!(
            responses_body(
                &changed,
                &restored.checkpoint.messages,
                &tool_catalog(&changed),
                false
            )
            .is_err()
        );
        reopened.close().await;

        let home = dir
            .path()
            .join("codex-profiles")
            .join(hash(b"installation-owner"))
            .join(&account);
        assert!(!home.join("auth.json").exists());
        assert!(!home.join("approved.txt").exists());
        broker.stop_account("installation-owner", &account).unwrap();
        server.abort();
    }
}
