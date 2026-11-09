use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new().route("/threads/{thread}/configuration", get(read).put(write))
}
async fn read(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(thread): Path<Uuid>,
) -> Answer {
    Ok(Json(
        serde_json::to_value(
            s.service
                .store()
                .actor_chat_configuration(&a, thread)
                .await?,
        )
        .map_err(|_| invalid())?,
    ))
}
async fn write(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(thread): Path<Uuid>,
    Json(v): Json<WriteActorChatConfiguration>,
) -> Answer {
    Ok(Json(
        serde_json::to_value(
            s.service
                .store()
                .write_actor_chat_configuration(&a, thread, &v)
                .await?,
        )
        .map_err(|_| invalid())?,
    ))
}
