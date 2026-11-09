//! Registration is initiated only by the trusted local application window.
pub fn initialize() {
    if cfg!(debug_assertions) {
        return;
    }
    // Linux packages cannot register a desktop user's manifests from a root
    // postinstall. Register at the first actual user launch instead.
    tauri::async_runtime::spawn_blocking(|| {
        if let Ok(executable) = std::env::current_exe()
            && let Some(directory) = executable.parent()
        {
            let mut command = std::process::Command::new(directory.join(if cfg!(windows) {
                "cowork-browser-host.exe"
            } else {
                "cowork-browser-host"
            }));
            command.arg("--register");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let _ = command.output();
        }
    });
}
#[tauri::command]
pub async fn browser_host_registration(
    window: tauri::WebviewWindow,
    action: String,
) -> Result<serde_json::Value, String> {
    if !super::application_window(&window)
        || !["register", "unregister", "diagnose"].contains(&action.as_str())
    {
        return Err("Forbidden".into());
    }
    let directory = std::env::current_exe()
        .map_err(|_| "Browser host path unavailable")?
        .parent()
        .ok_or("Browser host path unavailable")?
        .to_path_buf();
    let name = if cfg!(windows) {
        "cowork-browser-host.exe"
    } else {
        "cowork-browser-host"
    };
    let host = if cfg!(debug_assertions) {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "cowork-browser-host-{}{}",
                env!("COWORK_TARGET_TRIPLE"),
                if cfg!(windows) { ".exe" } else { "" }
            ))
    } else {
        directory.join(name)
    };
    tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(host);
        command.arg(format!("--{action}"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command.output().map_err(
            |_| "Browser host is missing. Build or install the current browser-host package.",
        )?;
        if !output.status.success() || output.stdout.len() > 16384 {
            return Err(
                "Browser host registration failed. Close active extension connections and retry.",
            );
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| "Browser host returned an invalid result.")
    })
    .await
    .map_err(|_| "Browser host operation failed.")?
    .map_err(str::to_owned)
}
