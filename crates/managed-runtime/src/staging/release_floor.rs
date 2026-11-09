//! Durable, restrictive release policy. This is neither an activation permit nor
//! recovery of a previous native operation. Only trusted UID-0 management uses it.
//! Every opened policy is new current custody of the policy files, never of any
//! published runtime, task, volume, qualification or old publication attempt.
use super::*;
use cap_std::fs::{FileExt, MetadataExt, OpenOptionsExt};
use serde::Deserialize;
use std::os::fd::AsRawFd;
#[cfg(test)]
#[path = "release_floor/anchor_tests.rs"]
mod anchor_tests;

const POLICY_FORMAT_VERSION: u32 = 2;
const SLOT_BYTES: usize = 256 * 1024;
const HEADER: usize = 36;
const FILES: [&str; 3] = ["owner.lock", "floor.a", "floor.b"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalId {
    device: u64,
    inode: u64,
}
impl LocalId {
    fn directory(dir: &Dir) -> Result<Self> {
        let m = dir.dir_metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
        })
    }
    fn file(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Binding {
    platform: Platform,
    directory: LocalId,
    parent: LocalId,
    name: String,
    owner: LocalId,
    slots: [LocalId; 2],
    trust_key_id: String,
    trust_key_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Record {
    format_version: u32,
    binding: Binding,
    revision: u64,
    release_sequence: u64,
    manifest_sha256: Option<String>,
    manifest: Option<String>,
    signature: Option<String>,
}
impl Record {
    fn validate(&self, trust: &TrustRoot, platform: Platform) -> Result<()> {
        if self.format_version != POLICY_FORMAT_VERSION
            || !token(&self.binding.name)
            || self.binding.platform != platform
            || self.binding.trust_key_id != trust.id
            || self.binding.trust_key_sha256 != sha256(&trust.public_key)
            || self.revision > i64::MAX as u64
        {
            return Err(Error::Archive("release_floor_binding"));
        }
        if self.revision == 0 {
            if self.release_sequence != 0
                || self.manifest_sha256.is_some()
                || self.manifest.is_some()
                || self.signature.is_some()
            {
                return Err(Error::Archive("release_floor_initial_state"));
            }
        } else {
            let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
            let signature = self.signature.as_ref().ok_or(Error::Signature)?;
            let verified =
                verify_manifest(manifest.as_bytes(), signature.as_bytes(), trust, platform)?;
            if self.release_sequence != verified.manifest().release_sequence
                || self.manifest_sha256.as_deref() != Some(verified.manifest_sha256())
            {
                return Err(Error::Archive("release_floor_signed_sequence"));
            }
        }
        Ok(())
    }
}

/// No Clone/Deserialize or public constructor from a receipt. Opening only
/// creates current policy custody after a complete independent state check.
pub struct ReleaseFloor {
    area: StagingArea,
    owner: File,
    slots: [File; 2],
    current: Record,
    pending: Option<Record>,
    io_failed: bool,
    trust: TrustRoot,
}

/// Live local policy identity only; independent read handles retain the original
/// inodes without duplicating the writer's locked open-file description. No
/// serialized record, path or matching contents can manufacture this anchor.
pub(crate) struct FloorAnchor {
    original: Binding,
    minimum_sequence: u64,
    minimum_manifest_sha256: Option<String>,
    parent: Dir,
    directory: Dir,
    name: String,
    owner: File,
    slots: [File; 2],
    trust: TrustRoot,
    platform: Platform,
}

impl FloorAnchor {
    fn verify(&self) -> Result<()> {
        root()?;
        native::private_directory(&self.parent)?;
        native::private_directory(&self.directory)?;
        if self.name != self.original.name
            || LocalId::directory(&self.parent)? != self.original.parent
            || LocalId::directory(&self.directory)? != self.original.directory
            || LocalId::directory(&self.parent.open_dir_nofollow(&self.name)?)?
                != self.original.directory
        {
            return Err(Error::Archive("release_floor_anchor_replaced"));
        }
        exact_entries(&self.directory, &FILES)?;
        for (name, original, expected, bytes) in [
            (FILES[0], &self.owner, &self.original.owner, 0),
            (
                FILES[1],
                &self.slots[0],
                &self.original.slots[0],
                SLOT_BYTES as u64,
            ),
            (
                FILES[2],
                &self.slots[1],
                &self.original.slots[1],
                SLOT_BYTES as u64,
            ),
        ] {
            native::file_identity(original)?;
            native::check_allocation(original, bytes)?;
            if LocalId::file(original)? != *expected
                || LocalId::file(&native::pin_file(&self.directory, name)?)? != *expected
            {
                return Err(Error::Archive("release_floor_anchor_replaced"));
            }
        }
        Ok(())
    }

    fn require_original_policy(&self, policy: &ReleaseFloor) -> Result<()> {
        // Compare the actual newly opened objects, not just their current names.
        // An ABA name restoration cannot turn a B owner lock into A's lock.
        if policy.current.binding != self.original
            || policy.area.name != self.original.name
            || LocalId::directory(&policy.area.parent)? != self.original.parent
            || LocalId::directory(&policy.area.directory)? != self.original.directory
            || LocalId::file(&policy.owner)? != self.original.owner
            || LocalId::file(&policy.slots[0])? != self.original.slots[0]
            || LocalId::file(&policy.slots[1])? != self.original.slots[1]
        {
            return Err(Error::Archive("release_floor_anchor_policy_changed"));
        }
        policy.verify_current()?;
        if policy.current.release_sequence < self.minimum_sequence
            || policy.current.release_sequence == self.minimum_sequence
                && policy.current.manifest_sha256 != self.minimum_manifest_sha256
        {
            return Err(Error::Archive("release_floor_anchor_minimum_changed"));
        }
        self.verify()
    }

    /// Keep the returned exclusive current-policy guard alive until the new
    /// dispatch boundary. Existing work and history do not call this method.
    pub(crate) fn admit(&self, package: &VerifiedPackage) -> Result<ReleaseFloor> {
        self.verify()?;
        #[cfg(test)]
        anchor_tests::fire("admit_before_open");
        let policy = ReleaseFloor::open(
            self.parent.try_clone()?,
            &self.name,
            &self.trust,
            self.platform,
        )?;
        policy.check_candidate(package)?;
        #[cfg(test)]
        anchor_tests::fire("admit_after_check");
        self.require_original_policy(&policy)?;
        Ok(policy)
    }
}

#[derive(Serialize)]
pub struct ReleaseFloorReceipt<'a> {
    policy_format_version: u32,
    platform: Platform,
    revision: u64,
    release_sequence: u64,
    manifest_sha256: Option<&'a str>,
    restrictive_policy_only: bool,
    installed: bool,
    activated: bool,
    qualified: bool,
    execution_authority: bool,
    recovered_native_authority: bool,
}

fn root() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(Error::Archive("release_floor_requires_management_root"));
    }
    Ok(())
}

