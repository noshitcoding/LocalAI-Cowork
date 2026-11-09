//! Isolated acceptance peer for the actual browser -> native host -> OS IPC ->
//! approved toolbox path. Only stdin commands from the test operator are used.
use cowork_contracts::{BrowserCommand, BrowserEngine, BrowserMode, CreateBrowserSession};
use cowork_toolbox::{Action, Config, Request, Toolbox, browser_bridge};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("private fixture root required")?,
    );
    let service = Toolbox::open(Config {
        root: root.join("tools"),
        programs: BTreeMap::from([("browser_host".into(), std::env::current_exe()?)]),
        environment: BTreeMap::new(),
        github_token: None,
    })?;
    service.start_browser_bridge()?;
    let label = root
        .file_name()
        .ok_or("fixture label required")?
        .to_string_lossy();
    let instance = browser_bridge::instances()?
        .into_iter()
        .find(|v| v["label"] == label.as_ref())
        .ok_or("fixture instance not found")?;
    println!("{}", json!({"instance_id":instance["id"]}));
    io::stdout().flush()?;
    let order = "00000000-0000-4000-8000-000000000001";
    let project = "00000000-0000-4000-8000-000000000002";
    let mut session = None;
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line?)?;
        let response = (|| -> Result<Value, Box<dyn std::error::Error>> {
            match input["operation"].as_str() {
                Some("connections") => Ok(json!({"connections":service.browser_connections()})),
                Some("create") => {
                    let engine: BrowserEngine = serde_json::from_value(input["engine"].clone())?;
                    let created = service.create_browser_v2(
                        project,
                        order,
                        CreateBrowserSession {
                            engine,
                            mode: BrowserMode::Extension,
                            connection_id: Some(
                                input["connection_id"]
                                    .as_str()
                                    .ok_or("connection required")?
                                    .into(),
                            ),
                        },
                    )?;
                    session = Some(created.id.clone());
                    Ok(json!({"session":created}))
                }
                Some("action") => {
                    let command: BrowserCommand = serde_json::from_value(input["command"].clone())?;
                    let mut request = Request {
                        idempotency_key: uuid::Uuid::new_v4().to_string(),
                        action: Action::Browser {
                            session_id: session.clone().ok_or("session required")?,
                            operation: "control".into(),
                            url: None,
                            selector: None,
                            text: Some(serde_json::to_string(&command)?),
                        },
                        timeout_seconds: 45,
                        output_bytes: 1_048_576,
                        approval_sha256: None,
                    };
                    request.approval_sha256 =
                        Some(service.preview(order, &request)?.request_sha256);
                    let job = service.submit(order, request.clone())?;
                    let deadline = Instant::now() + Duration::from_secs(50);
                    loop {
                        let state = service.status(order, &job.id)?;
                        if state.state != "running" {
                            if state.state != "completed" {
                                return Err("native browser action did not complete".into());
                            }
                            let result: Value = serde_json::from_slice(
                                &state.outcome.ok_or("outcome required")?.stdout.bytes,
                            )?;
                            if service.submit(order, request)?.id != job.id {
                                return Err("request replayed".into());
                            }
                            return Ok(json!({"result":result}));
                        }
                        if Instant::now() > deadline {
                            return Err("native browser action timeout".into());
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                Some("close") => {
                    if let Some(id) = session.take() {
                        service.close_browser_session(order, &id)?;
                    }
                    Ok(json!({"closed":true}))
                }
                _ => Err("unknown fixture command".into()),
            }
        })();
        match response {
            Ok(value) => println!("{value}"),
            Err(error) => println!("{}", json!({"error":error.to_string()})),
        };
        io::stdout().flush()?;
    }
    service.close_browser_sessions_for_order(order);
    service.shutdown();
    Ok(())
}
