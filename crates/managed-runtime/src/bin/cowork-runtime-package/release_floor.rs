//! Explicit local UID-0 release-policy management; never selects an executable.
use super::*;
use cowork_managed_runtime::staging::release_floor::ReleaseFloor;
use std::ffi::OsString;

fn platform(value: &OsString) -> Result<Platform, Box<dyn std::error::Error>> {
    match value.to_str() {
        Some("linux_x86_64") => Ok(Platform::LinuxX86_64),
        Some("windows_wsl2_x86_64") => Ok(Platform::WindowsWsl2X86_64),
        _ => Err("unsupported runtime package platform".into()),
    }
}

pub(super) fn management(
    args: &[OsString],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if args.len() != 6 {
        return Err("usage: cowork-runtime-package --initialize-release-floor|--release-floor-status ABS_PRIVATE_PARENT NAME ABS_TRUSTED_KEY KEY_ID PLATFORM".into());
    }
    if !Path::new(&args[1]).is_absolute() || !Path::new(&args[3]).is_absolute() {
        return Err("absolute private parent and independent key required".into());
    }
    let parent = Path::new(&args[1]).canonicalize()?;
    let key = Path::new(&args[3]).canonicalize()?;
    let name = args[2].to_str().ok_or("invalid release policy name")?;
    // Validate one leaf before joining; native initialization/open validates too.
    if name.is_empty()
        || name.len() > 96
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || key.starts_with(parent.join(name))
    {
        return Err("independent trust and one policy name required".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(key)?.take(33).read_to_end(&mut bytes)?;
    let trust = TrustRoot::new(
        args[4].to_str().ok_or("invalid key ID")?.into(),
        bytes
            .try_into()
            .map_err(|_| "expected raw 32-byte Ed25519 public key")?,
    )?;
    let parent = Dir::open_ambient_dir(parent, ambient_authority())?;
    let policy = if args[0] == "--initialize-release-floor" {
        ReleaseFloor::initialize(parent, name, &trust, platform(&args[5])?)?
    } else if args[0] == "--release-floor-status" {
        ReleaseFloor::open(parent, name, &trust, platform(&args[5])?)?
    } else {
        return Err("explicit release policy operation required".into());
    };
    Ok(serde_json::to_value(policy.receipt()?)?)
}

pub(super) fn candidate(
    args: &[OsString],
    source: &Dir,
    trust: &TrustRoot,
    platform: Platform,
    package: &Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if args.len() != 7 || !Path::new(&args[5]).is_absolute() {
        return Err("usage: cowork-runtime-package ABS_PACKAGE ABS_TRUSTED_KEY KEY_ID PLATFORM --check-release-floor|--advance-release-floor ABS_PRIVATE_POLICY_PARENT NAME".into());
    }
    let parent = Path::new(&args[5]).canonicalize()?;
    if parent.starts_with(package) {
        return Err("policy must be independent of package".into());
    }
    let package = verify_directory(source, trust, platform)?;
    let mut policy = ReleaseFloor::open(
        Dir::open_ambient_dir(parent, ambient_authority())?,
        args[6].to_str().ok_or("invalid policy name")?,
        trust,
        platform,
    )?;
    match args[4].to_str() {
        Some("--check-release-floor") => policy.check_candidate(&package)?,
        Some("--advance-release-floor") => policy.advance(&package)?,
        _ => return Err("explicit release policy operation required".into()),
    }
    Ok(
        serde_json::json!({"floor": policy.receipt()?, "candidate_manifest_sha256": package.manifest_sha256(),
        "candidate_release_sequence": package.manifest().release_sequence, "signature_verified":true,
        "payload_verified":true, "installed":false, "activated":false, "qualified":false, "execution_authority":false}),
    )
}
