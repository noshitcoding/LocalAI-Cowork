//! Isolated real API/worker/browser fixture with a synthetic loopback model.
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type Requests = Arc<Mutex<Vec<Value>>>;
async fn models() -> Json<Value> {
    Json(json!({"data":[{"id":"typed-fixture-model"}]}))
}
async fn requests(State(requests): State<Requests>) -> Json<Value> {
    Json(json!(requests.lock().unwrap().clone()))
}
async fn chat(State(requests): State<Requests>, Json(body): Json<Value>) -> String {
    requests.lock().unwrap().push(body.clone());
    let frame = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| value["workflow_step"].is_string())
        .unwrap();
    let output = if frame["workflow_step"] == "extract" {
        json!({"invoice_id":"INV-001","total":42.5})
    } else {
        json!({"summary":"INV-001: EUR 42.5"})
    };
    let reply = json!({"choices":[{"index":0,"delta":{"content":output.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":32}});
    format!("data: {reply}\n\ndata: [DONE]\n\n")
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Pass a new absolute fixture directory"))?,
    );
    if !directory.is_absolute() {
        anyhow::bail!("Fixture directory must be absolute");
    }
    std::fs::create_dir(&directory)?;
    let stop = CancellationToken::new();
    let model_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let model_base = format!("http://{}", model_listener.local_addr()?);
    let model_router = Router::new()
        .route("/models", get(models))
        .route("/chat/completions", post(chat))
        .route("/requests", get(requests))
        .with_state(Requests::default());
    let model_stop = stop.clone();
    let model = tokio::spawn(async move {
        axum::serve(model_listener, model_router)
            .with_graceful_shutdown(model_stop.cancelled_owned())
            .await
    });
    let store = Store::sqlite(&directory.join("data.sqlite3")).await?;
    let project = store
        .create_project(CreateProject {
            name: "Typed workflow qualification".into(),
        })
        .await?;
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Typed inputs and results".into(),
        })
        .await?;
    let profile: ProviderProfile = serde_json::from_value(
        json!({"id":"typed-fixture","name":"Synthetic loopback model","kind":"openai_compatible","endpoint":model_base,"model":"typed-fixture-model","revision":1,"context_tokens":16384,"credential_required":false,"credential_ready":true}),
    )?;
    store.insert_provider(&profile).await?;
    let definition: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-dataflow.json"
    ))?;
    let runtime = Runtime::new(
        store.clone(),
        vec![ConfiguredProvider {
            profile: profile.clone(),
            credential_env: None,
        }],
        false,
    )?
    .with_file_broker(cowork_broker::FileBroker::open(
        &directory.join("broker"),
        vec![],
    )?);
    let token = "workflow-dataflow-fixture-only";
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new(token.into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5191".into()]),
        },
        None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let ready = json!({"base":format!("http://{}",listener.local_addr()?),"token":token,"project":project,"order":order,"profile":profile,"definition":definition,"model_base":model_base});
    std::fs::write(
        directory.join("ready.json"),
        serde_json::to_vec_pretty(&ready)?,
    )?;
    println!("fixture_ready");
    let worker = tokio::spawn(runtime.clone().serve(2, stop.clone()));
    let events_stop = stop.clone();
    let events = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {_=events_stop.cancelled()=>break,_=interval.tick()=>{if let Err(error)=runtime.automation_tick().await {eprintln!("fixture_tick_error:{}",error.code());}}}
        }
    });
    let signal_stop = stop.clone();
    let signal_path = directory.join("STOP");
    let signal = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {_=signal_stop.cancelled()=>break,_=interval.tick()=>{if signal_path.exists(){signal_stop.cancel();break;}}}
        }
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(stop.clone().cancelled_owned())
        .await?;
    stop.cancel();
    worker.await?;
    events.await?;
    signal.await?;
    model.await??;
    store.close().await;
    println!("fixture_stopped");
    Ok(())
}
