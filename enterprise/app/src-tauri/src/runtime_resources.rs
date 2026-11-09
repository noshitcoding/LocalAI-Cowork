//! One validated installation root. In particular, never canonicalize the Windows
//! module path: package-identity virtualization can resolve it into the parent's
//! MSIX cache even though the executable and its resources live elsewhere.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};
#[cfg(any(not(windows), debug_assertions))]
use tauri::Manager;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeArtifact {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLayout {
    pub schema_version: u32,
    pub app_version: String,
    pub target: String,
    pub artifacts: BTreeMap<String, RuntimeArtifact>,
}

#[derive(Debug, Clone)]
pub struct RuntimeResources {
    root: PathBuf,
    pub layout: RuntimeLayout,
}
static RESOURCES: OnceLock<Result<RuntimeResources, String>> = OnceLock::new();

pub fn target() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else {
        "unsupported"
    }
}

pub fn safe_relative(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    // Check both separators on every platform, including manifests tested on Linux.
    if value.is_empty()
        || value.contains(':')
        || value.starts_with(['/', '\\'])
        || value
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == ".." || part == ".")
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("RUNTIME_ROOT_INVALID: resource path must be a confined relative path".into());
    }
    Ok(path)
}

pub fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    if expected.len() != 64 || !expected.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("RUNTIME_HASH_INVALID: invalid SHA-256 digest".into());
    }
    let mut file = fs::File::open(path)
        .map_err(|e| format!("RUNTIME_RESOURCE_UNREADABLE: {}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| format!("RUNTIME_RESOURCE_UNREADABLE: {e}"))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual = hash
        .finalize()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("RUNTIME_HASH_MISMATCH: {}", path.display()));
    }
    Ok(())
}

impl RuntimeResources {
    pub fn load(
        root: PathBuf,
        expected_version: &str,
        expected_target: &str,
    ) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("RUNTIME_ROOT_INVALID: installation root is not absolute".into());
        }
        let bytes = fs::read(root.join("runtime-layout.json"))
            .map_err(|e| format!("RUNTIME_ROOT_INVALID: {}: {e}", root.display()))?;
        let layout: RuntimeLayout =
            serde_json::from_slice(&bytes).map_err(|e| format!("RUNTIME_LAYOUT_INVALID: {e}"))?;
        if layout.schema_version != 1
            || layout.app_version != expected_version
            || layout.target != expected_target
        {
            return Err("RUNTIME_LAYOUT_MISMATCH: schema, app version or target differs".into());
        }
        let required = if expected_target == "linux-x64" {
            vec!["daemonManifest", "codexManifest", "crewScript"]
        } else {
            vec![
                "daemonManifest",
                "codexManifest",
                "crewManifest",
                "crewScript",
            ]
        };
        for name in required {
            if !layout.artifacts.contains_key(name) {
                return Err(format!("RUNTIME_LAYOUT_INVALID: missing {name}"));
            }
        }
        for (name, expected) in [
            (
                "daemonManifest",
                format!("daemon/{expected_target}/manifest.json"),
            ),
            ("codexManifest", "codex/runtime-bundle-manifest.json".into()),
            (
                "crewManifest",
                "python/crew_runtime/runtime-bundle-manifest.json".into(),
            ),
            ("crewScript", "python/crew_runtime/main.py".into()),
        ] {
            if layout
                .artifacts
                .get(name)
                .is_some_and(|artifact| artifact.path != expected)
            {
                return Err(format!(
                    "RUNTIME_LAYOUT_INVALID: inconsistent {name} location"
                ));
            }
        }
        for artifact in layout.artifacts.values() {
            let relative = safe_relative(&artifact.path)?;
            // Disallow symlink/junction escapes without canonicalizing the root.
            let mut checked = root.clone();
            for part in relative.components() {
                checked.push(part.as_os_str());
                let metadata = fs::symlink_metadata(&checked).map_err(|e| {
                    format!("RUNTIME_RESOURCE_UNREADABLE: {}: {e}", checked.display())
                })?;
                if metadata.file_type().is_symlink() {
                    return Err("RUNTIME_ROOT_INVALID: linked resource is not allowed".into());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err("RUNTIME_ROOT_INVALID: reparse resource is not allowed".into());
                    }
                }
            }
            verify_hash(&root.join(relative), &artifact.sha256)?;
        }
        Ok(Self { root, layout })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn artifact(&self, name: &str) -> Result<PathBuf, String> {
        let artifact = self
            .layout
            .artifacts
            .get(name)
            .ok_or_else(|| format!("RUNTIME_LAYOUT_INVALID: missing {name}"))?;
        Ok(self.root.join(safe_relative(&artifact.path)?))
    }
}

