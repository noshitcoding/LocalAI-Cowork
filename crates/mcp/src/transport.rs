use crate::interactions::{TaskInteractionHandler, TaskRequestContext, TaskServerRequest};
use crate::{Error, PROTOCOL_VERSION, Result, wire};
use futures_util::StreamExt;
use reqwest::{
    Client, Response, Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue},
};
use serde_json::{Value, json};
use std::{net::IpAddr, time::Duration};
#[path = "isolated_stdio.rs"]
mod isolated_stdio;
#[path = "legacy_sse.rs"]
mod legacy_sse;
#[path = "task_http.rs"]
mod task_http;

/// Private credential-bearing configuration: deliberately neither Debug nor Serialize.
pub struct Endpoint {
    url: Url,
    authorization: Option<HeaderValue>,
    headers: reqwest::header::HeaderMap,
    addresses: Option<Vec<std::net::SocketAddr>>,
    legacy: bool,
    stdio: Option<tokio::sync::Mutex<isolated_stdio::Connection>>,
}
impl Endpoint {
    pub fn url(&self) -> &str {
        self.url.as_str()
    }
    /// HTTP is allowed only for explicit loopback IP literals (no DNS inference).
    /// Credentials must come from the native secret store, never model arguments.
    pub fn new(url: &str, bearer: Option<&str>) -> Result<Self> {
        let url = Url::parse(url).map_err(|_| Error::Configuration)?;
        let loopback = url.host_str().is_some_and(|s| {
            s.trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Configuration);
        }
        let authorization = match bearer {
            Some(token)
                if !token.is_empty()
                    && token.len() <= 4096
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)) =>
            {
                let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| Error::Configuration)?;
                header.set_sensitive(true);
                Some(header)
            }
            Some(_) => return Err(Error::Configuration),
            None => None,
        };
        Ok(Self {
            url,
            authorization,
            headers: Default::default(),
            addresses: None,
            legacy: false,
            stdio: None,
        })
    }
    /// Only a trusted isolated runner may provide these original pipes. This does not launch a host process or grant filesystem authority.
    pub fn isolated_stdio(
        nonce: uuid::Uuid,
        read: impl tokio::io::AsyncRead + Send + Unpin + 'static,
        write: impl tokio::io::AsyncWrite + Send + Unpin + 'static,
    ) -> Result<Self> {
        if nonce.is_nil() {
            return Err(Error::Configuration);
        }
        let mut endpoint = Self::new("https://isolated.invalid", None)?;
        endpoint.url = Url::parse(&format!("cowork-isolated-stdio://{nonce}"))
            .map_err(|_| Error::Configuration)?;
        endpoint.stdio = Some(tokio::sync::Mutex::new(isolated_stdio::Connection::new(
            read, write,
        )));
        Ok(endpoint)
    }
    /// Explicit Android/server binding. Resolve once, require public addresses, then pin those TLS addresses for the complete connection lifetime.
    pub async fn public_https(
        url: &str,
        values: &std::collections::BTreeMap<String, String>,
        legacy: bool,
    ) -> Result<Self> {
        let mut endpoint = Self::new(url, None)?;
        let host = endpoint
            .url
            .host_str()
            .ok_or(Error::Configuration)?
            .to_owned();
        if endpoint.url.scheme() != "https"
            || endpoint.url.port_or_known_default() != Some(443)
            || host.parse::<IpAddr>().is_ok()
            || !host.contains('.')
            || host.ends_with('.')
            || values.len() > 64
            || values.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 32768
        {
            return Err(Error::Configuration);
        }
        let addresses = public_https_addresses(&endpoint.url).await?;
        let mut names = std::collections::BTreeSet::new();
        for (name, value) in values {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::Configuration)?;
            if !names.insert(name.as_str().to_owned())
                || [
                    "host",
                    "content-length",
                    "transfer-encoding",
                    "connection",
                    "accept",
                    "content-type",
                    "mcp-session-id",
                    "mcp-protocol-version",
                    "proxy-authorization",
                    "proxy-connection",
                    "te",
                    "trailer",
                    "upgrade",
                ]
                .contains(&name.as_str())
            {
                return Err(Error::Configuration);
            }
            let mut value = HeaderValue::from_str(value).map_err(|_| Error::Configuration)?;
            value.set_sensitive(true);
            endpoint.headers.insert(name, value);
        }
        endpoint.addresses = Some(addresses);
        endpoint.legacy = legacy;
        Ok(endpoint)
    }
}
/// Resolve an untrusted server-side HTTPS destination exactly once. Callers
/// must pin these addresses on a no-proxy, no-redirect client; allowing the
/// client to resolve the hostname again would undo the rebinding protection.
/// Non-secret query parameters are allowed; credential custody is the caller's
/// responsibility. The same address policy protects server MCP connections.
pub async fn public_https_addresses(url: &Url) -> Result<Vec<std::net::SocketAddr>> {
    let host = public_https_host(url)?;
    let addresses =
        tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443)))
            .await
            .map_err(|_| Error::Deadline)?
            .map_err(|_| Error::Transport)?
            .collect::<Vec<_>>();
    checked_public_addresses(addresses)
}
fn public_https_host(url: &Url) -> Result<&str> {
    let host = url.host_str().ok_or(Error::Configuration)?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
        || !host.contains('.')
        || host.ends_with('.')
    {
        return Err(Error::Configuration);
    }
    Ok(host)
}
fn checked_public_addresses(
    addresses: Vec<std::net::SocketAddr>,
) -> Result<Vec<std::net::SocketAddr>> {
    if addresses.is_empty()
        || addresses.len() > 64
        || addresses
            .iter()
            .any(|v| v.port() != 443 || !public_address(v.ip()))
    {
        return Err(Error::Configuration);
    }
    Ok(addresses)
}
pub(crate) fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && (b == 168 || b == 0 || b == 2 || b == 88 && c == 99)
                || a == 100 && (64..=127).contains(&b)
                || a == 198 && (b == 18 || b == 19 || b == 51 && c == 100)
                || a == 203 && b == 0 && c == 113
                || [a, b, c, d] == [255, 255, 255, 255])
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            ip.to_ipv4_mapped().map_or_else(
                || {
                    segments[0] & 0xe000 == 0x2000
                        && segments[0] != 0x2002
                        && !(segments[0] == 0x2001 && segments[1] <= 0x1ff)
                        && !(segments[0] == 0x2001 && segments[1] == 0xdb8)
                },
                |v| public_address(IpAddr::V4(v)),
            )
        }
    }
}

