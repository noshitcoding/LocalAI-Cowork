//! Shared runtime package verification. Package integrity is neither installation
//! nor physical qualification and never grants workload execution authority.
#[cfg(all(target_os = "linux", feature = "containerd"))]
pub mod containerd;
#[cfg(any(target_os = "linux", windows))]
pub mod staging;
#[cfg(target_os = "linux")]
pub mod volumes;
use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read};

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_COMPONENT: u64 = 12 * 1024 * 1024 * 1024;
const MAX_PACKAGE: u64 = 32 * 1024 * 1024 * 1024;
pub const SIGNING_DOMAIN: &[u8] = b"localai-cowork/runtime-package/v1\0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("runtime_package_signature_invalid")]
    Signature,
    #[error("runtime_package_manifest_invalid")]
    Manifest,
    #[error("runtime_package_platform_mismatch")]
    Platform,
    #[error("runtime_package_payload_invalid")]
    Payload,
    #[error("runtime_package_staging_boundary_invalid")]
    Staging,
    #[error("runtime_package_archive_invalid: {0}")]
    Archive(&'static str),
    #[error("runtime_package_io_failed")]
    Io(#[from] std::io::Error),
    #[error("runtime_package_encoding_invalid")]
    Encoding(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    LinuxX86_64,
    WindowsWsl2X86_64,
}

/// Declaration order is the required manifest order, independent of paths.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ComponentRole {
    BaseRootfs,
    BrowserTools,
    ContainerdBundle,
    DocumentTools,
    GitTools,
    GvisorBundle,
    NodeTools,
    PythonTools,
    RuntimeService,
}
const ROLES: [ComponentRole; 9] = [
    ComponentRole::BaseRootfs,
    ComponentRole::BrowserTools,
    ComponentRole::ContainerdBundle,
    ComponentRole::DocumentTools,
    ComponentRole::GitTools,
    ComponentRole::GvisorBundle,
    ComponentRole::NodeTools,
    ComponentRole::PythonTools,
    ComponentRole::RuntimeService,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageComponent {
    pub role: ComponentRole,
    pub filename: String,
    pub version: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub format_version: u32,
    pub platform: Platform,
    pub version: String,
    /// Installer anti-rollback state must retain this signed release sequence.
    /// Merely verifying an older package does not authorize activating it.
    pub release_sequence: u64,
    pub components: Vec<PackageComponent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub key_id: String,
    pub manifest_sha256: String,
    /// Detached Ed25519 signature over SIGNING_DOMAIN followed by exact bytes.
    pub signature: Vec<u8>,
}

/// Supplied by trusted application/operator configuration, never by the bundle.
pub struct TrustRoot {
    id: String,
    public_key: [u8; 32],
}
impl TrustRoot {
    pub fn new(id: String, public_key: [u8; 32]) -> Result<Self> {
        if !token(&id) {
            return Err(Error::Manifest);
        }
        Ok(Self { id, public_key })
    }
}

/// Only the exact signed manifest is admitted here. No public constructor can
/// turn a manifest struct or an unverified digest into this capability.
pub struct VerifiedPackage {
    #[cfg(target_os = "linux")]
    signed_manifest: Vec<u8>,
    #[cfg(target_os = "linux")]
    signed_signature: Vec<u8>,
    manifest: PackageManifest,
    manifest_sha256: String,
}
impl VerifiedPackage {
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Verifies a single stream with a fixed memory bound. A future installer
    /// must copy into quarantine while hashing, then publish only after *all*
    /// payloads verify. Reading this receipt never admits execution or unpacking.
    pub fn verify_component(&self, role: ComponentRole, reader: &mut impl Read) -> Result<()> {
        let item = self
            .manifest
            .components
            .iter()
            .find(|c| c.role == role)
            .ok_or(Error::Manifest)?;
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let allowed = (item.bytes - count + 1).min(buffer.len() as u64) as usize;
            let n = reader.read(&mut buffer[..allowed])?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > item.bytes {
                return Err(Error::Payload);
            }
            hash.update(&buffer[..n]);
        }
        if count != item.bytes || hex(&hash.finalize()) != item.sha256 {
            return Err(Error::Payload);
        }
        Ok(())
    }
}

pub fn verify_manifest(
    bytes: &[u8],
    signature: &[u8],
    trust: &TrustRoot,
    platform: Platform,
) -> Result<VerifiedPackage> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_MANIFEST || signature.len() > 2048 {
        return Err(Error::Manifest);
    }
    #[cfg(target_os = "linux")]
    let signed_signature = signature.to_vec();
    let signature: PackageSignature = serde_json::from_slice(signature)?;
    let digest = sha256(bytes);
    if signature.key_id != trust.id
        || signature.manifest_sha256 != digest
        || signature.signature.len() != 64
    {
        return Err(Error::Signature);
    }
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + bytes.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(bytes);
    UnparsedPublicKey::new(&ED25519, trust.public_key)
        .verify(&message, &signature.signature)
        .map_err(|_| Error::Signature)?;
    let manifest: PackageManifest = serde_json::from_slice(bytes)?;
    validate(&manifest)?;
    if manifest.platform != platform {
        return Err(Error::Platform);
    }
    Ok(VerifiedPackage {
        #[cfg(target_os = "linux")]
        signed_manifest: bytes.to_vec(),
        #[cfg(target_os = "linux")]
        signed_signature,
        manifest,
        manifest_sha256: digest,
    })
}

