//! Wire fields match Agent Sandbox v1.0.7 packages/sandboxd/spec/process/v1.
//! Kept explicit to avoid a host protoc dependency; protocol test fixes the tags.
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::time::Instant;
use tonic::{Request, transport::Channel};

#[derive(Clone, PartialEq, prost::Message)]
pub struct ProcessConfig {
    #[prost(string, repeated, tag = "1")]
    pub command: Vec<String>,
    #[prost(map = "string, string", tag = "2")]
    pub env_vars: HashMap<String, String>,
    #[prost(string, optional, tag = "3")]
    pub cwd: Option<String>,
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct Pty {
    #[prost(uint32, tag = "1")]
    pub cols: u32,
    #[prost(uint32, tag = "2")]
    pub rows: u32,
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct StartRequest {
    #[prost(message, optional, tag = "1")]
    pub config: Option<ProcessConfig>,
    #[prost(message, optional, tag = "2")]
    pub pty: Option<Pty>,
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct StartResponse {
    #[prost(oneof = "event::Event", tags = "1,2,3,4")]
    pub event: Option<event::Event>,
}
pub mod event {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Init {
        #[prost(int32, tag = "1")]
        pub process_id: i32,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Exit {
        #[prost(int32, tag = "1")]
        pub exit_code: i32,
    }
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        Init(Init),
        #[prost(bytes, tag = "2")]
        Stdout(Vec<u8>),
        #[prost(bytes, tag = "3")]
        Stderr(Vec<u8>),
        #[prost(message, tag = "4")]
        Exit(Exit),
    }
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct SignalRequest {
    #[prost(int32, tag = "1")]
    pub process_id: i32,
    #[prost(int32, tag = "2")]
    pub signal: i32,
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct Empty {}
#[derive(Clone, PartialEq, prost::Message)]
pub struct StdinRequest {
    #[prost(int32, tag = "1")]
    pub process_id: i32,
    #[prost(oneof = "stdin_payload::Payload", tags = "2,3")]
    pub payload: Option<stdin_payload::Payload>,
}
pub mod stdin_payload {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Payload {
        #[prost(bytes, tag = "2")]
        Input(Vec<u8>),
        #[prost(message, tag = "3")]
        Eof(super::Empty),
    }
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct ResizeRequest {
    #[prost(int32, tag = "1")]
    pub process_id: i32,
    #[prost(uint32, tag = "2")]
    pub cols: u32,
    #[prost(uint32, tag = "3")]
    pub rows: u32,
}
/// The owning supervisor must journal and fence the Pod before starting, retain
/// this stream, and physically stop the Pod even when this stream is disconnected.
pub struct InteractiveSession {
    pub process_id: i32,
    pub events: tonic::codec::Streaming<StartResponse>,
}
#[derive(Default, Debug)]
pub struct Capture {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}
impl Capture {
    fn append(&mut self, bytes: &[u8], bound: usize) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        let keep = bytes.len().min(bound.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&bytes[..keep]);
        self.truncated |= keep < bytes.len();
    }
}
#[derive(Debug)]
pub struct Execution {
    pub stdout: Capture,
    pub stderr: Capture,
    pub exit_code: Option<i32>,
    pub canceled: bool,
    pub timed_out: bool,
    pub complete: bool,
    pub duration_ms: u64,
}
pub struct Runtime {
    host: String,
    token: String,
    http: Client,
    grpc: tonic::client::Grpc<Channel>,
}
fn host_valid(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
pub fn path_valid(path: &str) -> bool {
    path.len() <= 1024
        && !path.contains(['\\', ':', '\0'])
        && (path.is_empty()
            || path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != ".."))
}
impl Runtime {
    pub async fn connect(host: &str, token: &str) -> Result<Self> {
        ensure!(host_valid(host), "Invalid sandbox Service hostname");
        ensure!(
            token.len() >= 48 && !token.chars().any(char::is_whitespace),
            "Runtime token missing"
        );
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        let channel = Channel::from_shared(format!("http://{host}:9090"))?
            .connect_timeout(Duration::from_secs(5))
            .connect()
            .await?;
        let grpc = tonic::client::Grpc::new(channel).max_decoding_message_size(4 * 1024 * 1024);
        let mut runtime = Self {
            host: host.into(),
            token: token.into(),
            http,
            grpc,
        };
        runtime.health().await?;
        Ok(runtime)
    }
    fn request<T>(&self, value: T) -> Result<Request<T>> {
        let mut request = Request::new(value);
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {}", self.token).parse()?);
        Ok(request)
    }
    fn url(&self, path: &str) -> Result<reqwest::Url> {
        ensure!(path_valid(path), "Invalid sandbox file path");
        let mut url = reqwest::Url::parse(&format!("http://{}:8080/v1/files/", self.host))?;
        if !path.is_empty() {
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid file base"))?
                .pop_if_empty()
                .extend(path.split('/'));
        }
        Ok(url)
    }
    pub async fn health(&mut self) -> Result<()> {
        let response = self
            .http
            .get(format!("http://{}:8080/v1/health", self.host))
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "sandboxd health unavailable"
        );
        Ok(())
    }
    pub async fn put(&self, path: &str, bytes: Vec<u8>, mode: &str) -> Result<()> {
        ensure!(matches!(mode, "0644" | "0755"), "Invalid upload mode");
        let mut url = self.url(path)?;
        url.query_pairs_mut().append_pair("mode", mode);
        let response = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "sandboxd file upload failed"
        );
        Ok(())
    }
    pub async fn get(&self, path: &str, bound: usize) -> Result<Vec<u8>> {
        let mut response = self
            .http
            .request(Method::GET, self.url(path)?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        ensure!(response.status().is_success(), "sandboxd file read failed");
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= bound,
                "sandboxd file exceeds transfer bound"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    /// Exactly one Start RPC. Timeout, EOF without Exit, and an uncertain RPC never
    /// cause an Execute/Start replay. The caller always physically suspends the Pod.
    pub async fn execute(
        &mut self,
        command: Vec<String>,
        env_vars: HashMap<String, String>,
        timeout: Duration,
        cancel: &AtomicBool,
        bound: usize,
    ) -> Result<Execution> {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Sandbox execution canceled before dispatch"
        );
        ensure!(
            !command.is_empty()
                && command.len() <= 1024
                && command.iter().all(|s| !s.contains('\0')),
            "Invalid sandbox argv"
        );
        let started = Instant::now();
        let mut request = self.request(StartRequest {
            config: Some(ProcessConfig {
                command,
                env_vars,
                cwd: Some("/workspace".into()),
            }),
            pty: None,
        })?;
        request.set_timeout(timeout);
        tokio::time::timeout(Duration::from_secs(5), self.grpc.ready())
            .await?
            .context("sandboxd gRPC unavailable before launch")?;
        let path = tonic::codegen::http::uri::PathAndQuery::from_static(
            "/process.v1.ProcessService/Start",
        );
        let response: tonic::Response<tonic::codec::Streaming<StartResponse>> = {
            ensure!(
                !cancel.load(Ordering::Acquire),
                "Sandbox execution canceled before dispatch"
            );
            let rpc = self
                .grpc
                .server_streaming(request, path, tonic_prost::ProstCodec::default());
            tokio::pin!(rpc);
            loop {
                tokio::select! {
                    response=&mut rpc=>break response.context("sandboxd Start uncertain; do not replay")?,
                    _=tokio::time::sleep(Duration::from_millis(100))=>{
                        ensure!(!cancel.load(Ordering::Acquire),"Start canceled before acknowledgment; execution uncertain");
                        ensure!(started.elapsed()<timeout,"Start timed out before acknowledgment; execution uncertain");
                    }
                }
            }
        };
        let mut stream = response.into_inner();
        let mut out = Execution {
            stdout: Capture::default(),
            stderr: Capture::default(),
            exit_code: None,
            canceled: false,
            timed_out: false,
            complete: false,
            duration_ms: 0,
        };
        let mut process = None;
        loop {
            if cancel.load(Ordering::Acquire) || started.elapsed() >= timeout {
                out.canceled = cancel.load(Ordering::Acquire);
                out.timed_out = !out.canceled;
                if let Some(process_id) = process {
                    self.signal(process_id, 9).await?;
                }
                // A signal acknowledgment is not a physical-stop receipt.
                break;
            }
            let next = tokio::select! { result=stream.message()=>Some(result),_=tokio::time::sleep(Duration::from_millis(100))=>None };
            let Some(next) = next else { continue };
            let Some(message) =
                next.context("sandboxd process stream failed; execution uncertain")?
            else {
                break;
            };
            match message.event.context("Empty sandboxd process event")? {
                event::Event::Init(init) => {
                    ensure!(
                        process.is_none() && init.process_id > 0,
                        "Invalid duplicate sandboxd Init event"
                    );
                    process = Some(init.process_id);
                }
                event::Event::Stdout(bytes) => {
                    ensure!(process.is_some(), "Output before Init");
                    out.stdout.append(&bytes, bound);
                }
                event::Event::Stderr(bytes) => {
                    ensure!(process.is_some(), "Output before Init");
                    out.stderr.append(&bytes, bound);
                }
                event::Event::Exit(exit) => {
                    ensure!(process.is_some(), "Exit before Init");
                    out.exit_code = Some(exit.exit_code);
                    out.complete = true;
                    break;
                }
            }
        }
        out.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        ensure!(
            out.complete || out.canceled || out.timed_out,
            "Process stream ended without Exit; do not replay"
        );
        Ok(out)
    }
    pub async fn start_interactive(
        &mut self,
        command: Vec<String>,
        env_vars: HashMap<String, String>,
        pty: Option<Pty>,
    ) -> Result<InteractiveSession> {
        ensure!(
            !command.is_empty()
                && command.len() <= 1024
                && command.iter().all(|s| !s.contains('\0')),
            "Invalid interactive argv"
        );
        if let Some(pty) = &pty {
            ensure!(
                (1..=4096).contains(&pty.cols) && (1..=4096).contains(&pty.rows),
                "Invalid terminal dimensions"
            );
        }
        let request = self.request(StartRequest {
            config: Some(ProcessConfig {
                command,
                env_vars,
                cwd: Some("/workspace".into()),
            }),
            pty,
        })?;
        tokio::time::timeout(Duration::from_secs(5), self.grpc.ready()).await??;
        let response: tonic::Response<tonic::codec::Streaming<StartResponse>> =
            tokio::time::timeout(
                Duration::from_secs(30),
                self.grpc.server_streaming(
                    request,
                    tonic::codegen::http::uri::PathAndQuery::from_static(
                        "/process.v1.ProcessService/Start",
                    ),
                    tonic_prost::ProstCodec::default(),
                ),
            )
            .await
            .context("Interactive Start uncertain; never replay")??;
        let mut stream = response.into_inner();
        let first = tokio::time::timeout(Duration::from_secs(30), stream.message())
            .await??
            .context("Interactive Init missing")?;
        let Some(event::Event::Init(init)) = first.event else {
            anyhow::bail!("Interactive Init ordering failed")
        };
        ensure!(init.process_id > 0, "Interactive process identity invalid");
        Ok(InteractiveSession {
            process_id: init.process_id,
            events: stream,
        })
    }
    pub async fn write_stdin(&mut self, process_id: i32, bytes: Vec<u8>, eof: bool) -> Result<()> {
        ensure!(
            process_id > 0 && bytes.len() <= 1024 * 1024 && (!eof || bytes.is_empty()),
            "Invalid process stdin"
        );
        let payload = if eof {
            stdin_payload::Payload::Eof(Empty {})
        } else {
            stdin_payload::Payload::Input(bytes)
        };
        let mut request = self.request(StdinRequest {
            process_id,
            payload: Some(payload),
        })?;
        request.set_timeout(Duration::from_secs(5));
        tokio::time::timeout(Duration::from_secs(5), self.grpc.ready()).await??;
        let _: tonic::Response<Empty> = self
            .grpc
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(
                    "/process.v1.ProcessService/WriteStdin",
                ),
                tonic_prost::ProstCodec::default(),
            )
            .await?;
        Ok(())
    }
    pub async fn resize_tty(&mut self, process_id: i32, cols: u32, rows: u32) -> Result<()> {
        ensure!(
            process_id > 0 && (1..=4096).contains(&cols) && (1..=4096).contains(&rows),
            "Invalid terminal dimensions"
        );
        let mut request = self.request(ResizeRequest {
            process_id,
            cols,
            rows,
        })?;
        request.set_timeout(Duration::from_secs(5));
        tokio::time::timeout(Duration::from_secs(5), self.grpc.ready()).await??;
        let _: tonic::Response<Empty> = self
            .grpc
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(
                    "/process.v1.ProcessService/ResizeTTY",
                ),
                tonic_prost::ProstCodec::default(),
            )
            .await?;
        Ok(())
    }
    pub async fn signal(&mut self, process_id: i32, signal: i32) -> Result<()> {
        ensure!(
            process_id > 0 && matches!(signal, 2 | 9 | 15),
            "Unsupported process signal"
        );
        let mut request = self.request(SignalRequest { process_id, signal })?;
        request.set_timeout(Duration::from_secs(5));
        tokio::time::timeout(Duration::from_secs(5), self.grpc.ready()).await??;
        let _: tonic::Response<Empty> = self
            .grpc
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(
                    "/process.v1.ProcessService/SendSignal",
                ),
                tonic_prost::ProstCodec::default(),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    #[test]
    fn versioned_wire_tags_and_path_boundaries() {
        assert_eq!(
            SignalRequest {
                process_id: 7,
                signal: 9
            }
            .encode_to_vec(),
            vec![8, 7, 16, 9]
        );
        assert_eq!(
            Pty { cols: 80, rows: 24 }.encode_to_vec(),
            vec![8, 80, 16, 24]
        );
        assert_eq!(
            StdinRequest {
                process_id: 7,
                payload: Some(stdin_payload::Payload::Eof(Empty {}))
            }
            .encode_to_vec(),
            vec![8, 7, 26, 0]
        );
        let r = StartResponse::decode(&[34, 2, 8, 42][..]).unwrap();
        assert!(matches!(
            r.event,
            Some(event::Event::Exit(event::Exit { exit_code: 42 }))
        ));
        for p in ["../x", "/abs", "a//b", "a/./b", "a\\b", "a:b"] {
            assert!(!path_valid(p));
        }
        assert!(path_valid("nested/a b.txt"));
        assert!(host_valid("sb.ns.svc.cluster.local"));
        assert!(!host_valid("attacker:9090"));
    }
    async fn fixture(
        events: Vec<StartResponse>,
    ) -> (
        Runtime,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::{
            Router,
            body::{Body, Bytes},
            http::{HeaderMap, Response},
            routing::post,
        };
        use http_body_util::BodyExt;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let router = Router::new().route(
            "/process.v1.ProcessService/Start",
            post(move |headers: HeaderMap| {
                let events = events.clone();
                let count = count.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        format!("Bearer {}", "t".repeat(64)).as_str()
                    );
                    count.fetch_add(1, Ordering::SeqCst);
                    let mut bytes = Vec::new();
                    for event in events {
                        let encoded = event.encode_to_vec();
                        bytes.push(0);
                        bytes.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
                        bytes.extend(encoded);
                    }
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", "0".parse().unwrap());
                    let body =
                        http_body_util::Full::new(Bytes::from(bytes)).with_trailers(async move {
                            Some(Ok::<_, std::convert::Infallible>(trailers))
                        });
                    Response::builder()
                        .header("content-type", "application/grpc")
                        .body(Body::new(body))
                        .unwrap()
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let channel = Channel::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let runtime = Runtime {
            host: "unused.invalid".into(),
            token: "t".repeat(64),
            http: Client::new(),
            grpc: tonic::client::Grpc::new(channel),
        };
        (runtime, calls, server)
    }
    #[tokio::test]
    async fn real_grpc_frames_stream_once_with_bounded_capture() {
        let events = vec![
            StartResponse {
                event: Some(event::Event::Init(event::Init { process_id: 4 })),
            },
            StartResponse {
                event: Some(event::Event::Stdout(vec![b'x'; 512])),
            },
            StartResponse {
                event: Some(event::Event::Exit(event::Exit { exit_code: 7 })),
            },
        ];
        let (mut runtime, calls, server) = fixture(events).await;
        let result = runtime
            .execute(
                vec!["/bin/tool".into()],
                HashMap::new(),
                Duration::from_secs(2),
                &AtomicBool::new(false),
                64,
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.stdout.bytes.len(), 64);
        assert_eq!(result.stdout.total_bytes, 512);
        assert!(result.stdout.truncated);
        assert!(result.complete);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
    #[tokio::test]
    async fn missing_exit_is_uncertain_and_never_replayed() {
        let (mut runtime, calls, server) = fixture(vec![StartResponse {
            event: Some(event::Event::Init(event::Init { process_id: 4 })),
        }])
        .await;
        assert!(
            runtime
                .execute(
                    vec!["/bin/tool".into()],
                    HashMap::new(),
                    Duration::from_secs(2),
                    &AtomicBool::new(false),
                    64
                )
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