#[cfg(windows)]
pub fn module_executable() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    let mut buffer = vec![0u16; 512];
    loop {
        let count = unsafe {
            GetModuleFileNameW(
                std::ptr::null_mut(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        } as usize;
        if count == 0 {
            return Err(format!(
                "RUNTIME_ROOT_INVALID: {}",
                std::io::Error::last_os_error()
            ));
        }
        if count < buffer.len() {
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..count],
            )));
        }
        if buffer.len() >= 32768 {
            return Err("RUNTIME_ROOT_INVALID: module path too long".into());
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}
#[cfg(not(windows))]
pub fn module_executable() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("RUNTIME_ROOT_INVALID: {e}"))
}

pub fn initialize<R: tauri::Runtime>(
    _app: &tauri::AppHandle<R>,
) -> Result<RuntimeResources, String> {
    RESOURCES
        .get_or_init(|| {
            #[cfg(all(windows, not(debug_assertions)))]
            let root = module_executable()?
                .parent()
                .ok_or("RUNTIME_ROOT_INVALID: no module parent")?
                .to_path_buf();
            #[cfg(any(not(windows), debug_assertions))]
            let root = {
                #[cfg(debug_assertions)]
                if let Some(path) = std::env::var_os("LACOWORK_RUNTIME_ROOT") {
                    return RuntimeResources::load(
                        PathBuf::from(path),
                        env!("CARGO_PKG_VERSION"),
                        target(),
                    );
                }
                _app.path()
                    .resource_dir()
                    .map_err(|e| format!("RUNTIME_ROOT_INVALID: {e}"))?
            };
            RuntimeResources::load(root, env!("CARGO_PKG_VERSION"), target())
        })
        .clone()
}

pub fn resource_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    initialize(app).map(|r| r.root)
}

/// Read-only pre-Tauri packaged smoke. No AppData, registration or GUI effects.
pub fn dispatch_smoke() -> Option<i32> {
    if !std::env::args().any(|arg| arg == "--lacowork-runtime-smoke") {
        return None;
    }
    let result = module_executable().and_then(|exe| {
        RuntimeResources::load(
            exe.parent()
                .ok_or("RUNTIME_ROOT_INVALID: no module parent")?
                .to_path_buf(),
            env!("CARGO_PKG_VERSION"),
            target(),
        )
    });
    match result {
        Ok(resources) => {
            println!(
                "{}",
                serde_json::json!({"ok":true,"schemaVersion":1,"appVersion":resources.layout.app_version,"target":resources.layout.target,"artifactCount":resources.layout.artifacts.len()})
            );
            Some(0)
        }
        Err(error) => {
            println!("{}", serde_json::json!({"ok":false,"error":error}));
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape_paths_on_every_platform() {
        for path in [
            "",
            "../x",
            "a/../x",
            "/x",
            "C:\\x",
            "a\\..\\x",
            "\\\\server\\x",
            "a//x",
            "./x",
        ] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert!(safe_relative("daemon/windows-x64/manifest.json").is_ok());
    }
    #[test]
    fn layout_checks_target_version_hash_and_root_without_cwd_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut artifacts = BTreeMap::new();
        for (name, path) in [
            ("daemonManifest", "daemon/fixture/manifest.json"),
            ("codexManifest", "codex/runtime-bundle-manifest.json"),
            (
                "crewManifest",
                "python/crew_runtime/runtime-bundle-manifest.json",
            ),
            ("crewScript", "python/crew_runtime/main.py"),
        ] {
            fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
            fs::write(dir.path().join(path), "fixture").unwrap();
            artifacts.insert(
                name.into(),
                RuntimeArtifact {
                    path: path.into(),
                    sha256: Sha256::digest(b"fixture")
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                },
            );
        }
        let layout = RuntimeLayout {
            schema_version: 1,
            app_version: "test".into(),
            target: "fixture".into(),
            artifacts,
        };
        fs::write(
            dir.path().join("runtime-layout.json"),
            serde_json::to_vec(&layout).unwrap(),
        )
        .unwrap();
        let resolved = RuntimeResources::load(dir.path().to_path_buf(), "test", "fixture").unwrap();
        assert_eq!(
            resolved.artifact("crewScript").unwrap(),
            dir.path().join("python/crew_runtime/main.py")
        );
        assert!(RuntimeResources::load(dir.path().to_path_buf(), "wrong", "fixture").is_err());
        assert!(RuntimeResources::load(dir.path().join("virtualized"), "test", "fixture").is_err());
        fs::write(dir.path().join("python/crew_runtime/main.py"), "tampered").unwrap();
        assert!(
            RuntimeResources::load(dir.path().to_path_buf(), "test", "fixture")
                .unwrap_err()
                .starts_with("RUNTIME_HASH_MISMATCH")
        );
    }
}
