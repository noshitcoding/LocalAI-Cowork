use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use crate::runtime_resources::RuntimeResources;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonManifest {
    schema_version: u32,
    target: String,
    binary: String,
    sha256: String,
    version: String,
    #[serde(default)]
    files: Vec<DaemonBundleFile>,
}

#[derive(Debug, Deserialize)]
struct DaemonBundleFile {
    name: String,
    sha256: String,
}

pub fn provision_and_start(resources: &RuntimeResources, app_data_dir: &Path) -> Result<Vec<String>, String> {
    let manifest_path = resources.artifact("daemonManifest")?;
    let source_root = manifest_path.parent().ok_or("DAEMON_MANIFEST_UNREADABLE: missing parent")?;
    let manifest: DaemonManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("DAEMON_MANIFEST_UNREADABLE: {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("invalid local daemon manifest: {error}"))?;
    validate_manifest(&manifest)?;
    let source = source_root.join(&manifest.binary);
    verify_digest(&source, &manifest.sha256)?;
    let installed = install_versioned_binary(source_root, &source, &manifest)?;
    write_runtime_paths(resources, app_data_dir)?;

    let pid = start_now(&installed).map_err(|error| format!("DAEMON_START_FAILED: {error}"))?;
    tauri::async_runtime::block_on(crate::local_daemon_bridge::bootstrap_health(&manifest.version, pid))?;

    let mut warnings = Vec::new();
    if let Err(error) = register_login_start(&installed) {
        warnings.push(error);
    }
    Ok(warnings)
}

fn write_runtime_paths(resources: &RuntimeResources, app_data_dir: &Path) -> Result<(), String> {
    let data_dir = default_data_dir();
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("failed to create {}: {error}", data_dir.display()))?;
    set_private_directory_permissions(&data_dir)?;
    #[cfg(windows)]
    let crew_python = app_data_dir
        .join("crew-runtime")
        .join("venv")
        .join("Scripts")
        .join("python.exe");
    #[cfg(not(windows))]
    let crew_python = app_data_dir
        .join("crew-runtime")
        .join("venv")
        .join("bin")
        .join("python");
    let payload = serde_json::json!({
        "schema_version": 1,
        "crew_python": crew_python,
        "crew_script": resources.artifact("crewScript")?,
        "codex_root": resources.artifact("codexManifest")?.parent(),
        "app_executable": crate::runtime_resources::module_executable()?,
        "codex_profiles": app_data_dir.join("codex-profiles"),
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    let path = data_dir.join("runtime-paths.json");
    let temporary = data_dir.join(format!(".runtime-paths-{}.json.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut output = fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
            .map_err(|error| format!("failed to create {}: {error}", temporary.display()))?;
        output.write_all(&serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?)
            .and_then(|_| output.sync_all())
            .map_err(|error| format!("failed to sync runtime paths: {error}"))?;
    }
    atomic_replace(&temporary, &path)
        .map_err(|error| format!("failed to activate {}: {error}", path.display()))?;
    Ok(())
}

fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 { return Err(std::io::Error::last_os_error()); }
        Ok(())
    }
    #[cfg(not(windows))] { fs::rename(source, target) }
}

fn validate_manifest(manifest: &DaemonManifest) -> Result<(), String> {
    if !matches!(manifest.schema_version, 1 | 2) {
        return Err(format!(
            "unsupported local daemon manifest schema {}",
            manifest.schema_version
        ));
    }
    if manifest.target != bundle_target()? {
        return Err(format!(
            "local daemon target mismatch: expected {}, found {}",
            bundle_target()?,
            manifest.target
        ));
    }
    if manifest.binary != binary_name()? {
        return Err("local daemon manifest contains an unexpected binary name".to_owned());
    }
    if manifest.sha256.len() != 64
        || !manifest
            .sha256
            .chars()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err("local daemon manifest contains an invalid SHA-256 digest".to_owned());
    }
    if manifest.version != env!("CARGO_PKG_VERSION") {
        return Err("DAEMON_VERSION_MISMATCH: bundled daemon and app versions differ".to_owned());
    }
    for file in &manifest.files {
        if file.name.is_empty()
            || Path::new(&file.name)
                .file_name()
                .and_then(|value| value.to_str())
                != Some(file.name.as_str())
            || file.sha256.len() != 64
            || !file
                .sha256
                .chars()
                .all(|value| value.is_ascii_hexdigit())
        {
            return Err("local daemon manifest contains an invalid resource file".to_owned());
        }
    }
    Ok(())
}

