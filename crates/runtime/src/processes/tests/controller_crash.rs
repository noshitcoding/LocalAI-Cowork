//! Loopback signed fixture only. The controller is a separate owned OS process;
//! its hard exit cannot run the Runtime cancellation/drop path in this parent.
use super::*;
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use std::io::{Read, Write};

#[derive(Serialize, Deserialize)]
struct ControllerInput {
    directory: std::path::PathBuf,
    endpoint: String,
    binding: Binding,
    control_pkcs8: Vec<u8>,
    providers: Vec<ConfiguredProvider>,
    run_id: String,
    worker_token: String,
    tool: ToolCall,
}

struct HttpFixtureAccess {
    endpoint: String,
    client: reqwest::Client,
}
impl HttpFixtureAccess {
    async fn request<T: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        request: &T,
    ) -> Result<R> {
        self.client
            .post(format!("{}{path}", self.endpoint))
            .json(request)
            .send()
            .await
            .map_err(|_| Error::UncertainEffect)?
            .error_for_status()
            .map_err(|_| Error::UncertainEffect)?
            .json()
            .await
            .map_err(|_| Error::UncertainEffect)
    }
}
#[async_trait::async_trait]
impl ProcessRunnerAccess for HttpFixtureAccess {
    async fn observe(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<Signed<EnvironmentObservation>> {
        self.request("/observe", reference).await
    }
    async fn fence(&self, request: Signed<FenceRequest>) -> Result<()> {
        self.request("/fence", &request).await
    }
    async fn renew_execution_lease(&self, request: Signed<ExecutionLeaseRenewal>) -> Result<()> {
        self.request("/lease", &request).await
    }
    async fn launch(
        &self,
        request: Signed<LaunchRequest>,
        _stop: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        // The server deliberately owns cancellation independently. Hard controller
        // death drops this socket; only the durable lease can stop its real child.
        self.request("/launch", &request).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_fixture_process() {
    if std::env::var_os("COWORK_TEST_HARD_CONTROLLER").is_none() {
        return;
    }
    let mut bytes = vec![];
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    let input: ControllerInput = serde_json::from_slice(&bytes).unwrap();
    zeroize::Zeroize::zeroize(&mut bytes);
    let key = SigningKey::from_pkcs8(&input.control_pkcs8).unwrap();
    let store = Store::sqlite(&input.directory.join("product.sqlite3"))
        .await
        .unwrap();
    let files = cowork_broker::FileBroker::open(&input.directory.join("files"), vec![]).unwrap();
    let access = Arc::new(HttpFixtureAccess {
        endpoint: input.endpoint,
        client: reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(90))
            .build()
            .unwrap(),
    });
    let runtime = Runtime::new(store, input.providers, true)
        .unwrap()
        .with_file_broker(files)
        .with_registered_process_runner(
            ProcessRunnerConfig::new(input.binding, key, access).unwrap(),
        )
        .await
        .unwrap();
    runtime
        .invoke_process_tool(
            &input.run_id,
            &input.worker_token,
            &input.tool,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    panic!("controller fixture must be killed before its held process finishes");
}

#[derive(Clone)]
struct HttpState {
    runner: Arc<FixtureRunner>,
    end: CancellationToken,
}
async fn observe(
    State(s): State<HttpState>,
    Json(reference): Json<ExecutionEnvironmentRef>,
) -> std::result::Result<Json<Signed<EnvironmentObservation>>, StatusCode> {
    s.runner
        .observe(&reference)
        .await
        .map(Json)
        .map_err(|_| StatusCode::FORBIDDEN)
}
async fn fence(
    State(s): State<HttpState>,
    Json(request): Json<Signed<FenceRequest>>,
) -> std::result::Result<Json<()>, StatusCode> {
    s.runner
        .fence(request)
        .await
        .map(Json)
        .map_err(|_| StatusCode::FORBIDDEN)
}
async fn lease(
    State(s): State<HttpState>,
    Json(request): Json<Signed<ExecutionLeaseRenewal>>,
) -> std::result::Result<Json<()>, StatusCode> {
    s.runner
        .renew_execution_lease(request)
        .await
        .map(Json)
        .map_err(|_| StatusCode::FORBIDDEN)
}
async fn launch(
    State(s): State<HttpState>,
    Json(request): Json<Signed<LaunchRequest>>,
) -> std::result::Result<Json<Signed<Receipt>>, StatusCode> {
    tokio::select! {
        _ = s.end.cancelled() => Err(StatusCode::SERVICE_UNAVAILABLE),
        result = s.runner.launch(request, CancellationToken::new()) =>
            result.map(Json).map_err(|_| StatusCode::FORBIDDEN),
    }
}

struct OwnedController(std::process::Child);
impl Drop for OwnedController {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct ServerEnd(CancellationToken);
impl Drop for ServerEnd {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hard_controller_exit_expires_authenticated_lease_and_reaps_actual_owned_child() {
    let f = setup().await;
    f.runner.mode.store(5, Ordering::Release);
    let (run, token, mut tool) = pending(&f, true).await;
    tool.arguments = json!({"program":"fixture","arguments":["hold"],"timeout_seconds":45});
    f.runtime
        .store
        .mutate(&run, Some(&token), |stored| {
            stored.checkpoint.pending_tools = vec![tool.clone()];
            stored.lease.as_mut().unwrap().until = now() + 5_000;
            Ok(Change::default())
        })
        .await
        .unwrap();
    let before = f.runtime.store.stored_run(&run).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server_end = CancellationToken::new();
    let _server_guard = ServerEnd(server_end.clone());
    let state = HttpState {
        runner: f.runner.clone(),
        end: server_end.clone(),
    };
    let server_stop = server_end.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/observe", post(observe))
                .route("/fence", post(fence))
                .route("/lease", post(lease))
                .route("/launch", post(launch))
                .with_state(state),
        )
        .with_graceful_shutdown(server_stop.cancelled_owned())
        .await
        .unwrap();
    });
    let input = ControllerInput {
        directory: f._dir.path().into(),
        endpoint,
        binding: f.runtime.process_runner.as_ref().unwrap().binding.clone(),
        control_pkcs8: f.control_pkcs8.clone(),
        providers: f.runtime.providers.read().await.clone(),
        run_id: run.clone(),
        worker_token: token.clone(),
        tool: tool.clone(),
    };
    let mut controller = OwnedController(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "processes::tests::controller_crash::controller_fixture_process",
                "--nocapture",
            ])
            .env("COWORK_TEST_HARD_CONTROLLER", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    {
        let mut stdin = controller.0.stdin.take().unwrap();
        serde_json::to_writer(&mut stdin, &input).unwrap();
        stdin.flush().unwrap();
    }
    tokio::time::timeout(Duration::from_secs(8), async {
        while f.runner.renewals.load(Ordering::Acquire) == 0 {
            assert!(
                controller.0.try_wait().unwrap().is_none(),
                "controller exited before renewal"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(f.runner.directory.join("effect.txt")).unwrap(),
        b"once"
    );
    let renewal = f
        .runner
        .renewal_requests
        .lock()
        .await
        .last()
        .unwrap()
        .payload
        .clone();
    assert_eq!(renewal.run_id, run);
    assert!(renewal.expires_at > now());
    assert!(controller.0.try_wait().unwrap().is_none());
    controller.0.kill().unwrap();
    let exit = controller.0.wait().unwrap(); // retained native child handle; hard exit, no Dropguard execution
    assert!(!exit.success());
    let count = f.runner.renewals.load(Ordering::Acquire);
    let supervisor = f.runner.supervisors.lock().await.pop().unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(22), supervisor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(f.runner.renewals.load(Ordering::Acquire), count);
    assert!(now() >= renewal.expires_at);
    assert!(now() < receipt.payload.request.execution_deadline);
    let ExecutionState::Completed { result } = &receipt.payload.state else {
        panic!("adapter physical termination receipt required");
    };
    assert_eq!(result.termination, ProcessTermination::Canceled);
    assert!(result.process_id.is_some());
    assert!(result.exit_code.is_some() || result.signal.is_some());
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    let after = f.runtime.store.stored_run(&run).await.unwrap();
    assert_eq!(
        canonical_hash(&after.record.spec),
        canonical_hash(&before.record.spec)
    );
    assert_eq!(after.record.started_at, before.record.started_at);
    assert_eq!(after.checkpoint.in_flight, Some(tool.clone()));
    assert!(matches!(
        f.runtime.store.process_attempts(&run).await.unwrap()[0].state,
        ProcessAttemptState::LaunchCommitted
    ));
    // A different currently valid worker is supplied by this private fixture only.
    // This is not a product Resume API or permission to revive the old controller.
    // Its fresh lease must reach the retained-attempt guard, not fail merely because
    // the crashed worker's token has expired.
    let replacement = id();
    f.runtime
        .store
        .mutate(&run, None, |stored| {
            let lease = stored.lease.as_mut().unwrap();
            lease.token = replacement.clone();
            lease.until = now() + 15_000;
            Ok(Change::default())
        })
        .await
        .unwrap();
    f.runtime
        .store
        .stored_run(&run)
        .await
        .unwrap()
        .assert_lease(&replacement, now())
        .unwrap();
    assert!(matches!(
        f.runtime
            .invoke_process_tool(&run, &replacement, &tool, &CancellationToken::new())
            .await,
        Err(Error::UncertainEffect)
    ));
    assert_eq!(f.runner.launches.load(Ordering::Acquire), 1);
    server_end.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
