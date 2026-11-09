//! Explicitly owned real HTTP/document fixture. No external providers or accounts.
//! --resume keeps the original database, broker, endpoints and engine selection.
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_domain::workflows::WorkflowDefinition;
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

async fn models() -> Json<Value> {
    Json(json!({"data":[{"id":"workflow-documents"}]}))
}
async fn chat(
    State(directory): State<Arc<PathBuf>>,
    Json(body): Json<Value>,
) -> anyhow::Result<String, axum::http::StatusCode> {
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("model-requests.jsonl"))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    writeln!(log, "{body}").map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let messages = body["messages"]
        .as_array()
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let tools = messages.iter().any(|message| message["role"] == "tool");
    let (delta, finish) = if tools {
        (
            json!({"content":"Single document task complete. The retained report is ready."}),
            "stop",
        )
    } else {
        (
            json!({"tool_calls":[{"index":0,"id":"create-report","type":"function","function":{"name":"document_write","arguments":json!({"operation":"create","output":{"path":"report.docx"},"spec":{"format":"docx","markdown":"# Confirmed workflow report\n\nCreated once from an explicitly confirmed document plan."}}).to_string()}}]}),
            "tool_calls",
        )
    };
    let response = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":{"prompt_tokens":20,"completion_tokens":8}});
    Ok(format!("data: {response}\n\ndata: [DONE]\n\n"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Pass an absolute owned fixture directory"))?,
    );
    let resume = std::env::args().nth(2).as_deref() == Some("--resume");
    if !directory.is_absolute() {
        anyhow::bail!("Fixture directory must be absolute");
    }
    if resume {
        if !directory.join("manifest.json").is_file() || directory.join("STOP").exists() {
            anyhow::bail!("Missing original owned fixture");
        }
    } else {
        std::fs::create_dir(&directory)?;
    }
    let manifest: Value = if resume {
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?
    } else {
        Value::Null
    };
    let port = |name: &str| -> anyhow::Result<u16> {
        if resume {
            Ok(manifest[name]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Missing original endpoint"))?
                .try_into()?)
        } else {
            Ok(0)
        }
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port("port")?)).await?;
    let model_listener = tokio::net::TcpListener::bind(("127.0.0.1", port("model_port")?)).await?;
    let address = listener.local_addr()?;
    let model_address = model_listener.local_addr()?;
    let store = Store::sqlite(&directory.join("documents.sqlite3")).await?;
    let definition: WorkflowDefinition = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow-documents.json"
    ))?;
    let (project, order, workflow): (Project, Order, Value) = if resume {
        let project: Project = serde_json::from_value(manifest["project"].clone())?;
        let original: Order = serde_json::from_value(manifest["order"].clone())?;
        let order = store.order(&original.id).await?;
        if order.project_id != project.id {
            anyhow::bail!("Fixture project mismatch");
        }
        (project, order, manifest["workflow"].clone())
    } else {
        let project = store
            .create_project(CreateProject {
                name: "Owned single document qualification".into(),
            })
            .await?;
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Confirmed report".into(),
            })
            .await?;
        (project, order, Value::Null)
    };
    let profile: ProviderProfile = serde_json::from_value(
        json!({"id":"model","name":"Explicit local document fixture","kind":"openai_compatible","endpoint":format!("http://{model_address}"),"model":"workflow-documents","revision":1,"context_tokens":32768,"credential_ready":true,"credential_required":false,"capabilities":{"image_input":true}}),
    )?;
    if !resume {
        store.insert_provider(&profile).await?;
    }
    let programs = BTreeMap::from([(
        "python".into(),
        PathBuf::from(
            std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON")
                .ok_or_else(|| anyhow::anyhow!("Explicit real document Python required"))?,
        ),
    )]);
    let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
        .collect();
    let toolbox = cowork_toolbox::Toolbox::open(cowork_toolbox::Config {
        root: directory.join("toolbox"),
        programs,
        environment,
        github_token: None,
    })?;
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    while !toolbox.availability().documents_available {
        if tokio::time::Instant::now() >= until {
            anyhow::bail!("Real document engine did not become ready");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let runtime = Runtime::new(
        store.clone(),
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        false,
    )?
    .with_file_broker(cowork_broker::FileBroker::open(
        &directory.join("broker"),
        vec![],
    )?)
    .with_toolbox(toolbox);
    runtime.workspace(&order.id).await?;
    if !resume {
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(
                &json!({"project":project,"order":order,"workflow":workflow,"port":address.port(),"model_port":model_address.port()}),
            )?,
        )?;
    }
    let token = "owned-single-document-fixture";
    let stop = CancellationToken::new();
    let model_stop = stop.clone();
    let model_directory = Arc::new(directory.clone());
    let model = tokio::spawn(async move {
        axum::serve(
            model_listener,
            Router::new()
                .route("/models", get(models))
                .route("/chat/completions", post(chat))
                .with_state(model_directory),
        )
        .with_graceful_shutdown(model_stop.cancelled_owned())
        .await
    });
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new(token.into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5195".into()]),
        },
        None,
    );
    std::fs::write(
        directory.join("ready.json"),
        serde_json::to_vec_pretty(
            &json!({"pid":std::process::id(),"base":format!("http://{address}"),"token":token,"project":project,"order":order,"workflow":workflow,"definition":definition,"workers":1,"providers":1,"resumed":resume}),
        )?,
    )?;
    println!("fixture_ready");
    let worker = tokio::spawn(runtime.serve(1, stop.clone()));
    let signal_stop = stop.clone();
    let stop_path = directory.join("STOP");
    let signal = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {_=signal_stop.cancelled()=>break,_=timer.tick()=>if stop_path.exists(){signal_stop.cancel();break;}}
        }
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(stop.clone().cancelled_owned())
        .await?;
    stop.cancel();
    worker.await?;
    signal.await?;
    model.await??;
    store.close().await;
    println!("fixture_stopped");
    Ok(())
}
