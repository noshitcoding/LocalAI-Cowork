//! Verify historical provenance into inert, read-only data. Never deserialize a
//! Runtime/RunSpec/Principal and never open a manifest-supplied filesystem path.
//! The old cold target is preview provenance; current live authorization is external.
mod canonical;
mod mapping;
mod profiles;
mod projection;
mod seeds;
pub use mapping::{EntityKind, HistoricalMapping, HistoryNamespace};

use canonical as c;
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;

const MAX_DEPTH: usize = 32;
const MAX_CELL: usize = 16 * 1024 * 1024;
const MAX_LINE: usize = 256 * 1024 * 1024;
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
const MAX_PREVIEW: usize = 256 * 1024 * 1024;
const MAX_ROWS: u64 = 1_000_000;
const MAX_CONTENT: u64 = 4 * 1024 * 1024 * 1024;
const MAX_QUARANTINE: u64 = 12 * 1024 * 1024 * 1024;
const MAX_SNAPSHOT: u64 = 512 * 1024 * 1024;
const QUARANTINE_FORMAT: &str = "localai-cowork-inactive-quarantine/1";
const MAPPING_FORMAT: &str = "localai-cowork-historical-mapping/1";
const PREVIEW_FORMAT: &str = "localai-cowork-quarantine-preview/1";

