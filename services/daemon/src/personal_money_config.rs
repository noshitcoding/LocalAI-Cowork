//! A private installation-owned file may activate durable personal budgets.
//! Browser, model and tool payloads never select this configuration authority.
use cowork_storage::model_money::PersonalMoneyConfiguration;
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

fn ordinary_path(path: &Path) -> anyhow::Result<()> {
    for component in path.ancestors() {
        let metadata = std::fs::symlink_metadata(component)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "Personal budget configuration cannot traverse a symlink"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            anyhow::ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Personal budget configuration cannot traverse a reparse point"
            );
        }
    }
    Ok(())
}

fn open_private(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep the actual file immutable while its operator bytes are read.
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= MAX_CONFIG_BYTES,
        "Personal budget configuration must be a bounded regular file"
    );
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        anyhow::ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Personal budget configuration handle is a reparse point"
        );
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        anyhow::ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
                && info.nNumberOfLinks == 1,
            "Personal budget configuration must not have hardlink aliases"
        );
        super::private_secret_file::validate(&file).map_err(|_| {
            anyhow::anyhow!(
                "Personal budget configuration must be owned by the current user with a private ACL"
            )
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1,
            "Personal budget configuration must be private, current-user-owned and without hardlinks"
        );
        let observed = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            observed.dev() == metadata.dev() && observed.ino() == metadata.ino(),
            "Personal budget configuration path changed while opening"
        );
    }
    Ok(file)
}

pub(super) fn read(
    path: &Path,
    data_directory: &Path,
    web_directory: Option<&Path>,
) -> anyhow::Result<PersonalMoneyConfiguration> {
    anyhow::ensure!(
        path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir)),
        "Personal budget configuration requires an absolute path without traversal"
    );
    ordinary_path(path)?;
    let data = data_directory.canonicalize()?;
    let canonical = path.canonicalize()?;
    anyhow::ensure!(
        canonical.parent() == Some(data.as_path()),
        "Personal budget configuration must be directly inside the protected daemon data directory"
    );
    if let Some(web) = web_directory {
        anyhow::ensure!(
            !canonical.starts_with(web.canonicalize()?),
            "Personal budget configuration must be outside the served web directory"
        );
    }
    let mut file = open_private(&canonical)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_CONFIG_BYTES && bytes.len() as u64 == file.metadata()?.len(),
        "Personal budget configuration changed or exceeded its size bound"
    );
    // Do not echo malformed configuration values into terminal or daemon logs.
    serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("Personal budget configuration JSON is invalid or unsupported")
    })
}

