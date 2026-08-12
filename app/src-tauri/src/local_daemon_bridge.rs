use std::{env, fs, path::PathBuf, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use crate::credential_store::{CredentialLocator, CredentialStore};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const UPSERT_PROVIDER_FROM_CREDENTIALS: &str = "provider_bindings.upsert_from_credentials";

#[derive(Debug, Serialize)]
struct DaemonRequest<'a> {
    id: Uuid,
    token: &'a str,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct DaemonResponse {
    result: Option<Value>,
    error: Option<DaemonError>,
}

#[derive(Debug, Deserialize)]
struct DaemonError {
    code: String,
    message: String,
}

#[tauri::command]
pub async fn local_daemon_call(
    method: String,
    params: Option<Value>,
    credential_state: tauri::State<'_, Arc<CredentialStore>>,
) -> Result<Value, String> {
    if method.trim().is_empty() || method.len() > 100 {
        return Err("invalid local daemon method".to_owned());
    }
    let (method, params) = if method == UPSERT_PROVIDER_FROM_CREDENTIALS {
        let store = credential_state.inner().clone();
        let params = params.unwrap_or(Value::Null);
        let resolved = tauri::async_runtime::spawn_blocking(move || {
            provider_binding_params_from_credentials(store.as_ref(), params)
        })
        .await
        .map_err(|_| "credential storage worker failed".to_owned())??;
        ("provider_bindings.upsert".to_owned(), resolved)
    } else {
        (method, params.unwrap_or(Value::Null))
    };
    let endpoint = daemon_endpoint();
    let token = daemon_token()?;
    let request = DaemonRequest {
        id: Uuid::new_v4(),
        token: &token,
        method: &method,
        params,
    };
    let encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_RESPONSE_BYTES {
        return Err("local daemon request exceeds 16 MiB".to_owned());
    }
    call_endpoint(&endpoint, &encoded).await
}

fn provider_binding_params_from_credentials(
    store: &CredentialStore,
    params: Value,
) -> Result<Value, String> {
    let profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "provider profile ID is required".to_owned())?;
    let base_url = params
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "provider base URL is required".to_owned())?;
    let api_key = store
        .get(&CredentialLocator {
            scope: "llm_profile".to_owned(),
            owner_id: profile_id.to_owned(),
            field: "api_key".to_owned(),
        })
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "profile_id": profile_id,
        "base_url": base_url,
        "api_key": api_key,
    }))
}

#[cfg(unix)]
async fn call_endpoint(endpoint: &str, encoded: &[u8]) -> Result<Value, String> {
    let stream = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|error| format!("local daemon is unavailable: {error}"))?;
    exchange(stream, encoded).await
}

#[cfg(windows)]
async fn call_endpoint(endpoint: &str, encoded: &[u8]) -> Result<Value, String> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
    const RETRY_INTERVAL: Duration = Duration::from_millis(50);
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PIPE_BUSY: i32 = 231;

    fn is_transient(error: &std::io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
        )
    }

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let last_error = loop {
        match ClientOptions::new().open(endpoint) {
            Ok(stream) => return exchange(stream, encoded).await,
            Err(error) if is_transient(&error) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            Err(error) => break error,
        }
    };
    Err(format!("local daemon is unavailable: {last_error}"))
}

async fn exchange<T>(stream: T, encoded: &[u8]) -> Result<Value, String>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(encoded)
        .await
        .map_err(|error| error.to_string())?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|error| error.to_string())?;
    writer.flush().await.map_err(|error| error.to_string())?;

    let mut line = String::new();
    BufReader::new(reader)
        .read_line(&mut line)
        .await
        .map_err(|error| error.to_string())?;
    if line.len() > MAX_RESPONSE_BYTES {
        return Err("local daemon response exceeds 16 MiB".to_owned());
    }
    let response: DaemonResponse =
        serde_json::from_str(&line).map_err(|error| format!("invalid daemon response: {error}"))?;
    if let Some(error) = response.error {
        return Err(format!("{}: {}", error.code, error.message));
    }
    response
        .result
        .ok_or_else(|| "local daemon returned neither a result nor an error".to_owned())
}

