//! Native Windows process isolation for AI-owned shell commands.
//!
//! The interactive terminal deliberately does not appear in this module.  A command is
//! launched as a dedicated local standard user, and the small in-process runner creates a
//! second restricted token, a private desktop and a kill-on-close job before starting the
//! requested shell.  Credentials are persisted only as DPAPI ciphertext.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

pub const SANDBOX_ACCOUNT: &str = "LACoworkOnline";
pub const SANDBOX_GROUP: &str = "LACoworkSandbox";
const SETUP_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const IPC_MAX_FRAME_BYTES: usize = 1024 * 1024;
const IPC_REQUEST: u8 = 1;
const IPC_STDOUT: u8 = 2;
const IPC_STDERR: u8 = 3;
const IPC_EXIT: u8 = 4;
const IPC_ERROR: u8 = 5;

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct RunningJob {
    handle: isize,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_os = "windows")]
fn running_jobs() -> &'static std::sync::Mutex<std::collections::HashMap<String, RunningJob>> {
    static JOBS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, RunningJob>>,
    > = std::sync::OnceLock::new();
    JOBS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn cancel(stream_id: &str) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        let jobs = running_jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(job) = jobs.get(stream_id) else {
            return Ok(false);
        };
        job.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        if unsafe { TerminateJobObject(job.handle as _, 130) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = stream_id;
        Ok(false)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub supported: bool,
    pub ready: bool,
    pub version: u32,
    pub account: String,
    pub group: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecRequest {
    pub run_id: String,
    pub command: String,
    pub shell: Option<String>,
    pub cwd: String,
    pub timeout_ms: Option<u64>,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub status: String,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub sandbox_id: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_invalid_utf8: bool,
    pub stderr_invalid_utf8: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupEnvelope {
    version: u32,
    encrypted_password: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupResult {
    ok: bool,
    version: u32,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SandboxSmokeResult {
    ok: bool,
    setup_ready: bool,
    repeated_setup_ready: bool,
    account: String,
    group: String,
    execution_status: Option<String>,
    identity_verified: bool,
    marker_written: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerRequest {
    command: String,
    shell: String,
    cwd: String,
    python_root: Option<String>,
    capability_sid: String,
    script_path: String,
}

fn write_ipc_frame<W: std::io::Write>(
    writer: &mut W,
    kind: u8,
    payload: &[u8],
) -> Result<(), String> {
    let frame_length = payload
        .len()
        .checked_add(1)
        .ok_or_else(|| "sandbox IPC frame length overflow".to_string())?;
    if frame_length > IPC_MAX_FRAME_BYTES {
        return Err(format!(
            "sandbox IPC frame is too large ({frame_length} bytes)"
        ));
    }
    writer
        .write_all(&(frame_length as u32).to_le_bytes())
        .and_then(|_| writer.write_all(&[kind]))
        .and_then(|_| writer.write_all(payload))
        .and_then(|_| writer.flush())
        .map_err(|error| error.to_string())
}

fn read_ipc_frame<R: std::io::Read>(reader: &mut R) -> Result<Option<(u8, Vec<u8>)>, String> {
    let mut length_bytes = [0u8; 4];
    match reader.read_exact(&mut length_bytes) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    }
    let frame_length = u32::from_le_bytes(length_bytes) as usize;
    if frame_length == 0 || frame_length > IPC_MAX_FRAME_BYTES {
        return Err(format!("invalid sandbox IPC frame length {frame_length}"));
    }
    let mut frame = vec![0u8; frame_length];
    reader
        .read_exact(&mut frame)
        .map_err(|error| error.to_string())?;
    Ok(Some((frame[0], frame[1..].to_vec())))
}

fn setup_dir(app_data: &Path) -> PathBuf {
    app_data.join("native_windows_sandbox")
}

fn credential_path(app_data: &Path) -> PathBuf {
    setup_dir(app_data).join("credential.dpapi")
}

fn capability_seed_path(app_data: &Path) -> PathBuf {
    setup_dir(app_data).join("capability-seed.dpapi")
}

fn setup_marker_path(app_data: &Path) -> PathBuf {
    setup_dir(app_data).join("setup.json")
}

pub fn setup_status(app_data: &Path) -> SetupStatus {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app_data;
        return SetupStatus {
            supported: false,
            ready: false,
            version: SETUP_VERSION,
            account: SANDBOX_ACCOUNT.to_string(),
            group: SANDBOX_GROUP.to_string(),
            reason: Some("native Windows sandboxing is only available on Windows".to_string()),
        };
    }

    #[cfg(target_os = "windows")]
    {
        let marker = fs::read(setup_marker_path(app_data))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SetupResult>(&bytes).ok());
        if !matches!(
            marker,
            Some(SetupResult {
                ok: true,
                version: SETUP_VERSION,
                ..
            })
        ) {
            return SetupStatus {
                supported: true,
                ready: false,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: Some("elevated sandbox setup has not completed".to_string()),
            };
        }
        if !sandbox_account_exists() {
            return SetupStatus {
                supported: true,
                ready: false,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: Some("sandbox account is missing or unavailable".to_string()),
            };
        }
        if let Err(reason) = sandbox_account_membership_ready() {
            return SetupStatus {
                supported: true,
                ready: false,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: Some(reason),
            };
        }
        match (load_password(app_data), load_capability_seed(app_data)) {
            (Ok(password), Ok(seed)) if !password.is_empty() && seed.len() == 32 => SetupStatus {
                supported: true,
                ready: true,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: None,
            },
            (Ok(_), Ok(_)) => SetupStatus {
                supported: true,
                ready: false,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: Some("sandbox credential or capability seed is invalid".to_string()),
            },
            (Err(error), _) | (_, Err(error)) => SetupStatus {
                supported: true,
                ready: false,
                version: SETUP_VERSION,
                account: SANDBOX_ACCOUNT.to_string(),
                group: SANDBOX_GROUP.to_string(),
                reason: Some(format!(
                    "sandbox protected setup data is unavailable: {error}"
                )),
            },
        }
    }
}

#[cfg(target_os = "windows")]
fn sandbox_account_exists() -> bool {
    use windows_sys::Win32::NetworkManagement::NetManagement::{
        NERR_Success, NetApiBufferFree, NetUserGetInfo,
    };
    let username = wide(SANDBOX_ACCOUNT);
    let mut buffer = std::ptr::null_mut();
    let result = unsafe { NetUserGetInfo(std::ptr::null(), username.as_ptr(), 0, &mut buffer) };
    if !buffer.is_null() {
        unsafe { NetApiBufferFree(buffer as *const core::ffi::c_void) };
    }
    result == NERR_Success
}

#[cfg(target_os = "windows")]
fn sandbox_account_membership_ready() -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::NetManagement::{
        NERR_Success, NetApiBufferFree, NetUserGetLocalGroups, LG_INCLUDE_INDIRECT,
        LOCALGROUP_USERS_INFO_0, MAX_PREFERRED_LENGTH,
    };
    let username = wide(SANDBOX_ACCOUNT);
    let mut buffer = std::ptr::null_mut();
    let mut entries = 0u32;
    let mut total = 0u32;
    let result = unsafe {
        NetUserGetLocalGroups(
            std::ptr::null(),
            username.as_ptr(),
            0,
            LG_INCLUDE_INDIRECT,
            &mut buffer,
            MAX_PREFERRED_LENGTH,
            &mut entries,
            &mut total,
        )
    };
    if result != NERR_Success {
        return Err(format!(
            "sandbox group membership could not be verified (Windows error {result})"
        ));
    }
    let administrators = localized_administrators_group()?;
    let administrators =
        String::from_utf16_lossy(&administrators[..administrators.len().saturating_sub(1)]);
    let memberships = if buffer.is_null() || entries == 0 {
        &[][..]
    } else {
        unsafe {
            std::slice::from_raw_parts(buffer as *const LOCALGROUP_USERS_INFO_0, entries as usize)
        }
    };
    let mut has_sandbox_group = false;
    let mut has_administrators = false;
    for membership in memberships {
        if membership.lgrui0_name.is_null() {
            continue;
        }
        let mut length = 0usize;
        unsafe {
            while *membership.lgrui0_name.add(length) != 0 {
                length += 1;
            }
        }
        let name = String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(membership.lgrui0_name, length)
        });
        has_sandbox_group |= name.eq_ignore_ascii_case(SANDBOX_GROUP);
        has_administrators |= name.eq_ignore_ascii_case(&administrators);
    }
    if !buffer.is_null() {
        unsafe {
            NetApiBufferFree(buffer as _);
        }
    }
    if !has_sandbox_group {
        return Err("sandbox account is not a member of its required local group".to_string());
    }
    if has_administrators {
        return Err("sandbox account is unexpectedly a local administrator".to_string());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn setup_start(app_data: &Path) -> Result<SetupStatus, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fs::create_dir_all(setup_dir(app_data)).map_err(|error| error.to_string())?;
    let password = generate_password()?;
    let encrypted_for_helper = protect_data(password.as_bytes(), true)
        .map_err(|error| format!("failed to protect the UAC setup secret: {error}"))?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let request_path = setup_dir(app_data).join(format!("setup-{nonce}.request"));
    let result_path = setup_dir(app_data).join(format!("setup-{nonce}.result"));
    let envelope = SetupEnvelope {
        version: SETUP_VERSION,
        encrypted_password: BASE64.encode(encrypted_for_helper),
    };
    write_private_file(
        &request_path,
        &serde_json::to_vec(&envelope).map_err(|e| e.to_string())?,
    )?;

    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let verb = wide("runas");
    let executable_wide = wide(&executable.display().to_string());
    let parameters = wide(&format!(
        "--lacowork-native-sandbox-setup \"{}\" \"{}\"",
        request_path.display(),
        result_path.display()
    ));
    let mut shell_info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: executable_wide.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };

    let launched = unsafe { ShellExecuteExW(&mut shell_info) };
    if launched == 0 {
        let _ = fs::remove_file(&request_path);
        return Err(format!(
            "UAC sandbox setup was not started: {}",
            std::io::Error::last_os_error()
        ));
    }
    let wait = unsafe { WaitForSingleObject(shell_info.hProcess, 120_000) };
    unsafe { CloseHandle(shell_info.hProcess) };
    let _ = fs::remove_file(&request_path);
    if wait != WAIT_OBJECT_0 {
        let _ = fs::remove_file(&result_path);
        return Err("UAC sandbox setup did not finish within two minutes".to_string());
    }
    let result_bytes = fs::read(&result_path)
        .map_err(|error| format!("elevated setup returned no result: {error}"))?;
    let _ = fs::remove_file(&result_path);
    let result: SetupResult = serde_json::from_slice(&result_bytes)
        .map_err(|error| format!("elevated setup returned an invalid result: {error}"))?;
    if !result.ok {
        return Err(result
            .error
            .unwrap_or_else(|| "elevated setup failed".to_string()));
    }

    let encrypted = protect_data(password.as_bytes(), false)
        .map_err(|error| format!("failed to protect the sandbox credential: {error}"))?;
    write_private_file(&credential_path(app_data), &encrypted)
        .map_err(|error| format!("failed to store the sandbox credential: {error}"))?;
    if !matches!(load_capability_seed(app_data), Ok(seed) if seed.len() == 32) {
        let mut capability_seed = [0u8; 32];
        getrandom::fill(&mut capability_seed).map_err(|error| error.to_string())?;
        let protected_seed = protect_data(&capability_seed, false)
            .map_err(|error| format!("failed to protect the sandbox capability seed: {error}"))?;
        capability_seed.fill(0);
        write_private_file(&capability_seed_path(app_data), &protected_seed)
            .map_err(|error| format!("failed to store the sandbox capability seed: {error}"))?;
    }
    write_private_file(
        &setup_marker_path(app_data),
        &serde_json::to_vec(&result).map_err(|e| e.to_string())?,
    )
    .map_err(|error| format!("failed to store the sandbox setup marker: {error}"))?;
    Ok(setup_status(app_data))
}

