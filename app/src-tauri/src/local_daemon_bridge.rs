use std::{env, fs, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

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
pub async fn local_daemon_call(method: String, params: Option<Value>) -> Result<Value, String> {
    if method.trim().is_empty() || method.len() > 100 {
        return Err("invalid local daemon method".to_owned());
    }
    let endpoint = daemon_endpoint();
    let token = daemon_token()?;
    let request = DaemonRequest {
        id: Uuid::new_v4(),
        token: &token,
        method: &method,
        params: params.unwrap_or(Value::Null),
    };
    let encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_RESPONSE_BYTES {
        return Err("local daemon request exceeds 16 MiB".to_owned());
    }
    call_endpoint(&endpoint, &encoded).await
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

    let mut last_error = None;
    for _ in 0..20 {
        match ClientOptions::new().open(endpoint) {
            Ok(stream) => return exchange(stream, encoded).await,
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Err(format!(
        "local daemon is unavailable: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "named pipe connection failed".to_owned())
    ))
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