fn verify_digest(path: &Path, expected: &str) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("failed to read local daemon {}: {error}", path.display()))?;
    let actual = Sha256::digest(bytes)
        .iter()
        .map(|value| format!("{value:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "DAEMON_HASH_MISMATCH: {}",
            path.display()
        ));
    }
    Ok(())
}

fn install_versioned_binary(
    source_root: &Path,
    source: &Path,
    manifest: &DaemonManifest,
) -> Result<PathBuf, String> {
    // Keep sidecars with the content-addressed executable. An upgrade must not
    // replace a DLL still used by the previous daemon or compromise rollback.
    let suffix = &manifest.sha256[..16];
    let bin_dir = default_data_dir().join("bin").join(suffix);
    fs::create_dir_all(&bin_dir)
        .map_err(|error| format!("failed to create {}: {error}", bin_dir.display()))?;
    set_private_directory_permissions(&bin_dir)?;
    let destination = bin_dir.join(&manifest.binary);
    if destination.exists() {
        verify_digest(&destination, &manifest.sha256)?;
    } else {
        let temporary = bin_dir.join(format!(".cowork-local-daemon-{suffix}.tmp"));
        fs::copy(source, &temporary).map_err(|error| {
            format!(
                "failed to install local daemon from {}: {error}",
                source.display()
            )
        })?;
        set_executable_permissions(&temporary)?;
        verify_digest(&temporary, &manifest.sha256)?;
        fs::rename(&temporary, &destination).map_err(|error| {
            format!(
                "failed to activate local daemon {}: {error}",
                destination.display()
            )
        })?;
    }
    for file in &manifest.files {
        let source_file = source_root.join(&file.name);
        verify_digest(&source_file, &file.sha256)?;
        let destination_file = bin_dir.join(&file.name);
        if destination_file.exists() && verify_digest(&destination_file, &file.sha256).is_ok() {
            continue;
        }
        let temporary_file = bin_dir.join(format!(".{}.tmp", file.name));
        fs::copy(&source_file, &temporary_file).map_err(|error| {
            format!("failed to install daemon resource {}: {error}", file.name)
        })?;
        verify_digest(&temporary_file, &file.sha256)?;
        if destination_file.exists() {
            fs::remove_file(&destination_file).map_err(|error| {
                format!("failed to replace daemon resource {}: {error}", file.name)
            })?;
        }
        fs::rename(&temporary_file, &destination_file).map_err(|error| {
            format!("failed to activate daemon resource {}: {error}", file.name)
        })?;
    }
    Ok(destination)
}

