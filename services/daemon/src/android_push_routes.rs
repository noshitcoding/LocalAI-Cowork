//! Original FCM registration wire, with metadata-only status and encrypted tokens.
use super::*;
use crate::android_device_services::push_scope;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/push/config", get(configuration))
        .route("/devices/config", get(device_configuration))
        .route("/push/subscriptions", get(list).post(register))
        .route("/push/subscriptions/{id}", axum::routing::delete(remove))
        .route("/push/status", get(status))
}
async fn configuration(State(s): State<CompatState>) -> Json<Value> {
    Json(
        json!({"schema_version":2,"fcm_enabled":s.devices.current().push.is_some(),"web_push_public_key":null,"delivery":if s.devices.current().push.is_some(){"configured"}else{"operator_credentials_required"}}),
    )
}
async fn device_configuration(State(s): State<CompatState>) -> Json<Value> {
    Json(s.devices.configuration())
}
async fn list(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(
        s.service.store().android_push_subscriptions(&a).await?
    )))
}
async fn register(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<RegisterDevicePush>,
) -> Answer {
    if v.provider != "fcm" || v.device_id.is_nil() {
        return Err(invalid().into());
    }
    let token = Zeroizing::new(v.token);
    if token.is_empty()
        || token.len() > 2048
        || token.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid().into());
    }
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(token.as_bytes()));
    let value = s
        .service
        .store()
        .android_push_register(&a, v.device_id, &hash(token.as_bytes()), |id, rev| {
            let sealed = seal_bearer(
                s.keys.as_ref(),
                s.keys
                    .key_for_purpose(&s.key_id, cowork_secrets::KeyPurpose::DataEncryption)
                    .map_err(|_| invalid())?,
                &push_scope(a.identity_id(), v.device_id, id, rev),
                &encoded,
            )
            .map_err(|_| invalid())?;
            serde_json::to_string(&sealed).map_err(|_| invalid())
        })
        .await?;
    Ok(Json(json!(value)))
}
async fn remove(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    s.service.store().android_push_remove(&a, id).await?;
    Ok(Json(json!({"schema_version":2,"accepted":true})))
}
async fn status(State(s): State<CompatState>, Session(a): Session) -> Answer {
    let mut v = s.service.store().android_push_status(&a).await?;
    v["configured"] = json!(s.devices.current().push.is_some());
    v["delivery"] = json!(if s.devices.current().push.is_some() {
        "provider_acceptance_is_not_device_delivery"
    } else {
        "operator_credentials_required"
    });
    Ok(Json(v))
}
