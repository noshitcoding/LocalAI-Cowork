//! Lease-bound outbound desktop channel with independent local view/control
//! consent. The HTTP transport owns claiming and heartbeats; this socket never
//! sends a heartbeat that could accidentally claim another run.
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{sync::Mutex, task::JoinSet};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{execution::local_consent, transport::AgentClient};

#[cfg(windows)]
#[path = "desktop_windows.rs"]
mod native;

#[cfg(target_os = "linux")]
#[path = "desktop_linux.rs"]
mod native;

type SessionConsentKey = (Uuid, Uuid, Uuid, bool);
type SessionConsentDecisions = Arc<Mutex<HashMap<SessionConsentKey, bool>>>;

pub type ActiveRun = Arc<Mutex<Option<(Uuid, Uuid, CancellationToken)>>>;

pub fn available() -> bool {
    #[cfg(any(windows, target_os = "linux"))]
    {
        native::available()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Hello {
        schema_version: u16,
        executor_id: Uuid,
    },
    DesktopStreamRequested {
        run_id: Uuid,
        session_id: Uuid,
        stream_id: Uuid,
        control: bool,
    },
    Ack {},
    Error {},
}

fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(16 * 1024 * 1024))
        .max_frame_size(Some(16 * 1024 * 1024))
}

pub async fn supervise(
    client: AgentClient,
    active: ActiveRun,
    consent_directory: PathBuf,
    personal: bool,
    policy: String,
    stop: CancellationToken,
) -> Result<()> {
    if personal
        && !matches!(
            policy.as_str(),
            "off" | "confirm_each_session" | "unattended"
        )
    {
        bail!("Invalid local desktop policy");
    }
    if personal && policy == "off" {
        return Ok(());
    }
    // Session decisions never survive process restart and are kept separately
    // for view and control. Changing the run's lease also clears the decision.
    let decisions = Arc::new(Mutex::new(HashMap::<SessionConsentKey, bool>::new()));
    loop {
        if stop.is_cancelled() {
            return Ok(());
        }
        let connection = connect_once(
            &client,
            &active,
            &consent_directory,
            personal,
            &policy,
            &stop,
            &decisions,
        )
        .await;
        if stop.is_cancelled() {
            return Ok(());
        }
        // Connecting again is read-only. Every stream has a unique server grant,
        // and connection loss drains it before another socket is opened.
        if let Err(error) = connection {
            eprintln!("Device desktop channel unavailable: {error}");
        }
        tokio::select! { biased;
            _ = stop.cancelled() => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(3)) => {},
        }
    }
}