fn lock(file: &File) -> Result<()> {
    // SAFETY: newly opened/pinned own management descriptor, no foreign unlock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error::Archive("release_floor_owner_busy"));
    }
    Ok(())
}

fn open_writable(dir: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = dir.open_with(name, &options)?;
    native::file_identity(&file)?;
    Ok(file)
}

fn frame(record: &Record) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(record)?;
    if payload.is_empty() || payload.len() > SLOT_BYTES - HEADER {
        return Err(Error::Archive("release_floor_frame_limit"));
    }
    let mut bytes = vec![0; SLOT_BYTES];
    bytes[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes[4..36].copy_from_slice(&Sha256::digest(&payload));
    bytes[36..36 + payload.len()].copy_from_slice(&payload);
    Ok(bytes)
}

fn read_frame(file: &File) -> Result<Record> {
    native::check_allocation(file, SLOT_BYTES as u64)?;
    let mut bytes = vec![0; SLOT_BYTES];
    // Independent positional reads also keep concurrent policy queries from
    // sharing a seek cursor on the retained original descriptor.
    let mut offset = 0;
    while offset < bytes.len() {
        match file.read_at(&mut bytes[offset..], offset as u64) {
            Ok(0) => return Err(Error::Archive("release_floor_short_frame")),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let length = u32::from_le_bytes(bytes[..4].try_into().map_err(|_| Error::Staging)?) as usize;
    if length == 0
        || length > SLOT_BYTES - HEADER
        || bytes[36 + length..].iter().any(|b| *b != 0)
        || Sha256::digest(&bytes[36..36 + length])[..] != bytes[4..36]
    {
        return Err(Error::Archive("release_floor_unconfirmed_frame"));
    }
    Ok(serde_json::from_slice(&bytes[36..36 + length])?)
}

impl ReleaseFloor {
    /// Create a long-lived native identity anchor without retaining this
    /// owner's exclusive lock. Each future new start still needs fresh admit.
    pub(crate) fn anchor(&self) -> Result<FloorAnchor> {
        self.verify_current()?;
        #[cfg(test)]
        anchor_tests::fire("anchor_before_pins");
        let value = FloorAnchor {
            original: self.current.binding.clone(),
            minimum_sequence: self.current.release_sequence,
            minimum_manifest_sha256: self.current.manifest_sha256.clone(),
            parent: self.area.parent.try_clone()?,
            directory: self.area.directory.try_clone()?,
            name: self.area.name.clone(),
            owner: native::pin_file(&self.area.directory, FILES[0])?,
            slots: [
                native::pin_file(&self.area.directory, FILES[1])?,
                native::pin_file(&self.area.directory, FILES[2])?,
            ],
            trust: TrustRoot::new(self.trust.id.clone(), self.trust.public_key)?,
            platform: self.current.binding.platform,
        };
        value.verify()?;
        self.verify_current()?;
        value.require_original_policy(self)?;
        Ok(value)
    }

    /// Explicit fresh setup. An existing name is never replaced, and Open never
    /// calls this on missing/corrupt state. Metadata/blocks require a bounded
    /// management filesystem independently of this 512-KiB payload reservation.
    pub fn initialize(
        parent: Dir,
        name: &str,
        trust: &TrustRoot,
        platform: Platform,
    ) -> Result<Self> {
        root()?;
        let area = StagingArea::create(parent, name, (2 * SLOT_BYTES) as u64)?;
        let owner = native::create_file(&area.directory, FILES[0])?;
        owner.sync_all()?;
        lock(&owner)?;
        let slots = [
            native::create_file(&area.directory, FILES[1])?,
            native::create_file(&area.directory, FILES[2])?,
        ];
        for file in &slots {
            native::allocate(file, SLOT_BYTES as u64)?;
        }
        let binding = Binding {
            platform,
            directory: LocalId::directory(&area.directory)?,
            parent: LocalId::directory(&area.parent)?,
            name: area.name.clone(),
            owner: LocalId::file(&owner)?,
            slots: [LocalId::file(&slots[0])?, LocalId::file(&slots[1])?],
            trust_key_id: trust.id.clone(),
            trust_key_sha256: sha256(&trust.public_key),
        };
        let current = Record {
            format_version: POLICY_FORMAT_VERSION,
            binding,
            revision: 0,
            release_sequence: 0,
            manifest_sha256: None,
            manifest: None,
            signature: None,
        };
        let bytes = frame(&current)?;
        for mut file in &slots {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        native::sync_directory(&area.directory)?;
        native::sync_directory(&area.parent)?;
        let value = Self {
            area,
            owner,
            slots,
            current,
            pending: None,
            io_failed: false,
            trust: TrustRoot::new(trust.id.clone(), trust.public_key)?,
        };
        value.verify_current()?;
        Ok(value)
    }

    /// Fail closed on any missing/torn slot. Unlike diagnostic history, a lower
    /// valid frame may never be selected while a possibly newer frame is invalid.
    pub fn open(parent: Dir, name: &str, trust: &TrustRoot, platform: Platform) -> Result<Self> {
        root()?;
        if !token(name) {
            return Err(Error::Staging);
        }
        native::private_directory(&parent)?;
        let parent_id = native::directory_identity(&parent)?;
        let directory = parent.open_dir_nofollow(name)?;
        native::private_directory(&directory)?;
        let directory_id = native::directory_identity(&directory)?;
        exact_entries(&directory, &FILES)?;
        let owner = open_writable(&directory, FILES[0])?;
        lock(&owner)?;
        let slots = [
            open_writable(&directory, FILES[1])?,
            open_writable(&directory, FILES[2])?,
        ];
        let current = Self::latest(&slots, trust, platform)?;
        let value = Self {
            area: StagingArea {
                parent,
                directory,
                name: name.into(),
                parent_id,
                directory_id,
                payload_limit: (2 * SLOT_BYTES) as u64,
            },
            owner,
            slots,
            current,
            pending: None,
            io_failed: false,
            trust: TrustRoot::new(trust.id.clone(), trust.public_key)?,
        };
        value.verify_current()?;
        Ok(value)
    }

    fn latest(slots: &[File; 2], trust: &TrustRoot, platform: Platform) -> Result<Record> {
        let records = [read_frame(&slots[0])?, read_frame(&slots[1])?];
        for (i, record) in records.iter().enumerate() {
            record.validate(trust, platform)?;
            if record.revision != 0 && (record.revision % 2) as usize != i {
                return Err(Error::Archive("release_floor_slot_sequence"));
            }
        }
        let [a, b] = records;
        if a.binding != b.binding {
            return Err(Error::Archive("release_floor_cross_binding"));
        }
        if a.revision == 0 && a == b {
            return Ok(a);
        }
        let (old, new) = if a.revision < b.revision {
            (a, b)
        } else {
            (b, a)
        };
        if old.revision.checked_add(1) != Some(new.revision)
            || old.release_sequence >= new.release_sequence
        {
            return Err(Error::Archive("release_floor_monotonicity"));
        }
        Ok(new)
    }

    fn verify_original(&self) -> Result<()> {
        root()?;
        self.area.check()?;
        exact_entries(&self.area.directory, &FILES)?;
        let b = &self.current.binding;
        if b.name != self.area.name {
            return Err(Error::Archive("release_floor_namespace_changed"));
        }
        if LocalId::directory(&self.area.directory)? != b.directory
            || LocalId::directory(&self.area.parent)? != b.parent
            || LocalId::file(&self.owner)? != b.owner
            || LocalId::file(&native::pin_file(&self.area.directory, FILES[0])?)? != b.owner
        {
            return Err(Error::Archive("release_floor_original_changed"));
        }
        native::check_allocation(&self.owner, 0)?;
        native::file_identity(&self.owner)?;
        lock(&self.owner)?;
        for (i, file) in self.slots.iter().enumerate() {
            native::check_allocation(file, SLOT_BYTES as u64)?;
            native::file_identity(file)?;
            if LocalId::file(file)? != b.slots[i]
                || LocalId::file(&native::pin_file(&self.area.directory, FILES[i + 1])?)?
                    != b.slots[i]
            {
                return Err(Error::Archive("release_floor_original_changed"));
            }
        }
        self.area.check()
    }

    fn verify_current(&self) -> Result<()> {
        if self.io_failed || self.pending.is_some() {
            return Err(Error::Archive("release_floor_original_io_failed"));
        }
        self.verify_original()?;
        if Self::latest(&self.slots, &self.trust, self.current.binding.platform)? != self.current {
            return Err(Error::Archive("release_floor_current_changed"));
        }
        self.verify_original()
    }

    fn verify_package(&self, package: &VerifiedPackage) -> Result<VerifiedPackage> {
        let verified = verify_manifest(
            &package.signed_manifest,
            &package.signed_signature,
            &self.trust,
            self.current.binding.platform,
        )?;
        if verified.manifest_sha256() != package.manifest_sha256() {
            return Err(Error::Signature);
        }
        Ok(verified)
    }

    /// Restrictive check only. Equality requires the exact signed manifest;
    /// a different artifact may not reuse the highest sequence number.
    pub fn check_candidate(&self, package: &VerifiedPackage) -> Result<()> {
        self.verify_current()?;
        let package = self.verify_package(package)?;
        if package.manifest().release_sequence < self.current.release_sequence
            || package.manifest().release_sequence == self.current.release_sequence
                && self.current.manifest_sha256.as_deref() != Some(package.manifest_sha256())
        {
            return Err(Error::Archive("release_floor_downgrade_or_sequence_reuse"));
        }
        self.verify_current()
    }

    /// An explicit management action may only tighten policy using a signed
    /// manifest. This never marks its package installed/qualified/active. The
    /// installer must separately require current physical qualification and the
    /// explicit operation grant before selecting any executable version.
    pub fn advance(&mut self, package: &VerifiedPackage) -> Result<()> {
        let package = self.verify_package(package)?;
        let desired = if let Some(pending) = &self.pending {
            if pending.manifest_sha256.as_deref() != Some(package.manifest_sha256()) {
                return Err(Error::Archive("release_floor_pending_changed"));
            }
            pending.clone()
        } else {
            self.check_candidate(&package)?;
            if package.manifest().release_sequence == self.current.release_sequence {
                return Ok(());
            }
            Record {
                format_version: POLICY_FORMAT_VERSION,
                binding: self.current.binding.clone(),
                revision: self
                    .current
                    .revision
                    .checked_add(1)
                    .filter(|r| *r <= i64::MAX as u64)
                    .ok_or(Error::Staging)?,
                release_sequence: package.manifest().release_sequence,
                manifest_sha256: Some(package.manifest_sha256().into()),
                manifest: Some(
                    String::from_utf8(package.signed_manifest).map_err(|_| Error::Manifest)?,
                ),
                signature: Some(
                    String::from_utf8(package.signed_signature).map_err(|_| Error::Signature)?,
                ),
            }
        };
        self.pending = Some(desired.clone());
        let result = (|| {
            self.verify_original()?;
            let bytes = frame(&desired)?;
            let mut file = &self.slots[(desired.revision % 2) as usize];
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            self.verify_original()?;
            if Self::latest(&self.slots, &self.trust, desired.binding.platform)? != desired {
                return Err(Error::Archive("release_floor_readback"));
            }
            self.verify_original()
        })();
        match result {
            Ok(()) => {
                self.current = desired;
                self.pending = None;
                Ok(())
            }
            Err(error) => {
                self.io_failed = true;
                Err(error)
            }
        }
    }

    pub fn receipt(&self) -> Result<ReleaseFloorReceipt<'_>> {
        self.verify_current()?;
        Ok(ReleaseFloorReceipt {
            policy_format_version: POLICY_FORMAT_VERSION,
            platform: self.current.binding.platform,
            revision: self.current.revision,
            release_sequence: self.current.release_sequence,
            manifest_sha256: self.current.manifest_sha256.as_deref(),
            restrictive_policy_only: true,
            installed: false,
            activated: false,
            qualified: false,
            execution_authority: false,
            recovered_native_authority: false,
        })
    }
}
