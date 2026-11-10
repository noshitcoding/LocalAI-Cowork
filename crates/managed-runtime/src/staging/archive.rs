//! Strict bounded USTAR inspection over an original signed staged component.
//! No generic unpacking command, ambient path or serialized custody is used.
use super::*;
use std::collections::BTreeMap;
mod installation;
pub use installation::{InspectedInstallation, InstallationLimits, InstallationReceipt};
#[cfg(target_os = "linux")]
pub use installation::{
    InstalledPackage, InstalledPackagePolicy, NewRuntimeAdmission, PreparedInstallation,
    PublicationAttempt, PublicationHistory, PublicationPhase, PublicationPolicy,
    PublicationReceipt, read_publication_history,
};
#[cfg(target_os = "linux")]
mod extraction;
#[cfg(target_os = "linux")]
pub use extraction::{ExtractedArchive, TargetDescriptorPolicy};

const BLOCK: usize = 512;
const MAX_ENTRIES: usize = 200_000;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;

/// Limits are chosen by trusted installation policy, independently of the archive.
pub struct ArchiveLimits {
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
}
impl ArchiveLimits {
    pub fn new(entries: usize, expanded_bytes: u64, path_bytes: usize) -> Result<Self> {
        if entries == 0
            || entries > MAX_ENTRIES
            || expanded_bytes == 0
            || expanded_bytes > MAX_COMPONENT
            || path_bytes == 0
            || path_bytes > MAX_PATH_BYTES
        {
            return Err(Error::Archive("limits"));
        }
        Ok(Self {
            entries,
            expanded_bytes,
            path_bytes,
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArchiveEntry {
    Directory,
    File {
        bytes: u64,
        sha256: String,
        executable: bool,
    },
    Symlink {
        target: String,
    },
}
struct Member {
    entry: ArchiveEntry,
    offset: u64,
}

/// Native source custody remains owned here. An exported inventory is never a
/// substitute for this capability and cannot authorize extraction after restart.
pub struct InspectedArchive {
    staged: StagedPackage,
    component: usize,
    members: BTreeMap<String, Member>,
    expanded_bytes: u64,
}
impl InspectedArchive {
    /// Copy one validated file into the caller's quarantined output. No archive
    /// path is followed, no symlink is dereferenced and no file is executed.
    /// A failed copy must never be published by the receiving installer.
    pub fn copy_file(&mut self, path: &str, output: &mut impl Write) -> Result<()> {
        let member = self
            .members
            .get(path)
            .ok_or(Error::Archive("missing_member"))?;
        copy_member(&mut self.staged, self.component, member, output)
    }

    pub fn role(&self) -> ComponentRole {
        self.staged.manifest().components[self.component].role
    }
    pub fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }
    pub fn entries(&self) -> impl Iterator<Item = (&str, &ArchiveEntry)> {
        self.members
            .iter()
            .map(|(name, member)| (name.as_str(), &member.entry))
    }
    pub fn verify(&mut self) -> Result<()> {
        self.staged.verify()
    }
}

fn copy_member(
    staged: &mut StagedPackage,
    component: usize,
    member: &Member,
    output: &mut impl Write,
) -> Result<()> {
    let ArchiveEntry::File { bytes, sha256, .. } = &member.entry else {
        return Err(Error::Archive("member_not_file"));
    };
    staged.area.check()?;
    let pinned = &mut staged.files[component + 2];
    if native::file_identity(&pinned.file)? != pinned.id
        || native::file_identity(&native::pin_file(&staged.area.directory, &pinned.name)?)?
            != pinned.id
        || pinned.file.metadata()?.len() != pinned.bytes
    {
        return Err(Error::Staging);
    }
    pinned.file.seek(SeekFrom::Start(member.offset))?;
    let mut remaining = *bytes;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        pinned.file.read_exact(&mut buffer[..n])?;
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if hex(&hash.finalize()) != *sha256
        || native::file_identity(&pinned.file)? != pinned.id
        || pinned.file.metadata()?.len() != pinned.bytes
    {
        return Err(Error::Archive("copied_bytes"));
    }
    staged.area.check()
}

impl StagedPackage {
    pub fn inspect_archive(
        mut self,
        role: ComponentRole,
        limits: ArchiveLimits,
    ) -> Result<InspectedArchive> {
        self.verify()?;
        let component = self
            .manifest()
            .components
            .iter()
            .position(|c| c.role == role)
            .ok_or(Error::Manifest)?;
        // Fixed slots follow the verified manifest's role order. No untrusted
        // archive path is ever opened on the host during this inspection.
        let pinned = &mut self.files[component + 2];
        pinned.file.seek(SeekFrom::Start(0))?;
        let (members, expanded_bytes) = inspect(&mut pinned.file, pinned.bytes, &limits)?;
        self.verify()?;
        Ok(InspectedArchive {
            staged: self,
            component,
            members,
            expanded_bytes,
        })
    }
}

fn inspect(
    file: &mut (impl Read + Seek),
    length: u64,
    limits: &ArchiveLimits,
) -> Result<(BTreeMap<String, Member>, u64)> {
    if length < 1024 || !length.is_multiple_of(BLOCK as u64) {
        return Err(Error::Archive("block_length"));
    }
    let mut members = BTreeMap::<String, Member>::new();
    let mut expanded = 0u64;
    let mut paths = 0usize;
    let mut header = [0u8; BLOCK];
    loop {
        file.read_exact(&mut header)?;
        if header.iter().all(|b| *b == 0) {
            file.read_exact(&mut header)?;
            if header.iter().any(|b| *b != 0) {
                return Err(Error::Archive("terminator"));
            }
            // Bounded, all-zero record padding only; concatenated archives and
            // arbitrary trailers are rejected, even when the outer hash is signed.
            let tail = length
                .checked_sub(file.stream_position()?)
                .ok_or(Error::Archive("length"))?;
            if tail > 1024 * 1024 {
                return Err(Error::Archive("padding_limit"));
            }
            let mut remaining = tail;
            while remaining != 0 {
                file.read_exact(&mut header)?;
                if header.iter().any(|b| *b != 0) {
                    return Err(Error::Archive("trailing_data"));
                }
                remaining -= BLOCK as u64;
            }
            if members.is_empty() {
                return Err(Error::Archive("empty"));
            }
            return Ok((members, expanded));
        }
        if members.len() >= limits.entries {
            return Err(Error::Archive("entry_limit"));
        }
        if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
            return Err(Error::Archive("unsupported_format"));
        }
        let sum: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *b as u64
                }
            })
            .sum();
        if octal(&header[148..156])? != sum {
            return Err(Error::Archive("header_checksum"));
        }
        let mode = octal(&header[100..108])?;
        if mode > 0o777 || octal(&header[108..116])? != 0 || octal(&header[116..124])? != 0 {
            return Err(Error::Archive("ownership_or_special_mode"));
        }
        octal(&header[136..148])?;
        let bytes = octal(&header[124..136])?;
        let raw_name = text(&header[..100])?;
        let prefix = text(&header[345..500])?;
        let joined = if prefix.is_empty() {
            raw_name.to_owned()
        } else {
            format!("{prefix}/{raw_name}")
        };
        let name = if header[156] == b'5' {
            joined.strip_suffix('/').unwrap_or(&joined)
        } else {
            &joined
        };
        validate_path(name)?;
        if members.contains_key(name) {
            return Err(Error::Archive("duplicate_path"));
        }
        if let Some((parent, _)) = name.rsplit_once('/')
            && !matches!(
                members.get(parent).map(|m| &m.entry),
                Some(ArchiveEntry::Directory)
            )
        {
            return Err(Error::Archive("parent_not_directory"));
        }
        paths = paths
            .checked_add(name.len())
            .ok_or(Error::Archive("path_limit"))?;
        let link = text(&header[157..257])?;
        paths = paths
            .checked_add(link.len())
            .ok_or(Error::Archive("path_limit"))?;
        if paths > limits.path_bytes {
            return Err(Error::Archive("path_limit"));
        }
        let offset = file.stream_position()?;
        let entry = match header[156] {
            0 | b'0' if link.is_empty() => {
                expanded = expanded
                    .checked_add(bytes)
                    .ok_or(Error::Archive("expanded_limit"))?;
                if bytes > MAX_COMPONENT || expanded > limits.expanded_bytes {
                    return Err(Error::Archive("expanded_limit"));
                }
                let padded = bytes.checked_add(511).ok_or(Error::Archive("length"))? / 512 * 512;
                if offset
                    .checked_add(padded)
                    .is_none_or(|end| end > length - 1024)
                {
                    return Err(Error::Archive("member_length"));
                }
                let mut buffer = [0u8; 64 * 1024];
                let mut remaining = bytes;
                let mut hash = Sha256::new();
                while remaining != 0 {
                    let n = remaining.min(buffer.len() as u64) as usize;
                    file.read_exact(&mut buffer[..n])?;
                    hash.update(&buffer[..n]);
                    remaining -= n as u64;
                }
                let mut padding = [0u8; 511];
                let n = (padded - bytes) as usize;
                file.read_exact(&mut padding[..n])?;
                if padding[..n].iter().any(|b| *b != 0) {
                    return Err(Error::Archive("member_padding"));
                }
                ArchiveEntry::File {
                    bytes,
                    sha256: hex(&hash.finalize()),
                    executable: mode & 0o111 != 0,
                }
            }
            b'5' if bytes == 0 && link.is_empty() => ArchiveEntry::Directory,
            b'2' if bytes == 0 => {
                validate_link(name, link)?;
                ArchiveEntry::Symlink {
                    target: link.into(),
                }
            }
            // Hardlinks, devices, FIFO, sparse and PAX/GNU extension records
            // require separate supported semantics, never generic fallback.
            _ => return Err(Error::Archive("unsupported_member")),
        };
        members.insert(name.into(), Member { entry, offset });
    }
}

