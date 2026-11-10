//! Human owner connector control plane. Runtime sends use normal reviewed MCP tools.
use super::*;
use axum::Extension;
use cowork_storage::NativeMcpFactory;
use cowork_toolbox::Toolbox;
use std::collections::BTreeMap;

#[derive(serde::Serialize, serde::Deserialize)]
struct Secrets {
    url: String,
    bearer: String,
    request_sha256: String,
}
impl Drop for Secrets {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.url.zeroize();
        self.bearer.zeroize();
    }
}
pub(super) fn owner_router(
    toolbox: Arc<Toolbox>,
    bridges: Arc<native_mcp::StdioBridges>,
) -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/connectors",
            get(list).post(save_server),
        )
        .route(
            "/projects/{project}/connectors/{id}/delete",
            post(delete_server),
        )
        .route(
            "/projects/{project}/connectors/{id}/test",
            post(test_server),
        )
        .layer(Extension(toolbox))
        .layer(Extension(bridges))
        .layer(DefaultBodyLimit::max(16 * 1024))
}
pub(super) fn native_router(
    state: AppState,
    toolbox: Arc<Toolbox>,
    bridges: Arc<native_mcp::StdioBridges>,
) -> Router<AppState> {
    Router::new()
        .route("/connectors/{project}", post(save_native))
        .route("/connectors/{project}/{id}/delete", post(delete_native))
        .route("/connectors/{project}/{id}/test", post(test_native))
        .layer(Extension(toolbox))
        .layer(Extension(bridges))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
fn server_allowed(state: &AppState) -> Result<(), Error> {
    if state.native_token.is_some() || !state.runtime.server_owner_mcp_available() {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn list(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<OwnerConnector>> {
    Ok(Json(s.runtime.store.owner_connectors(&project).await?))
}
async fn save_server(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Extension(t): Extension<Arc<Toolbox>>,
    Extension(b): Extension<Arc<native_mcp::StdioBridges>>,
    body: Input<SaveOwnerConnector>,
) -> Answer<OwnerConnector> {
    server_allowed(&s)?;
    save(s, project, t, b, input(body)?, false).await
}
async fn save_native(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Extension(t): Extension<Arc<Toolbox>>,
    Extension(b): Extension<Arc<native_mcp::StdioBridges>>,
    body: Input<SaveOwnerConnector>,
) -> Answer<OwnerConnector> {
    save(s, project, t, b, input(body)?, true).await
}
fn connector_origin(url: &str) -> std::result::Result<(String, String), Error> {
    if url.len() > 1900 || url.chars().any(char::is_control) {
        return Err(Error::Validation("Invalid connector URL".into()));
    }
    let parsed =
        reqwest::Url::parse(url).map_err(|_| Error::Validation("Invalid connector URL".into()))?;
    let loopback = parsed.host_str().is_some_and(|h| {
        h.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if !(parsed.scheme() == "https" || parsed.scheme() == "http" && loopback)
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Error::Validation("Connector requires HTTPS or literal loopback HTTP without user information or fragment".into()));
    }
    Ok((
        parsed.as_str().into(),
        parsed.origin().ascii_serialization(),
    ))
}
fn version_id(project: &str, id: &str, key: &str) -> String {
    let hash = cowork_domain::canonical_hash(&json!(["connector-version", project, id, key]));
    let bytes = (0..32)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hash[i..i + 2], 16).expect("hex digest"))
        .collect::<Vec<_>>();
    let mut fixed = [0u8; 16];
    fixed.copy_from_slice(&bytes);
    fixed[6] = (fixed[6] & 15) | 64;
    fixed[8] = (fixed[8] & 63) | 128;
    uuid::Uuid::from_bytes(fixed).to_string()
}
fn catalog(destination: &str, name: &str) -> Vec<Value> {
    vec![
        json!({"name":"send_json","title":format!("{name} — {destination}"),"description":format!("Send one JSON POST to {destination}. Endpoint: {name}. The configured path and credentials stay private. This action can modify external data; approve the concrete payload."),"inputSchema":{"type":"object","properties":{"destination":{"type":"string","const":destination},"payload":{"type":"object"}},"required":["destination","payload"],"additionalProperties":false},"_meta":{"io.localai.connector":true},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}),
    ]
}
async fn secrets_for(s: &AppState, f: &NativeMcpFactory) -> std::result::Result<Secrets, Error> {
    let env = s.runtime.mcp_stdio_environment(f).await?;
    serde_json::from_str(env.get("CONNECTOR_CONFIG").ok_or(Error::Forbidden)?)
        .map_err(|_| Error::Forbidden)
}
fn equal_private(first: &str, second: &str) -> bool {
    let key = ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        b"cowork-connector-private-equality",
    );
    let tag = ring::hmac::sign(&key, first.as_bytes());
    ring::hmac::verify(&key, second.as_bytes(), tag.as_ref()).is_ok()
}
fn compare_supplied(
    input: &SaveOwnerConnector,
    secrets: &Secrets,
) -> std::result::Result<(), Error> {
    if input
        .url
        .as_ref()
        .is_some_and(|u| match connector_origin(u) {
            Ok((u, _)) => !equal_private(&u, &secrets.url),
            Err(_) => true,
        })
        || input
            .bearer
            .as_ref()
            .is_some_and(|b| !equal_private(b, &secrets.bearer))
        || input.remove_bearer && !secrets.bearer.is_empty()
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
async fn save(
    s: AppState,
    project: String,
    t: Arc<Toolbox>,
    b: Arc<native_mcp::StdioBridges>,
    mut input: SaveOwnerConnector,
    native: bool,
) -> Answer<OwnerConnector> {
    uuid::Uuid::parse_str(&project).map_err(|_| Error::Forbidden)?;
    uuid::Uuid::parse_str(&input.id).map_err(|_| Error::Forbidden)?;
    if input.expected_revision < 0
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > 128
        || input.name.trim().is_empty()
        || input.name.len() > 200
        || input.name.chars().any(char::is_control)
        || input.note.len() > 4000
        || input
            .bearer
            .as_ref()
            .is_some_and(|v| v.len() > 512 || !v.bytes().all(|c| c.is_ascii_graphic()))
        || input.remove_bearer && input.bearer.as_ref().is_some_and(|v| !v.is_empty())
    {
        return Err(Error::Validation("Invalid connector settings".into()).into());
    }
    let adapter = t.connector_adapter().map_err(|_| {
        Error::Validation("Configure a Python runtime before creating a connector".into())
    })?;
    let digest = cowork_storage::connector_request_hash(&input);
    if let Some(receipt) = s
        .runtime
        .store
        .owner_connector_receipt(&project, &input.idempotency_key, &digest)
        .await?
    {
        if receipt.native_only != native {
            return Err(Error::Forbidden.into());
        }
        if input.url.is_some() || input.bearer.is_some() || input.remove_bearer {
            let f = s
                .runtime
                .store
                .native_mcp_factory(&receipt.server_id)
                .await?
                .ok_or(Error::NotFound)?;
            compare_supplied(&input, &secrets_for(&s, &f).await?)?;
        }
        return Ok(Json(receipt));
    }
    let previous = match s.runtime.store.owner_connector(&project, &input.id).await {
        Ok(c) => Some(c),
        Err(Error::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    if previous.as_ref().map_or(0, |c| c.revision) != input.expected_revision
        || previous
            .as_ref()
            .is_some_and(|c| c.deleted || c.native_only != native)
    {
        return Err(Error::Conflict.into());
    }
    let id = version_id(&project, &input.id, &input.idempotency_key);
    let origin = if native {
        "http://127.0.0.1:43821".to_owned()
    } else {
        native_mcp::server_bridge_origin(&s).ok_or(Error::Forbidden)?
    };
    let endpoint = format!("{origin}/native-mcp/{id}");
    let reference = format!("{}-mcp:{id}", if native { "owner" } else { "server" });
    let empty = NativeMcpFactory {
        server_id: id.clone(),
        project_id: project.clone(),
        program: "python".into(),
        arguments: vec![],
        cwd: None,
        environment: BTreeMap::from([("CONNECTOR_CONFIG".into(), 1)]),
        native_only: native,
        endpoint: endpoint.clone(),
        credential_reference: reference.clone(),
        credential_revision: 1,
        revision: 1,
        enabled: input.enabled,
    };
    let staged = match secrets_for(&s, &empty).await {
        Ok(value) => Some(value),
        Err(Error::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    let secrets = if let Some(value) = staged {
        if value.request_sha256 != digest {
            return Err(Error::Conflict.into());
        }
        compare_supplied(&input, &value)?;
        value
    } else {
        if input.recover_only {
            return Err(Error::Conflict.into());
        }
        let old = if let Some(c) = &previous {
            let f = s
                .runtime
                .store
                .native_mcp_factory(&c.server_id)
                .await?
                .ok_or(Error::NotFound)?;
            Some(secrets_for(&s, &f).await?)
        } else {
            None
        };
        let url = input
            .url
            .take()
            .or_else(|| old.as_ref().map(|v| v.url.clone()))
            .ok_or(Error::Validation("Provide a connector URL".into()))?;
        let (url, _) = connector_origin(&url)?;
        let bearer = if input.remove_bearer {
            String::new()
        } else {
            input
                .bearer
                .take()
                .or_else(|| old.as_ref().map(|v| v.bearer.clone()))
                .unwrap_or_default()
        };
        Secrets {
            url,
            bearer,
            request_sha256: digest.clone(),
        }
    };
    let (_, destination) = connector_origin(&secrets.url)?;
    let encoded =
        zeroize::Zeroizing::new(serde_json::to_string(&secrets).map_err(|_| Error::Forbidden)?);
    if encoded.len() > 2000 {
        return Err(Error::Validation(
            "Connector URL and key exceed the 2000-byte encrypted configuration limit".into(),
        )
        .into());
    }
    let environment = s
        .runtime
        .save_connector_environment(&project, &id, &endpoint, &encoded, native)
        .await?;
    let (reference, revision) = match s.runtime.store.mcp_credential(&project, &reference).await {
        Ok(c) => (c.scope.reference, c.revision),
        Err(Error::NotFound) => {
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            if native {
                s.runtime
                    .save_native_mcp_bearer(&project, &id, &endpoint, token, 0)
                    .await?
            } else {
                s.runtime
                    .save_server_mcp_bearer(&project, &id, &endpoint, token, 0)
                    .await?
            }
        }
        Err(e) => return Err(e.into()),
    };
    let value = OwnerConnector {
        id: input.id.clone(),
        project_id: project.clone(),
        revision: input
            .expected_revision
            .checked_add(1)
            .ok_or(Error::Conflict)?,
        name: input.name.clone(),
        note: input.note.clone(),
        destination: destination.clone(),
        enabled: input.enabled,
        deleted: false,
        native_only: native,
        server_id: id,
        has_bearer: !secrets.bearer.is_empty(),
    };
    let factory = NativeMcpFactory {
        environment,
        credential_reference: reference,
        credential_revision: revision,
        arguments: vec![
            "-I".into(),
            adapter.to_string_lossy().into(),
            destination.clone(),
            input.name.clone(),
        ],
        ..empty
    };
    let saved = s
        .runtime
        .store
        .publish_connector(&input, &value, &factory, catalog(&destination, &input.name))
        .await?;
    if !saved.enabled {
        for id in s
            .runtime
            .store
            .connector_version_ids(&project, &saved.id)
            .await?
        {
            native_mcp::remove_connections(&b, &t, &id).await?;
        }
    }
    Ok(Json(saved))
}
async fn delete_server(
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
    Extension(t): Extension<Arc<Toolbox>>,
    Extension(b): Extension<Arc<native_mcp::StdioBridges>>,
    body: Input<DeleteOwnerConnector>,
) -> Answer<OwnerConnector> {
    server_allowed(&s)?;
    delete(s, p, id, t, b, input(body)?, false).await
}
async fn delete_native(
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
    Extension(t): Extension<Arc<Toolbox>>,
    Extension(b): Extension<Arc<native_mcp::StdioBridges>>,
    body: Input<DeleteOwnerConnector>,
) -> Answer<OwnerConnector> {
    delete(s, p, id, t, b, input(body)?, true).await
}
async fn delete(
    s: AppState,
    p: String,
    id: String,
    t: Arc<Toolbox>,
    b: Arc<native_mcp::StdioBridges>,
    input: DeleteOwnerConnector,
    native: bool,
) -> Answer<OwnerConnector> {
    if input.idempotency_key.is_empty() || input.idempotency_key.len() > 128 {
        return Err(Error::Forbidden.into());
    }
    let saved = s
        .runtime
        .store
        .delete_connector(&p, &id, &input, native)
        .await?;
    for id in s.runtime.store.connector_version_ids(&p, &id).await? {
        native_mcp::remove_connections(&b, &t, &id).await?;
    }
    Ok(Json(saved))
}
async fn test_server(
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
) -> Answer<OwnerConnectorProbe> {
    server_allowed(&s)?;
    test(s, p, id, false).await
}
async fn test_native(
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
) -> Answer<OwnerConnectorProbe> {
    test(s, p, id, true).await
}
async fn test(s: AppState, p: String, id: String, native: bool) -> Answer<OwnerConnectorProbe> {
    let c = s.runtime.store.owner_connector(&p, &id).await?;
    if c.deleted || c.native_only != native {
        return Err(Error::Forbidden.into());
    }
    let f = s
        .runtime
        .store
        .native_mcp_factory(&c.server_id)
        .await?
        .ok_or(Error::NotFound)?;
    let secrets = secrets_for(&s, &f).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::McpSessionUnavailable)?;
    let mut request = client.head(&secrets.url);
    if !secrets.bearer.is_empty() {
        request = request.bearer_auth(&secrets.bearer)
    }
    let response = request
        .send()
        .await
        .map_err(|_| Error::McpSessionUnavailable)?;
    Ok(Json(OwnerConnectorProbe {
        destination: c.destination,
        status: response.status().as_u16(),
        reachable: true,
    }))
}

#[cfg(test)]
#[path = "connector_tests.rs"]
mod tests;
