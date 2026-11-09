//! Trusted framing outside the tenant's original MCP protocol. Never forwards tenant control frames.
use super::private_runner::Container;
use super::*;
use cowork_mcp::Endpoint;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

fn uncertain() -> Error {
    Error::UncertainEffect
}
async fn line(read: &mut (impl AsyncRead + Unpin), limit: usize) -> Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let mut b = [0];
        let n = read.read(&mut b).await.map_err(|_| uncertain())?;
        if n == 0 {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(uncertain())
            };
        }
        if b[0] == b'\n' {
            break;
        }
        if bytes.len() >= limit {
            return Err(Error::BudgetExceeded);
        }
        bytes.push(b[0]);
    }
    if bytes.is_empty() {
        return Err(uncertain());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| uncertain())
}
async fn frame(write: &Mutex<impl AsyncWrite + Unpin>, value: &Value) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| invalid())?);
    if bytes.len() > 24 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let mut write = write.lock().await;
    write.write_all(&bytes).await.map_err(|_| uncertain())?;
    write.write_all(b"\n").await.map_err(|_| uncertain())?;
    write.flush().await.map_err(|_| uncertain())
}
type Pending = Arc<Mutex<Option<(Uuid, oneshot::Sender<Result<Value>>)>>>;
pub(super) struct Bridge {
    nonce: Uuid,
    write: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Pending,
    serial: Mutex<()>,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Bridge {
    pub(super) fn attach(container: &mut Container) -> Result<(Self, Endpoint)> {
        let nonce = container.nonce;
        let write = Arc::new(Mutex::new(container.stdin.take().ok_or_else(uncertain)?));
        let mut guest = BufReader::new(container.stdout.take().ok_or_else(uncertain)?);
        let (library, peer) = tokio::io::duplex(65536);
        let (read, send) = tokio::io::split(library);
        let endpoint = Endpoint::isolated_stdio(nonce, read, send).map_err(|_| uncertain())?;
        let (mut from_library, mut to_library) = tokio::io::split(peer);
        let pending: Pending = Arc::new(Mutex::new(None));
        let pending_read = pending.clone();
        let rpc_write = write.clone();
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            let rpc = async {
                while let Some(value) = line(&mut from_library, 4 * 1024 * 1024).await? {
                    frame(
                        &rpc_write,
                        &json!({"kind":"rpc","nonce":nonce,"value":value}),
                    )
                    .await?;
                }
                Ok::<(), Error>(())
            };
            let receive = async {
                while let Some(value) = line(&mut guest, 24 * 1024 * 1024).await? {
                    if value["kind"] == "rpc" {
                        if !value["value"].is_object() {
                            return Err(uncertain());
                        }
                        let bytes = serde_json::to_vec(&value["value"]).map_err(|_| uncertain())?;
                        if bytes.len() > 4 * 1024 * 1024 {
                            return Err(Error::BudgetExceeded);
                        }
                        to_library
                            .write_all(&bytes)
                            .await
                            .map_err(|_| uncertain())?;
                        to_library.write_all(b"\n").await.map_err(|_| uncertain())?;
                    } else if value["kind"] == "workspace" && value["nonce"] == nonce.to_string() {
                        let (id, reply) = pending_read.lock().await.take().ok_or_else(uncertain)?;
                        if value["id"] != id.to_string() {
                            let _ = reply.send(Err(uncertain()));
                            return Err(uncertain());
                        }
                        let _ = reply.send(Ok(value));
                    } else {
                        return Err(uncertain());
                    }
                }
                Ok::<(), Error>(())
            };
            tokio::pin!(rpc);
            tokio::pin!(receive);
            let result =
                tokio::select! {_=task_stop.cancelled()=>Ok(()),v=&mut rpc=>v,v=&mut receive=>v};
            task_stop.cancel();
            if let Some((_, reply)) = pending_read.lock().await.take() {
                let _ = reply.send(Err(uncertain()));
            }
            result
        });
        Ok((
            Self {
                nonce,
                write,
                pending,
                serial: Mutex::new(()),
                stop,
                task: Some(task),
            },
            endpoint,
        ))
    }
    pub(super) async fn workspace(
        &self,
        operation: &str,
        files: Option<&[Value]>,
    ) -> Result<Value> {
        self.workspace_original(Uuid::new_v4(), operation, files)
            .await
    }
    pub(super) async fn workspace_original(
        &self,
        id: Uuid,
        operation: &str,
        files: Option<&[Value]>,
    ) -> Result<Value> {
        let _serial = self.serial.lock().await;
        if self.stop.is_cancelled() || !matches!(operation, "prepare" | "capture") {
            return Err(uncertain());
        }
        if id.is_nil() {
            return Err(uncertain());
        }
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(uncertain());
        }
        *pending = Some((id, tx));
        drop(pending);
        let value = json!({"kind":"workspace","operation":operation,"nonce":self.nonce,"id":id,"files":files});
        if let Err(error) = frame(&self.write, &value).await {
            self.stop.cancel();
            return Err(error);
        }
        let reply = tokio::time::timeout(Duration::from_secs(30), rx).await;
        let result = match reply {
            Ok(Ok(v)) => v,
            _ => {
                self.stop.cancel();
                Err(uncertain())
            }
        }?;
        if result["operation"] != operation
            || result["paused"] != true
            || !result["server_alive"].is_boolean()
            || !result["files"].is_array()
        {
            self.stop.cancel();
            return Err(uncertain());
        }
        if let Some(files) = files {
            let mut expected = files.to_vec();
            expected.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
            if result["files"] != json!(expected) {
                self.stop.cancel();
                return Err(uncertain());
            }
        }
        Ok(result)
    }
    pub(super) async fn close(&mut self) -> Result<()> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .map_err(|_| uncertain())?
                .map_err(|_| uncertain())??;
        }
        Ok(())
    }
}