fn daemon_token() -> Result<String, String> {
    if let Ok(token) = env::var("COWORK_DAEMON_IPC_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    let path = env::var("COWORK_DAEMON_IPC_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_data_dir().join("ipc-token.txt"));
    let token = fs::read_to_string(&path).map_err(|error| {
        format!(
            "failed to read local daemon token at {}: {error}",
            path.display()
        )
    })?;
    let token = token.trim().to_owned();
    if token.len() < 32 {
        return Err("local daemon token is missing or too short".to_owned());
    }
    Ok(token)
}

fn daemon_endpoint() -> String {
    if let Ok(endpoint) = env::var("COWORK_DAEMON_IPC_ENDPOINT") {
        if !endpoint.trim().is_empty() {
            return endpoint;
        }
    }
    #[cfg(windows)]
    {
        let user = env::var("USERNAME")
            .unwrap_or_else(|_| "user".to_owned())
            .replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
        format!(r"\\.\pipe\open-cowork-{user}")
    }
    #[cfg(not(windows))]
    {
        env::var("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("open-cowork").join("daemon.sock"))
            .unwrap_or_else(|_| default_data_dir().join("daemon.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_owned()))
            .join("OpenCowork")
            .join("daemon")
    }
    #[cfg(not(windows))]
    {
        env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_owned()))
                    .join(".local")
                    .join("state")
            })
            .join("open-cowork")
            .join("daemon")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_client_waits_for_a_busy_pipe_to_become_available() {
        use tokio::{
            io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
            net::windows::named_pipe::{ClientOptions, ServerOptions},
        };

        tauri::async_runtime::block_on(async {
            let endpoint = format!(r"\\.\pipe\open-cowork-bridge-test-{}", Uuid::new_v4());
            let busy_server = ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(1)
                .create(&endpoint)
                .unwrap();
            let busy_client = ClientOptions::new().open(&endpoint).unwrap();
            busy_server.connect().await.unwrap();

            let responder_endpoint = endpoint.clone();
            let responder = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1_200)).await;
                drop(busy_client);
                drop(busy_server);

                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                let server = loop {
                    match ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(&responder_endpoint)
                    {
                        Ok(server) => break server,
                        Err(_) if tokio::time::Instant::now() < deadline => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Err(error) => panic!("failed to recreate test pipe: {error}"),
                    }
                };
                server.connect().await.unwrap();
                let (reader, mut writer) = tokio::io::split(server);
                let mut request = String::new();
                BufReader::new(reader)
                    .read_line(&mut request)
                    .await
                    .unwrap();
                assert!(!request.trim().is_empty());
                writer
                    .write_all(b"{\"result\":{\"recovered\":true},\"error\":null}\n")
                    .await
                    .unwrap();
                writer.flush().await.unwrap();
            });

            let response = tokio::time::timeout(
                Duration::from_secs(5),
                call_endpoint(&endpoint, br#"{"method":"health"}"#),
            )
            .await
            .expect("client retry exceeded the regression-test timeout")
            .unwrap();
            assert_eq!(response["recovered"], true);
            responder.await.unwrap();
        });
    }

    #[test]
    fn provider_binding_credentials_are_resolved_without_frontend_read_access() {
        let store = CredentialStore::in_memory();
        store
            .set(
                &CredentialLocator {
                    scope: "llm_profile".to_owned(),
                    owner_id: "profile-1".to_owned(),
                    field: "api_key".to_owned(),
                },
                "native-only-secret",
            )
            .unwrap();

        let resolved = provider_binding_params_from_credentials(
            &store,
            json!({"profile_id":"profile-1","base_url":"https://example.test/v1"}),
        )
        .unwrap();

        assert_eq!(resolved["profile_id"], "profile-1");
        assert_eq!(resolved["base_url"], "https://example.test/v1");
        assert_eq!(resolved["api_key"], "native-only-secret");
    }

    #[test]
    fn provider_binding_allows_profiles_without_credentials() {
        let resolved = provider_binding_params_from_credentials(
            &CredentialStore::in_memory(),
            json!({"profile_id":"default-ollama","base_url":"http://127.0.0.1:11434/v1"}),
        )
        .unwrap();

        assert!(resolved["api_key"].is_null());
    }
}
