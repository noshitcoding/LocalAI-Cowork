use anyhow::{Context, Result, bail};
use cowork_contracts::ProviderProfile;
use cowork_model_accounting::{
    digest,
    registry::{QualificationBundle, RawWireMode, RegisteredModel},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Seek},
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[cfg(windows)]
#[path = "../../daemon/src/private_secret_file.rs"]
#[allow(dead_code)] // Shared native ACL module also exposes setup-only helpers.
mod private_secret_file;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub format: u32,
    pub backend: String,
    pub supervisor: String,
    pub server: String,
    pub gateway_sha256: String,
    pub runtime_files: BTreeMap<String, String>,
    pub model_sha256: String,
    pub model_id: String,
    pub context_tokens: u64,
    pub tokenizer_sha256: String,
    pub chat_template_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub format: u32,
    pub listen: SocketAddr,
    pub profile: ProviderProfile,
    pub bundle: QualificationBundle,
    pub deployment: Deployment,
    pub backend_directory: PathBuf,
    pub model_file: PathBuf,
    pub data_directory: PathBuf,
    pub bearer_file: PathBuf,
}
pub struct Loaded {
    pub config: Config,
    pub model: RegisteredModel,
    pub token: String,
    // Windows handles forbid replacement while the owned backend is alive.
    // Linux namespace/file metadata is checked again before dispatch.
    pub held_files: Vec<(PathBuf, File, std::fs::Metadata)>,
    pub _held_directories: Vec<File>,
}
pub fn ordinary(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("Absolute ordinary paths required")
    }
    for p in path.ancestors() {
        let m = std::fs::symlink_metadata(p)?;
        if m.file_type().is_symlink() {
            bail!("Linked paths refused")
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                bail!("Reparse paths refused")
            }
        }
    }
    Ok(())
}
pub fn open(path: &Path, private: bool) -> Result<File> {
    ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let m = file.metadata()?;
    if !m.is_file() {
        bail!("Ordinary file required")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.nlink() != 1
            || private && (m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0)
        {
            bail!("Private unlinked owned file required")
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) }
            == 0
            || unsafe { info.assume_init() }.nNumberOfLinks != 1
        {
            bail!("Unlinked file required")
        };
        if private {
            private_secret_file::validate(&file)?
        }
    }
    Ok(file)
}
pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open(path, true)?;
    if file.metadata()?.len() > limit {
        bail!("Private input exceeds limit")
    };
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("Private input exceeds limit")
    };
    Ok(bytes)
}
pub fn hash_file(mut file: &File) -> Result<String> {
    file.rewind()?;
    let mut h = Sha256::new();
    // Windows native entrypoint uses the normal 1 MiB stack. File hashing is
    // bounded on the heap so configuration inspection needs no stack override.
    let mut block = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        h.update(&block[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn canonical_digest(value: &impl Serialize) -> Result<String> {
    // Same sorted JSON-map contract as Domain and the standalone configurator.
    Ok(digest(&serde_json::to_value(value)?)?)
}
fn directory(path: &Path, private: bool) -> Result<File> {
    ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x0002_0080)
            .share_mode(3)
            .custom_flags(0x02200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        bail!("Ordinary directory required")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        if private && (m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0) {
            bail!("Private owned directory required")
        }
    }
    #[cfg(windows)]
    if private {
        private_secret_file::validate(&file)?;
    }
    Ok(file)
}
fn runtime_inventory(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(folder)? {
            let path = entry?.path();
            ordinary(&path)?;
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(root)?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                result.insert(name, path);
            }
            if result.len() + pending.len() > 4096 {
                bail!("Runtime inventory limit")
            }
        }
    }
    Ok(result)
}
impl Loaded {
    pub fn assert_original_files(&self) -> Result<()> {
        let inventory = runtime_inventory(&self.config.backend_directory)?;
        if inventory
            .keys()
            .ne(self.config.deployment.runtime_files.keys())
        {
            bail!("Pinned runtime namespace changed")
        }
        for (path, file, original) in &self.held_files {
            ordinary(path)?;
            let actual = file.metadata()?;
            let named = std::fs::metadata(path)?;
            if actual.len() != original.len()
                || actual.modified()? != original.modified()?
                || named.len() != original.len()
                || named.modified()? != original.modified()?
            {
                bail!("Pinned artifact changed")
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if actual.dev() != named.dev()
                    || actual.ino() != named.ino()
                    || actual.nlink() != 1
                    || actual.ctime() != original.ctime()
                    || actual.ctime_nsec() != original.ctime_nsec()
                {
                    bail!("Pinned artifact identity changed")
                }
            }
        }
        Ok(())
    }
}
pub fn load(path: &Path) -> Result<Loaded> {
    let config: Config = serde_json::from_slice(&read_private(path, 60 * 1024 * 1024)?)?;
    if config.format != 1
        || !config.listen.ip().is_loopback()
        || config.listen.port() == 0
        || config.deployment.format != 1
        || config.deployment.backend != "llama_cpp_token_ids_v1"
        || config.deployment.supervisor != "cowork-executor-owned-v1"
        || config.deployment.runtime_files.is_empty()
        || config.deployment.runtime_files.len() > 4096
        || !matches!(config.bundle.wire_mode, RawWireMode::OpenaiCompletions)
        || !matches!(
            config.profile.kind,
            cowork_contracts::ProviderKind::OpenaiCompatible
        )
    {
        bail!("Unsupported pinned gateway configuration")
    }
    if config.profile.endpoint != format!("http://{}", config.listen)
        || !config.profile.credential_required
        || !config.profile.credential_ready
        || config.bundle.provider_sha256 != canonical_digest(&config.profile)?
        || config.bundle.evidence.deployment_sha256 != canonical_digest(&config.deployment)?
    {
        bail!("Exact profile/deployment binding mismatch")
    }
    let model = RegisteredModel::load(config.bundle.clone())?;
    let identity = model.identity();
    if identity.provider_id != config.profile.id
        || config.deployment.model_sha256 != identity.model_sha256
        || config.deployment.model_id != identity.model_id
        || config.profile.model != identity.model_id
        || config.deployment.context_tokens != identity.context_tokens
        || config.deployment.context_tokens != u64::from(config.profile.context_tokens)
        || config.deployment.tokenizer_sha256 != identity.tokenizer_sha256
        || config.deployment.chat_template_sha256 != identity.chat_template_sha256
    {
        bail!("Pinned model/template mismatch")
    }
    let mut held_directories = vec![
        directory(&config.backend_directory, true)?,
        directory(&config.data_directory, true)?,
    ];
    // Pin the namespace ancestors too; an ordinary directory may not be exchanged
    // for a reparse point while Windows resolves the owned executable/model.
    let mut ancestors = std::collections::BTreeSet::new();
    for path in [
        &config.backend_directory,
        &config.data_directory,
        &config.model_file,
    ] {
        for parent in path.parent().into_iter().flat_map(Path::ancestors) {
            if ancestors.insert(parent.to_path_buf()) {
                held_directories.push(directory(parent, false)?);
            }
        }
    }
    if runtime_inventory(&config.backend_directory)?
        .keys()
        .ne(config.deployment.runtime_files.keys())
    {
        bail!("Unpinned backend file present")
    }
    let mut held_files = vec![];
    let gateway_file = std::env::current_exe()?;
    let file = open(&gateway_file, false)?;
    if hash_file(&file)? != config.deployment.gateway_sha256 {
        bail!("Original gateway executable changed")
    }
    let original = file.metadata()?;
    held_files.push((gateway_file, file, original));
    for (name, expected) in &config.deployment.runtime_files {
        let relative = Path::new(name);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || name.contains(':')
            || name.is_empty()
        {
            bail!("Invalid pinned runtime path")
        };
        let file = open(&config.backend_directory.join(relative), false)?;
        if hash_file(&file)? != *expected {
            bail!("Pinned backend artifact changed")
        };
        let original = file.metadata()?;
        held_files.push((config.backend_directory.join(relative), file, original));
    }
    if !config
        .deployment
        .runtime_files
        .contains_key(&config.deployment.server)
    {
        bail!("Exact backend binary missing")
    }
    let file = open(&config.model_file, false)?;
    if hash_file(&file)? != config.deployment.model_sha256 {
        bail!("Pinned GGUF changed")
    };
    let original = file.metadata()?;
    held_files.push((config.model_file.clone(), file, original));
    let token = String::from_utf8(read_private(&config.bearer_file, 256)?)?;
    let token = token.trim().to_string();
    if token.len() < 32 || token.len() > 128 || token.chars().any(char::is_control) {
        bail!("Invalid private gateway bearer")
    }
    Ok(Loaded {
        config,
        model,
        token,
        held_files,
        _held_directories: held_directories,
    })
}
pub fn create_private(path: &Path, bytes: &[u8]) -> Result<()> {
    ordinary(path.parent().context("Parent required")?)?;
    #[cfg(windows)]
    let mut file = private_secret_file::create_private_file(path)?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
    };
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn ownership_lock(path: &Path) -> Result<File> {
    if !path.exists() {
        create_private(path, b"owned model gateway\n")?
    }
    let file = open(path, true)?;
    file.try_lock()
        .context("Another model gateway owns this state directory")?;
    Ok(file)
}