#[derive(Debug)]
pub enum Error {
    Refused(&'static str),
    Io(std::io::Error),
    Sqlite(sqlx::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(v) => f.write_str(v),
            Self::Io(_) => f.write_str("Private historical IO failed"),
            Self::Sqlite(_) => f.write_str("Private historical SQLite validation failed"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Sqlite(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Deliberately has no Deserialize/Serialize and no operational-model conversion.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoricalCell {
    Null,
    Integer(i64),
    RealBits(u64),
    TextBytes(Vec<u8>),
    Blob(Vec<u8>),
}
#[derive(Clone, Debug)]
pub struct HistoricalRow {
    pub source_table: String,
    pub source_rowid: i64,
    pub position: u64,
    pub cells: Vec<HistoricalCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sha256Pin(String);
impl Sha256Pin {
    pub fn parse(value: &str) -> Result<Self> {
        c::sha(&Value::String(value.to_owned()))?;
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug)]
pub struct HistoricalPins {
    pub quarantine_manifest: Sha256Pin,
    pub quarantine_database: Sha256Pin,
    pub export_manifest: Sha256Pin,
    pub mapping: Sha256Pin,
    pub preview: Sha256Pin,
    pub cold_target_snapshot: Sha256Pin,
}
/// Readers are supplied by admitted server-owned intake. No JSON field nominates a path.
pub struct UntrustedHistoricalStreams {
    pub quarantine_manifest: Box<dyn Read + Send>,
    pub quarantine_database: Box<dyn Read + Send>,
    pub mapping: Box<dyn Read + Send>,
    pub preview: Box<dyn Read + Send>,
    pub cold_target_snapshot: Box<dyn Read + Send>,
}

#[derive(Clone, Debug)]
pub struct HistoricalProvenance {
    pub pins: HistoricalPins,
    pub source_profile: String,
    pub source_family: String,
    pub source_version: Option<u64>,
    pub original_rows: u64,
    pub original_canonical_bytes: u64,
    /// This identifies the historical preview snapshot, never the current live store.
    pub historical_target_profile: String,
    /// New Rust contract: the known installation INTEGER id=1 is a system
    /// singleton, never a history entity, target UUID, Principal or grant.
    pub verifier_contract: &'static str,
}

struct CapturedFile {
    name: &'static str,
    bytes: u64,
    sha256: String,
}
struct PrivateStage {
    // These fields drop before TempDir. HistoricalConnection retains this Stage
    // until actual SQLx close acknowledgment, including canceled request cleanup.
    files: Mutex<Vec<File>>,
    _namespace: Vec<File>,
    input_receipts: Vec<CapturedFile>,
    directory: TempDir,
}
impl PrivateStage {
    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
    fn read(&self, name: &str, maximum: usize) -> Result<Vec<u8>> {
        let index = self
            .input_receipts
            .iter()
            .position(|r| r.name == name)
            .ok_or(Error::Refused("Private captured file missing"))?;
        let mut files = self
            .files
            .lock()
            .map_err(|_| Error::Refused("Private file guard poisoned"))?;
        let stream = &mut files[index];
        stream.seek(SeekFrom::Start(0))?;
        let mut raw = Vec::new();
        stream.take(maximum as u64 + 1).read_to_end(&mut raw)?;
        if raw.len() > maximum {
            return Err(Error::Refused("Private JSON byte limit"));
        }
        Ok(raw)
    }
    fn check_receipts(&self, budget: Budget) -> Result<()> {
        let mut files = self
            .files
            .lock()
            .map_err(|_| Error::Refused("Private file guard poisoned"))?;
        for (index, receipt) in self.input_receipts.iter().enumerate() {
            let stream = &mut files[index];
            stream.seek(SeekFrom::Start(0))?;
            let mut hash = Sha256::new();
            let mut total = 0;
            let mut buffer = vec![0; 1024 * 1024];
            loop {
                budget.check()?;
                let count = stream.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
                total += count as u64;
            }
            if total != receipt.bytes || c::hex(&hash.finalize()) != receipt.sha256 {
                return Err(Error::Refused("Captured provenance bytes changed"));
            }
            let path_metadata = std::fs::symlink_metadata(self.path(receipt.name))?;
            if !path_metadata.is_file() || path_metadata.len() != receipt.bytes {
                return Err(Error::Refused("Private path changed"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let held = stream.metadata()?;
                if held.dev() != path_metadata.dev()
                    || held.ino() != path_metadata.ino()
                    || held.nlink() != 1
                    || path_metadata.nlink() != 1
                {
                    return Err(Error::Refused("Private physical identity changed"));
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                let path_file = OpenOptions::new()
                    .read(true)
                    .share_mode(3)
                    .custom_flags(0x00200000)
                    .open(self.path(receipt.name))?;
                let held = physical_identity(stream)?;
                let current = physical_identity(&path_file)?;
                if held != current || held.2 != 1 {
                    return Err(Error::Refused("Private physical identity changed"));
                }
            }
        }
        Ok(())
    }
}
#[cfg(windows)]
fn physical_identity(file: &File) -> Result<(u32, u64, u32)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Information {
        attributes: u32,
        creation: [u32; 2],
        access: [u32; 2],
        write: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut Information,
        ) -> i32;
    }
    let mut information = std::mem::MaybeUninit::<Information>::uninit();
    // One initialized owned File handle and one correctly sized output structure;
    // no manifest path, executable load or source application handle is involved.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    let information = unsafe { information.assume_init() };
    if information.attributes & 0x410 != 0 {
        return Err(Error::Refused("Private captured file is directory/reparse"));
    }
    Ok((
        information.volume,
        (u64::from(information.index_high) << 32) | u64::from(information.index_low),
        information.links,
    ))
}
#[derive(Clone, Copy)]
struct Budget {
    deadline: Instant,
}
impl Budget {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(120),
        }
    }
    fn check(self) -> Result<()> {
        if Instant::now() > self.deadline {
            Err(Error::Refused("Historical verification time limit"))
        } else {
            Ok(())
        }
    }
}

fn guarded_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn capture(
    mut input: UntrustedHistoricalStreams,
    pins: &HistoricalPins,
    budget: Budget,
) -> Result<Arc<PrivateStage>> {
    let directory = tempfile::Builder::new()
        .prefix(".cowork-inactive-history-")
        .tempdir()?;
    #[cfg(windows)]
    let mut namespace = Vec::new();
    #[cfg(not(windows))]
    let namespace = Vec::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        for path in directory.path().ancestors() {
            if std::fs::symlink_metadata(path)?.file_attributes() & 0x400 != 0 {
                return Err(Error::Refused("Private namespace reparse point"));
            }
            let guard = OpenOptions::new()
                .access_mode(1)
                .share_mode(3)
                .custom_flags(0x02200000)
                .open(path)?;
            if guard.metadata()?.file_attributes() & 0x400 != 0 {
                return Err(Error::Refused("Private held namespace reparse point"));
            }
            namespace.push(guard);
        }
    }
    let entries: [(&'static str, &mut dyn Read, u64, &Sha256Pin); 5] = [
        (
            "quarantine.json",
            &mut *input.quarantine_manifest,
            MAX_MANIFEST as u64,
            &pins.quarantine_manifest,
        ),
        (
            "quarantine.sqlite3",
            &mut *input.quarantine_database,
            MAX_QUARANTINE,
            &pins.quarantine_database,
        ),
        (
            "mapping.json",
            &mut *input.mapping,
            MAX_MANIFEST as u64,
            &pins.mapping,
        ),
        (
            "preview.json",
            &mut *input.preview,
            MAX_PREVIEW as u64,
            &pins.preview,
        ),
        (
            "cold-target.sqlite3",
            &mut *input.cold_target_snapshot,
            MAX_SNAPSHOT,
            &pins.cold_target_snapshot,
        ),
    ];
    let mut files = Vec::new();
    let mut receipts = Vec::new();
    let mut buffer = vec![0; 1024 * 1024];
    for (name, reader, maximum, pin) in entries {
        let mut file = guarded_options().open(directory.path().join(name))?;
        let mut digest = Sha256::new();
        let mut bytes = 0;
        loop {
            budget.check()?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            if bytes > maximum {
                return Err(Error::Refused("Captured historical byte limit"));
            }
            file.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
        }
        file.flush()?;
        file.sync_all()?;
        let sha256 = c::hex(&digest.finalize());
        if sha256 != pin.0 {
            return Err(Error::Refused("Explicit historical byte pin mismatch"));
        }
        files.push(file);
        receipts.push(CapturedFile {
            name,
            bytes,
            sha256,
        });
    }
    Ok(Arc::new(PrivateStage {
        files: Mutex::new(files),
        _namespace: namespace,
        input_receipts: receipts,
        directory,
    }))
}

struct HistoricalConnection {
    inner: Option<SqliteConnection>,
    stage: Arc<PrivateStage>,
}
static PRIVATE_CLOSE_OPERATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
struct CloseTicket;
impl Drop for CloseTicket {
    fn drop(&mut self) {
        PRIVATE_CLOSE_OPERATIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
struct CloseJob {
    conn: SqliteConnection,
    stage: Arc<PrivateStage>,
    ticket: CloseTicket,
    reply: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}
fn finish_private_close(job: CloseJob) {
    struct ThreadWake(std::thread::Thread);
    impl std::task::Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let CloseJob {
        conn,
        stage,
        ticket,
        reply,
    } = job;
    let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    // optimize_on_close is explicitly disabled. SQLx close only queues shutdown
    // and awaits its own worker acknowledgment; it needs no new async runtime.
    let mut close = conn.close();
    let result = loop {
        match close.as_mut().poll(&mut context) {
            std::task::Poll::Ready(result) => {
                if result.is_err() {
                    eprintln!("Owned historical SQLite close failed");
                }
                break result.map_err(Error::from);
            }
            std::task::Poll::Pending => std::thread::park(),
        }
    };
    drop(close);
    drop(stage);
    drop(ticket);
    if let Some(reply) = reply {
        let _ = reply.send(result);
    }
}
fn start_private_close(
    conn: SqliteConnection,
    stage: &Arc<PrivateStage>,
    reply: Option<tokio::sync::oneshot::Sender<Result<()>>>,
) {
    PRIVATE_CLOSE_OPERATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let job = Arc::new(Mutex::new(Some(CloseJob {
        conn,
        stage: Arc::clone(stage),
        ticket: CloseTicket,
        reply,
    })));
    let worker = Arc::clone(&job);
    let launched = std::thread::Builder::new()
        .name("historical-private-close".to_owned())
        .spawn(move || {
            if let Some(job) = worker.lock().expect("Own close job poisoned").take() {
                finish_private_close(job);
            }
        });
    if launched.is_err() {
        // Resource exhaustion cannot drop the Stage before the SQLite handle.
        // Fall back to completing this bounded private close synchronously.
        eprintln!("Historical close worker unavailable; closing owned handle synchronously");
        if let Some(job) = job.lock().expect("Own close job poisoned").take() {
            finish_private_close(job);
        }
    }
}
impl std::ops::Deref for HistoricalConnection {
    type Target = SqliteConnection;
    fn deref(&self) -> &Self::Target {
        self.inner
            .as_ref()
            .expect("Owned connection already closed")
    }
}
impl std::ops::DerefMut for HistoricalConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_mut()
            .expect("Owned connection already closed")
    }
}
impl HistoricalConnection {
    async fn close(mut self) -> Result<()> {
        if let Some(conn) = self.inner.take() {
            // The close acknowledgment waiter itself can be canceled. The
            // independent worker still retains Stage and finishes native close.
            let (sender, receiver) = tokio::sync::oneshot::channel();
            start_private_close(conn, &self.stage, Some(sender));
            receiver
                .await
                .map_err(|_| Error::Refused("Private close acknowledgment lost"))??;
        }
        Ok(())
    }
}
impl Drop for HistoricalConnection {
    fn drop(&mut self) {
        let Some(conn) = self.inner.take() else {
            return;
        };
        // SQLx removes callbacks BEFORE dropping its native database handle. A
        // callback-held TempDir could therefore leak after failed Windows deletion.
        // Hold the private Stage independently through actual close acknowledgment.
        // A dedicated close thread survives request/runtime cancellation and uses
        // only this already-owned private handle; it opens no caller-supplied path.
        start_private_close(conn, &self.stage, None);
    }
}
async fn connection(
    stage: &Arc<PrivateStage>,
    name: &str,
    readonly: bool,
    budget: Budget,
) -> Result<HistoricalConnection> {
    let receiver = begin_connection(
        stage,
        name,
        readonly,
        budget,
        #[cfg(test)]
        None,
    )?;
    receiver
        .await
        .map_err(|_| Error::Refused("Private startup acknowledgment lost"))?
}
static PRIVATE_BUILD_OPERATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
struct BuildTicket;
impl Drop for BuildTicket {
    fn drop(&mut self) {
        PRIVATE_BUILD_OPERATIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
struct BuildJob {
    stage: Arc<PrivateStage>,
    name: &'static str,
    readonly: bool,
    budget: Budget,
    reply: tokio::sync::oneshot::Sender<Result<HistoricalConnection>>,
    ticket: BuildTicket,
    #[cfg(test)]
    hook: Option<StartupHook>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum StartupPhase {
    MemoryReady,
    AttachmentReady,
}
#[cfg(test)]
struct StartupHook {
    phase: StartupPhase,
    arrived: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
    fail_after_attach: bool,
}
#[cfg(test)]
fn startup_pause(hook: &Option<StartupHook>, phase: StartupPhase) -> Result<()> {
    if let Some(hook) = hook
        && hook.phase == phase
    {
        let _ = hook.arrived.send(());
        hook.resume
            .recv()
            .map_err(|_| Error::Refused("Private test startup gate lost"))?;
    }
    Ok(())
}
fn poll_owned<F: std::future::Future>(future: F) -> F::Output {
    struct ThreadWake(std::thread::Thread);
    impl std::task::Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}
fn attachment_uri(stage: &PrivateStage, name: &str, readonly: bool) -> Result<String> {
    let path = stage.path(name);
    let raw = path
        .to_str()
        .ok_or(Error::Refused("Private SQLite path must be Unicode"))?;
    if !path.is_absolute() || raw.len() > 32768 || raw.as_bytes().contains(&0) {
        return Err(Error::Refused("Invalid own private SQLite path"));
    }
    #[cfg(windows)]
    let normalized = raw.replace('\\', "/");
    #[cfg(windows)]
    let raw = normalized.as_str();
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'/')
        .remove(b':')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.');
    let encoded = percent_encoding::utf8_percent_encode(raw, PATH).to_string();
    Ok(format!(
        "file:{encoded}?mode={}{}",
        if readonly { "ro" } else { "rw" },
        if readonly { "&immutable=1" } else { "" }
    ))
}
async fn build_owned_connection(job: &BuildJob) -> Result<HistoricalConnection> {
    job.budget.check()?;
    // SQLx establishes a worker and executes internal PRAGMAs before returning
    // a raw handle. That unwrapped phase is exclusively anonymous :memory:.
    // No private/caller file is opened until the close-ACK wrapper below exists.
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(false)
        .optimize_on_close(false, None)
        .busy_timeout(Duration::from_secs(1))
        .pragma("trusted_schema", "OFF");
    let inner = SqliteConnection::connect_with(&options).await?;
    let mut conn = HistoricalConnection {
        inner: Some(inner),
        stage: Arc::clone(&job.stage),
    };
    let setup = async {
        let deadline = job.budget.deadline;
        conn.lock_handle()
            .await?
            .set_progress_handler(1000, move || Instant::now() <= deadline);
        #[cfg(test)]
        startup_pause(&job.hook, StartupPhase::MemoryReady)?;
        job.budget.check()?;
        let uri = attachment_uri(&job.stage, job.name, job.readonly)?;
        sqlx::query("ATTACH DATABASE ? AS history_data")
            .bind(uri)
            .execute(&mut *conn)
            .await?;
        #[cfg(test)]
        startup_pause(&job.hook, StartupPhase::AttachmentReady)?;
        job.budget.check()?;
        #[cfg(test)]
        if job.hook.as_ref().is_some_and(|h| h.fail_after_attach) {
            sqlx::query("SELECT * FROM intentionally_absent_startup_table")
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query(if job.readonly {
            "PRAGMA foreign_keys=ON;PRAGMA query_only=ON"
        } else {
            "PRAGMA foreign_keys=OFF"
        })
        .execute(&mut *conn)
        .await?;
        Ok::<(), Error>(())
    }
    .await;
    if let Err(error) = setup {
        conn.close().await?;
        return Err(error);
    }
    Ok(conn)
}
fn run_build(job: BuildJob) {
    let result = poll_owned(build_owned_connection(&job));
    if let Err(abandoned) = job.reply.send(result)
        && let Ok(conn) = abandoned
    {
        // Losing the caller receiver does not cancel initialization or drop a
        // native connection. The owned worker waits for real shutdown ACK.
        if poll_owned(conn.close()).is_err() {
            eprintln!("Abandoned historical startup close failed");
        }
    }
    drop(job.ticket);
}
fn begin_connection(
    stage: &Arc<PrivateStage>,
    name: &str,
    readonly: bool,
    budget: Budget,
    #[cfg(test)] hook: Option<StartupHook>,
) -> Result<tokio::sync::oneshot::Receiver<Result<HistoricalConnection>>> {
    let name = match name {
        "quarantine.sqlite3" => "quarantine.sqlite3",
        "cold-target.sqlite3" => "cold-target.sqlite3",
        "source-validation.sqlite3" if !readonly => "source-validation.sqlite3",
        "source-validation.sqlite3" => "source-validation.sqlite3",
        "projection-validation.sqlite3" if !readonly => "projection-validation.sqlite3",
        _ => return Err(Error::Refused("Unknown own SQLite attachment")),
    };
    if !readonly
        && !matches!(
            name,
            "source-validation.sqlite3" | "projection-validation.sqlite3"
        )
    {
        return Err(Error::Refused(
            "Captured history inputs are always readonly",
        ));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    PRIVATE_BUILD_OPERATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let job = Arc::new(Mutex::new(Some(BuildJob {
        stage: Arc::clone(stage),
        name,
        readonly,
        budget,
        reply: sender,
        ticket: BuildTicket,
        #[cfg(test)]
        hook,
    })));
    let worker = Arc::clone(&job);
    if std::thread::Builder::new()
        .name("historical-private-build".to_owned())
        .spawn(move || {
            if let Some(job) = worker.lock().expect("Own build job poisoned").take() {
                run_build(job);
            }
        })
        .is_err()
    {
        eprintln!("Historical startup worker unavailable; building owned handle synchronously");
        if let Some(job) = job.lock().expect("Own build job poisoned").take() {
            run_build(job);
        }
    }
    Ok(receiver)
}
pub(super) fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn policy() -> Value {
    json!({"historical_only":true,"activate":false,"resume":false,"replay":false,"regrant":false,"issue_credentials":false,"schedule_execution":false})
}
fn uuid(value: &Value) -> Result<Uuid> {
    let raw = c::string(value)?;
    let id = Uuid::parse_str(raw)
        .map_err(|_| Error::Refused("Canonical nonnil history target UUID required"))?;
    if id.is_nil() || id.to_string() != raw {
        return Err(Error::Refused(
            "Canonical nonnil history target UUID required",
        ));
    }
    Ok(id)
}

fn export_header(export: &Value, fixed: &Value) -> Result<Value> {
    c::object(
        export,
        &[
            "format",
            "stage",
            "execution_policy",
            "inventory_sha256",
            "authority_sha256",
            "inventory_manifest_sha256",
            "source_files",
            "snapshot",
            "source_bindings",
            "details",
            "content",
            "content_sha256",
            "canonical_cells",
            "manifest_sha256",
        ],
    )?;
    c::sealed(export)?;
    if export["format"] != "localai-cowork-v1-sqlite-history/1"
        || export["stage"] != "readonly_export"
        || export["execution_policy"] != policy()
        || export["source_bindings"] != fixed["source_bindings"]
    {
        return Err(Error::Refused(
            "Unknown historical export format/policy/source binding",
        ));
    }
    if export["canonical_cells"]
        != "integer-decimal/null/text-utf8-bytes-base64/blob-base64/finite-real-ieee754-binary64-be"
    {
        return Err(Error::Refused("Unknown historical typed encoding"));
    }
    for key in [
        "inventory_sha256",
        "authority_sha256",
        "inventory_manifest_sha256",
        "content_sha256",
    ] {
        c::sha(&export[key])?;
    }
    if c::sha(&export["content_sha256"])? != c::hash_value(&export["content"])? {
        return Err(Error::Refused("Complete export content seal"));
    }
    let files = export["source_files"]
        .as_array()
        .ok_or(Error::Refused("Original provenance file set"))?;
    if files.is_empty() || files.len() > 4 {
        return Err(Error::Refused("Original provenance file count"));
    }
    let mut paths = std::collections::BTreeSet::new();
    let mut total = 0;
    for file in files {
        c::object(file, &["bytes", "volume", "file_id", "sha256", "path"])?;
        total += c::natural(&file["bytes"], MAX_SNAPSHOT)?;
        c::natural(&file["volume"], u32::MAX as u64)?;
        c::sha(&file["sha256"])?;
        let id = c::string(&file["file_id"])?;
        let n = id
            .parse::<u64>()
            .map_err(|_| Error::Refused("Original physical identity"))?;
        if n.to_string() != id {
            return Err(Error::Refused("Original physical identity decimal"));
        }
        if !paths.insert(c::string(&file["path"])?) || total > MAX_SNAPSHOT {
            return Err(Error::Refused(
                "Duplicate/oversized original byte provenance",
            ));
        }
    }
    c::object(&export["snapshot"], &["file", "bytes", "sha256"])?;
    if export["snapshot"]["file"] != "snapshot.sqlite3" {
        return Err(Error::Refused("Unknown original snapshot name"));
    }
    c::natural(&export["snapshot"]["bytes"], MAX_SNAPSHOT)?;
    c::sha(&export["snapshot"]["sha256"])?;
    let details = &export["details"];
    c::object(
        details,
        &[
            "family",
            "version",
            "profile",
            "schema",
            "schema_sha256",
            "row_counts",
            "sqlite_metadata",
        ],
    )?;
    let selected = profiles::source(
        fixed,
        c::string(&details["family"])?,
        &details["version"],
        c::string(&details["profile"])?,
    )?;
    if details["schema_sha256"] != selected["schema_sha256"]
        || profiles::signature(&details["schema"])? != c::string(&selected["schema_sha256"])?
    {
        return Err(Error::Refused("Unknown/changed complete source schema"));
    }
    c::object(
        &details["sqlite_metadata"],
        &[
            "user_version",
            "application_id",
            "schema_version",
            "page_size",
        ],
    )?;
    for value in details["sqlite_metadata"]
        .as_object()
        .ok_or(Error::Refused("SQLite metadata"))?
        .values()
    {
        if value.as_i64().is_none() {
            return Err(Error::Refused("SQLite metadata integer type"));
        }
    }
    if details["sqlite_metadata"]["user_version"] != 0
        || details["sqlite_metadata"]["application_id"] != 0
    {
        return Err(Error::Refused("Unknown historical SQLite metadata"));
    }
    let tables = selected["schema"]["tables"]
        .as_object()
        .ok_or(Error::Refused("Fixed source tables"))?;
    let counts = details["row_counts"]
        .as_object()
        .ok_or(Error::Refused("Source row counts"))?;
    let content = export["content"]
        .as_object()
        .ok_or(Error::Refused("Source content"))?;
    if counts.keys().ne(tables.keys()) || content.keys().ne(tables.keys()) {
        return Err(Error::Refused("Incomplete historical source tables"));
    }
    let mut rows = 0;
    let mut bytes = 0;
    for (ordinal, (name, metadata)) in tables.iter().enumerate() {
        let item = &content[name];
        c::object(item, &["file", "rows", "bytes", "sha256", "columns"])?;
        let columns: Vec<_> = metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Fixed columns"))?
            .iter()
            .map(|v| v[1].clone())
            .collect();
        if item["file"] != format!("table-{ordinal:03}.jsonl")
            || item["columns"] != json!(columns)
            || item["rows"] != counts[name]
        {
            return Err(Error::Refused("Historical table path/column/count binding"));
        }
        rows += c::natural(&item["rows"], MAX_ROWS)?;
        bytes += c::natural(&item["bytes"], MAX_CONTENT)?;
        c::sha(&item["sha256"])?;
    }
    if rows > MAX_ROWS || bytes > MAX_CONTENT {
        return Err(Error::Refused("Complete historical row/content limit"));
    }
    Ok(selected.clone())
}

struct TableProof {
    name: String,
    columns: Vec<String>,
    rows: Arc<[[u8; 32]]>,
}
/// No deserializer, public path, SQLite handle, operational RunRecord or grant is exposed.
pub struct VerifiedHistoryArchive {
    provenance: HistoricalProvenance,
    tables: Vec<TableProof>,
    mapping: Vec<mapping::HistoricalMapping>,
    projections: Vec<projection::ProjectionProof>,
    stage: Arc<PrivateStage>,
}
impl VerifiedHistoryArchive {
    pub fn provenance(&self) -> &HistoricalProvenance {
        &self.provenance
    }
    pub fn mapping(&self) -> &[mapping::HistoricalMapping] {
        &self.mapping
    }
    pub async fn projections(&self) -> Result<HistoricalProjections> {
        let budget = Budget::new();
        let conn = connection(&self.stage, "quarantine.sqlite3", true, budget).await?;
        Ok(HistoricalProjections {
            conn,
            proofs: self
                .projections
                .iter()
                .map(|p| projection::ProjectionProof {
                    name: p.name,
                    hashes: p.hashes.clone(),
                })
                .collect(),
            table: 0,
            position: 0,
            last: Vec::new(),
            budget,
        })
    }
    pub async fn rows(&self) -> Result<HistoricalRows> {
        let budget = Budget::new();
        let conn = connection(&self.stage, "quarantine.sqlite3", true, budget).await?;
        Ok(HistoricalRows {
            conn,
            tables: self
                .tables
                .iter()
                .map(|p| TableProof {
                    name: p.name.clone(),
                    columns: p.columns.clone(),
                    rows: p.rows.clone(),
                })
                .collect(),
            table: 0,
            position: 0,
            budget,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HistoricalProject {
    pub original_id: String,
    pub source_table: Option<String>,
    pub source_rowid: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct HistoricalActor {
    pub original_id: String,
}
#[derive(Clone, Debug)]
pub struct HistoricalEntity {
    pub kind: EntityKind,
    pub original_id: String,
    pub source_table: Option<String>,
    pub source_rowid: Option<i64>,
    pub original_project_id: Option<String>,
    pub original_actor_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoricalRelationKind {
    ProjectThread,
    DaemonProject,
    ParentRun,
    RunArtifact,
}
#[derive(Clone, Debug)]
pub struct HistoricalRelation {
    pub kind: HistoricalRelationKind,
    pub source_kind: EntityKind,
    pub original_source_id: String,
    pub target_kind: EntityKind,
    pub original_target_id: String,
}
/// Inert history records have no conversion into operational RunSpec/StoredRun.
#[derive(Clone, Debug)]
pub enum HistoricalProjection {
    Project(HistoricalProject),
    Actor(HistoricalActor),
    Entity(HistoricalEntity),
    Relation(HistoricalRelation),
}
pub struct HistoricalProjections {
    conn: HistoricalConnection,
    proofs: Vec<projection::ProjectionProof>,
    table: usize,
    position: usize,
    last: Vec<String>,
    budget: Budget,
}
impl HistoricalProjections {
    pub async fn next(&mut self) -> Result<Option<HistoricalProjection>> {
        self.budget.check()?;
        while self.table < self.proofs.len() {
            let proof = &self.proofs[self.table];
            if self.position == proof.hashes.len() {
                self.table += 1;
                self.position = 0;
                self.last.clear();
                continue;
            }
            let (_, columns) = projection::PROJECTIONS
                .iter()
                .find(|(name, _)| *name == proof.name)
                .ok_or(Error::Refused("Unknown verified projection"))?;
            let key_count = match proof.name {
                "history_projects" | "history_actors" => 1,
                "history_entities" => 2,
                "history_relations" => 5,
                _ => return Err(Error::Refused("Unknown projection key")),
            };
            let keys = &columns[..key_count];
            let after = if self.last.is_empty() {
                String::new()
            } else {
                format!(
                    " AND ({})>({})",
                    keys.iter().map(|v| quote(v)).collect::<Vec<_>>().join(","),
                    vec!["?"; key_count].join(",")
                )
            };
            let bounded = columns
                .iter()
                .map(|v| {
                    format!(
                        "({} IS NULL OR length(CAST({} AS BLOB))<=256)",
                        quote(v),
                        quote(v)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let query = format!(
                "SELECT * FROM {} WHERE {}{} ORDER BY {} LIMIT 1",
                quote(proof.name),
                bounded,
                after,
                columns
                    .iter()
                    .map(|v| quote(v))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut query = sqlx::query(&query);
            for key in &self.last {
                query = query.bind(key);
            }
            let row = query
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(Error::Refused("Verified projection missing"))?;
            let value = projection::projection_value(proof.name, &row)?;
            if c::hash(&c::canonical(&value)?) != proof.hashes[self.position] {
                return Err(Error::Refused(
                    "Historical projection changed after verification",
                ));
            }
            let typed = match proof.name {
                "history_projects" => HistoricalProjection::Project(HistoricalProject {
                    original_id: row.try_get(0)?,
                    source_table: row.try_get(1)?,
                    source_rowid: row.try_get(2)?,
                }),
                "history_actors" => HistoricalProjection::Actor(HistoricalActor {
                    original_id: row.try_get(0)?,
                }),
                "history_entities" => HistoricalProjection::Entity(HistoricalEntity {
                    kind: EntityKind::parse(&row.try_get::<String, _>(0)?)?,
                    original_id: row.try_get(1)?,
                    source_table: row.try_get(2)?,
                    source_rowid: row.try_get(3)?,
                    original_project_id: row.try_get(4)?,
                    original_actor_id: row.try_get(5)?,
                }),
                "history_relations" => {
                    let kind = match row.try_get::<String, _>(0)?.as_str() {
                        "project_thread" => HistoricalRelationKind::ProjectThread,
                        "daemon_project" => HistoricalRelationKind::DaemonProject,
                        "parent_run" => HistoricalRelationKind::ParentRun,
                        "run_artifact" => HistoricalRelationKind::RunArtifact,
                        _ => return Err(Error::Refused("Unknown original relation kind")),
                    };
                    HistoricalProjection::Relation(HistoricalRelation {
                        kind,
                        source_kind: EntityKind::parse(&row.try_get::<String, _>(1)?)?,
                        original_source_id: row.try_get(2)?,
                        target_kind: EntityKind::parse(&row.try_get::<String, _>(3)?)?,
                        original_target_id: row.try_get(4)?,
                    })
                }
                _ => return Err(Error::Refused("Unknown original projection")),
            };
            self.last = (0..key_count)
                .map(|at| row.try_get::<String, _>(at))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            self.position += 1;
            return Ok(Some(typed));
        }
        Ok(None)
    }
    pub async fn close(self) -> Result<()> {
        self.conn.close().await?;
        Ok(())
    }
}

fn seed(stage: &PrivateStage, name: &str, bytes: &[u8], expected_sha: Option<&str>) -> Result<()> {
    if expected_sha.is_some_and(|sha| c::hex(&c::hash(bytes)) != sha) {
        return Err(Error::Refused("Fixed source seed bytes changed"));
    }
    let mut file = guarded_options().open(stage.path(name))?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    // Derivative databases use only reviewed fixed seed bytes. Release this own
    // writer before SQLx reconstructs the historical source inside its own TX.
    drop(file);
    Ok(())
}
fn guard_derivative(stage: &PrivateStage, name: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let file = options.open(stage.path(name))?;
    stage
        .files
        .lock()
        .map_err(|_| Error::Refused("Private guard poisoned"))?
        .push(file);
    Ok(())
}

/// Admission proves provenance only. This makes no live write, quota reservation,
/// session check, credential export, scheduling call or activation permit.
pub async fn verify(
    input: UntrustedHistoricalStreams,
    pins: HistoricalPins,
) -> Result<VerifiedHistoryArchive> {
    let budget = Budget::new();
    let fixed = profiles::fixed()?;
    let stage = capture(input, &pins, budget)?;
    let manifest = c::parse(&stage.read("quarantine.json", MAX_MANIFEST)?, MAX_MANIFEST)?;
    let mapping = c::parse(&stage.read("mapping.json", MAX_MANIFEST)?, MAX_MANIFEST)?;
    let preview = c::parse(&stage.read("preview.json", MAX_PREVIEW)?, MAX_PREVIEW)?;
    let mut quarantine = connection(&stage, "quarantine.sqlite3", true, budget).await?;
    let (export, selected) =
        validate_quarantine(&mut quarantine, &manifest, &pins, &stage, &fixed).await?;
    let source_profile = c::string(&selected["profile"])?;
    let source_seed =
        seeds::source_seed(source_profile).ok_or(Error::Refused("Unknown fixed source seed"))?;
    seed(
        &stage,
        "source-validation.sqlite3",
        source_seed,
        Some(c::sha(&selected["seed_sha256"])?),
    )?;
    seed(
        &stage,
        "projection-validation.sqlite3",
        include_bytes!("verifier/data/quarantine-seed.sqlite3"),
        None,
    )?;
    let mut source = connection(&stage, "source-validation.sqlite3", false, budget).await?;
    let mut expected = connection(&stage, "projection-validation.sqlite3", false, budget).await?;
    profiles::schema(&mut source, &selected["schema"]).await?;
    profiles::schema(&mut expected, &fixed["quarantine"]["schema"]).await?;
    let tables = reconstructed_rows(
        &mut quarantine,
        &mut source,
        &mut expected,
        &export,
        &selected,
        budget,
    )
    .await?;
    source.close().await?;
    guard_derivative(&stage, "source-validation.sqlite3")?;
    let mut source = connection(&stage, "source-validation.sqlite3", true, budget).await?;
    projection::derive(&mut source, &mut expected, &selected, budget).await?;
    let projections = projection::compare(&mut quarantine, &mut expected, budget).await?;
    sqlx::query("COMMIT").execute(&mut *expected).await?;
    profiles::integrity(&mut expected).await?;
    expected.close().await?;
    source.close().await?;
    let mut target = connection(&stage, "cold-target.sqlite3", true, budget).await?;
    profiles::historical_target(&mut target, &fixed).await?;
    let mapping = mapping::verify(
        &mut quarantine,
        &mut target,
        &mapping,
        &preview,
        &fixed,
        &pins,
        budget,
    )
    .await?;
    target.close().await?;
    quarantine.close().await?;
    stage.check_receipts(budget)?;
    let rows = export["content"]
        .as_object()
        .ok_or(Error::Refused("Complete original content"))?
        .values()
        .map(|v| c::natural(&v["rows"], MAX_ROWS))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum();
    let bytes = export["content"]
        .as_object()
        .ok_or(Error::Refused("Complete original content"))?
        .values()
        .map(|v| c::natural(&v["bytes"], MAX_CONTENT))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum();
    let provenance = HistoricalProvenance {
        pins,
        source_profile: source_profile.to_owned(),
        source_family: c::string(&selected["family"])?.to_owned(),
        source_version: selected["version"].as_u64(),
        original_rows: rows,
        original_canonical_bytes: bytes,
        historical_target_profile: c::string(&fixed["historical_target"]["profile"])?.to_owned(),
        verifier_contract: "localai-cowork-rust-inert-history/1-installation-singleton",
    };
    Ok(VerifiedHistoryArchive {
        provenance,
        tables,
        mapping,
        projections,
        stage,
    })
}
pub struct HistoricalRows {
    conn: HistoricalConnection,
    tables: Vec<TableProof>,
    table: usize,
    position: usize,
    budget: Budget,
}
impl HistoricalRows {
    pub async fn next(&mut self) -> Result<Option<HistoricalRow>> {
        self.budget.check()?;
        while self.table < self.tables.len() {
            let table = &self.tables[self.table];
            if self.position == table.rows.len() {
                self.table += 1;
                self.position = 0;
                continue;
            }
            let position = self.position + 1;
            let row=sqlx::query("SELECT source_rowid,canonical_row FROM history_rows WHERE table_name=? AND position=? AND length(canonical_row)<=?")
                .bind(&table.name).bind(position as i64).bind(MAX_LINE as i64).fetch_optional(&mut *self.conn).await?.ok_or(Error::Refused("Verified historical row missing/oversized"))?;
            let raw: Vec<u8> = row.try_get(1)?;
            if c::hash(&raw) != table.rows[self.position] {
                return Err(Error::Refused("Historical row changed after verification"));
            }
            let typed = decode_row(
                &raw,
                &table.name,
                row.try_get(0)?,
                position as u64,
                table.columns.len(),
            )?;
            self.position = position;
            return Ok(Some(typed));
        }
        Ok(None)
    }
    pub async fn close(self) -> Result<()> {
        self.conn.close().await?;
        Ok(())
    }
}

fn decode_row(
    raw: &[u8],
    table: &str,
    rowid: i64,
    position: u64,
    columns: usize,
) -> Result<HistoricalRow> {
    let value = c::parse(raw, MAX_LINE)?;
    c::object(&value, &["rowid", "cells"])?;
    if c::decimal(c::string(&value["rowid"])?)? != rowid {
        return Err(Error::Refused("Original rowid binding differs"));
    }
    let mut canonical = c::canonical(&value)?;
    canonical.push(b'\n');
    if raw != canonical {
        return Err(Error::Refused("Noncanonical original typed row bytes"));
    }
    let cells = value["cells"]
        .as_array()
        .ok_or(Error::Refused("Original typed cells"))?;
    if cells.len() != columns {
        return Err(Error::Refused("Original typed column count"));
    }
    Ok(HistoricalRow {
        source_table: table.to_owned(),
        source_rowid: rowid,
        position,
        cells: cells.iter().map(c::cell).collect::<Result<_>>()?,
    })
}

async fn validate_quarantine(
    conn: &mut SqliteConnection,
    manifest: &Value,
    pins: &HistoricalPins,
    stage: &PrivateStage,
    fixed: &Value,
) -> Result<(Value, Value)> {
    c::object(
        manifest,
        &[
            "format",
            "stage",
            "state",
            "execution_policy",
            "export_sha256",
            "source_profile",
            "database",
            "rows",
            "entities",
            "schema_sha256",
            "manifest_sha256",
        ],
    )?;
    c::sealed(manifest)?;
    if manifest["format"] != QUARANTINE_FORMAT
        || manifest["stage"] != "inactive_quarantine"
        || manifest["state"] != "inactive"
        || manifest["execution_policy"] != policy()
    {
        return Err(Error::Refused(
            "Unknown quarantine format or executable authority",
        ));
    }
    c::natural(&manifest["rows"], MAX_ROWS)?;
    c::sha(&manifest["export_sha256"])?;
    c::sha(&manifest["schema_sha256"])?;
    if manifest["export_sha256"] != pins.export_manifest.0
        || manifest["schema_sha256"] != fixed["quarantine"]["schema_sha256"]
    {
        return Err(Error::Refused("Quarantine origin/schema pin differs"));
    }
    c::object(&manifest["database"], &["file", "bytes", "sha256"])?;
    let database = &stage.input_receipts[1];
    if manifest["database"]
        != json!({"file":"quarantine.sqlite3","bytes":database.bytes,"sha256":database.sha256})
    {
        return Err(Error::Refused("Quarantine database receipt differs"));
    }
    profiles::schema(conn, &fixed["quarantine"]["schema"]).await?;
    profiles::integrity(conn).await?;
    let meta=sqlx::query("SELECT id,format,state,export_sha256,export_manifest,execution_policy,length(export_manifest) FROM quarantine_meta WHERE length(export_manifest)<=?")
        .bind(MAX_MANIFEST as i64).fetch_all(&mut *conn).await?;
    if meta.len() != 1
        || meta[0].try_get::<i64, _>(0)? != 1
        || meta[0].try_get::<String, _>(1)? != QUARANTINE_FORMAT
        || meta[0].try_get::<String, _>(2)? != "inactive"
        || meta[0].try_get::<String, _>(3)? != pins.export_manifest.0
        || meta[0].try_get::<Vec<u8>, _>(5)? != c::canonical(&policy())?
    {
        return Err(Error::Refused(
            "Quarantine metadata has another origin/authority",
        ));
    }
    let raw: Vec<u8> = meta[0].try_get(4)?;
    if c::hex(&c::hash(&raw)) != pins.export_manifest.0 {
        return Err(Error::Refused("Embedded original export SHA differs"));
    }
    let export = c::parse(&raw, MAX_MANIFEST)?;
    let selected = export_header(&export, fixed)?;
    if export["details"]["profile"] != manifest["source_profile"] {
        return Err(Error::Refused("Quarantine source profile differs"));
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM history_rows")
        .fetch_one(&mut *conn)
        .await?;
    if json!(rows) != manifest["rows"] {
        return Err(Error::Refused("Quarantine original row count differs"));
    }
    let mut entities = serde_json::Map::new();
    for row in sqlx::query("SELECT kind,count(*) FROM history_entities GROUP BY kind")
        .fetch_all(&mut *conn)
        .await?
    {
        let kind: String = row.try_get(0)?;
        mapping::EntityKind::parse(&kind)?;
        let count: i64 = row.try_get(1)?;
        c::natural(&json!(count), MAX_ROWS)?;
        entities.insert(kind, json!(count));
    }
    if manifest["entities"] != Value::Object(entities) {
        return Err(Error::Refused("Quarantine entity count/kind differs"));
    }
    Ok((export, selected))
}

async fn reconstructed_rows(
    q: &mut SqliteConnection,
    source: &mut SqliteConnection,
    expected: &mut SqliteConnection,
    export: &Value,
    selected: &Value,
    budget: Budget,
) -> Result<Vec<TableProof>> {
    let content = export["content"]
        .as_object()
        .ok_or(Error::Refused("Original content"))?;
    let mut original_tables = Vec::new();
    for row in sqlx::query(
        "SELECT name,columns_json,row_count,content_sha256 FROM source_tables ORDER BY name",
    )
    .fetch_all(&mut *q)
    .await?
    {
        original_tables.push(json!([
            row.try_get::<String, _>(0)?,
            row.try_get::<Vec<u8>, _>(1)?,
            row.try_get::<i64, _>(2)?,
            row.try_get::<String, _>(3)?
        ]));
    }
    let expected_tables: Vec<_> = content
        .iter()
        .map(|(name, item)| {
            Ok(json!([
                name,
                c::canonical(&item["columns"])?,
                item["rows"],
                item["sha256"]
            ]))
        })
        .collect::<Result<_>>()?;
    if original_tables != expected_tables {
        return Err(Error::Refused(
            "Source table metadata is not the complete original export",
        ));
    }
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *source).await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *expected)
        .await?;
    for name in content.keys() {
        sqlx::query(&format!("DELETE FROM {}", quote(name)))
            .execute(&mut *source)
            .await?;
    }
    let mut total = 0;
    let mut proofs = Vec::new();
    for (name, item) in content {
        budget.check()?;
        let columns: Vec<String> = item["columns"]
            .as_array()
            .ok_or(Error::Refused("Original columns"))?
            .iter()
            .map(|v| c::string(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        if columns
            .iter()
            .any(|c| ["rowid", "_rowid_", "oid"].contains(&c.to_ascii_lowercase().as_str()))
        {
            return Err(Error::Refused("Source rowid shadowing unsupported"));
        }
        sqlx::query("INSERT INTO source_tables VALUES(?,?,?,?)")
            .bind(name)
            .bind(c::canonical(&item["columns"])?)
            .bind(c::natural(&item["rows"], MAX_ROWS)? as i64)
            .bind(c::sha(&item["sha256"])?)
            .execute(&mut *expected)
            .await?;
        if name == "sqlite_sequence" {
            sqlx::query("DELETE FROM sqlite_sequence")
                .execute(&mut *source)
                .await?;
        }
        let mut cursor=sqlx::query("SELECT source_rowid,position,canonical_row,row_sha256 FROM history_rows WHERE table_name=? AND length(canonical_row)<=? ORDER BY position")
            .bind(name).bind(MAX_LINE as i64).fetch(&mut *q);
        let mut count = 0;
        let mut bytes = 0;
        let mut digest = Sha256::new();
        let mut table_proofs = Vec::new();
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            total += 1;
            count += 1;
            if total > MAX_ROWS {
                return Err(Error::Refused("Original row limit"));
            }
            let rowid: i64 = row.try_get(0)?;
            let position: i64 = row.try_get(1)?;
            let raw: Vec<u8> = row.try_get(2)?;
            let row_sha: String = row.try_get(3)?;
            if position != count as i64 || c::hex(&c::hash(&raw)) != row_sha {
                return Err(Error::Refused("Original row position/SHA differs"));
            }
            let typed = decode_row(&raw, name, rowid, count, columns.len())?;
            let placeholders: Vec<_> = typed
                .cells
                .iter()
                .map(|cell| {
                    if matches!(cell, HistoricalCell::TextBytes(_)) {
                        "CAST(? AS TEXT)"
                    } else {
                        "?"
                    }
                })
                .collect();
            let sql = format!(
                "INSERT INTO {}(_rowid_,{}) VALUES(?,{})",
                quote(name),
                columns
                    .iter()
                    .map(|v| quote(v))
                    .collect::<Vec<_>>()
                    .join(","),
                placeholders.join(",")
            );
            let mut insert = sqlx::query(&sql).bind(rowid);
            for cell in &typed.cells {
                insert = match cell {
                    HistoricalCell::Null => insert.bind(Option::<String>::None),
                    HistoricalCell::Integer(n) => insert.bind(*n),
                    HistoricalCell::RealBits(bits) => insert.bind(f64::from_bits(*bits)),
                    HistoricalCell::TextBytes(raw) | HistoricalCell::Blob(raw) => insert.bind(raw),
                };
            }
            insert.execute(&mut *source).await?;
            // Only projection foreign-key anchors are needed in this private derived
            // database. Original payloads remain once in the immutable captured archive.
            sqlx::query("INSERT INTO history_rows VALUES(?,?,?,X'',?)")
                .bind(name)
                .bind(rowid)
                .bind(count as i64)
                .bind(&row_sha)
                .execute(&mut *expected)
                .await?;
            digest.update(&raw);
            bytes += raw.len() as u64;
            table_proofs.push(c::hash(&raw));
        }
        if json!(count) != item["rows"]
            || json!(bytes) != item["bytes"]
            || c::hex(&digest.finalize()) != c::sha(&item["sha256"])?
        {
            return Err(Error::Refused("Incomplete original table content/hash"));
        }
        proofs.push(TableProof {
            name: name.clone(),
            columns,
            rows: Arc::from(table_proofs),
        });
    }
    sqlx::query("COMMIT").execute(&mut *source).await?;
    profiles::integrity(source).await?;
    if selected["family"] == "desktop"
        && selected["schema"]["tables"].get("schema_version").is_some()
    {
        let markers = sqlx::query("SELECT version,typeof(version) FROM schema_version LIMIT 2")
            .fetch_all(&mut *source)
            .await?;
        if markers.len() != 1
            || markers[0].try_get::<String, _>(1)? != "integer"
            || json!(markers[0].try_get::<i64, _>(0)?) != selected["version"]
        {
            return Err(Error::Refused(
                "Original schema version marker differs from complete fixed profile",
            ));
        }
    }
    roundtrip(source, export, selected, budget).await?;
    Ok(proofs)
}

async fn roundtrip(
    source: &mut SqliteConnection,
    export: &Value,
    selected: &Value,
    budget: Budget,
) -> Result<()> {
    let tables = selected["schema"]["tables"]
        .as_object()
        .ok_or(Error::Refused("Fixed tables"))?;
    for (table, metadata) in tables {
        let columns = metadata["columns"]
            .as_array()
            .ok_or(Error::Refused("Fixed columns"))?;
        let mut selection = vec!["_rowid_".to_owned()];
        let mut primary = Vec::new();
        for column in columns {
            let name = c::string(&column[1])?;
            let quoted = quote(name);
            selection.push(format!("typeof({quoted})"));
            selection.push(format!(
                "CASE WHEN typeof({quoted})='text' THEN CAST({quoted} AS BLOB) ELSE {quoted} END"
            ));
            if let Some(n) = column[5].as_i64().filter(|n| *n > 0) {
                primary.push((n, quoted));
            }
        }
        primary.sort_by_key(|(ordinal, _)| *ordinal);
        let mut order: Vec<_> = primary.into_iter().map(|(_, name)| name).collect();
        order.push("_rowid_".to_owned());
        let query = format!(
            "SELECT {} FROM {} ORDER BY {}",
            selection.join(","),
            quote(table),
            order.join(",")
        );
        let mut cursor = sqlx::query(&query).fetch(&mut *source);
        let mut count = 0;
        let mut bytes = 0;
        let mut digest = Sha256::new();
        while let Some(row) = cursor.try_next().await? {
            budget.check()?;
            count += 1;
            if count > MAX_ROWS {
                return Err(Error::Refused("Private roundtrip row limit"));
            }
            let mut cells = Vec::new();
            for index in 0..columns.len() {
                let kind: String = row.try_get(1 + index * 2)?;
                let at = 2 + index * 2;
                let cell = match kind.as_str() {
                    "null" => HistoricalCell::Null,
                    "integer" => HistoricalCell::Integer(row.try_get(at)?),
                    "real" => {
                        let value: f64 = row.try_get(at)?;
                        if !value.is_finite() {
                            return Err(Error::Refused("Private nonfinite real"));
                        }
                        HistoricalCell::RealBits(value.to_bits())
                    }
                    "text" => HistoricalCell::TextBytes(row.try_get(at)?),
                    "blob" => HistoricalCell::Blob(row.try_get(at)?),
                    _ => return Err(Error::Refused("Unknown private SQLite storage class")),
                };
                if matches!(&cell,HistoricalCell::TextBytes(raw)|HistoricalCell::Blob(raw) if raw.len()>MAX_CELL)
                {
                    return Err(Error::Refused("Private roundtrip cell limit"));
                }
                cells.push(c::encoded_cell(&cell));
            }
            let rowid: i64 = row.try_get(0)?;
            let mut raw = c::canonical(&json!({"rowid":rowid.to_string(),"cells":cells}))?;
            raw.push(b'\n');
            if raw.len() > MAX_LINE {
                return Err(Error::Refused("Roundtrip row byte limit"));
            }
            bytes += raw.len() as u64;
            digest.update(raw);
        }
        let expected = &export["content"][table];
        if json!(count) != expected["rows"]
            || json!(bytes) != expected["bytes"]
            || c::hex(&digest.finalize()) != c::sha(&expected["sha256"])?
        {
            return Err(Error::Refused(
                "Reconstructed source SQLite types/order differ",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod qualification_tests;
