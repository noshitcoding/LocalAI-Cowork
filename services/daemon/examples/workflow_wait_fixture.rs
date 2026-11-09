//! Owned, single-worker real HTTP fixture. --resume reopens exactly the same
//! private SQLite/broker state and endpoint; no providers or external services.
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_domain::workflows::*;
use cowork_runtime::Runtime;
use cowork_storage::Store;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Pass an absolute owned fixture directory"))?,
    );
    let resume = std::env::args().nth(2).as_deref() == Some("--resume");
    if !directory.is_absolute() {
        anyhow::bail!("Fixture path must be absolute");
    }
    if !resume {
        std::fs::create_dir(&directory)?;
    } else if !directory.join("manifest.json").is_file() || directory.join("STOP").exists() {
        anyhow::bail!("Resume requires the original owned fixture without a STOP request");
    }
    let store = Store::sqlite(&directory.join("waits.sqlite3")).await?;
    let (project, order, fast_order, port): (Project, Order, Order, u16) = if resume {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
        let project: Project = serde_json::from_value(manifest["project"].clone())?;
        let original: Order = serde_json::from_value(manifest["order"].clone())?;
        let fast: Order = serde_json::from_value(manifest["fast_order"].clone())?;
        let order = store.order(&original.id).await?;
        let fast_order = store.order(&fast.id).await?;
        if order.project_id != project.id || fast_order.project_id != project.id {
            anyhow::bail!("Fixture manifest does not match its database");
        }
        (
            project,
            order,
            fast_order,
            manifest["port"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Missing original endpoint"))?
                .try_into()?,
        )
    } else {
        let project = store
            .create_project(CreateProject {
                name: "Durable timer browser qualification".into(),
            })
            .await?;
        let order = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Scheduled continuation".into(),
            })
            .await?;
        let fast = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Independent work".into(),
            })
            .await?;
        (project, order, fast, 0)
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let address = listener.local_addr()?;
    if !resume {
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(
                &json!({"format_version":1,"project":project,"order":order,"fast_order":fast_order,"port":address.port()}),
            )?,
        )?;
    }
    let definition: WorkflowDefinition =
        serde_json::from_str(include_str!("../../../tests/fixtures/workflow-wait.json"))?;
    let mut fast_definition = definition.clone();
    fast_definition.name = "Independent native work".into();
    fast_definition.controls.remove("wait");
    fast_definition
        .controls
        .get_mut("after")
        .unwrap()
        .dependencies
        .clear();
    let data = fast_definition.dataflow.as_mut().unwrap();
    data.input = WorkflowDataType::Object {
        fields: Default::default(),
    };
    data.steps.remove("wait");
    data.steps
        .get_mut("after")
        .unwrap()
        .inputs
        .get_mut("scheduled_at")
        .unwrap()
        .value = WorkflowDataSource::Literal { value: json!(7) };
    let runtime = Runtime::new(store.clone(), vec![], false)?.with_file_broker(
        cowork_broker::FileBroker::open(&directory.join("broker"), vec![])?,
    );
    let token = "owned-workflow-wait-fixture";
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new(token.into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5191".into()]),
        },
        None,
    );
    std::fs::write(
        directory.join("ready.json"),
        serde_json::to_vec_pretty(
            &json!({"pid":std::process::id(),"base":format!("http://{address}"),"token":token,"project":project,"order":order,"fast_order":fast_order,"definition":definition,"fast_definition":fast_definition,"workers":1,"providers":0,"resumed":resume}),
        )?,
    )?;
    println!("fixture_ready");
    let stop = CancellationToken::new();
    let worker = tokio::spawn(runtime.clone().serve(1, stop.clone()));
    let signal_stop = stop.clone();
    let stop_path = directory.join("STOP");
    let signal = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {_=signal_stop.cancelled()=>break,_=timer.tick()=>{if stop_path.exists(){signal_stop.cancel();break;}}}
        }
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(stop.clone().cancelled_owned())
        .await?;
    stop.cancel();
    worker.await?;
    signal.await?;
    store.close().await;
    println!("fixture_stopped");
    Ok(())
}
