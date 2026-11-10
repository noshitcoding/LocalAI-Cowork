use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};
const HOST: &str = "dev.localai.cowork.browser";
fn identity() -> Value {
    serde_json::from_str(include_str!("../../../extensions/browser/identity.json"))
        .expect("bundled public extension identity")
}
fn ordinary(path: &Path) -> io::Result<()> {
    for parent in path.ancestors() {
        let Ok(meta) = fs::symlink_metadata(parent) else {
            continue;
        };
        if meta.is_symlink() {
            return Err(io::Error::other("browser_host_alias"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("browser_host_reparse_point"));
            }
        }
    }
    Ok(())
}
fn directory(path: &Path) -> io::Result<()> {
    ordinary(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn write(path: &Path, value: &Value) -> io::Result<()> {
    ordinary(path)?;
    directory(
        path.parent()
            .ok_or_else(|| io::Error::other("manifest_directory"))?,
    )?;
    let raw = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_file_name(format!(".manifest-{}.tmp", uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)
}
fn manifests() -> io::Result<Vec<(String, PathBuf)>> {
    let root = cowork_toolbox::browser_bridge::discovery_directory()
        .map_err(|_| io::Error::other("user_directory"))?
        .join("native-host");
    #[cfg(windows)]
    {
        Ok(vec![
            ("chrome".into(), root.join("chrome.json")),
            ("firefox".into(), root.join("firefox.json")),
        ])
    }
    #[cfg(unix)]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("user_directory"))?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let _ = root;
        Ok(vec![
            (
                "chrome".into(),
                config.join(format!("google-chrome/NativeMessagingHosts/{HOST}.json")),
            ),
            (
                "firefox".into(),
                home.join(format!(".mozilla/native-messaging-hosts/{HOST}.json")),
            ),
        ])
    }
}
#[cfg(windows)]
fn registry(engine: &str) -> String {
    format!(
        "HKCU\\Software\\{}\\NativeMessagingHosts\\{HOST}",
        if engine == "chrome" {
            "Google\\Chrome"
        } else {
            "Mozilla"
        }
    )
}
#[cfg(windows)]
fn reg(args: &[&str]) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let result = Command::new("reg.exe")
        .args(args)
        .creation_flags(0x08000000)
        .output()?;
    if result.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("browser_registry_operation_failed"))
    }
}
pub fn run(action: &str) -> io::Result<Value> {
    let paths = manifests()?;
    let identity = identity();
    if action == "--register" {
        let source = std::env::current_exe()?;
        ordinary(&source)?;
        let bytes = fs::read(&source)?;
        let hash = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let directory_path = cowork_toolbox::browser_bridge::discovery_directory()
            .map_err(|_| io::Error::other("user_directory"))?
            .join("native-host")
            .join(hash);
        directory(&directory_path)?;
        let installed = directory_path.join(if cfg!(windows) {
            "cowork-browser-host.exe"
        } else {
            "cowork-browser-host"
        });
        ordinary(&installed)?;
        if installed.exists() {
            if fs::read(&installed)? != bytes {
                return Err(io::Error::other("browser_host_binary_changed"));
            }
        } else {
            use std::io::Write;
            let temporary = directory_path.join(format!(".host-{}.tmp", uuid::Uuid::new_v4()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(temporary, &installed)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&installed, fs::Permissions::from_mode(0o700))?;
        }
        for (engine, path) in &paths {
            let mut manifest = json!({"name":HOST,"description":"LocalAI Cowork selected-tab bridge","path":installed,"type":"stdio"});
            if engine == "chrome" {
                manifest["allowed_origins"] = json!([format!(
                    "chrome-extension://{}/",
                    identity["chrome_id"].as_str().unwrap()
                )]);
            } else {
                manifest["allowed_extensions"] = json!([identity["firefox_id"]]);
            }
            write(path, &manifest)?;
            #[cfg(windows)]
            reg(&[
                "ADD",
                &registry(engine),
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                &path.to_string_lossy(),
                "/f",
            ])?;
        }
    } else if action == "--unregister" {
        for (engine, path) in &paths {
            ordinary(path)?;
            if path.exists() {
                let manifest: Value = serde_json::from_slice(&fs::read(path)?)?;
                if manifest["name"] != HOST {
                    return Err(io::Error::other("browser_manifest_not_owned"));
                }
                let root = cowork_toolbox::browser_bridge::discovery_directory()
                    .map_err(|_| io::Error::other("user_directory"))?
                    .join("native-host");
                if !Path::new(manifest["path"].as_str().unwrap_or("")).starts_with(root) {
                    return Err(io::Error::other("browser_manifest_not_owned"));
                }
                #[cfg(windows)]
                reg(&["DELETE", &registry(engine), "/f"])?;
                fs::remove_file(path)?;
            }
            let _ = engine;
        }
    } else if action != "--diagnose" {
        return Err(io::Error::other("invalid_registration_action"));
    }
    #[cfg(unix)]
    let portal = {
        let result = Command::new("gdbus")
            .args([
                "introspect",
                "--session",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
            ])
            .output();
        result.is_ok_and(|r| {
            r.status.success()
                && (String::from_utf8_lossy(&r.stdout).contains("WebExtensions")
                    || String::from_utf8_lossy(&r.stdout).contains("NativeMessaging"))
        })
    };
    #[cfg(windows)]
    let portal = true;
    #[cfg(windows)]
    let registered = paths.iter().all(|(engine, path)| {
        use std::os::windows::process::CommandExt;
        path.is_file()
            && Command::new("reg.exe")
                .args(["QUERY", &registry(engine), "/ve"])
                .creation_flags(0x08000000)
                .output()
                .is_ok_and(|o| {
                    o.status.success()
                        && String::from_utf8_lossy(&o.stdout)
                            .contains(path.to_string_lossy().as_ref())
                })
    });
    #[cfg(unix)]
    let registered = paths.iter().all(|(_, p)| p.is_file());
    Ok(
        json!({"registered":registered,"chrome_id":identity["chrome_id"],"firefox_id":identity["firefox_id"],"confined_firefox_portal_available":portal,"confined_firefox_hint":if portal{""}else{"Firefox Snap/Flatpak requires a WebExtensions portal or native messaging proxy. Use the distribution portal package or an unconfined Firefox installation."}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_replacement_publishes_complete_json_without_partial_files() {
        let root =
            std::env::temp_dir().join(format!("cowork-host-manifest-{}", uuid::Uuid::new_v4()));
        directory(&root).unwrap();
        let path = root.join("host.json");
        write(&path, &json!({"version":1})).unwrap();
        write(
            &path,
            &json!({"version":2,"allowed_extensions":["fixture"]}),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["version"],
            2
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
