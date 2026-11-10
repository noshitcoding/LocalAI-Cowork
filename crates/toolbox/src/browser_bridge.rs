//! Versioned local-only bridge. The extension cannot supply commands or daemon
//! credentials: it sends grants/results, while the admitted toolbox sends actions.
use super::*;
use cowork_contracts::*;
use serde_json::{Value, json};
use std::sync::{Weak, mpsc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub const VERSION: u32 = 1;
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
pub const CHUNK: usize = 192 * 1024;

pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Value> {
    let size = reader.read_u32_le().await? as usize;
    if size == 0 || size > MAX_MESSAGE {
        return Err(std::io::Error::other("browser_message_limit"));
    }
    let mut raw = vec![0; size];
    reader.read_exact(&mut raw).await?;
    serde_json::from_slice(&raw).map_err(|_| std::io::Error::other("browser_invalid_message"))
}
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> std::io::Result<()> {
    let raw = serde_json::to_vec(value)?;
    if raw.is_empty() || raw.len() > MAX_MESSAGE {
        return Err(std::io::Error::other("browser_message_limit"));
    }
    writer.write_u32_le(raw.len() as u32).await?;
    writer.write_all(&raw).await?;
    writer.flush().await
}
pub fn discovery_directory() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/share")));
    let path = base
        .filter(|p| p.is_absolute())
        .ok_or_else(|| invalid("browser_user_directory_unavailable"))?
        .join("localai-cowork-v2/browser-bridge");
    private_dir(&path)?;
    Ok(path)
}
pub fn instances() -> Result<Vec<Value>> {
    let root = discovery_directory()?;
    let mut output = Vec::new();
    for item in fs::read_dir(root)?.take(64) {
        let path = item?.path();
        if path.extension().is_none_or(|v| v != "json") {
            continue;
        }
        namespace(&path)?;
        let meta = fs::metadata(&path)?;
        if meta.len() > 8192 {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&fs::read(path)?)
            && v["version"] == VERSION
            && v["id"].as_str().is_some_and(|s| Uuid::parse_str(s).is_ok())
        {
            output.push(v);
        }
    }
    Ok(output)
}
struct Connection {
    view: BrowserExtensionConnection,
    sender: tokio::sync::mpsc::Sender<Value>,
    pending: BTreeMap<String, (String, mpsc::SyncSender<Value>)>,
}
pub(crate) struct Bridge {
    root: PathBuf,
    connections: Mutex<BTreeMap<String, Connection>>,
    stopped: AtomicBool,
    started: Mutex<bool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Bridge {
    pub fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            connections: Mutex::new(BTreeMap::new()),
            stopped: AtomicBool::new(false),
            started: Mutex::new(false),
            worker: Mutex::new(None),
        })
    }
    pub fn start(self: &Arc<Self>) -> Result<()> {
        let mut started = self.started.lock().map_err(|_| Error::Conflict)?;
        if *started {
            return Ok(());
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        let id = Uuid::new_v4().to_string();
        let discovery = discovery_directory()?.join(format!("{id}.json"));
        #[cfg(windows)]
        let endpoint = format!(r"\\.\pipe\cowork-browser-{id}");
        #[cfg(unix)]
        let endpoint = {
            let directory = discovery_directory()?.join("sockets");
            private_dir(&directory)?;
            directory
                .join(format!("{}.sock", &id[..8]))
                .to_string_lossy()
                .into_owned()
        };
        let instance = json!({"version":VERSION,"id":id,"label":self.root.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy(),"endpoint":endpoint});
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&discovery)?;
        file.write_all(&serde_json::to_vec(&instance)?)?;
        file.sync_all()?;
        let weak = Arc::downgrade(self);
        let (tx, rx) = mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name("browser-extension-bridge".into())
            .spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    runtime.block_on(serve(weak, endpoint, tx));
                }
                let _ = fs::remove_file(discovery);
            })?;
        *self.worker.lock().map_err(|_| Error::Conflict)? = Some(handle);
        if rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false) {
            *started = true;
            Ok(())
        } else {
            Err(invalid("browser_bridge_could_not_start"))
        }
    }
    pub fn connections(self: &Arc<Self>) -> Vec<BrowserExtensionConnection> {
        if self.start().is_err() {
            return Vec::new();
        }
        self.connections
            .lock()
            .map(|v| v.values().map(|c| c.view.clone()).collect())
            .unwrap_or_default()
    }
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(mut v) = self.connections.lock() {
            v.clear();
        }
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
    pub fn call(
        &self,
        id: &str,
        epoch: &str,
        mut command: BrowserCommand,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut all = self.connections.lock().map_err(|_| Error::Conflict)?;
            let c = all
                .get_mut(id)
                .filter(|v| v.view.connected)
                .ok_or(Error::NotFound)?;
            if !c.pending.is_empty() {
                return Err(Error::Conflict);
            }
            if command.operation == "tabs" {
                return Ok(json!({"tabs":c.view.tabs,"context_epoch":epoch}));
            }
            if command.operation != "new_tab" {
                if command.tab_id.is_none() && c.view.tabs.len() > 1 {
                    return Err(invalid("browser_select_exact_tab"));
                }
                let tab = if let Some(tab) = &command.tab_id {
                    c.view.tabs.iter().find(|t| &t.id == tab)
                } else {
                    c.view
                        .tabs
                        .iter()
                        .find(|t| t.active)
                        .or_else(|| c.view.tabs.first())
                }
                .ok_or_else(|| invalid("browser_tab_not_granted"))?;
                if command
                    .document_id
                    .as_ref()
                    .is_some_and(|d| d != &tab.document_id)
                {
                    return Err(invalid("browser_document_changed"));
                }
                command.tab_id = Some(tab.id.clone());
                command.document_id = Some(tab.document_id.clone());
            }
            c.pending.insert(request_id.clone(), (epoch.into(), tx));
            if c.sender.try_send(json!({"type":"command","version":VERSION,"request_id":request_id,"epoch":epoch,"command":command})).is_err(){c.pending.remove(&request_id);c.view.connected=false;return Err(invalid("browser_extension_disconnected"));}
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        loop {
            if cancel.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
                self.disconnect(id);
                return Err(invalid("browser_extension_effect_uncertain_do_not_replay"));
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(value) => {
                    if value["ok"] != true {
                        return Err(invalid("browser_extension_action_failed_or_uncertain"));
                    }
                    return Ok(value["result"].clone());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(invalid("browser_extension_effect_uncertain_do_not_replay"));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn disconnect(&self, id: &str) {
        if let Ok(mut v) = self.connections.lock() {
            v.remove(id);
        }
    }
}
fn tabs(value: &Value) -> Result<Vec<BrowserTab>> {
    let tabs: Vec<BrowserTab> = serde_json::from_value(value.clone())?;
    if tabs.len() > 64
        || tabs.iter().any(|t| {
            t.id.len() > 100
                || t.document_id.len() > 100
                || t.title.len() > 1000
                || t.url.len() > 8192
                || !(t.url.starts_with("https://") || t.url.starts_with("http://"))
        })
    {
        return Err(invalid("browser_invalid_tab_grants"));
    }
    Ok(tabs)
}
async fn connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    bridge: Weak<Bridge>,
    mut stream: S,
) -> std::io::Result<()> {
    let hello = tokio::time::timeout(Duration::from_secs(10), read_message(&mut stream)).await??;
    if hello["type"] != "hello" || hello["version"] != VERSION {
        return Err(std::io::Error::other("browser_protocol_mismatch"));
    }
    let engine: BrowserEngine = serde_json::from_value(hello["engine"].clone())?;
    let granted =
        tabs(&hello["tabs"]).map_err(|_| std::io::Error::other("browser_invalid_grants"))?;
    let id = Uuid::new_v4().to_string();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let Some(state) = bridge.upgrade() else {
        return Ok(());
    };
    {
        let mut all = state
            .connections
            .lock()
            .map_err(|_| std::io::Error::other("bridge_lock"))?;
        if all.len() >= 16 {
            return Err(std::io::Error::other("browser_connection_limit"));
        }
        all.insert(
            id.clone(),
            Connection {
                view: BrowserExtensionConnection {
                    id: id.clone(),
                    engine,
                    label: hello["label"]
                        .as_str()
                        .unwrap_or("Browser")
                        .chars()
                        .take(100)
                        .collect(),
                    tabs: granted,
                    connected: true,
                },
                sender,
                pending: BTreeMap::new(),
            },
        );
    }
    drop(state);
    let (mut reader, mut writer) = tokio::io::split(stream);
    // read_exact is not cancellation safe. Keep one dedicated reader alive
    // across outgoing messages and lifecycle ticks.
    let (read_tx, mut read_rx) = tokio::sync::mpsc::channel(8);
    let read_task = tokio::spawn(async move {
        loop {
            let value = read_message(&mut reader).await;
            let failed = value.is_err();
            if read_tx.send(value).await.is_err() || failed {
                break;
            }
        }
    });
    let result=async{
        write_message(&mut writer,&json!({"type":"connected","version":VERSION,"connection_id":id})).await?;
        loop{tokio::select!{
            incoming=read_rx.recv()=>{
                let Some(incoming)=incoming else{break;};let incoming=incoming?;let Some(state)=bridge.upgrade()else{break;};
                if incoming["version"]!=VERSION{break;}
                let mut all=state.connections.lock().map_err(|_|std::io::Error::other("bridge_lock"))?;
                let Some(c)=all.get_mut(&id)else{break;};
                match incoming["type"].as_str(){
                    Some("grants")=>{c.view.tabs=tabs(&incoming["tabs"]).map_err(|_|std::io::Error::other("browser_invalid_grants"))?;},
                    Some("revoke")=>break,
                    Some("result")=>{if let Some(request)=incoming["request_id"].as_str()&& let Some((epoch,reply))=c.pending.remove(request){if incoming["epoch"]!=epoch{break;}let _=reply.send(incoming);}},
                    _=>break,
                }
            },
            outgoing=receiver.recv()=>{match outgoing{Some(value)=>write_message(&mut writer,&value).await?,None=>break}},
            _=tokio::time::sleep(Duration::from_millis(250))=>{if bridge.upgrade().is_none_or(|s|s.stopped.load(Ordering::Acquire)){break;}}
        }}Ok::<(),std::io::Error>(())
    }.await;
    read_task.abort();
    if let Some(state) = bridge.upgrade() {
        state.disconnect(&id);
    }
    result
}
#[cfg(unix)]
async fn serve(bridge: Weak<Bridge>, endpoint: String, ready: mpsc::SyncSender<bool>) {
    let Ok(listener) = tokio::net::UnixListener::bind(&endpoint) else {
        let _ = ready.send(false);
        return;
    };
    let _ = ready.send(true);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted
                    && stream.peer_cred().is_ok_and(|c| c.uid() == unsafe { libc::geteuid() })
                {
                    tokio::spawn(connection(bridge.clone(), stream));
                }
            },
            _=tokio::time::sleep(Duration::from_millis(100))=>{if bridge.upgrade().is_none_or(|s|s.stopped.load(Ordering::Acquire)){break;}}
        }
    }
    let _ = fs::remove_file(endpoint);
}
#[cfg(windows)]
async fn serve(bridge: Weak<Bridge>, endpoint: String, ready: mpsc::SyncSender<bool>) {
    let Ok(mut server) = private_pipe(&endpoint, true) else {
        let _ = ready.send(false);
        return;
    };
    let _ = ready.send(true);
    loop {
        tokio::select! {
            accepted=server.connect()=>{if accepted.is_err(){break;}let Ok(next)=private_pipe(&endpoint,false)else{break;};let stream=std::mem::replace(&mut server,next);tokio::spawn(connection(bridge.clone(),stream));},
            _=tokio::time::sleep(Duration::from_millis(100))=>{if bridge.upgrade().is_none_or(|s|s.stopped.load(Ordering::Acquire)){break;}}
        }
    }
}
#[cfg(windows)]
fn private_pipe(
    endpoint: &str,
    first: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{Authorization::*, *},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let success = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        );
        CloseHandle(token);
        if success == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut sid = ptr::null_mut();
        if ConvertSidToStringSidW((*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid, &mut sid) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut len = 0;
        while *sid.add(len) != 0 {
            len += 1;
        }
        let user = String::from_utf16_lossy(std::slice::from_raw_parts(sid, len));
        LocalFree(sid.cast());
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{user})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                endpoint,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            );
        LocalFree(descriptor);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn grants_scope_dispatch_and_disconnect_never_replays_a_command() {
        let bridge = Bridge::new(std::env::temp_dir().join(Uuid::new_v4().to_string()));
        let (server, mut client) = tokio::io::duplex(65536);
        let task = tokio::spawn(connection(Arc::downgrade(&bridge), server));
        write_message(&mut client,&json!({"type":"hello","version":VERSION,"engine":"firefox","label":"Firefox fixture","tabs":[{"id":"1","document_id":"doc-1","url":"https://example.com/","title":"Shared","active":true}]})).await.unwrap();
        let connected = read_message(&mut client).await.unwrap();
        let id = connected["connection_id"].as_str().unwrap().to_owned();
        assert!(
            bridge
                .call(
                    &id,
                    "epoch",
                    BrowserCommand {
                        operation: "dom".into(),
                        tab_id: Some("2".into()),
                        ..Default::default()
                    },
                    &AtomicBool::new(false)
                )
                .is_err()
        );
        let call_bridge = bridge.clone();
        let call_id = id.clone();
        let call = tokio::task::spawn_blocking(move || {
            call_bridge.call(
                &call_id,
                "epoch",
                BrowserCommand {
                    operation: "dom".into(),
                    ..Default::default()
                },
                &AtomicBool::new(false),
            )
        });
        let command = read_message(&mut client).await.unwrap();
        assert_eq!(command["command"]["tab_id"], "1");
        assert_eq!(command["command"]["document_id"], "doc-1");
        write_message(&mut client,&json!({"type":"result","version":VERSION,"request_id":command["request_id"],"epoch":"epoch","ok":true,"result":{"dom":"Fixture"}})).await.unwrap();
        assert_eq!(call.await.unwrap().unwrap()["dom"], "Fixture");
        let call_bridge = bridge.clone();
        let call_id = id.clone();
        let call = tokio::task::spawn_blocking(move || {
            call_bridge.call(
                &call_id,
                "epoch",
                BrowserCommand {
                    operation: "click".into(),
                    selector: Some("#save".into()),
                    ..Default::default()
                },
                &AtomicBool::new(false),
            )
        });
        let dispatched = read_message(&mut client).await.unwrap();
        assert_eq!(dispatched["command"]["operation"], "click");
        drop(client);
        assert!(
            call.await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("uncertain")
        );
        let _ = task.await;
        assert!(!bridge.connections.lock().unwrap().contains_key(&id));
    }
    #[tokio::test]
    async fn protocol_mismatch_and_oversized_frames_are_rejected() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer.write_u32_le((MAX_MESSAGE + 1) as u32).await.unwrap();
        assert!(read_message(&mut reader).await.is_err());
        let bridge = Bridge::new(PathBuf::new());
        let (server, mut client) = tokio::io::duplex(4096);
        let task = tokio::spawn(connection(Arc::downgrade(&bridge), server));
        write_message(&mut client, &json!({"type":"hello","version":999}))
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
    }
    #[tokio::test]
    async fn real_operating_system_transport_accepts_the_current_user() {
        let root = std::env::temp_dir().join(format!("cowork-browser-ipc-{}", Uuid::new_v4()));
        private_dir(&root).unwrap();
        let bridge = Bridge::new(root.clone());
        #[cfg(windows)]
        let endpoint = format!(r"\\.\pipe\cowork-browser-{}", Uuid::new_v4());
        #[cfg(unix)]
        let endpoint = root.join("bridge.sock").to_string_lossy().into_owned();
        let (tx, rx) = mpsc::sync_channel(1);
        let server = tokio::spawn(serve(Arc::downgrade(&bridge), endpoint.clone(), tx));
        assert!(
            tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)).unwrap())
                .await
                .unwrap()
        );
        #[cfg(windows)]
        let mut client = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&endpoint)
            .unwrap();
        #[cfg(unix)]
        let mut client = tokio::net::UnixStream::connect(&endpoint).await.unwrap();
        write_message(&mut client,&json!({"type":"hello","version":VERSION,"engine":"chrome","label":"IPC fixture","tabs":[]})).await.unwrap();
        assert_eq!(
            read_message(&mut client).await.unwrap()["type"],
            "connected"
        );
        drop(client);
        bridge.shutdown();
        server.await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
