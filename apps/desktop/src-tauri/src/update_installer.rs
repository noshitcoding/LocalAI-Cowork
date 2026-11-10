//! Detached Windows installer observer over the exact signed, staged NSIS bytes.
//! No installer replay on restart, PID adoption, timeout-as-exit or target guessing.
use super::{daemon_process::DaemonProcess, update_journal as journal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::*,
    System::{Com::*, Threading::*},
    UI::Shell::*,
};

const ARG: &str = "--cowork-update-installer";
const CONTRACT: &str = "nsis.currentUser.originalProcess.v1";
const MAX_BINARY: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    format: u32,
    attempt: String,
    attempt_sha256: String,
    payload_sha256: String,
    observer_sha256: String,
    target_version: String,
    install_directory: PathBuf,
    application_name: String,
    installer_contract: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format: u32,
    folder: PathBuf,
    attempt: String,
    attempt_sha256: String,
    plan_sha256: String,
    application_process: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outcome {
    format: u32,
    attempt: String,
    attempt_sha256: String,
    plan_sha256: String,
    payload_sha256: String,
    state: String,
    exit_code: Option<u32>,
    launch_error: Option<u32>,
}
impl Outcome {
    pub(crate) fn successful(&self) -> bool {
        self.state == "original_process_exited"
            && self.exit_code == Some(0)
            && self.launch_error.is_none()
    }
}
fn path(folder: &Path, id: &str, suffix: &str) -> PathBuf {
    folder.join(format!("{id}.{suffix}"))
}
fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>, String> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err("invalid_installer_path".into());
    }
    value.push(0);
    Ok(value)
}
fn parameters(directory: &Path) -> Result<Vec<u16>, String> {
    let value = directory.to_str().ok_or("invalid_installer_path")?;
    let value = value.strip_prefix(r"\\?\").unwrap_or(value);
    if !directory.is_absolute()
        || value.starts_with(r"\\")
        || value.len() > 240
        || value.as_bytes().get(1) != Some(&b':')
        || value.contains(['"', '\r', '\n', '\0'])
    {
        return Err("invalid_installer_path".into());
    }
    wide(std::ffi::OsStr::new(&format!("/P /UPDATE /D={value}")))
}
/// Retain every directory identity and disallow namespace deletion/rename.
fn directories(root: &Path) -> Result<Vec<File>, String> {
    if !root.is_absolute() {
        return Err("invalid_installer_path".into());
    }
    let text = root.as_os_str().to_string_lossy();
    let ordinary = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if ordinary.starts_with(r"\\") || ordinary.as_bytes().get(1) != Some(&b':') {
        return Err("local_installer_directory_required".into());
    }
    let mut pins = Vec::new();
    for directory in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory)
            .map_err(|_| "installer_directory_unavailable")?;
        let metadata = file
            .metadata()
            .map_err(|_| "installer_directory_unavailable")?;
        use std::os::windows::fs::MetadataExt;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("ordinary_installer_directory_required".into());
        }
        pins.push(file);
    }
    Ok(pins)
}
fn checked_file(file: &Path) -> Result<File, String> {
    let value = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(file)
        .map_err(|_| "installer_file_unavailable")?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(value.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || info.nNumberOfLinks != 1
        || value
            .metadata()
            .map_err(|_| "installer_file_unavailable")?
            .len()
            > MAX_BINARY
    {
        return Err("ordinary_installer_file_required".into());
    }
    Ok(value)
}
fn file_hash(file: &mut File) -> Result<String, String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let size = file
            .read(&mut buffer)
            .map_err(|_| "installer_file_read_failed")?;
        if size == 0 {
            break;
        }
        total = total
            .checked_add(size as u64)
            .ok_or("installer_file_too_large")?;
        if total > MAX_BINARY {
            return Err("installer_file_too_large".into());
        }
        digest.update(&buffer[..size]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn version_value(file: &Path, key: &str) -> Result<String, String> {
    let file = wide(file.as_os_str())?;
    let size = unsafe { GetFileVersionInfoSizeW(file.as_ptr(), std::ptr::null_mut()) };
    if size == 0 || size > 1024 * 1024 {
        return Err("installer_contract_missing".into());
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(file.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return Err("installer_contract_missing".into());
    }
    let translation = wide(std::ffi::OsStr::new(r"\VarFileInfo\Translation"))?;
    let mut pointer = std::ptr::null_mut();
    let mut length = 0;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            translation.as_ptr(),
            &mut pointer,
            &mut length,
        )
    } == 0
        || pointer.is_null()
        || length == 0
        || length > 64
        || length % 4 != 0
    {
        return Err("installer_contract_missing".into());
    }
    // Version APIs return pointers into this bounded buffer. Check before reading.
    let begin = data.as_ptr() as usize;
    let end = begin + data.len();
    let start = pointer as usize;
    if start < begin || start.checked_add(length as usize).is_none_or(|v| v > end) {
        return Err("installer_contract_invalid".into());
    }
    let translations =
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length as usize) }.to_vec();
    for pair in translations.chunks_exact(4) {
        let language = u16::from_le_bytes([pair[0], pair[1]]);
        let codepage = u16::from_le_bytes([pair[2], pair[3]]);
        let query = wide(std::ffi::OsStr::new(&format!(
            r"\StringFileInfo\{language:04x}{codepage:04x}\{key}"
        )))?;
        pointer = std::ptr::null_mut();
        length = 0;
        if unsafe {
            VerQueryValueW(
                data.as_ptr().cast(),
                query.as_ptr(),
                &mut pointer,
                &mut length,
            )
        } == 0
        {
            continue;
        }
        let start = pointer as usize;
        if pointer.is_null()
            || !start.is_multiple_of(2)
            || length == 0
            || length > 1024
            || start < begin
            || start
                .checked_add(length as usize * 2)
                .is_none_or(|v| v > end)
        {
            return Err("installer_contract_invalid".into());
        }
        let value = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), length as usize) };
        if value.last() != Some(&0) || value[..value.len() - 1].contains(&0) {
            return Err("installer_contract_invalid".into());
        }
        return String::from_utf16(&value[..value.len() - 1])
            .map_err(|_| "installer_contract_invalid".into());
    }
    Err("installer_contract_missing".into())
}
fn verify_payload(file: &Path, hash: &str, version: &str) -> Result<File, String> {
    let mut pin = checked_file(file)?;
    if file_hash(&mut pin)? != hash
        || version_value(file, "CoworkInstallerContract")? != CONTRACT
        || version_value(file, "ProductVersion")? != version
    {
        return Err("signed_installer_contract_does_not_match".into());
    }
    Ok(pin)
}
fn load_plan(folder: &Path, id: &str, digest: &str) -> Result<(Plan, String), String> {
    let (attempt, original) = journal::load(folder, id)?;
    if original != digest {
        return Err("update_receipt_binding_changed".into());
    }
    let bytes = journal::read(&path(folder, id, "installer-plan.json"))?;
    let plan: Plan = serde_json::from_slice(&bytes).map_err(|_| "installer_plan_invalid")?;
    if plan.format != 1
        || plan.attempt != id
        || plan.attempt_sha256 != digest
        || plan.payload_sha256 != attempt.payload_sha256
        || plan.target_version != attempt.to
        || !journal::valid_sha(&plan.observer_sha256)
        || plan.installer_contract != CONTRACT
        || !plan.install_directory.is_absolute()
        || plan.application_name.is_empty()
        || plan.application_name.len() > 240
        || plan.application_name.contains(['/', '\\', ':', '\0'])
        || !plan.application_name.to_ascii_lowercase().ends_with(".exe")
    {
        return Err("installer_plan_invalid".into());
    }
    Ok((plan, journal::hash(&bytes)))
}
pub(crate) fn outcome(folder: &Path, id: &str, digest: &str) -> Result<Option<Outcome>, String> {
    let result = path(folder, id, "installer-result.json");
    match std::fs::symlink_metadata(&result) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("installer_result_read_failed".into()),
        Ok(_) => {}
    }
    let (plan, plan_digest) = load_plan(folder, id, digest)?;
    let value: Outcome =
        serde_json::from_slice(&journal::read(&result)?).map_err(|_| "installer_result_invalid")?;
    if value.format != 1
        || value.attempt != id
        || value.attempt_sha256 != digest
        || value.plan_sha256 != plan_digest
        || value.payload_sha256 != plan.payload_sha256
        || !matches!(
            (&*value.state, value.exit_code, value.launch_error),
            ("original_process_exited", Some(_), None)
                | ("launch_rejected", None, Some(1..))
                | ("original_process_exited", Some(_), Some(1..))
        )
        || !journal::has_phase(folder, id, digest, "application-exited")?
        || !journal::has_phase(folder, id, digest, "installer-starting")?
    {
        return Err("installer_result_invalid".into());
    }
    Ok(Some(value))
}
/// Staging and contract verification happen before the daemon receives shutdown.
pub(crate) fn prepare(folder: &Path, id: &str, digest: &str, bytes: &[u8]) -> Result<(), String> {
    let (attempt, original) = journal::load(folder, id)?;
    if original != digest || journal::hash(bytes) != attempt.payload_sha256 {
        return Err("update_receipt_binding_changed".into());
    }
    let application = std::env::current_exe().map_err(|_| "application_path_missing")?;
    let install_directory = application
        .parent()
        .ok_or("application_path_missing")?
        .canonicalize()
        .map_err(|_| "application_path_missing")?;
    let _directories = directories(&install_directory)?;
    parameters(&install_directory)?;
    let _staging = directories(folder)?;
    let installer = path(folder, id, "installer.exe");
    journal::publish(&installer, bytes, MAX_BINARY)?;
    let _payload = verify_payload(&installer, &attempt.payload_sha256, &attempt.to)?;
    let mut source = checked_file(&application)?;
    let mut helper = Vec::new();
    Read::by_ref(&mut source)
        .take(MAX_BINARY + 1)
        .read_to_end(&mut helper)
        .map_err(|_| "installer_observer_copy_failed")?;
    let observer_sha256 = journal::publish(
        &path(folder, id, "installer-observer.exe"),
        &helper,
        MAX_BINARY,
    )?;
    let plan = Plan {
        format: 1,
        attempt: id.into(),
        attempt_sha256: digest.into(),
        payload_sha256: attempt.payload_sha256,
        observer_sha256,
        target_version: attempt.to,
        install_directory,
        application_name: application
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("application_path_missing")?
            .into(),
        installer_contract: CONTRACT.into(),
    };
    journal::record(&path(folder, id, "installer-plan.json"), &plan)?;
    Ok(())
}
pub(crate) fn spawn(folder: &Path, id: &str, digest: &str) -> Result<Child, String> {
    use std::os::windows::process::CommandExt;
    let (_, plan_digest) = load_plan(folder, id, digest)?;
    let process = DaemonProcess::current_application()?;
    let mut child = Command::new(path(folder, id, "installer-observer.exe"))
        .arg(ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "installer_observer_start_failed")?;
    let handoff = (|| -> Result<(), String> {
        let request = Request {
            format: 1,
            folder: folder.into(),
            attempt: id.into(),
            attempt_sha256: digest.into(),
            plan_sha256: plan_digest,
            application_process: process.duplicate_into(&child)?,
        };
        let bytes =
            serde_json::to_vec(&request).map_err(|_| "installer_observer_handoff_failed")?;
        child
            .stdin
            .take()
            .ok_or("installer_observer_handoff_failed")?
            .write_all(&bytes)
            .map_err(|_| "installer_observer_handoff_failed".into())
    })();
    if let Err(e) = handoff {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    Ok(child)
}
pub(crate) async fn ready(
    child: &mut Child,
    folder: &Path,
    id: &str,
    digest: &str,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if journal::has_phase(folder, id, digest, "installer-observer-ready")? {
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|_| "installer_observer_observation_failed")?
            .is_some()
        {
            return Err("installer_observer_start_failed".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("installer_observer_ready_unconfirmed".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}
struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn observe(process: &ProcessHandle) -> Result<u32, String> {
    loop {
        match unsafe { WaitForSingleObject(process.0, 1000) } {
            WAIT_OBJECT_0 => break,
            WAIT_TIMEOUT => continue,
            _ => return Err("installer_process_observation_failed".into()),
        }
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err("installer_exit_code_unconfirmed".into());
    }
    Ok(code)
}
fn image_matches(process: &ProcessHandle, expected: &Path) -> Result<(), String> {
    let mut buffer = vec![0u16; 32768];
    let mut count = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut count) } == 0 {
        return Err("installer_process_image_unconfirmed".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let actual = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count as usize]));
    let actual = actual
        .canonicalize()
        .map_err(|_| "installer_process_image_unconfirmed")?;
    let expected = expected
        .canonicalize()
        .map_err(|_| "installer_process_image_unconfirmed")?;
    if !actual
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
    {
        return Err("installer_process_image_changed".into());
    }
    Ok(())
}
fn run() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(65537)
        .read_to_end(&mut input)
        .map_err(|_| "installer_request_invalid")?;
    if input.len() > 65536 {
        return Err("installer_request_invalid".into());
    }
    let request: Request =
        serde_json::from_slice(&input).map_err(|_| "installer_request_invalid")?;
    if request.format != 1
        || !request.folder.is_absolute()
        || !journal::valid_id(&request.attempt)
        || !journal::valid_sha(&request.attempt_sha256)
        || !journal::valid_sha(&request.plan_sha256)
        || request.folder.file_name().and_then(|v| v.to_str()) != Some("update-receipts")
        || request
            .folder
            .canonicalize()
            .map_err(|_| "installer_request_invalid")?
            != request.folder
    {
        return Err("installer_request_invalid".into());
    }
    let folder = &request.folder;
    let id = &request.attempt;
    let digest = &request.attempt_sha256;
    let (plan, plan_digest) = load_plan(folder, id, digest)?;
    if plan_digest != request.plan_sha256 {
        return Err("installer_plan_changed".into());
    }
    let _staging = directories(folder)?;
    let _target = directories(&plan.install_directory)?;
    let arguments = parameters(&plan.install_directory)?;
    let executable = std::env::current_exe().map_err(|_| "installer_observer_path_missing")?;
    if executable
        .canonicalize()
        .map_err(|_| "installer_observer_path_missing")?
        != path(folder, id, "installer-observer.exe")
            .canonicalize()
            .map_err(|_| "installer_observer_path_missing")?
    {
        return Err("installer_observer_path_changed".into());
    }
    let mut observer = checked_file(&executable)?;
    if file_hash(&mut observer)? != plan.observer_sha256 {
        return Err("installer_observer_changed".into());
    }
    let installer = path(folder, id, "installer.exe");
    let _payload = verify_payload(&installer, &plan.payload_sha256, &plan.target_version)?;
    let gui = unsafe { DaemonProcess::from_transfer(request.application_process)? };
    gui.wait_for_exit(0)?;
    if journal::has_phase(folder, id, digest, "installer-starting")? {
        return Err("installer_already_dispatched".into());
    }
    journal::phase(folder, id, digest, "installer-observer-ready")?;
    while !gui.wait_for_exit(1000)? {
        if journal::has_phase(folder, id, digest, "installer-handoff-aborted")? {
            return Ok(());
        }
    }
    let _lock = journal::lock(folder)?;
    if load_plan(folder, id, digest)?.1 != plan_digest {
        return Err("installer_plan_changed".into());
    }
    // Only the original application's explicit grant permits this single dispatch.
    if journal::has_phase(folder, id, digest, "installer-handoff-aborted")?
        || !journal::has_phase(folder, id, digest, "install-requested")?
    {
        return Ok(());
    }
    if !journal::has_phase(folder, id, digest, "daemon-stopped")?
        || journal::has_phase(folder, id, digest, "installer-starting")?
        || journal::has_phase(folder, id, digest, "reconciled-without-replay")?
    {
        return Err("installer_dispatch_not_authorized".into());
    }
    journal::phase(folder, id, digest, "application-exited")?;
    journal::phase(folder, id, digest, "installer-starting")?;
    // NSIS requires /D to be the last argument, unquoted even for spaces.
    let executable = wide(installer.as_os_str())?;
    let verb = wide(std::ffi::OsStr::new("open"))?;
    let working = wide(folder.as_os_str())?;
    let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if initialized < 0 {
        return Err("installer_com_initialization_failed".into());
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: arguments.as_ptr(),
        lpDirectory: working.as_ptr(),
        nShow: 1,
        ..Default::default()
    };
    let launched = unsafe { ShellExecuteExW(&mut info) };
    let launch_error = if launched == 0 {
        Some(std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32)
    } else {
        None
    };
    let (state, exit_code, launch_error) = if !info.hProcess.is_null() {
        let process = ProcessHandle(info.hProcess);
        image_matches(&process, &installer)?;
        (
            "original_process_exited",
            Some(observe(&process)?),
            launch_error,
        )
    } else if let Some(code) = launch_error {
        ("launch_rejected", None, Some(code.max(1)))
    } else {
        // A shell success without the physical process handle cannot prove completion.
        return Err("installer_process_handle_unconfirmed".into());
    };
    let result = Outcome {
        format: 1,
        attempt: id.clone(),
        attempt_sha256: digest.clone(),
        plan_sha256: plan_digest,
        payload_sha256: plan.payload_sha256,
        state: state.into(),
        exit_code,
        launch_error,
    };
    journal::record(&path(folder, id, "installer-result.json"), &result)?;
    journal::phase(
        folder,
        id,
        digest,
        if result.successful() {
            "install-applied"
        } else {
            "installer-failed"
        },
    )
}
pub(crate) fn early_mode() -> Option<Result<(), String>> {
    let args: Vec<_> = std::env::args_os().collect();
    args.get(1).filter(|v| *v == ARG).map(|_| {
        if args.len() == 2 {
            run()
        } else {
            Err("installer_request_invalid".into())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("cowork-installer-proof-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    std::fs::remove_file(entry.path()).unwrap();
                }
            }
            std::fs::remove_dir(&self.0).unwrap();
        }
    }
    #[test]
    fn original_native_process_exit_code_is_observed_without_pid_lookup() {
        use std::os::windows::process::CommandExt;
        let executable = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let mut child = Command::new(executable)
            .args(["/d", "/c"])
            .raw_arg("exit /b 7")
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let mut handle = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                windows_sys::Win32::Foundation::DuplicateHandle(
                    GetCurrentProcess(),
                    child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut handle,
                    PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(observe(&ProcessHandle(handle)).unwrap(), 7);
        assert_eq!(child.wait().unwrap().code(), Some(7));
    }
    #[test]
    fn bound_native_result_survives_lost_phase_and_tamper_never_releases_gate() {
        let fixture = Fixture::new();
        let update_id = "a".repeat(64);
        let id = format!("{update_id}-{}", uuid::Uuid::new_v4());
        let attempt = journal::Attempt { format:journal::FORMAT,attempt:id.clone(),update_id,
            from:env!("CARGO_PKG_VERSION").into(),to:env!("CARGO_PKG_VERSION").into(),
            payload_sha256:"b".repeat(64),signature_verified:true,state:"shutdown_prepared".into(),
            daemon:serde_json::from_value(serde_json::json!({"format":1,"product":"LocalAI Cowork V2",
                "version":env!("CARGO_PKG_VERSION"),"schema_version":8,"pid":4321,"instance_id":uuid::Uuid::new_v4(),"closing":false})).unwrap() };
        let digest = journal::record(&fixture.0.join(format!("{id}.json")), &attempt).unwrap();
        let plan = Plan {
            format: 1,
            attempt: id.clone(),
            attempt_sha256: digest.clone(),
            payload_sha256: attempt.payload_sha256.clone(),
            observer_sha256: "c".repeat(64),
            target_version: attempt.to.clone(),
            install_directory: fixture.0.clone(),
            application_name: "cowork.exe".into(),
            installer_contract: CONTRACT.into(),
        };
        let plan_digest =
            journal::record(&path(&fixture.0, &id, "installer-plan.json"), &plan).unwrap();
        for phase in [
            "daemon-stopped",
            "install-requested",
            "application-exited",
            "installer-starting",
        ] {
            journal::phase(&fixture.0, &id, &digest, phase).unwrap();
        }
        assert!(!journal::pending(&fixture.0).unwrap()[0].can_reconcile);
        let result = Outcome {
            format: 1,
            attempt: id.clone(),
            attempt_sha256: digest.clone(),
            plan_sha256: plan_digest,
            payload_sha256: attempt.payload_sha256,
            state: "original_process_exited".into(),
            exit_code: Some(0),
            launch_error: None,
        };
        let result_path = path(&fixture.0, &id, "installer-result.json");
        journal::record(&result_path, &result).unwrap();
        // Simulate the observer ending between the native result and phase write.
        assert!(!journal::has_phase(&fixture.0, &id, &digest, "install-applied").unwrap());
        assert!(journal::pending(&fixture.0).unwrap().is_empty());
        let mut changed = serde_json::to_value(&result).unwrap();
        changed["payload_sha256"] = serde_json::json!("d".repeat(64));
        std::fs::write(&result_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(journal::pending(&fixture.0).is_err());
    }
    #[test]
    fn nsis_target_is_exact_last_unquoted_argument_and_rejects_injection() {
        let params = parameters(Path::new(r"C:\Users\A B\工具")).unwrap();
        assert_eq!(
            String::from_utf16(&params[..params.len() - 1]).unwrap(),
            r"/P /UPDATE /D=C:\Users\A B\工具"
        );
        for bad in [
            r"relative\folder",
            r"\\server\share",
            "C:\\folder\" /S",
            "C:\\folder\n/S",
        ] {
            assert!(parameters(Path::new(bad)).is_err());
        }
    }
}