/// This is a read-only package check. No file from this directory is executed,
/// extracted, installed, qualified or selected as the active runtime version.
pub fn verify_directory(
    package: &Dir,
    trust: &TrustRoot,
    platform: Platform,
) -> Result<VerifiedPackage> {
    exact_entries(package, &["manifest.json", "manifest.sig.json", "payload"])?;
    let bytes = read_small(package, "manifest.json", MAX_MANIFEST)?;
    let signature = read_small(package, "manifest.sig.json", 2048)?;
    let verified = verify_manifest(&bytes, &signature, trust, platform)?;
    let payload = package.open_dir_nofollow("payload")?;
    let names: Vec<_> = verified
        .manifest
        .components
        .iter()
        .map(|c| c.filename.as_str())
        .collect();
    exact_entries(&payload, &names)?;
    for component in &verified.manifest.components {
        let mut file = open_regular(&payload, &component.filename, component.bytes)?;
        let before = file.metadata()?;
        if before.len() != component.bytes {
            return Err(Error::Payload);
        }
        verified.verify_component(component.role, &mut file)?;
        let after = file.metadata()?;
        if after.len() != before.len()
            || after.modified()? != before.modified()?
            || after.nlink() != 1
        {
            return Err(Error::Payload);
        }
    }
    Ok(verified)
}

fn validate(manifest: &PackageManifest) -> Result<()> {
    if manifest.format_version != 1
        || !token(&manifest.version)
        || manifest.release_sequence == 0
        || manifest.release_sequence > i64::MAX as u64
        || manifest.components.len() != ROLES.len()
    {
        return Err(Error::Manifest);
    }
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for (component, expected) in manifest.components.iter().zip(ROLES) {
        if component.role != expected
            || !token(&component.filename)
            || !token(&component.version)
            || component.bytes == 0
            || component.bytes > MAX_COMPONENT
            || !valid_hash(&component.sha256)
            || !names.insert(component.filename.to_ascii_lowercase())
        {
            return Err(Error::Manifest);
        }
        total = total.checked_add(component.bytes).ok_or(Error::Manifest)?;
    }
    if total > MAX_PACKAGE {
        return Err(Error::Manifest);
    }
    Ok(())
}
fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value != "."
        && value != ".."
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !matches!(
            value
                .split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn open_regular(dir: &Dir, name: &str, maximum: u64) -> Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > maximum {
        return Err(Error::Payload);
    }
    Ok(file)
}
fn read_small(dir: &Dir, name: &str, max: u64) -> Result<Vec<u8>> {
    let mut file = open_regular(dir, name, max)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    file.by_ref().take(max + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.nlink() != 1
        || after.modified()? != before.modified()?
    {
        return Err(Error::Payload);
    }
    Ok(bytes)
}
fn exact_entries(dir: &Dir, expected: &[&str]) -> Result<()> {
    let expected: BTreeSet<_> = expected.iter().map(std::ffi::OsString::from).collect();
    let mut names = BTreeSet::new();
    // Untrusted directories may contain millions of extra members. Reject
    // immediately instead of collecting an unbounded directory inventory.
    for entry in dir.entries()? {
        let name = entry?.file_name();
        if !expected.contains(&name) || !names.insert(name) {
            return Err(Error::Payload);
        }
    }
    if names != expected {
        return Err(Error::Payload);
    }
    Ok(())
}
