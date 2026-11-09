//! Explicit MCP 2024-11-05 HTTP+SSE. The original GET stream is retained; no reconnect or POST replay.
use super::*;
use std::collections::VecDeque;
#[cfg(test)]
#[path = "legacy_sse_tests.rs"]
mod tests;

pub(super) struct Connection {
    response: Response,
    decoder: Decoder,
    pending: VecDeque<Frame>,
    post_url: Url,
}
enum Frame {
    Endpoint(String),
    Message(Value),
}
impl Connection {
    pub(super) fn endpoint(&self) -> &str {
        self.post_url.as_str()
    }
    async fn connect(client: &Client, endpoint: &Endpoint, limits: Limits) -> Result<Self> {
        let response = client
            .get(endpoint.url.clone())
            .headers(endpoint.headers.clone())
            .header(ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(status_error(&response));
        }
        let mut types = response.headers().get_all(CONTENT_TYPE).iter();
        if types
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("text/event-stream")
            || types.next().is_some()
        {
            return Err(Error::Protocol);
        }
        let mut current = Self {
            response,
            decoder: Decoder::new(limits.max_response_bytes, limits.max_messages),
            pending: Default::default(),
            post_url: endpoint.url.clone(),
        };
        let mut total = 0usize;
        loop {
            let (frames, n) = current.read().await?;
            total = total.checked_add(n).ok_or(Error::Limit)?;
            if total > limits.max_response_bytes {
                return Err(Error::Limit);
            }
            let mut found = false;
            for frame in frames {
                match frame {
                    Frame::Endpoint(uri) if !found => {
                        let url = endpoint.url.join(&uri).map_err(|_| Error::Protocol)?;
                        if url.scheme() != endpoint.url.scheme()
                            || url.host_str() != endpoint.url.host_str()
                            || url.port_or_known_default() != endpoint.url.port_or_known_default()
                            || !url.username().is_empty()
                            || url.password().is_some()
                            || url.fragment().is_some()
                            || url.as_str().len() > 4096
                        {
                            return Err(Error::Protocol);
                        }
                        current.post_url = url;
                        found = true;
                    }
                    Frame::Endpoint(_) => return Err(Error::Protocol),
                    Frame::Message(value) if found => {
                        current.pending.push_back(Frame::Message(value))
                    }
                    Frame::Message(_) => return Err(Error::Protocol),
                }
            }
            if found {
                return Ok(current);
            }
        }
    }
    async fn read(&mut self) -> Result<(Vec<Frame>, usize)> {
        let chunk = self
            .response
            .chunk()
            .await
            .map_err(|_| Error::Transport)?
            .ok_or(Error::SessionUnavailable)?;
        let frames = self.decoder.push(&chunk)?;
        Ok((frames, chunk.len()))
    }
    async fn messages(&mut self) -> Result<(Vec<Frame>, usize)> {
        if !self.pending.is_empty() {
            return Ok((self.pending.drain(..).collect(), 0));
        }
        self.read().await
    }
}
impl Transport {
    async fn ensure_legacy(&mut self) -> Result<()> {
        if self.legacy.is_none() {
            self.legacy =
                Some(Connection::connect(&self.client, &self.endpoint, self.limits).await?);
        }
        Ok(())
    }
    async fn legacy_post(&mut self, body: &Value) -> Result<()> {
        self.ensure_legacy().await?;
        let url = self
            .legacy
            .as_ref()
            .ok_or(Error::SessionUnavailable)?
            .post_url
            .clone();
        let response = self
            .client
            .post(url)
            .headers(self.endpoint.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header("MCP-Protocol-Version", "2024-11-05")
            .json(body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(status_error(&response));
        }
        // The official legacy server SDK returns a bounded "Accepted" body.
        // It is only a transport ACK; the actual RPC response remains on the original SSE stream.
        let _ = bounded_body(response, 16 * 1024).await?;
        Ok(())
    }
    pub(super) async fn legacy_notify(&mut self, body: Value) -> Result<()> {
        tokio::time::timeout(
            Duration::from_millis(self.limits.timeout_ms),
            self.legacy_post(&body),
        )
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub(super) async fn legacy_request(
        &mut self,
        method: &str,
        params: Value,
        request_id: String,
    ) -> Result<Reply> {
        self.legacy_post(&json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":params}))
            .await?;
        let mut total = 0usize;
        let mut count = 0usize;
        let mut catalog_changed = false;
        loop {
            let (frames, n) = self
                .legacy
                .as_mut()
                .ok_or(Error::SessionUnavailable)?
                .messages()
                .await?;
            total = total.checked_add(n).ok_or(Error::Limit)?;
            if total > self.limits.max_response_bytes {
                return Err(Error::Limit);
            }
            let mut response = None;
            for frame in frames {
                count += 1;
                if count > self.limits.max_messages {
                    return Err(Error::Limit);
                }
                let Frame::Message(value) = frame else {
                    return Err(Error::Protocol);
                };
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
                            .is_some_and(|v| !v.is_object() && !v.is_array())
                    {
                        return Err(Error::Protocol);
                    }
                    if let Some(id) = value.get("id") {
                        valid_id(id)?;
                        let reply = if method == "ping" {
                            json!({"jsonrpc":"2.0","id":id,"result":{}})
                        } else {
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                        };
                        self.legacy_post(&reply).await?;
                    } else if method == "notifications/tools/list_changed" {
                        catalog_changed = true;
                    }
                } else {
                    if response.is_some() {
                        return Err(Error::Protocol);
                    }
                    response = Some(response_message(value, &request_id, catalog_changed)?);
                }
            }
            if let Some(mut reply) = response {
                reply.catalog_changed |= catalog_changed;
                return Ok(reply);
            }
        }
    }
}
struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    kind: Option<String>,
    after_cr: bool,
    first: bool,
    limit: usize,
    messages: usize,
}
impl Decoder {
    fn new(limit: usize, messages: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            kind: None,
            after_cr: false,
            first: true,
            limit,
            messages,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if matches!(*byte, b'\r' | b'\n') {
                self.after_cr = *byte == b'\r';
                if let Some(frame) = self.end_line()? {
                    if frames.len() >= self.messages {
                        return Err(Error::Limit);
                    }
                    frames.push(frame);
                }
            } else {
                if self.line.len() + self.data.len() >= self.limit {
                    return Err(Error::Limit);
                }
                self.line.push(*byte);
            }
        }
        Ok(frames)
    }
    fn end_line(&mut self) -> Result<Option<Frame>> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = std::str::from_utf8(&bytes).map_err(|_| Error::Protocol)?;
        if self.first {
            self.first = false;
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            let kind = self.kind.take().unwrap_or_else(|| "message".into());
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop();
            let data = std::mem::take(&mut self.data);
            return match kind.as_str() {
                "endpoint" => {
                    let uri = std::str::from_utf8(&data).map_err(|_| Error::Protocol)?;
                    if uri.is_empty() || uri.len() > 4096 || uri.chars().any(char::is_control) {
                        return Err(Error::Protocol);
                    }
                    Ok(Some(Frame::Endpoint(uri.into())))
                }
                "message" => Ok(Some(Frame::Message(wire::parse(&data)?))),
                _ => Err(Error::Protocol),
            };
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => {
                if self.kind.is_some() || !matches!(value, "endpoint" | "message") {
                    return Err(Error::Protocol);
                }
                self.kind = Some(value.into());
            }
            "data" => {
                if self.data.len() + value.len() + 1 > self.limit {
                    return Err(Error::Limit);
                }
                self.data.extend_from_slice(value.as_bytes());
                self.data.push(b'\n');
            }
            // Reconnect and retry fields never grant another GET or POST.
            "id" | "retry" => {}
            _ => {}
        }
        Ok(None)
    }
}
