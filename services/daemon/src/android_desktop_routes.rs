use super::*;
use axum::extract::WebSocketUpgrade;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/runs/{run}/desktop-sessions", get(list).post(start))
        .route(
            "/runs/{run}/desktop-sessions/{session}",
            axum::routing::delete(stop),
        )
        .route(
            "/runs/{run}/desktop-sessions/{session}/tickets",
            post(ticket),
        )
        .route("/auth/reauthenticate", post(reauthenticate))
        .route("/desktop-sessions/{session}/stream", get(stream))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    #[serde(default = "width")]
    width: u32,
    #[serde(default = "height")]
    height: u32,
}
fn width() -> u32 {
    1440
}
fn height() -> u32 {
    900
}
async fn start(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
    Json(v): Json<Start>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_desktop_start(&a, run, v.width, v.height)
            .await?,
    ))
}
async fn list(State(s): State<CompatState>, Session(a): Session, Path(run): Path<Uuid>) -> Answer {
    Ok(Json(json!(
        s.service.store().android_desktops(&a, run).await?
    )))
}
async fn stop(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, id)): Path<(Uuid, Uuid)>,
) -> Answer {
    Ok(Json(
        s.service.store().android_desktop_stop(&a, run, id).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reauth {
    password: String,
    purpose: String,
}
async fn reauthenticate(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<Reauth>,
) -> Answer {
    if v.purpose != "desktop_control" {
        return Err(invalid().into());
    }
    let password = Zeroizing::new(v.password);
    Ok(Json(
        s.service
            .store()
            .android_security_desktop_reauthentication(&a, &password, &agents::secret()?)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    #[serde(default)]
    control: bool,
    reauthentication_token: Option<String>,
}
async fn ticket(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, id)): Path<(Uuid, Uuid)>,
    Json(v): Json<Ticket>,
) -> Answer {
    let reauth = v.reauthentication_token.map(Zeroizing::new);
    Ok(Json(
        s.service
            .store()
            .android_desktop_ticket(
                &a,
                run,
                id,
                v.control,
                reauth.as_deref().map(String::as_str),
                &agents::secret()?,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
struct Stream {
    ticket: String,
}
async fn stream(
    State(s): State<CompatState>,
    Path(id): Path<Uuid>,
    Query(v): Query<Stream>,
    ws: WebSocketUpgrade,
) -> std::result::Result<Response, Failure> {
    let token = Zeroizing::new(v.ticket);
    let access = Arc::new(
        s.service
            .store()
            .android_desktop_consume(id, &token)
            .await?,
    );
    let guard = s.devices.hub.connection()?;
    Ok(ws
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _ = s.devices.hub.relay(s.service.store(), access, socket).await;
        })
        .into_response())
}
