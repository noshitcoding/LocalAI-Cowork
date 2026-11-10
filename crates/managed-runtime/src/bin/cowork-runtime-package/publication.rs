use super::*;
use cowork_managed_runtime::staging::{
    StagingArea,
    archive::{InstallationLimits, PublicationAttempt, PublicationPolicy, TargetDescriptorPolicy},
};

pub(super) fn history(
    args: &[std::ffi::OsString],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if args.len() != 5 {
        return Err("usage: cowork-runtime-package --publication-status ABS_ORIGINAL_JOURNAL ABS_TRUSTED_KEY KEY_ID PLATFORM".into());
    }
    if !Path::new(&args[1]).is_absolute() || !Path::new(&args[2]).is_absolute() {
        return Err("absolute original journal and independent trust key required".into());
    }
    let directory = Path::new(&args[1]).canonicalize()?;
    let key = Path::new(&args[2]).canonicalize()?;
    if key.starts_with(&directory) {
        return Err("trust key cannot come from the journal".into());
    }
    let mut public_key = Vec::new();
    std::fs::File::open(key)?
        .take(33)
        .read_to_end(&mut public_key)?;
    let trust = TrustRoot::new(
        args[3].to_str().ok_or("invalid key ID")?.into(),
        public_key
            .try_into()
            .map_err(|_| "expected raw 32-byte Ed25519 public key")?,
    )?;
    let platform = match args[4].to_str() {
        Some("linux_x86_64") => Platform::LinuxX86_64,
        Some("windows_wsl2_x86_64") => Platform::WindowsWsl2X86_64,
        _ => return Err("unsupported runtime package platform".into()),
    };
    let history = cowork_managed_runtime::staging::archive::read_publication_history(
        Dir::open_ambient_dir(directory, ambient_authority())?,
        &trust,
        platform,
    )?;
    Ok(serde_json::to_value(history)?)
}

pub(super) fn run(
    args: &[std::ffi::OsString],
    source: &Dir,
    trust: &TrustRoot,
    platform: Platform,
    package: &Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if args.len() != 13 {
        return Err("usage: cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM --publish-installation ABS_ATTEMPT_PARENT ATTEMPT_NAME ABS_VERSIONS_PARENT TOTAL_PAYLOAD_BYTES INPUT_BYTES MAX_ENTRIES MAX_EXPANDED_BYTES MAX_PATH_BYTES".into());
    }
    let private_parent = |index| -> Result<Dir, Box<dyn std::error::Error>> {
        let path = Path::new(&args[index]);
        if !path.is_absolute() {
            return Err("absolute independent private parent required".into());
        }
        let path = path.canonicalize()?;
        if path.starts_with(package) {
            return Err("publication parent must be independent of package".into());
        }
        Ok(Dir::open_ambient_dir(path, ambient_authority())?)
    };
    let parent = private_parent(5)?;
    let versions = private_parent(7)?;
    let total: u64 = args[8]
        .to_str()
        .ok_or("invalid total payload budget")?
        .parse()?;
    let input: u64 = args[9]
        .to_str()
        .ok_or("invalid input payload budget")?
        .parse()?;
    let entries = args[10]
        .to_str()
        .ok_or("invalid publication entry limit")?
        .parse()?;
    let expanded = args[11]
        .to_str()
        .ok_or("invalid publication expanded limit")?
        .parse()?;
    let paths = args[12]
        .to_str()
        .ok_or("invalid publication path limit")?
        .parse()?;
    let policy = PublicationPolicy::new(
        input,
        InstallationLimits::new(entries, expanded, paths)?,
        TargetDescriptorPolicy::new(entries)?,
    )?;
    let area = StagingArea::create(
        parent,
        args[6].to_str().ok_or("invalid publication attempt name")?,
        total,
    )?;
    let mut attempt = PublicationAttempt::prepare(area, source, trust, platform, policy)?;
    attempt.publish(versions)?;
    Ok(serde_json::to_value(attempt.receipt()?)?)
}