#[cfg(not(target_os = "windows"))]
pub fn setup_start(_app_data: &Path) -> Result<SetupStatus, String> {
    Err("native Windows sandboxing is only available on Windows".to_string())
}

fn capability_sid_for_run(app_data: &Path, run_id: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let seed = load_capability_seed(app_data)?;
    let mut digest = Sha256::new();
    digest.update(b"LocalAI-Cowork/native-windows-sandbox/capability/v1\0");
    digest.update(&seed);
    digest.update(b"\0");
    digest.update(run_id.as_bytes());
    let hash = digest.finalize();
    let components = (0..4)
        .map(|index| {
            let offset = index * 4;
            u32::from_le_bytes(
                hash[offset..offset + 4]
                    .try_into()
                    .expect("fixed digest slice"),
            )
        })
        .collect::<Vec<_>>();
    Ok(format!(
        "S-1-5-21-{}-{}-{}-{}",
        components[0], components[1], components[2], components[3]
    ))
}

pub fn grant_workspace_access(
    app_data: &Path,
    run_id: &str,
    workspace: &Path,
) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app_data, run_id, workspace);
        return Err("native Windows sandboxing is only available on Windows".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let capability_sid = capability_sid_for_run(app_data, run_id)?;
        let status = std::process::Command::new("icacls.exe")
            .arg(workspace)
            .args(["/grant:r"])
            .arg(format!("{}:(OI)(CI)M", SANDBOX_GROUP))
            .args(["/grant:r"])
            .arg(format!("*{}:(OI)(CI)M", capability_sid))
            .args(["/grant:r", "SYSTEM:(OI)(CI)F", "/T", "/C", "/Q"])
            .creation_flags(0x08000000)
            .status()
            .map_err(|error| format!("failed to set sandbox workspace ACL: {error}"))?;
        if !status.success() {
            return Err(format!(
                "failed to set sandbox workspace ACL (icacls {status})"
            ));
        }
        Ok(capability_sid)
    }
}

pub fn grant_workspace_access_for_roots(
    app_data: &Path,
    run_id: &str,
    workspace: &Path,
    writable_roots: &[PathBuf],
) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app_data, run_id, workspace, writable_roots);
        return Err("native Windows sandboxing is only available on Windows".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let capability_sid = capability_sid_for_run(app_data, run_id)?;
        let status = std::process::Command::new("icacls.exe")
            .arg(workspace)
            .args(["/grant:r"])
            .arg(format!("{}:(OI)(CI)RX", SANDBOX_GROUP))
            .args(["/grant:r"])
            .arg(format!("*{}:(OI)(CI)RX", capability_sid))
            .args(["/grant:r", "SYSTEM:(OI)(CI)F", "/T", "/C", "/Q"])
            .creation_flags(0x08000000)
            .status()
            .map_err(|error| format!("failed to set sandbox workspace ACL: {error}"))?;
        if !status.success() {
            return Err(format!(
                "failed to set read-only sandbox workspace ACL (icacls {status})"
            ));
        }
        for writable_root in writable_roots {
            if !writable_root.starts_with(workspace) {
                return Err("writable sandbox root escapes its workspace".to_string());
            }
            let status = std::process::Command::new("icacls.exe")
                .arg(writable_root)
                .args(["/grant:r"])
                .arg(format!("{}:(OI)(CI)M", SANDBOX_GROUP))
                .args(["/grant:r"])
                .arg(format!("*{}:(OI)(CI)M", capability_sid))
                .args(["/grant:r", "SYSTEM:(OI)(CI)F", "/T", "/C", "/Q"])
                .creation_flags(0x08000000)
                .status()
                .map_err(|error| format!("failed to set writable sandbox root ACL: {error}"))?;
            if !status.success() {
                return Err(format!(
                    "failed to set writable sandbox root ACL (icacls {status})"
                ));
            }
        }
        Ok(capability_sid)
    }
}

