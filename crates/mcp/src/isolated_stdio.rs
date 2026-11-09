//! Original newline-delimited MCP pipes supplied by an independently owned isolated guest.
//! This module never launches a host process, reconnects, or constructs process authority.
use super::{Reply, envelope, response_message, valid_id};
use crate::interactions::{TaskInteractionHandler, TaskRequestContext, TaskServerRequest};
use crate::{Error, Limits, Result, wire};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub(super) struct Connection {
    read: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
    write: Box<dyn AsyncWrite + Send + Unpin>,
    limits: Limits,
}
impl Connection {
    pub(super) fn new(
        read: impl AsyncRead + Send + Unpin + 'static,
        write: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self {
            read: BufReader::new(Box::new(read)),
            write: Box::new(write),
            limits: Limits::default(),
        }
    }
    pub(super) fn limits(&mut self, limits: Limits) {
        self.limits = limits;
    }
    pub(super) async fn notify(&mut self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Protocol)?;
        if bytes.len() > self.limits.max_response_bytes {
            return Err(Error::Limit);
        }
        bytes.push(b'\n');
        self.write
            .write_all(&bytes)
            .await
            .map_err(|_| Error::Transport)?;
        self.write.flush().await.map_err(|_| Error::Transport)
    }
    async fn message(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            let byte = self.read.read_u8().await.map_err(|_| Error::Transport)?;
            if byte == b'\n' {
                break;
            }
            if bytes.len() >= self.limits.max_response_bytes {
                return Err(Error::Limit);
            }
            bytes.push(byte);
        }
        if bytes.is_empty() {
            return Err(Error::Protocol);
        }
        wire::parse(&bytes)
    }
    pub(super) async fn request(
        &mut self,
        method: &str,
        params: Value,
        id: String,
        interaction: Option<(&TaskRequestContext, &dyn TaskInteractionHandler)>,
    ) -> Result<Reply> {
        self.notify(&json!({"jsonrpc":"2.0","method":method,"params":params,"id":id}))
            .await?;
        let mut total = 0usize;
        let mut changed = false;
        for _ in 0..self.limits.max_messages {
            let value = self.message().await?;
            total = total
                .checked_add(
                    serde_json::to_vec(&value)
                        .map_err(|_| Error::Protocol)?
                        .len(),
                )
                .ok_or(Error::Limit)?;
            if total > self.limits.max_response_bytes {
                return Err(Error::Limit);
            }
            envelope(&value)?;
            if let Some(method) = value.get("method") {
                let method = method
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or(Error::Protocol)?;
                if value.get("result").is_some()
                    || value.get("error").is_some()
                    || value
                        .get("params")
                        .is_some_and(|p| !p.is_object() && !p.is_array())
                {
                    return Err(Error::Protocol);
                }
                if let Some(id) = value.get("id") {
                    valid_id(id)?;
                    if matches!(method, "sampling/createMessage" | "elicitation/create")
                        && interaction.is_some()
                    {
                        let (context, handler) = interaction.ok_or(Error::Protocol)?;
                        let request = TaskServerRequest::checked(value.clone(), context)?;
                        let prepared = handler.respond(request).await?;
                        handler.validate_response(&prepared).await?;
                        let mut response = prepared.body().clone();
                        response["jsonrpc"] = json!("2.0");
                        response["id"] = id.clone();
                        match self.notify(&response).await{Ok(())=>handler.acknowledge(&prepared,json!({"transport":"isolated_stdio","original_write_flushed":true,"remote_receipt_unavailable":true,"remote_effects_unknown":true})).await?,Err(error)=>{let _=handler.unknown(&prepared).await;return Err(error);}}
                        continue;
                    }
                    let response = if method == "ping" {
                        json!({"jsonrpc":"2.0","id":id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                    };
                    self.notify(&response).await?;
                } else if method == "notifications/tools/list_changed" {
                    changed = true;
                }
            } else {
                return response_message(value, &id, changed);
            }
        }
        Err(Error::Limit)
    }
}
