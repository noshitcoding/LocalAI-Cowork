//! Explicit native update admission; no process signals or arbitrary PID input.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const PATH: &str = "/api/v2/native/lifecycle";
const SHUTDOWN: &str = "/api/v2/native/lifecycle/shutdown";

#[derive(Clone)]
pub struct LifecycleControl {
    admission: Arc<RwLock<()>>,
    closing: Arc<AtomicBool>,
    instance: Arc<String>,
    stop: CancellationToken,
    terminals: Option<Arc<crate::owner_terminal::TerminalService>>,
}
impl LifecycleControl {
    pub fn new(stop: CancellationToken) -> Self {
        Self {
            admission: Arc::new(RwLock::new(())),
            closing: Arc::new(AtomicBool::new(false)),
            instance: Arc::new(id()),
            stop,
            terminals: None,
        }
    }
    pub fn admission(&self) -> Arc<RwLock<()>> {
        self.admission.clone()
    }
    pub fn with_terminals(
        mut self,
        terminals: Arc<crate::owner_terminal::TerminalService>,
    ) -> Self {
        self.terminals = Some(terminals);
        self
    }
}
#[derive(Clone)]
struct LifecycleState {
    app: AppState,
    control: LifecycleControl,
    local: bool,
}

#[derive(Serialize)]
struct Snapshot {
    format: u32,
    product: &'static str,
    version: &'static str,
    schema_version: u32,
    pid: u32,
    instance_id: String,
    closing: bool,
    active_owner_terminals: usize,
}
fn snapshot(control: &LifecycleControl) -> Snapshot {
    Snapshot {
        format: 1,
        product: "LocalAI Cowork V2",
        version: env!("CARGO_PKG_VERSION"),
        schema_version: SCHEMA_VERSION,
        pid: std::process::id(),
        instance_id: (*control.instance).clone(),
        closing: control.closing.load(Ordering::SeqCst),
        active_owner_terminals: control.terminals.as_ref().map_or(0, |t| t.active_count()),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShutdownInput {
    version: String,
    schema_version: u32,
    pid: u32,
    instance_id: String,
}

pub fn attach(app: Router, state: AppState, control: LifecycleControl, local: bool) -> Router {
    let lifecycle = Router::new()
        .route(PATH, get(status))
        .route(SHUTDOWN, post(shutdown))
        .with_state(LifecycleState {
            app: state.clone(),
            control: control.clone(),
            local,
        })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            native_folders::authorize_native,
        ))
        .layer(middleware::from_fn_with_state(state, authorize))
        .layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn(response_headers));
    app.merge(lifecycle)
        .layer(middleware::from_fn_with_state(control.clone(), admission))
        .layer(middleware::from_fn_with_state(control, drain_streams))
}
async fn drain_streams(
    State(control): State<LifecycleControl>,
    request: Request,
    next: Next,
) -> Response {
    let cancel_body = request.uri().path().starts_with("/api/")
        && request.uri().path() != PATH
        && request.uri().path() != SHUTDOWN;
    let response = next.run(request).await;
    if !cancel_body {
        return response;
    }
    // Open SSE responses must not keep graceful shutdown alive indefinitely.
    use futures_util::StreamExt;
    let (parts, body) = response.into_parts();
    // Keep finite-download integrity metadata. If shutdown truncates the stream,
    // the receiver rejects the incomplete declared length/digest as intended.
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), control.stop),
        |(mut body, stop)| async move {
            tokio::select! { biased; _ = stop.cancelled() => None,
            item = body.next() => item.map(|item| (item, (body, stop))) }
        },
    );
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}
async fn admission(
    State(control): State<LifecycleControl>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/api/") || path == PATH || path == SHUTDOWN {
        return next.run(request).await;
    }
    if control.closing.load(Ordering::SeqCst) {
        return ApiFailure(Error::WorkspaceBusy).into_response();
    }
    let _guard = control.admission.read().await;
    if control.closing.load(Ordering::SeqCst) {
        return ApiFailure(Error::WorkspaceBusy).into_response();
    }
    next.run(request).await
}
async fn status(State(s): State<LifecycleState>) -> Answer<Snapshot> {
    if !s.local || !s.app.runtime.store.is_local_sqlite() {
        return Err(Error::Forbidden.into());
    }
    Ok(Json(snapshot(&s.control)))
}
async fn shutdown(State(s): State<LifecycleState>, body: Input<ShutdownInput>) -> Answer<Snapshot> {
    if !s.local || !s.app.runtime.store.is_local_sqlite() {
        return Err(Error::Forbidden.into());
    }
    let input = input(body)?;
    if input.version != env!("CARGO_PKG_VERSION")
        || input.schema_version != SCHEMA_VERSION
        || input.pid != std::process::id()
        || input.instance_id != *s.control.instance
    {
        return Err(Error::Conflict.into());
    }
    let _guard = s
        .control
        .admission
        .try_write()
        .map_err(|_| Error::WorkspaceBusy)?;
    if s.control.closing.load(Ordering::SeqCst) {
        return Err(Error::Conflict.into());
    }
    s.app.runtime.store.native_update_preflight().await?;
    if s.control
        .terminals
        .as_ref()
        .is_some_and(|t| t.active_count() > 0)
    {
        return Err(Error::WorkspaceBusy.into());
    }
    // The admission writer excludes API writes and schedule creation until closing
    // is published. Existing supervisors are joined by main before process exit.
    s.control.closing.store(true, Ordering::SeqCst);
    s.control.stop.cancel();
    Ok(Json(snapshot(&s.control)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn finite_verified_download_keeps_length_through_cancellable_lifecycle() {
        use tower::ServiceExt;
        let control = LifecycleControl::new(CancellationToken::new());
        let app = Router::new()
            .route(
                "/api/v2/file",
                get(|| async {
                    (
                        [
                            (header::CONTENT_LENGTH, "3"),
                            (header::CONTENT_TYPE, "application/octet-stream"),
                        ],
                        "abc",
                    )
                }),
            )
            .layer(middleware::from_fn_with_state(control, drain_streams));
        let app = app.layer(middleware::from_fn(response_headers));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v2/file")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "3");
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(csp.contains("style-src 'self' 'unsafe-inline';"));
        assert!(csp.contains("script-src 'self';"));
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .as_ref(),
            b"abc"
        );
    }
    #[tokio::test]
    async fn owner_pty_blocks_native_update_until_explicit_stop() {
        let (temp, terminals, program) = crate::owner_terminal::tests::service();
        let store = cowork_storage::Store::sqlite(&temp.path().join("db.sqlite3"))
            .await
            .unwrap();
        let runtime = Runtime::new(store.clone(), vec![], true).unwrap();
        let stop = CancellationToken::new();
        let control = LifecycleControl::new(stop.clone()).with_terminals(terminals.clone());
        let mut launch = OwnerTerminalLaunch {
            idempotency_key: "update-check".into(),
            program,
            rows: 24,
            cols: 80,
            approval_sha256: None,
        };
        launch.approval_sha256 = Some(
            terminals
                .preview(crate::owner_terminal::tests::TEST_ORDER, &launch)
                .unwrap()
                .approval_sha256,
        );
        let session = terminals
            .launch(crate::owner_terminal::tests::TEST_ORDER, &launch)
            .unwrap();
        let state = LifecycleState {
            app: AppState {
                runtime,
                token: Arc::new("owner".into()),
                native_token: Some(Arc::new("native".into())),
                origins: Arc::new(vec![]),
            },
            control: control.clone(),
            local: true,
        };
        assert_eq!(snapshot(&control).active_owner_terminals, 1);
        let input = || {
            Ok(Json(ShutdownInput {
                version: env!("CARGO_PKG_VERSION").into(),
                schema_version: SCHEMA_VERSION,
                pid: std::process::id(),
                instance_id: (*control.instance).clone(),
            }))
        };
        assert!(matches!(
            shutdown(State(state.clone()), input()).await,
            Err(ApiFailure(Error::WorkspaceBusy))
        ));
        assert!(!stop.is_cancelled());
        terminals
            .stop(crate::owner_terminal::tests::TEST_ORDER, &session.id)
            .unwrap();
        assert_eq!(snapshot(&control).active_owner_terminals, 0);
        assert!(
            shutdown(State(state), input())
                .await
                .ok()
                .expect("idle test lifecycle must close")
                .0
                .closing
        );
        assert!(stop.is_cancelled());
        store.close().await;
    }
}
