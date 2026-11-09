//! Private immutable Docker-host inventory. Node UIDs come from the operator's
//! copied lifecycle map, never from a container-supplied hostname or label.
use super::{ContainerConfig, invalid, regular};
#[cfg(windows)]
pub(super) use crate::private_secret_file;
use cowork_domain::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
use uuid::Uuid;
#[cfg(windows)]
pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    private_secret_file::create_private_directory(path)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct HostBinding {
    pub format: u32,
    pub host_id: Uuid,
    pub engine_id: String,
    pub node_name: Option<String>,
    pub node_uid: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeMap {
    format: u32,
    deployment_id: String,
    nodes: Vec<Node>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    name: String,
    uid: Uuid,
    host_id: Uuid,
}
pub(super) fn private(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(cowork_domain::storage_error)?;
    if metadata.is_symlink() || metadata.is_dir() != directory || !directory && !metadata.is_file()
    {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || !directory && metadata.nlink() != 1
        {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .access_mode(0x0002_0081)
            .share_mode(3)
            .custom_flags(if directory { 0x0220_0000 } else { 0x0020_0000 })
            .open(path)
            .map_err(cowork_domain::storage_error)?;
        if directory {
            private_secret_file::validate_directory(&file).map_err(|_| invalid())?;
        } else {
            private_secret_file::validate(&file).map_err(|_| invalid())?;
        }
    }
    Ok(())
}
pub(super) fn read(path: &Path) -> Result<Vec<u8>> {
    regular(path)?;
    private(path, false)?;
    if fs::metadata(path)
        .map_err(cowork_domain::storage_error)?
        .len()
        > 16384
    {
        return Err(invalid());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let mut file = options.open(path).map_err(cowork_domain::storage_error)?;
    let before = file.metadata().map_err(cowork_domain::storage_error)?;
    if before.len() > 16384 || !before.is_file() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1
            || before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err(invalid());
        }
        private_secret_file::validate(&file).map_err(|_| invalid())?;
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(cowork_domain::storage_error)?;
    let after = file.metadata().map_err(cowork_domain::storage_error)?;
    if bytes.len() > 16384
        || bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(super) fn hold_namespace(root: &Path) -> Result<Vec<fs::File>> {
    let mut held = Vec::new();
    for parent in root.ancestors() {
        let m = fs::symlink_metadata(parent).map_err(cowork_domain::storage_error)?;
        if !m.is_dir() || m.is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            if m.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
            let file = fs::OpenOptions::new()
                .read(true)
                .access_mode(0x0002_0081)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(parent)
                .map_err(cowork_domain::storage_error)?;
            if parent == root {
                private_secret_file::validate_directory(&file).map_err(|_| invalid())?;
            }
            held.push(file);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            if (m.uid() != unsafe { libc::geteuid() } && m.uid() != 0)
                || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
            {
                return Err(invalid());
            }
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(parent)
                .map_err(cowork_domain::storage_error)?;
            held.push(file);
        }
    }
    Ok(held)
}

// Fully sync the private stage before atomic no-replace publication. An
// interrupted or unsupported publication retains its stage and fails closed.
pub(super) fn publish(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    if fs::read_dir(root)
        .map_err(cowork_domain::storage_error)?
        .count()
        >= 64
    {
        return Err(invalid());
    }
    let staging = root.join(format!("publish-{}.json", Uuid::new_v4()));
    #[cfg(windows)]
    let mut file =
        private_secret_file::create_private_file(&staging).map_err(cowork_domain::storage_error)?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&staging)
            .map_err(cowork_domain::storage_error)?
    };
    file.write_all(bytes)
        .map_err(cowork_domain::storage_error)?;
    file.sync_all().map_err(cowork_domain::storage_error)?;
    drop(file);
    let target = root.join(name);
    #[cfg(target_os = "linux")]
    let result = {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(staging.as_os_str().as_bytes()).map_err(|_| invalid())?;
        let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| invalid())?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    #[cfg(windows)]
    let result = {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let source: Vec<_> = staging.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let result: std::io::Result<()> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Atomic immutable host publication requires Linux or Windows",
    ));
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            regular(&staging)?;
            private(&staging, false)?;
            fs::remove_file(&staging).map_err(cowork_domain::storage_error)?;
        }
        Err(e) => return Err(cowork_domain::storage_error(e)),
    }
    #[cfg(unix)]
    fs::File::open(root)
        .map_err(cowork_domain::storage_error)?
        .sync_all()
        .map_err(cowork_domain::storage_error)?;
    Ok(())
}
pub(super) fn resolve(config: &ContainerConfig, engine_id: String) -> Result<HostBinding> {
    private(&config.control_directory, true)?;
    let _control_namespace = hold_namespace(&config.control_directory)?;
    if engine_id.is_empty()
        || engine_id.len() > 128
        || !engine_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(Error::UncertainEffect);
    }
    let (host_id, node_name, node_uid) = if let Some(name) = &config.node_name {
        if config.host_id.is_some() {
            return Err(invalid());
        }
        let path = config.node_bindings_file.as_ref().ok_or_else(invalid)?;
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(invalid());
        }
        let _node_namespace = hold_namespace(path.parent().ok_or_else(invalid)?)?;
        for parent in path.parent().ok_or_else(invalid)?.ancestors() {
            let m = fs::symlink_metadata(parent).map_err(cowork_domain::storage_error)?;
            if !m.is_dir() || m.is_symlink() {
                return Err(invalid());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return Err(invalid());
                }
            }
        }
        let map: NodeMap = serde_json::from_slice(&read(path)?).map_err(|_| invalid())?;
        if map.format != 1
            || map.deployment_id.len() != 32
            || !map.deployment_id.bytes().all(|b| b.is_ascii_hexdigit())
            || map.nodes.is_empty()
            || map.nodes.len() > 64
        {
            return Err(invalid());
        }
        let mut names = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut hosts = std::collections::BTreeSet::new();
        for node in &map.nodes {
            if node.uid.is_nil()
                || node.host_id.is_nil()
                || node.name.is_empty()
                || node.name.len() > 253
                || !node
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
                || !names.insert(node.name.clone())
                || !ids.insert(node.uid)
                || !hosts.insert(node.host_id)
            {
                return Err(invalid());
            }
        }
        let selected: Vec<_> = map.nodes.iter().filter(|node| node.name == *name).collect();
        if selected.len() != 1 || selected[0].uid.is_nil() || selected[0].host_id.is_nil() {
            return Err(invalid());
        }
        let node = selected[0];
        (node.host_id, Some(name.clone()), Some(node.uid))
    } else {
        if config.node_bindings_file.is_some() {
            return Err(invalid());
        }
        (
            config
                .host_id
                .filter(|id| !id.is_nil())
                .ok_or_else(invalid)?,
            None,
            None,
        )
    };
    let host = HostBinding {
        format: 1,
        host_id,
        engine_id,
        node_name,
        node_uid,
    };
    let root = config.control_directory.join("hosts");
    if !root.exists() {
        #[cfg(windows)]
        let result = private_secret_file::create_private_directory(&root);
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&root)
        };
        match result {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(cowork_domain::storage_error(e)),
        }
    }
    private(&root, true)?;
    let _host_namespace = hold_namespace(&root)?;
    let path = root.join(format!("host-{host_id}.json"));
    if path.exists() {
        let prior: HostBinding = serde_json::from_slice(&read(&path)?).map_err(|_| invalid())?;
        if prior != host {
            return Err(Error::UncertainEffect);
        }
        return Ok(host);
    }
    let bytes = serde_json::to_vec(&host).map_err(cowork_domain::storage_error)?;
    publish(&root, &format!("host-{host_id}.json"), &bytes)?;
    let prior: HostBinding = serde_json::from_slice(&read(&path)?).map_err(|_| invalid())?;
    if prior != host {
        return Err(Error::UncertainEffect);
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(path: &Path) -> ContainerConfig {
        let control = path.join("control");
        #[cfg(windows)]
        private_secret_file::create_private_directory(&control).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&control).unwrap();
        }
        ContainerConfig {
            image: format!("tools@sha256:{}", "a".repeat(64)),
            runtime: None,
            host_workspace_root: path.to_path_buf(),
            control_directory: control,
            network: "none".into(),
            host_id: Some(Uuid::from_u128(1)),
            node_name: None,
            node_bindings_file: None,
        }
    }
    #[test]
    fn immutable_host_inventory_refuses_changed_engine_and_preserves_original() {
        let directory = tempfile::tempdir().unwrap();
        let c = config(directory.path());
        let first = resolve(&c, "engine-one".into()).unwrap();
        let file = c
            .control_directory
            .join("hosts")
            .join(format!("host-{}.json", first.host_id));
        let bytes = fs::read(&file).unwrap();
        assert_eq!(resolve(&c, "engine-one".into()).unwrap(), first);
        assert!(resolve(&c, "engine-two".into()).is_err());
        assert_eq!(fs::read(file).unwrap(), bytes);
    }
    #[test]
    fn copied_node_map_requires_unique_name_uid_and_host() {
        let directory = tempfile::tempdir().unwrap();
        let mut c = config(directory.path());
        let path = c.control_directory.join("nodes.json");
        let raw = serde_json::json!({"format":1,"deployment_id":"a".repeat(32),"nodes":[{"name":"node-a","uid":Uuid::from_u128(2),"host_id":Uuid::from_u128(3)},
            {"name":"node-b","uid":Uuid::from_u128(4),"host_id":Uuid::from_u128(3)}]});
        #[cfg(unix)]
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        #[cfg(windows)]
        {
            let mut file = private_secret_file::create_private_file(&path).unwrap();
            file.write_all(&serde_json::to_vec(&raw).unwrap()).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        c.host_id = None;
        c.node_name = Some("node-a".into());
        c.node_bindings_file = Some(path);
        assert!(resolve(&c, "engine-one".into()).is_err());
        assert!(!c.control_directory.join("hosts").exists());
    }
}
