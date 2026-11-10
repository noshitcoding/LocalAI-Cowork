//! Current-user registration for the same name read by the autostart plugin.
//! The pinned plugin concatenates raw argv on Windows; encode OS argv explicitly.
//! These writes are reachable only from the explicit native enable/disable action.
use tauri_plugin_autostart::ManagerExt;
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
#[derive(Clone)]
struct Value {
    kind: REG_VALUE_TYPE,
    bytes: Vec<u8>,
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn open(path: &str, create: bool) -> Result<Option<Key>, String> {
    let mut key = std::ptr::null_mut();
    let path = wide(path);
    let status = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut key,
            )
        }
    };
    match status {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND if !create => Ok(None),
        _ => Err("autostart_registry_unavailable".into()),
    }
}
fn read(key: &Key, name: &[u16]) -> Result<Option<Value>, String> {
    let mut size = 0;
    let mut kind = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS || size > 65536 {
        return Err("autostart_registration_unavailable".into());
    }
    let mut bytes = vec![0; size as usize];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size as usize > bytes.len() {
        return Err("autostart_registration_changed".into());
    }
    bytes.truncate(size as usize);
    Ok(Some(Value { kind, bytes }))
}
fn put(key: &Key, name: &[u16], value: Option<&Value>) -> Result<(), String> {
    let status = unsafe {
        match value {
            Some(value) => RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                value.kind,
                value.bytes.as_ptr(),
                value.bytes.len() as u32,
            ),
            None => RegDeleteValueW(key.0, name.as_ptr()),
        }
    };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND && value.is_none() {
        Ok(())
    } else {
        Err("autostart_registration_failed".into())
    }
}
pub fn set(app: &tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let name = wide(&app.package_info().name);
    let executable = std::env::current_exe().map_err(|_| "autostart_executable_unavailable")?;
    let directory = super::private_data_dir(app)?;
    let command = super::desktop_launch::startup_command(&executable, &directory)?;
    let Some(run) = open(r"Software\Microsoft\Windows\CurrentVersion\Run", enabled)? else {
        let actual = app
            .autolaunch()
            .is_enabled()
            .map_err(|_| "autostart_status_failed")?;
        return if actual == enabled {
            Ok(actual)
        } else {
            Err("autostart_state_not_confirmed".into())
        };
    };
    let approved = open(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        false,
    )?;
    let previous = read(&run, &name)?;
    let previous_approval = approved
        .as_ref()
        .map(|key| read(key, &name))
        .transpose()?
        .flatten();
    let outcome = (|| {
        if enabled {
            if let Some(key) = &approved {
                put(
                    key,
                    &name,
                    Some(&Value {
                        kind: REG_BINARY,
                        bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    }),
                )?;
            }
            let bytes: Vec<u8> = wide(&command)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect();
            put(
                &run,
                &name,
                Some(&Value {
                    kind: REG_SZ,
                    bytes,
                }),
            )?;
        } else {
            put(&run, &name, None)?;
        }
        let actual = app
            .autolaunch()
            .is_enabled()
            .map_err(|_| "autostart_status_failed".to_owned())?;
        if actual != enabled {
            return Err("autostart_state_not_confirmed".into());
        }
        Ok(actual)
    })();
    if outcome.is_err() {
        let restored = put(&run, &name, previous.as_ref());
        let approved_restored = approved
            .as_ref()
            .map(|key| put(key, &name, previous_approval.as_ref()))
            .transpose();
        if restored.is_err() || approved_restored.is_err() {
            return Err("autostart_registration_uncertain".into());
        }
    }
    outcome
}
