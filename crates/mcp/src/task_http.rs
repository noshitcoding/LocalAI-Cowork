//! One task operation and its optional server-to-client GET stream, under the
//! same held session and original operation deadline. No reconnect or replay.
use super::*;
use futures_util::{future::BoxFuture, stream::BoxStream};
use std::collections::BTreeSet;

type Pending = BoxFuture<'static, Result<Response>>;
type Body = BoxStream<'static, Result<Vec<u8>>>;

enum Event {
    Post(Response),
    Get(Response),
    PostChunk(Option<Result<Vec<u8>>>),
    GetChunk(Option<Result<Vec<u8>>>),
}

async fn response(pending: &mut Option<Pending>) -> Result<Response> {
    match pending {
        Some(value) => value.await,
        None => std::future::pending().await,
    }
}
async fn chunk(body: &mut Option<Body>) -> Option<Result<Vec<u8>>> {
    match body {
        Some(value) => value.next().await,
        None => std::future::pending().await,
    }
}
fn mime(response: &Response, maximum: usize) -> Result<String> {
    let mut values = response.headers().get_all(CONTENT_TYPE).iter();
    let value = values
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Protocol)?;
    if values.next().is_some()
        || response
            .content_length()
            .is_some_and(|n| n > maximum as u64)
    {
        return Err(Error::Protocol);
    }
    Ok(value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase())
}
fn body(response: Response) -> Body {
    Box::pin(
        response
            .bytes_stream()
            .map(|value| value.map(|v| v.to_vec()).map_err(|_| Error::Transport)),
    )
}