pub(super) fn configured_path() -> anyhow::Result<Option<PathBuf>> {
    match std::env::var("COWORK_PERSONAL_MONEY_CONFIG_FILE") {
        Ok(path) if !path.trim().is_empty() => Ok(Some(path.into())),
        Ok(_) => anyhow::bail!("Personal budget configuration path must not be empty"),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => anyhow::bail!("Personal budget configuration path is not Unicode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn configuration() -> serde_json::Value {
        json!({
            "format_version":1,
            "installation":{"scope":{"kind":"installation","id":"owned-fixture"},"version":1,
                            "currency":{"code":"USD","scale":6},"limit_units":1000},
            "project_limit_units":500,"run_limit_units":100,"pool_policy_version":1,
            "deployments":[{"deployment":{"id":"owned-model", "provider":{
                "id":"owned-provider","name":"Fixture","kind":"ollama",
                "endpoint":"http://127.0.0.1:19001","model":"fixture","revision":1,
                "context_tokens":32768,"credential_required":false,"credential_ready":true},
                "model_sha256":"a".repeat(64),"price_id":"owned-price","price_version":1,
                "token_mode":"conservative_context_upper_bound"},
                "price":{"id":"owned-price","version":1,"provider_id":"owned-provider",
                    "model_sha256":"a".repeat(64),"currency":{"code":"USD","scale":6},
                    "input_units_per_million":1,"output_units_per_million":1,
                    "cached_input_units_per_million":null,"source_sha256":"b".repeat(64)}}]
        })
    }

    #[cfg(unix)]
    fn permissions(path: &Path, public: bool) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if public { 0o644 } else { 0o600 }),
        )
        .unwrap();
    }

    #[cfg(windows)]
    fn permissions(path: &Path, public: bool) {
        use std::{
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
            ptr,
        };
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{Authorization::*, *},
        };
        let file = OpenOptions::new()
            .read(true)
            .access_mode(0x20000 | 0x40000)
            .open(path)
            .unwrap();
        let mut descriptor = ptr::null_mut();
        let mut owner = ptr::null_mut();
        assert_eq!(
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    &mut owner,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let mut sid = ptr::null_mut();
        assert_ne!(unsafe { ConvertSidToStringSidW(owner, &mut sid) }, 0);
        let mut length = 0;
        while unsafe { *sid.add(length) } != 0 {
            length += 1;
            assert!(length < 256);
        }
        let trustee =
            String::from_utf16(unsafe { std::slice::from_raw_parts(sid, length) }).unwrap();
        unsafe {
            LocalFree(sid.cast());
            LocalFree(descriptor);
        }
        let sddl = format!(
            "D:P(A;;FA;;;{trustee}){}",
            if public { "(A;;FR;;;WD)" } else { "" }
        );
        let sddl: Vec<_> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut acl_descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut acl_descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(acl_descriptor, &mut present, &mut dacl, &mut defaulted)
            },
            0
        );
        let result = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        unsafe {
            LocalFree(acl_descriptor);
        }
        assert_eq!(result, 0);
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("personal-money.json");
        std::fs::write(&path, serde_json::to_vec(&configuration()).unwrap()).unwrap();
        permissions(&path, false);
        (directory, path)
    }

    #[test]
    fn private_configuration_reads_without_changing_its_bytes() {
        let (data, path) = fixture();
        let before = std::fs::read(&path).unwrap();
        let parsed = read(&path, data.path(), None).unwrap();
        assert_eq!(parsed.format_version, 1);
        assert_eq!(parsed.run_limit_units, 100);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn browser_served_and_external_configuration_paths_are_refused() {
        let (data, path) = fixture();
        let unrelated = tempfile::tempdir().unwrap();
        assert!(read(&path, unrelated.path(), None).is_err());
        assert!(read(&path, data.path(), Some(data.path())).is_err());
        assert!(read(Path::new("personal-money.json"), data.path(), None).is_err());
        let traversed = data.path().join("child/../personal-money.json");
        assert!(read(&traversed, data.path(), None).is_err());
    }

    #[test]
    fn public_configuration_and_hardlink_alias_are_refused() {
        let (data, path) = fixture();
        permissions(&path, true);
        assert!(read(&path, data.path(), None).is_err());
        permissions(&path, false);
        std::fs::hard_link(&path, data.path().join("alias.json")).unwrap();
        assert!(read(&path, data.path(), None).is_err());
    }

    #[test]
    fn empty_and_oversized_configuration_files_are_refused() {
        let (data, path) = fixture();
        std::fs::write(&path, b"").unwrap();
        assert!(read(&path, data.path(), None).is_err());
        std::fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert!(read(&path, data.path(), None).is_err());
    }

    #[test]
    fn unknown_fields_in_operator_config_and_provider_are_refused() {
        let (data, path) = fixture();
        let mut config = configuration();
        config["client_money_authority"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(read(&path, data.path(), None).is_err());
        config = configuration();
        config["deployments"][0]["deployment"]["provider"]["client_money_authority"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(read(&path, data.path(), None).is_err());
    }

    #[test]
    fn duplicate_fields_and_invalid_values_do_not_echo_input() {
        let (data, path) = fixture();
        let text = serde_json::to_string(&configuration()).unwrap();
        let duplicate = text.replacen(
            "\"format_version\":1",
            "\"format_version\":1,\"format_version\":1",
            1,
        );
        std::fs::write(&path, duplicate).unwrap();
        assert!(read(&path, data.path(), None).is_err());
        std::fs::write(&path, b"{\"format_version\":\"private-fixture-value\"}").unwrap();
        let error = read(&path, data.path(), None).err().unwrap().to_string();
        assert!(!error.contains("private-fixture-value"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_files_and_parent_namespaces_are_refused() {
        let (data, path) = fixture();
        let aliases = tempfile::tempdir().unwrap();
        let link = data.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read(&link, data.path(), None).is_err());
        let parent = aliases.path().join("linked-data");
        std::os::unix::fs::symlink(data.path(), &parent).unwrap();
        assert!(read(&parent.join("personal-money.json"), data.path(), None).is_err());
    }
}
