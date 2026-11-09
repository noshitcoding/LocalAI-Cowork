//! Original V1 terminal shapes over a genuine actor-private V2 run.
use super::*;
use axum::extract::WebSocketUpgrade;
fn failure(error: Error) -> Response {
    let (status, code) = match error {
        Error::Forbidden | Error::NotFound => (StatusCode::NOT_FOUND, "terminal_unavailable"),
        Error::Conflict | Error::UncertainEffect => {
            (StatusCode::CONFLICT, "terminal_original_state_unresolved")
        }
        Error::Validation(_) => (StatusCode::BAD_REQUEST, "invalid_terminal_request"),
        Error::BudgetExceeded => (StatusCode::TOO_MANY_REQUESTS, "terminal_limit"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "terminal_unavailable"),
    };
    (status, Json(json!({"error":code}))).into_response()
}

pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/runs/{run}/terminal-sessions", get(history).post(create))
        .route("/runs/{run}/terminal-sessions/{session}", get(transcript))
        .route(
            "/runs/{run}/terminal-sessions/{session}/observations/{version}",
            get(observation),
        )
        .route("/terminal-sessions/{session}/stream", get(stream))
}
async fn create(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<AndroidCreateTerminalSession>,
) -> std::result::Result<(StatusCode, Json<Value>), Response> {
    let mut keys = headers.get_all("idempotency-key").iter();
    let key = keys
        .next()
        .map(|v| v.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| failure(invalid()))?
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if keys.next().is_some() {
        return Err(failure(invalid()));
    }
    if !s.devices.private_server_available() {
        return Err(failure(Error::Forbidden));
    }
    let ticket = s
        .service
        .store()
        .android_private_terminal_create(
            &actor,
            run,
            &key,
            request.columns,
            request.rows,
            s.keys.as_ref(),
            s.key_id,
        )
        .await
        .map_err(failure)?;
    Ok((StatusCode::CREATED, Json(ticket)))
}
#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}
async fn stream(
    State(s): State<CompatState>,
    Path(session): Path<Uuid>,
    Query(query): Query<Ticket>,
    upgrade: WebSocketUpgrade,
) -> std::result::Result<Response, Response> {
    let connection = s
        .devices
        .prepare_private_terminal(session, &query.ticket)
        .await
        .map_err(failure)?;
    let service = s.devices;
    Ok(upgrade
        .protocols(["terminal.binary.v1"])
        .max_message_size(65536)
        .max_frame_size(65536)
        .on_upgrade(move |socket| service.private_terminal_stream(socket, connection)))
}
async fn history(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path(run): Path<Uuid>,
) -> std::result::Result<Json<Value>, Response> {
    s.service
        .store()
        .android_private_terminal_history(&actor, run, s.keys.as_ref(), None, 0)
        .await
        .map(Json)
        .map_err(failure)
}
#[derive(Deserialize, Default)]
struct Cursor {
    #[serde(default)]
    after: i64,
}
async fn transcript(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path((run, session)): Path<(Uuid, Uuid)>,
    Query(cursor): Query<Cursor>,
) -> std::result::Result<Json<Value>, Response> {
    s.service
        .store()
        .android_private_terminal_history(&actor, run, s.keys.as_ref(), Some(session), cursor.after)
        .await
        .map(Json)
        .map_err(failure)
}
async fn observation(
    State(s): State<CompatState>,
    Session(actor): Session,
    Path((run, session, version)): Path<(Uuid, Uuid, Uuid)>,
) -> std::result::Result<Json<Value>, Response> {
    s.service
        .store()
        .android_private_terminal_observation(
            &actor,
            run,
            session,
            &version.to_string(),
            s.keys.as_ref(),
        )
        .await
        .map(Json)
        .map_err(failure)
}
