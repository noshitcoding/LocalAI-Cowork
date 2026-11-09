//! Frame forwarding only; no private control payload supplies a callback URL.
use anyhow::{ensure, Result};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message as RunnerMessage;

use super::transport::{self, FrameReader, PeerSocket};

pub(crate) type DesktopSocket = PeerSocket;

pub(crate) async fn user(
    socket: WebSocket,
    peer: PeerSocket,
    control: bool,
) -> Result<(u64, u64, Option<String>)> {
    let (mut ws_send, mut ws_recv) = socket.split();
    let (mut read, mut write) = tokio::io::split(peer);
    let mut frames = FrameReader::default();
    let mut messages = 0;
    let mut bytes = 0;
    let mut digest = Sha256::new();
    loop {
        tokio::select! {
            incoming=ws_recv.next()=>match incoming{
                Some(Ok(Message::Binary(data)))=>{
                    if control {messages+=1;bytes+=data.len() as u64;digest.update(&data);}
                    tokio::time::timeout(std::time::Duration::from_secs(3),transport::write_frames(&mut write,&data)).await??;
                },
                Some(Ok(Message::Ping(data)))=>ws_send.send(Message::Pong(data)).await?,
                Some(Ok(Message::Close(_)))|None=>break,
                Some(Err(error))=>return Err(error.into()),_=>{},
            },
            incoming=frames.read_frame(&mut read)=>match incoming?{
                Some(data)=>tokio::time::timeout(std::time::Duration::from_secs(3),ws_send.send(Message::Binary(data.into()))).await??,
                None=>break,
            },
        }
    }
    Ok((
        messages,
        bytes,
        control.then(|| hex::encode(digest.finalize())),
    ))
}

pub(crate) async fn executor(socket: WebSocket, peer: PeerSocket) -> Result<()> {
    user(socket, peer, false).await.map(|_| ())
}

pub(super) async fn owner(user: PeerSocket, executor: DesktopSocket) -> Result<()> {
    let (mut user_read, mut user_write) = tokio::io::split(user);
    let (mut peer_read, mut peer_write) = tokio::io::split(executor);
    let mut user_frames = FrameReader::default();
    let mut peer_frames = FrameReader::default();
    loop {
        tokio::select! {
            data=user_frames.read_frame(&mut user_read)=>match data? {Some(bytes)=>tokio::time::timeout(std::time::Duration::from_secs(3),transport::write_frames(&mut peer_write,&bytes)).await??,None=>break},
            data=peer_frames.read_frame(&mut peer_read)=>match data? {Some(bytes)=>tokio::time::timeout(std::time::Duration::from_secs(3),transport::write_frames(&mut user_write,&bytes)).await??,None=>break},
        }
    }
    Ok(())
}

pub(super) async fn runner(
    user: PeerSocket,
    runner: &crate::desktop::RunnerControl,
    pool: &sqlx::PgPool,
    session: uuid::Uuid,
    control: bool,
) -> Result<()> {
    let binding = super::linux::binding(pool, session).await?;
    super::linux::guard_job(pool,&binding,async {
    let socket=runner.proven_stream(&binding,control).await?;
    let (mut runner_send, mut runner_recv) = socket.split();
    let (mut user_read, mut user_write) = tokio::io::split(user);
    let mut user_frames = FrameReader::default();
    loop {
        tokio::select! {
            data=user_frames.read_frame(&mut user_read)=>match data? {
                Some(bytes)=>tokio::time::timeout(std::time::Duration::from_secs(3),runner_send.send(RunnerMessage::Binary(bytes.into()))).await??,None=>break},
            incoming=runner_recv.next()=>match incoming {
                Some(Ok(RunnerMessage::Binary(bytes)))=>tokio::time::timeout(std::time::Duration::from_secs(3),transport::write_frames(&mut user_write,&bytes)).await??,
                Some(Ok(RunnerMessage::Ping(data)))=>runner_send.send(RunnerMessage::Pong(data)).await?,
                Some(Ok(RunnerMessage::Close(_)))|None=>break,
                Some(Err(error))=>return Err(error.into()),_=>{},
            },
        }
    }
    Ok(())
    }).await
}

pub(super) fn callback_matches(
    pending: &super::tickets::Pending,
    principal: &crate::auth::ExecutorPrincipal,
) -> Result<()> {
    ensure!(
        pending.owner.executor_id == Some(principal.executor_id)
            && pending.owner.credential_id == Some(principal.credential_id),
        "private callback belongs to a different executor credential"
    );
    Ok(())
}