#[cfg(windows)]
fn register_login_start(binary: &Path) -> Result<(), String> {
    let command = format!("\"{}\"", binary.display());
    let status = Command::new("reg.exe")
        .args([
            "ADD",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "OpenCoworkLocalDaemon",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to register daemon login start: {error}"))?;
    if !status.success() {
        return Err(format!(
            "failed to register daemon login start: reg.exe exited with {status}"
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn register_login_start(binary: &Path) -> Result<(), String> {
    let config_home = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".config"));
    let unit_dir = config_home.join("systemd").join("user");
    fs::create_dir_all(&unit_dir)
        .map_err(|error| format!("failed to create {}: {error}", unit_dir.display()))?;
    let unit = format!(
        "[Unit]\nDescription=Open Cowork local user daemon\n\n[Service]\nType=simple\nExecStart={}\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n",
        quote_exec_path(binary)
    );
    let unit_path = unit_dir.join("open-cowork-daemon.service");
    fs::write(&unit_path, unit)
        .map_err(|error| format!("failed to write {}: {error}", unit_path.display()))?;

    let reload = quiet_command("systemctl", &["--user", "daemon-reload"]);
    let enable = quiet_command(
        "systemctl",
        &["--user", "enable", "open-cowork-daemon.service"],
    );
    if reload.is_ok() && enable.is_ok() {
        return Ok(());
    }

    let autostart_dir = config_home.join("autostart");
    fs::create_dir_all(&autostart_dir)
        .map_err(|error| format!("failed to create {}: {error}", autostart_dir.display()))?;
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Open Cowork Local Daemon\nExec={}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        quote_exec_path(binary)
    );
    let desktop_path = autostart_dir.join("open-cowork-daemon.desktop");
    fs::write(&desktop_path, desktop)
        .map_err(|error| format!("failed to write {}: {error}", desktop_path.display()))?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn register_login_start(_binary: &Path) -> Result<(), String> {
    Err("automatic daemon login start is unsupported on this platform".to_owned())
}

fn start_now(binary: &Path) -> Result<u32, String> {
    let mut command = Command::new(binary);
    command
        .arg("--replace")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map(|child| child.id())
        .map_err(|error| format!("failed to start local daemon {}: {error}", binary.display()))
}

#[cfg(target_os = "linux")]
fn quiet_command(program: &str, args: &[&str]) -> Result<(), ()> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ())?
        .success()
        .then_some(())
        .ok_or(())
}

#[cfg(target_os = "linux")]
fn quote_exec_path(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_owned()))
        .join("OpenCowork")
        .join("daemon");
    #[cfg(not(windows))]
    return env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".local").join("state"))
        .join("open-cowork")
        .join("daemon");
}

#[cfg(not(windows))]
fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_owned()))
}

#[cfg(windows)]
fn bundle_target() -> Result<&'static str, String> {
    if cfg!(target_arch = "x86_64") {
        Ok("windows-x64")
    } else {
        Err("the bundled local daemon supports Windows x64 only".to_owned())
    }
}

#[cfg(target_os = "linux")]
fn bundle_target() -> Result<&'static str, String> {
    if cfg!(target_arch = "x86_64") {
        Ok("linux-x64")
    } else {
        Err("the bundled local daemon supports Linux x64 only".to_owned())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn bundle_target() -> Result<&'static str, String> {
    Err("the bundled local daemon is unsupported on this platform".to_owned())
}

#[cfg(windows)]
fn binary_name() -> Result<&'static str, String> {
    Ok("cowork-local-daemon.exe")
}

#[cfg(not(windows))]
fn binary_name() -> Result<&'static str, String> {
    Ok("cowork-local-daemon")
}

fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to secure {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn set_executable_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to secure {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_paths_replace_is_atomic_and_keeps_previous_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("runtime-paths.json");
        let next = dir.path().join("next.json");
        fs::write(&target, b"old").unwrap();
        assert!(atomic_replace(&next, &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        fs::write(&next, b"new").unwrap();
        atomic_replace(&next, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!next.exists());
    }

    #[test]
    fn manifest_rejects_path_substitution() {
        let manifest = DaemonManifest {
            schema_version: 1,
            target: bundle_target().unwrap().to_owned(),
            binary: "../unexpected".to_owned(),
            sha256: "0".repeat(64),
            version: "0.3.0".to_owned(),
            files: Vec::new(),
        };
        assert!(validate_manifest(&manifest).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_exec_path_quotes_spaces() {
        assert_eq!(
            quote_exec_path(Path::new("/tmp/Open Cowork/daemon")),
            "\"/tmp/Open Cowork/daemon\""
        );
    }
}
