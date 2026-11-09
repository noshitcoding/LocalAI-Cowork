//! Operator-selected federation key. The ticket never creates or adopts a V1 actor.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub(super) fn routes() -> Router<CompatState> {
    Router::new().route("/auth/fleet", post(accept))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    ticket: String,
    device_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    purpose: String,
    organisation_id: Uuid,
    instance_id: Uuid,
    user_id: Uuid,
    email: String,
    display_name: String,
    nonce: Uuid,
    expires_at: i64,
}
async fn accept(State(s): State<CompatState>, Json(v): Json<Request>) -> Answer {
    let authority = s.devices.current().fleet.clone().ok_or_else(|| {
        Error::Validation("Fleet sign-in requires explicit operator configuration".into())
    })?;
    let token = Zeroizing::new(v.ticket);
    if token.len() > 4096 || v.device_id.is_nil() {
        return Err(Error::Forbidden.into());
    }
    let (payload, signature) = token.split_once('.').ok_or(Error::Forbidden)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| Error::Forbidden)?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, authority.key.as_bytes());
    ring::hmac::verify(&key, payload.as_bytes(), &signature).map_err(|_| Error::Forbidden)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| Error::Forbidden)?;
    let t: Ticket = serde_json::from_slice(&bytes).map_err(|_| Error::Forbidden)?;
    let current = chrono::Utc::now().timestamp();
    if !matches!(
        t.purpose.as_str(),
        "instance_login" | "instance_viewer" | "instance_support"
    ) || t.organisation_id != authority.team
        || t.instance_id != authority.instance
        || t.nonce.is_nil()
        || t.expires_at <= current
        || t.expires_at > current + 90
        || t.email.len() > 200
        || t.display_name.len() > 200
    {
        return Err(Error::Forbidden.into());
    }
    let issued = s
        .service
        .store()
        .android_fleet_session(
            t.user_id,
            t.organisation_id,
            &t.email,
            t.nonce,
            t.expires_at * 1000,
            t.purpose == "instance_viewer",
            t.purpose == "instance_support",
        )
        .await?;
    Ok(Json(
        issued_tokens(&s, issued, &v.device_id.to_string()).await?,
    ))
}