fn text(bytes: &[u8]) -> Result<&str> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|b| *b != 0) {
        return Err(Error::Archive("hidden_text"));
    }
    std::str::from_utf8(&bytes[..end]).map_err(|_| Error::Archive("utf8"))
}
fn octal(bytes: &[u8]) -> Result<u64> {
    let start = bytes
        .iter()
        .position(|b| *b != 0 && *b != b' ')
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| *b != 0 && *b != b' ')
        .map_or(start, |n| n + 1);
    if start == end {
        return Ok(0);
    }
    bytes[start..end].iter().try_fold(0u64, |n, byte| {
        if !(b'0'..=b'7').contains(byte) {
            return Err(Error::Archive("numeric_encoding"));
        }
        n.checked_mul(8)
            .and_then(|n| n.checked_add(u64::from(byte - b'0')))
            .ok_or(Error::Archive("numeric_overflow"))
    })
}
fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 256
        || path.contains(['\\', '\0', ':'])
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.bytes().any(|b| b < 32 || b == 127))
    {
        return Err(Error::Archive("unsafe_path"));
    }
    Ok(())
}
fn validate_link(path: &str, target: &str) -> Result<()> {
    if target.is_empty()
        || target.contains(['\\', '\0', ':'])
        || target.bytes().any(|b| b < 32 || b == 127)
    {
        return Err(Error::Archive("unsafe_link"));
    }
    let mut depth = if target.starts_with('/') {
        0
    } else {
        path.split('/').count() - 1
    };
    for component in target.split('/') {
        match component {
            "" | "." => (),
            ".." => {
                depth = depth.checked_sub(1).ok_or(Error::Archive("link_escape"))?;
            }
            _ => depth += 1,
        }
    }
    Ok(())
}
