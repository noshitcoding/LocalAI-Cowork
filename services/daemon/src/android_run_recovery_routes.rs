//! Public checkpoint reads and current server-agent-only explicit continuation.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/runs/{run}/checkpoints", get(checkpoints))
        .route("/runs/{run}/checkpoints/page", get(page))
        .route(
            "/runs/{run}/checkpoints/{checkpoint}/state/{page}",
            get(bytes),
        )
        .route("/runs/{run}/waiting", get(waiting))
        .route(
            "/runs/{run}/resume",
            post(resume).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .layer(middleware::from_fn(no_store))
}
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response
}
fn failure(error: Error) -> Response {
    let (status, code) = match error {
        Error::Validation(ref message) if message.starts_with("checkpoint_page_required:") => {
            (StatusCode::PAYLOAD_TOO_LARGE, "checkpoint_page_required")
        }
        Error::Validation(_) => (StatusCode::BAD_REQUEST, "invalid_run_recovery_request"),
        Error::Forbidden | Error::NotFound => (StatusCode::NOT_FOUND, "run_recovery_unavailable"),
        Error::Conflict | Error::UncertainEffect => {
            (StatusCode::CONFLICT, "private_resume_original_unresolved")
        }
        Error::BudgetExceeded => (StatusCode::TOO_MANY_REQUESTS, "checkpoint_storage_limit"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "run_recovery_unavailable"),
    };
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"error":code,"message":code})),
    )
        .into_response()
}
type RecoveryAnswer = std::result::Result<Json<Value>, Response>;
async fn checkpoints(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
) -> RecoveryAnswer {
    match s
        .service
        .store()
        .android_run_checkpoints(&actor, run, s.keys.as_ref(), s.key_id)
        .await
    {
        Ok(value) => Ok(Json(json!(value))),
        Err(Error::NotFound) => {
            s.service
                .store()
                .android_run_request(&actor, run)
                .await
                .map_err(failure)?;
            Ok(Json(json!([])))
        }
        Err(error) => Err(failure(error)),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    #[serde(default)]
    after_sequence: i64,
    #[serde(default = "page_limit")]
    limit: u32,
}
fn page_limit() -> u32 {
    10
}
async fn page(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> RecoveryAnswer {
    if query.after_sequence < 0 || query.limit == 0 || query.limit > 20 {
        return Err(failure(invalid()));
    }
    match s
        .service
        .store()
        .android_run_checkpoint_page(
            &actor,
            run,
            query.after_sequence,
            query.limit,
            s.keys.as_ref(),
            s.key_id,
        )
        .await
    {
        Ok(value) => Ok(Json(json!(value))),
        Err(Error::NotFound) => {
            s.service
                .store()
                .android_run_request(&actor, run)
                .await
                .map_err(failure)?;
            Ok(Json(json!(AndroidRunCheckpointPage {
                run_id: run,
                entries: vec![],
                next_sequence: None
            })))
        }
        Err(error) => Err(failure(error)),
    }
}
async fn bytes(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path((run, checkpoint, page)): Path<(Uuid, Uuid, u32)>,
) -> std::result::Result<Response, Response> {
    let value = s
        .service
        .store()
        .android_run_checkpoint_bytes(&actor, run, checkpoint, page, s.keys.as_ref())
        .await
        .map_err(failure)?;
    if value.len() > 1024 * 1024 {
        return Err(failure(Error::BudgetExceeded));
    }
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        value.to_vec(),
    )
        .into_response())
}
async fn waiting(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
) -> RecoveryAnswer {
    match s
        .service
        .store()
        .android_run_waiting(&actor, run, s.devices.private_recovery_observer())
        .await
    {
        Ok(value) => Ok(Json(json!(value))),
        Err(Error::NotFound) => {
            s.service
                .store()
                .android_run_request(&actor, run)
                .await
                .map_err(failure)?;
            Ok(Json(json!(AndroidRunWaiting {
                run_id: run,
                reason: None,
                checked_at: None
            })))
        }
        Err(error) => Err(failure(error)),
    }
}
async fn resume(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
    Json(request): Json<AndroidResumeRun>,
) -> RecoveryAnswer {
    if s.service
        .store()
        .android_private_server_run(&actor, run)
        .await
        .map_err(failure)?
        .is_none()
    {
        // Existence/read authorization precedes origin diagnostics. Neither
        // device checkpoints nor native/team workflow state supply this grant.
        if s.service
            .store()
            .android_device_run(&actor, run)
            .await
            .map_err(failure)?
            .is_none()
        {
            s.service
                .store()
                .android_run_request(&actor, run)
                .await
                .map_err(failure)?;
        }
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error":"private_resume_origin_unsupported"})),
        )
            .into_response());
    }
    Ok(Json(json!(
        s.devices
            .resume_private_run(&actor, run, &request)
            .await
            .map_err(failure)?
    )))
}
