use super::*;
use axum::{
    Json, Router,
    extract::State,
    response::sse::{Event, Sse},
    routing::{get, post},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, mpsc};
#[derive(Clone)]
struct Server {
    out: mpsc::Sender<Event>,
    input: Arc<Mutex<Option<mpsc::Receiver<Event>>>>,
    gets: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}
async fn stream(
    State(s): State<Server>,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, std::convert::Infallible>>> {
    s.gets.fetch_add(1, Ordering::SeqCst);
    let input = s.input.lock().await.take().unwrap();
    let first = Event::default()
        .event("endpoint")
        .data("/messages?session_id=original");
    let stream =
        futures_util::stream::unfold((Some(first), input), |(first, mut input)| async move {
            let event = if let Some(first) = first {
                first
            } else {
                input.recv().await?
            };
            Some((Ok(event), (None, input)))
        });
    Sse::new(stream)
}
async fn receive(
    State(s): State<Server>,
    Json(v): Json<Value>,
) -> (reqwest::StatusCode, &'static str) {
    if let Some(id) = v.get("id") {
        let result = match v["method"].as_str() {
            Some("initialize") => {
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"Legacy fixture","version":"1"}})
            }
            Some("tools/list") => {
                json!({"tools":[{"name":"inspect","inputSchema":{"type":"object","additionalProperties":false}}]})
            }
            Some("tools/call") => {
                s.calls.fetch_add(1, Ordering::SeqCst);
                json!({"content":[{"type":"text","text":"Original SSE reply"}]})
            }
            _ => json!({}),
        };
        s.out
            .send(
                Event::default()
                    .event("message")
                    .json_data(json!({"jsonrpc":"2.0","id":id,"result":result}))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    (reqwest::StatusCode::ACCEPTED, "Accepted")
}
#[tokio::test]
async fn original_legacy_stream_drives_discovery_and_one_exact_call() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel(16);
    let server = Server {
        out: tx,
        input: Arc::new(Mutex::new(Some(rx))),
        gets: Arc::new(AtomicUsize::new(0)),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/sse", get(stream))
        .route("/messages", post(receive))
        .with_state(server.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut endpoint = Endpoint::new(&format!("http://{address}/sse"), None).unwrap();
    endpoint.legacy = true; // Local fixture only; production Legacy construction requires pinned public HTTPS.
    let operation = async {
        let mut session = crate::Session::connect(endpoint, Limits::default())
            .await
            .unwrap();
        assert_eq!(session.protocol_version(), "2024-11-05");
        let catalog = session.discover().await.unwrap().sha256().to_owned();
        let prepared = session.prepare(&catalog, "inspect", json!({})).unwrap();
        assert!(prepared.endpoint().contains("session_id=original"));
        let reply = session.call(prepared).await.unwrap();
        assert_eq!(reply.response["content"][0]["text"], "Original SSE reply");
        assert_eq!(server.gets.load(Ordering::SeqCst), 1);
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    };
    let result = tokio::time::timeout(Duration::from_secs(5), operation).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    result.unwrap();
}
