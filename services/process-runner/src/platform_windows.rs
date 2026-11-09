//! Fresh LPAC capsules on fixed VHDs: hard writable-space limits, private desktop,
//! restricted token and Job containment. Unsupported privileges fail closed.
use super::*;
use cowork_domain::{canonical_hash, environments::EnvironmentArtifact};
use cowork_executor::windows_isolation::{Capsule, Limits};

fn bundle<'a>(config: &'a Configuration, definition: &EnvironmentDefinition) -> Result<&'a Path> {
    let hash = match &definition.artifact {
        EnvironmentArtifact::WindowsBundle { sha256 } => sha256,
        _ => anyhow::bail!("This receiver installs Windows bundles only"),
    };
    config
        .windows_bundles
        .get(hash)
        .map(PathBuf::as_path)
        .context("Bundle digest has not been privately installed")
}
fn manifest_hash(root: &Path) -> Result<String> {
    Ok(canonical_hash(&workspace::files(root)?))
}
fn diskpart() -> Result<PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut value = vec![0u16; 32768];
    let n = unsafe { GetSystemDirectoryW(value.as_mut_ptr(), value.len() as u32) } as usize;
    ensure!(
        n > 0 && n < value.len(),
        "Windows system directory unavailable"
    );
    Ok(PathBuf::from(String::from_utf16(&value[..n])?).join("diskpart.exe"))
}
fn disk(config: &Configuration, attempt: &Path, script: &str) -> Result<()> {
    let file = attempt.join(format!("diskpart-{}.txt", uuid::Uuid::new_v4()));
    crate::config::write_new(&file, script.as_bytes())?;
    command(
        &diskpart()?,
        &config.data_directory,
        vec!["/s".into(), file.to_string_lossy().into_owned()],
    )?;
    Ok(())
}
fn safe_path(path: &Path) -> Result<String> {
    crate::config::ordinary(path.parent().context("Missing VHD parent")?, true)?;
    let value = path.to_string_lossy().into_owned();
    ensure!(
        !value.contains(['"', '\r', '\n', '\0']),
        "Invalid private disk path"
    );
    Ok(value)
}
pub fn inspect(config: &Configuration, definition: &EnvironmentDefinition) -> Result<()> {
    let root = bundle(config, definition)?;
    crate::config::ordinary(root, true)?;
    ensure!(
        manifest_hash(root)? == definition.artifact.sha256(),
        "Windows bundle bytes do not match installed digest"
    );
    ensure!(
        definition.limits.processes <= 64
            && definition.limits.memory_bytes <= 64 * 1024 * 1024 * 1024,
        "Windows Job limits cannot implement these bounds"
    );
    ensure!(
        definition.limits.scratch_bytes >= 64 * 1024 * 1024
            && definition.limits.scratch_bytes.is_multiple_of(1024 * 1024),
        "Fixed NTFS quota requires an integral MiB size of at least 64 MiB"
    );
    for path in definition.programs.values() {
        ensure!(
            !path.contains('/')
                && path.to_ascii_lowercase().ends_with(".exe")
                && root.join(path).is_file(),
            "Windows programs must be existing top-level EXEs in the approved bundle"
        );
    }
    crate::config::ordinary(&diskpart()?, false)?;
    Ok(())
}
pub fn recover(config: &Configuration, record: &Ownership) -> Result<()> {
    ensure!(
        record.adapter == "windows_lpac_vhd"
            && record.owned_directory.starts_with(&config.data_directory),
        "Foreign adapter journal"
    );
    let mount = record.owned_directory.join("mount");
    let vhd = record.owned_directory.join(format!("{}.vhd", record.nonce));
    let mut namespace_verified = false;
    match mount.symlink_metadata() {
        Ok(metadata) => {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.is_dir(),
                "Original mount changed to a non-directory object"
            );
            if metadata.file_attributes() & 0x400 == 0 {
                // Before attach/after detach this is the private empty host mount
                // point, never an alternate place to discover/adopt capsules.
                ensure!(
                    std::fs::read_dir(&mount)?.next().is_none(),
                    "Unmounted original mount contains unknown data"
                );
            } else {
                crate::config::ordinary(&vhd, false)?;
                validate_mount(&vhd, &mount)?;
                ensure!(
                    record.mount_volume.as_deref() == Some(mount_volume(&mount)?.as_str()),
                    "Original confirmed VHD volume differs before capsule recovery"
                );
                for child in std::fs::read_dir(&mount)? {
                    let child = child?;
                    if child.file_name().to_string_lossy().starts_with("capsule-") {
                        if !child.path().join("owned").exists() {
                            crate::windows_orphan::confirmed(config, &child.path())?;
                        }
                        cowork_executor::windows_isolation::recover(&child.path())?;
                    }
                }
                namespace_verified = true;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    match vhd.symlink_metadata() {
        Ok(_) => detach_owned_disk(
            &vhd,
            &mount,
            record.mount_volume.as_deref(),
            namespace_verified,
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
// DiskPart's exit status alone does not prove detach. Ask the VHD provider for
// IS_LOADED and accept only an actual successful observation of false.
fn mount_volume(mount: &Path) -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
    let mut path: Vec<u16> = mount.as_os_str().encode_wide().collect();
    path.extend([b'\\' as u16, 0]);
    let mut volume = vec![0u16; 32768];
    ensure!(
        unsafe {
            GetVolumeNameForVolumeMountPointW(
                path.as_ptr(),
                volume.as_mut_ptr(),
                volume.len() as u32,
            )
        } != 0,
        "Owned mount volume observation failed"
    );
    let n = volume
        .iter()
        .position(|v| *v == 0)
        .context("Invalid owned volume name")?;
    Ok(String::from_utf16(&volume[..n])?)
}
fn validate_mount(vhd: &Path, mount: &Path) -> Result<()> {
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Storage::{FileSystem::*, Vhd::*},
        System::{IO::DeviceIoControl, Ioctl::VOLUME_DISK_EXTENTS},
    };
    let wide: Vec<u16> = vhd.as_os_str().encode_wide().chain(Some(0)).collect();
    let storage = VIRTUAL_STORAGE_TYPE {
        DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHD,
        VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
    };
    let parameters = OPEN_VIRTUAL_DISK_PARAMETERS {
        Version: OPEN_VIRTUAL_DISK_VERSION_1,
        Anonymous: OPEN_VIRTUAL_DISK_PARAMETERS_0 {
            Version1: OPEN_VIRTUAL_DISK_PARAMETERS_0_0 { RWDepth: 1 },
        },
    };
    let mut handle = std::ptr::null_mut();
    let code = unsafe {
        OpenVirtualDisk(
            &storage,
            wide.as_ptr(),
            VIRTUAL_DISK_ACCESS_GET_INFO,
            OPEN_VIRTUAL_DISK_FLAG_NONE,
            &parameters,
            &mut handle,
        )
    };
    ensure!(
        code == ERROR_SUCCESS,
        "Owned VHD physical identity unavailable: {code}"
    );
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut physical = vec![0u16; 32768];
    let mut size = (physical.len() * 2) as u32;
    let code = unsafe {
        GetVirtualDiskPhysicalPath(handle.as_raw_handle(), &mut size, physical.as_mut_ptr())
    };
    ensure!(
        code == ERROR_SUCCESS,
        "Owned VHD device path unavailable: {code}"
    );
    let n = physical
        .iter()
        .position(|v| *v == 0)
        .context("Invalid device path")?;
    let disk = String::from_utf16(&physical[..n])?;
    let disk: u32 = disk
        .strip_prefix("\\\\.\\PhysicalDrive")
        .context("Unexpected VHD physical device")?
        .parse()?;
    let volume = mount_volume(mount)?;
    let wide: Vec<u16> = volume
        .trim_end_matches('\\')
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        handle != INVALID_HANDLE_VALUE,
        "Owned mount handle unavailable"
    );
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut extents = VOLUME_DISK_EXTENTS::default();
    let mut returned = 0;
    // CTL_CODE(IOCTL_VOLUME_BASE,0,METHOD_BUFFERED,FILE_ANY_ACCESS).
    ensure!(
        unsafe {
            DeviceIoControl(
                handle.as_raw_handle(),
                0x00560000,
                std::ptr::null(),
                0,
                (&mut extents as *mut VOLUME_DISK_EXTENTS).cast(),
                std::mem::size_of::<VOLUME_DISK_EXTENTS>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        } != 0
            && extents.NumberOfDiskExtents == 1
            && extents.Extents[0].DiskNumber == disk,
        "Mount is not backed by this exact owned VHD"
    );
    Ok(())
}
fn detach_owned_disk(
    vhd: &Path,
    mount: &Path,
    expected_volume: Option<&str>,
    namespace_verified: bool,
) -> Result<()> {
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Storage::{FileSystem::DeleteVolumeMountPointW, Vhd::*},
    };
    crate::config::ordinary(vhd, false)?;
    let wide: Vec<u16> = vhd.as_os_str().encode_wide().chain(Some(0)).collect();
    let storage = VIRTUAL_STORAGE_TYPE {
        DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHD,
        VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
    };
    let parameters = OPEN_VIRTUAL_DISK_PARAMETERS {
        Version: OPEN_VIRTUAL_DISK_VERSION_1,
        Anonymous: OPEN_VIRTUAL_DISK_PARAMETERS_0 {
            Version1: OPEN_VIRTUAL_DISK_PARAMETERS_0_0 { RWDepth: 1 },
        },
    };
    let mut handle = std::ptr::null_mut();
    let code = unsafe {
        OpenVirtualDisk(
            &storage,
            wide.as_ptr(),
            VIRTUAL_DISK_ACCESS_GET_INFO | VIRTUAL_DISK_ACCESS_DETACH,
            OPEN_VIRTUAL_DISK_FLAG_NONE,
            &parameters,
            &mut handle,
        )
    };
    ensure!(
        code == ERROR_SUCCESS,
        "Owned VHD observation failed: {code}"
    );
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let loaded = || -> Result<bool> {
        let mut info = GET_VIRTUAL_DISK_INFO {
            Version: GET_VIRTUAL_DISK_INFO_IS_LOADED,
            ..Default::default()
        };
        let mut size = std::mem::size_of::<GET_VIRTUAL_DISK_INFO>() as u32;
        let code = unsafe {
            GetVirtualDiskInformation(
                handle.as_raw_handle(),
                &mut size,
                &mut info,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            code == ERROR_SUCCESS,
            "VHD loaded-state observation failed: {code}"
        );
        Ok(unsafe { info.Anonymous.IsLoaded } != 0)
    };
    let mounted = loaded()?;
    ensure!(
        !mounted || namespace_verified,
        "Loaded original VHD lacks a positively inspected capsule namespace"
    );
    match mount.symlink_metadata() {
        Ok(meta) => {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                meta.is_dir(),
                "Original mount changed to a non-directory object before detach"
            );
            if meta.file_attributes() & 0x400 != 0 {
                if mounted {
                    validate_mount(vhd, mount)?;
                }
                ensure!(
                    mounted || expected_volume == Some(mount_volume(mount)?.as_str()),
                    "Owned mount volume identity changed or is unrecorded"
                );
                if let Some(expected) = expected_volume {
                    ensure!(
                        expected == mount_volume(mount)?,
                        "Owned mount volume identity changed"
                    );
                }
                let mut path: Vec<u16> = mount.as_os_str().encode_wide().collect();
                path.extend([b'\\' as u16, 0]);
                ensure!(
                    unsafe { DeleteVolumeMountPointW(path.as_ptr()) } != 0,
                    "Owned VHD mount-point removal failed"
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                !mounted,
                "Loaded original VHD mount disappeared before detach"
            );
        }
        Err(e) => return Err(e.into()),
    }
    if mounted {
        let code =
            unsafe { DetachVirtualDisk(handle.as_raw_handle(), DETACH_VIRTUAL_DISK_FLAG_NONE, 0) };
        ensure!(code == ERROR_SUCCESS, "Owned VHD detach failed: {code}");
    }
    ensure!(!loaded()?, "Owned VHD is still physically loaded");
    Ok(())
}
pub fn release(config: &Configuration, record: &Ownership) -> Result<()> {
    recover(config, record)?;
    let vhd = record.owned_directory.join(format!("{}.vhd", record.nonce));
    if vhd.exists() {
        crate::config::ordinary(&vhd, false)?;
        std::fs::remove_file(vhd)?;
        crate::config::sync_directory(&record.owned_directory)?;
    }
    Ok(())
}

pub fn inspect_orphan(
    config: &Configuration,
    record: &Ownership,
    root: &Path,
) -> Result<cowork_executor::windows_isolation::OrphanInspection> {
    ensure!(
        record.adapter == "windows_lpac_vhd"
            && record.owned_directory.parent()
                == Some(config.data_directory.join("attempts").as_path()),
        "Orphan is outside the original receiver attempt namespace"
    );
    let mount = record.owned_directory.join("mount");
    let vhd = record.owned_directory.join(format!("{}.vhd", record.nonce));
    let volume = record
        .mount_volume
        .as_deref()
        .context("Original VHD mount was never confirmed")?;
    crate::config::ordinary(&vhd, false)?;
    validate_mount(&vhd, &mount)?;
    ensure!(
        mount_volume(&mount)? == volume
            && root.canonicalize()?.parent() == Some(mount.canonicalize()?.as_path()),
        "Original capsule is outside this exact recorded VHD volume"
    );
    Ok(cowork_executor::windows_isolation::OrphanInspection::open(
        root,
    )?)
}

pub fn validate_orphan_mount(
    config: &Configuration,
    record: &Ownership,
    root: &Path,
) -> Result<()> {
    ensure!(
        record.adapter == "windows_lpac_vhd"
            && record.owned_directory.parent()
                == Some(config.data_directory.join("attempts").as_path()),
        "Foreign orphan attempt"
    );
    let mount = record.owned_directory.join("mount");
    let vhd = record.owned_directory.join(format!("{}.vhd", record.nonce));
    crate::config::ordinary(&vhd, false)?;
    validate_mount(&vhd, &mount)?;
    ensure!(
        record.mount_volume.as_deref() == Some(mount_volume(&mount)?.as_str())
            && root.canonicalize()?.parent() == Some(mount.canonicalize()?.as_path()),
        "Original orphan mount changed"
    );
    Ok(())
}
pub fn run(
    config: &Configuration,
    definition: &EnvironmentDefinition,
    invocation: &ProcessInvocation,
    input: &Path,
    attempt: &Path,
    cancel: &AtomicBool,
    probe: bool,
) -> Result<(Outcome, PathBuf)> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let vhd = attempt.join(format!("{nonce}.vhd"));
    let mount = attempt.join("mount");
    crate::config::private_directory(&mount)?;
    let mut record = Ownership {
        nonce,
        adapter: "windows_lpac_vhd".into(),
        physical_id: None,
        mount_volume: None,
        artifact: definition.artifact.sha256().into(),
        definition_sha256: canonical_hash(definition),
        owned_directory: attempt.to_path_buf(),
        agent_sandbox: None,
    };
    crate::config::durable_json(&attempt.join("ownership.json"), &record)?;
    let vhd_path = safe_path(&vhd)?;
    let mount_path = safe_path(&mount)?;
    disk(
        config,
        attempt,
        &format!(
            "create vdisk file=\"{vhd_path}\" maximum={} type=fixed\r\nselect vdisk file=\"{vhd_path}\"\r\nattach vdisk\r\ncreate partition primary\r\nformat fs=ntfs quick\r\nassign mount=\"{mount_path}\"\r\n",
            definition.limits.scratch_bytes / (1024 * 1024)
        ),
    )?;
    validate_mount(&vhd, &mount)?;
    record.mount_volume = Some(mount_volume(&mount)?);
    crate::config::durable_json(&attempt.join("ownership.json"), &record)?;
    let execution = (|| -> Result<(Outcome, PathBuf)> {
        // The backing file's fixed virtual capacity bounds all capsule bundle/work
        // writes. DiskPart failure or unexpected capacity cannot be softened.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                path: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = mount.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut available, mut total, mut free) = (0, 0, 0);
        ensure!(
            unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut available, &mut total, &mut free) } != 0
                && total > 0
                && total <= definition.limits.scratch_bytes,
            "Hard capsule disk quota could not be verified"
        );
        let mut capsule = Capsule::create(
            &mount,
            Limits {
                memory_bytes: usize::try_from(definition.limits.memory_bytes)?,
                processes: definition.limits.processes,
                cpu_millis: definition.limits.cpu_millis,
            },
        )?;
        let root = bundle(config, definition)?;
        let manifest = workspace::files(root)?;
        ensure!(
            canonical_hash(&manifest) == definition.artifact.sha256(),
            "Bundle changed during staging"
        );
        for (name, entry) in &manifest {
            if entry.directory {
                capsule.stage_directory(name, true)?;
                continue;
            }
            let bytes = std::fs::read(root.join(name))?;
            ensure!(
                workspace::digest(&bytes) == entry.sha256,
                "Bundle changed during staging"
            );
            if definition.programs.values().any(|p| p == name) {
                capsule.stage_program(name, &bytes, &entry.sha256)?;
            } else {
                capsule.stage_bundle_file(name, &bytes, &entry.sha256)?;
            }
        }
        for (name, entry) in workspace::files(input)? {
            if entry.directory {
                capsule.stage_directory(&name, false)?;
                continue;
            }
            let bytes = std::fs::read(input.join(&name))?;
            ensure!(
                workspace::digest(&bytes) == entry.sha256,
                "Input changed during staging"
            );
            capsule.stage_input(&name, &bytes)?;
        }
        let program = if probe {
            let bytes = std::fs::read(&config.probe_executable)?;
            capsule.stage_program(
                "cowork-runner-probe.exe",
                &bytes,
                &workspace::digest(&bytes),
            )?;
            "cowork-runner-probe.exe".into()
        } else {
            definition
                .programs
                .get(&invocation.program)
                .context("Program alias is not installed")?
                .clone()
        };
        let outcome = capsule.run(
            &program,
            invocation.arguments.clone(),
            Duration::from_secs(invocation.timeout_seconds),
            cancel,
        )?;
        let closed = capsule.close()?;
        let output = attempt.join("output");
        let manifest = workspace::files_capsule(&closed.join("work"))?;
        workspace::copy_capsule(&closed.join("work"), &output, &manifest)?;
        Ok((outcome, output))
    })();
    // Job supervision and identity cleanup precede disk detach and any receipt.
    // If capsule cleanup fails, recovery refuses to detach a live named Job.
    let recovered = recover(config, &record);
    let result = execution?;
    recovered?;
    Ok(result)
}