#[cfg(test)]
mod public_destination_tests {
    use super::*;
    #[test]
    fn public_https_destination_rejects_literal_internal_and_alternate_origins() {
        for value in [
            "http://memory.example.test",
            "https://127.0.0.1",
            "https://0x7f000001",
            "https://2130706433",
            "https://[::1]",
            "https://[::ffff:127.0.0.1]",
            "https://169.254.169.254",
            "https://10.0.0.1",
            "https://localhost",
            "https://memory.example.test:8443",
            "https://memory.example.test.",
            "https://user@memory.example.test",
            "https://memory.example.test/#fragment",
        ] {
            assert!(
                public_https_host(&Url::parse(value).unwrap()).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            public_https_host(
                &Url::parse("https://memory.example.test/items?collection=public").unwrap()
            )
            .unwrap(),
            "memory.example.test"
        );
    }
    #[test]
    fn every_original_dns_answer_must_be_public_before_addresses_are_pinned() {
        let public: std::net::SocketAddr = "93.184.216.34:443".parse().unwrap();
        assert_eq!(
            checked_public_addresses(vec![public]).unwrap(),
            vec![public]
        );
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            let denied = std::net::SocketAddr::new(ip.parse().unwrap(), 443);
            assert!(
                checked_public_addresses(vec![public, denied]).is_err(),
                "{ip}"
            );
        }
        assert!(checked_public_addresses(vec![]).is_err());
        assert!(checked_public_addresses(vec![public; 65]).is_err());
        assert!(checked_public_addresses(vec!["93.184.216.34:80".parse().unwrap()]).is_err());
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub timeout_ms: u64,
    pub max_response_bytes: usize,
    pub max_messages: usize,
    pub max_pages: usize,
    pub max_tools: usize,
    pub max_catalog_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_response_bytes: 1024 * 1024,
            max_messages: 128,
            max_pages: 16,
            max_tools: 128,
            max_catalog_bytes: 2 * 1024 * 1024,
        }
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        if !(1..=120_000).contains(&self.timeout_ms)
            || !(256..=4 * 1024 * 1024).contains(&self.max_response_bytes)
            || !(1..=256).contains(&self.max_messages)
            || !(1..=32).contains(&self.max_pages)
            || !(1..=256).contains(&self.max_tools)
            || !(256..=4 * 1024 * 1024).contains(&self.max_catalog_bytes)
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}
pub(crate) struct Reply {
    pub result: Result<Value>,
    pub error_evidence: Option<Value>,
    pub catalog_changed: bool,
}
pub(crate) struct Transport {
    client: Client,
    endpoint: Endpoint,
    session_id: Option<HeaderValue>,
    pub limits: Limits,
    legacy: Option<legacy_sse::Connection>,
    stdio_protocol: Option<&'static str>,
}
impl Transport {
    pub(crate) fn replace_bearer(&mut self, bearer: &str) -> Result<()> {
        if self.endpoint.legacy || self.endpoint.stdio.is_some() {
            return Err(Error::Configuration);
        }
        let replacement = Endpoint::new(self.endpoint.url.as_str(), Some(bearer))?;
        self.endpoint.headers.remove(AUTHORIZATION);
        self.endpoint.authorization = replacement.authorization;
        Ok(())
    }
    pub(crate) fn checkpoint_session_id(&self) -> Option<&[u8]> {
        self.session_id.as_ref().map(HeaderValue::as_bytes)
    }
    pub(crate) fn restore_session_id(&mut self, value: Option<&[u8]>) -> Result<()> {
        self.session_id = match value {
            None => None,
            Some(value)
                if !value.is_empty()
                    && value.len() <= 512
                    && value.iter().all(|b| (0x21..=0x7e).contains(b)) =>
            {
                let mut header = HeaderValue::from_bytes(value).map_err(|_| Error::Protocol)?;
                header.set_sensitive(true);
                Some(header)
            }
            Some(_) => return Err(Error::Protocol),
        };
        Ok(())
    }
    pub fn new(mut endpoint: Endpoint, limits: Limits) -> Result<Self> {
        limits.validate()?;
        if let Some(stdio) = endpoint.stdio.as_mut() {
            stdio.get_mut().limits(limits);
        }
        let mut builder = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .http1_only()
            // Avoid pooled half-closed connections and any implicit replay on reuse.
            .pool_max_idle_per_host(0)
            .connect_timeout(Duration::from_millis(limits.timeout_ms));
        if let Some(addresses) = &endpoint.addresses {
            builder = builder.resolve_to_addrs(
                endpoint.url.host_str().ok_or(Error::Configuration)?,
                addresses,
            );
        }
        let client = builder.build().map_err(|_| Error::Configuration)?;
        Ok(Self {
            client,
            endpoint,
            session_id: None,
            limits,
            legacy: None,
            stdio_protocol: None,
        })
    }
    pub fn endpoint_identity(&self) -> &str {
        self.legacy
            .as_ref()
            .map(|v| v.endpoint())
            .unwrap_or(self.endpoint.url.as_str())
    }
    pub fn protocol_version(&self) -> &'static str {
        if let Some(version) = self.stdio_protocol {
            return version;
        }
        if self.endpoint.legacy {
            "2024-11-05"
        } else {
            PROTOCOL_VERSION
        }
    }
    pub fn negotiate_protocol(&mut self, version: &str) -> Result<()> {
        if self.endpoint.legacy {
            return if version == "2024-11-05" {
                Ok(())
            } else {
                Err(Error::Protocol)
            };
        }
        if self.endpoint.stdio.is_some() {
            self.stdio_protocol = Some(match version {
                "2024-11-05" => "2024-11-05",
                "2025-03-26" => "2025-03-26",
                "2025-06-18" => "2025-06-18",
                PROTOCOL_VERSION => PROTOCOL_VERSION,
                _ => return Err(Error::Protocol),
            });
            Ok(())
        } else if matches!(version, PROTOCOL_VERSION | "2025-03-26" | "2025-06-18") {
            self.stdio_protocol = Some(match version {
                "2025-03-26" => "2025-03-26",
                "2025-06-18" => "2025-06-18",
                _ => PROTOCOL_VERSION,
            });
            Ok(())
        } else {
            Err(Error::Protocol)
        }
    }
    pub fn supports_checkpoint(&self) -> bool {
        !self.endpoint.legacy && self.endpoint.stdio.is_none()
    }
    async fn post(&self, body: &Value, initialize: bool) -> Result<Response> {
        let mut request = self
            .client
            .post(self.endpoint.url.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .headers(self.endpoint.headers.clone())
            .json(body);
        if let Some(auth) = &self.endpoint.authorization {
            request = request.header(AUTHORIZATION, auth);
        }
        if !initialize {
            request = request.header("MCP-Protocol-Version", self.protocol_version());
            if let Some(session) = &self.session_id {
                request = request.header("Mcp-Session-Id", session);
            }
        }
        request.send().await.map_err(|_| Error::Transport)
    }
    fn check_headers(&mut self, response: &Response, initialize: bool) -> Result<()> {
        let headers = response.headers().get_all("Mcp-Session-Id");
        let mut values = headers.iter();
        if let Some(value) = values.next() {
            if values.next().is_some()
                || value.as_bytes().is_empty()
                || value.as_bytes().len() > 512
                || !value.as_bytes().iter().all(|b| (0x21..=0x7e).contains(b))
            {
                return Err(Error::Protocol);
            }
            if initialize {
                let mut value = value.clone();
                value.set_sensitive(true);
                self.session_id = Some(value);
            } else if self.session_id.as_ref() != Some(value) {
                return Err(Error::Protocol);
            }
        }
        Ok(())
    }
    pub async fn notify(&mut self, body: Value) -> Result<()> {
        if let Some(stdio) = self.endpoint.stdio.as_mut() {
            return tokio::time::timeout(
                Duration::from_millis(self.limits.timeout_ms),
                stdio.get_mut().notify(&body),
            )
            .await
            .map_err(|_| Error::Deadline)?;
        }
        if self.endpoint.legacy {
            return self.legacy_notify(body).await;
        }
        tokio::time::timeout(Duration::from_millis(self.limits.timeout_ms), async {
            let response = self.post(&body, false).await?;
            self.check_headers(&response, false)?;
            if response.status() != reqwest::StatusCode::ACCEPTED {
                return Err(status_error(&response));
            }
            // A notification acknowledgement is empty; don't accept a hidden second response.
            let bytes = bounded_body(response, 1).await?;
            if !bytes.is_empty() {
                return Err(Error::Protocol);
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        initialize: bool,
    ) -> Result<Reply> {
        self.request_with_id(method, params, initialize, uuid::Uuid::new_v4().to_string())
            .await
    }
    pub async fn request_with_id(
        &mut self,
        method: &str,
        params: Value,
        initialize: bool,
        request_id: String,
    ) -> Result<Reply> {
        tokio::time::timeout(
            Duration::from_millis(self.limits.timeout_ms),
            self.request_inner(method, params, initialize, request_id, None),
        )
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub async fn request_task_with_id(
        &mut self,
        method: &str,
        params: Value,
        request_id: String,
        deadline_ms: u64,
    ) -> Result<Reply> {
        if !matches!(method, "tasks/get" | "tasks/result" | "tasks/cancel")
            || !(1..=86_400_000).contains(&deadline_ms)
        {
            return Err(Error::Configuration);
        }
        // The product's original run/task deadline limits this single RPC. It
        // does not extend that authority or change the underlying tool timeout.
        tokio::time::timeout(
            Duration::from_millis(deadline_ms),
            self.request_inner(method, params, false, request_id, None),
        )
        .await
        .map_err(|_| Error::Deadline)?
    }
    pub(crate) async fn request_task_interactive(
        &mut self,
        method: &str,
        params: Value,
        request_id: String,
        deadline_ms: u64,
        context: TaskRequestContext,
        handler: &dyn TaskInteractionHandler,
    ) -> Result<Reply> {
        if !matches!(method, "tasks/get" | "tasks/result" | "tasks/cancel")
            || !(1..=86_400_000).contains(&deadline_ms)
            || context.operation_rpc_id != request_id
        {
            return Err(Error::Configuration);
        }
        tokio::time::timeout(
            Duration::from_millis(deadline_ms),
            self.request_inner(method, params, false, request_id, Some((&context, handler))),
        )
        .await
        .map_err(|_| Error::Deadline)?
    }
    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
        initialize: bool,
        request_id: String,
        interaction: Option<(&TaskRequestContext, &dyn TaskInteractionHandler)>,
    ) -> Result<Reply> {
        if let Some(stdio) = self.endpoint.stdio.as_mut() {
            return stdio
                .get_mut()
                .request(method, params, request_id, interaction)
                .await;
        }
        if self.endpoint.legacy {
            return self.legacy_request(method, params, request_id).await;
        }
        if let Some((context, handler)) = interaction {
            return self
                .request_interactive_http(method, params, request_id, context, handler)
                .await;
        }
        let response = self
            .post(
                &json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":params}),
                initialize,
            )
            .await?;
        self.check_headers(&response, initialize)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(status_error(&response));
        }
        let mut types = response.headers().get_all(CONTENT_TYPE).iter();
        let content_type = types
            .next()
            .and_then(|v| v.to_str().ok())
            .ok_or(Error::Protocol)?;
        if types.next().is_some() {
            return Err(Error::Protocol);
        }
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if response
            .content_length()
            .is_some_and(|n| n > self.limits.max_response_bytes as u64)
        {
            return Err(Error::Limit);
        }
        match mime.as_str() {
            "application/json" => {
                let bytes = bounded_body(response, self.limits.max_response_bytes).await?;
                let value = wire::parse(&bytes)?;
                response_message(value, &request_id, false)
            }
            "text/event-stream" => {
                let mut stream = response.bytes_stream();
                let mut parser =
                    wire::Sse::new(self.limits.max_response_bytes, self.limits.max_messages);
                let mut total = 0;
                let mut count = 0;
                let mut catalog_changed = false;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| Error::Transport)?;
                    total += chunk.len();
                    if total > self.limits.max_response_bytes {
                        return Err(Error::Limit);
                    }
                    for value in parser.push(&chunk)? {
                        count += 1;
                        if count > self.limits.max_messages {
                            return Err(Error::Limit);
                        }
                        envelope(&value)?;
                        if let Some(method) = value.get("method") {
                            let method = method.as_str().ok_or(Error::Protocol)?;
                            if method.is_empty()
                                || method.len() > 256
                                || value.get("result").is_some()
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
                                    let request =
                                        TaskServerRequest::checked(value.clone(), context)?;
                                    let prepared = handler.respond(request).await?;
                                    handler.validate_response(&prepared).await?;
                                    let mut reply = prepared.body().clone();
                                    reply["jsonrpc"] = json!("2.0");
                                    reply["id"] = id.clone();
                                    match self.notify(reply).await {
                                        Ok(())=>handler.acknowledge(&prepared,json!({"transport":"streamable_http","status":202,"empty_body":true,"remote_effects_unknown":true})).await?,
                                        Err(error)=>{let _=handler.unknown(&prepared).await;return Err(error);}
                                    }
                                    continue;
                                }
                                let reply = if method == "ping" {
                                    json!({"jsonrpc":"2.0","id":id,"result":{}})
                                } else {
                                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                                };
                                // Unsupported or unbound requests never reach a privileged handler.
                                self.notify(reply).await?;
                            } else if method == "notifications/tools/list_changed" {
                                catalog_changed = true;
                            }
                        } else {
                            return response_message(value, &request_id, catalog_changed);
                        }
                    }
                }
                Err(Error::Protocol)
            }
            _ => Err(Error::Protocol),
        }
    }
}
fn envelope(value: &Value) -> Result<()> {
    if !value.is_object() || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(Error::Protocol);
    }
    Ok(())
}
fn valid_id(value: &Value) -> Result<()> {
    if value
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 256)
        || value.is_i64()
        || value.is_u64()
    {
        Ok(())
    } else {
        Err(Error::Protocol)
    }
}
fn response_message(value: Value, id: &str, catalog_changed: bool) -> Result<Reply> {
    envelope(&value)?;
    if value.get("method").is_some()
        || value.get("id").and_then(Value::as_str) != Some(id)
        || value.get("result").is_some() == value.get("error").is_some()
    {
        return Err(Error::Protocol);
    }
    if let Some(result) = value.get("result") {
        if !result.is_object() {
            return Err(Error::Protocol);
        }
        Ok(Reply {
            result: Ok(result.clone()),
            error_evidence: None,
            catalog_changed,
        })
    } else {
        let error = value.get("error").ok_or(Error::Protocol)?;
        if !error.get("code").is_some_and(|v| v.is_i64())
            || !error.get("message").is_some_and(Value::is_string)
        {
            return Err(Error::Protocol);
        }
        Ok(Reply {
            result: Err(Error::Remote),
            error_evidence: Some(value),
            catalog_changed,
        })
    }
}
fn status_error(response: &Response) -> Error {
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        Error::SessionUnavailable
    } else {
        Error::Transport
    }
}
async fn bounded_body(response: Response, maximum: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::Transport)?;
        if bytes.len() + chunk.len() > maximum {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