impl Transport {
    fn task_request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, self.endpoint.url.clone())
            .headers(self.endpoint.headers.clone())
            .header("MCP-Protocol-Version", self.protocol_version());
        if let Some(auth) = &self.endpoint.authorization {
            request = request.header(AUTHORIZATION, auth);
        }
        if let Some(session) = &self.session_id {
            request = request.header("Mcp-Session-Id", session);
        }
        request
    }

    pub(super) async fn request_interactive_http(
        &mut self,
        method: &str,
        params: Value,
        request_id: String,
        context: &TaskRequestContext,
        handler: &dyn TaskInteractionHandler,
    ) -> Result<Reply> {
        let post = self
            .task_request(reqwest::Method::POST)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":params}));
        let get = self
            .task_request(reqwest::Method::GET)
            .header(ACCEPT, "text/event-stream");
        let mut post_pending: Option<Pending> = Some(Box::pin(async move {
            post.send().await.map_err(|_| Error::Transport)
        }));
        let mut get_pending: Option<Pending> = Some(Box::pin(async move {
            get.send().await.map_err(|_| Error::Transport)
        }));
        let mut post_body = None;
        let mut get_body = None;
        let mut post_sse = wire::Sse::new(self.limits.max_response_bytes, self.limits.max_messages);
        let mut get_sse = wire::Sse::new(self.limits.max_response_bytes, self.limits.max_messages);
        let mut post_json: Option<Vec<u8>> = None;
        let mut total = 0usize;
        let mut count = 0usize;
        let mut requests = BTreeSet::new();
        let mut catalog_changed = false;
        loop {
            let event = tokio::select! {
                value = response(&mut post_pending) => Event::Post(value?),
                value = response(&mut get_pending) => Event::Get(value?),
                value = chunk(&mut post_body) => Event::PostChunk(value),
                value = chunk(&mut get_body) => Event::GetChunk(value),
            };
            let (values, side) = match event {
                Event::Post(value) => {
                    post_pending = None;
                    self.check_headers(&value, false)?;
                    if value.status() != reqwest::StatusCode::OK {
                        return Err(status_error(&value));
                    }
                    match mime(&value, self.limits.max_response_bytes)?.as_str() {
                        "application/json" => post_json = Some(Vec::new()),
                        "text/event-stream" => {}
                        _ => return Err(Error::Protocol),
                    }
                    post_body = Some(body(value));
                    continue;
                }
                Event::Get(value) => {
                    get_pending = None;
                    self.check_headers(&value, false)?;
                    if value.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
                        continue;
                    }
                    if value.status() != reqwest::StatusCode::OK {
                        return Err(status_error(&value));
                    }
                    if mime(&value, self.limits.max_response_bytes)? != "text/event-stream" {
                        return Err(Error::Protocol);
                    }
                    get_body = Some(body(value));
                    continue;
                }
                Event::PostChunk(None) => {
                    if let Some(bytes) = post_json {
                        return response_message(
                            wire::parse(&bytes)?,
                            &request_id,
                            catalog_changed,
                        );
                    }
                    return Err(Error::Protocol);
                }
                Event::GetChunk(None) => {
                    get_body = None;
                    continue;
                }
                Event::PostChunk(Some(value)) => {
                    let bytes = value?;
                    total = total.checked_add(bytes.len()).ok_or(Error::Limit)?;
                    if total > self.limits.max_response_bytes {
                        return Err(Error::Limit);
                    }
                    if let Some(json) = &mut post_json {
                        json.extend_from_slice(&bytes);
                        continue;
                    }
                    (post_sse.push(&bytes)?, false)
                }
                Event::GetChunk(Some(value)) => {
                    let bytes = value?;
                    total = total.checked_add(bytes.len()).ok_or(Error::Limit)?;
                    if total > self.limits.max_response_bytes {
                        return Err(Error::Limit);
                    }
                    (get_sse.push(&bytes)?, true)
                }
            };
            for value in values {
                count += 1;
                if count > self.limits.max_messages {
                    return Err(Error::Limit);
                }
                envelope(&value)?;
                let Some(method) = value.get("method") else {
                    if side {
                        return Err(Error::Protocol);
                    }
                    return response_message(value, &request_id, catalog_changed);
                };
                let method = method.as_str().ok_or(Error::Protocol)?;
                if method.is_empty()
                    || method.len() > 256
                    || value.get("result").is_some()
                    || value.get("error").is_some()
                    || value
                        .get("params")
                        .is_some_and(|p| !p.is_object() && !p.is_array())
                {
                    return Err(Error::Protocol);
                }
                if let Some(id) = value.get("id") {
                    valid_id(id)?;
                    // A broken server broadcasting one request across both streams
                    // cannot cause a second model call or a second human response.
                    if !requests.insert(serde_json::to_string(id).map_err(|_| Error::Protocol)?) {
                        return Err(Error::Protocol);
                    }
                    if matches!(method, "sampling/createMessage" | "elicitation/create") {
                        let original = TaskServerRequest::checked(value.clone(), context)?;
                        let prepared = handler.respond(original).await?;
                        handler.validate_response(&prepared).await?;
                        if let Some(bearer) = handler.response_bearer().await? {
                            self.replace_bearer(&bearer)?;
                        }
                        // Refresh may wait for the original OAuth operation.
                        handler.validate_response(&prepared).await?;
                        let mut reply = prepared.body().clone();
                        reply["jsonrpc"] = json!("2.0");
                        reply["id"] = id.clone();
                        match self.notify(reply).await {
                            Ok(()) => handler.acknowledge(&prepared, json!({"transport":"streamable_http","request_stream":if side {"get"} else {"post"},"status":202,"empty_body":true,"remote_effects_unknown":true})).await?,
                            Err(error) => {let _ = handler.unknown(&prepared).await;return Err(error);}
                        }
                    } else {
                        let reply = if method == "ping" {
                            json!({"jsonrpc":"2.0","id":id,"result":{}})
                        } else {
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                        };
                        self.notify(reply).await?;
                    }
                } else if method == "notifications/tools/list_changed" {
                    catalog_changed = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Body as HttpBody,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response as HttpResponse},
        routing::post,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Shared {
        requests: Mutex<Vec<(String, HeaderMap, Value)>>,
        response_received: tokio::sync::Notify,
        responses: AtomicUsize,
        acknowledged: AtomicUsize,
    }
    async fn post_message(
        State(state): State<Arc<Shared>>,
        headers: HeaderMap,
        Json(value): Json<Value>,
    ) -> HttpResponse {
        state
            .requests
            .lock()
            .unwrap()
            .push(("post".into(), headers, value.clone()));
        if value.get("method").is_none() {
            state.response_received.notify_one();
            return StatusCode::ACCEPTED.into_response();
        }
        state.response_received.notified().await;
        Json(json!({"jsonrpc":"2.0","id":value["id"],"result":{"content":[{"type":"text","text":"original result"}],"_meta":{"io.modelcontextprotocol/related-task":{"taskId":"held-task"}}}})).into_response()
    }
    async fn get_messages(State(state): State<Arc<Shared>>, headers: HeaderMap) -> HttpResponse {
        state
            .requests
            .lock()
            .unwrap()
            .push(("get".into(), headers, Value::Null));
        let request = json!({"jsonrpc":"2.0","id":"original-server-request","method":"elicitation/create","params":{"mode":"form","message":"original question","requestedSchema":{"type":"object","properties":{}},"_meta":{"io.modelcontextprotocol/related-task":{"taskId":"held-task"}}}});
        HttpResponse::builder()
            .header("content-type", "text/event-stream")
            .body(HttpBody::from(format!("data: {request}\n\n")))
            .unwrap()
    }
    impl TaskInteractionHandler for Shared {
        fn response_bearer(&self) -> BoxFuture<'_, Result<Option<zeroize::Zeroizing<String>>>> {
            Box::pin(async { Ok(Some(zeroize::Zeroizing::new("after-human-token".into()))) })
        }
        fn respond<'a>(
            &'a self,
            request: TaskServerRequest,
        ) -> BoxFuture<'a, Result<crate::TaskServerResponse>> {
            Box::pin(async move {
                assert_eq!(request.task_id(), "held-task");
                assert_eq!(request.operation_rpc_id(), "original-operation");
                assert_eq!(request.connection_epoch(), 7);
                self.responses.fetch_add(1, Ordering::SeqCst);
                crate::TaskServerResponse::prepared(
                    "persisted-original-response".into(),
                    json!({"result":{"action":"accept","content":{}}}),
                )
            })
        }
        fn validate_response<'a>(
            &'a self,
            _: &'a crate::TaskServerResponse,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn acknowledge<'a>(
            &'a self,
            _: &'a crate::TaskServerResponse,
            receipt: Value,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                assert_eq!(receipt["status"], 202);
                assert_eq!(receipt["request_stream"], "get");
                self.acknowledged.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
        fn unknown<'a>(&'a self, _: &'a crate::TaskServerResponse) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { panic!("unexpected uncertain reply") })
        }
    }
    #[tokio::test]
    async fn task_get_sse_delivers_the_bound_question_without_reposting_the_tool() {
        let state = Arc::new(Shared::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/mcp", post(post_message).get(get_messages))
                    .with_state(state.clone()),
            )
            .into_future(),
        );
        let mut transport = Transport::new(
            Endpoint::new(&url, Some("old-token")).unwrap(),
            Limits::default(),
        )
        .unwrap();
        transport
            .restore_session_id(Some(b"original-http-session"))
            .unwrap();
        transport.replace_bearer("fresh-token").unwrap();
        let result = transport
            .request_task_interactive(
                "tasks/result",
                json!({"taskId":"held-task"}),
                "original-operation".into(),
                2000,
                TaskRequestContext {
                    connection_id: uuid::Uuid::new_v4().to_string(),
                    connection_epoch: 7,
                    task_id: "held-task".into(),
                    operation_rpc_id: "original-operation".into(),
                },
                state.as_ref(),
            )
            .await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        assert_eq!(
            result.unwrap().result.unwrap()["content"][0]["text"],
            "original result"
        );
        assert_eq!(state.responses.load(Ordering::SeqCst), 1);
        assert_eq!(state.acknowledged.load(Ordering::SeqCst), 1);
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests
                .iter()
                .filter(|(_, _, v)| v["method"] == "tasks/result")
                .count(),
            1
        );
        assert!(requests.iter().all(|(_, h, v)| h["authorization"]
            == if v.get("result").is_some() {
                "Bearer after-human-token"
            } else {
                "Bearer fresh-token"
            }
            && h["mcp-session-id"] == "original-http-session"
            && h["mcp-protocol-version"] == PROTOCOL_VERSION
            && v["method"] != "tools/call"));
        assert_eq!(
            requests
                .iter()
                .find(|(_, _, v)| v.get("result").is_some())
                .unwrap()
                .2["id"],
            "original-server-request"
        );
    }
}
