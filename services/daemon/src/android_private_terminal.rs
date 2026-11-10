//! Service-owned original PTY streams. Disconnect closes the exact guest, never reconnects it.
use super::*;
use axum::extract::ws::{Message, WebSocket};
use base64::Engine;
use cowork_storage::android_private_terminal::TerminalAdmission;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

pub(super) struct Runtime {
    closed: AtomicBool,
    slots: Arc<Semaphore>,
    jobs: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    stop: CancellationToken,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            closed: AtomicBool::new(false),
            slots: Arc::new(Semaphore::new(8)),
            jobs: std::sync::Mutex::new(Vec::new()),
            stop: CancellationToken::new(),
        }
    }
}
impl Runtime {
    fn reserve(&self) -> Result<OwnedSemaphorePermit> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Forbidden);
        }
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::BudgetExceeded)
    }
    pub(super) async fn shutdown(&self) {
        let jobs = {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            self.closed.store(true, Ordering::Release);
            self.slots.close();
            self.stop.cancel();
            std::mem::take(&mut *jobs)
        };
        for job in jobs {
            if job.await.is_err() {
                tracing::warn!("Original terminal stream ended without a confirmed closure");
            }
        }
    }
}
pub(crate) struct Connection {
    original: TerminalAdmission,
    slot: OwnedSemaphorePermit,
}
async fn frame(write: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| invalid())?);
    if bytes.len() > 128 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        write.write_all(&bytes).await?;
        write.write_all(b"\n").await?;
        write.flush().await
    })
    .await
    .map_err(|_| Error::UncertainEffect)?
    .map_err(|_| Error::UncertainEffect)
}
impl AndroidDeviceServices {
    pub(crate) async fn prepare_private_terminal(
        &self,
        sid: Uuid,
        ticket: &str,
    ) -> Result<Connection> {
        if !self.private_server_available() {
            return Err(Error::Forbidden);
        }
        let slot = self.private_terminal.reserve()?;
        let binding = self
            .private_runner
            .as_ref()
            .ok_or(Error::Forbidden)?
            .binding();
        let original = self
            .store
            .android_private_terminal_connect(sid, ticket, &binding, self.keys.as_ref())
            .await?;
        Ok(Connection { original, slot })
    }
    pub(crate) async fn private_terminal_stream(
        self: Arc<Self>,
        socket: WebSocket,
        connection: Connection,
    ) {
        let (done, waiting) = tokio::sync::oneshot::channel();
        {
            let mut jobs = self
                .private_terminal
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if self.private_terminal.closed.load(Ordering::Acquire) {
                return;
            }
            jobs.retain(|job| !job.is_finished());
            let service = self.clone();
            jobs.push(tokio::spawn(async move {
                let Connection { original, slot } = connection;
                let _slot = slot;
                if service
                    .relay_private_terminal(socket, original)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Original private terminal remains unresolved; no input or execution replay"
                    );
                }
                let _ = done.send(());
            }));
        }
        // The websocket upgrade waiter may disappear; the original service job still closes its guest.
        let _ = waiting.await;
    }
    async fn relay_private_terminal(
        &self,
        socket: WebSocket,
        original: TerminalAdmission,
    ) -> Result<()> {
        let runner = self.private_runner.as_ref().ok_or(Error::Forbidden)?;
        let stop = self.private_terminal.stop.child_token();
        let mut container = runner
            .start(
                original.run,
                original.lease,
                &format!("terminal:{}", original.session),
                &original.bootstrap,
                &stop,
            )
            .await?;
        let nonce = container.nonce;
        let attached = self
            .store
            .android_private_terminal_attached(original.session, nonce, self.keys.as_ref())
            .await;
        if let Err(error) = attached {
            container.finish().await?;
            return Err(error);
        }
        let (Some(mut write), Some(mut read)) = (container.stdin.take(), container.stdout.take())
        else {
            container.finish().await?;
            return Err(Error::UncertainEffect);
        };
        let resize_protocol = socket
            .protocol()
            .is_some_and(|protocol| protocol == "terminal.binary.v1");
        let (mut sending, mut receiving) = socket.split();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut pending = None;
        let mut closing = false;
        let mut close_sent = false;
        let mut original_closed = None;
        let mut unretained_frame: Option<Vec<u8>> = None;
        let mut retention_error = None;
        let mut pulse = tokio::time::interval(Duration::from_secs(1));
        let mut close_deadline = None;
        let outcome:Result<()>=async{
            loop{
                // Drain each complete original frame before taking another client action.
                if let Some(position)=buffer.iter().position(|b|*b==b'\n'){
                    unretained_frame=Some(buffer[..=position].to_vec());
                    let value:Value=serde_json::from_slice(&buffer[..position]).map_err(|_|Error::UncertainEffect)?;
                    buffer.drain(..=position);
                    if value["nonce"]!=nonce.to_string()||value["session_id"]!=original.session.to_string(){return Err(Error::Forbidden)}
                    match value["kind"].as_str(){
                        Some("terminal_ready") if value["process_id"].as_u64().is_some_and(|n|n>0)=>{},
                        Some("terminal_output")=>{
                            self.store.android_private_terminal_received(original.session,nonce,&value,self.keys.as_ref(),self.key_id).await?;
                            let bytes=STANDARD.decode(value["data"].as_str().ok_or(Error::UncertainEffect)?).map_err(|_|Error::UncertainEffect)?;
                            if !closing
                                && !matches!(tokio::time::timeout(Duration::from_secs(5),sending.send(Message::Binary(bytes.into()))).await,Ok(Ok(()))){closing=true;close_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(5));}
                        }
                        Some("terminal_input")=>{
                            self.store.android_private_terminal_received(original.session,nonce,&value,self.keys.as_ref(),self.key_id).await?;
                            if pending!=value["sequence"].as_i64(){return Err(Error::UncertainEffect)}pending=None;
                            if value["complete"]!=true{closing=true;close_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(5));}
                        }
                        Some("terminal_closed")=>{
                            original_closed=Some(value);
                            if let Some(bytes)=unretained_frame.as_ref()
                                && let Err(error)=self.store.android_private_terminal_partial_received(original.session,nonce,bytes,true,self.keys.as_ref(),self.key_id).await {retention_error=Some(error);}
                            unretained_frame=None;break
                        },
                        _=>return Err(Error::UncertainEffect),
                    }
                    unretained_frame=None;
                    continue
                }
                if closing&&!close_sent&&pending.is_none(){
                    let request=json!({"operation":"close"});
                    let sequence=self.store.android_private_terminal_input_admit(original.session,&request,self.keys.as_ref(),self.key_id).await?;
                    self.store.android_private_terminal_validate(original.session,self.keys.as_ref()).await?;
                    frame(&mut write,&json!({"nonce":nonce,"session_id":original.session,"sequence":sequence,"operation":"close"})).await?;
                    pending=Some(sequence);close_sent=true;
                }
                if close_deadline.is_some_and(|d|tokio::time::Instant::now()>=d){return Err(Error::UncertainEffect)}
                tokio::select!{
                    bytes=read.read(&mut chunk)=>{
                        let count=bytes.map_err(|_|Error::UncertainEffect)?;
                        if count==0{return Err(Error::UncertainEffect)}
                        buffer.extend_from_slice(&chunk[..count]);if buffer.len()>32*1024*1024{return Err(Error::BudgetExceeded)}
                    }
                    message=receiving.next(),if !closing&&pending.is_none()=>{
                        let request=match message{
                            Some(Ok(Message::Binary(bytes)))=>{if bytes.is_empty()||bytes.len()>65536{return Err(Error::BudgetExceeded)}json!({"operation":"input","data":STANDARD.encode(&bytes)})},
                            Some(Ok(Message::Text(text)))=>{
                                if text.is_empty()||text.len()>65536{return Err(Error::BudgetExceeded)}
                                if resize_protocol && let Ok(v)=serde_json::from_str::<Value>(&text){
                                    if v["operation"]=="resize"{v}else{json!({"operation":"input","data":STANDARD.encode(text.as_bytes())})}
                                }else{json!({"operation":"input","data":STANDARD.encode(text.as_bytes())})}
                            }
                            Some(Ok(Message::Ping(bytes)))=>{let _=sending.send(Message::Pong(bytes)).await;continue},
                            Some(Ok(Message::Pong(_)))=>continue,
                            Some(Ok(Message::Close(_)))|None|Some(Err(_))=>{closing=true;close_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(5));json!({"operation":"close"})},
                        };
                        let sequence=self.store.android_private_terminal_input_admit(original.session,&request,self.keys.as_ref(),self.key_id).await?;
                        self.store.android_private_terminal_validate(original.session,self.keys.as_ref()).await?;
                        if request["operation"]=="close"{close_sent=true;}
                        let mut original_frame=request;original_frame["nonce"]=json!(nonce);original_frame["session_id"]=json!(original.session);original_frame["sequence"]=json!(sequence);
                        frame(&mut write,&original_frame).await?;pending=Some(sequence);
                    }
                    _=pulse.tick()=>{
                        if self.store.android_private_terminal_validate(original.session,self.keys.as_ref()).await.is_err(){return Err(Error::Forbidden)}
                    }
                    _=stop.cancelled()=>{return Err(Error::Forbidden)}
                }
            }
            Ok(())
        }.await;
        drop(write);
        if original_closed.is_none() {
            // Closing our original control pipe is retirement, not another input/effect admission.
            // Retain all available original frames before physical cleanup, even after authority loss.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(position) = buffer.iter().position(|b| *b == b'\n') {
                    let parsed = serde_json::from_slice::<Value>(&buffer[..position]);
                    let Ok(value) = parsed else { break };
                    if value["nonce"] != nonce.to_string()
                        || value["session_id"] != original.session.to_string()
                    {
                        break;
                    }
                    unretained_frame = Some(buffer[..=position].to_vec());
                    buffer.drain(..=position);
                    if value["kind"] == "terminal_closed" {
                        original_closed = Some(value);
                        if let Some(bytes) = unretained_frame.as_ref()
                            && let Err(error) = self
                                .store
                                .android_private_terminal_partial_received(
                                    original.session,
                                    nonce,
                                    bytes,
                                    true,
                                    self.keys.as_ref(),
                                    self.key_id,
                                )
                                .await
                        {
                            retention_error = Some(error);
                        }
                        unretained_frame = None;
                        break;
                    }
                    if matches!(
                        value["kind"].as_str(),
                        Some("terminal_input" | "terminal_output")
                    ) {
                        if self
                            .store
                            .android_private_terminal_received(
                                original.session,
                                nonce,
                                &value,
                                self.keys.as_ref(),
                                self.key_id,
                            )
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else if value["kind"] != "terminal_ready" {
                        break;
                    }
                    unretained_frame = None;
                    continue;
                }
                let received = tokio::time::timeout_at(deadline, read.read(&mut chunk)).await;
                let Ok(Ok(count)) = received else { break };
                if count == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..count]);
                if buffer.len() > 32 * 1024 * 1024 {
                    break;
                }
            }
        }
        if !buffer.is_empty()
            && let Err(error) = self
                .store
                .android_private_terminal_partial_received(
                    original.session,
                    nonce,
                    &buffer,
                    false,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await
        {
            retention_error = Some(error);
        }
        if let Some(bytes) = unretained_frame
            && !buffer.starts_with(&bytes)
            && let Err(error) = self
                .store
                .android_private_terminal_partial_received(
                    original.session,
                    nonce,
                    &bytes,
                    false,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await
        {
            retention_error = Some(error);
        }
        // A positive physical stop/removal is required even after client failure or revocation.
        let finished = container.finish().await;
        let retained = if finished.is_ok() {
            self.store
                .android_private_terminal_close(
                    original.session,
                    nonce,
                    original_closed.as_ref(),
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await
        } else {
            Err(Error::UncertainEffect)
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), sending.close()).await;
        finished?;
        retained?;
        if let Some(error) = retention_error {
            return Err(error);
        }
        outcome
    }
}
