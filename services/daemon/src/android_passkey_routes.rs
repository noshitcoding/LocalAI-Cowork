//! Cryptographic WebAuthn operations, using server-owned durable challenge state.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cowork_storage::{AndroidPasskeyFinished, AndroidPasskeyState, AndroidVerifiedPasskey};
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/auth/passkeys", get(list))
        .route("/auth/passkeys/config", get(configuration))
        .route("/auth/passkeys/register/start", post(register_start))
        .route("/auth/passkeys/register/finish", post(register_finish))
        .route("/auth/passkeys/{id}", axum::routing::delete(remove))
        .route(
            "/auth/passkeys/authenticate/start",
            post(authenticate_start),
        )
        .route(
            "/auth/passkeys/authenticate/finish",
            post(authenticate_finish),
        )
        .route("/auth/native/passkey/authorize", get(native_page))
        .route("/auth/native/passkey/client.js", get(native_script))
        .route("/auth/native/passkey/start", post(native_start))
        .route("/auth/native/passkey/finish", post(native_finish))
        .route(
            "/auth/native/passkey/register/start",
            post(native_registration_start),
        )
        .route(
            "/auth/native/passkey/register/options",
            post(native_registration_options),
        )
        .route(
            "/auth/native/passkey/register/finish",
            post(native_registration_finish),
        )
        .route(
            "/auth/native/passkey/register/ack",
            post(native_registration_ack),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRegistrationStart {
    label: String,
    device_id: Uuid,
    code_challenge: String,
    state: String,
    redirect_uri: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationTicket {
    purpose: String,
    challenge_id: Uuid,
    public_key: Value,
    label: String,
    device_id: Uuid,
    code_challenge: String,
    state: String,
    redirect_uri: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    ticket: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationFinish {
    ticket: String,
    credential: Value,
}
async fn native_registration_start(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<NativeRegistrationStart>,
) -> Answer {
    if v.label.trim().is_empty()
        || v.label.len() > 200
        || v.label.chars().any(char::is_control)
        || !native_redirect(&v.redirect_uri, &v.state)
        || !(43..=128).contains(&v.state.len())
        || v.code_challenge.len() != 43
        || !v
            .code_challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        || !v
            .state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return Err(invalid().into());
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(Error::Forbidden)?;
    let operation = auth_security::operation(&headers)?;
    let request = json!({"label":v.label.trim(),"device_id":v.device_id,"code_challenge":v.code_challenge,"state":v.state,"redirect_uri":v.redirect_uri,"binding":bound(&s)?});
    if let Some(reply) = s
        .service
        .store()
        .android_transport_registration_start(
            s.keys.as_ref(),
            s.key_id,
            &a,
            token,
            operation,
            &request,
            None,
            None,
        )
        .await?
    {
        return Ok(Json(reply));
    }
    let (challenge, public) = registration_challenge(&s, &a).await?;
    let metadata = RegistrationTicket {
        purpose: "native_passkey_registration".into(),
        challenge_id: challenge.id,
        public_key: public,
        label: v.label.trim().into(),
        device_id: v.device_id,
        code_challenge: v.code_challenge,
        state: v.state,
        redirect_uri: v.redirect_uri,
    };
    let metadata = serde_json::to_value(metadata).map_err(|_| invalid())?;
    let response = s
        .service
        .store()
        .android_transport_registration_start(
            s.keys.as_ref(),
            s.key_id,
            &a,
            token,
            operation,
            &request,
            Some(&challenge),
            Some(&metadata),
        )
        .await?
        .ok_or(Error::Conflict)?;
    Ok(Json(response))
}

async fn registration_ticket(
    s: &CompatState,
    secret: &str,
) -> Result<(AndroidAuthGrant, AuthPayload, Principal, RegistrationTicket)> {
    if !secret.starts_with("cw2a_") || secret.len() > 256 {
        return Err(Error::Forbidden);
    }
    let grant = s
        .service
        .store()
        .android_grant("code", &hash(secret.as_bytes()))
        .await?;
    let (payload, actor) = open_grant(s, &grant).await?;
    let metadata: RegistrationTicket =
        serde_json::from_str(payload.challenge.as_deref().ok_or(Error::Forbidden)?)
            .map_err(|_| Error::Forbidden)?;
    if metadata.purpose != "native_passkey_registration"
        || metadata.device_id.to_string() != grant.device_id
        || !native_redirect(&metadata.redirect_uri, &metadata.state)
    {
        return Err(Error::Forbidden);
    }
    Ok((grant, payload, actor, metadata))
}
async fn native_registration_options(
    State(s): State<CompatState>,
    Json(v): Json<Ticket>,
) -> Answer {
    let (_, _, _, metadata) = registration_ticket(&s, &v.ticket).await?;
    Ok(Json(
        json!({"public_key":metadata.public_key,"label":metadata.label,"state":metadata.state,"redirect_uri":metadata.redirect_uri}),
    ))
}
async fn native_registration_finish(
    State(s): State<CompatState>,
    Json(v): Json<RegistrationFinish>,
) -> Answer {
    let raw = s
        .service
        .store()
        .android_transport_registration_ticket(s.keys.as_ref(), &v.ticket)
        .await?;
    let metadata: RegistrationTicket =
        serde_json::from_value(raw.clone()).map_err(|_| invalid())?;
    if !native_redirect(&metadata.redirect_uri, &metadata.state) {
        return Err(Error::Forbidden.into());
    }
    let request = json!({"credential":v.credential,"binding":bound(&s)?});
    Ok(Json(
        s.service
            .store()
            .android_transport_registration_finish(
                s.keys.as_ref(),
                s.key_id,
                metadata.challenge_id,
                &v.ticket,
                &request,
                &raw,
                |challenge, _| {
                    checked_binding(&s, &challenge.binding)?;
                    let state: PasskeyRegistration =
                        serde_json::from_value(challenge.state.clone())
                            .map_err(|_| Error::Conflict)?;
                    let credential: RegisterPublicKeyCredential =
                        serde_json::from_value(v.credential).map_err(|_| invalid())?;
                    let passkey = webauthn(&s)?
                        .finish_passkey_registration(&credential, &state)
                        .map_err(|_| Error::Forbidden)?;
                    Ok(AndroidVerifiedPasskey::Registration {
                        credential_id: URL_SAFE_NO_PAD.encode(passkey.cred_id().as_ref()),
                        credential: serde_json::to_value(passkey).map_err(|_| invalid())?,
                        label: metadata.label,
                    })
                },
            )
            .await?,
    ))
}
async fn native_registration_ack(
    State(s): State<CompatState>,
    headers: HeaderMap,
    Json(v): Json<TokenExchange>,
) -> Answer {
    let code = Zeroizing::new(v.code);
    let verifier = Zeroizing::new(v.code_verifier);
    Ok(Json(
        s.service
            .store()
            .android_transport_registration_ack(
                s.keys.as_ref(),
                s.key_id,
                auth_security::operation(&headers)?,
                &code,
                &verifier,
                &v.device_id,
            )
            .await?,
    ))
}
fn webauthn(s: &CompatState) -> Result<Arc<webauthn_rs::Webauthn>> {
    s.devices.current().webauthn.clone().ok_or_else(|| {
        Error::Validation(
            "Passkeys require the operator's exact public HTTPS origin and RP ID".into(),
        )
    })
}
fn bound(s: &CompatState) -> Result<Value> {
    let c = s.devices.current();
    Ok(
        json!({"origin":c.origin.as_ref().ok_or(Error::Forbidden)?,"rp_id":c.rp_id.as_ref().ok_or(Error::Forbidden)?,"configuration_revision":c.revision}),
    )
}
fn checked_binding(s: &CompatState, value: &Value) -> Result<()> {
    let b = bound(s)?;
    if value["origin"] != b["origin"]
        || value["rp_id"] != b["rp_id"]
        || value["configuration_revision"] != b["configuration_revision"]
    {
        Err(Error::Forbidden)
    } else {
        Ok(())
    }
}
async fn configuration(State(s): State<CompatState>) -> Json<Value> {
    let c = s.devices.current();
    Json(
        json!({"schema_version":2,"enabled":c.webauthn.is_some(),"rp_id":c.rp_id,"origin":c.origin}),
    )
}
async fn list(State(s): State<CompatState>, Session(a): Session) -> Answer {
    Ok(Json(json!(s.service.store().android_passkeys(&a).await?)))
}
async fn registration_challenge(
    s: &CompatState,
    a: &Principal,
) -> Result<(AndroidPasskeyState, Value)> {
    let (id, revision, name, display, keys) = s
        .service
        .store()
        .android_passkey_registration_identity(a)
        .await?;
    let excluded = keys
        .iter()
        .map(|k| {
            serde_json::from_value::<Passkey>(k.credential.clone())
                .map(|p| p.cred_id().clone())
                .map_err(|_| Error::Conflict)
        })
        .collect::<Result<Vec<_>>>()?;
    let (public, state) = webauthn(s)?
        .start_passkey_registration(
            id,
            &name,
            &display,
            (!excluded.is_empty()).then_some(excluded),
        )
        .map_err(|_| invalid())?;
    let value = AndroidPasskeyState {
        id: Uuid::new_v4(),
        identity: id,
        identity_revision: revision,
        kind: "registration".into(),
        device_id: None,
        state: serde_json::to_value(state).map_err(|_| invalid())?,
        binding: bound(s)?,
        expires_at: now() + 300_000,
    };
    Ok((value, serde_json::to_value(public).map_err(|_| invalid())?))
}
async fn register_start(State(s): State<CompatState>, Session(a): Session) -> Answer {
    let (value, public) = registration_challenge(&s, &a).await?;
    s.service
        .store()
        .android_passkey_create_challenge(Some(&a), &value)
        .await?;
    Ok(Json(
        json!({"schema_version":2,"challenge_id":value.id,"public_key":public,"expires_at":iso(value.expires_at)?}),
    ))
}
async fn register_finish(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<FinishDevicePasskeyRegistration>,
) -> Answer {
    let result = s
        .service
        .store()
        .android_passkey_finish(Some(&a), v.challenge_id, "registration", |challenge, _| {
            checked_binding(&s, &challenge.binding)?;
            let state: PasskeyRegistration =
                serde_json::from_value(challenge.state.clone()).map_err(|_| Error::Conflict)?;
            let credential: RegisterPublicKeyCredential =
                serde_json::from_value(v.credential).map_err(|_| invalid())?;
            let passkey = webauthn(&s)?
                .finish_passkey_registration(&credential, &state)
                .map_err(|_| Error::Forbidden)?;
            Ok(AndroidVerifiedPasskey::Registration {
                credential_id: URL_SAFE_NO_PAD.encode(passkey.cred_id().as_ref()),
                credential: serde_json::to_value(passkey).map_err(|_| invalid())?,
                label: v.label,
            })
        })
        .await?;
    match result {
        AndroidPasskeyFinished::Registered(value) => Ok(Json(json!(value))),
        _ => Err(Error::Conflict.into()),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    password: String,
    #[serde(default)]
    second_factor: Option<String>,
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<Remove>,
) -> Answer {
    if v.second_factor.is_some() {
        return Err(invalid().into());
    }
    let password = Zeroizing::new(v.password);
    s.service
        .store()
        .android_transport_remove_passkey(&a, id, &password)
        .await?;
    Ok(Json(json!({"schema_version":2,"accepted":true})))
}
async fn begin_auth(
    s: &CompatState,
    email: &str,
    device: Uuid,
    kind: &str,
    binding: Value,
) -> Result<Value> {
    let (id, revision, _, _, keys) = s
        .service
        .store()
        .android_passkey_start_identity(email)
        .await?;
    if keys.is_empty() {
        return Err(Error::Forbidden);
    }
    let credentials = keys
        .into_iter()
        .map(|k| serde_json::from_value::<Passkey>(k.credential).map_err(|_| Error::Conflict))
        .collect::<Result<Vec<_>>>()?;
    let (public, state) = webauthn(s)?
        .start_passkey_authentication(&credentials)
        .map_err(|_| Error::Forbidden)?;
    let value = AndroidPasskeyState {
        id: Uuid::new_v4(),
        identity: id,
        identity_revision: revision,
        kind: kind.into(),
        device_id: Some(device),
        state: serde_json::to_value(state).map_err(|_| invalid())?,
        binding,
        expires_at: now() + 300_000,
    };
    s.service
        .store()
        .android_passkey_create_challenge(None, &value)
        .await?;
    Ok(
        json!({"schema_version":2,"challenge_id":value.id,"public_key":public,"expires_at":iso(value.expires_at)?}),
    )
}
async fn authenticate_start(
    State(s): State<CompatState>,
    Json(v): Json<StartDevicePasskeyAuthentication>,
) -> Answer {
    Ok(Json(
        begin_auth(&s, &v.email, v.device_id, "authentication", bound(&s)?).await?,
    ))
}
fn verify_auth(
    s: &CompatState,
    challenge: &AndroidPasskeyState,
    keys: &[cowork_storage::AndroidStoredPasskey],
    credential: Value,
    authorization: Option<Uuid>,
) -> Result<AndroidVerifiedPasskey> {
    checked_binding(s, &challenge.binding)?;
    if authorization.is_some_and(|id| challenge.binding["authorization_id"] != json!(id)) {
        return Err(Error::Forbidden);
    }
    let state: PasskeyAuthentication =
        serde_json::from_value(challenge.state.clone()).map_err(|_| Error::Conflict)?;
    let credential: PublicKeyCredential =
        serde_json::from_value(credential).map_err(|_| invalid())?;
    let result = webauthn(s)?
        .finish_passkey_authentication(&credential, &state)
        .map_err(|_| Error::Forbidden)?;
    if !result.user_verified() {
        return Err(Error::Forbidden);
    }
    for key in keys {
        let mut current: Passkey =
            serde_json::from_value(key.credential.clone()).map_err(|_| Error::Conflict)?;
        if current.cred_id() != result.cred_id() {
            continue;
        }
        let counter = key
            .credential
            .pointer("/cred/counter")
            .and_then(Value::as_u64)
            .ok_or(Error::Conflict)?;
        if counter > 0 && u64::from(result.counter()) <= counter {
            return Err(Error::Forbidden);
        }
        if current.update_credential(&result).is_none() {
            return Err(Error::Forbidden);
        }
        return Ok(AndroidVerifiedPasskey::Authentication {
            id: key.id,
            credential: serde_json::to_value(current).map_err(|_| invalid())?,
        });
    }
    Err(Error::Forbidden)
}
async fn authenticate_finish(
    State(s): State<CompatState>,
    Json(v): Json<FinishDevicePasskeyAuthentication>,
) -> Answer {
    let request = json!({"credential":v.credential,"binding":bound(&s)?});
    Ok(Json(
        s.service
            .store()
            .android_transport_passkey_finish(
                s.keys.as_ref(),
                s.key_id,
                v.challenge_id,
                v.challenge_id,
                "authentication",
                &request,
                |challenge, keys| verify_auth(&s, challenge, keys, v.credential, None),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeStart {
    email: String,
    device_id: Uuid,
    code_challenge: String,
    code_challenge_method: String,
    state: String,
    redirect_uri: String,
}
async fn native_start(State(s): State<CompatState>, Json(v): Json<NativeStart>) -> Answer {
    if !native_redirect(&v.redirect_uri, &v.state)
        || v.code_challenge_method != "S256"
        || v.code_challenge.len() != 43
        || !(43..=128).contains(&v.state.len())
        || !v
            .code_challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        || !v
            .state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return Err(invalid().into());
    }
    let id = Uuid::new_v4();
    let mut binding = bound(&s)?;
    binding["authorization_id"] = json!(id);
    binding["challenge"] = json!(v.code_challenge);
    binding["client_state"] = json!(v.state);
    binding["redirect_uri"] = json!(v.redirect_uri);
    let mut response =
        begin_auth(&s, &v.email, v.device_id, "native_authentication", binding).await?;
    response["authorization_id"] = json!(id);
    Ok(Json(response))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFinish {
    challenge_id: Uuid,
    authorization_id: Uuid,
    credential: Value,
}
async fn native_finish(State(s): State<CompatState>, Json(v): Json<NativeFinish>) -> Answer {
    let request =
        json!({"credential":v.credential,"authorization":v.authorization_id,"binding":bound(&s)?});
    Ok(Json(
        s.service
            .store()
            .android_transport_passkey_finish(
                s.keys.as_ref(),
                s.key_id,
                v.authorization_id,
                v.challenge_id,
                "native_authentication",
                &request,
                |challenge, keys| {
                    let state = challenge.binding["client_state"]
                        .as_str()
                        .ok_or(Error::Forbidden)?;
                    let redirect = challenge.binding["redirect_uri"]
                        .as_str()
                        .ok_or(Error::Forbidden)?;
                    if !native_redirect(redirect, state) {
                        return Err(Error::Forbidden);
                    }
                    verify_auth(&s, challenge, keys, v.credential, Some(v.authorization_id))
                },
            )
            .await?,
    ))
}
fn native_redirect(value: &str, state: &str) -> bool {
    if value == "open-cowork://auth/callback" {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some_and(|p| p >= 1024)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == format!("/callback/{state}")
}
async fn native_page() -> Response {
    ([(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CACHE_CONTROL,"no-store")],"<!doctype html><html lang=\"de\"><meta charset=\"utf-8\"><title>LocalAI Cowork Passkey</title><h1>Mit Passkey anmelden</h1><form id=\"login\"><label>Benutzername <input id=\"email\" autocomplete=\"username\" required></label><button>Anmelden</button></form><p id=\"status\" role=\"status\"></p><button id=\"abandon\" type=\"button\" hidden>Lokalen Marker entfernen / Remove local marker</button><script src=\"/api/v1/auth/native/passkey/client.js\"></script></html>").into_response()
}
async fn native_script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("android_native_passkey.js"),
    )
        .into_response()
}
