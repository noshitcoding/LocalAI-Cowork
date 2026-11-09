//! Explicit, isolated browser-qualification fixture. Creates a new directory and
//! a loopback API with a synthetic provider; never loads a user's daemon profile.
use cowork_broker::{FileBroker, NativeFolderSelection};
use cowork_contracts::*;
use cowork_daemon::{AppState, router};
use cowork_runtime::{ConfiguredProvider, Runtime};
use cowork_storage::Store;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Pass a new fixture directory"))?,
    );
    if !directory.is_absolute() {
        anyhow::bail!("Fixture directory must be absolute");
    }
    std::fs::create_dir(&directory)?;
    let inbox = directory.join("inbox");
    std::fs::create_dir(&inbox)?;
    std::fs::write(
        inbox.join("existing.pdf"),
        b"Existing qualification fixture; not a real invoice",
    )?;
    let store = Store::sqlite(&directory.join("data.sqlite3")).await?;
    let project = store
        .create_project(CreateProject {
            name: "File watch qualification".into(),
        })
        .await?;
    let order = store
        .create_order(CreateOrder {
            project_id: project.id.clone(),
            title: "Observed files".into(),
        })
        .await?;
    let broker = FileBroker::open(&directory.join("broker"), vec![])?;
    let folder = broker
        .register_native_folder(&NativeFolderSelection {
            idempotency_key: "fixture-folder".into(),
            project_id: project.id.clone(),
            name: "Invoice inbox".into(),
            path: inbox.clone(),
            expected_resource_id: cowork_broker::inspect_native_folder(&inbox)?.1,
            writable: false,
        })?
        .grant;
    let profile: ProviderProfile = serde_json::from_value(
        json!({"id":"fixture","name":"Deterministic qualification provider","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true}),
    )?;
    store.insert_provider(&profile).await?;
    let task=store.write_workspace_task(&project.id,None,&serde_json::from_value(json!({"expected_revision":0,"idempotency_key":"fixture-task","value":{"name":"Check incoming invoices","description":"Isolated fixture with synthetic provider","order_id":order.id,"prompt":"Summarize the observed file metadata. This is a synthetic qualification task.","expected_output":"A deterministic fixture result","execution":{"kind":"model","provider_id":"fixture","provider_revision":1},"access_mode":"autonomous","folders":[folder],"budgets":Budgets::default()}}))?).await?;
    let runtime = Runtime::new(
        store.clone(),
        vec![ConfiguredProvider {
            profile,
            credential_env: None,
        }],
        true,
    )?
    .with_file_broker(broker);
    let token = "automation-file-watch-fixture-only";
    let app = router(
        AppState {
            runtime: runtime.clone(),
            token: Arc::new(token.into()),
            native_token: None,
            origins: Arc::new(vec!["http://127.0.0.1:5190".into()]),
        },
        None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let ready = json!({"base":format!("http://{}",listener.local_addr()?),"token":token,"project":project,"task":task,"inbox":inbox});
    std::fs::write(
        directory.join("ready.json"),
        serde_json::to_vec_pretty(&ready)?,
    )?;
    println!("fixture_ready");
    let stop = CancellationToken::new();
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
    store.close().await;
    println!("fixture_stopped");
    Ok(())
}
