//! Pinned App Server account broker. Only explicit V2 runtime configuration and
//! per-owner/per-account homes are used. Credentials stay in the OS keyring.
//! This module does not execute agent turns or authorize tools.
use cowork_contracts::*;
use cowork_domain::{Error, Result, hash};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

pub const VERSION: &str = "0.147.0";
pub const PROTOCOL: &str = "app-server-0.147.0";
fn failure(code: &str) -> Error {
    Error::Provider(code.into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    manifest: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: String,
    protocol_schema: String,
    binary: String,
    sha256: String,
    binary_size: Option<u64>,
    license: String,
    license_sha256: Option<String>,
}
#[derive(Clone)]
struct Verified {
    binary: PathBuf,
}
pub struct CodexBroker {
    data: PathBuf,
    processes: Mutex<HashMap<String, Arc<Process>>>,
    #[cfg(test)]
    pub(crate) fixture_auth: bool,
}
#[derive(Clone, Copy)]
pub enum AccountOperation {
    Read,
    Models,
    Limits,
    LoginBrowser,
    LoginDevice,
    Logout,
}
impl CodexBroker {
    pub fn new(data: PathBuf) -> Self {
        Self {
            data,
            processes: Mutex::new(HashMap::new()),
            #[cfg(test)]
            fixture_auth: false,
        }
    }
    pub fn unavailable() -> CodexRuntimeStatus {
        CodexRuntimeStatus {
            configured: false,
            available: false,
            version: VERSION.into(),
            protocol_schema: PROTOCOL.into(),
            error_code: Some("codex_runtime_not_configured".into()),
        }
    }
    pub fn stop_account(&self, scope: &str, account: &str) -> Result<()> {
        uuid::Uuid::parse_str(account).map_err(|_| Error::Forbidden)?;
        let key = format!("{}:{account}", hash(scope.as_bytes()));
        let process = self
            .processes
            .lock()
            .map_err(|_| failure("codex_runtime_unavailable"))?
            .remove(&key);
        if let Some(process) = process {
            process.stop();
        }
        Ok(())
    }
    pub fn status(&self) -> CodexRuntimeStatus {
        match self.verify() {
            Ok(_) => CodexRuntimeStatus {
                configured: true,
                available: true,
                version: VERSION.into(),
                protocol_schema: PROTOCOL.into(),
                error_code: None,
            },
            Err(error) => CodexRuntimeStatus {
                configured: self.data.join("codex-runtime.json").is_file(),
                available: false,
                version: VERSION.into(),
                protocol_schema: PROTOCOL.into(),
                error_code: Some(match error {
                    Error::Provider(code) => code,
                    _ => "codex_runtime_unavailable".into(),
                }),
            },
        }
    }
    fn verify(&self) -> Result<Verified> {
        let config = self.data.join("codex-runtime.json");
        safe_path(&config)?;
        let raw = fs::read(&config).map_err(|_| failure("codex_runtime_not_configured"))?;
        if raw.len() > 16 * 1024 {
            return Err(failure("codex_runtime_configuration_invalid"));
        }
        let config: Configuration = serde_json::from_slice(&raw)
            .map_err(|_| failure("codex_runtime_configuration_invalid"))?;
        if !config.manifest.is_absolute() {
            return Err(failure("codex_runtime_configuration_invalid"));
        }
        safe_path(&config.manifest)?;
        let raw =
            fs::read(&config.manifest).map_err(|_| failure("codex_runtime_manifest_invalid"))?;
        if raw.len() > 64 * 1024 {
            return Err(failure("codex_runtime_manifest_invalid"));
        }
        let manifest: Manifest =
            serde_json::from_slice(&raw).map_err(|_| failure("codex_runtime_manifest_invalid"))?;
        if manifest.version != VERSION || manifest.protocol_schema != PROTOCOL {
            return Err(failure("codex_runtime_version_mismatch"));
        }
        let parent = config
            .manifest
            .parent()
            .ok_or_else(|| failure("codex_runtime_manifest_invalid"))?;
        let binary = relative(parent, &manifest.binary)?;
        let license = relative(parent, &manifest.license)?;
        if digest_file(&binary)? != manifest.sha256
            || manifest
                .binary_size
                .is_some_and(|n| fs::metadata(&binary).map(|m| m.len() != n).unwrap_or(true))
        {
            return Err(failure("codex_runtime_binary_mismatch"));
        }
        let license_digest = digest_file(&license)?;
        if manifest.license_sha256.is_some_and(|h| h != license_digest) {
            return Err(failure("codex_runtime_license_mismatch"));
        }
        Ok(Verified { binary })
    }
    pub async fn call(
        self: &Arc<Self>,
        scope: String,
        account: String,
        operation: AccountOperation,
    ) -> Result<Value> {
        let broker = self.clone();
        tokio::task::spawn_blocking(move||{
            let process=broker.process(&scope,&account)?;
            let (method,params)=match operation{
                AccountOperation::Read=>("account/read",Some(json!({"refreshToken":false}))),
                AccountOperation::Models=>("model/list",Some(json!({"limit":100,"includeHidden":false}))),
                AccountOperation::Limits=>("account/rateLimits/read",None),
                AccountOperation::LoginBrowser=>("account/login/start",Some(json!({"type":"chatgpt","useHostedLoginSuccessPage":true,"appBrand":"chatgpt"}))),
                AccountOperation::LoginDevice=>("account/login/start",Some(json!({"type":"chatgptDeviceCode"}))),
                AccountOperation::Logout=>("account/logout",None),
            };
            process.request(method,params)
        }).await.map_err(|_|failure("codex_runtime_worker_unavailable"))?
    }
    fn process(&self, scope: &str, account: &str) -> Result<Arc<Process>> {
        uuid::Uuid::parse_str(account).map_err(|_| Error::Forbidden)?;
        if scope != "installation-owner"
            && scope
                .strip_prefix("actor:")
                .is_none_or(|v| uuid::Uuid::parse_str(v).is_err())
        {
            return Err(Error::Forbidden);
        }
        let key = format!("{}:{account}", hash(scope.as_bytes()));
        let mut processes = self
            .processes
            .lock()
            .map_err(|_| failure("codex_runtime_unavailable"))?;
        if let Some(process) = processes.get(&key)
            && !process.dead.load(Ordering::Acquire)
        {
            return Ok(process.clone());
        }
        processes.remove(&key);
        if processes.len() >= 32 {
            return Err(Error::BudgetExceeded);
        }
        let binary = self.verify()?.binary;
        let home = self
            .data
            .join("codex-profiles")
            .join(hash(scope.as_bytes()))
            .join(account);
        safe_path(&home)?;
        fs::create_dir_all(&home).map_err(|_| failure("codex_private_home_unavailable"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
                .map_err(|_| failure("codex_private_home_unavailable"))?;
        }
        let config =
            b"cli_auth_credentials_store = \"keyring\"\ncheck_for_update_on_startup = false\n";
        let config_path = home.join("config.toml");
        safe_path(&config_path)?;
        if config_path.exists() {
            if fs::read(&config_path).map_err(|_| failure("codex_private_home_unavailable"))?
                != config
            {
                return Err(failure("codex_private_configuration_conflict"));
            }
        } else {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&config_path)
                .map_err(|_| failure("codex_private_home_unavailable"))?;
            file.write_all(config)
                .map_err(|_| failure("codex_private_home_unavailable"))?;
        }
        let process = Arc::new(Process::spawn(&binary, home)?);
        processes.insert(key, process.clone());
        Ok(process)
    }
    pub(crate) fn gateway_process(
        &self,
        scope: &str,
        account: &str,
        gateway: &str,
    ) -> Result<Process> {
        let existing = self.process(scope, account)?;
        {
            #[cfg(not(test))]
            let fixture = false;
            #[cfg(test)]
            let fixture = self.fixture_auth;
            Process::spawn_configured(
                &self.verify()?.binary,
                existing.home.clone(),
                Some(gateway),
                fixture,
            )
        }
    }
}
fn relative(root: &Path, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(failure("codex_runtime_manifest_invalid"));
    }
    let result = root.join(path);
    safe_path(&result)?;
    Ok(result)
}
fn safe_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(failure("codex_private_path_invalid"));
    }
    for ancestor in path.ancestors() {
        let Ok(metadata) = fs::symlink_metadata(ancestor) else {
            continue;
        };
        if metadata.is_symlink() {
            return Err(failure("codex_private_path_invalid"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(failure("codex_private_path_invalid"));
            }
        }
    }
    Ok(())
}
fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|_| failure("codex_runtime_file_unavailable"))?;
    if !file
        .metadata()
        .map_err(|_| failure("codex_runtime_file_unavailable"))?
        .is_file()
    {
        return Err(failure("codex_runtime_file_unavailable"));
    }
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 65536];
    loop {
        let size = file
            .read(&mut buffer)
            .map_err(|_| failure("codex_runtime_file_unavailable"))?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
    }
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
type PendingReplies = Arc<Mutex<HashMap<u64, mpsc::Sender<std::result::Result<Value, String>>>>>;

