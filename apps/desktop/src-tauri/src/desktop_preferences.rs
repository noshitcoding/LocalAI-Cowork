//! Native effects are explicit UI actions; startup never changes OS registration.
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;
fn permit(window: &tauri::WebviewWindow) -> Result<(), String> {
    if super::application_window(window) {
        Ok(())
    } else {
        Err("desktop_window_required".into())
    }
}
#[derive(serde::Serialize)]
pub struct DesktopStatus {
    launch_at_startup: bool,
    notification_permission: String,
    sound_available: bool,
}
#[tauri::command]
pub fn desktop_preferences_status(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<DesktopStatus, String> {
    permit(&window)?;
    Ok(DesktopStatus {
        launch_at_startup: app
            .autolaunch()
            .is_enabled()
            .map_err(|_| "autostart_status_failed")?,
        notification_permission: format!(
            "{:?}",
            app.notification()
                .permission_state()
                .map_err(|_| "notification_status_failed")?
        )
        .to_lowercase(),
        sound_available: cfg!(any(windows, target_os = "macos")) || linux_sound().is_some(),
    })
}
#[tauri::command]
pub fn desktop_set_autostart(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<bool, String> {
    permit(&window)?;
    #[cfg(windows)]
    {
        super::windows_startup::set(&app, enabled)
    }
    #[cfg(not(windows))]
    {
        let manager = app.autolaunch();
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|_| "autostart_change_failed")?;
        manager
            .is_enabled()
            .map_err(|_| "autostart_status_failed".into())
    }
}
#[tauri::command]
pub fn desktop_request_notifications(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<String, String> {
    permit(&window)?;
    Ok(format!(
        "{:?}",
        app.notification()
            .request_permission()
            .map_err(|_| "notification_permission_failed")?
    )
    .to_lowercase())
}
#[tauri::command]
pub fn desktop_notify(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    permit(&window)?;
    if title.len() > 256 || body.len() > 4096 || title.contains('\0') || body.contains('\0') {
        return Err("notification_invalid".into());
    }
    if format!(
        "{:?}",
        app.notification()
            .permission_state()
            .map_err(|_| "notification_status_failed")?
    ) != "Granted"
    {
        return Err("notification_permission_required".into());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .silent()
        .show()
        .map_err(|_| "notification_failed".into())
}
fn linux_sound() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let candidate = dir.join("canberra-gtk-play");
        if candidate.is_file() {
            return candidate.canonicalize().ok();
        }
    }
    None
}
#[tauri::command]
pub fn desktop_sound(window: tauri::WebviewWindow) -> Result<(), String> {
    permit(&window)?;
    #[cfg(windows)]
    {
        if unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0x40) } != 0 {
            return Ok(());
        }
        return Err("sound_failed".into());
    }
    #[cfg(target_os = "macos")]
    {
        if std::process::Command::new("/usr/bin/afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .status()
            .is_ok_and(|s| s.success())
        {
            return Ok(());
        }
        return Err("sound_failed".into());
    }
    #[cfg(target_os = "linux")]
    {
        let program = linux_sound().ok_or("sound_unavailable")?;
        if std::process::Command::new(program)
            .args(["-i", "complete"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Ok(());
        }
        return Err("sound_failed".into());
    }
    #[allow(unreachable_code)]
    Err("sound_unavailable".into())
}
