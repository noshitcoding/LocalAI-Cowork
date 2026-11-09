//! Inspection and explicit bounded staging for the runtime package pipeline.
use cap_std::{ambient_authority, fs::Dir};
use cowork_managed_runtime::{Platform, TrustRoot, verify_directory};
use std::{io::Read, path::Path};
#[cfg(target_os = "linux")]
#[path = "cowork-runtime-package/publication.rs"]
mod publication;
#[cfg(target_os = "linux")]
#[path = "cowork-runtime-package/release_floor.rs"]
mod release_floor;

fn inspect() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--publication-status")
    {
        #[cfg(target_os = "linux")]
        {
            let receipt = publication::history(&args)?;
            println!("{receipt}");
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        return Err("publication history requires the Linux management runtime".into());
    }
    if args
        .first()
        .is_some_and(|v| v == "--initialize-release-floor" || v == "--release-floor-status")
    {
        #[cfg(target_os = "linux")]
        {
            println!("{}", release_floor::management(&args)?);
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        return Err("release policy requires the Linux management runtime".into());
    }
    if ![4, 7, 8, 12, 13, 14, 15].contains(&args.len()) {
        return Err("usage: cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM [--stage ABS_PRIVATE_PARENT ATTEMPT_NAME MAX_PAYLOAD_BYTES [--inspect-archive ROLE MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES | --extract-archive ABS_PRIVATE_TARGET NAME ROLE MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES | --inspect-installation MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES | --prepare-installation ABS_PRIVATE_TARGET NAME MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES]]".into());
    }
    let package_path = Path::new(&args[0]);
    let key_path = Path::new(&args[1]);
    if !package_path.is_absolute() || !key_path.is_absolute() {
        return Err("absolute package and independently configured trust key required".into());
    }
    let package_path = package_path.canonicalize()?;
    let key_path = key_path.canonicalize()?;
    if key_path.starts_with(&package_path) {
        return Err("trust key cannot come from the package".into());
    }
    let mut public_key = Vec::new();
    std::fs::File::open(key_path)?
        .take(33)
        .read_to_end(&mut public_key)?;
    let trust = TrustRoot::new(
        args[2].to_str().ok_or("invalid key ID")?.into(),
        public_key
            .try_into()
            .map_err(|_| "expected raw 32-byte Ed25519 public key")?,
    )?;
    let platform = match args[3].to_str() {
        Some("linux_x86_64") => Platform::LinuxX86_64,
        Some("windows_wsl2_x86_64") => Platform::WindowsWsl2X86_64,
        _ => return Err("unsupported runtime package platform".into()),
    };
    let source = Dir::open_ambient_dir(&package_path, ambient_authority())?;
    if args
        .get(4)
        .is_some_and(|value| value == "--publish-installation")
    {
        #[cfg(target_os = "linux")]
        {
            let receipt = publication::run(&args, &source, &trust, platform, &package_path)?;
            println!("{receipt}");
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        return Err("package publication requires the Linux management runtime".into());
    }
    if args
        .get(4)
        .is_some_and(|v| v == "--check-release-floor" || v == "--advance-release-floor")
    {
        #[cfg(target_os = "linux")]
        {
            println!(
                "{}",
                release_floor::candidate(&args, &source, &trust, platform, &package_path)?
            );
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        return Err("release policy requires the Linux management runtime".into());
    }
    if args.len() == 7 {
        return Err("explicit release policy operation required".into());
    }
    let receipt = if args.len() >= 8 {
        #[cfg(any(target_os = "linux", windows))]
        {
            if args[4] != "--stage" || !Path::new(&args[5]).is_absolute() {
                return Err("explicit staging option and absolute private parent required".into());
            }
            let parent = Path::new(&args[5]).canonicalize()?;
            if parent.starts_with(&package_path) {
                return Err("staging parent must be independent of the package".into());
            }
            let name = args[6].to_str().ok_or("invalid staging attempt name")?;
            let limit: u64 = args[7].to_str().ok_or("invalid staging budget")?.parse()?;
            let area = cowork_managed_runtime::staging::StagingArea::create(
                Dir::open_ambient_dir(parent, ambient_authority())?,
                name,
                limit,
            )?;
            let mut staged = area.stage(&source, &trust, platform)?;
            let mut receipt = serde_json::to_value(staged.receipt()?)?;
            if [12, 14].contains(&args.len()) {
                let index = match (args.len(), args[8].to_str()) {
                    (12, Some("--inspect-installation")) => 9,
                    (14, Some("--prepare-installation")) => 11,
                    _ => {
                        return Err("explicit whole-package installation operation required".into());
                    }
                };
                let entries = args[index]
                    .to_str()
                    .ok_or("invalid installation entry limit")?
                    .parse()?;
                let expanded = args[index + 1]
                    .to_str()
                    .ok_or("invalid installation byte limit")?
                    .parse()?;
                let paths = args[index + 2]
                    .to_str()
                    .ok_or("invalid installation path limit")?
                    .parse()?;
                let limits = cowork_managed_runtime::staging::archive::InstallationLimits::new(
                    entries, expanded, paths,
                )?;
                let mut inspected = staged.inspect_installation(limits)?;
                receipt["installation"] = serde_json::to_value(inspected.receipt()?)?;
                if args.len() == 14 {
                    #[cfg(target_os = "linux")]
                    {
                        if !Path::new(&args[9]).is_absolute() {
                            return Err("absolute private installation parent required".into());
                        }
                        let target = Path::new(&args[9]).canonicalize()?;
                        if target.starts_with(&package_path) {
                            return Err("installation parent must be independent of package".into());
                        }
                        let area = cowork_managed_runtime::staging::StagingArea::create(
                            Dir::open_ambient_dir(target, ambient_authority())?,
                            args[10]
                                .to_str()
                                .ok_or("invalid installation attempt name")?,
                            expanded,
                        )?;
                        let policy =
                            cowork_managed_runtime::staging::archive::TargetDescriptorPolicy::new(
                                entries,
                            )?;
                        let mut prepared = inspected.prepare(area, policy)?;
                        receipt["installation"] = serde_json::to_value(prepared.receipt()?)?;
                    }
                    #[cfg(not(target_os = "linux"))]
                    return Err(
                        "installation preparation requires the Linux management runtime".into(),
                    );
                }
            } else if args.len() > 8 {
                let role_index = match (args.len(), args[8].to_str()) {
                    (13, Some("--inspect-archive")) => 9,
                    (15, Some("--extract-archive")) => 11,
                    _ => return Err("explicit archive operation required".into()),
                };
                let role = serde_json::from_value(serde_json::Value::String(
                    args[role_index]
                        .to_str()
                        .ok_or("invalid archive role")?
                        .into(),
                ))?;
                let entries = args[role_index + 1]
                    .to_str()
                    .ok_or("invalid archive entry limit")?
                    .parse()?;
                let expanded = args[role_index + 2]
                    .to_str()
                    .ok_or("invalid archive byte limit")?
                    .parse()?;
                let paths = args[role_index + 3]
                    .to_str()
                    .ok_or("invalid archive path limit")?
                    .parse()?;
                let limits = cowork_managed_runtime::staging::archive::ArchiveLimits::new(
                    entries, expanded, paths,
                )?;
                let mut archive = staged.inspect_archive(role, limits)?;
                archive.verify()?;
                receipt["archive"] = serde_json::json!({"role": archive.role(), "entries": archive.entries().count(),
                    "expanded_bytes": archive.expanded_bytes(), "inspected":true, "extracted": false});
                if args.len() == 15 {
                    #[cfg(target_os = "linux")]
                    {
                        if !Path::new(&args[9]).is_absolute() {
                            return Err("absolute private extraction parent required".into());
                        }
                        let target = Path::new(&args[9]).canonicalize()?;
                        if target.starts_with(&package_path) {
                            return Err("extraction parent must be independent of package".into());
                        }
                        let area = cowork_managed_runtime::staging::StagingArea::create(
                            Dir::open_ambient_dir(target, ambient_authority())?,
                            args[10].to_str().ok_or("invalid extraction attempt name")?,
                            expanded,
                        )?;
                        let mut extracted = archive.extract(area)?;
                        extracted.verify()?;
                        receipt["archive"]["extracted"] = true.into();
                    }
                    #[cfg(not(target_os = "linux"))]
                    return Err("archive extraction requires the Linux management runtime".into());
                }
            }
            receipt
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        return Err("staging unsupported on this platform".into());
    } else {
        let package = verify_directory(&source, &trust, platform)?;
        serde_json::json!({"manifest_sha256":package.manifest_sha256(), "manifest":package.manifest(),
            "signature_verified":true,"payload_verified":true,"staged":false,"installed":false,"qualified":false,"execution_authority":false})
    };
    println!("{receipt}");
    Ok(())
}
fn main() {
    if let Err(error) = inspect() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
