use super::*;
use axum::{
    body::Body,
    http::{HeaderMap, Request},
};
use std::{path::PathBuf, sync::Mutex};
use tower::ServiceExt;
struct Keys(cowork_secrets::KeyId);
impl cowork_secrets::KeyStore for Keys {
    fn load(
        &self,
        id: &cowork_secrets::KeyId,
    ) -> cowork_secrets::Result<zeroize::Zeroizing<[u8; 32]>> {
        if id != &self.0 {
            return Err(cowork_secrets::Error::Missing);
        }
        Ok(zeroize::Zeroizing::new([19; 32]))
    }
}
struct Fixture {
    state: AppState,
    toolbox: Arc<Toolbox>,
    bridges: Arc<native_mcp::StdioBridges>,
    project: String,
    _temp: tempfile::TempDir,
}
async fn fixture(port: u16) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let store = cowork_storage::Store::sqlite(&temp.path().join("db"))
        .await
        .unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Connectors".into(),
        })
        .await
        .unwrap();
    let key = cowork_secrets::KeyId {
        instance: uuid::Uuid::new_v4(),
        key: uuid::Uuid::new_v4(),
    };
    let runtime = Runtime::new(store, vec![], false)
        .unwrap()
        .with_personal_provider_keys(Arc::new(Keys(key)), key);
    let state = AppState {
        runtime,
        token: Arc::new("fixture-owner".into()),
        native_token: None,
        origins: Arc::new(server_config::origins(port, None).unwrap()),
    };
    #[cfg(windows)]
    let python = {
        let result = std::process::Command::new("python")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .expect("Python test prerequisite");
        assert!(result.status.success());
        PathBuf::from(String::from_utf8(result.stdout).unwrap().trim())
    };
    #[cfg(unix)]
    let python = PathBuf::from("/usr/bin/python3");
    let environment = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.into(), v)))
        .collect();
    let toolbox = Toolbox::open(cowork_toolbox::Config {
        root: temp.path().join("tools"),
        programs: BTreeMap::from([("python".into(), python)]),
        environment,
        github_token: None,
    })
    .unwrap();
    Fixture {
        state,
        toolbox,
        bridges: Arc::new(native_mcp::StdioBridges::default()),
        project: project.id,
        _temp: temp,
    }
}
fn request() -> SaveOwnerConnector {
    SaveOwnerConnector {
        id: id(),
        idempotency_key: "original".into(),
        expected_revision: 0,
        name: "Reviewed webhook".into(),
        note: "Team endpoint".into(),
        enabled: true,
        url: Some("https://hooks.example/private-path?token=synthetic-private-query".into()),
        bearer: Some("synthetic-private-key".into()),
        remove_bearer: false,
        recover_only: false,
    }
}
async fn write(
    f: &Fixture,
    input: SaveOwnerConnector,
) -> std::result::Result<OwnerConnector, Error> {
    save(
        f.state.clone(),
        f.project.clone(),
        f.toolbox.clone(),
        f.bridges.clone(),
        input,
        false,
    )
    .await
    .map(|v| v.0)
    .map_err(|e| e.0)
}
#[tokio::test]
async fn connector_versions_freeze_credentials_recover_original_and_delete_revokes_all() {
    let f = fixture(43821).await;
    let input = request();
    let first = write(&f, input.clone()).await.unwrap();
    let text = serde_json::to_string(&first).unwrap();
    assert!(!text.contains("synthetic-private"));
    assert!(!text.contains("private-path"));
    assert_eq!(first.destination, "https://hooks.example");
    let old_factory = f
        .state
        .runtime
        .store
        .native_mcp_factory(&first.server_id)
        .await
        .unwrap()
        .unwrap();
    let binding = f
        .state
        .runtime
        .store
        .select_mcp_tools(
            &f.project,
            &[McpToolSelection {
                server_id: first.server_id.clone(),
                server_revision: 1,
                tool_name: "send_json".into(),
            }],
        )
        .await
        .unwrap()
        .remove(0);
    let mut recovery = input.clone();
    recovery.url = None;
    recovery.bearer = None;
    recovery.recover_only = true;
    assert_eq!(write(&f, recovery.clone()).await.unwrap(), first);
    let mut changed = input.clone();
    changed.bearer = Some("other-key".into());
    assert!(matches!(write(&f, changed).await, Err(Error::Conflict)));
    let mut edit = input.clone();
    edit.expected_revision = 1;
    edit.idempotency_key = "edit".into();
    edit.url = Some("https://another.example/new-secret-path".into());
    edit.bearer = Some("new-private-key".into());
    let second = write(&f, edit).await.unwrap();
    assert_ne!(first.server_id, second.server_id);
    let old = secrets_for(&f.state, &old_factory).await.unwrap();
    assert_eq!(old.url, input.url.as_ref().unwrap().as_str());
    assert_eq!(old.bearer, "synthetic-private-key");
    assert!(
        f.state
            .runtime
            .store
            .mcp_server(&f.project, &first.server_id)
            .await
            .unwrap()
            .bindings()
            .unwrap()
            .contains(&binding)
    );
    let current = f.state.runtime.store.mcp_servers(&f.project).await.unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, second.server_id);
    assert!(
        !f.state
            .runtime
            .store
            .connector_server_selectable(&first.server_id)
            .await
            .unwrap()
    );
    assert!(
        f.state
            .runtime
            .store
            .configure_mcp_server(
                &f.project,
                &first.server_id,
                1,
                "generic-override",
                &current[0].configuration
            )
            .await
            .is_err()
    );
    assert_eq!(write(&f, recovery.clone()).await.unwrap(), first);
    assert_eq!(
        f.state
            .runtime
            .store
            .owner_connector(&f.project, &first.id)
            .await
            .unwrap(),
        second
    );
    let deleted = f
        .state
        .runtime
        .store
        .delete_connector(
            &f.project,
            &first.id,
            &DeleteOwnerConnector {
                idempotency_key: "delete".into(),
                expected_revision: 2,
            },
            false,
        )
        .await
        .unwrap();
    assert!(deleted.deleted);
    for version in [&first, &second] {
        let factory = f
            .state
            .runtime
            .store
            .native_mcp_factory(&version.server_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!factory.enabled);
        assert!(secrets_for(&f.state, &factory).await.is_err());
    }
    assert_eq!(write(&f, recovery).await.unwrap(), first);
    assert!(
        f.state
            .runtime
            .store
            .owner_connector(&f.project, &first.id)
            .await
            .unwrap()
            .deleted
    );
    f.toolbox.shutdown();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_real_bridge_discovers_exact_catalog_and_sends_to_original_frozen_destination() {
    type Records = Arc<Mutex<Vec<(String, String, Value)>>>;
    async fn sink(
        State(records): State<Records>,
        uri: axum::http::Uri,
        headers: HeaderMap,
        Json(value): Json<Value>,
    ) -> StatusCode {
        records.lock().unwrap().push((
            uri.to_string(),
            headers["authorization"].to_str().unwrap().into(),
            value,
        ));
        StatusCode::OK
    }
    let records: Records = Arc::new(Mutex::new(vec![]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap();
    let sink_stop = tokio_util::sync::CancellationToken::new();
    let sink_token = sink_stop.clone();
    let app = Router::new()
        .route("/private-webhook", post(sink))
        .with_state(records.clone());
    let sink_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(sink_token.cancelled_owned())
            .await
            .unwrap()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let f = fixture(listener.local_addr().unwrap().port()).await;
    let bridge_stop = tokio_util::sync::CancellationToken::new();
    let bridge_token = bridge_stop.clone();
    let app =
        native_mcp::bridge_router(f.toolbox.clone(), f.bridges.clone()).with_state(f.state.clone());
    let bridge_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(bridge_token.cancelled_owned())
            .await
            .unwrap()
    });
    let mut input = request();
    input.url = Some(format!(
        "http://{target}/private-webhook?secret=private-query"
    ));
    let first = write(&f, input.clone()).await.unwrap();
    let factory = f
        .state
        .runtime
        .store
        .native_mcp_factory(&first.server_id)
        .await
        .unwrap()
        .unwrap();
    let token = f
        .state
        .runtime
        .native_mcp_bearer(
            &f.project,
            &factory.server_id,
            &factory.endpoint,
            &factory.credential_reference,
            factory.credential_revision,
        )
        .await
        .unwrap();
    let mut session = cowork_mcp::Session::connect(
        cowork_mcp::Endpoint::new(&factory.endpoint, Some(&token)).unwrap(),
        Default::default(),
    )
    .await
    .unwrap();
    let discovered = session.discover().await.unwrap();
    let expected =
        cowork_mcp::Catalog::from_definitions(catalog(&first.destination, &first.name)).unwrap();
    assert_eq!(discovered.sha256(), expected.sha256());
    assert!(records.lock().unwrap().is_empty());
    let mut edit = input.clone();
    edit.idempotency_key = "changed-url".into();
    edit.expected_revision = 1;
    edit.url = Some("https://never-contact.invalid/secret".into());
    let _second = write(&f, edit).await.unwrap();
    let call = session
        .prepare(
            expected.sha256(),
            "send_json",
            json!({"destination":first.destination,"payload":{"text":"approved exact payload"}}),
        )
        .unwrap();
    let result = session.call(call).await.unwrap();
    assert!(!result.response.to_string().contains("private-query"));
    assert_eq!(
        *records.lock().unwrap(),
        vec![(
            "/private-webhook?secret=private-query".into(),
            "Bearer synthetic-private-key".into(),
            json!({"text":"approved exact payload"})
        )]
    );
    native_mcp::remove_connections(&f.bridges, &f.toolbox, &first.server_id)
        .await
        .unwrap_or_else(|_| panic!("Connector bridge cleanup failed"));
    drop(session);
    bridge_stop.cancel();
    sink_stop.cancel();
    bridge_task.await.unwrap();
    sink_task.await.unwrap();
    f.toolbox.shutdown();
}
#[tokio::test]
async fn connector_owner_api_cannot_use_actor_token_bad_origin_or_native_browser_fallback() {
    let f = fixture(43821).await;
    let app = owner_router(f.toolbox.clone(), f.bridges.clone())
        .layer(middleware::from_fn_with_state(f.state.clone(), authorize))
        .with_state(f.state.clone());
    for (token, origin, expected) in [
        (
            "actor-token",
            "http://127.0.0.1:43821",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "fixture-owner",
            "https://hostile.invalid",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/connectors", f.project))
                    .method("POST")
                    .header("authorization", format!("Bearer {token}"))
                    .header("origin", origin)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let mut state = f.state.clone();
    state.native_token = Some(Arc::new("n".repeat(64)));
    assert!(server_allowed(&state).is_err());
    assert!(
        f.state
            .runtime
            .store
            .owner_connectors(&f.project)
            .await
            .unwrap()
            .is_empty()
    );
    f.toolbox.shutdown();
}

#[tokio::test]
async fn mcp_lifecycle_retries_only_new_handshake_and_never_replays_uncertain_tool_call() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Remote {
        initializations: AtomicUsize,
        calls: AtomicUsize,
    }
    async fn rpc(State(state): State<Arc<Remote>>, Json(request): Json<Value>) -> Response {
        if request["method"] == "initialize"
            && state.initializations.fetch_add(1, Ordering::SeqCst) == 0
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if request.get("id").is_none() {
            return StatusCode::ACCEPTED.into_response();
        }
        let result = match request["method"].as_str() {
            Some("initialize") => {
                json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            Some("tools/list") => json!({"tools":catalog("https://fixture.example","Fixture")}),
            Some("tools/call") => {
                state.calls.fetch_add(1, Ordering::SeqCst);
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            _ => json!({}),
        };
        Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
    }
    let remote = Arc::new(Remote::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let token = stop.clone();
    let app = Router::new()
        .route("/mcp", post(rpc))
        .with_state(remote.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(token.cancelled_owned())
            .await
            .unwrap()
    });
    let f = fixture(43821).await;
    let limits = cowork_mcp::Limits {
        timeout_ms: 1000,
        ..Default::default()
    };
    let mut session = f
        .state
        .runtime
        .connect_owner_mcp_catalog("fixture", &url, None, limits, None, false)
        .await
        .unwrap();
    assert_eq!(remote.initializations.load(Ordering::SeqCst), 2);
    let digest = session.catalog().unwrap().sha256().to_owned();
    let call=session.prepare(&digest,"send_json",json!({"destination":"https://fixture.example","payload":{"text":"one approved message"}})).unwrap();
    let failure = session.call(call).await.err().unwrap();
    assert!(failure.outcome_uncertain);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
    assert_eq!(remote.initializations.load(Ordering::SeqCst), 2);
    let prefs = f
        .state
        .runtime
        .store
        .library_preferences(cowork_storage::library_config::LibraryOwner::Local)
        .await
        .unwrap();
    let mut value = prefs.value;
    value.mcp_auto_reconnect = false;
    value.mcp_verbose_logging = true;
    f.state
        .runtime
        .store
        .library_write_preferences(
            cowork_storage::library_config::LibraryOwner::Local,
            &WritePreferences {
                idempotency_key: "disable-retry".into(),
                expected_revision: prefs.revision,
                value,
            },
        )
        .await
        .unwrap();
    remote.initializations.store(0, Ordering::SeqCst);
    assert!(
        f.state
            .runtime
            .connect_owner_mcp_catalog("fixture", &url, None, limits, None, false)
            .await
            .is_err()
    );
    assert_eq!(remote.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
    stop.cancel();
    task.await.unwrap();
    f.toolbox.shutdown();
}