pub fn prepare_bundled_python(resource_dir: &Path, app_data: &Path) -> Result<PathBuf, String> {
    let destination = setup_dir(app_data).join("runtime").join("python");
    let executable = destination.join("python.exe");
    if !executable.is_file() {
        let archive_path = resource_dir.join("python").join("windows.zip");
        let file = fs::File::open(&archive_path).map_err(|error| {
            format!(
                "bundled Python archive is unavailable ({}): {error}",
                archive_path.display()
            )
        })?;
        let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
        if destination.exists() {
            fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| "bundled Python archive contains an unsafe path".to_string())?;
            let target = destination.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let mut output = fs::File::create(&target).map_err(|error| error.to_string())?;
                std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
            }
        }
    }
    if !executable.is_file() {
        return Err("bundled Python extraction did not produce python.exe".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("icacls.exe")
            .arg(&destination)
            .args(["/grant:r"])
            .arg(format!("{}:(OI)(CI)RX", SANDBOX_GROUP))
            .args(["/T", "/C", "/Q"])
            .creation_flags(0x08000000)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err("failed to grant sandbox access to bundled Python".to_string());
        }
    }
    Ok(destination)
}

#[cfg(target_os = "windows")]
pub fn grant_capability_read_access(path: &Path, capability_sid: &str) -> Result<(), String> {
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .args(["/grant:r"])
        .arg(format!("*{}:(OI)(CI)RX", capability_sid))
        .args(["/T", "/C", "/Q"])
        .creation_flags(0x08000000)
        .status()
        .map_err(|error| format!("failed to grant runtime capability ACL: {error}"))?;
    if !status.success() {
        return Err(format!(
            "failed to grant runtime capability ACL (icacls {status})"
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn grant_capability_read_access(_path: &Path, _capability_sid: &str) -> Result<(), String> {
    Err("native Windows sandboxing is only available on Windows".to_string())
}

#[cfg(target_os = "windows")]
fn sandbox_account_sid_string() -> Result<String, String> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree, ERROR_INSUFFICIENT_BUFFER};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE};

    let account = wide(SANDBOX_ACCOUNT);
    let mut sid_length = 0u32;
    let mut domain_length = 0u32;
    let mut sid_kind: SID_NAME_USE = 0;
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_length,
            std::ptr::null_mut(),
            &mut domain_length,
            &mut sid_kind,
        );
    }
    if sid_length == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!(
            "failed to resolve sandbox account SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut sid = vec![0u8; sid_length as usize];
    let mut domain = vec![0u16; domain_length.max(1) as usize];
    if unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            account.as_ptr(),
            sid.as_mut_ptr() as _,
            &mut sid_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut sid_kind,
        )
    } == 0
    {
        return Err(format!(
            "failed to resolve sandbox account SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut string_sid = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_mut_ptr() as _, &mut string_sid) } == 0 {
        return Err(format!(
            "failed to format sandbox account SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut length = 0usize;
    unsafe {
        while *string_sid.add(length) != 0 {
            length += 1;
        }
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(string_sid, length) })
        .map_err(|error| error.to_string());
    unsafe {
        LocalFree(string_sid as _);
    }
    value
}

#[cfg(target_os = "windows")]
fn create_sandbox_named_pipe(name: &str) -> Result<isize, String> {
    use windows_sys::Win32::Foundation::{LocalFree, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    };

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    let sid = sandbox_account_sid_string()?;
    let descriptor_text = wide(&format!("D:P(A;;GA;;;{sid})"));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_text.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(format!(
            "failed to build sandbox pipe ACL: {}",
            std::io::Error::last_os_error()
        ));
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let pipe_name = wide(name);
    let handle = unsafe {
        CreateNamedPipeW(
            pipe_name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT,
            1,
            65_536,
            65_536,
            0,
            &security,
        )
    };
    unsafe {
        LocalFree(descriptor as _);
    }
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(format!(
            "failed to create sandbox named pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(handle as isize)
}

#[cfg(target_os = "windows")]
fn connect_sandbox_named_pipe(
    handle: isize,
    expected_process_id: u32,
    process_handle: isize,
) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, GetNamedPipeClientProcessId, SetNamedPipeHandleState, PIPE_READMODE_BYTE,
        PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    let started = Instant::now();
    loop {
        let connected = unsafe { ConnectNamedPipe(handle as _, std::ptr::null_mut()) };
        if connected != 0 {
            break;
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_PIPE_CONNECTED {
            break;
        }
        if error != ERROR_PIPE_LISTENING {
            return Err(format!(
                "sandbox named-pipe connection failed: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
        if unsafe { WaitForSingleObject(process_handle as _, 0) } == WAIT_OBJECT_0 {
            return Err("sandbox runner exited before connecting its named pipe".to_string());
        }
        if started.elapsed().as_secs() >= 15 {
            return Err(
                "sandbox runner did not connect its named pipe within 15 seconds".to_string(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mode = PIPE_READMODE_BYTE | PIPE_WAIT;
    if unsafe { SetNamedPipeHandleState(handle as _, &mode, std::ptr::null(), std::ptr::null()) }
        == 0
    {
        return Err(format!(
            "failed to switch sandbox named pipe to blocking mode: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut client_process_id = 0u32;
    if unsafe { GetNamedPipeClientProcessId(handle as _, &mut client_process_id) } == 0 {
        return Err(format!(
            "failed to verify sandbox named-pipe client: {}",
            std::io::Error::last_os_error()
        ));
    }
    if client_process_id != expected_process_id {
        return Err(format!(
            "sandbox named-pipe client PID mismatch ({client_process_id} != {expected_process_id})"
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_sandbox_named_pipe(name: &str) -> Result<std::fs::File, String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    };

    let name = wide(name);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(format!(
            "sandbox runner could not open its named pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle as _) })
}

#[cfg(target_os = "windows")]
pub fn execute<F>(app_data: &Path, request: &ExecRequest, emit: F) -> Result<ExecResponse, String>
where
    F: Fn(&str, u64, &[u8]) + Send + Sync + Clone + 'static,
{
    use std::os::windows::io::FromRawHandle;
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessWithLogonW, GetExitCodeProcess, ResumeThread, WaitForSingleObject,
        CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let status = setup_status(app_data);
    if !status.ready {
        return Err(format!(
            "native sandbox is not ready: {}",
            status.reason.unwrap_or_default()
        ));
    }
    if request.run_id.trim().is_empty() || request.stream_id.trim().is_empty() {
        return Err("sandbox execution requires runId and streamId".to_string());
    }
    if request.command.trim().is_empty() {
        return Err("sandbox command must not be empty".to_string());
    }
    let shell = request
        .shell
        .as_deref()
        .unwrap_or("powershell")
        .to_ascii_lowercase();
    if !matches!(shell.as_str(), "powershell" | "cmd") {
        return Err("sandbox shell must be 'powershell' or 'cmd'".to_string());
    }
    let cwd = PathBuf::from(&request.cwd)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let password = load_password(app_data)?;
    let capability_sid = capability_sid_for_run(app_data, &request.run_id)?;
    let python_root = {
        let candidate = setup_dir(app_data).join("runtime").join("python");
        candidate.join("python.exe").is_file().then_some(candidate)
    };
    let script_dir = cwd.join(".lacowork");
    fs::create_dir_all(&script_dir).map_err(|error| error.to_string())?;
    let script_path = script_dir.join(format!(
        "command-{}.{}",
        uuid::Uuid::new_v4(),
        if shell == "cmd" { "cmd" } else { "ps1" },
    ));
    let runner_request = RunnerRequest {
        command: request.command.clone(),
        shell,
        cwd: cwd.display().to_string(),
        python_root: python_root.map(|path| path.display().to_string()),
        capability_sid,
        script_path: script_path.display().to_string(),
    };
    let serialized_request = serde_json::to_vec(&runner_request).map_err(|e| e.to_string())?;
    let pipe_name = format!(
        r"\\.\pipe\lacowork-sandbox-{}",
        uuid::Uuid::new_v4().simple()
    );
    let pipe_handle = create_sandbox_named_pipe(&pipe_name)?;

    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command_line = wide(&format!(
        "\"{}\" --lacowork-native-sandbox-runner \"{}\"",
        executable.display(),
        pipe_name
    ));
    let username = wide(SANDBOX_ACCOUNT);
    let domain = wide(".");
    let password_wide = wide(&password);
    let executable_wide = wide(&executable.display().to_string());
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let runner_cwd = PathBuf::from(&system_root).join("System32");
    let runner_cwd_wide = wide(&runner_cwd.display().to_string());
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let start = Instant::now();
    let environment = minimal_environment_block(&cwd, runner_request.python_root.as_deref());
    let spawned = unsafe {
        CreateProcessWithLogonW(
            username.as_ptr(),
            domain.as_ptr(),
            password_wide.as_ptr(),
            0,
            executable_wide.as_ptr(),
            command_line.as_mut_ptr(),
            CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | windows_sys::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr() as *const core::ffi::c_void,
            runner_cwd_wide.as_ptr(),
            &startup,
            &mut process,
        )
    };
    if spawned == 0 {
        unsafe {
            CloseHandle(pipe_handle as _);
        }
        return Err(format!(
            "sandbox logon/spawn failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        unsafe {
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            CloseHandle(pipe_handle as _);
        }
        return Err(format!(
            "sandbox job creation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let job_ready = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0
            && AssignProcessToJobObject(job, process.hProcess) != 0
    };
    if !job_ready {
        unsafe {
            CloseHandle(job);
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            CloseHandle(pipe_handle as _);
        }
        return Err(format!(
            "sandbox job attachment failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    running_jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            request.stream_id.clone(),
            RunningJob {
                handle: job as isize,
                cancelled: cancelled.clone(),
            },
        );
    unsafe {
        ResumeThread(process.hThread);
        CloseHandle(process.hThread);
    }

    if let Err(error) =
        connect_sandbox_named_pipe(pipe_handle, process.dwProcessId, process.hProcess as isize)
    {
        running_jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request.stream_id);
        unsafe {
            TerminateJobObject(job, 125);
            CloseHandle(pipe_handle as _);
            CloseHandle(process.hProcess);
            CloseHandle(job);
        }
        let _ = fs::remove_file(&script_path);
        return Err(error);
    }
    let mut pipe =
        unsafe { std::fs::File::from_raw_handle(pipe_handle as std::os::windows::io::RawHandle) };
    if let Err(error) = write_ipc_frame(&mut pipe, IPC_REQUEST, &serialized_request) {
        unsafe {
            TerminateJobObject(job, 125);
        }
        running_jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request.stream_id);
        unsafe {
            CloseHandle(process.hProcess);
            CloseHandle(job);
        }
        let _ = fs::remove_file(&script_path);
        return Err(format!("failed to send sandbox spawn request: {error}"));
    }

    let stdout_capture = Arc::new(Mutex::new(BoundedCapture::default()));
    let stderr_capture = Arc::new(Mutex::new(BoundedCapture::default()));
    let framed_exit = Arc::new(Mutex::new(None));
    let ipc_thread = spawn_ipc_reader(
        pipe,
        emit,
        stdout_capture.clone(),
        stderr_capture.clone(),
        framed_exit.clone(),
    );

    let timeout_ms = request.timeout_ms.unwrap_or(30_000).clamp(1_000, 600_000);
    let mut timed_out = false;
    loop {
        let waited = unsafe { WaitForSingleObject(process.hProcess, 50) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        if waited != WAIT_TIMEOUT {
            unsafe {
                TerminateJobObject(job, 125);
            }
            break;
        }
        if start.elapsed().as_millis() as u64 >= timeout_ms {
            timed_out = true;
            unsafe {
                TerminateJobObject(job, 124);
            }
            break;
        }
    }
    unsafe {
        WaitForSingleObject(process.hProcess, 5_000);
    }
    let mut raw_exit_code = 125u32;
    running_jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&request.stream_id);
    unsafe {
        GetExitCodeProcess(process.hProcess, &mut raw_exit_code);
        CloseHandle(process.hProcess);
        CloseHandle(job);
    }
    let _ = ipc_thread.join();
    let _ = fs::remove_file(&script_path);

    let stdout = stdout_capture
        .lock()
        .map_err(|_| "stdout capture poisoned".to_string())?
        .clone();
    let stderr = stderr_capture
        .lock()
        .map_err(|_| "stderr capture poisoned".to_string())?
        .clone();
    let framed_exit_code = *framed_exit
        .lock()
        .map_err(|_| "sandbox IPC exit capture poisoned".to_string())?;
    let exit_code = if timed_out {
        124
    } else if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        130
    } else {
        framed_exit_code.unwrap_or(raw_exit_code as i32)
    };
    Ok(ExecResponse {
        stdout: decode_capture(&stdout),
        stderr: decode_capture(&stderr),
        exit_code: Some(exit_code),
        status: if timed_out {
            "timeout"
        } else if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            "cancelled"
        } else if exit_code == 0 {
            "completed"
        } else {
            "failed"
        }
        .to_string(),
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
        sandbox_id: format!("native:{}", request.run_id),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout_invalid_utf8: std::str::from_utf8(&stdout.bytes).is_err(),
        stderr_invalid_utf8: std::str::from_utf8(&stderr.bytes).is_err(),
    })
}

#[cfg(not(target_os = "windows"))]
pub fn execute<F>(
    _app_data: &Path,
    _request: &ExecRequest,
    _emit: F,
) -> Result<ExecResponse, String>
where
    F: Fn(&str, u64, &[u8]) + Send + Sync + Clone + 'static,
{
    Err("native Windows sandboxing is only available on Windows".to_string())
}

#[derive(Default, Clone)]
struct BoundedCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

#[cfg(target_os = "windows")]
fn spawn_ipc_reader<F>(
    mut pipe: std::fs::File,
    emit: F,
    stdout_capture: std::sync::Arc<std::sync::Mutex<BoundedCapture>>,
    stderr_capture: std::sync::Arc<std::sync::Mutex<BoundedCapture>>,
    exit_code: std::sync::Arc<std::sync::Mutex<Option<i32>>>,
) -> std::thread::JoinHandle<()>
where
    F: Fn(&str, u64, &[u8]) + Send + Sync + Clone + 'static,
{
    std::thread::spawn(move || {
        let mut sequence = 0u64;
        loop {
            match read_ipc_frame(&mut pipe) {
                Ok(Some((IPC_STDOUT, bytes))) => {
                    append_ipc_output("stdout", &bytes, &emit, &mut sequence, &stdout_capture);
                }
                Ok(Some((IPC_STDERR, bytes))) => {
                    append_ipc_output("stderr", &bytes, &emit, &mut sequence, &stderr_capture);
                }
                Ok(Some((IPC_ERROR, bytes))) => {
                    let mut message = b"[sandbox runner error] ".to_vec();
                    message.extend_from_slice(&bytes);
                    message.push(b'\n');
                    append_ipc_output("stderr", &message, &emit, &mut sequence, &stderr_capture);
                }
                Ok(Some((IPC_EXIT, bytes))) if bytes.len() == 4 => {
                    if let Ok(mut value) = exit_code.lock() {
                        *value = Some(i32::from_le_bytes(
                            bytes.try_into().expect("checked length"),
                        ));
                    }
                }
                Ok(Some((kind, _))) => {
                    let message = format!("[sandbox IPC error] unexpected frame type {kind}\n");
                    append_ipc_output(
                        "stderr",
                        message.as_bytes(),
                        &emit,
                        &mut sequence,
                        &stderr_capture,
                    );
                }
                Ok(None) => break,
                Err(error) => {
                    let message = format!("[sandbox IPC error] {error}\n");
                    append_ipc_output(
                        "stderr",
                        message.as_bytes(),
                        &emit,
                        &mut sequence,
                        &stderr_capture,
                    );
                    break;
                }
            }
        }
    })
}

#[cfg(target_os = "windows")]
fn append_ipc_output<F>(
    channel: &str,
    bytes: &[u8],
    emit: &F,
    sequence: &mut u64,
    capture: &std::sync::Arc<std::sync::Mutex<BoundedCapture>>,
) where
    F: Fn(&str, u64, &[u8]),
{
    let mut emitted = Vec::new();
    if let Ok(mut output) = capture.lock() {
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.bytes.len());
        let accepted = bytes.len().min(remaining);
        output.bytes.extend_from_slice(&bytes[..accepted]);
        emitted.extend_from_slice(&bytes[..accepted]);
        if accepted < bytes.len() {
            output.truncated = true;
        }
    }
    if !emitted.is_empty() {
        emit(channel, *sequence, &emitted);
        *sequence += 1;
    }
}

fn decode_capture(capture: &BoundedCapture) -> String {
    let mut value = String::from_utf8_lossy(&capture.bytes).into_owned();
    if capture.truncated {
        value.push_str("\n[output truncated at 4 MiB]\n");
    }
    value
}

#[cfg(target_os = "windows")]
fn minimal_environment_block(cwd: &Path, python_root: Option<&str>) -> Vec<u16> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let temp = cwd.join(".lacowork").join("temp");
    let _ = fs::create_dir_all(&temp);
    let mut path = format!(
        "{}\\System32;{}\\System32\\WindowsPowerShell\\v1.0",
        system_root, system_root
    );
    if let Some(python) = python_root {
        path = format!("{python};{path}");
    }
    let profile = format!(
        "{}\\Users\\{}",
        system_root.get(..2).unwrap_or("C:"),
        SANDBOX_ACCOUNT
    );
    let entries = [
        format!("COMSPEC={}\\System32\\cmd.exe", system_root),
        format!("PATH={path}"),
        "PATHEXT=.COM;.EXE;.BAT;.CMD".to_string(),
        format!("SYSTEMROOT={system_root}"),
        format!("TEMP={}", temp.display()),
        format!("TMP={}", temp.display()),
        format!("USERPROFILE={profile}"),
        format!("WINDIR={system_root}"),
    ];
    let mut block = Vec::new();
    for entry in entries {
        block.extend(wide(&entry));
    }
    block.push(0);
    block
}

pub fn dispatch_helper_from_args() -> Option<i32> {
    let args = std::env::args_os().collect::<Vec<_>>();
    let mode = args.get(1)?.to_string_lossy();
    if mode == "--lacowork-native-sandbox-setup" {
        let code = match (args.get(2), args.get(3)) {
            (Some(request), Some(result)) => {
                elevated_setup_helper(Path::new(request), Path::new(result))
            }
            _ => Err("setup helper arguments are missing".to_string()),
        };
        return Some(if code.is_ok() { 0 } else { 1 });
    }
    if mode == "--lacowork-native-sandbox-runner" {
        let code = match args.get(2) {
            Some(pipe_name) => command_runner(&pipe_name.to_string_lossy()),
            None => {
                eprintln!("sandbox runner request is missing");
                125
            }
        };
        return Some(code);
    }
    if mode == "--lacowork-native-sandbox-smoke" {
        let code = match (args.get(2), args.get(3)) {
            (Some(app_data), Some(result)) => {
                native_sandbox_smoke_helper(Path::new(app_data), Path::new(result))
            }
            _ => Err("sandbox smoke arguments are missing".to_string()),
        };
        return Some(if code.is_ok() { 0 } else { 1 });
    }
    None
}

fn native_sandbox_smoke_helper(app_data: &Path, result_path: &Path) -> Result<(), String> {
    let mut report = SandboxSmokeResult {
        ok: false,
        setup_ready: false,
        repeated_setup_ready: false,
        account: SANDBOX_ACCOUNT.to_string(),
        group: SANDBOX_GROUP.to_string(),
        execution_status: None,
        identity_verified: false,
        marker_written: false,
        error: None,
    };
    let workspace = std::env::temp_dir().join(format!(
        "lacowork-native-sandbox-smoke-{}",
        uuid::Uuid::new_v4()
    ));
    let outcome = (|| {
        let first = setup_start(app_data)?;
        report.setup_ready = first.ready;
        if !first.ready {
            return Err(format!(
                "initial sandbox setup is not ready: {}",
                first.reason.unwrap_or_default()
            ));
        }

        let repeated = setup_start(app_data)?;
        report.repeated_setup_ready = repeated.ready;
        if !repeated.ready {
            return Err(format!(
                "repeated sandbox setup is not ready: {}",
                repeated.reason.unwrap_or_default()
            ));
        }

        fs::create_dir_all(&workspace).map_err(|error| {
            format!("failed to create the sandbox smoke workspace: {error}")
        })?;
        let run_id = uuid::Uuid::new_v4().to_string();
        grant_workspace_access(app_data, &run_id, &workspace)?;
        let marker = workspace.join("sandbox-smoke.txt");
        let marker_literal = marker.display().to_string().replace('\'', "''");
        let response = execute(
            app_data,
            &ExecRequest {
                run_id,
                command: format!(
                    "$identity = whoami; $identity; Set-Content -LiteralPath '{marker_literal}' -Value 'sandbox-smoke-ok' -NoNewline"
                ),
                shell: Some("powershell".to_string()),
                cwd: workspace.display().to_string(),
                timeout_ms: Some(30_000),
                stream_id: uuid::Uuid::new_v4().to_string(),
            },
            |_, _, _| {},
        )?;
        report.execution_status = Some(response.status.clone());
        report.identity_verified = response
            .stdout
            .to_ascii_lowercase()
            .contains(&SANDBOX_ACCOUNT.to_ascii_lowercase());
        report.marker_written = fs::read_to_string(&marker)
            .map(|contents| contents == "sandbox-smoke-ok")
            .unwrap_or(false);
        if response.status != "completed" || response.exit_code != Some(0) {
            return Err(format!(
                "sandbox identity probe did not complete successfully (status {}, exit {:?}): {}",
                response.status, response.exit_code, response.stderr
            ));
        }
        if !report.identity_verified {
            return Err(format!(
                "sandbox identity probe did not run as {SANDBOX_ACCOUNT}: {}",
                response.stdout
            ));
        }
        if !report.marker_written {
            return Err("sandbox identity probe did not write its workspace marker".to_string());
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&workspace);
    report.ok = outcome.is_ok();
    report.error = outcome.as_ref().err().cloned();
    let serialized = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(parent) = result_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(result_path, serialized).map_err(|error| error.to_string())?;
    outcome
}

#[cfg(not(target_os = "windows"))]
fn elevated_setup_helper(_request: &Path, _result: &Path) -> Result<(), String> {
    Err("unsupported".to_string())
}
#[cfg(not(target_os = "windows"))]
fn command_runner(_pipe_name: &str) -> i32 {
    125
}

#[cfg(target_os = "windows")]
fn elevated_setup_helper(request_path: &Path, result_path: &Path) -> Result<(), String> {
    let outcome = (|| {
        let envelope: SetupEnvelope =
            serde_json::from_slice(&fs::read(request_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if envelope.version != SETUP_VERSION {
            return Err("unsupported setup request version".to_string());
        }
        let encrypted = BASE64
            .decode(envelope.encrypted_password)
            .map_err(|e| e.to_string())?;
        let password = String::from_utf8(unprotect_data(&encrypted)?).map_err(|e| e.to_string())?;
        create_or_update_local_principal(&password)
    })();
    let result = SetupResult {
        ok: outcome.is_ok(),
        version: SETUP_VERSION,
        error: outcome.as_ref().err().cloned(),
    };
    let serialized = serde_json::to_vec(&result).map_err(|e| e.to_string())?;
    fs::write(result_path, serialized).map_err(|e| e.to_string())?;
    outcome
}

#[cfg(target_os = "windows")]
fn create_or_update_local_principal(password: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{
        ERROR_ALIAS_EXISTS, ERROR_MEMBER_IN_ALIAS, ERROR_MEMBER_NOT_IN_ALIAS, ERROR_NO_SUCH_MEMBER,
    };
    use windows_sys::Win32::NetworkManagement::NetManagement::{
        NERR_GroupExists, NERR_Success, NERR_UserExists, NetLocalGroupAdd, NetLocalGroupAddMembers,
        NetLocalGroupDelMembers, NetUserAdd, NetUserSetInfo, LOCALGROUP_INFO_1,
        LOCALGROUP_MEMBERS_INFO_3, UF_DONT_EXPIRE_PASSWD, UF_SCRIPT, USER_INFO_1, USER_INFO_1003,
        USER_PRIV_USER,
    };
    let mut group = wide(SANDBOX_GROUP);
    let mut group_comment = wide("LocalAI Cowork native sandbox identities");
    let group_info = LOCALGROUP_INFO_1 {
        lgrpi1_name: group.as_mut_ptr(),
        lgrpi1_comment: group_comment.as_mut_ptr(),
    };
    let group_result = unsafe {
        NetLocalGroupAdd(
            std::ptr::null(),
            1,
            &group_info as *const _ as *const u8,
            std::ptr::null_mut(),
        )
    };
    if group_result != NERR_Success
        && group_result != NERR_GroupExists
        && group_result != ERROR_ALIAS_EXISTS
    {
        return Err(format!(
            "failed to create sandbox group (Windows error {group_result})"
        ));
    }

    let mut name = wide(SANDBOX_ACCOUNT);
    let mut pass = wide(password);
    let mut comment = wide("LocalAI Cowork low privilege sandbox account");
    let user = USER_INFO_1 {
        usri1_name: name.as_mut_ptr(),
        usri1_password: pass.as_mut_ptr(),
        usri1_password_age: 0,
        usri1_priv: USER_PRIV_USER,
        usri1_home_dir: std::ptr::null_mut(),
        usri1_comment: comment.as_mut_ptr(),
        usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD,
        usri1_script_path: std::ptr::null_mut(),
    };
    let user_result = unsafe {
        NetUserAdd(
            std::ptr::null(),
            1,
            &user as *const _ as *const u8,
            std::ptr::null_mut(),
        )
    };
    if user_result == NERR_UserExists {
        let password_info = USER_INFO_1003 {
            usri1003_password: pass.as_mut_ptr(),
        };
        let changed = unsafe {
            NetUserSetInfo(
                std::ptr::null(),
                name.as_ptr(),
                1003,
                &password_info as *const _ as *const u8,
                std::ptr::null_mut(),
            )
        };
        if changed != NERR_Success {
            return Err(format!(
                "failed to rotate sandbox password (Windows error {changed})"
            ));
        }
    } else if user_result != NERR_Success {
        return Err(format!(
            "failed to create sandbox user (Windows error {user_result})"
        ));
    }
    let member = LOCALGROUP_MEMBERS_INFO_3 {
        lgrmi3_domainandname: name.as_mut_ptr(),
    };
    let member_result = unsafe {
        NetLocalGroupAddMembers(
            std::ptr::null(),
            group.as_ptr(),
            3,
            &member as *const _ as *const u8,
            1,
        )
    };
    if member_result != NERR_Success && member_result != ERROR_MEMBER_IN_ALIAS {
        return Err(format!(
            "failed to add sandbox user to group (Windows error {member_result})"
        ));
    }
    let administrators = localized_administrators_group()?;
    let mut enforcement_errors = Vec::new();
    let administrators_result = unsafe {
        NetLocalGroupDelMembers(
            std::ptr::null(),
            administrators.as_ptr(),
            3,
            &member as *const _ as *const u8,
            1,
        )
    };
    if administrators_result != NERR_Success
        && administrators_result != ERROR_NO_SUCH_MEMBER
        && administrators_result != ERROR_MEMBER_NOT_IN_ALIAS
    {
        enforcement_errors.push(format!(
            "failed to enforce standard-user membership (Windows error {administrators_result})"
        ));
    }
    let sandbox_group_member = LOCALGROUP_MEMBERS_INFO_3 {
        lgrmi3_domainandname: group.as_mut_ptr(),
    };
    let group_administrators_result = unsafe {
        NetLocalGroupDelMembers(
            std::ptr::null(),
            administrators.as_ptr(),
            3,
            &sandbox_group_member as *const _ as *const u8,
            1,
        )
    };
    if group_administrators_result != NERR_Success
        && group_administrators_result != ERROR_NO_SUCH_MEMBER
        && group_administrators_result != ERROR_MEMBER_NOT_IN_ALIAS
    {
        enforcement_errors.push(format!(
            "failed to enforce non-admin sandbox group membership (Windows error {group_administrators_result})"
        ));
    }
    let membership = sandbox_account_membership_ready();
    pass.fill(0);
    finalize_membership_enforcement(enforcement_errors, membership)
}

fn finalize_membership_enforcement(
    enforcement_errors: Vec<String>,
    membership: Result<(), String>,
) -> Result<(), String> {
    match membership {
        Ok(()) => Ok(()),
        Err(reason) if enforcement_errors.is_empty() => Err(reason),
        Err(reason) => Err(format!(
            "{}; final membership verification failed: {reason}",
            enforcement_errors.join("; ")
        )),
    }
}

#[cfg(target_os = "windows")]
fn localized_administrators_group() -> Result<Vec<u16>, String> {
    use windows_sys::Win32::Security::{
        CreateWellKnownSid, LookupAccountSidW, WinBuiltinAdministratorsSid, SECURITY_MAX_SID_SIZE,
        SID_NAME_USE,
    };
    let mut sid = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut sid_size = sid.len() as u32;
    if unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            std::ptr::null_mut(),
            sid.as_mut_ptr() as _,
            &mut sid_size,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut name = vec![0u16; 256];
    let mut name_size = name.len() as u32;
    let mut domain = vec![0u16; 256];
    let mut domain_size = domain.len() as u32;
    let mut sid_use: SID_NAME_USE = 0;
    if unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid.as_mut_ptr() as _,
            name.as_mut_ptr(),
            &mut name_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut sid_use,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    name.truncate(name_size as usize);
    name.push(0);
    Ok(name)
}

#[cfg(target_os = "windows")]
fn command_runner(pipe_name: &str) -> i32 {
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    };
    use windows_sys::Win32::Security::{SECURITY_ATTRIBUTES, TOKEN_ALL_ACCESS};
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::StationsAndDesktops::{
        CloseDesktop, CreateDesktopW, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW, DESKTOP_ENUMERATE,
        DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
        WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };

    let mut pipe = match open_sandbox_named_pipe(pipe_name) {
        Ok(pipe) => pipe,
        Err(_) => return 125,
    };
    let request = match read_ipc_frame(&mut pipe) {
        Ok(Some((IPC_REQUEST, payload))) => serde_json::from_slice::<RunnerRequest>(&payload)
            .map_err(|error| format!("invalid sandbox spawn request: {error}")),
        Ok(Some((kind, _))) => Err(format!("expected sandbox spawn request, got frame {kind}")),
        Ok(None) => Err("sandbox host closed the IPC pipe before the spawn request".to_string()),
        Err(error) => Err(format!("failed to read sandbox spawn request: {error}")),
    };
    let pipe = Arc::new(Mutex::new(pipe));
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            if let Ok(mut writer) = pipe.lock() {
                let _ = write_ipc_frame(&mut *writer, IPC_ERROR, error.as_bytes());
            }
            return 125;
        }
    };

    let result = (|| -> Result<i32, String> {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let script_dir = PathBuf::from(&request.cwd).join(".lacowork");
        let script_path = PathBuf::from(&request.script_path);
        if script_path.parent() != Some(script_dir.as_path())
            || script_path.file_name().is_none()
            || !matches!(
                script_path.extension().and_then(|value| value.to_str()),
                Some("cmd" | "ps1")
            )
        {
            return Err(
                "sandbox command script path escaped its private run directory".to_string(),
            );
        }
        fs::create_dir_all(&script_dir).map_err(|error| error.to_string())?;
        let script_bytes = if request.shell == "cmd" {
            format!("@chcp 65001>nul\r\n{}\r\n", request.command).into_bytes()
        } else {
            let wrapped = format!(
                "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)\r\n[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)\r\n$OutputEncoding = [Console]::OutputEncoding\r\n& {{\r\n{}\r\n}}\r\n$__lacowork_ok = $?\r\n$__lacowork_exit = $LASTEXITCODE\r\nif (-not $__lacowork_ok) {{ exit 1 }}\r\nif ($null -ne $__lacowork_exit) {{ exit $__lacowork_exit }}\r\nexit 0\r\n",
                request.command,
            );
            let mut bytes = vec![0xef, 0xbb, 0xbf];
            bytes.extend_from_slice(wrapped.as_bytes());
            bytes
        };
        fs::write(&script_path, script_bytes).map_err(|error| error.to_string())?;
        let (application, line) = if request.shell == "cmd" {
            let app = PathBuf::from(&system_root).join("System32").join("cmd.exe");
            (
                app.clone(),
                format!(
                    "\"{}\" /D /S /C \"\"{}\"\"",
                    app.display(),
                    script_path.display()
                ),
            )
        } else {
            let app = PathBuf::from(&system_root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            (
                app.clone(),
                format!(
                    "\"{}\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
                    app.display(),
                    script_path.display()
                ),
            )
        };
        if !application.is_file() {
            return Err(format!(
                "required sandbox shell is missing: {}",
                application.display()
            ));
        }

        let desktop_name = format!("LACowork-{}", uuid::Uuid::new_v4());
        let desktop_wide = wide(&desktop_name);
        let desktop = unsafe {
            CreateDesktopW(
                desktop_wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                DESKTOP_CREATEMENU
                    | DESKTOP_CREATEWINDOW
                    | DESKTOP_ENUMERATE
                    | DESKTOP_READOBJECTS
                    | DESKTOP_WRITEOBJECTS,
                std::ptr::null(),
            )
        };
        if desktop.is_null() {
            return Err(format!(
                "private desktop creation failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token) } == 0 {
            unsafe {
                CloseDesktop(desktop);
            }
            return Err(format!(
                "sandbox token open failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let restricted = match create_capability_restricted_token(token, &request.capability_sid) {
            Ok(token) => token,
            Err(error) => {
                unsafe {
                    CloseHandle(token);
                    CloseDesktop(desktop);
                }
                return Err(error);
            }
        };

        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            bInheritHandle: 1,
            ..Default::default()
        };
        let (mut stdout_read, mut stdout_write): (HANDLE, HANDLE) =
            (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut stderr_read, mut stderr_write): (HANDLE, HANDLE) =
            (std::ptr::null_mut(), std::ptr::null_mut());
        let pipes_ready = unsafe {
            CreatePipe(&mut stdout_read, &mut stdout_write, &security, 0) != 0
                && CreatePipe(&mut stderr_read, &mut stderr_write, &security, 0) != 0
        };
        if !pipes_ready {
            unsafe {
                if !stdout_read.is_null() {
                    CloseHandle(stdout_read);
                }
                if !stdout_write.is_null() {
                    CloseHandle(stdout_write);
                }
                if !stderr_read.is_null() {
                    CloseHandle(stderr_read);
                }
                if !stderr_write.is_null() {
                    CloseHandle(stderr_write);
                }
                CloseHandle(restricted);
                CloseHandle(token);
                CloseDesktop(desktop);
            }
            return Err(format!(
                "sandbox output-pipe creation failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe {
            SetHandleInformation(stdout_read, HANDLE_FLAG_INHERIT, 0);
            SetHandleInformation(stderr_read, HANDLE_FLAG_INHERIT, 0);
        }
        let mut startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut startup_desktop = wide(&format!("Winsta0\\{desktop_name}"));
        startup.lpDesktop = startup_desktop.as_mut_ptr();
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = std::ptr::null_mut();
        startup.hStdOutput = stdout_write;
        startup.hStdError = stderr_write;
        let mut process = PROCESS_INFORMATION::default();
        let application_wide = wide(&application.display().to_string());
        let cwd_wide = wide(&request.cwd);
        let mut line_wide = wide(&line);
        let created = unsafe {
            CreateProcessAsUserW(
                restricted,
                application_wide.as_ptr(),
                line_wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_NO_WINDOW,
                std::ptr::null(),
                cwd_wide.as_ptr(),
                &startup,
                &mut process,
            )
        };
        unsafe {
            CloseHandle(stdout_write);
            CloseHandle(stderr_write);
        }
        if created == 0 {
            let error = unsafe { GetLastError() };
            unsafe {
                CloseHandle(stdout_read);
                CloseHandle(stderr_read);
                CloseHandle(restricted);
                CloseHandle(token);
                CloseDesktop(desktop);
            }
            return Err(format!(
                "restricted shell spawn failed (Windows error {error})"
            ));
        }
        let stdout_forwarder =
            spawn_runner_output_forwarder(stdout_read as isize, IPC_STDOUT, pipe.clone());
        let stderr_forwarder =
            spawn_runner_output_forwarder(stderr_read as isize, IPC_STDERR, pipe.clone());
        unsafe {
            CloseHandle(process.hThread);
            WaitForSingleObject(process.hProcess, u32::MAX);
        }
        let mut exit = 125u32;
        unsafe {
            GetExitCodeProcess(process.hProcess, &mut exit);
            CloseHandle(process.hProcess);
            CloseHandle(restricted);
            CloseHandle(token);
            CloseDesktop(desktop);
        }
        let _ = stdout_forwarder.join();
        let _ = stderr_forwarder.join();
        let _ = fs::remove_file(script_path);
        Ok(exit as i32)
    })();
    match result {
        Ok(code) => {
            if let Ok(mut writer) = pipe.lock() {
                let _ = write_ipc_frame(&mut *writer, IPC_EXIT, &code.to_le_bytes());
            }
            code
        }
        Err(error) => {
            if let Ok(mut writer) = pipe.lock() {
                let _ = write_ipc_frame(&mut *writer, IPC_ERROR, error.as_bytes());
                let _ = write_ipc_frame(&mut *writer, IPC_EXIT, &125i32.to_le_bytes());
            }
            125
        }
    }
}

#[cfg(target_os = "windows")]
fn spawn_runner_output_forwarder(
    handle: isize,
    kind: u8,
    pipe: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
) -> std::thread::JoinHandle<()> {
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;

    std::thread::spawn(move || {
        let mut source =
            unsafe { std::fs::File::from_raw_handle(handle as std::os::windows::io::RawHandle) };
        let mut buffer = [0u8; 32 * 1024];
        while let Ok(read) = source.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let Ok(mut writer) = pipe.lock() else {
                break;
            };
            if write_ipc_frame(&mut *writer, kind, &buffer[..read]).is_err() {
                break;
            }
        }
    })
}

#[cfg(target_os = "windows")]
fn create_capability_restricted_token(
    base_token: windows_sys::Win32::Foundation::HANDLE,
    capability_sid: &str,
) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, CreateWellKnownSid, GetTokenInformation, TokenUser, WinWorldSid,
        DISABLE_MAX_PRIVILEGE, SID_AND_ATTRIBUTES, TOKEN_USER,
    };

    const LUA_TOKEN: u32 = 0x04;
    const WRITE_RESTRICTED: u32 = 0x08;
    let capability = wide(capability_sid);
    let mut capability_ptr = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(capability.as_ptr(), &mut capability_ptr) } == 0 {
        return Err(format!(
            "invalid sandbox capability SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut world_size = 0u32;
    unsafe {
        CreateWellKnownSid(
            WinWorldSid,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut world_size,
        );
    }
    if world_size == 0 {
        unsafe {
            LocalFree(capability_ptr as _);
        }
        return Err("failed to size the Windows Everyone SID".to_string());
    }
    let mut world = vec![0u8; world_size as usize];
    if unsafe {
        CreateWellKnownSid(
            WinWorldSid,
            std::ptr::null_mut(),
            world.as_mut_ptr() as _,
            &mut world_size,
        )
    } == 0
    {
        unsafe {
            LocalFree(capability_ptr as _);
        }
        return Err(format!(
            "failed to create the Windows Everyone SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut token_user_size = 0u32;
    unsafe {
        GetTokenInformation(
            base_token,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut token_user_size,
        );
    }
    if token_user_size < std::mem::size_of::<TOKEN_USER>() as u32 {
        unsafe {
            LocalFree(capability_ptr as _);
        }
        return Err("failed to size the sandbox token user SID".to_string());
    }
    let mut token_user = vec![0u8; token_user_size as usize];
    if unsafe {
        GetTokenInformation(
            base_token,
            TokenUser,
            token_user.as_mut_ptr() as _,
            token_user_size,
            &mut token_user_size,
        )
    } == 0
    {
        unsafe {
            LocalFree(capability_ptr as _);
        }
        return Err(format!(
            "failed to resolve the sandbox token user SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let token_user_sid = unsafe { (*(token_user.as_ptr() as *const TOKEN_USER)).User.Sid };
    let mut restrictions = [
        SID_AND_ATTRIBUTES {
            Sid: capability_ptr,
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: token_user_sid,
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: world.as_mut_ptr() as _,
            Attributes: 0,
        },
    ];
    let mut restricted: HANDLE = std::ptr::null_mut();
    let created = unsafe {
        CreateRestrictedToken(
            base_token,
            DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            restrictions.len() as u32,
            restrictions.as_mut_ptr(),
            &mut restricted,
        )
    };
    unsafe {
        LocalFree(capability_ptr as _);
    }
    if created == 0 {
        return Err(format!(
            "capability-restricted token creation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(restricted)
}

#[cfg(target_os = "windows")]
fn generate_password() -> Result<String, String> {
    let mut bytes = [0u8; 28];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789-_!@";
    let mut password = String::from("Aa1!");
    password.extend(
        bytes
            .iter()
            .map(|value| ALPHABET[*value as usize % ALPHABET.len()] as char),
    );
    Ok(password)
}

#[cfg(target_os = "windows")]
fn protect_data(input: &[u8], machine_scope: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let flags = CRYPTPROTECT_UI_FORBIDDEN
        | if machine_scope {
            CRYPTPROTECT_LOCAL_MACHINE
        } else {
            0
        };
    if unsafe {
        CryptProtectData(
            &source,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
            &mut output,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn unprotect_data(input: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    if unsafe {
        CryptUnprotectData(
            &source,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn load_password(app_data: &Path) -> Result<String, String> {
    let encrypted = fs::read(credential_path(app_data)).map_err(|e| e.to_string())?;
    String::from_utf8(unprotect_data(&encrypted)?).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn load_capability_seed(app_data: &Path) -> Result<Vec<u8>, String> {
    let encrypted = fs::read(capability_seed_path(app_data)).map_err(|e| e.to_string())?;
    unprotect_data(&encrypted)
}

#[cfg(not(target_os = "windows"))]
fn load_capability_seed(_app_data: &Path) -> Result<Vec<u8>, String> {
    Err("native Windows sandboxing is only available on Windows".to_string())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "private file has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let temporary_wide = wide(&temporary.display().to_string());
        let path_wide = wide(&path.display().to_string());
        if unsafe {
            MoveFileExW(
                temporary_wide.as_ptr(),
                path_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_membership_enforcement, read_ipc_frame, write_ipc_frame, IPC_STDERR, IPC_STDOUT,
    };

    #[test]
    fn final_safe_membership_accepts_noncanonical_cleanup_results() {
        assert!(finalize_membership_enforcement(
            vec!["unexpected Windows cleanup result".to_string()],
            Ok(()),
        )
        .is_ok());
    }

    #[test]
    fn unsafe_final_membership_preserves_cleanup_diagnostics() {
        let error = finalize_membership_enforcement(
            vec!["cleanup failed".to_string()],
            Err("sandbox account is unexpectedly a local administrator".to_string()),
        )
        .expect_err("unsafe membership must fail");
        assert!(error.contains("cleanup failed"));
        assert!(error.contains("unexpectedly a local administrator"));
    }

    #[test]
    fn binary_ipc_frames_round_trip_without_text_markers() {
        let mut bytes = Vec::new();
        write_ipc_frame(&mut bytes, IPC_STDOUT, b"hello").expect("stdout frame");
        write_ipc_frame(&mut bytes, IPC_STDERR, &[0xff, 0x00, 0x80]).expect("stderr frame");

        let mut reader = bytes.as_slice();
        assert_eq!(
            read_ipc_frame(&mut reader).expect("read stdout"),
            Some((IPC_STDOUT, b"hello".to_vec()))
        );
        assert_eq!(
            read_ipc_frame(&mut reader).expect("read stderr"),
            Some((IPC_STDERR, vec![0xff, 0x00, 0x80]))
        );
        assert_eq!(read_ipc_frame(&mut reader).expect("read eof"), None);
    }

    #[test]
    fn ipc_rejects_unbounded_frames() {
        let mut bytes = Vec::new();
        let error = write_ipc_frame(
            &mut bytes,
            IPC_STDOUT,
            &vec![0u8; super::IPC_MAX_FRAME_BYTES],
        )
        .expect_err("oversized frame must fail");
        assert!(error.contains("too large"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires completed UAC sandbox setup; set LACOWORK_TEST_APP_DATA"]
    fn native_windows_acceptance_identity_cmd_unicode_and_separate_streams() {
        use super::{execute, grant_workspace_access, setup_status, ExecRequest, SANDBOX_ACCOUNT};

        let app_data = std::env::var_os("LACOWORK_TEST_APP_DATA")
            .map(std::path::PathBuf::from)
            .expect("LACOWORK_TEST_APP_DATA must point at the configured app-data directory");
        assert!(setup_status(&app_data).ready, "native setup must be ready");
        let workspace = std::env::temp_dir().join(format!(
            "lacowork-native-acceptance-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let run_id = uuid::Uuid::new_v4().to_string();
        grant_workspace_access(&app_data, &run_id, &workspace).unwrap();
        let result = execute(
            &app_data,
            &ExecRequest {
                run_id: run_id.clone(),
                command:
                    "whoami; [Console]::Out.Write('Hello 日本語'); [Console]::Error.Write('Error Ω')"
                        .to_string(),
                shell: Some("powershell".to_string()),
                cwd: workspace.display().to_string(),
                timeout_ms: Some(20_000),
                stream_id: uuid::Uuid::new_v4().to_string(),
            },
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(result.status, "completed");
        assert!(result
            .stdout
            .to_ascii_lowercase()
            .contains(&SANDBOX_ACCOUNT.to_ascii_lowercase()));
        assert!(result.stdout.contains("Hello 日本語"));
        assert!(result.stderr.contains("Error Ω"));

        let cmd = execute(
            &app_data,
            &ExecRequest {
                run_id,
                command: "echo Hallo".to_string(),
                shell: Some("cmd".to_string()),
                cwd: workspace.display().to_string(),
                timeout_ms: Some(20_000),
                stream_id: uuid::Uuid::new_v4().to_string(),
            },
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(cmd.status, "completed");
        assert!(cmd.stdout.contains("Hallo"));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires completed UAC sandbox setup; set LACOWORK_TEST_APP_DATA"]
    fn native_windows_timeout_kills_the_complete_job_tree() {
        use super::{execute, grant_workspace_access, setup_status, ExecRequest};

        let app_data = std::env::var_os("LACOWORK_TEST_APP_DATA")
            .map(std::path::PathBuf::from)
            .expect("LACOWORK_TEST_APP_DATA must point at the configured app-data directory");
        assert!(setup_status(&app_data).ready, "native setup must be ready");
        let workspace =
            std::env::temp_dir().join(format!("lacowork-native-timeout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let run_id = uuid::Uuid::new_v4().to_string();
        grant_workspace_access(&app_data, &run_id, &workspace).unwrap();
        let marker = workspace.join("escaped-child.txt");
        let marker_literal = marker.display().to_string().replace('\'', "''");
        let command = format!(
            "$p=Start-Process powershell.exe -PassThru -ArgumentList '-NoProfile','-Command',\"Start-Sleep -Seconds 3; Set-Content -LiteralPath '{marker_literal}' -Value escaped\"; Wait-Process -Id $p.Id"
        );
        let result = execute(
            &app_data,
            &ExecRequest {
                run_id,
                command,
                shell: Some("powershell".to_string()),
                cwd: workspace.display().to_string(),
                timeout_ms: Some(1_000),
                stream_id: uuid::Uuid::new_v4().to_string(),
            },
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(result.status, "timeout");
        std::thread::sleep(std::time::Duration::from_secs(4));
        assert!(!marker.exists(), "child process escaped the sandbox job");
        let _ = std::fs::remove_dir_all(workspace);
    }
}