async fn connect_once(
    client: &AgentClient,
    active: &ActiveRun,
    consent_directory: &std::path::Path,
    personal: bool,
    policy: &str,
    stop: &CancellationToken,
    decisions: &SessionConsentDecisions,
) -> Result<()> {
    let request = client.websocket_request("connect")?;
    let (mut socket, _) = tokio::select! { biased;
        _ = stop.cancelled() => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(10), connect_async_with_config(request, Some(socket_config()), false)) => result.context("Device desktop connection timed out")?.context("Device desktop connection failed")?,
    };
    let connection_stop = stop.child_token();
    let mut streams = JoinSet::new();
    let mut hello = false;
    let result: Result<()> = async {
        loop {
            tokio::select! { biased;
                _ = stop.cancelled() => return Ok(()),
                done = streams.join_next(), if !streams.is_empty() => {
                    if let Some(Err(error)) = done { eprintln!("Device desktop stream worker failed: {error}"); }
                },
                message = socket.next() => {
                    let message = message.context("Device desktop channel closed")??;
                    match message {
                        Message::Text(raw) => {
                            if raw.len() > 1024 * 1024 { bail!("Device desktop message exceeds its size limit"); }
                            let message: ServerMessage = serde_json::from_str(&raw).context("Invalid device desktop message")?;
                            match message {
                                ServerMessage::Hello { schema_version, executor_id } => {
                                    if hello || schema_version != 2 || executor_id != client.executor_id() {
                                        bail!("Device desktop channel identity mismatch");
                                    }
                                    hello = true;
                                },
                                ServerMessage::DesktopStreamRequested { run_id, session_id, stream_id, control } => {
                                    if !hello || streams.len() >= 8 || run_id.is_nil() || session_id.is_nil() || stream_id.is_nil() {
                                        bail!("Invalid or excessive device desktop stream request");
                                    }
                                    let current = active.lock().await.clone();
                                    let Some((run, lease, run_stop)) = current.filter(|(run, _, stop)| *run == run_id && !stop.is_cancelled()) else {
                                        continue;
                                    };
                                    let client = client.clone();
                                    let active = active.clone();
                                    let directory = consent_directory.to_path_buf();
                                    let decisions = decisions.clone();
                                    let stream_stop = connection_stop.child_token();
                                    let policy = policy.to_owned();
                                    streams.spawn(async move {
                                        let result = serve_stream(&client, &active, &directory, personal, &policy,
                                            &decisions, run, lease, session_id, stream_id, control,
                                            &run_stop, &stream_stop).await;
                                        if let Err(error) = result { eprintln!("Device desktop stream ended: {error}"); }
                                    });
                                },
                                ServerMessage::Ack {} => {},
                                ServerMessage::Error {} => {
                                    if let Some((_, _, run_stop)) = active.lock().await.as_ref() {
                                        run_stop.cancel();
                                    }
                                    bail!("Device desktop authority rejected");
                                },
                            }
                        },
                        Message::Ping(bytes) => {
                            tokio::select! { biased;
                                _ = stop.cancelled() => return Ok(()),
                                result = tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Pong(bytes))) => result.context("Device desktop pong timed out")??,
                            }
                        },
                        Message::Pong(_) => {},
                        Message::Close(_) => return Ok(()),
                        _ => bail!("Unexpected device desktop channel data"),
                    }
                },
            }
        }
    }.await;
    connection_stop.cancel();
    while streams.join_next().await.is_some() {}
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn serve_stream(
    client: &AgentClient,
    active: &ActiveRun,
    directory: &std::path::Path,
    personal: bool,
    policy: &str,
    decisions: &SessionConsentDecisions,
    run: Uuid,
    lease: Uuid,
    session: Uuid,
    stream: Uuid,
    control: bool,
    run_stop: &CancellationToken,
    stream_stop: &CancellationToken,
) -> Result<()> {
    let stop = stream_stop.child_token();
    let work = async {
        let key = (run, lease, session, control);
        let allowed = if !personal || policy == "unattended" {
            true
        } else if policy == "off" {
            false
        } else {
            let cached = decisions.lock().await.get(&key).copied();
            if let Some(allowed) = cached {
                allowed
            } else {
                let decision = tokio::time::timeout(Duration::from_secs(105), local_consent(directory,
                        run, Uuid::new_v4(), json!({"tool":"remote_desktop","run_id":run,
                            "session_id":session,"stream_id":stream,"control":control,
                            "access":if control { "screen_keyboard_pointer_clipboard" } else { "screen_view_only" }}), &stop)).await;
                let allowed = match decision {
                    Ok(result) => result?,
                    Err(_) => false,
                };
                let mut decisions = decisions.lock().await;
                decisions.retain(|(known_run, known_lease, _, _), _| {
                    *known_run == run && *known_lease == lease
                });
                decisions.insert(key, allowed);
                if allowed && control {
                    decisions.insert((run, lease, session, false), true);
                }
                allowed
            }
        };
        if !allowed || !available() {
            // Consuming and immediately closing the reverse socket ends the
            // server's waiting stream promptly. No framebuffer is sent.
            let request = client.websocket_request(&format!("desktop-streams/{stream}"))?;
            if let Ok(Ok((mut socket, _))) = tokio::time::timeout(
                Duration::from_secs(5),
                connect_async_with_config(request, Some(socket_config()), false),
            )
            .await
            {
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
            }
            return Ok(());
        }
        let request = client.websocket_request(&format!("desktop-streams/{stream}"))?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            connect_async_with_config(request, Some(socket_config()), false),
        )
        .await
        .context("Device reverse desktop connection timed out")?
        .context("Device reverse desktop connection failed")?;
        #[cfg(any(windows, target_os = "linux"))]
        {
            native::serve(socket, control, stop.clone()).await
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            drop(socket);
            bail!("Native desktop streaming is unavailable on this platform")
        }
    };
    let check = async {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            timer.tick().await;
            if active.lock().await.as_ref().is_none_or(
                |(current_run, current_lease, current_stop)| {
                    *current_run != run || *current_lease != lease || current_stop.is_cancelled()
                },
            ) {
                return;
            }
        }
    };
    let result = tokio::select! { biased;
        _ = run_stop.cancelled() => Ok(()),
        _ = stream_stop.cancelled() => Ok(()),
        _ = check => Ok(()),
        result = work => result,
    };
    stop.cancel();
    result
}
