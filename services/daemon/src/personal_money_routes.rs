use super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/personal-money", get(status))
}

async fn status(State(state): State<AppState>) -> Answer<PersonalMoneyStatus> {
    Ok(Json(state.runtime.store.personal_money_status().await?))
}
