use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    programs: BTreeMap<String, PathBuf>,
    github_token_file: Option<PathBuf>,
}
fn discover(names: &[&str]) -> Option<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH")?) {
        for name in names {
            let path = directory.join(name);
            if path.is_file()
                && let Ok(path) = path.canonicalize()
            {
                return Some(path);
            }
        }
    }
    None
}
pub(super) fn open(
    data: &Path,
    web: Option<&Path>,
) -> anyhow::Result<Arc<cowork_toolbox::Toolbox>> {
    open_at(data, web, std::path::absolute(data)?.join("toolbox"))
}
// Operator host inspection must not initialize or clean the live application toolbox.
// This fixed separate root retains its own original journals and ownership lease.
pub(super) fn host_trust_directory(data: &Path) -> PathBuf {
    data.join("host-trust-tools")
}
pub(super) fn open_host_trust(
    data: &Path,
    web: Option<&Path>,
) -> anyhow::Result<Arc<cowork_toolbox::Toolbox>> {
    open_at(data, web, std::path::absolute(host_trust_directory(data))?)
}
fn open_at(
    data: &Path,
    web: Option<&Path>,
    root: PathBuf,
) -> anyhow::Result<Arc<cowork_toolbox::Toolbox>> {
    let path = data.join("toolbox.json");
    let mut settings: Settings = if path.is_file() {
        anyhow::ensure!(
            path.metadata()?.len() < 256 * 1024,
            "Tool configuration exceeds limit"
        );
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        Settings::default()
    };
    let candidates: [(&str, &[&str]); 10] = [
        ("python", &["python.exe", "python3", "python"]),
        ("node", &["node.exe", "node"]),
        ("git", &["git.exe", "git"]),
        ("gh", &["gh.exe", "gh"]),
        ("bash", &["bash"]),
        ("powershell", &["pwsh.exe", "powershell.exe", "pwsh"]),
        (
            "chromium",
            &[
                "chromium",
                "chromium-browser",
                "google-chrome",
                "chrome.exe",
            ],
        ),
        ("libreoffice", &["soffice.exe", "libreoffice", "soffice"]),
        ("docker", &["docker.exe", "docker"]),
        ("tectonic", &["tectonic.exe", "tectonic"]),
    ];
    for (alias, names) in candidates {
        if !settings.programs.contains_key(alias)
            && let Some(path) = discover(names)
        {
            settings.programs.insert(alias.into(), path);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(system) = std::env::var("SystemRoot") {
            let path = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
            if path.is_file() {
                settings.programs.entry("powershell".into()).or_insert(path);
            }
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(folder) = std::env::var(variable) {
                for name in [
                    "Google/Chrome/Application/chrome.exe",
                    "Microsoft/Edge/Application/msedge.exe",
                ] {
                    let path = PathBuf::from(&folder).join(name);
                    if path.is_file() {
                        settings.programs.entry("chromium".into()).or_insert(path);
                    }
                }
            }
        }
    }
    let github_token = if let Some(path) = settings.github_token_file {
        let path = path.canonicalize()?;
        if let Some(web) = web {
            anyhow::ensure!(
                !path.starts_with(web.canonicalize()?),
                "Tool credentials must be outside served files"
            );
        }
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(
            file.metadata()?.len() < 4096,
            "Tool credential exceeds limit"
        );
        #[cfg(windows)]
        super::private_secret_file::validate(&file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                file.metadata()?.permissions().mode() & 0o077 == 0,
                "Tool credential file must be private"
            );
        }
        use std::io::Read;
        let mut raw = String::new();
        file.take(4096).read_to_string(&mut raw)?;
        Some(raw.trim().to_owned())
    } else {
        None
    };
    let environment = [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "LANG",
        "LC_ALL",
        "TEMP",
        "TMP",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
    .collect();
    let mut toolbox=cowork_toolbox::Toolbox::open(cowork_toolbox::Config{root:root.clone(),programs:settings.programs,environment,github_token}).map_err(|error|match error {cowork_toolbox::Error::RootInUse=>anyhow::anyhow!("This application data directory is already in use; close the other Cowork instance before starting another one"),_=>anyhow::anyhow!("Tool configuration is invalid or unavailable")})?;
    super::document_workspace::configure(data, &root, web, &mut toolbox)?;
    // Tool setup keeps the Tectonic cache and its warm marker in the private tool runtime.
    // Images bake a read-only runtime outside the data volume and name it explicitly.
    let runtime = match std::env::var_os("COWORK_DOCUMENT_RUNTIME_DIR").map(PathBuf::from) {
        Some(path) => {
            anyhow::ensure!(
                path.is_absolute(),
                "COWORK_DOCUMENT_RUNTIME_DIR must be absolute"
            );
            path
        }
        None => std::path::absolute(data)?.join("tool-runtime"),
    };
    toolbox
        .configure_document_runtime(runtime)
        .map_err(|_| anyhow::anyhow!("Document runtime directory is invalid"))?;
    Ok(toolbox)
}

#[cfg(test)]
mod root_owner_tests {
    use super::*;
    #[test]
    fn host_trust_toolbox_keeps_the_live_application_root_untouched() {
        let scratch = tempfile::tempdir().unwrap();
        let data = scratch.path().join("data");
        let _data_guard = crate::authority_token_file::private_directory(&data, None).unwrap();
        let application = open(&data, None).unwrap();
        let stage = data.join("toolbox/document-stages/7b3f53de-4c54-44c9-8b08-1dc61229e1e7");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("sentinel"), b"original active document").unwrap();
        let _operator_guard =
            crate::authority_token_file::private_directory(&host_trust_directory(&data), None)
                .unwrap();
        let operator = open_host_trust(&data, None).unwrap();
        assert_eq!(
            std::fs::read(stage.join("sentinel")).unwrap(),
            b"original active document"
        );
        assert!(
            host_trust_directory(&data)
                .join(".toolbox-owner.lock")
                .is_file()
        );
        assert!(data.join("toolbox/.toolbox-owner.lock").is_file());
        assert!(
            open(&data, None)
                .err()
                .unwrap()
                .to_string()
                .contains("already in use")
        );
        assert!(
            open_host_trust(&data, None)
                .err()
                .unwrap()
                .to_string()
                .contains("already in use")
        );
        drop(operator);
        let next_operator = open_host_trust(&data, None).unwrap();
        assert_eq!(
            std::fs::read(stage.join("sentinel")).unwrap(),
            b"original active document"
        );
        drop(next_operator);
        drop(application);
    }
}