pub(crate) struct Process {
    home: PathBuf,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: PendingReplies,
    next: AtomicU64,
    dead: Arc<AtomicBool>,
}
impl Process {
    fn spawn(binary: &Path, home: PathBuf) -> Result<Self> {
        Self::spawn_configured(binary, home, None, false)
    }
    fn spawn_configured(
        binary: &Path,
        home: PathBuf,
        gateway: Option<&str>,
        fixture_auth: bool,
    ) -> Result<Self> {
        if home.join("auth.json").exists() {
            return Err(failure("codex_plaintext_auth_forbidden"));
        }
        let mut command = Command::new(binary);
        command
            .args(["app-server", "--listen", "stdio://"])
            .current_dir(&home)
            .env_clear();
        if let Some(gateway) = gateway {
            for (key, value) in [
                ("model_provider", json!("cowork_v2_gateway")),
                (
                    "model_providers.cowork_v2_gateway.name",
                    json!("Cowork bounded auth gateway"),
                ),
                ("model_providers.cowork_v2_gateway.base_url", json!(gateway)),
                (
                    "model_providers.cowork_v2_gateway.wire_api",
                    json!("responses"),
                ),
                (
                    "model_providers.cowork_v2_gateway.requires_openai_auth",
                    json!(true),
                ),
                (
                    "model_providers.cowork_v2_gateway.supports_websockets",
                    json!(false),
                ),
                (
                    "model_providers.cowork_v2_gateway.request_max_retries",
                    json!(0),
                ),
                (
                    "model_providers.cowork_v2_gateway.stream_max_retries",
                    json!(0),
                ),
                ("features.shell_tool", json!(false)),
                ("features.unified_exec", json!(false)),
                ("web_search", json!("disabled")),
                ("approval_policy", json!("never")),
                ("sandbox_mode", json!("read-only")),
            ] {
                command.arg("-c").arg(format!("{key}={value}"));
            }
        }
        #[cfg(test)]
        if fixture_auth {
            command
                .arg("-c")
                .arg("model_providers.cowork_v2_gateway.requires_openai_auth=false")
                .arg("-c")
                .arg("model_providers.cowork_v2_gateway.env_key=\"COWORK_FIXTURE_KEY\"")
                .env("COWORK_FIXTURE_KEY", "synthetic-gateway-fixture-key");
        }
        #[cfg(not(test))]
        let _ = fixture_auth;
        // OS/keyring discovery only; provider and agent credentials never inherit.
        for key in [
            "SystemRoot",
            "SYSTEMROOT",
            "WINDIR",
            "TEMP",
            "TMP",
            "HOME",
            "USERPROFILE",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "LANG",
            "LC_ALL",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("CODEX_HOME", &home)
            .env("NO_PROXY", "127.0.0.1,localhost")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| failure("codex_runtime_start_failed"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| failure("codex_runtime_start_failed"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| failure("codex_runtime_start_failed"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| failure("codex_runtime_start_failed"))?;
        let pending: PendingReplies = Arc::new(Mutex::new(HashMap::new()));
        let dead = Arc::new(AtomicBool::new(false));
        let reader_pending = pending.clone();
        let reader_dead = dead.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut frame = Vec::new();
                let result = (&mut reader)
                    .take(2 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut frame);
                if result.is_err()
                    || frame.is_empty()
                    || frame.len() > 2 * 1024 * 1024
                    || frame.last() != Some(&b'\n')
                {
                    break;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&frame) else {
                    break;
                };
                if value.get("method").is_some() && value.get("id").is_some() {
                    break;
                } // no inline tools or approvals are authorized here
                if let Some(id) = value["id"].as_u64()
                    && let Ok(mut requests) = reader_pending.lock()
                    && let Some(sender) = requests.remove(&id)
                {
                    let result = if value.get("error").is_some() {
                        let keyring = value["error"]
                            .to_string()
                            .to_lowercase()
                            .contains("keyring");
                        Err(if keyring {
                            "codex_keyring_unavailable"
                        } else {
                            "codex_account_request_failed"
                        }
                        .into())
                    } else {
                        Ok(value["result"].clone())
                    };
                    let _ = sender.send(result);
                }
            }
            reader_dead.store(true, Ordering::Release);
            if let Ok(mut requests) = reader_pending.lock() {
                for (_, sender) in requests.drain() {
                    let _ = sender.send(Err("codex_runtime_disconnected".into()));
                }
            }
        });
        std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut discard = [0u8; 4096];
            while stderr.read(&mut discard).is_ok_and(|n| n > 0) {}
        });
        let result = Self {
            home,
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending,
            next: AtomicU64::new(1),
            dead,
        };
        let handshake=result.request("initialize",Some(json!({"clientInfo":{"name":"localai_cowork_v2","title":"LocalAI Cowork","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})))?;
        if !handshake["userAgent"].is_string() {
            return Err(failure("codex_runtime_protocol_mismatch"));
        }
        result.send(&json!({"method":"initialized","params":{}}))?;
        Ok(result)
    }
    fn send(&self, value: &Value) -> Result<()> {
        let raw =
            serde_json::to_vec(value).map_err(|_| failure("codex_runtime_protocol_invalid"))?;
        let mut input = self
            .stdin
            .lock()
            .map_err(|_| failure("codex_runtime_unavailable"))?;
        input
            .write_all(&raw)
            .and_then(|_| input.write_all(b"\n"))
            .and_then(|_| input.flush())
            .map_err(|_| failure("codex_runtime_disconnected"))
    }
    pub(crate) fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        if self.dead.load(Ordering::Acquire) {
            return Err(failure("codex_runtime_disconnected"));
        }
        if self.home.join("auth.json").exists() {
            self.stop();
            return Err(failure("codex_plaintext_auth_forbidden"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| failure("codex_runtime_unavailable"))?
            .insert(id, sender);
        if let Err(e) = self.send(&json!({"id":id,"method":method,"params":params})) {
            self.stop();
            return Err(e);
        }
        let response = match receiver.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(value)) => value,
            Ok(Err(code)) => {
                self.stop();
                return Err(failure(&code));
            }
            Err(_) => {
                self.stop();
                return Err(Error::UncertainEffect);
            }
        };
        if self.home.join("auth.json").exists() {
            self.stop();
            return Err(failure("codex_plaintext_auth_forbidden"));
        }
        Ok(response)
    }
    pub(crate) fn stop(&self) {
        self.dead.store(true, Ordering::Release);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn login_view(value: Value, flow: CodexLoginFlow) -> Result<CodexLoginView> {
    fn url(value: &Value, key: &str) -> Result<Option<String>> {
        let Some(raw) = value[key].as_str() else {
            return Ok(None);
        };
        let url = reqwest::Url::parse(raw).map_err(|_| failure("codex_login_response_invalid"))?;
        if raw.len() > 16384
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failure("codex_login_response_invalid"));
        }
        Ok(Some(raw.into()))
    }
    let login_id = value["loginId"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| failure("codex_login_response_invalid"))?
        .to_owned();
    Ok(CodexLoginView {
        login_id,
        flow,
        authorization_url: url(&value, "authUrl")?,
        verification_url: url(&value, "verificationUrl")?,
        user_code: value["userCode"]
            .as_str()
            .filter(|v| v.len() <= 200)
            .map(str::to_owned),
    })
}
pub fn models(value: &Value) -> Result<Vec<CodexModel>> {
    let entries = value["data"]
        .as_array()
        .filter(|a| a.len() <= 100)
        .ok_or_else(|| failure("codex_model_response_invalid"))?;
    entries
        .iter()
        .map(|v| {
            let id = v["id"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 200)
                .ok_or_else(|| failure("codex_model_response_invalid"))?
                .to_owned();
            Ok(CodexModel {
                id,
                name: v["displayName"]
                    .as_str()
                    .unwrap_or("Codex model")
                    .chars()
                    .take(200)
                    .collect(),
                default_reasoning_effort: v["defaultReasoningEffort"].as_str().map(str::to_owned),
                reasoning_efforts: v["supportedReasoningEfforts"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .take(20)
                            .filter_map(|v| v["reasoningEffort"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_configuration_and_untrusted_manifest_are_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let broker = CodexBroker::new(dir.path().to_owned());
        assert_eq!(
            broker.status().error_code.as_deref(),
            Some("codex_runtime_not_configured")
        );
        fs::write(
            dir.path().join("codex-runtime.json"),
            r#"{"manifest":"../foreign-home/manifest.json"}"#,
        )
        .unwrap();
        assert_eq!(
            broker.status().error_code.as_deref(),
            Some("codex_runtime_configuration_invalid")
        );
        assert!(relative(dir.path(), "../codex.exe").is_err());
        assert!(
            login_view(
                json!({"loginId":"id","authUrl":"file:///private/auth.json"}),
                CodexLoginFlow::Browser
            )
            .is_err()
        );
    }
    #[test]
    fn model_catalog_preserves_provider_reasoning_choices() {
        let values=models(&json!({"data":[{"id":"model","displayName":"Fixture","defaultReasoningEffort":"high","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]}]})).unwrap();
        assert_eq!(values[0].reasoning_efforts, vec!["low", "high"]);
        assert!(models(&json!({"data":[{"id":""}]})).is_err());
    }
    #[test]
    fn protocol_fixture_never_executes_inline_tool_requests_and_uses_isolated_home() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("fixture.rs");
        fs::write(&source,r##"
use std::io::{self,BufRead,Write};
fn main(){
 let home=std::env::var_os("CODEX_HOME").unwrap();std::fs::write(std::path::Path::new(&home).join("fixture-started"),b"isolated").unwrap();
 for line in io::stdin().lock().lines(){let line=line.unwrap();let Some(rest)=line.split("\"id\":").nth(1)else{continue};let id:String=rest.trim_start().chars().take_while(|c|c.is_ascii_digit()).collect();
 if line.contains("fixture/inline"){println!("{{\"id\":99,\"method\":\"item/tool/call\",\"params\":{{\"tool\":\"execute_program\"}}}}");io::stdout().flush().unwrap();continue;}
 let result=if line.contains("initialize"){r#"{"userAgent":"fixture"}"#}else{r#"{"account":null}"#};
 println!("{{\"id\":{},\"result\":{}}}",id,result);io::stdout().flush().unwrap();
 }}
"##).unwrap();
        let binary = directory.path().join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        let compiled = Command::new("rustc")
            .arg("--edition=2024")
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let home = directory.path().join("isolated-home");
        fs::create_dir(&home).unwrap();
        let process = Process::spawn(&binary, home.clone()).unwrap();
        assert!(process.request("account/read", None).unwrap()["account"].is_null());
        assert!(home.join("fixture-started").is_file());
        assert!(process.request("fixture/inline", None).is_err());
        assert!(process.dead.load(Ordering::Acquire));
        assert!(!home.join("auth.json").exists());
    }
}
